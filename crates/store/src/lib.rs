// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

pub mod card;
pub mod envelope;
mod lens;
pub mod memory;
mod surreal;

pub use card::SurrealCardStore;
pub use envelope::SurrealEnvelopeStore;
pub use lens::{LensManager, LensStatus};
pub use memory::SurrealMemoryStore;
pub use surreal::SurrealStore;

use daemon8_types::{
    ActorCard, AgentCard, AgentStatus, Checkpoint, EnvelopeKind, EnvelopePriority, EnvelopeRecord,
    EnvelopeStatus, Filter, MemoryKind, Observation, ProjectCard, RuntimeSummary, StateSlice,
    TeamCard, UserCard,
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
