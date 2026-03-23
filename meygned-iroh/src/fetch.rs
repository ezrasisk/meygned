use iroh_blobs::Hash;
use iroh_docs::DocTicket;
use tracing::{debug, warn};

use meygned_core::ContentRef;

use crate::{error::IrohError, node::IrohNode};

/// Default path key used when no specific path is requested.
/// Mirrors the web convention: "/" → index page.
pub const DEFAULT_PATH_KEY: &str = "/";

/// Fetch content identified by a [`ContentRef`] from the Iroh p2p network.
///
/// This is the main entry point for `meygned-iroh`. It implements the
/// three-tier routing priority for `Doc` refs:
///
/// 1. `ticket` present  → import namespace directly (fastest, no discovery)
/// 2. `node_id` present → dial node directly via relay or DHT
/// 3. namespace only    → check local store, error if not found
pub struct IrohFetcher<'a> {
    node: &'a IrohNode,
}

impl<'a> IrohFetcher<'a> {
    pub fn new(node: &'a IrohNode) -> Self {
        Self { node }
    }

    /// Fetch the content at `path` (e.g. `"/"`, `"/style.css"`) for the
    /// given [`ContentRef`]. Returns raw bytes.
    ///
    /// For `Blob` refs, `path` is ignored — the hash identifies the content
    /// exactly. For `Doc` refs, `path` is used as the doc key.
    pub async fn fetch(&self, content_ref: &ContentRef, path: &str) -> Result<Vec<u8>, IrohError> {
        match content_ref {
            ContentRef::Blob { hash } => self.fetch_blob(hash).await,
            ContentRef::Doc {
                namespace_id,
                ticket,
                node_id,
                relay_url,
            } => {
                self.fetch_doc(namespace_id, ticket.as_deref(), node_id.as_deref(), relay_url.as_deref(), path)
                    .await
            }
        }
    }

    // -----------------------------------------------------------------------
    // Blob path
    // -----------------------------------------------------------------------

    /// Fetch a blob by its BLAKE3 hash hex string.
    async fn fetch_blob(&self, hash_hex: &str) -> Result<Vec<u8>, IrohError> {
        let hash = parse_hash(hash_hex)?;

        debug!(hash = %hash_hex, "Fetching blob");

        // Check local store first — no network needed if we already have it
        if let Ok(bytes) = self.read_blob_local(hash).await {
            debug!(hash = %hash_hex, "Blob served from local store");
            return Ok(bytes);
        }

        // Not local — need to download. Requires a NodeAddr; for pure blob
        // fetches without a Doc ticket the caller must have provided routing
        // info separately. For MVP this is a limitation: blob-only ContentRefs
        // require the blob to already be locally available or a prior Doc sync
        // to have fetched it.
        //
        // TODO: accept optional NodeAddr here for direct blob downloads
        // once the gRPC adapter can supply peer addresses.
        Err(IrohError::BlobFetch(
            hash_hex.to_string(),
            "blob not in local store and no peer address available for download".to_string(),
        ))
    }

    /// Read a blob from the local in-memory store.
    async fn read_blob_local(&self, hash: Hash) -> Result<Vec<u8>, IrohError> {
        self.node
            .store
            .blobs()
            .read_to_bytes(hash)
            .await
            .map(|b| b.to_vec())
            .map_err(|e| IrohError::BlobFetch(hash.to_string(), e.to_string()))
    }

    // -----------------------------------------------------------------------
    // Doc path
    // -----------------------------------------------------------------------

    /// Fetch a value from an Iroh Doc at `path` key.
    ///
    /// Connects to the doc using the best available routing info, then
    /// looks up the path key and fetches the referenced blob.
    async fn fetch_doc(
        &self,
        namespace_id: &str,
        ticket: Option<&str>,
        node_id: Option<&str>,
        relay_url: Option<&str>,
        path: &str,
    ) -> Result<Vec<u8>, IrohError> {
        let effective_path = if path.is_empty() { DEFAULT_PATH_KEY } else { path };

        // --- Tier 1: full ticket (fastest) ---
        if let Some(ticket_str) = ticket {
            debug!(namespace = %namespace_id, "Connecting to doc via ticket");
            return self.fetch_doc_via_ticket(ticket_str, effective_path).await;
        }

        // --- Tier 2: node_id + optional relay (direct dial) ---
        if let Some(nid) = node_id {
            debug!(
                namespace = %namespace_id,
                node_id = %nid,
                "Connecting to doc via node_id"
            );
            return self
                .fetch_doc_via_node_id(namespace_id, nid, relay_url, effective_path)
                .await;
        }

        // --- Tier 3: namespace only — local lookup ---
        debug!(
            namespace = %namespace_id,
            "No routing info — attempting local doc lookup"
        );
        self.fetch_doc_local(namespace_id, effective_path).await
    }

