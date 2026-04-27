// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

mod lens;
pub mod memory;
mod surreal;

pub use lens::{LensManager, LensStatus};
pub use memory::SurrealMemoryStore;
pub use surreal::SurrealStore;

use daemon8_types::{Checkpoint, Filter, MemoryKind, Observation, RuntimeSummary, StateSlice};
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
