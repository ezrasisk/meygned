use meygned_core::PayloadOp;
use tracing::{debug, warn};

use crate::{
    error::KaspaError,
    parser::{parse_payload, validate_op},
    store::{NameRecord, Store},
};

// ---------------------------------------------------------------------------
// Tx — minimal transaction representation fed to the indexer
// ---------------------------------------------------------------------------

/// A minimal representation of a Kaspa transaction as seen by the indexer.
/// The RPC layer is responsible for mapping the full gRPC response into this.
#[derive(Debug, Clone)]
pub struct IndexerTx {
    /// Transaction ID (hex string).
    pub tx_id: String,
    /// DAA score of the block that contains this transaction.
    pub daa_score: u64,
    /// The sender / signer address (derived from inputs).
    pub sender: String,
    /// First non-change output address — used as new_owner for Transfer ops.
    pub first_output_address: Option<String>,
    /// Raw payload bytes from the transaction.
    pub payload: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Indexer
// ---------------------------------------------------------------------------

/// The Meygned indexer state machine.
///
/// Processes transactions in DAA order and maintains the local name registry
/// in a [`Store`]. All operations are idempotent — re-processing an already-
/// indexed transaction is safe (it will be skipped by ownership/existence checks).
pub struct Indexer {
    store: Store,
}

impl Indexer {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    /// Process a single block's worth of transactions.
    ///
    /// Transactions MUST be provided in ascending DAA order; within the same
    /// DAA score, in lexicographic tx_id order. This is the caller's
    /// responsibility (the RPC layer sorts before calling).
    ///
    /// Individual tx errors are logged and skipped — they never abort the block.
    pub fn process_block(&self, txs: &[IndexerTx]) -> Result<(), KaspaError> {
        for tx in txs {
            if let Err(e) = self.process_tx(tx) {
                // Log but never propagate per-tx errors — a bad payload
                // from one tx should never stop indexing the rest.
                warn!(tx_id = %tx.tx_id, error = %e, "skipping tx due to error");
            }
        }
        Ok(())
    }

    /// Process a single transaction through the state machine.
    fn process_tx(&self, tx: &IndexerTx) -> Result<(), KaspaError> {
        // Step 1: parse payload — skip if not a Meygned payload
        let payload = match parse_payload(&tx.payload)? {
            Some(p) => p,
            None => return Ok(()),
        };

        // Step 2: validate op fields
        if let Err(reason) = validate_op(&payload.op) {
            debug!(tx_id = %tx.tx_id, reason, "invalid op fields, skipping");
            return Ok(());
        }

        // Step 3: dispatch to op handler
        match payload.op {
            PayloadOp::Register {
                name,
                content_ref,
                access_policy,
            } => self.handle_register(tx, name, content_ref, access_policy),

            PayloadOp::Update {
                name,
                content_ref,
                access_policy,
            } => self.handle_update(tx, name, content_ref, access_policy),

            PayloadOp::Transfer { name } => self.handle_transfer(tx, name),
        }
    }

    // -----------------------------------------------------------------------
    // Op handlers
    // -----------------------------------------------------------------------

    fn handle_register(
        &self,
        tx: &IndexerTx,
        name: String,
        content_ref: meygned_core::ContentRef,
        access_policy: Option<meygned_core::AccessPolicy>,
    ) -> Result<(), KaspaError> {
        // First registration wins — skip if already registered
        if self.store.get_name(&name)?.is_some() {
            debug!(name, tx_id = %tx.tx_id, "Register skipped: name already registered");
            return Ok(());
        }

        let record = NameRecord {
            name: name.clone(),
            owner: tx.sender.clone(),
            tx_id: tx.tx_id.clone(),
            daa_score: tx.daa_score,
            content_ref,
            access_policy,
        };

        self.store.insert_name(&record)?;
        self.store
            .append_ownership_history(&name, tx.daa_score, &tx.tx_id, &tx.sender)?;

        debug!(name, owner = %tx.sender, tx_id = %tx.tx_id, "Registered name");
        Ok(())
    }

