use std::sync::Arc;
use std::time::Duration;

use kaspa_wallet_core::api::WalletApi;
use kaspa_wallet_core::events::Events;
use kaspa_wallet_core::tx::{
    Fees, Generator, GeneratorSettings, PaymentDestination, PaymentOutput,
};
use kaspa_wallet_core::utxo::UtxoContext;
use kaspa_wallet_core::wallet::Wallet;
use kaspa_wrpc_client::prelude::*;
use tokio::time::timeout;
use tracing::{debug, info};

use meygned_core::{AccessPolicy, ContentRef, KnsName, MeygnedPayload};

use crate::error::KaspaError;
use crate::wallet::WalletHandle;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum payload size in bytes, per KIP-13 transient storage mass limits.
/// A MeygnedPayload with a full Doc ticket typically serializes to 200-350 bytes.
pub const MAX_PAYLOAD_BYTES: usize = 520;

/// Default wRPC endpoint for a local Kaspa node (mainnet).
pub const DEFAULT_RPC_URL: &str = "ws://127.0.0.1:17110";

/// How long to wait for transaction confirmation before timing out.
pub const CONFIRMATION_TIMEOUT_SECS: u64 = 120;

// ---------------------------------------------------------------------------
// PublishResult
// ---------------------------------------------------------------------------

/// The result of a successful `meygned publish` operation.
#[derive(Debug, Clone)]
pub struct PublishResult {
    /// The Kaspa transaction ID carrying the MeygnedPayload.
    /// This is what the resolver will find when someone runs `meygned resolve`.
    pub tx_id: String,
    /// Size of the serialized payload in bytes.
    pub payload_size_bytes: usize,
    /// Transaction fee paid in sompi (1 KAS = 100_000_000 sompi).
    pub fee_sompi: u64,
}

// ---------------------------------------------------------------------------
// Publisher
// ---------------------------------------------------------------------------

/// Builds, signs, broadcasts, and confirms a Kaspa transaction carrying
/// a `MeygnedPayload` that binds Iroh content to a KNS `.kas` name.
pub struct Publisher {
    rpc_url: String,
}

impl Publisher {
    pub fn new(rpc_url: &str) -> Self {
        Self {
            rpc_url: rpc_url.to_string(),
        }
    }

