use std::path::{Path, PathBuf};
use std::sync::Arc;

use kaspa_wallet_core::wallet::{Wallet, WalletCreateArgs};
use kaspa_wallet_core::storage::WalletDescriptor;
use kaspa_wallet_core::api::WalletApi;
use kaspa_wallet_core::prelude::*;
use kaspa_addresses::Address;
use kaspa_bip32::Mnemonic;

use crate::error::KaspaError;
use crate::signer::SignerNetwork;

// ---------------------------------------------------------------------------
// Default wallet path
// ---------------------------------------------------------------------------

/// Returns the default wallet file path: `~/.meygned/wallet.wallet`
pub fn default_wallet_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".meygned")
        .join("wallet.wallet")
}

// ---------------------------------------------------------------------------
// WalletHandle — opened wallet with decrypted key access
// ---------------------------------------------------------------------------

/// An opened kaspa-wallet-core wallet, ready for signing transactions.
///
/// Compatible with wallets created by Kaspa NG and Kaspa CLI — users can
/// manage their KNS wallet in Kaspa NG and publish from Meygned using the
/// same wallet file.
pub struct WalletHandle {
    pub(crate) wallet: Arc<Wallet>,
    pub(crate) wallet_secret: Secret,
    pub(crate) network: SignerNetwork,
}

impl WalletHandle {
    /// Open an existing wallet file and decrypt it with a password.
    ///
    /// Prompts for the wallet password securely (no terminal echo).
    /// Compatible with Kaspa NG / Kaspa CLI wallet files.
    pub async fn open(
        wallet_path: &Path,
        network: SignerNetwork,
    ) -> Result<Self, KaspaError> {
        if !wallet_path.exists() {
            return Err(KaspaError::WalletNotFound(
                wallet_path.display().to_string(),
            ));
        }

        // Prompt for password — no echo
        let password = prompt_password("Wallet password: ")?;
        let wallet_secret = Secret::from(password.into_bytes());

        // Build the wallet instance pointing at our file
        let wallet = Wallet::try_new(
            kaspa_wallet_core::storage::WalletStorage::default(),
            Some(kaspa_wallet_core::rpc::ConnectOptions::default()),
            None,
        )
        .await
        .map_err(|e| KaspaError::WalletOpen(e.to_string()))?;

        let wallet = Arc::new(wallet);

        // Open the wallet file — decrypts metadata but not key material yet
        wallet
            .open(&wallet_secret, Some(wallet_path.to_path_buf()))
            .await
            .map_err(|e| KaspaError::WalletOpen(format!(
                "failed to open '{}': {e}",
                wallet_path.display()
            )))?;

        tracing::debug!(path = %wallet_path.display(), "Wallet opened");

        Ok(Self {
            wallet,
            wallet_secret,
            network,
        })
    }

    /// Derive the receive address for account at `account_index`.
    ///
    /// For most users this is account 0. Matches Kaspa NG's default
    /// account derivation path.
    pub async fn receive_address(
        &self,
        account_index: u32,
    ) -> Result<Address, KaspaError> {
        let accounts = self
            .wallet
            .accounts(None)
            .await
            .map_err(|e| KaspaError::WalletOpen(format!("failed to list accounts: {e}")))?;

        let account = accounts
            .get(account_index as usize)
            .ok_or_else(|| KaspaError::AccountNotFound(account_index))?;

        let address = account
            .receive_address()
            .await
            .map_err(|e| KaspaError::WalletOpen(format!("address derivation failed: {e}")))?;

        Ok(address)
    }

    /// Create a new Meygned wallet at `wallet_path`.
    ///
    /// Generates a fresh BIP39 mnemonic and saves an encrypted wallet file.
    /// Prints the mnemonic to stderr for the user to back up.
    ///
    /// Use this if the user doesn't have an existing Kaspa wallet.
    pub async fn create_new(
        wallet_path: &Path,
        network: SignerNetwork,
    ) -> Result<Self, KaspaError> {
        // Ensure parent directory exists
        if let Some(parent) = wallet_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                KaspaError::WalletOpen(format!("failed to create wallet dir: {e}"))
            })?;
        }

        // Prompt for new password twice
        let password = prompt_password("New wallet password: ")?;
        let confirm = prompt_password("Confirm password: ")?;
        if password != confirm {
            return Err(KaspaError::WalletOpen(
                "passwords do not match".to_string(),
            ));
        }

        let wallet_secret = Secret::from(password.into_bytes());

        // Generate fresh mnemonic
        let mnemonic = Mnemonic::random(Default::default(), Default::default())
            .map_err(|e| KaspaError::WalletOpen(format!("mnemonic generation failed: {e}")))?;

        eprintln!();
        eprintln!("  ┌─────────────────────────────────────────────────┐");
        eprintln!("  │  IMPORTANT — Save your recovery phrase          │");
        eprintln!("  │  Write these 24 words down and store them       │");
        eprintln!("  │  somewhere safe. They cannot be recovered.      │");
        eprintln!("  └─────────────────────────────────────────────────┘");
        eprintln!();
        eprintln!("  {}", mnemonic.phrase());
        eprintln!();

        let wallet = Wallet::try_new(
            kaspa_wallet_core::storage::WalletStorage::default(),
            None,
            None,
        )
        .await
        .map_err(|e| KaspaError::WalletOpen(e.to_string()))?;

        let wallet = Arc::new(wallet);

        wallet
            .create_bip32_wallet(
                &wallet_secret,
                &Secret::from(vec![]), // no payment secret for MVP
                &WalletCreateArgs {
                    name: Some("meygned".to_string()),
                    ..Default::default()
                },
                &mnemonic,
                Some(wallet_path.to_path_buf()),
            )
            .await
            .map_err(|e| KaspaError::WalletOpen(format!("wallet creation failed: {e}")))?;

        tracing::info!(path = %wallet_path.display(), "New wallet created");

        Ok(Self {
            wallet,
            wallet_secret,
            network,
        })
    }
}

// ---------------------------------------------------------------------------
// Secure password prompt — no terminal echo
// ---------------------------------------------------------------------------

fn prompt_password(prompt: &str) -> Result<String, KaspaError> {
    rpassword::prompt_password(prompt)
        .map_err(|e| KaspaError::WalletOpen(format!("password prompt failed: {e}")))
}
