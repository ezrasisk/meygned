use std::path::Path;

use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use meygned_core::{AccessPolicy, ContentRef};

use crate::error::KaspaError;

// ---------------------------------------------------------------------------
// redb table definitions
// ---------------------------------------------------------------------------

/// Primary name registry.
/// key:   name string (e.g. "ezra.p2phost")
/// value: JSON-serialized NameRecord
const NAMES: TableDefinition<&str, &str> = TableDefinition::new("names");

/// Ownership history for auditing and transfer tracking.
/// key:   "{name}:{daa_score:020}:{tx_id}"  (zero-padded for lex sort)
/// value: owner Kaspa address
const OWNERSHIP_HISTORY: TableDefinition<&str, &str> =
    TableDefinition::new("ownership_history");

/// Sync state — single-row table tracking resume point.
/// key:   "last_daa_score"
/// value: decimal string of u64
const SYNC_STATE: TableDefinition<&str, &str> = TableDefinition::new("sync_state");

const LAST_DAA_KEY: &str = "last_daa_score";

// ---------------------------------------------------------------------------
// NameRecord — persisted per registered name
// ---------------------------------------------------------------------------

/// The full state of a registered name as stored in the local index.
/// Serialized as JSON into redb values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameRecord {
    /// The full registered name, e.g. `"ezra.p2phost"`.
    pub name: String,
    /// Current owner Kaspa address.
    pub owner: String,
    /// Kaspa tx_id of the transaction that established the current state.
    pub tx_id: String,
    /// DAA score of the block containing `tx_id`.
    pub daa_score: u64,
    /// Iroh content pointer from the payload.
    pub content_ref: ContentRef,
    /// Optional access policy from the payload.
    pub access_policy: Option<AccessPolicy>,
}

// ---------------------------------------------------------------------------
// Store — thin wrapper around the redb Database
// ---------------------------------------------------------------------------

pub struct Store {
    db: Database,
}

impl Store {
    /// Open (or create) the redb database at `path`.
    /// Creates all required tables on first open.
    pub fn open(path: &Path) -> Result<Self, KaspaError> {
        let db = Database::create(path)?;

        // Ensure all tables exist
        let wtx = db.begin_write()?;
        {
            wtx.open_table(NAMES)?;
            wtx.open_table(OWNERSHIP_HISTORY)?;
            wtx.open_table(SYNC_STATE)?;
        }
        wtx.commit()?;

        Ok(Self { db })
    }

    // -----------------------------------------------------------------------
    // Sync state
    // -----------------------------------------------------------------------

    /// Read the last successfully indexed DAA score.
    /// Returns `0` if the index is fresh (never synced).
    pub fn last_daa_score(&self) -> Result<u64, KaspaError> {
        let rtx = self.db.begin_read()?;
        let table = rtx.open_table(SYNC_STATE)?;
        match table.get(LAST_DAA_KEY)? {
            Some(v) => v
                .value()
                .parse::<u64>()
                .map_err(|e| KaspaError::Database(format!("corrupt last_daa_score: {e}"))),
            None => Ok(0),
        }
    }

    /// Persist the last successfully indexed DAA score.
    pub fn set_last_daa_score(&self, score: u64) -> Result<(), KaspaError> {
        let wtx = self.db.begin_write()?;
        {
            let mut table = wtx.open_table(SYNC_STATE)?;
            let s = score.to_string();
            table.insert(LAST_DAA_KEY, s.as_str())?;
        }
        wtx.commit()?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Name records
    // -----------------------------------------------------------------------

    /// Look up the current state of a registered name.
    /// Returns `None` if the name has never been registered.
    pub fn get_name(&self, name: &str) -> Result<Option<NameRecord>, KaspaError> {
        let rtx = self.db.begin_read()?;
        let table = rtx.open_table(NAMES)?;
        match table.get(name)? {
            Some(v) => {
                let record: NameRecord = serde_json::from_str(v.value())
                    .map_err(|e| KaspaError::Database(format!("corrupt name record: {e}")))?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    /// Insert a new name registration.
    /// Caller must verify the name is not already registered before calling.
    pub fn insert_name(&self, record: &NameRecord) -> Result<(), KaspaError> {
        let json = serde_json::to_string(record)
            .map_err(|e| KaspaError::Database(format!("serialize error: {e}")))?;

        let wtx = self.db.begin_write()?;
        {
            let mut table = wtx.open_table(NAMES)?;
            table.insert(record.name.as_str(), json.as_str())?;
        }
        wtx.commit()?;
        Ok(())
    }

    /// Update an existing name record (for Update and Transfer ops).
    /// Caller must verify ownership before calling.
    pub fn update_name(&self, record: &NameRecord) -> Result<(), KaspaError> {
        let json = serde_json::to_string(record)
            .map_err(|e| KaspaError::Database(format!("serialize error: {e}")))?;

        let wtx = self.db.begin_write()?;
        {
            let mut names = wtx.open_table(NAMES)?;
            names.insert(record.name.as_str(), json.as_str())?;
        }
        wtx.commit()?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Ownership history
    // -----------------------------------------------------------------------

    /// Append an ownership change to the history table.
    /// Key is zero-padded so entries sort chronologically.
    pub fn append_ownership_history(
        &self,
        name: &str,
        daa_score: u64,
        tx_id: &str,
        owner: &str,
    ) -> Result<(), KaspaError> {
        // Zero-pad daa_score to 20 digits for correct lexicographic ordering
        let key = format!("{name}:{daa_score:020}:{tx_id}");

        let wtx = self.db.begin_write()?;
        {
            let mut table = wtx.open_table(OWNERSHIP_HISTORY)?;
            table.insert(key.as_str(), owner)?;
        }
        wtx.commit()?;
        Ok(())
    }
}
