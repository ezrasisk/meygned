use thiserror::Error;

#[derive(Error, Debug)]
pub enum KaspaError {
    // -- KNS API --
    #[error("KNS API request failed: {0}")]
    KnsHttp(String),

    #[error("KNS name not found: '{0}'")]
    KnsNameNotFound(String),

    #[error("KNS API returned unexpected response: {0}")]
    KnsUnexpectedResponse(String),

    // -- Kaspa REST (transaction lookup) --
    #[error("Kaspa REST API request failed: {0}")]
    KaspaHttp(String),

    #[error("no Meygned payload found for '{0}'")]
    NoPayloadFound(String),

    // -- Signer derivation --
    #[error("signer address derivation failed: {0}")]
    SignerDerivationFailed(String),

    // -- Anti-hijack validation --
    #[error("payload signer '{signer}' does not match KNS owner '{owner}' for '{name}'")]
    SignerMismatch {
        name: String,
        owner: String,
        signer: String,
    },

    // -- Payload --
    #[error("payload parse error: {0}")]
    PayloadParse(String),

    #[error("payload too large: {size} bytes (max {max})")]
    PayloadTooLarge { size: usize, max: usize },

    // -- Wallet --
    #[error("wallet file not found: '{0}' — run `meygned wallet create` to create one")]
    WalletNotFound(String),

    #[error("failed to open wallet: {0}")]
    WalletOpen(String),

    #[error("account index {0} not found in wallet")]
    AccountNotFound(u32),

    // -- RPC / node --
    #[error("failed to connect to Kaspa node at '{0}': {1}")]
    RpcConnect(String, String),

    #[error("transaction signing failed: {0}")]
    Signing(String),

    #[error("transaction broadcast failed: {0}")]
    Broadcast(String),

    #[error("timeout: {0}")]
    Timeout(String),

    // -- Catch-all --
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<KaspaError> for meygned_core::MeygnedError {
    fn from(e: KaspaError) -> Self {
        match e {
            KaspaError::KnsNameNotFound(n) => meygned_core::MeygnedError::KnsNameNotFound(n),
            KaspaError::NoPayloadFound(n) => meygned_core::MeygnedError::NoPayloadFound(n),
            KaspaError::SignerMismatch { name, owner, signer } => {
                meygned_core::MeygnedError::SignerMismatch { name, owner, signer }
            }
            KaspaError::SignerDerivationFailed(e) => {
                meygned_core::MeygnedError::Internal(format!("signer derivation: {e}"))
            }
            KaspaError::KnsHttp(e) | KaspaError::KaspaHttp(e) => {
                meygned_core::MeygnedError::Http(e)
            }
            other => meygned_core::MeygnedError::Internal(other.to_string()),
        }
    }
}