    fn handle_update(
        &self,
        tx: &IndexerTx,
        name: String,
        content_ref: meygned_core::ContentRef,
        access_policy: Option<meygned_core::AccessPolicy>,
    ) -> Result<(), KaspaError> {
        let mut record = match self.store.get_name(&name)? {
            Some(r) => r,
            None => {
                debug!(name, tx_id = %tx.tx_id, "Update skipped: name not registered");
                return Ok(());
            }
        };

        // Only the current owner may update
        if record.owner != tx.sender {
            debug!(
                name,
                tx_id = %tx.tx_id,
                expected_owner = %record.owner,
                actual_sender = %tx.sender,
                "Update skipped: sender is not owner"
            );
            return Ok(());
        }

        record.content_ref = content_ref;
        // None access_policy in an Update means "leave existing policy unchanged"
        if let Some(policy) = access_policy {
            record.access_policy = Some(policy);
        }
        record.tx_id = tx.tx_id.clone();
        record.daa_score = tx.daa_score;

        self.store.update_name(&record)?;
        debug!(name, owner = %tx.sender, tx_id = %tx.tx_id, "Updated name");
        Ok(())
    }

    fn handle_transfer(&self, tx: &IndexerTx, name: String) -> Result<(), KaspaError> {
        let mut record = match self.store.get_name(&name)? {
            Some(r) => r,
            None => {
                debug!(name, tx_id = %tx.tx_id, "Transfer skipped: name not registered");
                return Ok(());
            }
        };

        // Only the current owner may transfer
        if record.owner != tx.sender {
            debug!(
                name,
                tx_id = %tx.tx_id,
                expected_owner = %record.owner,
                actual_sender = %tx.sender,
                "Transfer skipped: sender is not owner"
            );
            return Ok(());
        }

        // New owner is derived from the first non-change output address —
        // NOT from the payload. This makes transfers unforgeable.
        let new_owner = match &tx.first_output_address {
            Some(addr) => addr.clone(),
            None => {
                warn!(
                    name,
                    tx_id = %tx.tx_id,
                    "Transfer skipped: no output address found"
                );
                return Ok(());
            }
        };

        let old_owner = record.owner.clone();
        record.owner = new_owner.clone();
        record.tx_id = tx.tx_id.clone();
        record.daa_score = tx.daa_score;

        self.store.update_name(&record)?;
        self.store
            .append_ownership_history(&name, tx.daa_score, &tx.tx_id, &new_owner)?;

        debug!(
            name,
            from = %old_owner,
            to = %new_owner,
            tx_id = %tx.tx_id,
            "Transferred name"
        );
        Ok(())
    }

