// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use rmcp::schemars;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One declared embedding generator. Vectors stored on memory rows must be
/// compared only against vectors produced by the same `EmbeddingProfile`;
/// mixing models silently corrupts semantic search. The vault doctrine in
/// `21-memory-tiers.md` MT-9 calls for one declared profile per project,
/// re-embedding any rows whose profile changes. This struct is the substrate
/// that records "which generator produced this vector".
///
/// `provider` is a free string (e.g. "openai", "local-onnx", "cohere") so
/// adding a new backend does not require an enum bump.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddingProfile {
    /// Stable id — typically `<provider>:<model>` so callers can reference
    /// it without round-tripping through the store. Stored as the SurrealDB
    /// record id.
    pub id: String,
    pub provider: String,
    pub model: String,
    pub dimensions: u32,
    pub created_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_profile_roundtrip() {
        let p = EmbeddingProfile {
            id: "openai:text-embedding-3-small".into(),
            provider: "openai".into(),
            model: "text-embedding-3-small".into(),
            dimensions: 1536,
            created_at: 1_700_000_000_000,
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: EmbeddingProfile = serde_json::from_str(&s).unwrap();
        assert_eq!(back, p);
    }
}
