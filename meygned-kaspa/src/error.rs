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

    // -- Anti-hijack validation --
    #[error("payload signer '{signer}' does not match KNS owner '{owner}' for '{name}'")]
    SignerMismatch {
        name: String,
        owner: String,
        signer: String,
    },

    // -- Payload parsing --
    #[error("payload parse error: {0}")]
    PayloadParse(String),

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
            KaspaError::KnsHttp(e) | KaspaError::KaspaHttp(e) => {
                meygned_core::MeygnedError::Http(e)
            }
            other => meygned_core::MeygnedError::Internal(other.to_string()),
        }
    }
}
