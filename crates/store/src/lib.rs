// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

pub mod bookkeeper;
pub mod card;
pub mod embedding_profile;
pub mod envelope;
mod lens;
pub mod memory;
pub mod memory_long;
pub mod memory_reference;
pub mod memory_short;
mod surreal;

pub use bookkeeper::SurrealBookkeeperStore;
pub use card::SurrealCardStore;
pub use embedding_profile::SurrealEmbeddingProfileStore;
pub use envelope::SurrealEnvelopeStore;
pub use lens::{LensManager, LensStatus};
pub use memory::SurrealMemoryStore;
pub use memory_long::SurrealMemoryLongStore;
pub use memory_reference::SurrealMemoryReferenceStore;
pub use memory_short::SurrealMemoryShortStore;
pub use surreal::SurrealStore;

use daemon8_types::{
    ActorCard, AgentCard, AgentStatus, Checkpoint, EmbeddingProfile, EnvelopeKind,
    EnvelopePriority, EnvelopeRecord, EnvelopeStatus, Filter, LongContentKind, MemoryKind,
    MemoryScope, Observation, ProjectCard, ProvenanceEntry, ReferenceSourceKind, RuntimeSummary,
    ShortContentKind, StateSlice, TeamCard, UserCard,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub kind: MemoryKind,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    pub source_observations: Vec<u64>,
    pub tags: Vec<String>,
    pub project_slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub confidence: f64,
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

#[derive(Debug, Clone, Default)]
pub struct AgentCardFilter {
    pub statuses: Option<Vec<AgentStatus>>,
    pub project_ref: Option<String>,
    pub team_ref: Option<String>,
    pub limit: Option<usize>,
}

#[async_trait::async_trait]
pub trait CardStore: Send + Sync {
    async fn init_schema(&self) -> Result<(), StoreError>;

    async fn upsert_actor(&self, card: ActorCard) -> Result<(), StoreError>;
    async fn get_actor_by_address(&self, address: &str) -> Result<Option<ActorCard>, StoreError>;
    async fn list_actors(&self) -> Result<Vec<ActorCard>, StoreError>;

    async fn upsert_user(&self, card: UserCard) -> Result<(), StoreError>;
    async fn get_user_by_address(&self, address: &str) -> Result<Option<UserCard>, StoreError>;

    async fn upsert_agent(&self, card: AgentCard) -> Result<(), StoreError>;
    async fn get_agent_by_slug(&self, slug: &str) -> Result<Option<AgentCard>, StoreError>;
    async fn list_agents(&self, filter: &AgentCardFilter) -> Result<Vec<AgentCard>, StoreError>;
    async fn update_agent_status(
        &self,
        id: &str,
        status: AgentStatus,
        updated_at: u64,
    ) -> Result<(), StoreError>;
    async fn record_agent_heartbeat(&self, id: &str, seen_at: u64) -> Result<(), StoreError>;

    async fn upsert_project(&self, card: ProjectCard) -> Result<(), StoreError>;
    async fn get_project_by_slug(&self, slug: &str) -> Result<Option<ProjectCard>, StoreError>;

    async fn upsert_team(&self, card: TeamCard) -> Result<(), StoreError>;
    async fn get_team_by_slug(
        &self,
        project_ref: Option<&str>,
        slug: &str,
    ) -> Result<Option<TeamCard>, StoreError>;
}

#[derive(Debug, Clone, Default)]
pub struct EnvelopeFilter {
    pub inbox_address: Option<String>,
    pub to_address: Option<String>,
    pub from_address: Option<String>,
    pub statuses: Option<Vec<EnvelopeStatus>>,
    pub kinds: Option<Vec<EnvelopeKind>>,
    pub priorities: Option<Vec<EnvelopePriority>>,
    pub tags: Option<Vec<String>>,
    pub project_refs: Option<Vec<String>>,
    pub team_refs: Option<Vec<String>>,
    pub correlation_id: Option<String>,
    pub thread_id: Option<String>,
    pub since_ns: Option<u64>,
    pub limit: Option<usize>,
}

// ----------------------------------------------------------------------------
// Embedding profile registry (MVP-09 substrate)
//
// Vectors stored on memory rows must be compared only against vectors
// produced by the same `EmbeddingProfile`. The store records "which
// generator produced which vector" so a future semantic-search layer can
// gate KNN queries on profile id.
// ----------------------------------------------------------------------------

#[async_trait::async_trait]
pub trait EmbeddingProfileStore: Send + Sync {
    async fn init_schema(&self) -> Result<(), StoreError>;
    async fn upsert(&self, profile: EmbeddingProfile) -> Result<String, StoreError>;
    async fn get(&self, id: &str) -> Result<Option<EmbeddingProfile>, StoreError>;
    async fn list(&self) -> Result<Vec<EmbeddingProfile>, StoreError>;
    async fn find_by_provider_and_model(
        &self,
        provider: &str,
        model: &str,
    ) -> Result<Option<EmbeddingProfile>, StoreError>;
    async fn delete(&self, id: &str) -> Result<bool, StoreError>;
}

// ----------------------------------------------------------------------------
// Memory tier records and stores
//
// These three structs back the `memory_short`, `memory_reference`, and
// `memory_long` tables defined in
// `50-projects/daemon8/poc/deliber8/21-memory-tiers.md`. The legacy `Memory`
// struct above continues to back the original `memory` table used by MCP
// `save_memory` / `query_memory` and the CLI `memory query` surface; the new
// tier tables coexist with it until a separate migration round wires the MCP
// surface over to the tier model.
//
// Embedding columns (`embedding`, `embedding_profile_id`) landed in MVP-09 as
// optional fields. Vectors must only be searched within an exact
// `embedding_profile_id` -- mixing models silently corrupts retrieval. The
// `EmbeddingProfile` table records which generator produced which vector.
// Actual semantic search (KNN over an mtree index) is deferred to a later
// round; this substrate just stores the columns and lets callers filter.
// ----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryShortRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub content: String,
    pub content_kind: ShortContentKind,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub scope: MemoryScope,
    #[serde(default)]
    pub tags: Vec<String>,
    pub content_hash: String,
    pub expires_at: u64,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_profile_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MemoryShortFilter {
    pub agent_id: Option<String>,
    pub thread_id: Option<String>,
    pub scope: Option<MemoryScope>,
    pub content_kinds: Option<Vec<ShortContentKind>>,
    pub tags_any: Option<Vec<String>>,
    pub embedding_profile_id: Option<String>,
    pub include_expired: bool,
    pub now_ns: Option<u64>,
    pub limit: Option<usize>,
}

#[async_trait::async_trait]
pub trait MemoryShortStore: Send + Sync {
    async fn init_schema(&self) -> Result<(), StoreError>;
    async fn save(&self, record: MemoryShortRecord) -> Result<String, StoreError>;
    async fn get(&self, id: &str) -> Result<Option<MemoryShortRecord>, StoreError>;
    async fn list(&self, filter: &MemoryShortFilter) -> Result<Vec<MemoryShortRecord>, StoreError>;
    async fn delete(&self, id: &str) -> Result<bool, StoreError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryReferenceRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub content: String,
    pub source_kind: ReferenceSourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    pub source_hash: String,
    pub scope: MemoryScope,
    #[serde(default)]
    pub tags: Vec<String>,
    pub content_hash: String,
    pub refreshed_at: u64,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_profile_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MemoryReferenceFilter {
    pub source_kinds: Option<Vec<ReferenceSourceKind>>,
    pub scope: Option<MemoryScope>,
    pub tags_any: Option<Vec<String>>,
    pub embedding_profile_id: Option<String>,
    pub limit: Option<usize>,
}

#[async_trait::async_trait]
pub trait MemoryReferenceStore: Send + Sync {
    async fn init_schema(&self) -> Result<(), StoreError>;
    async fn save(&self, record: MemoryReferenceRecord) -> Result<String, StoreError>;
    async fn get(&self, id: &str) -> Result<Option<MemoryReferenceRecord>, StoreError>;
    async fn list(
        &self,
        filter: &MemoryReferenceFilter,
    ) -> Result<Vec<MemoryReferenceRecord>, StoreError>;
    async fn find_by_source_hash(
        &self,
        source_hash: &str,
    ) -> Result<Option<MemoryReferenceRecord>, StoreError>;
    async fn mark_refreshed(&self, id: &str, refreshed_at: u64) -> Result<(), StoreError>;
    async fn delete(&self, id: &str) -> Result<bool, StoreError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryLongRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub content: String,
    pub content_kind: LongContentKind,
    pub scope: MemoryScope,
    #[serde(default)]
    pub tags: Vec<String>,
    pub content_hash: String,
    #[serde(default)]
    pub provenance: Vec<ProvenanceEntry>,
    pub confidence: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<u64>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_profile_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MemoryLongFilter {
    pub scope: Option<MemoryScope>,
    pub content_kinds: Option<Vec<LongContentKind>>,
    pub tags_any: Option<Vec<String>>,
    pub embedding_profile_id: Option<String>,
    pub include_revoked: bool,
    pub limit: Option<usize>,
}

#[async_trait::async_trait]
pub trait MemoryLongStore: Send + Sync {
    async fn init_schema(&self) -> Result<(), StoreError>;
    async fn save(&self, record: MemoryLongRecord) -> Result<String, StoreError>;
    async fn get(&self, id: &str) -> Result<Option<MemoryLongRecord>, StoreError>;
    async fn list(&self, filter: &MemoryLongFilter) -> Result<Vec<MemoryLongRecord>, StoreError>;
    async fn find_by_content_hash(
        &self,
        content_hash: &str,
        scope: Option<MemoryScope>,
    ) -> Result<Vec<MemoryLongRecord>, StoreError>;
    async fn supersede(&self, old_id: &str, new_id: &str, at_ns: u64) -> Result<(), StoreError>;
    async fn revoke(&self, id: &str, at_ns: u64) -> Result<(), StoreError>;
    async fn delete(&self, id: &str) -> Result<bool, StoreError>;
}

// ----------------------------------------------------------------------------
// Bookkeeper substrate
//
// `sweep_short` enforces the vault's MT-2 nightly TTL on `memory_short`.
// `dedupe_long` collapses exact `content_hash` collisions on `memory_long`,
// keeping the highest-confidence entry (ties broken by oldest `created_at`).
// Embedding-aware semantic dedup at promotion time is MVP-09 work.
// ----------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct SweepShortScope {
    /// Restrict the sweep to a single agent's working memory. `None` sweeps
    /// every agent's expired short entries in one pass.
    pub agent_id: Option<String>,
    /// Optional injection point so tests can pin "now" and the runtime can
    /// quantize sweep cadence. `None` uses `SystemTime::now()`.
    pub now_ns: Option<u64>,
    /// `false` is dry-run; `true` performs the deletion.
    pub apply: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SweepReport {
    pub examined: usize,
    pub expired: usize,
    pub deleted_ids: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DedupLongScope {
    pub scope: Option<MemoryScope>,
    pub apply: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DedupReport {
    pub examined: usize,
    /// Distinct content_hashes in scope with at least two members.
    pub groups: usize,
    pub redundant: usize,
    pub removed_ids: Vec<String>,
}

#[async_trait::async_trait]
pub trait BookkeeperStore: Send + Sync {
    async fn sweep_short(&self, scope: SweepShortScope) -> Result<SweepReport, StoreError>;
    async fn dedupe_long(&self, scope: DedupLongScope) -> Result<DedupReport, StoreError>;
}

#[async_trait::async_trait]
pub trait EnvelopeStore: Send + Sync {
    async fn init_schema(&self) -> Result<(), StoreError>;
    async fn enqueue_envelope(&self, record: EnvelopeRecord) -> Result<String, StoreError>;
    async fn get_envelope(&self, id: &str) -> Result<Option<EnvelopeRecord>, StoreError>;
    async fn query_inbox(&self, filter: &EnvelopeFilter)
    -> Result<Vec<EnvelopeRecord>, StoreError>;
    async fn list_pending(
        &self,
        inbox_address: &str,
        now_ns: Option<u64>,
        limit: Option<usize>,
    ) -> Result<Vec<EnvelopeRecord>, StoreError>;
    async fn mark_delivered(&self, id: &str, at_ns: u64) -> Result<(), StoreError>;
    async fn mark_read(&self, id: &str, at_ns: u64) -> Result<(), StoreError>;
    async fn mark_failed(&self, id: &str, reason: &str, at_ns: u64) -> Result<(), StoreError>;
    async fn cancel_envelope(&self, id: &str, at_ns: u64) -> Result<(), StoreError>;
}
