//! # meygned-core
//!
//! Shared types, resolver flow, and errors for the Meygned decentralized
//! web hosting and resolution system.
//!
//! ## Resolver Flow
//!
//! ```text
//! Input:  "ezra.p2phost"
//! Output: MeygnedRecord | MeygnedError
//!
//! Step 1 — Parse name         (meygned-core)    "ezra.p2phost" → MeygnedName
//! Step 2 — Index lookup       (meygned-kaspa)   name → tx_id + owner + raw payload bytes
//! Step 3 — Deserialize payload(meygned-kaspa)   bytes → KaspaPayload (version check)
//! Step 4 — Access check       (meygned-core)    public → proceed | paywall → meygned-igra (post-MVP)
//! Step 5 — Content fetch      (meygned-iroh)    ContentRef → bytes (ticket → DHT fallback)
//! Step 6 — Assemble record    (meygned-core)    all parts → MeygnedRecord
//! ```

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// MeygnedName — parsed human-readable name
// ---------------------------------------------------------------------------

/// A parsed Meygned name, e.g. `"ezra.p2phost"` →
/// `MeygnedName { label: "ezra", suffix: "p2phost" }`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MeygnedName {
    /// The human-readable label, e.g. `"ezra"`.
    pub label: String,
    /// The suffix / TLD, e.g. `"p2phost"`.
    pub suffix: String,
}

impl MeygnedName {
    /// Parse a raw name string into a `MeygnedName`.
    ///
    /// Splits on the *last* `.` so that multi-part labels like
    /// `"sub.ezra.p2phost"` are supported in the future.
    ///
    /// # Errors
    /// Returns [`MeygnedError::InvalidName`] if the input has no `.`,
    /// or if either the label or suffix is empty.
    pub fn parse(raw: &str) -> Result<Self, MeygnedError> {
        let raw = raw.trim().to_lowercase();
        let (label, suffix) = raw
            .rsplit_once('.')
            .ok_or_else(|| MeygnedError::InvalidName(raw.clone()))?;

        if label.is_empty() || suffix.is_empty() {
            return Err(MeygnedError::InvalidName(raw));
        }

        Ok(Self {
            label: label.to_string(),
            suffix: suffix.to_string(),
        })
    }

    /// Reconstruct the full name string, e.g. `"ezra.p2phost"`.
    pub fn as_str(&self) -> String {
        format!("{}.{}", self.label, self.suffix)
    }
}

impl std::fmt::Display for MeygnedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.label, self.suffix)
    }
}

// ---------------------------------------------------------------------------
// ContentRef — Iroh content pointer with tiered routing
// ---------------------------------------------------------------------------

/// A reference to content stored in Iroh, carrying enough routing information
/// to fetch it efficiently.
///
/// ## Resolution priority for `Doc` variants
/// 1. `ticket` present → direct connection (fastest)
/// 2. `node_id` + `relay_url` → dial via relay
/// 3. `node_id` only → DHT / Pkarr discovery (slowest, may time out)
/// 4. `namespace_id` only → error: insufficient routing info
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentRef {
    /// Points to a single immutable blob identified by its BLAKE3 hash.
    /// Use for static sites that never change. No routing info needed —
    /// the hash is self-certifying and any Iroh node that has it can serve it.
    Blob {
        /// Hex-encoded BLAKE3 hash.
        hash: String,
    },

    /// Points to a mutable Iroh Doc (CRDT key-value store).
    /// Use for dynamic sites or anything that needs updates without a new
    /// Kaspa transaction.
    ///
    /// Doc keys mirror URL paths: `"/"` → `index.html` blob hash,
    /// `"/style.css"` → stylesheet blob hash, etc.
    Doc {
        /// Iroh `NamespaceId` (always required).
        namespace_id: String,

        /// Full `DocTicket` string — includes `NodeId` and `RelayUrl`.
        /// Preferred: enables direct connection without discovery.
        #[serde(skip_serializing_if = "Option::is_none")]
        ticket: Option<String>,

        /// Iroh `NodeId` — used when the hoster moves frequently and
        /// re-publishes a full ticket is impractical.
        #[serde(skip_serializing_if = "Option::is_none")]
        node_id: Option<String>,

        /// Relay URL associated with `node_id`. Enables direct dial
        /// without waiting for DHT propagation.
        #[serde(skip_serializing_if = "Option::is_none")]
        relay_url: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// AccessPolicy — who can fetch the content
// ---------------------------------------------------------------------------

/// Describes who is allowed to fetch the content referenced by a record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AccessPolicy {
    /// No restrictions — anyone can fetch.
    Public,

    /// Content is gated behind an Igra transaction payload.
    /// `tx_id` is the Kaspa transaction that proves payment or access rights.
    /// Full verification logic lives in `meygned-igra` (post-MVP).
    Paywall {
        /// The Kaspa transaction ID that proves payment / access.
        tx_id: String,
        /// Human-readable description of the access requirement, e.g.
        /// `"0.1 KAS per access"`.
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
}

impl Default for AccessPolicy {
    fn default() -> Self {
        Self::Public
    }
}

// ---------------------------------------------------------------------------
// KaspaPayload — what is serialized into the Kaspa transaction payload field
// ---------------------------------------------------------------------------

/// The envelope stored in the Kaspa transaction `payload` field.
///
/// Must remain as small as possible — every byte costs KIP-13 mass.
/// The `version` field enables forward-compatible parsing: unknown versions
/// are skipped by the indexer rather than causing errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KaspaPayload {
    /// Payload format version. Current: `1`.
    /// Indexers MUST skip payloads with unrecognized versions.
    pub version: u8,

    /// The operation this transaction encodes.
    pub op: PayloadOp,
}

impl KaspaPayload {
    /// Serialize to bytes for inclusion in a Kaspa transaction payload.
    pub fn to_bytes(&self) -> Result<Vec<u8>, MeygnedError> {
        serde_json::to_vec(self).map_err(|e| MeygnedError::Serialization(e.to_string()))
    }

    /// Deserialize from raw transaction payload bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, MeygnedError> {
        serde_json::from_slice(bytes).map_err(|e| MeygnedError::Serialization(e.to_string()))
    }
}

