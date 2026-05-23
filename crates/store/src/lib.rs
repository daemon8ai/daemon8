// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

pub mod active_session;
pub mod cursor;
pub mod debug_session;
pub mod error_hash;
pub mod hash_cache;
mod lens;
pub mod memory;
pub mod scope_ledger;
mod surreal;

pub use active_session::{ActiveDebugSession, ActiveSessionState};
pub use cursor::SurrealCursorStore;
pub use debug_session::SurrealDebugSessionStore;
pub use hash_cache::ObservationHashCache;
pub use lens::{LensManager, LensStatus};
pub use memory::SurrealMemoryStore;
pub use scope_ledger::SurrealScopeLedgerStore;
pub use surreal::{ResetReport, SCHEMA_VERSION, SurrealStore};

use daemon8_types::{
    Checkpoint, DebugSessionOutcome, DebugSessionStatus, Filter, MemoryKind, Observation,
    RuntimeSummary, StateSlice,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Db(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("lock poisoned")]
    LockPoisoned,

    #[error("{0}")]
    Other(String),
}

#[async_trait::async_trait]
pub trait StateModel: Send + Sync {
    async fn insert(&self, obs: &Observation) -> Result<u64, StoreError>;
    async fn query(&self, filter: &Filter) -> Result<StateSlice, StoreError>;
    async fn summary(&self) -> Result<RuntimeSummary, StoreError>;
    async fn checkpoint(&self) -> Checkpoint;
    async fn oldest_id(&self) -> Option<u64>;
    async fn cleanup_before(&self, timestamp_ns: u64) -> Result<u64, StoreError>;
    async fn health_check(&self) -> Result<(), StoreError>;
    async fn memory_export_select_page(
        &self,
        query: &str,
        limit: u64,
        start: u64,
    ) -> Result<Vec<serde_json::Value>, StoreError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub kind: MemoryKind,
    pub content: String,
    pub source_observations: Vec<u64>,
    pub tags: Vec<String>,
    pub project_slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub confidence: f64,
    /// Optional structured payload. SessionSummary uses this to carry
    /// resolve_debug_session's rich-capture fields (root_cause, fix_diff,
    /// commands_used, related_errors). Other memory kinds may leave it empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub scope_root: String,
    pub source: String,
    pub source_instance: String,
    pub position: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[async_trait::async_trait]
pub trait CursorStore: Send + Sync {
    async fn upsert_cursor(&self, cursor: CursorState) -> Result<(), StoreError>;
    async fn get_cursor(
        &self,
        scope_root: &str,
        source: &str,
        source_instance: &str,
    ) -> Result<Option<CursorState>, StoreError>;
    async fn list_cursors_for_scope(
        &self,
        scope_root: &str,
    ) -> Result<Vec<CursorState>, StoreError>;
}

#[derive(Debug, Clone, Default)]
pub struct MemoryFilter {
    pub kinds: Option<Vec<MemoryKind>>,
    pub tags: Option<Vec<String>>,
    pub project_slug: Option<String>,
    pub session_id: Option<String>,
    pub text_match: Option<String>,
    pub limit: Option<usize>,
}

#[async_trait::async_trait]
pub trait MemoryStore: Send + Sync {
    async fn save_memory(&self, memory: Memory) -> Result<String, StoreError>;
    async fn query_memory(&self, filter: &MemoryFilter) -> Result<Vec<Memory>, StoreError>;
    async fn get_memory(&self, id: &str) -> Result<Option<Memory>, StoreError>;
    async fn update_memory(&self, memory: Memory) -> Result<(), StoreError>;
    async fn forget_memory(&self, id: &str) -> Result<bool, StoreError>;
}

/// A persistent debug investigation. Multiple checkpoints belong to one session;
/// the session is the high-level lifecycle that bookends a debugging effort.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugSession {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    pub last_activity: u64,
    pub project_slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: DebugSessionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<DebugSessionOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_memory_id: Option<String>,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<String>,
}

/// A bookmark within a debug session — anchors a moment in the observation
/// stream so the agent can ask "what changed since this point" later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugCheckpoint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub debug_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: u64,
    pub seq_at_creation: u64,
}

#[async_trait::async_trait]
pub trait DebugSessionStore: Send + Sync {
    async fn start_debug_session(&self, session: DebugSession) -> Result<String, StoreError>;
    async fn get_debug_session(&self, id: &str) -> Result<Option<DebugSession>, StoreError>;
    async fn list_debug_sessions(
        &self,
        status: Option<DebugSessionStatus>,
    ) -> Result<Vec<DebugSession>, StoreError>;
    async fn end_debug_session(
        &self,
        id: &str,
        status: DebugSessionStatus,
        outcome: Option<DebugSessionOutcome>,
        summary_memory_id: Option<String>,
        ended_at: u64,
    ) -> Result<(), StoreError>;
    async fn touch_debug_session(&self, id: &str, last_activity: u64) -> Result<(), StoreError>;
    async fn find_stale_active(&self, threshold_ns: u64) -> Result<Vec<DebugSession>, StoreError>;

    async fn create_checkpoint(&self, checkpoint: DebugCheckpoint) -> Result<String, StoreError>;
    async fn get_checkpoint(&self, id: &str) -> Result<Option<DebugCheckpoint>, StoreError>;
    async fn list_checkpoints(
        &self,
        debug_session_id: &str,
    ) -> Result<Vec<DebugCheckpoint>, StoreError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeSessionRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub session_id: String,
    pub provider: String,
    pub agent_name: String,
    pub mode: String,
    pub requested_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_count: Option<u64>,
    pub connected_at: u64,
    pub last_seen_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentScopeRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub mode: String,
    pub requested_path: String,
    pub scope_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_count: Option<u64>,
    pub first_seen_at: u64,
    pub last_seen_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeConnectFailureRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub session_id: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    pub requested_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    pub mode: String,
    pub status: String,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    pub attempt_count: u64,
    pub first_seen_at: u64,
    pub last_seen_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeIgnoreRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub scope_root: String,
    pub ignored_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScopeLedgerSummary {
    pub recent_scopes: Vec<RecentScopeRecord>,
    pub recent_failures: Vec<ScopeConnectFailureRecord>,
}

#[async_trait::async_trait]
pub trait ScopeLedgerStore: Send + Sync {
    async fn record_connect_success(&self, record: ScopeSessionRecord) -> Result<(), StoreError>;
    async fn record_recent_scope(&self, record: RecentScopeRecord) -> Result<(), StoreError>;
    async fn record_connect_failure(
        &self,
        record: ScopeConnectFailureRecord,
    ) -> Result<(), StoreError>;
    async fn scope_ledger_summary(&self, limit: usize) -> Result<ScopeLedgerSummary, StoreError>;
    async fn list_recent_scopes(&self) -> Result<Vec<RecentScopeRecord>, StoreError>;
    async fn is_scope_ignored(&self, scope_root: &str) -> Result<bool, StoreError>;
    async fn set_scope_ignored(&self, scope_root: &str) -> Result<(), StoreError>;
    async fn remove_scope_ignored(&self, scope_root: &str) -> Result<(), StoreError>;
    async fn remove_scope_records(&self, scope_root: &str) -> Result<usize, StoreError>;
}
