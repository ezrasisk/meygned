use meygned_core::MeygnedPayload;
use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use crate::{
    error::KaspaError,
    signer::{derive_signer_address, ResolvedInput, SignerNetwork},
};

/// Kaspa REST API base URL.
pub const KASPA_REST_BASE: &str = "https://api.kaspa.org";

// ---------------------------------------------------------------------------
// Kaspa REST API response shapes
// ---------------------------------------------------------------------------

/// A full transaction as returned by the Kaspa REST API with
/// `resolve_previous_outpoints=light`. The `inputs` array contains
/// inline `previous_outpoint_resolved` fields with `script_public_key`
/// data — enabling signer derivation without a second round-trip.
#[derive(Debug, Deserialize)]
pub struct KaspaTransaction {
    pub transaction_id: String,

    /// Payload field as a hex string. Empty string if no payload.
    #[serde(default)]
    pub payload: String,

    /// Transaction inputs with resolved previous outpoints.
    #[serde(default)]
    pub inputs: Vec<ResolvedInput>,
}

// ---------------------------------------------------------------------------
// PayloadScanResult
// ---------------------------------------------------------------------------

/// A validated Meygned payload found in a Kaspa transaction, with a
/// fully derived and verified signer address.
#[derive(Debug, Clone)]
pub struct PayloadScanResult {
    /// Kaspa transaction ID carrying this payload.
    pub tx_id: String,
    /// The deserialized Meygned payload.
    pub payload: MeygnedPayload,
    /// The cryptographically derived signer address.
    /// Verified to match the KNS owner before this result is returned.
    pub signer: String,
}

// ---------------------------------------------------------------------------
// PayloadScanner
// ---------------------------------------------------------------------------

pub struct PayloadScanner {
    http: Client,
    base_url: String,
    network: SignerNetwork,
}

impl PayloadScanner {
    pub fn new() -> Self {
        Self::with_config(KASPA_REST_BASE.to_string(), SignerNetwork::Mainnet)
    }

    pub fn with_config(base_url: String, network: SignerNetwork) -> Self {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("failed to build HTTP client");
        Self { http, base_url, network }
    }

    /// Find the most recent valid `MeygnedPayload` for `name` in the
    /// transactions of `owner_address`, with full signer verification.
    ///
    /// ## What "valid" means
    /// 1. Transaction has a non-empty payload starting with `MEYGNED:`
    /// 2. Payload deserializes cleanly and has a recognized version
    /// 3. `payload.name` matches the requested name (case-insensitive)
    /// 4. Signer address derived from transaction inputs == `owner_address`
    ///
    /// Step 4 is the anti-hijack guarantee. It ensures that even if
    /// someone crafts a payload claiming `name = "ezra.kas"`, it's only
    /// valid if it was signed by the actual KNS owner of `ezra.kas`.
    pub async fn find_payload(
        &self,
        owner_address: &str,
        name: &str,
    ) -> Result<PayloadScanResult, KaspaError> {
        let txs = self.fetch_transactions(owner_address).await?;

        // Scan in reverse (most recent first) — return first valid match
        for tx in txs.iter().rev() {
            // Decode hex payload bytes
            let payload_bytes = match hex::decode(&tx.payload) {
                Ok(b) if !b.is_empty() => b,
                _ => continue,
            };

            // Attempt to parse as MeygnedPayload
            let payload = match MeygnedPayload::from_bytes(&payload_bytes) {
                Ok(Some(p)) => p,
                Ok(None) => continue,
                Err(e) => {
                    debug!(
                        tx_id = %tx.transaction_id,
                        error = %e,
                        "Payload parse error, skipping tx"
                    );
                    continue;
                }
            };

            // Name must match what we're resolving
            if payload.name.to_lowercase() != name.to_lowercase() {
                continue;
            }

            // Derive the actual signer from transaction inputs
            let signer = match derive_signer_address(&tx.inputs, self.network) {
                Ok(s) => s,
                Err(e) => {
                    debug!(
                        tx_id = %tx.transaction_id,
                        error = %e,
                        "Signer derivation failed, skipping tx"
                    );
                    continue;
                }
            };

            // Anti-hijack: signer must match the KNS-registered owner
            if !addresses_match(&signer, owner_address) {
                return Err(KaspaError::SignerMismatch {
                    name: name.to_string(),
                    owner: owner_address.to_string(),
                    signer,
                });
            }

            debug!(
                tx_id = %tx.transaction_id,
                name,
                signer,
                "Found valid, verified MeygnedPayload"
            );

            return Ok(PayloadScanResult {
                tx_id: tx.transaction_id.clone(),
                payload,
                signer,
            });
        }

        Err(KaspaError::NoPayloadFound(name.to_string()))
    }

    /// Fetch full transactions for `address` from the Kaspa REST API.
    ///
    /// Uses `resolve_previous_outpoints=light` so each input's
    /// `script_public_key` is returned inline — enabling signer
    /// derivation without a second HTTP call per transaction.
    async fn fetch_transactions(
        &self,
        address: &str,
    ) -> Result<Vec<KaspaTransaction>, KaspaError> {
        let url = format!(
            "{}/addresses/{}/full-transactions\
             ?limit=100&offset=0&resolve_previous_outpoints=light",
            self.base_url, address
        );

        debug!(url, "Fetching transactions for address");

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| KaspaError::KaspaHttp(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(KaspaError::KaspaHttp(format!(
                "HTTP {} from Kaspa REST API",
                resp.status()
            )));
        }

        let txs: Vec<KaspaTransaction> = resp
            .json()
            .await
            .map_err(|e| KaspaError::KaspaHttp(format!("JSON parse error: {e}")))?;

        Ok(txs)
    }
}

impl Default for PayloadScanner {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Address comparison — case-insensitive, prefix-aware
// ---------------------------------------------------------------------------

/// Compare two Kaspa addresses for equality.
/// Kaspa addresses are bech32 with a case-insensitive payload portion,
/// so we normalise to lowercase before comparing.
fn addresses_match(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_match_is_case_insensitive() {
        assert!(addresses_match(
            "kaspa:QPAUQSVK7YF9UNEXWMXSNMG547MHYGA37CSH0KJ53Q6XXG",
            "kaspa:qpauqsvk7yf9unexwmxsnmg547mhyga37csh0kj53q6xxg"
        ));
        assert!(!addresses_match(
            "kaspa:qpauqsvk7yf9unexwmxsnmg547",
            "kaspa:qdifferentaddress"
        ));
    }
}
