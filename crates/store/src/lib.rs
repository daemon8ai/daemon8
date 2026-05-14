// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

pub mod active_session;
pub mod awareness;
pub mod debug_session;
pub mod error_hash;
pub mod hash_cache;
mod lens;
pub mod librarian;
pub mod librarian_validators;
pub mod memory;
mod surreal;

pub use active_session::{ActiveDebugSession, ActiveSessionState};
pub use awareness::SurrealAwarenessStore;
pub use debug_session::SurrealDebugSessionStore;
pub use hash_cache::ObservationHashCache;
pub use lens::{LensManager, LensStatus};
pub use librarian::SurrealLibrarianStore;
pub use memory::SurrealMemoryStore;
pub use surreal::SurrealStore;

use daemon8_types::{
    AwarenessAuthority, AwarenessEdgeKind, AwarenessNodeKind, AwarenessNodeState,
    AwarenessOperation, Checkpoint, DebugSessionOutcome, DebugSessionStatus, Filter,
    LibrarianEdgeKind, LibrarianNodeKind, LocatorKind, MemoryKind, Observation, RuntimeSummary,
    StateSlice,
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
    async fn insert(&self, obs: Observation) -> Result<u64, StoreError>;
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

// ── Awareness Tree ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwarenessNode {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub project_slug: String,
    pub path: String,
    pub kind: AwarenessNodeKind,
    pub state: AwarenessNodeState,
    pub authority: AwarenessAuthority,
    pub confidence: f64,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redex: Option<String>,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    #[serde(default)]
    pub debug_session_ids: Vec<String>,
    #[serde(default)]
    pub checkpoint_ids: Vec<String>,
    pub observation_ids: Vec<u64>,
    pub librarian_node_ids: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwarenessEdge {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub kind: AwarenessEdgeKind,
    pub from_node: String,
    pub to_node: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AwarenessEvidence {
    pub observation_ids: Vec<u64>,
    pub debug_session_ids: Vec<String>,
    pub checkpoint_ids: Vec<String>,
    pub librarian_node_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AwarenessSync {
    pub operation: AwarenessOperation,
    pub project_slug: String,
    pub path: String,
    pub kind: AwarenessNodeKind,
    pub authority: Option<AwarenessAuthority>,
    pub confidence: Option<f64>,
    pub summary: Option<String>,
    pub note: Option<String>,
    pub redex: Option<String>,
    pub tags: Vec<String>,
    pub debug_session_id: Option<String>,
    pub checkpoint_id: Option<String>,
    pub evidence: AwarenessEvidence,
    pub target_node_id: Option<String>,
    pub supersedes: Vec<String>,
    pub answers: Vec<String>,
    pub contradicts: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AwarenessFilter {
    pub project_slug: String,
    pub include_inactive: bool,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct AwarenessTraversalFilter {
    pub project_slug: String,
    pub focus_path: String,
    pub depth: usize,
    pub include_inactive: bool,
    pub include_notes: bool,
    pub include_evidence: bool,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwarenessManifest {
    pub project_slug: String,
    pub counts_by_kind: std::collections::BTreeMap<String, usize>,
    pub active_objectives: Vec<AwarenessNode>,
    pub open_questions: Vec<AwarenessNode>,
    pub active_hypotheses: Vec<AwarenessNode>,
    pub stale_risk_count: usize,
    pub conflict_count: usize,
    pub suggested_focus_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwarenessTree {
    pub project_slug: String,
    pub focus_path: String,
    pub nodes: Vec<AwarenessNode>,
    pub edges: Vec<AwarenessEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwarenessConflict {
    pub reason: String,
    pub incoming_path: String,
    pub existing_nodes: Vec<AwarenessNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwarenessSyncResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<AwarenessNode>,
    pub edges: Vec<AwarenessEdge>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<AwarenessConflict>,
}

#[async_trait::async_trait]
pub trait AwarenessStore: Send + Sync {
    async fn sync_node(&self, input: AwarenessSync) -> Result<AwarenessSyncResult, StoreError>;
    async fn get_node(&self, id: &str) -> Result<Option<AwarenessNode>, StoreError>;
    async fn manifest(&self, filter: &AwarenessFilter) -> Result<AwarenessManifest, StoreError>;
    async fn traverse(
        &self,
        filter: &AwarenessTraversalFilter,
    ) -> Result<AwarenessTree, StoreError>;
}

// ── Librarian catalog ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibrarianNode {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub kind: LibrarianNodeKind,
    pub label: String,
    pub locator_kind: LocatorKind,
    pub locator: String,
    pub tags: Vec<String>,
    pub project_slug: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_read_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonicalized_at: Option<u64>,
    // Kind-specific payload (D6). The schema column is option<object>;
    // per-kind shapes (SourceTemplateData, ProjectNodeData) live in
    // daemon8-types and are validated at write time by
    // crates/store/src/librarian_validators.rs. Older rows without
    // this field stay valid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibrarianEdge {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub kind: LibrarianEdgeKind,
    pub from_node: String,
    pub to_node: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Default)]
pub struct LibrarianFilter {
    pub kinds: Option<Vec<LibrarianNodeKind>>,
    pub tags: Option<Vec<String>>,
    pub project_slug: Option<String>,
    pub text_match: Option<String>,
    pub limit: Option<usize>,
    pub include_deprecated: bool,
    pub stale_before: Option<u64>,
    pub parent_id: Option<String>,
}

#[async_trait::async_trait]
pub trait LibrarianStore: Send + Sync {
    async fn index_node(&self, node: LibrarianNode) -> Result<String, StoreError>;
    async fn index_edge(&self, edge: LibrarianEdge) -> Result<String, StoreError>;
    async fn lookup(&self, filter: &LibrarianFilter) -> Result<Vec<LibrarianNode>, StoreError>;
    async fn get_node(&self, id: &str) -> Result<Option<LibrarianNode>, StoreError>;
    async fn get_edges(&self, node_id: &str) -> Result<Vec<LibrarianEdge>, StoreError>;
    async fn get_children(&self, parent_id: &str) -> Result<Vec<LibrarianNode>, StoreError>;
    async fn touch_read(&self, id: &str) -> Result<(), StoreError>;
    async fn deprecate_node(&self, id: &str) -> Result<bool, StoreError>;
    async fn forget_node(&self, id: &str) -> Result<bool, StoreError>;
    async fn forget_edge(&self, id: &str) -> Result<bool, StoreError>;
}