    /// Publish a content binding for `name`.
    ///
    /// ## Flow
    /// 1. Serialize `MeygnedPayload` → `MEYGNED:{...}` bytes, size-check
    /// 2. Connect to Kaspa node via wRPC
    /// 3. Register owner address with UtxoContext, wait for UTXO sync
    /// 4. Build transaction via Generator with payload attached
    /// 5. Sign via PSKBSigner
    /// 6. Broadcast
    /// 7. Wait for confirmation event (up to CONFIRMATION_TIMEOUT_SECS)
    /// 8. Return PublishResult with confirmed tx_id
    pub async fn publish(
        &self,
        name: &KnsName,
        content_ref: ContentRef,
        access_policy: Option<AccessPolicy>,
        wallet: &WalletHandle,
        account_index: u32,
    ) -> Result<PublishResult, KaspaError> {
        // ------------------------------------------------------------------
        // Step 1: Build and size-check payload
        // ------------------------------------------------------------------
        let payload = MeygnedPayload {
            version: 1,
            name: name.full(),
            content_ref,
            access_policy,
        };

        let payload_bytes = payload
            .to_bytes()
            .map_err(|e| KaspaError::Internal(e.to_string()))?;

        if payload_bytes.len() > MAX_PAYLOAD_BYTES {
            return Err(KaspaError::PayloadTooLarge {
                size: payload_bytes.len(),
                max: MAX_PAYLOAD_BYTES,
            });
        }

        info!(
            name = %name,
            payload_bytes = payload_bytes.len(),
            "MeygnedPayload ready"
        );

        // ------------------------------------------------------------------
        // Step 2: Connect to Kaspa node
        // ------------------------------------------------------------------
        let network_id = wallet.network.to_network_id();

        let rpc_client = KaspaRpcClient::new_with_args(
            WrpcEncoding::Borsh,
            &self.rpc_url,
            None,
            Some(network_id),
            None,
        )
        .map_err(|e| KaspaError::RpcConnect(self.rpc_url.clone(), e.to_string()))?;

        rpc_client
            .connect(ConnectOptions::default())
            .await
            .map_err(|e| KaspaError::RpcConnect(self.rpc_url.clone(), e.to_string()))?;

        info!(url = %self.rpc_url, "Connected to Kaspa node");

        // ------------------------------------------------------------------
        // Step 3: Register address with UtxoContext and wait for UTXO sync
        // ------------------------------------------------------------------
        let owner_address = wallet
            .receive_address(account_index)
            .await?;

        info!(address = %owner_address, "Using owner address");

        // Wire UtxoProcessor to the RPC client
        let utxo_processor = kaspa_wallet_core::utxo::UtxoProcessor::new(
            &Arc::new(rpc_client.clone()),
            Some(network_id),
            None,
        );

        let utxo_context = UtxoContext::new(
            &utxo_processor,
            kaspa_wallet_core::utxo::UtxoContextBinding::Internal,
        );

        utxo_context
            .register_addresses(vec![owner_address.clone()])
            .await
            .map_err(|e| KaspaError::Internal(format!("UTXO context setup failed: {e}")))?;

        // Start the processor and wait for initial UTXO scan to complete
        utxo_processor
            .start()
            .await
            .map_err(|e| KaspaError::Internal(format!("UtxoProcessor start failed: {e}")))?;

        self.wait_for_utxo_sync(&utxo_processor).await?;

        debug!("UTXO sync complete");

        // ------------------------------------------------------------------
        // Step 4 & 5: Build, sign via Generator
        // ------------------------------------------------------------------

        // Get the account for signing
        let accounts = wallet
            .wallet
            .accounts(None)
            .await
            .map_err(|e| KaspaError::Internal(format!("account list failed: {e}")))?;

        let account = accounts
            .get(account_index as usize)
            .ok_or(KaspaError::AccountNotFound(account_index))?
            .clone();

        // Send a minimal dust amount back to ourselves — the real value is
        // in the payload. Kaspa transactions require at least one output.
        let payment = PaymentDestination::PaymentOutputs(vec![PaymentOutput::new(
            owner_address.clone(),
            kaspa_wallet_core::tx::DUST_THRESHOLD, // minimum viable output
        )]);

        let settings = GeneratorSettings::try_new_with_account(
            account.clone().as_dyn_arc(),
            payment,
            None,                                          // fee rate (None = default)
            Fees::SenderPays(0),                          // let Generator calculate fees
            Some(payload_bytes.clone().into()),            // ← our MEYGNED: payload
        )
        .map_err(|e| KaspaError::Internal(format!("GeneratorSettings failed: {e}")))?;

        let generator = Generator::try_new(settings, None, None)
            .map_err(|e| KaspaError::Internal(format!("Generator::try_new failed: {e}")))?;

        // ------------------------------------------------------------------
        // Step 5 (cont.): Collect pending transactions and sign
        // ------------------------------------------------------------------
        use kaspa_wallet_core::account::pskb::{
            bundle_from_pskt_generator, PSKBSigner, PSKTGenerator,
        };

        let keydata = account
            .prv_key_data(&wallet.wallet_secret)
            .await
            .map_err(|e| KaspaError::Signing(e.to_string()))?;

        let signer = Arc::new(PSKBSigner::new(
            account.clone().as_dyn_arc(),
            keydata,
            Secret::from(vec![]),  // no payment secret for MVP
        ));

        let pskt_generator = PSKTGenerator::new(
            generator,
            signer,
            wallet.wallet.address_prefix()
                .map_err(|e| KaspaError::Internal(e.to_string()))?,
        );

        let bundle = bundle_from_pskt_generator(pskt_generator)
            .await
            .map_err(|e| KaspaError::Signing(format!("bundle signing failed: {e}")))?;

        // Extract fee from generator summary
        let summary = bundle.summary();
        let fee_sompi = summary.fees;

        debug!(fee_sompi, "Transaction signed");

        // ------------------------------------------------------------------
        // Step 6: Broadcast
        // ------------------------------------------------------------------
        let transactions = bundle
            .into_transactions()
            .map_err(|e| KaspaError::Internal(format!("bundle finalization failed: {e}")))?;

        // The last transaction in the bundle carries our payload
        let tx_id = transactions
            .last()
            .map(|tx| tx.id().to_string())
            .ok_or_else(|| KaspaError::Internal("empty transaction bundle".to_string()))?;

        for tx in &transactions {
            rpc_client
                .submit_transaction(tx.as_ref().into(), false)
                .await
                .map_err(|e| KaspaError::Broadcast(e.to_string()))?;

            debug!(tx_id = %tx.id(), "Transaction submitted");
        }

        info!(tx_id, "Transactions broadcast — waiting for confirmation");

        // ------------------------------------------------------------------
        // Step 7: Wait for confirmation
        // ------------------------------------------------------------------
        self.wait_for_confirmation(&utxo_processor, &tx_id).await?;

        info!(tx_id, "Transaction confirmed");

        // Disconnect cleanly
        rpc_client.disconnect().await.ok();

        Ok(PublishResult {
            tx_id,
            payload_size_bytes: payload_bytes.len(),
            fee_sompi,
        })
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Wait for the UtxoProcessor to emit a `Sync` event indicating the
    /// initial UTXO scan is complete and UTXOs are available for spending.
    async fn wait_for_utxo_sync(
        &self,
        processor: &kaspa_wallet_core::utxo::UtxoProcessor,
    ) -> Result<(), KaspaError> {
        let mut receiver = processor.multiplexer().channel();

        timeout(Duration::from_secs(30), async {
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        if matches!(*event, Events::Sync { synced: true, .. }) {
                            return Ok(());
                        }
                    }
                    Err(e) => {
                        return Err(KaspaError::Internal(format!(
                            "event channel closed during UTXO sync: {e}"
                        )));
                    }
                }
            }
        })
        .await
        .map_err(|_| KaspaError::Timeout("UTXO sync timed out after 30s".to_string()))?
    }

    /// Wait for a transaction confirmed event matching `tx_id`.
    async fn wait_for_confirmation(
        &self,
        processor: &kaspa_wallet_core::utxo::UtxoProcessor,
        tx_id: &str,
    ) -> Result<(), KaspaError> {
        let mut receiver = processor.multiplexer().channel();

        timeout(
            Duration::from_secs(CONFIRMATION_TIMEOUT_SECS),
            async {
                loop {
                    match receiver.recv().await {
                        Ok(event) => {
                            // Match on outgoing transaction maturity event
                            if let Events::Pending { record } = &*event {
                                if record.id().to_string() == tx_id {
                                    return Ok(());
                                }
                            }
                            if let Events::Maturity { record } = &*event {
                                if record.id().to_string() == tx_id {
                                    return Ok(());
                                }
                            }
                        }
                        Err(e) => {
                            return Err(KaspaError::Internal(format!(
                                "event channel closed awaiting confirmation: {e}"
                            )));
                        }
                    }
                }
            },
        )
        .await
        .map_err(|_| {
            KaspaError::Timeout(format!(
                "confirmation timed out after {CONFIRMATION_TIMEOUT_SECS}s — \
                 tx may still confirm; check tx_id: {tx_id}"
            ))
        })?
    }
}