/// The operation encoded in a [`KaspaPayload`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PayloadOp {
    /// Register a new name. The sender's address (derived from the tx)
    /// becomes the owner. Fails if the name is already registered.
    Register {
        /// The full name being registered, e.g. `"ezra.p2phost"`.
        name: String,
        /// Where the content lives in Iroh.
        content_ref: ContentRef,
        /// Who can access the content. Defaults to `Public` if omitted.
        #[serde(skip_serializing_if = "Option::is_none")]
        access_policy: Option<AccessPolicy>,
    },

    /// Update the content or access policy of an existing name.
    /// Only valid if the sender == current owner (verified by indexer).
    Update {
        /// The full name being updated.
        name: String,
        /// New content reference.
        content_ref: ContentRef,
        /// New access policy. `None` leaves the existing policy unchanged.
        #[serde(skip_serializing_if = "Option::is_none")]
        access_policy: Option<AccessPolicy>,
    },

    /// Transfer ownership of a name to a new owner.
    ///
    /// The new owner is derived from the **first non-change output address**
    /// of the Kaspa transaction — NOT from any field in this payload.
    /// This makes transfers unforgeable: you must actually send KAS to the
    /// new owner's address as part of the transfer transaction.
    Transfer {
        /// The full name being transferred.
        name: String,
    },
}

// ---------------------------------------------------------------------------
// MeygnedRecord — the fully resolved record returned to callers
// ---------------------------------------------------------------------------

/// The fully assembled result of a successful name resolution.
///
/// Never stored on-chain as a whole — assembled by the resolver from
/// Kaspa index data + deserialized payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeygnedRecord {
    /// The parsed name that was resolved.
    pub name: MeygnedName,

    /// The Kaspa address of the current owner.
    pub owner: String,

    /// The Kaspa transaction ID that carries the authoritative payload.
    /// Provides on-chain provenance.
    pub tx_id: String,

    /// The deserialized payload from that transaction.
    pub payload: KaspaPayload,

    /// Arbitrary metadata attached by the resolver (e.g. DAA score,
    /// resolution timestamp). Not stored on-chain.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub meta: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// MeygnedError — unified error type
// ---------------------------------------------------------------------------

/// All errors that can occur during Meygned resolution.
#[derive(thiserror::Error, Debug)]
pub enum MeygnedError {
    // -- Name parsing --
    #[error("invalid name: '{0}' (must contain at least one '.' with non-empty label and suffix)")]
    InvalidName(String),

    // -- Kaspa / indexer --
    #[error("name not found: '{0}'")]
    NameNotFound(String),

    #[error("name '{0}' is owned by '{1}', operation requires ownership")]
    NotOwner(String, String),

