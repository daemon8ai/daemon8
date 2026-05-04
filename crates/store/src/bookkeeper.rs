// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC
//
// Bookkeeper substrate. Two operations:
//
// `sweep_short` enforces the vault's MT-2 nightly TTL on `memory_short` --
// expired rows (`expires_at < now`) are reaped, optionally narrowed to a
// single agent.
//
// `dedupe_long` collapses exact `content_hash` collisions on `memory_long`
// within a scope, keeping the highest-confidence entry (ties broken by
// oldest `created_at`). This is content-equality dedup; embedding-aware
// semantic dedup at promotion time is MVP-09 work.
//
// Both operations support dry-run (`apply: false`) so callers can preview
// before mutating.

use std::collections::HashMap;

use surrealdb::Surreal;
use surrealdb::engine::local::Db;

use crate::{
    BookkeeperStore, DedupLongScope, DedupReport, MemoryLongRecord, MemoryShortRecord, StoreError,
    SweepReport, SweepShortScope,
};

pub struct SurrealBookkeeperStore {
    db: Surreal<Db>,
}

impl SurrealBookkeeperStore {
    pub fn new(db: Surreal<Db>) -> Self {
        Self { db }
    }
}

fn current_unix_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(u64::MAX)
}

fn strip_table_prefix(raw: &str) -> &str {
    raw.split_once(':')
        .map_or(raw, |(_, id)| id)
        .trim_matches('`')
}

