//! # meygned-iroh
//!
//! Iroh node management and content fetching for Meygned.
//!
//! Handles both blob and doc resolution, with three-tier routing
//! for mutable docs: ticket → node_id dial → local-only fallback.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use meygned_iroh::{IrohNode, IrohFetcher};
//! use meygned_core::ContentRef;
//!
//! #[tokio::main]
//! async fn main() {
//!     let node = IrohNode::spawn().await.unwrap();
//!     let fetcher = IrohFetcher::new(&node);
//!
//!     let content_ref = ContentRef::Doc {
//!         namespace_id: "some_namespace_id".to_string(),
//!         ticket: Some("ticket_string_here".to_string()),
//!         node_id: None,
//!         relay_url: None,
//!     };
//!
//!     let bytes = fetcher.fetch(&content_ref, "/").await.unwrap();
//!     println!("Fetched {} bytes", bytes.len());
//!
//!     node.shutdown().await.unwrap();
//! }
//! ```

pub mod error;
pub mod fetch;
pub mod node;

pub use error::IrohError;
pub use fetch::{IrohFetcher, DEFAULT_PATH_KEY};
pub use node::IrohNode;
