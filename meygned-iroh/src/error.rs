use thiserror::Error;

#[derive(Error, Debug)]
pub enum IrohError {
    #[error("failed to bind iroh endpoint: {0}")]
    Bind(String),

    #[error("blob fetch failed for hash '{0}': {1}")]
    BlobFetch(String, String),

    #[error("content hash mismatch: expected {expected}, got {got}")]
    HashMismatch { expected: String, got: String },

    #[error("doc not found or not synced: namespace '{0}'")]
    DocNotFound(String),

    #[error("doc key not found: '{0}'")]
    DocKeyNotFound(String),

    #[error("invalid ticket string: {0}")]
    InvalidTicket(String),

    #[error("invalid hash string: {0}")]
    InvalidHash(String),

    #[error("insufficient routing info for namespace '{0}': provide a ticket or node_id")]
    InsufficientRoutingInfo(String),

    #[error("fetch timed out after {0}s")]
    Timeout(u64),

    #[error("iroh node not running — call IrohNode::spawn() first")]
    NodeNotRunning,

    #[error("internal iroh error: {0}")]
    Internal(String),
}

// Map into meygned-core's unified error type
impl From<IrohError> for meygned_core::MeygnedError {
    fn from(e: IrohError) -> Self {
        match e {
            IrohError::HashMismatch { expected, got } => {
                meygned_core::MeygnedError::HashMismatch { expected, got }
            }
            IrohError::DocKeyNotFound(k) => meygned_core::MeygnedError::DocKeyNotFound(k),
            IrohError::Timeout(s) => meygned_core::MeygnedError::FetchTimeout(s),
            IrohError::InsufficientRoutingInfo(ns) => {
                meygned_core::MeygnedError::InsufficientRoutingInfo(ns)
            }
            other => meygned_core::MeygnedError::IrohFetch(other.to_string()),
        }
    }
}
