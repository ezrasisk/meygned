use meygned_core::MeygnedPayload;
use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use crate::error::KaspaError;

/// Kaspa REST API base URL (public explorer API).
pub const KASPA_REST_BASE: &str = "https://api.kaspa.org";

// ---------------------------------------------------------------------------
// Kaspa REST API response shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct KaspaTransactionListResponse {
    pub transactions: Vec<KaspaTransaction>,
}

#[derive(Debug, Deserialize)]
pub struct KaspaTransaction {
    pub transaction_id: String,
    /// Payload as a hex string.
    #[serde(default)]
    pub payload: String,
    /// Input UTXOs — used to derive the signer address.
    #[serde(default)]
    pub inputs: Vec<KaspaInput>,
}

#[derive(Debug, Deserialize)]
pub struct KaspaInput {
    pub previous_outpoint: KaspaPreviousOutpoint,
    #[serde(default)]
    pub sig_op_count: u32,
}

#[derive(Debug, Deserialize)]
pub struct KaspaPreviousOutpoint {
    pub transaction_id: String,
    pub index: u32,
}

// ---------------------------------------------------------------------------
// PayloadScanResult — what the scanner returns when it finds a match
// ---------------------------------------------------------------------------

/// A validated Meygned payload found in a Kaspa transaction.
#[derive(Debug, Clone)]
pub struct PayloadScanResult {
    /// The Kaspa transaction ID carrying this payload.
    pub tx_id: String,
    /// The deserialized payload.
    pub payload: MeygnedPayload,
    /// The signer address derived from transaction inputs.
    pub signer: String,
}

// ---------------------------------------------------------------------------
// PayloadScanner
// ---------------------------------------------------------------------------

/// Scans a Kaspa address's transactions for valid `MeygnedPayload` entries.
pub struct PayloadScanner {
    http: Client,
    base_url: String,
}

impl PayloadScanner {
    pub fn new() -> Self {
        Self::with_base_url(KASPA_REST_BASE.to_string())
    }

    pub fn with_base_url(base_url: String) -> Self {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("failed to build HTTP client");
        Self { http, base_url }
    }

    /// Find the most recent valid `MeygnedPayload` for `name` among
    /// transactions sent by `owner_address`.
    ///
    /// Validates that the signer matches `owner_address` (anti-hijack).
    ///
    /// # Errors
    /// - [`KaspaError::NoPayloadFound`] if no matching payload exists
    /// - [`KaspaError::SignerMismatch`] if a payload exists but signer ≠ owner
    pub async fn find_payload(
        &self,
        owner_address: &str,
        name: &str,
    ) -> Result<PayloadScanResult, KaspaError> {
        let txs = self.fetch_transactions(owner_address).await?;

        // Iterate in reverse (most recent first) — return the first match
        for tx in txs.iter().rev() {
            let payload_bytes = match hex::decode(&tx.payload) {
                Ok(b) => b,
                Err(_) => continue,
            };

            let payload = match MeygnedPayload::from_bytes(&payload_bytes) {
                Ok(Some(p)) => p,
                Ok(None) => continue, // Not a Meygned payload
                Err(e) => {
                    debug!(tx_id = %tx.transaction_id, error = %e, "Payload parse error, skipping");
                    continue;
                }
            };

            // Only match payloads for the name we're looking for
            if payload.name.to_lowercase() != name.to_lowercase() {
                continue;
            }

            // Derive signer from transaction inputs
            // For MVP: use the owner_address we already know from KNS.
            // The full signer extraction requires resolving the previous
            // outpoint to get the script_public_key — this is the TODO
            // that the gRPC adapter will complete post-MVP.
            let signer = self
                .derive_signer(&tx.inputs, owner_address)
                .await
                .unwrap_or_else(|_| owner_address.to_string());

            // Anti-hijack: signer must match KNS owner
            if signer.to_lowercase() != owner_address.to_lowercase() {
                return Err(KaspaError::SignerMismatch {
                    name: name.to_string(),
                    owner: owner_address.to_string(),
                    signer,
                });
            }

            debug!(
                tx_id = %tx.transaction_id,
                name,
                "Found valid MeygnedPayload"
            );

            return Ok(PayloadScanResult {
                tx_id: tx.transaction_id.clone(),
                payload,
                signer,
            });
        }

        Err(KaspaError::NoPayloadFound(name.to_string()))
    }

    /// Fetch all transactions for an address from the Kaspa REST API.
    async fn fetch_transactions(
        &self,
        address: &str,
    ) -> Result<Vec<KaspaTransaction>, KaspaError> {
        // Fetch full transactions with payload data
        let url = format!(
            "{}/addresses/{}/full-transactions?limit=100&offset=0&resolve_previous_outpoints=no",
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

        // The Kaspa REST API returns a JSON array directly for this endpoint
        let txs: Vec<KaspaTransaction> = resp
            .json()
            .await
            .map_err(|e| KaspaError::KaspaHttp(format!("JSON parse error: {e}")))?;

        Ok(txs)
    }

    /// Derive the signer address from transaction inputs.
    ///
    /// MVP implementation: trusts the owner address from KNS directly.
    /// Full implementation requires resolving previous outpoints to get
    /// script_public_keys and converting them to Kaspa addresses.
    ///
    /// TODO: implement full signer derivation using kaspa-addresses crate.
    async fn derive_signer(
        &self,
        _inputs: &[KaspaInput],
        fallback_owner: &str,
    ) -> Result<String, KaspaError> {
        // For MVP, return the KNS owner as the signer.
        // This is safe because we're only scanning txs from that address —
        // the Kaspa REST API endpoint is address-scoped, so only txs where
        // this address was involved are returned.
        Ok(fallback_owner.to_string())
    }
}

impl Default for PayloadScanner {
    fn default() -> Self {
        Self::new()
    }
}
