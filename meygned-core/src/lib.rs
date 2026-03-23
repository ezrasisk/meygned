//! # meygned-core
//!
//! Shared types, resolver flow, and errors for the Meygned decentralized
//! web hosting and resolution system.
//!
//! ## Architecture (Path A — built on KNS)
//!
//! Meygned does NOT manage name ownership. That is delegated entirely to KNS.
//! Meygned's job is:
//!   1. Ask KNS: "who owns ezra.kas?"  → get owner address
//!   2. Ask Kaspa: "does this owner have a MeygnedPayload for ezra.kas?" → get ContentRef
//!   3. Ask Iroh: "fetch the content at this ContentRef" → get bytes
//!
//! ## Resolver Flow
//!
//! ```text
//! Input:  "ezra.kas"
//! Output: MeygnedRecord | MeygnedError
//!
//! Step 1 — Parse & validate name     (meygned-core)
//!           "ezra.kas" → KnsName { label: "ezra" }
//!           Only .kas suffix accepted (KNS constraint)
//!
//! Step 2 — KNS owner lookup          (meygned-kaspa)
//!           GET api.knsdomains.org/mainnet/api/v1/domain/ezra
//!           → KnsRecord { owner: "kaspa:alice...", tx_id: "..." }
//!
//! Step 3 — Meygned payload lookup    (meygned-kaspa)
//!           Scan owner's transactions for latest valid MeygnedPayload
//!           where payload.name == "ezra.kas" && signer == owner
//!           Anti-hijack: signer must match KNS owner at inscription time
//!
//! Step 4 — Access check              (meygned-core)
//!           Public → proceed | Paywall → meygned-igra (post-MVP)
//!
//! Step 5 — Content fetch             (meygned-iroh)
//!           ContentRef → bytes (ticket → node_id → local fallback)
//!
//! Step 6 — Assemble MeygnedRecord    (meygned-core)
//! ```

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// KnsName — a validated .kas domain name
// ---------------------------------------------------------------------------

/// A validated KNS domain name. Always ends in `.kas` — the only suffix
/// supported by the KNS protocol.
///
/// The label is stored without the `.kas` suffix. "ezra.kas" → label "ezra".
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KnsName {
    /// The human-readable label, e.g. `"ezra"`. Never contains a dot.
    pub label: String,
}

impl KnsName {
    /// The only suffix KNS supports.
    pub const SUFFIX: &'static str = "kas";

    /// Parse and validate a raw name string.
    ///
    /// Accepts both `"ezra"` (bare label) and `"ezra.kas"` (with suffix).
    /// Rejects anything with a suffix other than `.kas`.
    ///
    /// # Errors
    /// Returns [`MeygnedError::InvalidName`] if the name is empty, contains
    /// invalid characters, or uses an unsupported suffix.
    pub fn parse(raw: &str) -> Result<Self, MeygnedError> {
        let raw = raw.trim().to_lowercase();

        if raw.is_empty() {
            return Err(MeygnedError::InvalidName(raw));
        }

        // Strip .kas suffix if present
        let label = if let Some(stripped) = raw.strip_suffix(".kas") {
            stripped.to_string()
        } else if raw.contains('.') {
            // Has a dot but not .kas — unsupported suffix
            return Err(MeygnedError::UnsupportedSuffix(raw));
        } else {
            // Bare label — valid
            raw.clone()
        };

        if label.is_empty() {
            return Err(MeygnedError::InvalidName(raw));
        }

        // Basic character validation: alphanumeric, hyphens, no leading/trailing hyphen
        if label.starts_with('-') || label.ends_with('-') {
            return Err(MeygnedError::InvalidName(label));
        }
        if label.chars().any(|c| !c.is_alphanumeric() && c != '-') {
            return Err(MeygnedError::InvalidName(label));
        }

        Ok(Self { label })
    }

    /// The full name with suffix, e.g. `"ezra.kas"`.
    pub fn full(&self) -> String {
        format!("{}.{}", self.label, Self::SUFFIX)
    }
}

impl std::fmt::Display for KnsName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.label, Self::SUFFIX)
    }
}

// ---------------------------------------------------------------------------
// KnsRecord — what the KNS API returns for a domain lookup
// ---------------------------------------------------------------------------

/// The result of a KNS API domain lookup.
/// Represents the current ownership state of a `.kas` name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnsRecord {
    /// The full domain name, e.g. `"ezra.kas"`.
    pub name: String,
    /// Current owner's Kaspa address.
    pub owner: String,
    /// The Kaspa transaction ID that inscribed this registration.
    pub tx_id: String,
}

// ---------------------------------------------------------------------------
// ContentRef — Iroh content pointer with tiered routing
// ---------------------------------------------------------------------------

