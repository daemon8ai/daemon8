// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Error fingerprinting: turn a noisy error message + stack trace into a
//! stable, normalized fingerprint and a 16-character hex hash.
//!
//! The point is recurrence detection. Two errors with different timestamps,
//! line numbers, request IDs, or temp paths but the same shape should produce
//! the same hash so we can answer "have I seen this exact failure before?"
//! with one indexed lookup.

use daemon8_types::{Observation, ObservationKind};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

use crate::{Memory, MemoryFilter, MemoryStore, StoreError};

/// 16 hex characters from a SHA-256 of the normalized fingerprint.
/// 64 bits of collision space — fine for human-scale recurrence detection;
/// this is not a cryptographic identifier.
pub fn hash_error(normalized: &str) -> String {
    let digest = Sha256::digest(normalized.as_bytes());
    digest
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

/// Pull a normalize-able text representation from an observation that's
/// recognizably an error. Returns None for non-error kinds.
pub fn extract_error_text(obs: &Observation) -> Option<String> {
    match &obs.kind {
        ObservationKind::Exception { message, trace } => Some(combine(message, trace.as_deref())),
        ObservationKind::JsException { message, .. } => Some(message.clone()),
        // Promote any error-severity observation whose data carries an
        // explicit message field — covers app logs that report errors via
        // the generic Log kind.
        _ if obs.severity == daemon8_types::Severity::Error => obs
            .data
            .get("message")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                obs.data
                    .get("error")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            }),
        _ => None,
    }
}

fn combine(message: &str, trace: Option<&str>) -> String {
    match trace {
        Some(t) => format!("{message}\n{t}"),
        None => message.to_string(),
    }
}

/// Strip volatile tokens (numbers, hex, UUIDs, paths) so structurally
/// identical errors produce identical fingerprints.
///
/// Order matters: UUIDs first (they look like hex+digits), then absolute
/// paths (they contain digits), then long hex runs, then bare integers.
pub fn normalize_error_text(raw: &str) -> String {
    static UUID_RE: OnceLock<Regex> = OnceLock::new();
    static PATH_RE: OnceLock<Regex> = OnceLock::new();
    static HEX_RE: OnceLock<Regex> = OnceLock::new();
    static NUM_RE: OnceLock<Regex> = OnceLock::new();
    static WS_RE: OnceLock<Regex> = OnceLock::new();

    let uuid = UUID_RE.get_or_init(|| {
        Regex::new(
            r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b",
        )
        .expect("uuid regex")
    });
    let path = PATH_RE.get_or_init(|| {
        // Posix absolute paths with at least one segment, optional :line:col tail
        Regex::new(r"(/[\w.\-+]+){2,}(:\d+(:\d+)?)?").expect("path regex")
    });
    let hex = HEX_RE.get_or_init(|| Regex::new(r"\b(0x)?[0-9a-fA-F]{8,}\b").expect("hex regex"));
    // `\d+` (no word boundaries) so digit runs adjacent to letters
    // (e.g. "5000ms", "v12") are still normalized to <NUM>.
    let num = NUM_RE.get_or_init(|| Regex::new(r"\d+").expect("num regex"));
    let ws = WS_RE.get_or_init(|| Regex::new(r"\s+").expect("ws regex"));

    let mut s = uuid.replace_all(raw, "<UUID>").into_owned();
    s = path.replace_all(&s, "<PATH>").into_owned();
    s = hex.replace_all(&s, "<HEX>").into_owned();
    s = num.replace_all(&s, "<NUM>").into_owned();
    s = ws.replace_all(&s, " ").into_owned();
    s.trim().to_string()
}