    /// Tier 1: import namespace from a full `DocTicket` string and fetch path.
    async fn fetch_doc_via_ticket(
        &self,
        ticket_str: &str,
        path: &str,
    ) -> Result<Vec<u8>, IrohError> {
        let ticket: DocTicket = ticket_str
            .parse()
            .map_err(|e| IrohError::InvalidTicket(format!("{e}")))?;

        // Import the namespace — this syncs the doc from the peer in the ticket
        let doc = self
            .node
            .docs
            .import_namespace(ticket.capability.clone())
            .await
            .map_err(|e| IrohError::Internal(format!("import_namespace failed: {e}")))?;

        // Connect to the peer specified in the ticket to trigger sync
        if let Some(peer) = ticket.nodes.first() {
            doc.start_sync(vec![peer.clone()])
                .await
                .map_err(|e| {
                    warn!(error = %e, "start_sync failed, proceeding with local state");
                })
                .ok();
        }

        self.read_doc_key(&doc, path).await
    }

    /// Tier 2: open a doc by namespace ID and dial a specific node for sync.
    async fn fetch_doc_via_node_id(
        &self,
        namespace_id: &str,
        node_id_str: &str,
        _relay_url: Option<&str>,
        path: &str,
    ) -> Result<Vec<u8>, IrohError> {
        // Parse node ID
        let node_id: iroh::NodeId = node_id_str
            .parse()
            .map_err(|e| IrohError::Internal(format!("invalid node_id '{node_id_str}': {e}")))?;

        // Open or create the doc locally
        let ns_id: iroh_docs::NamespaceId = namespace_id
            .parse()
            .map_err(|e| IrohError::Internal(format!("invalid namespace_id: {e}")))?;

        let doc = self
            .node
            .docs
            .open(ns_id)
            .await
            .map_err(|e| IrohError::Internal(format!("docs open failed: {e}")))?
            .ok_or_else(|| IrohError::DocNotFound(namespace_id.to_string()))?;

        // Dial the peer directly — relay_url improves reliability but isn't
        // required if the node is reachable via DHT/Pkarr
        let peer_addr = iroh::NodeAddr::new(node_id);
        doc.start_sync(vec![peer_addr])
            .await
            .map_err(|e| {
                warn!(error = %e, node_id = %node_id_str, "start_sync via node_id failed");
            })
            .ok();

        self.read_doc_key(&doc, path).await
    }

    /// Tier 3: look up a doc that is already present in the local store.
    async fn fetch_doc_local(
        &self,
        namespace_id: &str,
        path: &str,
    ) -> Result<Vec<u8>, IrohError> {
        let ns_id: iroh_docs::NamespaceId = namespace_id
            .parse()
            .map_err(|e| IrohError::Internal(format!("invalid namespace_id: {e}")))?;

        let doc = self
            .node
            .docs
            .open(ns_id)
            .await
            .map_err(|e| IrohError::Internal(format!("docs open failed: {e}")))?
            .ok_or_else(|| IrohError::InsufficientRoutingInfo(namespace_id.to_string()))?;

        self.read_doc_key(&doc, path).await
    }

    // -----------------------------------------------------------------------
    // Shared: read a key from an open doc and fetch the referenced blob
    // -----------------------------------------------------------------------

    async fn read_doc_key(
        &self,
        doc: &iroh_docs::Doc,
        path: &str,
    ) -> Result<Vec<u8>, IrohError> {
        use iroh_docs::store::Query;

        // Query for the latest entry at `path` key
        let entry = doc
            .get_many(Query::key_exact(path.as_bytes()))
            .await
            .map_err(|e| IrohError::Internal(format!("doc query failed: {e}")))?
            .next()
            .await
            .ok_or_else(|| IrohError::DocKeyNotFound(path.to_string()))?
            .map_err(|e| IrohError::Internal(format!("entry stream error: {e}")))?;

        let blob_hash = entry.content_hash();

        debug!(
            path,
            hash = %blob_hash,
            "Doc key resolved to blob hash"
        );

        // Fetch the blob the entry points to
        self.read_blob_local(blob_hash).await
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_hash(hex: &str) -> Result<Hash, IrohError> {
    hex.parse::<Hash>()
        .map_err(|e| IrohError::InvalidHash(format!("'{hex}': {e}")))
}