/// A reference to content stored in Iroh.
///
/// ## Doc resolution priority
/// 1. `ticket` present  → direct connection (fastest, no discovery needed)
/// 2. `node_id` present → dial via relay or DHT
/// 3. `namespace_id` only → local store lookup only (slowest, may fail)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentRef {
    /// Immutable content identified by BLAKE3 hash.
    /// Use for static sites. Hash is self-certifying.
    Blob {
        /// Hex-encoded BLAKE3 hash.
        hash: String,
    },

    /// Mutable Iroh Doc (CRDT key-value store).
    /// Use for dynamic sites. Doc keys mirror URL paths:
    /// `"/"` → index.html blob hash, `"/app.js"` → JS blob hash, etc.
    Doc {
        /// Iroh `NamespaceId` — always required.
        namespace_id: String,

        /// Full `DocTicket` — fastest resolution, includes NodeId + RelayUrl.
        #[serde(skip_serializing_if = "Option::is_none")]
        ticket: Option<String>,

        /// Iroh `NodeId` — for dynamic hosters who move frequently.
        #[serde(skip_serializing_if = "Option::is_none")]
        node_id: Option<String>,

        /// Relay URL paired with `node_id`.
        #[serde(skip_serializing_if = "Option::is_none")]
        relay_url: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// AccessPolicy — who can fetch the content
// ---------------------------------------------------------------------------

/// Describes who is permitted to fetch the content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AccessPolicy {
    /// No restrictions.
    Public,

    /// Content is gated behind an Igra transaction.
    /// Full verification is post-MVP (`meygned-igra`).
    Paywall {
        tx_id: String,
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
// MeygnedPayload — inscribed into a Kaspa transaction by the site owner
// ---------------------------------------------------------------------------

/// The data inscribed into a Kaspa transaction payload by a site owner.
///
/// ## How it works
/// The owner of `ezra.kas` (per KNS) sends a Kaspa transaction with this
/// struct serialized as JSON in the payload field. The Meygned resolver
/// finds this transaction, verifies the signer matches the KNS owner, and
/// extracts the `content_ref` to fetch content from Iroh.
///
/// ## Anti-hijack guarantee
/// Validity requires: `signer_address == kns_owner_at_inscription_time`.
/// This is verified by `meygned-kaspa` — not stored here.
///
/// ## Size budget
/// KNS/KIP-13 limits payloads to ~520 bytes. Keep this struct lean.
/// A typical `MeygnedPayload` with a Doc ticket serializes to ~200-350 bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeygnedPayload {
    /// Format version. Current: `1`.
    /// Future parsers skip payloads with versions they don't recognize.
    pub version: u8,

    /// The `.kas` name this payload binds content to, e.g. `"ezra.kas"`.
    /// Must be owned by the transaction signer (verified externally by KNS).
    pub name: String,

    /// Where the content lives in Iroh.
    pub content_ref: ContentRef,

    /// Who can access the content. Defaults to `Public`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_policy: Option<AccessPolicy>,
}

impl MeygnedPayload {
    /// The protocol tag prefix. Payloads not starting with this are skipped.
    /// Using a prefix avoids false-positive matches on unrelated transactions.
    pub const TAG: &'static str = "MEYGNED:";

    /// Serialize to bytes for inclusion in a Kaspa transaction payload.
    /// Prepends the `MEYGNED:` tag so the scanner can skip non-Meygned txs fast.
    pub fn to_bytes(&self) -> Result<Vec<u8>, MeygnedError> {
        let json = serde_json::to_string(self)
            .map_err(|e| MeygnedError::Serialization(e.to_string()))?;
        let tagged = format!("{}{}", Self::TAG, json);
        Ok(tagged.into_bytes())
    }

    /// Deserialize from raw transaction payload bytes.
    /// Returns `None` if the bytes are not a Meygned payload (not an error).
    /// Returns `Err` only for internal/storage failures.
    pub fn from_bytes(bytes: &[u8]) -> Result<Option<Self>, MeygnedError> {
        if bytes.is_empty() {
            return Ok(None);
        }

        // Fast prefix check — skip non-Meygned payloads immediately
        let s = match std::str::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return Ok(None),
        };

        let json = match s.strip_prefix(Self::TAG) {
            Some(j) => j,
            None => return Ok(None),
        };

        // Parse JSON
        let payload: MeygnedPayload = serde_json::from_str(json)
            .map_err(|e| MeygnedError::Serialization(e.to_string()))?;

        // Skip unrecognized versions for forward compatibility
        if payload.version > 1 {
            return Ok(None);
        }

        Ok(Some(payload))
    }
}

// ---------------------------------------------------------------------------
// MeygnedRecord — fully resolved record returned to callers
// ---------------------------------------------------------------------------

/// The assembled result of a successful name resolution.
/// Never stored on-chain — constructed by the resolver at query time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeygnedRecord {
    /// The resolved KNS name.
    pub name: KnsName,
    /// Current owner address (from KNS).
    pub owner: String,
    /// KNS registration transaction ID.
    pub kns_tx_id: String,
    /// Kaspa transaction ID carrying the MeygnedPayload.
    pub payload_tx_id: String,
    /// The deserialized payload.
    pub payload: MeygnedPayload,
    /// Optional resolver metadata (e.g. resolution timestamp, DAA score).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub meta: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// MeygnedError — unified error type
// ---------------------------------------------------------------------------

