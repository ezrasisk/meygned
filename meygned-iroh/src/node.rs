use iroh::{Endpoint, protocol::Router};
use iroh_blobs::{BlobsProtocol, store::mem::MemStore, ALPN as BLOBS_ALPN};
use iroh_docs::{protocol::Docs, ALPN as DOCS_ALPN};
use iroh_gossip::{net::Gossip, ALPN as GOSSIP_ALPN};
use tracing::info;

use crate::error::IrohError;

// ---------------------------------------------------------------------------
// IrohNode — owns the endpoint, store, and all protocol handles
// ---------------------------------------------------------------------------

/// A running Iroh node with blobs, gossip, and docs protocols active.
///
/// For the MVP this uses in-memory storage only. Fetched blobs and synced
/// docs are lost on shutdown — persistence can be added later by swapping
/// `MemStore` for a `FsStore` and `Docs::memory()` for `Docs::persistent()`.
///
/// # Usage
/// ```rust,no_run
/// # tokio_test::block_on(async {
/// use meygned_iroh::node::IrohNode;
/// let node = IrohNode::spawn().await.unwrap();
/// // pass node into IrohFetcher::new(&node)
/// # });
/// ```
pub struct IrohNode {
    /// The Iroh QUIC endpoint — handles all p2p connections.
    pub(crate) endpoint: Endpoint,
    /// In-memory blob store.
    pub(crate) store: MemStore,
    /// Docs protocol handle — used for key-value doc lookups.
    pub(crate) docs: Docs,
    /// The router — keeps all protocol loops alive while held.
    /// Dropping this shuts down the node.
    _router: Router,
}

impl IrohNode {
    /// Spawn a new in-memory Iroh node with all three protocols.
    ///
    /// Binds a QUIC endpoint on a random port and starts the blobs,
    /// gossip, and docs protocol handlers.
    pub async fn spawn() -> Result<Self, IrohError> {
        // Bind endpoint with N0 discovery (DNS + Pkarr)
        let endpoint = Endpoint::builder()
            .discovery_n0()
            .bind()
            .await
            .map_err(|e| IrohError::Bind(e.to_string()))?;

        info!(
            node_id = %endpoint.node_id(),
            "Iroh endpoint bound"
        );

        // In-memory blob store
        let store = MemStore::default();

        // Gossip — required by Docs
        let gossip = Gossip::builder().spawn(endpoint.clone());

        // Docs — in-memory, depends on blobs + gossip
        let docs = Docs::memory()
            .spawn(endpoint.clone(), (*store).clone(), gossip.clone())
            .await
            .map_err(|e| IrohError::Internal(format!("docs spawn failed: {e}")))?;

        // Wire all protocols into the router
        let router = Router::builder(endpoint.clone())
            .accept(BLOBS_ALPN, BlobsProtocol::new(&store, None))
            .accept(GOSSIP_ALPN, gossip)
            .accept(DOCS_ALPN, docs.clone())
            .spawn();

        info!("Iroh node ready (blobs + gossip + docs)");

        Ok(Self {
            endpoint,
            store,
            docs,
            _router: router,
        })
    }

    /// Gracefully shut down the node and all its connections.
    pub async fn shutdown(self) -> Result<(), IrohError> {
        self._router
            .shutdown()
            .await
            .map_err(|e| IrohError::Internal(format!("shutdown error: {e}")))?;
        info!("Iroh node shut down");
        Ok(())
    }

    /// The node's public key / node ID.
    pub fn node_id(&self) -> iroh::NodeId {
        self.endpoint.node_id()
    }
}
