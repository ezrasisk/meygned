use meygned_core::KnsRecord;
use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use crate::error::KaspaError;

/// Public KNS API base URL.
pub const KNS_API_BASE: &str = "https://api.knsdomains.org/mainnet/api/v1";

// ---------------------------------------------------------------------------
// KNS API response shapes
// ---------------------------------------------------------------------------

/// Raw JSON response from `GET /domain/{label}`.
/// Field names match the KNS API exactly.
#[derive(Debug, Deserialize)]
struct KnsApiDomainResponse {
    /// The domain label (without .kas suffix).
    pub domain: String,
    /// Current owner Kaspa address.
    pub owner: String,
    /// The on-chain transaction ID of the registration inscription.
    pub txid: String,
    /// Whether the domain is currently registered.
    #[serde(default)]
    pub registered: bool,
}

/// Raw JSON response from `GET /assets?owner={address}`.
#[derive(Debug, Deserialize)]
struct KnsApiAssetsResponse {
    pub assets: Vec<KnsApiAsset>,
}

#[derive(Debug, Deserialize)]
pub struct KnsApiAsset {
    pub domain: String,
    pub owner: String,
    pub txid: String,
}

// ---------------------------------------------------------------------------
// KnsClient
// ---------------------------------------------------------------------------

/// HTTP client for the KNS public API.
pub struct KnsClient {
    http: Client,
    base_url: String,
}

impl KnsClient {
    /// Create a client pointing at the KNS mainnet API.
    pub fn new() -> Self {
        Self::with_base_url(KNS_API_BASE.to_string())
    }

    /// Create a client with a custom base URL — useful for testing against
    /// a mock server or a local KNS instance.
    pub fn with_base_url(base_url: String) -> Self {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("failed to build HTTP client");
        Self { http, base_url }
    }

    /// Look up the current owner of a `.kas` domain.
    ///
    /// `label` should be the bare label without the `.kas` suffix,
    /// e.g. pass `"ezra"` to look up `ezra.kas`.
    ///
    /// # Errors
    /// - [`KaspaError::KnsNameNotFound`] if the domain is unregistered
    /// - [`KaspaError::KnsHttp`] on network or API errors
    pub async fn get_domain(&self, label: &str) -> Result<KnsRecord, KaspaError> {
        let url = format!("{}/domain/{}", self.base_url, label);
        debug!(url, "KNS domain lookup");

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| KaspaError::KnsHttp(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(KaspaError::KnsNameNotFound(format!("{label}.kas")));
        }

        if !resp.status().is_success() {
            return Err(KaspaError::KnsHttp(format!(
                "HTTP {} from KNS API",
                resp.status()
            )));
        }

        let data: KnsApiDomainResponse = resp
            .json()
            .await
            .map_err(|e| KaspaError::KnsUnexpectedResponse(e.to_string()))?;

        if !data.registered {
            return Err(KaspaError::KnsNameNotFound(format!("{label}.kas")));
        }

        Ok(KnsRecord {
            name: format!("{}.kas", data.domain),
            owner: data.owner,
            tx_id: data.txid,
        })
    }

    /// Fetch all KNS assets owned by a given Kaspa address.
    /// Used for listing all names a wallet owns.
    pub async fn get_assets_by_owner(
        &self,
        owner: &str,
    ) -> Result<Vec<KnsRecord>, KaspaError> {
        let url = format!("{}/assets?owner={}", self.base_url, owner);
        debug!(url, "KNS assets by owner lookup");

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| KaspaError::KnsHttp(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(KaspaError::KnsHttp(format!(
                "HTTP {} from KNS assets API",
                resp.status()
            )));
        }

        let data: KnsApiAssetsResponse = resp
            .json()
            .await
            .map_err(|e| KaspaError::KnsUnexpectedResponse(e.to_string()))?;

        Ok(data
            .assets
            .into_iter()
            .map(|a| KnsRecord {
                name: format!("{}.kas", a.domain),
                owner: a.owner,
                tx_id: a.txid,
            })
            .collect())
    }
}

impl Default for KnsClient {
    fn default() -> Self {
        Self::new()
    }
}