/// First-sight or repeat? If a Memory of kind ErrorSignature with this hash
/// exists in `project_slug`, bump its `seen_count` (in `confidence` we don't
/// track count today; we use a tag `seen:N` rotation to avoid widening the
/// Memory schema in v0.3). On first sight, write a new Memory.
///
/// Returns the memory id (existing or new).
pub async fn promote_error_signature(
    memory_store: &dyn MemoryStore,
    hash: &str,
    normalized_text: &str,
    project_slug: &str,
    observation_id: u64,
    now_ns: u64,
) -> Result<String, StoreError> {
    let hash_tag = format!("hash:{hash}");
    let filter = MemoryFilter {
        kinds: Some(vec![daemon8_types::MemoryKind::ErrorSignature]),
        tags: Some(vec![hash_tag.clone()]),
        project_slug: Some(project_slug.to_string()),
        session_id: None,
        text_match: None,
        limit: Some(1),
    };
    let existing = memory_store.query_memory(&filter).await?;

    if let Some(mut mem) = existing.into_iter().next() {
        // Bump observation list (cap to last 50 to avoid unbounded growth)
        if !mem.source_observations.contains(&observation_id) {
            mem.source_observations.push(observation_id);
            if mem.source_observations.len() > 50 {
                let drop = mem.source_observations.len() - 50;
                mem.source_observations.drain(0..drop);
            }
        }
        // seen_count carried as a tag we replace
        let mut next_count: u64 = 1;
        mem.tags.retain(|t| {
            if let Some(rest) = t.strip_prefix("seen:") {
                if let Ok(n) = rest.parse::<u64>() {
                    next_count = n + 1;
                }
                false
            } else {
                true
            }
        });
        mem.tags.push(format!("seen:{next_count}"));
        mem.updated_at = now_ns;
        let id = mem
            .id
            .clone()
            .ok_or_else(|| StoreError::Db("error signature missing id".into()))?;
        memory_store.update_memory(mem).await?;
        return Ok(id);
    }

    let mem = Memory {
        id: None,
        created_at: now_ns,
        updated_at: now_ns,
        kind: daemon8_types::MemoryKind::ErrorSignature,
        content: normalized_text.to_string(),
        source_observations: vec![observation_id],
        tags: vec![hash_tag, "seen:1".to_string()],
        project_slug: project_slug.to_string(),
        session_id: None,
        confidence: 1.0,
        data: None,
    };
    memory_store.save_memory(mem).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SurrealStore;
    use daemon8_types::{AppName, Observation, ObservationKind, Origin, Severity, SourceLocation};

    fn make_error_obs(message: &str, trace: Option<&str>) -> Observation {
        Observation {
            id: 0,
            origin: Origin::Application {
                name: AppName::from("daemon8"),
            },
            kind: ObservationKind::Exception {
                message: message.into(),
                trace: trace.map(String::from),
            },
            data: serde_json::json!({}),
            severity: Severity::Error,
            source_location: Some(SourceLocation {
                file: "src/foo.rs".into(),
                line: 42,
                function: None,
            }),
            timestamp_ns: 0,
            correlation_id: None,
            parent_id: None,
            tags: None,
            session_id: None,
            node_id: None,
            debug_session_id: None,
            checkpoint_id: None,
            error_hash: None,
        }
    }

    #[test]
    fn normalizes_numbers() {
        let n = normalize_error_text("connection refused on port 12345 to host x");
        assert_eq!(n, "connection refused on port <NUM> to host x");
    }

    #[test]
    fn normalizes_uuids() {
        let n = normalize_error_text("request 550e8400-e29b-41d4-a716-446655440000 failed");
        assert_eq!(n, "request <UUID> failed");
    }

    #[test]
    fn normalizes_paths() {
        let n = normalize_error_text("file /Users/foo/bar/baz.rs:42:11 not found");
        assert_eq!(n, "file <PATH> not found");
    }

    #[test]
    fn normalizes_hex_addresses() {
        let n = normalize_error_text("segfault at 0xdeadbeef in module");
        assert_eq!(n, "segfault at <HEX> in module");
    }

    #[test]
    fn collapses_whitespace() {
        let n = normalize_error_text("error\n\tat foo\n\tat bar");
        assert_eq!(n, "error at foo at bar");
    }

    #[test]
    fn structural_collisions_produce_same_hash() {
        let a = normalize_error_text("connection refused on port 8080 at 0xaabbccdd in worker 7");
        let b = normalize_error_text("connection refused on port 9999 at 0xddeeff00 in worker 12");
        assert_eq!(a, b);
        assert_eq!(hash_error(&a), hash_error(&b));
    }

    #[test]
    fn distinct_errors_produce_distinct_hashes() {
        let a = hash_error(&normalize_error_text("DB connection refused"));
        let b = hash_error(&normalize_error_text("Permission denied for file"));
        assert_ne!(a, b);
    }

    #[test]
    fn extracts_text_from_exception_kind() {
        let obs = make_error_obs("boom", Some("at frame 1"));
        let text = extract_error_text(&obs).unwrap();
        assert!(text.contains("boom"));
        assert!(text.contains("at frame 1"));
    }

    #[test]
    fn extracts_nothing_from_log_info() {
        let mut obs = make_error_obs("boom", None);
        obs.kind = ObservationKind::Log;
        obs.severity = Severity::Info;
        assert!(extract_error_text(&obs).is_none());
    }

    #[test]
    fn extracts_message_from_error_log() {
        let mut obs = make_error_obs("ignored", None);
        obs.kind = ObservationKind::Log;
        obs.severity = Severity::Error;
        obs.data = serde_json::json!({"message": "disk full"});
        let text = extract_error_text(&obs).unwrap();
        assert_eq!(text, "disk full");
    }

    #[tokio::test]
    async fn promote_first_sight_creates_memory() {
        let store = SurrealStore::memory().await.unwrap();
        let memory_store = store.memory_store();

        let normalized = "connection refused on port <NUM>";
        let hash = hash_error(normalized);
        let id = promote_error_signature(&memory_store, &hash, normalized, "daemon8", 1, 100)
            .await
            .unwrap();
        assert!(!id.is_empty());

        let mem = memory_store.get_memory(&id).await.unwrap().unwrap();
        assert_eq!(mem.content, normalized);
        assert_eq!(mem.kind, daemon8_types::MemoryKind::ErrorSignature);
        assert!(mem.tags.contains(&format!("hash:{hash}")));
        assert!(mem.tags.contains(&"seen:1".to_string()));
        assert_eq!(mem.source_observations, vec![1]);
    }

    #[tokio::test]
    async fn promote_repeat_bumps_seen_and_observations() {
        let store = SurrealStore::memory().await.unwrap();
        let memory_store = store.memory_store();
        let normalized = "DB error <NUM>";
        let hash = hash_error(normalized);

        let id1 = promote_error_signature(&memory_store, &hash, normalized, "p1", 1, 100)
            .await
            .unwrap();
        let id2 = promote_error_signature(&memory_store, &hash, normalized, "p1", 2, 200)
            .await
            .unwrap();
        let id3 = promote_error_signature(&memory_store, &hash, normalized, "p1", 3, 300)
            .await
            .unwrap();

        assert_eq!(id1, id2);
        assert_eq!(id2, id3);
        let mem = memory_store.get_memory(&id1).await.unwrap().unwrap();
        assert_eq!(mem.source_observations, vec![1, 2, 3]);
        assert!(mem.tags.contains(&"seen:3".to_string()));
        assert_eq!(mem.updated_at, 300);
    }

    #[tokio::test]
    async fn promote_isolates_by_project_slug() {
        let store = SurrealStore::memory().await.unwrap();
        let memory_store = store.memory_store();
        let normalized = "shared error";
        let hash = hash_error(normalized);

        let id_a = promote_error_signature(&memory_store, &hash, normalized, "alpha", 1, 1)
            .await
            .unwrap();
        let id_b = promote_error_signature(&memory_store, &hash, normalized, "beta", 2, 2)
            .await
            .unwrap();
        assert_ne!(id_a, id_b);
    }
}