    #[error("kaspa RPC error: {0}")]
    KaspaRpc(String),

    // -- Payload serialization --
    #[error("payload serialization error: {0}")]
    Serialization(String),

    #[error("unknown payload version: {0}")]
    UnknownPayloadVersion(u8),

    // -- Iroh / content fetch --
    #[error("iroh fetch failed: {0}")]
    IrohFetch(String),

    #[error("content hash mismatch: expected {expected}, got {got}")]
    HashMismatch { expected: String, got: String },

    #[error("doc key not found: '{0}'")]
    DocKeyNotFound(String),

    #[error("fetch timed out after {0}s")]
    FetchTimeout(u64),

    #[error("insufficient routing info for doc '{0}': provide ticket, node_id, or enable DHT")]
    InsufficientRoutingInfo(String),

    // -- Access control --
    #[error("access denied: content requires payment (tx: {0})")]
    PaywallRequired(String),

    // -- Catch-all --
    #[error("internal error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- MeygnedName ---

    #[test]
    fn parse_simple_name() {
        let name = MeygnedName::parse("ezra.p2phost").unwrap();
        assert_eq!(name.label, "ezra");
        assert_eq!(name.suffix, "p2phost");
    }

    #[test]
    fn parse_name_is_lowercased() {
        let name = MeygnedName::parse("Ezra.P2PHOST").unwrap();
        assert_eq!(name.label, "ezra");
        assert_eq!(name.suffix, "p2phost");
    }

    #[test]
    fn parse_multilabel_name_splits_on_last_dot() {
        let name = MeygnedName::parse("sub.ezra.p2phost").unwrap();
        assert_eq!(name.label, "sub.ezra");
        assert_eq!(name.suffix, "p2phost");
    }

    #[test]
    fn parse_name_no_dot_errors() {
        assert!(matches!(
            MeygnedName::parse("nodot"),
            Err(MeygnedError::InvalidName(_))
        ));
    }

    #[test]
    fn parse_name_empty_label_errors() {
        assert!(matches!(
            MeygnedName::parse(".suffix"),
            Err(MeygnedError::InvalidName(_))
        ));
    }

    #[test]
    fn parse_name_empty_suffix_errors() {
        assert!(matches!(
            MeygnedName::parse("label."),
            Err(MeygnedError::InvalidName(_))
        ));
    }

    #[test]
    fn name_display_roundtrip() {
        let name = MeygnedName::parse("ezra.p2phost").unwrap();
        assert_eq!(name.as_str(), "ezra.p2phost");
        assert_eq!(name.to_string(), "ezra.p2phost");
    }

    // --- KaspaPayload serialization roundtrip ---

    #[test]
    fn payload_register_roundtrip() {
        let payload = KaspaPayload {
            version: 1,
            op: PayloadOp::Register {
                name: "ezra.p2phost".to_string(),
                content_ref: ContentRef::Blob {
                    hash: "abc123".to_string(),
                },
                access_policy: None,
            },
        };

        let bytes = payload.to_bytes().unwrap();
        let decoded = KaspaPayload::from_bytes(&bytes).unwrap();

        // Version preserved
        assert_eq!(decoded.version, 1);

        // Op preserved
        if let PayloadOp::Register { name, .. } = decoded.op {
            assert_eq!(name, "ezra.p2phost");
        } else {
            panic!("expected Register op");
        }
    }

    #[test]
    fn payload_transfer_has_no_new_owner_field() {
        let payload = KaspaPayload {
            version: 1,
            op: PayloadOp::Transfer {
                name: "ezra.p2phost".to_string(),
            },
        };

        let bytes = payload.to_bytes().unwrap();
        let json = String::from_utf8(bytes).unwrap();

        // Ensure "new_owner" never appears in a Transfer payload
        assert!(!json.contains("new_owner"));
    }

    #[test]
    fn doc_content_ref_with_ticket_roundtrip() {
        let cr = ContentRef::Doc {
            namespace_id: "ns123".to_string(),
            ticket: Some("ticket_abc".to_string()),
            node_id: None,
            relay_url: None,
        };

        let json = serde_json::to_string(&cr).unwrap();
        let decoded: ContentRef = serde_json::from_str(&json).unwrap();
        assert_eq!(cr, decoded);
    }

    #[test]
    fn access_policy_default_is_public() {
        assert_eq!(AccessPolicy::default(), AccessPolicy::Public);
    }
}
