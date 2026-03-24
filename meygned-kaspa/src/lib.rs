//! # meygned-kaspa
//!
//! KNS domain resolution, Meygned payload scanning, signer verification,
//! and content publishing for Meygned.

pub mod error;
pub mod kns;
pub mod publisher;
pub mod scanner;
pub mod signer;
pub mod wallet;

pub use error::KaspaError;
pub use kns::{KnsClient, KNS_API_BASE};
pub use publisher::{PublishResult, Publisher, DEFAULT_RPC_URL, MAX_PAYLOAD_BYTES};
pub use scanner::{PayloadScanResult, PayloadScanner, KASPA_REST_BASE};
pub use signer::SignerNetwork;
pub use wallet::{WalletHandle, default_wallet_path};

use meygned_core::{KnsName, KnsRecord};

/// Convenience: resolve a `.kas` name end-to-end (KNS + payload scan).
pub async fn resolve_name(
    kns: &KnsClient,
    scanner: &PayloadScanner,
    name: &str,
) -> Result<(KnsRecord, PayloadScanResult), KaspaError> {
    let kns_name = KnsName::parse(name)
        .map_err(|e| KaspaError::Internal(e.to_string()))?;

    let kns_record = kns.get_domain(&kns_name.label).await?;
    let scan_result = scanner
        .find_payload(&kns_record.owner, &kns_name.full())
        .await?;

    Ok((kns_record, scan_result))
}
