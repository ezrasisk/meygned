//! # meygned-kaspa
//!
//! KNS domain resolution and Meygned payload scanning for Meygned.
//!
//! ## What this crate does (Path A — built on KNS)
//!
//! 1. Queries the KNS public API to resolve a `.kas` name → owner address
//! 2. Scans the owner's Kaspa transactions for a valid `MeygnedPayload`
//! 3. Validates that the payload signer matches the KNS owner (anti-hijack)
//! 4. Returns the `ContentRef` for `meygned-iroh` to fetch
//!
//! ## What this crate does NOT do
//! - Name registration (handled by KNS)
//! - Ownership tracking (handled by KNS)
//! - Running a Kaspa node (uses public REST APIs for MVP)
//!
//! ## Usage
//!
//! ```rust,no_run
//! use meygned_kaspa::{resolve_name, KnsClient, PayloadScanner};
//!
//! #[tokio::main]
//! async fn main() {
//!     let kns = KnsClient::new();
//!     let scanner = PayloadScanner::new();
//!
//!     // Full resolution: KNS lookup + payload scan
//!     let (kns_record, scan_result) = resolve_name(&kns, &scanner, "ezra.kas")
//!         .await
//!         .unwrap();
//!
//!     println!("Owner: {}", kns_record.owner);
//!     println!("Content: {:?}", scan_result.payload.content_ref);
//! }
//! ```

pub mod error;
pub mod kns;
pub mod scanner;

pub use error::KaspaError;
pub use kns::{KnsClient, KNS_API_BASE};
pub use scanner::{PayloadScanResult, PayloadScanner, KASPA_REST_BASE};

use meygned_core::{KnsName, KnsRecord};

/// Convenience function: resolve a `.kas` name end-to-end.
///
/// 1. Parses and validates the name
/// 2. Queries KNS for the current owner
/// 3. Scans the owner's transactions for a MeygnedPayload
///
/// Returns `(KnsRecord, PayloadScanResult)` on success.
pub async fn resolve_name(
    kns: &KnsClient,
    scanner: &PayloadScanner,
    name: &str,
) -> Result<(KnsRecord, PayloadScanResult), KaspaError> {
    // Parse and validate
    let kns_name = KnsName::parse(name)
        .map_err(|e| KaspaError::Internal(e.to_string()))?;

    // Step 1: KNS owner lookup
    let kns_record = kns.get_domain(&kns_name.label).await?;

    // Step 2: Meygned payload scan
    let scan_result = scanner
        .find_payload(&kns_record.owner, &kns_name.full())
        .await?;

    Ok((kns_record, scan_result))
}
