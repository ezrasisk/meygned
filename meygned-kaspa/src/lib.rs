//! # meygned-kaspa
//!
//! Kaspa RPC client, payload parser, and indexer state machine for Meygned.
//!
//! ## Responsibilities
//! - Connect to a local Kaspa full node via gRPC
//! - Scan transactions for valid [`KaspaPayload`] data
//! - Maintain a local name registry index using redb
//! - Answer name resolution queries for the resolver pipeline
//!
//! ## Usage
//!
//! ```rust,no_run
//! use meygned_kaspa::{KaspaConfig, KaspaNetwork, open_store, Indexer};
//! use std::path::PathBuf;
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = KaspaConfig {
//!         rpc_url: "grpc://127.0.0.1:16110".to_string(),
//!         db_path: PathBuf::from("meygned-index.redb"),
//!         network: KaspaNetwork::Mainnet,
//!     };
//!
//!     let store = open_store(&config.db_path).unwrap();
//!     let indexer = Indexer::new(store);
//!
//!     // Resolve a name after indexing
//!     let record = indexer.store().get_name("ezra.p2phost").unwrap();
//! }
//! ```

pub mod error;
pub mod indexer;
pub mod parser;
pub mod rpc;
pub mod store;

// ---------------------------------------------------------------------------
// Re-exports — public API surface
// ---------------------------------------------------------------------------

pub use error::KaspaError;
pub use indexer::{IndexerTx, Indexer};
pub use parser::{parse_payload, CURRENT_PAYLOAD_VERSION};
pub use rpc::{KaspaConfig, KaspaNetwork, KaspaRpcClient, RawBlock, run_catchup, run_live_stream};
pub use store::{NameRecord, Store};

// ---------------------------------------------------------------------------
// Convenience constructors
// ---------------------------------------------------------------------------

/// Open (or create) the redb index store at the path specified in `config`.
pub fn open_store(db_path: &std::path::Path) -> Result<Store, KaspaError> {
    Store::open(db_path)
}

/// Resolve a name from the local index, mapping `None` to a `NameNotFound` error.
pub fn resolve_name(store: &Store, name: &str) -> Result<NameRecord, KaspaError> {
    store
        .get_name(name)?
        .ok_or_else(|| KaspaError::NameNotFound(name.to_string()))
}