fn extract_record_id(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::String(s) => Some(strip_table_prefix(s).to_string()),
        serde_json::Value::Object(obj) => {
            let id_field = obj.get("id")?;
            match id_field {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Object(inner) => {
                    inner.get("String")?.as_str().map(|s| s.to_string())
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn decode_short(mut val: serde_json::Value) -> Result<MemoryShortRecord, StoreError> {
    if let Some(id_val) = val.get("id")
        && let Some(id) = extract_record_id(id_val)
    {
        val["id"] = serde_json::Value::String(id);
    }
    serde_json::from_value(val).map_err(StoreError::from)
}

fn decode_long(mut val: serde_json::Value) -> Result<MemoryLongRecord, StoreError> {
    if let Some(id_val) = val.get("id")
        && let Some(id) = extract_record_id(id_val)
    {
        val["id"] = serde_json::Value::String(id);
    }
    serde_json::from_value(val).map_err(StoreError::from)
}

#[async_trait::async_trait]
impl BookkeeperStore for SurrealBookkeeperStore {
    async fn sweep_short(&self, scope: SweepShortScope) -> Result<SweepReport, StoreError> {
        let now = scope.now_ns.unwrap_or_else(current_unix_nanos);

        let (sql, binds): (&str, Vec<(String, serde_json::Value)>) = match &scope.agent_id {
            Some(agent) => (
                "SELECT * FROM memory_short WHERE expires_at <= $now AND agent_id = $agent ORDER BY created_at ASC",
                vec![
                    ("now".into(), serde_json::json!(now)),
                    ("agent".into(), serde_json::json!(agent)),
                ],
            ),
            None => (
                "SELECT * FROM memory_short WHERE expires_at <= $now ORDER BY created_at ASC",
                vec![("now".into(), serde_json::json!(now))],
            ),
        };

        let mut query = self.db.query(sql);
        for (name, value) in binds {
            query = query.bind((name, value));
        }
        let mut result = query
            .await
            .map_err(|e| StoreError::Db(format!("sweep_short select: {e}")))?;
        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("sweep_short select read: {e}")))?;

        let expired: Vec<MemoryShortRecord> = rows
            .into_iter()
            .map(decode_short)
            .collect::<Result<_, _>>()?;

        let mut total_count: usize = 0;
        let total_sql: &str = match scope.agent_id {
            Some(_) => {
                "SELECT count() AS total FROM memory_short WHERE agent_id = $agent GROUP ALL"
            }
            None => "SELECT count() AS total FROM memory_short GROUP ALL",
        };
        let mut total_q = self.db.query(total_sql);
        if let Some(ref agent) = scope.agent_id {
            total_q = total_q.bind(("agent", serde_json::json!(agent)));
        }
        let mut total_result = total_q
            .await
            .map_err(|e| StoreError::Db(format!("sweep_short count: {e}")))?;
        let total_row: Option<serde_json::Value> = total_result
            .take(0)
            .map_err(|e| StoreError::Db(format!("sweep_short count read: {e}")))?;
        if let Some(v) = total_row
            && let Some(t) = v.get("total").and_then(|t| t.as_u64())
        {
            total_count = t as usize;
        }

        let mut report = SweepReport {
            examined: total_count,
            expired: expired.len(),
            deleted_ids: Vec::new(),
        };

        if scope.apply {
            for row in &expired {
                let id = row
                    .id
                    .as_deref()
                    .ok_or_else(|| StoreError::Db("sweep_short: row missing id".into()))?;
                self.db
                    .query("DELETE type::record('memory_short', $id)")
                    .bind(("id", serde_json::json!(id)))
                    .await
                    .map_err(|e| StoreError::Db(format!("sweep_short delete: {e}")))?
                    .check()
                    .map_err(|e| StoreError::Db(format!("sweep_short delete check: {e}")))?;
                report.deleted_ids.push(id.to_string());
            }
        }

        Ok(report)
    }

    async fn dedupe_long(&self, scope: DedupLongScope) -> Result<DedupReport, StoreError> {
        let (sql, binds): (&str, Vec<(String, serde_json::Value)>) = match scope.scope {
            Some(s) => (
                "SELECT * FROM memory_long WHERE scope = $scope_v AND revoked_at IS NONE \
                 ORDER BY created_at ASC",
                vec![("scope_v".into(), serde_json::json!(s.to_string()))],
            ),
            None => (
                "SELECT * FROM memory_long WHERE revoked_at IS NONE ORDER BY created_at ASC",
                Vec::new(),
            ),
        };

        let mut query = self.db.query(sql);
        for (name, value) in binds {
            query = query.bind((name, value));
        }
        let mut result = query
            .await
            .map_err(|e| StoreError::Db(format!("dedupe_long select: {e}")))?;
        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("dedupe_long select read: {e}")))?;

        let candidates: Vec<MemoryLongRecord> = rows
            .into_iter()
            .map(decode_long)
            .collect::<Result<_, _>>()?;

        // Group by content_hash. When the caller did not narrow by scope, we
        // still respect tier doctrine and keep groups bucketed per
        // content_hash + scope so cross-scope rows never dedup against each
        // other.
        let mut groups: HashMap<(String, String), Vec<MemoryLongRecord>> = HashMap::new();
        for row in candidates.iter() {
            groups
                .entry((row.content_hash.clone(), row.scope.to_string()))
                .or_default()
                .push(row.clone());
        }

        let mut report = DedupReport {
            examined: candidates.len(),
            groups: 0,
            redundant: 0,
            removed_ids: Vec::new(),
        };

        for ((_hash, _scope), members) in groups {
            if members.len() < 2 {
                continue;
            }
            report.groups += 1;

            // Keep highest confidence; ties break to oldest created_at.
            // Iterating after sorting keeps the implementation obvious.
            let mut sorted = members.clone();
            sorted.sort_by(|a, b| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.created_at.cmp(&b.created_at))
            });
            let keeper_id = sorted[0]
                .id
                .clone()
                .ok_or_else(|| StoreError::Db("dedupe_long: row missing id".into()))?;

            for redundant in sorted.into_iter().skip(1) {
                let rid = redundant
                    .id
                    .ok_or_else(|| StoreError::Db("dedupe_long: row missing id".into()))?;
                if rid == keeper_id {
                    continue;
                }
                report.redundant += 1;
                if scope.apply {
                    self.db
                        .query("DELETE type::record('memory_long', $id)")
                        .bind(("id", serde_json::json!(rid)))
                        .await
                        .map_err(|e| StoreError::Db(format!("dedupe_long delete: {e}")))?
                        .check()
                        .map_err(|e| StoreError::Db(format!("dedupe_long delete check: {e}")))?;
                    report.removed_ids.push(rid);
                }
            }
        }

        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        LongContentKind, MemoryLongRecord, MemoryLongStore, MemoryScope, MemoryShortRecord,
        MemoryShortStore, ShortContentKind, SurrealStore,
    };

    async fn setup() -> (
        SurrealStore,
        crate::SurrealMemoryShortStore,
        crate::SurrealMemoryLongStore,
        SurrealBookkeeperStore,
    ) {
        let store = SurrealStore::memory().await.unwrap();
        let short = store.memory_short_store();
        short.init_schema().await.unwrap();
        let long = store.memory_long_store();
        long.init_schema().await.unwrap();
        let bk = store.bookkeeper_store();
        (store, short, long, bk)
    }

    fn short(agent: &str, content: &str, created: u64, expires: u64) -> MemoryShortRecord {
        MemoryShortRecord {
            id: None,
            content: content.into(),
            content_kind: ShortContentKind::Fact,
            agent_id: agent.into(),
            thread_id: None,
            scope: MemoryScope::Agent,
            tags: vec![],
            content_hash: format!("h-{content}"),
            expires_at: expires,
            created_at: created,
            updated_at: created,
            embedding: None,
            embedding_profile_id: None,
        }
    }

    fn long(content: &str, hash: &str, confidence: f32, created: u64) -> MemoryLongRecord {
        MemoryLongRecord {
            id: None,
            content: content.into(),
            content_kind: LongContentKind::Fact,
            scope: MemoryScope::Project,
            tags: vec![],
            content_hash: hash.into(),
            provenance: vec![],
            confidence,
            supersedes: None,
            revoked_at: None,
            created_at: created,
            updated_at: created,
            embedding: None,
            embedding_profile_id: None,
        }
    }

    // ---------- sweep_short ----------

    #[tokio::test]
    async fn sweep_short_empty_store_zero_report() {
        let (_s, _sh, _lo, bk) = setup().await;
        let report = bk
            .sweep_short(SweepShortScope {
                now_ns: Some(1_000),
                apply: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(report.examined, 0);
        assert_eq!(report.expired, 0);
        assert!(report.deleted_ids.is_empty());
    }

    #[tokio::test]
    async fn sweep_short_no_expired_rows_unchanged() {
        let (_s, sh, _lo, bk) = setup().await;
        sh.save(short("a", "x", 1, 999_999)).await.unwrap();
        sh.save(short("a", "y", 2, 999_999)).await.unwrap();
        sh.save(short("a", "z", 3, 999_999)).await.unwrap();

        let report = bk
            .sweep_short(SweepShortScope {
                now_ns: Some(500),
                apply: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(report.examined, 3);
        assert_eq!(report.expired, 0);
        assert!(report.deleted_ids.is_empty());
    }

    #[tokio::test]
    async fn sweep_short_dry_run_reports_without_deleting() {
        let (_s, sh, _lo, bk) = setup().await;
        sh.save(short("a", "live", 1, 1_000)).await.unwrap();
        sh.save(short("a", "dead1", 2, 50)).await.unwrap();
        sh.save(short("a", "dead2", 3, 60)).await.unwrap();

        let report = bk
            .sweep_short(SweepShortScope {
                now_ns: Some(500),
                apply: false,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(report.examined, 3);
        assert_eq!(report.expired, 2);
        assert!(report.deleted_ids.is_empty());

        // Rows must still be there.
        let listed = sh
            .list(&crate::MemoryShortFilter {
                include_expired: true,
                now_ns: Some(0),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(listed.len(), 3);
    }

    #[tokio::test]
    async fn sweep_short_apply_deletes_expired_only() {
        let (_s, sh, _lo, bk) = setup().await;
        sh.save(short("a", "live", 1, 1_000)).await.unwrap();
        sh.save(short("a", "dead1", 2, 50)).await.unwrap();
        sh.save(short("a", "dead2", 3, 60)).await.unwrap();

        let report = bk
            .sweep_short(SweepShortScope {
                now_ns: Some(500),
                apply: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(report.expired, 2);
        assert_eq!(report.deleted_ids.len(), 2);

        let remaining = sh
            .list(&crate::MemoryShortFilter {
                include_expired: true,
                now_ns: Some(0),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].content, "live");
    }

    #[tokio::test]
    async fn sweep_short_with_agent_filter_only_touches_that_agent() {
        let (_s, sh, _lo, bk) = setup().await;
        sh.save(short("alice", "ax-dead", 1, 50)).await.unwrap();
        sh.save(short("bob", "bx-dead", 2, 50)).await.unwrap();

        let report = bk
            .sweep_short(SweepShortScope {
                agent_id: Some("alice".into()),
                now_ns: Some(500),
                apply: true,
            })
            .await
            .unwrap();
        assert_eq!(report.expired, 1);
        assert_eq!(report.deleted_ids.len(), 1);

        let remaining = sh
            .list(&crate::MemoryShortFilter {
                include_expired: true,
                now_ns: Some(0),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].agent_id, "bob");
    }

    // ---------- dedupe_long ----------

    #[tokio::test]
    async fn dedupe_long_empty_store_zero_report() {
        let (_s, _sh, _lo, bk) = setup().await;
        let report = bk
            .dedupe_long(DedupLongScope {
                apply: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(report.examined, 0);
        assert_eq!(report.groups, 0);
        assert_eq!(report.redundant, 0);
    }

    #[tokio::test]
    async fn dedupe_long_no_collisions_no_groups() {
        let (_s, _sh, lo, bk) = setup().await;
        lo.save(long("a", "h-a", 0.5, 1)).await.unwrap();
        lo.save(long("b", "h-b", 0.5, 2)).await.unwrap();
        lo.save(long("c", "h-c", 0.5, 3)).await.unwrap();

        let report = bk
            .dedupe_long(DedupLongScope {
                apply: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(report.examined, 3);
        assert_eq!(report.groups, 0);
        assert_eq!(report.redundant, 0);
    }

    #[tokio::test]
    async fn dedupe_long_dry_run_reports_without_deleting() {
        let (_s, _sh, lo, bk) = setup().await;
        lo.save(long("a", "shared", 0.5, 1)).await.unwrap();
        lo.save(long("a", "shared", 0.5, 2)).await.unwrap();
        lo.save(long("a", "shared", 0.5, 3)).await.unwrap();

        let report = bk
            .dedupe_long(DedupLongScope {
                apply: false,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(report.examined, 3);
        assert_eq!(report.groups, 1);
        assert_eq!(report.redundant, 2);
        assert!(report.removed_ids.is_empty());

        let listed = lo.list(&crate::MemoryLongFilter::default()).await.unwrap();
        assert_eq!(listed.len(), 3, "dry-run must not delete");
    }

    #[tokio::test]
    async fn dedupe_long_keeps_highest_confidence_then_oldest() {
        let (_s, _sh, lo, bk) = setup().await;
        let low_old = lo.save(long("a", "h", 0.3, 1)).await.unwrap();
        let high_middle = lo.save(long("a", "h", 0.9, 2)).await.unwrap();
        let high_late = lo.save(long("a", "h", 0.9, 3)).await.unwrap();

        let report = bk
            .dedupe_long(DedupLongScope {
                apply: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(report.groups, 1);
        assert_eq!(report.redundant, 2);

        let remaining = lo.list(&crate::MemoryLongFilter::default()).await.unwrap();
        assert_eq!(remaining.len(), 1);
        let kept = &remaining[0];
        // The 0.9-confidence entry created earliest wins.
        assert_eq!(kept.id.as_deref(), Some(high_middle.as_str()));
        // Both losers were deleted.
        assert!(report.removed_ids.contains(&low_old));
        assert!(report.removed_ids.contains(&high_late));
    }

    #[tokio::test]
    async fn dedupe_long_respects_scope_isolation() {
        let (_s, _sh, lo, bk) = setup().await;
        let mut a1 = long("a", "h", 0.5, 1);
        a1.scope = MemoryScope::Agent;
        let mut a2 = long("a", "h", 0.5, 2);
        a2.scope = MemoryScope::Agent;
        let mut p1 = long("a", "h", 0.5, 3);
        p1.scope = MemoryScope::Project;
        lo.save(a1).await.unwrap();
        lo.save(a2).await.unwrap();
        lo.save(p1).await.unwrap();

        // Without a scope filter, dedup still buckets per (hash, scope) so
        // Agent rows collapse but the Project row is untouched.
        let report = bk
            .dedupe_long(DedupLongScope {
                apply: true,
                scope: None,
            })
            .await
            .unwrap();
        assert_eq!(
            report.groups, 1,
            "only the Agent-scope group has duplicates"
        );
        assert_eq!(report.redundant, 1);

        let all = lo.list(&crate::MemoryLongFilter::default()).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn dedupe_long_scope_filter_limits_what_is_examined() {
        let (_s, _sh, lo, bk) = setup().await;
        let mut a1 = long("a", "h", 0.5, 1);
        a1.scope = MemoryScope::Agent;
        let mut a2 = long("a", "h", 0.5, 2);
        a2.scope = MemoryScope::Agent;
        let mut p1 = long("a", "h", 0.5, 3);
        p1.scope = MemoryScope::Project;
        let mut p2 = long("a", "h", 0.5, 4);
        p2.scope = MemoryScope::Project;
        lo.save(a1).await.unwrap();
        lo.save(a2).await.unwrap();
        lo.save(p1).await.unwrap();
        lo.save(p2).await.unwrap();

        let project_only = bk
            .dedupe_long(DedupLongScope {
                apply: true,
                scope: Some(MemoryScope::Project),
            })
            .await
            .unwrap();
        assert_eq!(project_only.examined, 2);
        assert_eq!(project_only.groups, 1);
        assert_eq!(project_only.redundant, 1);

        // Agent rows untouched.
        let remaining = lo.list(&crate::MemoryLongFilter::default()).await.unwrap();
        let agents = remaining
            .iter()
            .filter(|r| r.scope == MemoryScope::Agent)
            .count();
        assert_eq!(agents, 2);
    }

    #[tokio::test]
    async fn dedupe_long_skips_revoked_rows() {
        let (_s, _sh, lo, bk) = setup().await;
        let alive_a = lo.save(long("a", "h", 0.9, 1)).await.unwrap();
        let alive_b = lo.save(long("a", "h", 0.5, 2)).await.unwrap();
        let revoked = lo.save(long("a", "h", 0.99, 3)).await.unwrap();
        lo.revoke(&revoked, 50).await.unwrap();

        let report = bk
            .dedupe_long(DedupLongScope {
                apply: true,
                ..Default::default()
            })
            .await
            .unwrap();
        // Examined excludes the revoked row; the alive duplicates collapse.
        assert_eq!(report.examined, 2);
        assert_eq!(report.groups, 1);
        assert_eq!(report.redundant, 1);
        assert!(report.removed_ids.contains(&alive_b));
        assert!(!report.removed_ids.contains(&alive_a));
        assert!(!report.removed_ids.contains(&revoked));
    }
}