#[derive(thiserror::Error, Debug)]
pub enum MeygnedError {
    // -- Name parsing --
    #[error("invalid name: '{0}'")]
    InvalidName(String),

    #[error("unsupported suffix in '{0}': only .kas is supported")]
    UnsupportedSuffix(String),

    // -- KNS --
    #[error("KNS name not found: '{0}'")]
    KnsNameNotFound(String),

    #[error("KNS API error: {0}")]
    KnsApi(String),

    // -- Meygned payload --
    #[error("no Meygned payload found for '{0}' (name is registered in KNS but has no Meygned content binding)")]
    NoPayloadFound(String),

    #[error("payload serialization error: {0}")]
    Serialization(String),

    #[error("unknown payload version: {0}")]
    UnknownPayloadVersion(u8),

    // -- Anti-hijack --
    #[error("payload signer '{signer}' does not match KNS owner '{owner}' for name '{name}'")]
    SignerMismatch {
        name: String,
        owner: String,
        signer: String,
    },

    // -- Iroh / content fetch --
    #[error("iroh fetch failed: {0}")]
    IrohFetch(String),

    #[error("content hash mismatch: expected {expected}, got {got}")]
    HashMismatch { expected: String, got: String },

    #[error("doc key not found: '{0}'")]
    DocKeyNotFound(String),

    #[error("fetch timed out after {0}s")]
    FetchTimeout(u64),

    #[error("insufficient routing info for doc '{0}'")]
    InsufficientRoutingInfo(String),

    // -- Access control --
    #[error("access denied: content requires payment (tx: {0})")]
    PaywallRequired(String),

    // -- HTTP / network --
    #[error("HTTP error: {0}")]
    Http(String),

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

    // --- KnsName ---

    #[test]
    fn parse_full_name_with_suffix() {
        let n = KnsName::parse("ezra.kas").unwrap();
        assert_eq!(n.label, "ezra");
        assert_eq!(n.full(), "ezra.kas");
    }

    #[test]
    fn parse_bare_label_without_suffix() {
        let n = KnsName::parse("ezra").unwrap();
        assert_eq!(n.label, "ezra");
        assert_eq!(n.full(), "ezra.kas");
    }

    #[test]
    fn parse_is_case_insensitive() {
        let n = KnsName::parse("EZRA.KAS").unwrap();
        assert_eq!(n.label, "ezra");
    }

    #[test]
    fn rejects_unsupported_suffix() {
        assert!(matches!(
            KnsName::parse("ezra.eth"),
            Err(MeygnedError::UnsupportedSuffix(_))
        ));
        assert!(matches!(
            KnsName::parse("ezra.p2phost"),
            Err(MeygnedError::UnsupportedSuffix(_))
        ));
    }

    #[test]
    fn rejects_empty_name() {
        assert!(matches!(
            KnsName::parse(""),
            Err(MeygnedError::InvalidName(_))
        ));
    }

    #[test]
    fn rejects_leading_hyphen() {
        assert!(matches!(
            KnsName::parse("-ezra.kas"),
            Err(MeygnedError::InvalidName(_))
        ));
    }

    #[test]
    fn display_includes_suffix() {
        let n = KnsName::parse("ezra").unwrap();
        assert_eq!(n.to_string(), "ezra.kas");
    }

    // --- MeygnedPayload serialization ---

    fn sample_payload(name: &str) -> MeygnedPayload {
        MeygnedPayload {
            version: 1,
            name: name.to_string(),
            content_ref: ContentRef::Blob {
                hash: "deadbeef".to_string(),
            },
            access_policy: None,
        }
    }

    #[test]
    fn payload_roundtrip_via_bytes() {
        let p = sample_payload("ezra.kas");
        let bytes = p.to_bytes().unwrap();
        let decoded = MeygnedPayload::from_bytes(&bytes).unwrap().unwrap();
        assert_eq!(decoded.name, "ezra.kas");
        assert_eq!(decoded.version, 1);
    }

    #[test]
    fn payload_bytes_have_meygned_tag_prefix() {
        let p = sample_payload("ezra.kas");
        let bytes = p.to_bytes().unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.starts_with("MEYGNED:"));
    }

    #[test]
    fn non_meygned_bytes_return_none() {
        let result = MeygnedPayload::from_bytes(b"some random kaspa tx data").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn empty_bytes_return_none() {
        let result = MeygnedPayload::from_bytes(&[]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn future_version_returns_none() {
        let p = MeygnedPayload {
            version: 99,
            name: "ezra.kas".to_string(),
            content_ref: ContentRef::Blob {
                hash: "abc".to_string(),
            },
            access_policy: None,
        };
        // Manually build tagged bytes bypassing to_bytes() version guard
        let json = serde_json::to_string(&p).unwrap();
        let tagged = format!("MEYGNED:{json}");
        let result = MeygnedPayload::from_bytes(tagged.as_bytes()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn doc_content_ref_roundtrip() {
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
    fn access_policy_defaults_to_public() {
        assert_eq!(AccessPolicy::default(), AccessPolicy::Public);
    }
}