    /// Expose the store for read-only queries from the RPC layer.
    pub fn store(&self) -> &Store {
        &self.store
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use meygned_core::{ContentRef, KaspaPayload, PayloadOp};

    use super::*;
    use crate::store::Store;

    fn temp_store() -> Store {
        let dir = tempfile::tempdir().unwrap();
        Store::open(&dir.path().join("test.redb")).unwrap()
    }

    fn blob_ref() -> ContentRef {
        ContentRef::Blob {
            hash: "deadbeef".to_string(),
        }
    }

    fn make_tx(
        tx_id: &str,
        daa_score: u64,
        sender: &str,
        op: PayloadOp,
        first_output: Option<&str>,
    ) -> IndexerTx {
        let payload = KaspaPayload { version: 1, op };
        IndexerTx {
            tx_id: tx_id.to_string(),
            daa_score,
            sender: sender.to_string(),
            first_output_address: first_output.map(|s| s.to_string()),
            payload: serde_json::to_vec(&payload).unwrap(),
        }
    }

    #[test]
    fn register_new_name() {
        let indexer = Indexer::new(temp_store());
        let tx = make_tx(
            "tx001",
            100,
            "kaspa:alice",
            PayloadOp::Register {
                name: "alice.kas".to_string(),
                content_ref: blob_ref(),
                access_policy: None,
            },
            None,
        );
        indexer.process_tx(&tx).unwrap();
        let record = indexer.store().get_name("alice.kas").unwrap().unwrap();
        assert_eq!(record.owner, "kaspa:alice");
        assert_eq!(record.tx_id, "tx001");
    }

    #[test]
    fn second_register_is_ignored() {
        let indexer = Indexer::new(temp_store());
        let tx1 = make_tx(
            "tx001",
            100,
            "kaspa:alice",
            PayloadOp::Register {
                name: "alice.kas".to_string(),
                content_ref: blob_ref(),
                access_policy: None,
            },
            None,
        );
        let tx2 = make_tx(
            "tx002",
            101,
            "kaspa:bob",
            PayloadOp::Register {
                name: "alice.kas".to_string(),
                content_ref: blob_ref(),
                access_policy: None,
            },
            None,
        );
        indexer.process_tx(&tx1).unwrap();
        indexer.process_tx(&tx2).unwrap();
        // Alice still owns it
        let record = indexer.store().get_name("alice.kas").unwrap().unwrap();
        assert_eq!(record.owner, "kaspa:alice");
    }

    #[test]
    fn owner_can_update() {
        let indexer = Indexer::new(temp_store());
        let tx1 = make_tx(
            "tx001",
            100,
            "kaspa:alice",
            PayloadOp::Register {
                name: "alice.kas".to_string(),
                content_ref: blob_ref(),
                access_policy: None,
            },
            None,
        );
        let tx2 = make_tx(
            "tx002",
            101,
            "kaspa:alice",
            PayloadOp::Update {
                name: "alice.kas".to_string(),
                content_ref: ContentRef::Blob {
                    hash: "newcontent".to_string(),
                },
                access_policy: None,
            },
            None,
        );
        indexer.process_tx(&tx1).unwrap();
        indexer.process_tx(&tx2).unwrap();
        let record = indexer.store().get_name("alice.kas").unwrap().unwrap();
        if let ContentRef::Blob { hash } = &record.content_ref {
            assert_eq!(hash, "newcontent");
        } else {
            panic!("expected blob ref");
        }
    }

    #[test]
    fn non_owner_cannot_update() {
        let indexer = Indexer::new(temp_store());
        let tx1 = make_tx(
            "tx001",
            100,
            "kaspa:alice",
            PayloadOp::Register {
                name: "alice.kas".to_string(),
                content_ref: blob_ref(),
                access_policy: None,
            },
            None,
        );
        let tx2 = make_tx(
            "tx002",
            101,
            "kaspa:bob",
            PayloadOp::Update {
                name: "alice.kas".to_string(),
                content_ref: ContentRef::Blob {
                    hash: "newcontent".to_string(),
                },
                access_policy: None,
            },
            None,
        );
        indexer.process_tx(&tx1).unwrap();
        indexer.process_tx(&tx2).unwrap();
        // Content unchanged — bob's update was ignored
        let record = indexer.store().get_name("alice.kas").unwrap().unwrap();
        if let ContentRef::Blob { hash } = &record.content_ref {
            assert_eq!(hash, "deadbeef");
        } else {
            panic!("expected blob ref");
        }
    }

    #[test]
    fn transfer_changes_owner_to_output_address() {
        let indexer = Indexer::new(temp_store());
        let tx1 = make_tx(
            "tx001",
            100,
            "kaspa:alice",
            PayloadOp::Register {
                name: "alice.kas".to_string(),
                content_ref: blob_ref(),
                access_policy: None,
            },
            None,
        );
        let tx2 = make_tx(
            "tx002",
            101,
            "kaspa:alice",
            PayloadOp::Transfer {
                name: "alice.kas".to_string(),
            },
            Some("kaspa:bob"),
        );
        indexer.process_tx(&tx1).unwrap();
        indexer.process_tx(&tx2).unwrap();
        let record = indexer.store().get_name("alice.kas").unwrap().unwrap();
        assert_eq!(record.owner, "kaspa:bob");
    }

    #[test]
    fn transfer_without_output_is_skipped() {
        let indexer = Indexer::new(temp_store());
        let tx1 = make_tx(
            "tx001",
            100,
            "kaspa:alice",
            PayloadOp::Register {
                name: "alice.kas".to_string(),
                content_ref: blob_ref(),
                access_policy: None,
            },
            None,
        );
        let tx2 = make_tx(
            "tx002",
            101,
            "kaspa:alice",
            PayloadOp::Transfer {
                name: "alice.kas".to_string(),
            },
            None, // no output address
        );
        indexer.process_tx(&tx1).unwrap();
        indexer.process_tx(&tx2).unwrap();
        // Alice still owns it
        let record = indexer.store().get_name("alice.kas").unwrap().unwrap();
        assert_eq!(record.owner, "kaspa:alice");
    }
}
