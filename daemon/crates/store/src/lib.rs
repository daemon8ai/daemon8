// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

mod memory;
mod sqlite;

pub use memory::MemoryStore;
pub use sqlite::SqliteStore;

use daemon8_types::{Checkpoint, Filter, Observation, RuntimeSummary, StateSlice};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("lock poisoned")]
    LockPoisoned,

    #[error("{0}")]
    Other(String),
}

pub trait StateModel: Send + Sync {
    /// Insert an observation, returning the assigned id.
    fn insert(&self, obs: Observation) -> Result<u64, StoreError>;

    /// Query observations matching the filter.
    fn query(&self, filter: &Filter) -> Result<StateSlice, StoreError>;

    /// High-level runtime summary (counts, health, connections).
    fn summary(&self) -> Result<RuntimeSummary, StoreError>;

    /// Current checkpoint (max sequence seen).
    fn checkpoint(&self) -> Checkpoint;

    /// Smallest id currently retained, or None if the store is empty.
    /// Callers use this to detect replay gaps when a subscriber resumes
    /// from an id below the retention window.
    fn oldest_id(&self) -> Option<u64>;

    /// Delete observations older than the given timestamp. Returns count deleted.
    fn cleanup_before(&self, timestamp_ns: u64) -> Result<u64, StoreError>;

    /// Reclaim disk space from deleted rows (incremental vacuum).
    fn vacuum_incremental(&self, pages: u32) -> Result<(), StoreError>;

    /// Checkpoint and truncate the WAL file.
    fn wal_checkpoint(&self) -> Result<(), StoreError>;
}
