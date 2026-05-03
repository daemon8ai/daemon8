// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use surrealdb::Surreal;
use surrealdb::engine::local::Db;

use crate::{EnvelopeFilter, EnvelopeRecord, EnvelopeStore, StoreError};

// Envelope schema lives here so both SurrealStore::init_schema (workspace
// bootstrap) and EnvelopeStore::init_schema (standalone construction) can
// run the same idempotent DDL. `IF NOT EXISTS` makes repeated execution safe.
pub(crate) const ENVELOPE_DDL: &str = "DEFINE TABLE IF NOT EXISTS envelope SCHEMAFULL;

DEFINE FIELD IF NOT EXISTS kind            ON envelope TYPE string;
DEFINE FIELD IF NOT EXISTS status          ON envelope TYPE string;
DEFINE FIELD IF NOT EXISTS priority        ON envelope TYPE string;
DEFINE FIELD IF NOT EXISTS from_address    ON envelope TYPE string;
DEFINE FIELD IF NOT EXISTS to_address      ON envelope TYPE string;
DEFINE FIELD IF NOT EXISTS inbox_address   ON envelope TYPE string;
DEFINE FIELD IF NOT EXISTS subject         ON envelope TYPE option<string>;
DEFINE FIELD IF NOT EXISTS body            ON envelope TYPE option<string>;
DEFINE FIELD IF NOT EXISTS payload         ON envelope TYPE option<object> FLEXIBLE;
DEFINE FIELD IF NOT EXISTS correlation_id  ON envelope TYPE option<string>;
DEFINE FIELD IF NOT EXISTS thread_id       ON envelope TYPE option<string>;
DEFINE FIELD IF NOT EXISTS reply_to        ON envelope TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at      ON envelope TYPE int;
DEFINE FIELD IF NOT EXISTS updated_at      ON envelope TYPE int;
DEFINE FIELD IF NOT EXISTS deliver_after   ON envelope TYPE option<int>;
DEFINE FIELD IF NOT EXISTS delivered_at    ON envelope TYPE option<int>;
DEFINE FIELD IF NOT EXISTS read_at         ON envelope TYPE option<int>;
DEFINE FIELD IF NOT EXISTS expires_at      ON envelope TYPE option<int>;
DEFINE FIELD IF NOT EXISTS failed_at       ON envelope TYPE option<int>;
DEFINE FIELD IF NOT EXISTS failure_reason  ON envelope TYPE option<string>;
DEFINE FIELD IF NOT EXISTS tags            ON envelope TYPE array<string>;
DEFINE FIELD IF NOT EXISTS project_refs    ON envelope TYPE array<string>;
DEFINE FIELD IF NOT EXISTS team_refs       ON envelope TYPE array<string>;

DEFINE INDEX IF NOT EXISTS idx_env_inbox_status_created
    ON envelope FIELDS inbox_address, status, created_at;
DEFINE INDEX IF NOT EXISTS idx_env_to_status      ON envelope FIELDS to_address, status;
DEFINE INDEX IF NOT EXISTS idx_env_correlation    ON envelope FIELDS correlation_id;
DEFINE INDEX IF NOT EXISTS idx_env_thread         ON envelope FIELDS thread_id;
DEFINE INDEX IF NOT EXISTS idx_env_deliver_after  ON envelope FIELDS deliver_after;
DEFINE INDEX IF NOT EXISTS idx_env_expires        ON envelope FIELDS expires_at;
DEFINE INDEX IF NOT EXISTS idx_env_project_refs   ON envelope FIELDS project_refs;
DEFINE INDEX IF NOT EXISTS idx_env_team_refs      ON envelope FIELDS team_refs;
DEFINE INDEX IF NOT EXISTS idx_env_tags           ON envelope FIELDS tags;";

pub struct SurrealEnvelopeStore {
    db: Surreal<Db>,
}

impl SurrealEnvelopeStore {
    pub fn new(db: Surreal<Db>) -> Self {
        Self { db }
    }

    // Differentiate "no rows updated" between not-found and wrong-state by
    // following up with a get. Only invoked on the failure path so the success
    // path remains a single round trip. `allowed` is the human-readable list of
    // statuses the caller would accept, used only to phrase the error message.
    async fn diagnose_no_rows(&self, id: &str, op: &str, allowed: &str) -> StoreError {
        match self.get_envelope(id).await {
            Ok(Some(env)) => StoreError::Other(format!(
                "envelope '{id}' is in state {} and cannot {op} (allowed: {allowed})",
                env.status
            )),
            Ok(None) => StoreError::Other(format!("envelope '{id}' not found")),
            Err(e) => e,
        }
    }

    // Builds a SELECT against the envelope table. `since_ns` is inclusive
    // (`created_at >= $since_ns`). When `filter.limit` is `None` the query is
    // unbounded — matching `MemoryStore::query_memory`'s contract; callers
    // working over large inboxes should always pass a limit.
    fn build_query_sql(filter: &EnvelopeFilter) -> (String, Vec<(String, serde_json::Value)>) {
        let mut conditions = Vec::new();
        let mut binds: Vec<(String, serde_json::Value)> = Vec::new();

        if let Some(ref addr) = filter.inbox_address {
            conditions.push("inbox_address = $inbox_addr".to_string());
            binds.push(("inbox_addr".into(), serde_json::json!(addr)));
        }

        if let Some(ref addr) = filter.to_address {
            conditions.push("to_address = $to_addr".to_string());
            binds.push(("to_addr".into(), serde_json::json!(addr)));
        }

        if let Some(ref addr) = filter.from_address {
            conditions.push("from_address = $from_addr".to_string());
            binds.push(("from_addr".into(), serde_json::json!(addr)));
        }

        if let Some(ref statuses) = filter.statuses
            && !statuses.is_empty()
        {
            let strs: Vec<String> = statuses.iter().map(|s| s.to_string()).collect();
            conditions.push("status IN $allowed_statuses".to_string());
            binds.push(("allowed_statuses".into(), serde_json::json!(strs)));
        }

        if let Some(ref kinds) = filter.kinds
            && !kinds.is_empty()
        {
            let strs: Vec<String> = kinds.iter().map(|k| k.to_string()).collect();
            conditions.push("kind IN $allowed_kinds".to_string());
            binds.push(("allowed_kinds".into(), serde_json::json!(strs)));
        }

        if let Some(ref priorities) = filter.priorities
            && !priorities.is_empty()
        {
            let strs: Vec<String> = priorities.iter().map(|p| p.to_string()).collect();
            conditions.push("priority IN $allowed_priorities".to_string());
            binds.push(("allowed_priorities".into(), serde_json::json!(strs)));
        }

        if let Some(ref tags) = filter.tags
            && !tags.is_empty()
        {
            conditions.push("tags CONTAINSALL $required_tags".to_string());
            binds.push(("required_tags".into(), serde_json::json!(tags)));
        }

        if let Some(ref refs) = filter.project_refs
            && !refs.is_empty()
        {
            conditions.push("project_refs CONTAINSALL $required_projects".to_string());
            binds.push(("required_projects".into(), serde_json::json!(refs)));
        }

        if let Some(ref refs) = filter.team_refs
            && !refs.is_empty()
        {
            conditions.push("team_refs CONTAINSALL $required_teams".to_string());
            binds.push(("required_teams".into(), serde_json::json!(refs)));
        }

        if let Some(ref cid) = filter.correlation_id {
            conditions.push("correlation_id = $corr_id".to_string());
            binds.push(("corr_id".into(), serde_json::json!(cid)));
        }

        if let Some(ref tid) = filter.thread_id {
            conditions.push("thread_id = $thread_id_v".to_string());
            binds.push(("thread_id_v".into(), serde_json::json!(tid)));
        }

        if let Some(since) = filter.since_ns {
            conditions.push("created_at >= $since_ns".to_string());
            binds.push(("since_ns".into(), serde_json::json!(since)));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };

        let limit_clause = match filter.limit {
            Some(n) => format!(" LIMIT {n}"),
            None => String::new(),
        };

        let sql =
            format!("SELECT * FROM envelope{where_clause} ORDER BY created_at ASC{limit_clause}");

        (sql, binds)
    }
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

fn decode_envelope(mut val: serde_json::Value) -> Result<EnvelopeRecord, StoreError> {
    if let Some(id_val) = val.get("id")
        && let Some(id) = extract_record_id(id_val)
    {
        val["id"] = serde_json::Value::String(id);
    }
    serde_json::from_value(val).map_err(StoreError::from)
}

#[async_trait::async_trait]
impl EnvelopeStore for SurrealEnvelopeStore {
    // Idempotent — safe to call regardless of whether SurrealStore::init_schema
    // already ran the same DDL during workspace bootstrap. Callers who construct
    // SurrealEnvelopeStore from a raw Surreal<Db> they opened directly must
    // invoke this before any queries; callers who got the store from
    // SurrealStore::envelope_store() may skip it.
    async fn init_schema(&self) -> Result<(), StoreError> {
        self.db
            .query(ENVELOPE_DDL)
            .await
            .map_err(|e| StoreError::Db(format!("envelope schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("envelope schema init check: {e}")))?;
        Ok(())
    }

    async fn enqueue_envelope(&self, record: EnvelopeRecord) -> Result<String, StoreError> {
        let mut content = serde_json::to_value(&record)?;
        if let serde_json::Value::Object(ref mut obj) = content {
            obj.remove("id");
        }

        // Whitespace-only ids are treated as empty so callers who accidentally
        // pass `"  "` get a generated id instead of a record keyed on whitespace.
        let trimmed = record.id.trim();
        let explicit_id = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };

        match explicit_id {
            Some(id) => {
                let mut result = self
                    .db
                    .query("UPSERT type::record('envelope', $id) CONTENT $content RETURN AFTER")
                    .bind(("id", serde_json::json!(id)))
                    .bind(("content", content))
                    .await
                    .map_err(|e| StoreError::Db(format!("enqueue_envelope upsert: {e}")))?;

                let row: Option<serde_json::Value> = result
                    .take(0)
                    .map_err(|e| StoreError::Db(format!("enqueue_envelope read: {e}")))?;

                row.as_ref()
                    .and_then(|v| v.get("id"))
                    .and_then(extract_record_id)
                    .ok_or_else(|| StoreError::Db("enqueue_envelope: no id returned".into()))
            }
            None => {
                let mut result = self
                    .db
                    .query("CREATE envelope CONTENT $content")
                    .bind(("content", content))
                    .await
                    .map_err(|e| StoreError::Db(format!("enqueue_envelope create: {e}")))?;

                let row: Option<serde_json::Value> = result
                    .take(0)
                    .map_err(|e| StoreError::Db(format!("enqueue_envelope read: {e}")))?;

                row.as_ref()
                    .and_then(|v| v.get("id"))
                    .and_then(extract_record_id)
                    .ok_or_else(|| StoreError::Db("enqueue_envelope: no id returned".into()))
            }
        }
    }

    async fn get_envelope(&self, id: &str) -> Result<Option<EnvelopeRecord>, StoreError> {
        let mut result = self
            .db
            .query("SELECT * FROM type::record('envelope', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("get_envelope: {e}")))?;

        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("get_envelope read: {e}")))?;

        row.map(decode_envelope).transpose()
    }

    async fn query_inbox(
        &self,
        filter: &EnvelopeFilter,
    ) -> Result<Vec<EnvelopeRecord>, StoreError> {
        let (sql, binds) = Self::build_query_sql(filter);

        let mut query = self.db.query(&sql);
        for (name, value) in binds {
            query = query.bind((name, value));
        }

        let mut result = query
            .await
            .map_err(|e| StoreError::Db(format!("query_inbox: {e}")))?;

        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("query_inbox read: {e}")))?;

        rows.into_iter().map(decode_envelope).collect()
    }

    async fn list_pending(
        &self,
        inbox_address: &str,
        now_ns: Option<u64>,
        limit: Option<usize>,
    ) -> Result<Vec<EnvelopeRecord>, StoreError> {
        let mut sql = String::from(
            "SELECT * FROM envelope WHERE inbox_address = $addr AND status = 'queued'",
        );
        let mut binds: Vec<(String, serde_json::Value)> =
            vec![("addr".into(), serde_json::json!(inbox_address))];

        if let Some(now) = now_ns {
            sql.push_str(" AND (deliver_after IS NONE OR deliver_after <= $now_ns)");
            binds.push(("now_ns".into(), serde_json::json!(now)));
        }

        sql.push_str(" ORDER BY created_at ASC");
        if let Some(n) = limit {
            sql.push_str(&format!(" LIMIT {n}"));
        }

        let mut query = self.db.query(&sql);
        for (name, value) in binds {
            query = query.bind((name, value));
        }

        let mut result = query
            .await
            .map_err(|e| StoreError::Db(format!("list_pending: {e}")))?;

        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("list_pending read: {e}")))?;

        rows.into_iter().map(decode_envelope).collect()
    }

    // Forward-only state transitions. Each mark_* operation embeds the allowed
    // source-state set in the SurrealQL WHERE clause so the transition is
    // atomic — there is no read-then-write race window. When zero rows match,
    // we follow up with a single SELECT to differentiate "envelope doesn't
    // exist" from "envelope exists but is in a disallowed state".
    async fn mark_delivered(&self, id: &str, at_ns: u64) -> Result<(), StoreError> {
        let mut result = self
            .db
            .query(
                "UPDATE type::record('envelope', $id) \
                 SET status = 'delivered', delivered_at = $at, updated_at = $at \
                 WHERE status = 'queued' RETURN AFTER",
            )
            .bind(("id", serde_json::json!(id)))
            .bind(("at", serde_json::json!(at_ns)))
            .await
            .map_err(|e| StoreError::Db(format!("mark_delivered: {e}")))?;

        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("mark_delivered read: {e}")))?;

        if rows.is_empty() {
            return Err(self
                .diagnose_no_rows(id, "transition to delivered", "queued")
                .await);
        }
        Ok(())
    }

    async fn mark_read(&self, id: &str, at_ns: u64) -> Result<(), StoreError> {
        // delivered_at is filled in-place: if the row never reached delivered,
        // we stamp $at; otherwise we preserve whatever delivered_at already
        // holds. This eliminates the read-then-write race that an external
        // mark_delivered could open.
        let mut result = self
            .db
            .query(
                "UPDATE type::record('envelope', $id) \
                 SET status = 'read', read_at = $at, \
                     delivered_at = IF delivered_at = NONE THEN $at ELSE delivered_at END, \
                     updated_at = $at \
                 WHERE status IN ['queued', 'delivered'] RETURN AFTER",
            )
            .bind(("id", serde_json::json!(id)))
            .bind(("at", serde_json::json!(at_ns)))
            .await
            .map_err(|e| StoreError::Db(format!("mark_read: {e}")))?;

        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("mark_read read: {e}")))?;

        if rows.is_empty() {
            return Err(self
                .diagnose_no_rows(id, "transition to read", "queued, delivered")
                .await);
        }
        Ok(())
    }

    // mark_failed is allowed from queued, delivered, or read (downstream
    // processing can discover that an already-read message was malformed).
    // It is NOT allowed from failed/expired/cancelled — those are terminal
    // and re-failing them would erase prior context.
    async fn mark_failed(&self, id: &str, reason: &str, at_ns: u64) -> Result<(), StoreError> {
        let mut result = self
            .db
            .query(
                "UPDATE type::record('envelope', $id) \
                 SET status = 'failed', failed_at = $at, failure_reason = $reason, updated_at = $at \
                 WHERE status IN ['queued', 'delivered', 'read'] RETURN AFTER",
            )
            .bind(("id", serde_json::json!(id)))
            .bind(("at", serde_json::json!(at_ns)))
            .bind(("reason", serde_json::json!(reason)))
            .await
            .map_err(|e| StoreError::Db(format!("mark_failed: {e}")))?;

        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("mark_failed read: {e}")))?;

        if rows.is_empty() {
            return Err(self
                .diagnose_no_rows(id, "transition to failed", "queued, delivered, read")
                .await);
        }
        Ok(())
    }

    async fn cancel_envelope(&self, id: &str, at_ns: u64) -> Result<(), StoreError> {
        let mut result = self
            .db
            .query(
                "UPDATE type::record('envelope', $id) \
                 SET status = 'cancelled', updated_at = $at \
                 WHERE status IN ['queued', 'delivered'] RETURN AFTER",
            )
            .bind(("id", serde_json::json!(id)))
            .bind(("at", serde_json::json!(at_ns)))
            .await
            .map_err(|e| StoreError::Db(format!("cancel_envelope: {e}")))?;

        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("cancel_envelope read: {e}")))?;

        if rows.is_empty() {
            return Err(self
                .diagnose_no_rows(id, "cancel", "queued, delivered")
                .await);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnvelopeFilter, EnvelopeStore, SurrealStore};
    use daemon8_types::{EnvelopeKind, EnvelopePriority, EnvelopeRecord, EnvelopeStatus};
    use serde_json::json;

    async fn setup() -> (SurrealStore, SurrealEnvelopeStore) {
        let store = SurrealStore::memory().await.unwrap();
        let env_store = store.envelope_store();
        env_store.init_schema().await.unwrap();
        (store, env_store)
    }

    fn make_envelope(
        from: &str,
        to: &str,
        inbox: &str,
        kind: EnvelopeKind,
        priority: EnvelopePriority,
        created_at: u64,
    ) -> EnvelopeRecord {
        EnvelopeRecord {
            id: String::new(),
            kind,
            status: EnvelopeStatus::Queued,
            priority,
            from_address: from.into(),
            to_address: to.into(),
            inbox_address: inbox.into(),
            subject: None,
            body: None,
            payload: None,
            correlation_id: None,
            thread_id: None,
            reply_to: None,
            created_at,
            updated_at: created_at,
            deliver_after: None,
            delivered_at: None,
            read_at: None,
            expires_at: None,
            failed_at: None,
            failure_reason: None,
            tags: vec![],
            project_refs: vec![],
            team_refs: vec![],
        }
    }

    #[tokio::test]
    async fn schema_init_is_idempotent() {
        let (_store, env_store) = setup().await;
        env_store.init_schema().await.unwrap();
        env_store.init_schema().await.unwrap();
    }

    #[tokio::test]
    async fn enqueue_and_get_roundtrip_full_record() {
        let (_store, env_store) = setup().await;

        let mut env = make_envelope(
            "agent:supervisor",
            "agent:specialist",
            "agent:specialist",
            EnvelopeKind::Request,
            EnvelopePriority::High,
            1_000,
        );
        env.subject = Some("inspect".into());
        env.body = Some("walk crates/types".into());
        env.payload = Some(json!({"depth": 2, "files": ["lib.rs"]}));
        env.correlation_id = Some("corr-1".into());
        env.thread_id = Some("thread-1".into());
        env.reply_to = Some("agent:supervisor".into());
        env.deliver_after = Some(2_000);
        env.expires_at = Some(10_000);
        env.tags = vec!["urgent".into(), "ops".into()];
        env.project_refs = vec!["proj:daemon8".into()];
        env.team_refs = vec!["team:core".into()];

        let id = env_store.enqueue_envelope(env.clone()).await.unwrap();
        assert!(!id.is_empty());

        let fetched = env_store.get_envelope(&id).await.unwrap().unwrap();
        assert_eq!(fetched.kind, env.kind);
        assert_eq!(fetched.status, EnvelopeStatus::Queued);
        assert_eq!(fetched.priority, env.priority);
        assert_eq!(fetched.from_address, env.from_address);
        assert_eq!(fetched.to_address, env.to_address);
        assert_eq!(fetched.inbox_address, env.inbox_address);
        assert_eq!(fetched.subject, env.subject);
        assert_eq!(fetched.body, env.body);
        assert_eq!(fetched.payload, env.payload);
        assert_eq!(fetched.correlation_id, env.correlation_id);
        assert_eq!(fetched.thread_id, env.thread_id);
        assert_eq!(fetched.reply_to, env.reply_to);
        assert_eq!(fetched.deliver_after, env.deliver_after);
        assert_eq!(fetched.expires_at, env.expires_at);
        assert_eq!(fetched.tags, env.tags);
        assert_eq!(fetched.project_refs, env.project_refs);
        assert_eq!(fetched.team_refs, env.team_refs);
    }

    #[tokio::test]
    async fn enqueue_with_explicit_id_preserves_it() {
        let (_store, env_store) = setup().await;

        let mut env = make_envelope(
            "a",
            "b",
            "b",
            EnvelopeKind::Notice,
            EnvelopePriority::Normal,
            1,
        );
        env.id = "env_pinned_42".into();

        let id = env_store.enqueue_envelope(env).await.unwrap();
        assert_eq!(id, "env_pinned_42");

        let fetched = env_store.get_envelope("env_pinned_42").await.unwrap();
        assert!(fetched.is_some());
    }

    #[tokio::test]
    async fn query_inbox_filters_by_status_kind_priority() {
        let (_store, env_store) = setup().await;

        let inbox = "agent:worker";
        env_store
            .enqueue_envelope(make_envelope(
                "a",
                inbox,
                inbox,
                EnvelopeKind::Request,
                EnvelopePriority::High,
                10,
            ))
            .await
            .unwrap();
        env_store
            .enqueue_envelope(make_envelope(
                "a",
                inbox,
                inbox,
                EnvelopeKind::Notice,
                EnvelopePriority::Low,
                20,
            ))
            .await
            .unwrap();
        env_store
            .enqueue_envelope(make_envelope(
                "a",
                "agent:other",
                "agent:other",
                EnvelopeKind::Request,
                EnvelopePriority::High,
                30,
            ))
            .await
            .unwrap();

        let by_kind = env_store
            .query_inbox(&EnvelopeFilter {
                inbox_address: Some(inbox.into()),
                kinds: Some(vec![EnvelopeKind::Request]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_kind.len(), 1);
        assert_eq!(by_kind[0].kind, EnvelopeKind::Request);

        let by_priority = env_store
            .query_inbox(&EnvelopeFilter {
                inbox_address: Some(inbox.into()),
                priorities: Some(vec![EnvelopePriority::Low]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_priority.len(), 1);
        assert_eq!(by_priority[0].priority, EnvelopePriority::Low);

        let by_status = env_store
            .query_inbox(&EnvelopeFilter {
                inbox_address: Some(inbox.into()),
                statuses: Some(vec![EnvelopeStatus::Queued]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_status.len(), 2);
    }

    #[tokio::test]
    async fn query_inbox_filters_by_tags_and_refs() {
        let (_store, env_store) = setup().await;

        let mut tagged = make_envelope(
            "a",
            "b",
            "b",
            EnvelopeKind::Message,
            EnvelopePriority::Normal,
            1,
        );
        tagged.tags = vec!["alpha".into(), "beta".into()];
        tagged.project_refs = vec!["proj:x".into()];
        tagged.team_refs = vec!["team:y".into()];
        env_store.enqueue_envelope(tagged).await.unwrap();

        let mut other = make_envelope(
            "a",
            "b",
            "b",
            EnvelopeKind::Message,
            EnvelopePriority::Normal,
            2,
        );
        other.tags = vec!["beta".into()];
        env_store.enqueue_envelope(other).await.unwrap();

        let by_tag = env_store
            .query_inbox(&EnvelopeFilter {
                tags: Some(vec!["alpha".into()]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_tag.len(), 1);

        let by_project = env_store
            .query_inbox(&EnvelopeFilter {
                project_refs: Some(vec!["proj:x".into()]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_project.len(), 1);

        let by_team = env_store
            .query_inbox(&EnvelopeFilter {
                team_refs: Some(vec!["team:y".into()]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_team.len(), 1);
    }

    #[tokio::test]
    async fn query_inbox_since_ns_filters_by_creation_time() {
        let (_store, env_store) = setup().await;
        env_store
            .enqueue_envelope(make_envelope(
                "a",
                "b",
                "b",
                EnvelopeKind::Message,
                EnvelopePriority::Normal,
                100,
            ))
            .await
            .unwrap();
        env_store
            .enqueue_envelope(make_envelope(
                "a",
                "b",
                "b",
                EnvelopeKind::Message,
                EnvelopePriority::Normal,
                200,
            ))
            .await
            .unwrap();
        env_store
            .enqueue_envelope(make_envelope(
                "a",
                "b",
                "b",
                EnvelopeKind::Message,
                EnvelopePriority::Normal,
                300,
            ))
            .await
            .unwrap();

        let recent = env_store
            .query_inbox(&EnvelopeFilter {
                since_ns: Some(200),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(recent.len(), 2);
        assert!(recent.iter().all(|e| e.created_at >= 200));
    }

    #[tokio::test]
    async fn list_pending_excludes_terminal_states() {
        let (_store, env_store) = setup().await;
        let inbox = "agent:worker";

        let queued_id = env_store
            .enqueue_envelope(make_envelope(
                "a",
                inbox,
                inbox,
                EnvelopeKind::Message,
                EnvelopePriority::Normal,
                10,
            ))
            .await
            .unwrap();
        let to_deliver = env_store
            .enqueue_envelope(make_envelope(
                "a",
                inbox,
                inbox,
                EnvelopeKind::Message,
                EnvelopePriority::Normal,
                20,
            ))
            .await
            .unwrap();
        let to_read = env_store
            .enqueue_envelope(make_envelope(
                "a",
                inbox,
                inbox,
                EnvelopeKind::Message,
                EnvelopePriority::Normal,
                30,
            ))
            .await
            .unwrap();
        let to_fail = env_store
            .enqueue_envelope(make_envelope(
                "a",
                inbox,
                inbox,
                EnvelopeKind::Message,
                EnvelopePriority::Normal,
                40,
            ))
            .await
            .unwrap();
        let to_cancel = env_store
            .enqueue_envelope(make_envelope(
                "a",
                inbox,
                inbox,
                EnvelopeKind::Message,
                EnvelopePriority::Normal,
                50,
            ))
            .await
            .unwrap();

        env_store.mark_delivered(&to_deliver, 100).await.unwrap();
        env_store.mark_read(&to_read, 110).await.unwrap();
        env_store.mark_failed(&to_fail, "boom", 120).await.unwrap();
        env_store.cancel_envelope(&to_cancel, 130).await.unwrap();

        let pending = env_store.list_pending(inbox, None, None).await.unwrap();
        assert_eq!(
            pending.len(),
            1,
            "list_pending must only return queued (delivered/read/failed/cancelled excluded)"
        );
        assert_eq!(pending[0].id, queued_id);
        let pending_ids: Vec<&String> = pending.iter().map(|e| &e.id).collect();
        assert!(!pending_ids.iter().any(|id| **id == to_deliver));
        assert!(!pending_ids.iter().any(|id| **id == to_read));
        assert!(!pending_ids.iter().any(|id| **id == to_fail));
        assert!(!pending_ids.iter().any(|id| **id == to_cancel));
    }

    #[tokio::test]
    async fn list_pending_respects_deliver_after() {
        let (_store, env_store) = setup().await;
        let inbox = "agent:worker";

        let now_due = make_envelope(
            "a",
            inbox,
            inbox,
            EnvelopeKind::Message,
            EnvelopePriority::Normal,
            10,
        );
        let mut later_due = make_envelope(
            "a",
            inbox,
            inbox,
            EnvelopeKind::Message,
            EnvelopePriority::Normal,
            20,
        );
        later_due.deliver_after = Some(500);

        env_store.enqueue_envelope(now_due).await.unwrap();
        env_store.enqueue_envelope(later_due).await.unwrap();

        let early = env_store
            .list_pending(inbox, Some(100), None)
            .await
            .unwrap();
        assert_eq!(early.len(), 1, "only the now-due envelope should appear");

        let after_window = env_store
            .list_pending(inbox, Some(600), None)
            .await
            .unwrap();
        assert_eq!(after_window.len(), 2);

        let no_cutoff = env_store.list_pending(inbox, None, None).await.unwrap();
        assert_eq!(no_cutoff.len(), 2, "None means treat all queued as due");
    }

    #[tokio::test]
    async fn mark_delivered_preserves_identity_fields() {
        let (_store, env_store) = setup().await;

        let mut env = make_envelope(
            "agent:from",
            "agent:to",
            "agent:to",
            EnvelopeKind::Request,
            EnvelopePriority::High,
            1,
        );
        env.subject = Some("subj".into());
        env.body = Some("body".into());
        env.payload = Some(json!({"k": "v"}));
        let id = env_store.enqueue_envelope(env.clone()).await.unwrap();

        env_store.mark_delivered(&id, 999).await.unwrap();

        let after = env_store.get_envelope(&id).await.unwrap().unwrap();
        assert_eq!(after.status, EnvelopeStatus::Delivered);
        assert_eq!(after.delivered_at, Some(999));
        assert_eq!(after.updated_at, 999);
        assert_eq!(after.from_address, env.from_address);
        assert_eq!(after.to_address, env.to_address);
        assert_eq!(after.inbox_address, env.inbox_address);
        assert_eq!(after.subject, env.subject);
        assert_eq!(after.body, env.body);
        assert_eq!(after.payload, env.payload);
    }

    #[tokio::test]
    async fn mark_read_from_queued_fills_delivered_and_read() {
        let (_store, env_store) = setup().await;

        let id = env_store
            .enqueue_envelope(make_envelope(
                "a",
                "b",
                "b",
                EnvelopeKind::Message,
                EnvelopePriority::Normal,
                1,
            ))
            .await
            .unwrap();

        env_store.mark_read(&id, 555).await.unwrap();

        let after = env_store.get_envelope(&id).await.unwrap().unwrap();
        assert_eq!(after.status, EnvelopeStatus::Read);
        assert_eq!(after.delivered_at, Some(555));
        assert_eq!(after.read_at, Some(555));
        assert_eq!(after.updated_at, 555);
    }

    #[tokio::test]
    async fn mark_read_after_delivered_keeps_existing_delivered_at() {
        let (_store, env_store) = setup().await;

        let id = env_store
            .enqueue_envelope(make_envelope(
                "a",
                "b",
                "b",
                EnvelopeKind::Message,
                EnvelopePriority::Normal,
                1,
            ))
            .await
            .unwrap();

        env_store.mark_delivered(&id, 100).await.unwrap();
        env_store.mark_read(&id, 200).await.unwrap();

        let after = env_store.get_envelope(&id).await.unwrap().unwrap();
        assert_eq!(after.status, EnvelopeStatus::Read);
        assert_eq!(after.delivered_at, Some(100));
        assert_eq!(after.read_at, Some(200));
        assert_eq!(after.updated_at, 200);
    }

    #[tokio::test]
    async fn mark_failed_records_reason_and_preserves_identity() {
        let (_store, env_store) = setup().await;

        let mut env = make_envelope(
            "agent:from",
            "agent:to",
            "agent:to",
            EnvelopeKind::Request,
            EnvelopePriority::High,
            1,
        );
        env.payload = Some(json!({"x": 1}));
        let id = env_store.enqueue_envelope(env.clone()).await.unwrap();

        env_store
            .mark_failed(&id, "downstream timeout", 777)
            .await
            .unwrap();

        let after = env_store.get_envelope(&id).await.unwrap().unwrap();
        assert_eq!(after.status, EnvelopeStatus::Failed);
        assert_eq!(after.failed_at, Some(777));
        assert_eq!(after.failure_reason.as_deref(), Some("downstream timeout"));
        assert_eq!(after.from_address, env.from_address);
        assert_eq!(after.to_address, env.to_address);
        assert_eq!(after.payload, env.payload);
    }

    #[tokio::test]
    async fn cancel_from_queued_transitions_to_cancelled() {
        let (_store, env_store) = setup().await;

        let id = env_store
            .enqueue_envelope(make_envelope(
                "a",
                "b",
                "b",
                EnvelopeKind::Message,
                EnvelopePriority::Normal,
                1,
            ))
            .await
            .unwrap();

        env_store.cancel_envelope(&id, 42).await.unwrap();

        let after = env_store.get_envelope(&id).await.unwrap().unwrap();
        assert_eq!(after.status, EnvelopeStatus::Cancelled);
        assert_eq!(after.updated_at, 42);
    }

    #[tokio::test]
    async fn cancel_rejects_terminal_states() {
        let (_store, env_store) = setup().await;

        let id = env_store
            .enqueue_envelope(make_envelope(
                "a",
                "b",
                "b",
                EnvelopeKind::Message,
                EnvelopePriority::Normal,
                1,
            ))
            .await
            .unwrap();
        env_store.mark_failed(&id, "x", 10).await.unwrap();

        let err = env_store.cancel_envelope(&id, 20).await;
        assert!(err.is_err(), "cancelling a failed envelope must error");
    }

    #[tokio::test]
    async fn loose_refs_to_unknown_cards_are_preserved() {
        let (_store, env_store) = setup().await;

        let mut env = make_envelope(
            "agent:does-not-exist@nowhere",
            "agent:also-fake",
            "agent:also-fake",
            EnvelopeKind::Notice,
            EnvelopePriority::Low,
            1,
        );
        env.project_refs = vec!["proj:ghost".into()];
        env.team_refs = vec!["team:nobody".into()];

        let id = env_store.enqueue_envelope(env.clone()).await.unwrap();
        let after = env_store.get_envelope(&id).await.unwrap().unwrap();
        assert_eq!(after.from_address, env.from_address);
        assert_eq!(after.to_address, env.to_address);
        assert_eq!(after.project_refs, env.project_refs);
        assert_eq!(after.team_refs, env.team_refs);
    }

    #[tokio::test]
    async fn correlation_thread_reply_fields_filterable() {
        let (_store, env_store) = setup().await;

        let mut req = make_envelope(
            "agent:supervisor",
            "agent:specialist",
            "agent:specialist",
            EnvelopeKind::Request,
            EnvelopePriority::Normal,
            1,
        );
        req.correlation_id = Some("corr-99".into());
        req.thread_id = Some("thread-99".into());
        let req_id = env_store.enqueue_envelope(req).await.unwrap();

        let mut resp = make_envelope(
            "agent:specialist",
            "agent:supervisor",
            "agent:supervisor",
            EnvelopeKind::Response,
            EnvelopePriority::Normal,
            2,
        );
        resp.correlation_id = Some("corr-99".into());
        resp.thread_id = Some("thread-99".into());
        resp.reply_to = Some(req_id.clone());
        env_store.enqueue_envelope(resp).await.unwrap();

        let by_corr = env_store
            .query_inbox(&EnvelopeFilter {
                correlation_id: Some("corr-99".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_corr.len(), 2);

        let by_thread = env_store
            .query_inbox(&EnvelopeFilter {
                thread_id: Some("thread-99".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_thread.len(), 2);

        let response = env_store
            .query_inbox(&EnvelopeFilter {
                kinds: Some(vec![EnvelopeKind::Response]),
                correlation_id: Some("corr-99".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(response.len(), 1);
        assert_eq!(response[0].reply_to.as_deref(), Some(req_id.as_str()));
    }

    #[tokio::test]
    async fn embedded_colons_in_explicit_id_roundtrip() {
        let (_store, env_store) = setup().await;

        let mut env = make_envelope(
            "a",
            "b",
            "b",
            EnvelopeKind::Notice,
            EnvelopePriority::Normal,
            1,
        );
        env.id = "env:custom:abc".into();

        let id = env_store.enqueue_envelope(env).await.unwrap();
        assert_eq!(id, "env:custom:abc");

        let fetched = env_store.get_envelope("env:custom:abc").await.unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().id, "env:custom:abc");
    }

    #[tokio::test]
    async fn whitespace_only_id_is_treated_as_empty() {
        let (_store, env_store) = setup().await;

        let mut env = make_envelope(
            "a",
            "b",
            "b",
            EnvelopeKind::Notice,
            EnvelopePriority::Normal,
            1,
        );
        env.id = "   ".into();

        let id = env_store.enqueue_envelope(env).await.unwrap();
        assert!(!id.is_empty());
        assert_ne!(id.trim(), "", "auto-generated id must not be whitespace");
        assert!(
            !id.chars().all(char::is_whitespace),
            "auto-generated id must not be whitespace"
        );
    }

    #[tokio::test]
    async fn explicit_id_collision_overwrites_silently() {
        // Documented behavior: enqueue_envelope with an existing explicit id
        // performs an UPSERT, replacing the prior payload. Mirrors the card
        // store's upsert pattern. Callers that want fail-on-duplicate must
        // gate on get_envelope first.
        let (_store, env_store) = setup().await;

        let mut first = make_envelope(
            "agent:from-1",
            "agent:to",
            "agent:to",
            EnvelopeKind::Message,
            EnvelopePriority::Normal,
            1,
        );
        first.id = "env_pinned".into();
        first.subject = Some("first".into());
        env_store.enqueue_envelope(first).await.unwrap();

        let mut second = make_envelope(
            "agent:from-2",
            "agent:to",
            "agent:to",
            EnvelopeKind::Message,
            EnvelopePriority::Normal,
            2,
        );
        second.id = "env_pinned".into();
        second.subject = Some("second".into());
        env_store.enqueue_envelope(second).await.unwrap();

        let after = env_store.get_envelope("env_pinned").await.unwrap().unwrap();
        assert_eq!(after.subject.as_deref(), Some("second"));
        assert_eq!(after.from_address, "agent:from-2");
    }

    #[tokio::test]
    async fn mark_delivered_on_missing_id_errors_clearly() {
        let (_store, env_store) = setup().await;
        let err = env_store
            .mark_delivered("does_not_exist", 1)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("does_not_exist"),
            "error must include id, got: {err}"
        );
        assert!(
            err.contains("not found"),
            "error must say not found, got: {err}"
        );
    }

    #[tokio::test]
    async fn mark_read_on_missing_id_errors_clearly() {
        let (_store, env_store) = setup().await;
        let err = env_store
            .mark_read("ghost_envelope", 1)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("ghost_envelope"));
        assert!(err.contains("not found"));
    }

    #[tokio::test]
    async fn mark_failed_on_missing_id_errors_clearly() {
        let (_store, env_store) = setup().await;
        let err = env_store
            .mark_failed("ghost", "reason", 1)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("ghost"));
        assert!(err.contains("not found"));
    }

    #[tokio::test]
    async fn cancel_on_missing_id_errors_clearly() {
        let (_store, env_store) = setup().await;
        let err = env_store
            .cancel_envelope("ghost", 1)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("ghost"));
        assert!(err.contains("not found"));
    }

    async fn enqueue_one(env_store: &SurrealEnvelopeStore) -> String {
        env_store
            .enqueue_envelope(make_envelope(
                "a",
                "b",
                "b",
                EnvelopeKind::Message,
                EnvelopePriority::Normal,
                1,
            ))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn mark_delivered_after_read_is_rejected() {
        let (_store, env_store) = setup().await;
        let id = enqueue_one(&env_store).await;
        env_store.mark_read(&id, 100).await.unwrap();
        let err = env_store
            .mark_delivered(&id, 200)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("read"),
            "error must mention current state: {err}"
        );
        assert!(
            err.contains("delivered"),
            "error must mention target: {err}"
        );
    }

    #[tokio::test]
    async fn mark_delivered_after_failed_is_rejected() {
        let (_store, env_store) = setup().await;
        let id = enqueue_one(&env_store).await;
        env_store.mark_failed(&id, "x", 100).await.unwrap();
        let err = env_store
            .mark_delivered(&id, 200)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("failed"));
    }

    #[tokio::test]
    async fn mark_delivered_after_cancelled_is_rejected() {
        let (_store, env_store) = setup().await;
        let id = enqueue_one(&env_store).await;
        env_store.cancel_envelope(&id, 100).await.unwrap();
        let err = env_store
            .mark_delivered(&id, 200)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("cancelled"));
    }

    #[tokio::test]
    async fn mark_delivered_twice_is_rejected() {
        // mark_delivered is forward-only from queued; calling it again from
        // delivered must error so callers cannot accidentally rewrite the
        // delivered_at timestamp.
        let (_store, env_store) = setup().await;
        let id = enqueue_one(&env_store).await;
        env_store.mark_delivered(&id, 100).await.unwrap();
        let err = env_store
            .mark_delivered(&id, 200)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("delivered"));

        let after = env_store.get_envelope(&id).await.unwrap().unwrap();
        assert_eq!(
            after.delivered_at,
            Some(100),
            "rejected mark_delivered must not overwrite the original timestamp"
        );
    }

    #[tokio::test]
    async fn mark_read_after_failed_is_rejected() {
        let (_store, env_store) = setup().await;
        let id = enqueue_one(&env_store).await;
        env_store.mark_failed(&id, "x", 100).await.unwrap();
        let err = env_store.mark_read(&id, 200).await.unwrap_err().to_string();
        assert!(err.contains("failed"));
    }

    #[tokio::test]
    async fn mark_failed_after_read_succeeds() {
        // Documented allowed transition: downstream processing may discover
        // an already-read message was malformed and want to fail it.
        let (_store, env_store) = setup().await;
        let id = enqueue_one(&env_store).await;
        env_store.mark_read(&id, 100).await.unwrap();
        env_store
            .mark_failed(&id, "downstream malformed", 200)
            .await
            .unwrap();

        let after = env_store.get_envelope(&id).await.unwrap().unwrap();
        assert_eq!(after.status, EnvelopeStatus::Failed);
        assert_eq!(after.failed_at, Some(200));
        assert_eq!(
            after.failure_reason.as_deref(),
            Some("downstream malformed")
        );
        // read_at must be preserved across the read -> failed transition.
        assert_eq!(after.read_at, Some(100));
    }

    #[tokio::test]
    async fn mark_failed_twice_is_rejected() {
        let (_store, env_store) = setup().await;
        let id = enqueue_one(&env_store).await;
        env_store.mark_failed(&id, "first", 100).await.unwrap();
        let err = env_store
            .mark_failed(&id, "second", 200)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("failed"));

        let after = env_store.get_envelope(&id).await.unwrap().unwrap();
        assert_eq!(after.failure_reason.as_deref(), Some("first"));
    }

    #[tokio::test]
    async fn cancel_after_read_is_rejected() {
        let (_store, env_store) = setup().await;
        let id = enqueue_one(&env_store).await;
        env_store.mark_read(&id, 100).await.unwrap();
        let err = env_store
            .cancel_envelope(&id, 200)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("read"));
    }

    #[tokio::test]
    async fn query_inbox_combined_filters() {
        let (_store, env_store) = setup().await;
        let inbox = "agent:worker";

        let mut hit = make_envelope(
            "a",
            inbox,
            inbox,
            EnvelopeKind::Request,
            EnvelopePriority::High,
            300,
        );
        hit.tags = vec!["alpha".into(), "beta".into()];
        env_store.enqueue_envelope(hit).await.unwrap();

        // Wrong kind
        let mut wrong_kind = make_envelope(
            "a",
            inbox,
            inbox,
            EnvelopeKind::Notice,
            EnvelopePriority::High,
            301,
        );
        wrong_kind.tags = vec!["alpha".into(), "beta".into()];
        env_store.enqueue_envelope(wrong_kind).await.unwrap();

        // Missing required tag
        let mut wrong_tag = make_envelope(
            "a",
            inbox,
            inbox,
            EnvelopeKind::Request,
            EnvelopePriority::High,
            302,
        );
        wrong_tag.tags = vec!["alpha".into()];
        env_store.enqueue_envelope(wrong_tag).await.unwrap();

        // Too old
        let mut too_old = make_envelope(
            "a",
            inbox,
            inbox,
            EnvelopeKind::Request,
            EnvelopePriority::High,
            100,
        );
        too_old.tags = vec!["alpha".into(), "beta".into()];
        env_store.enqueue_envelope(too_old).await.unwrap();

        let results = env_store
            .query_inbox(&EnvelopeFilter {
                inbox_address: Some(inbox.into()),
                statuses: Some(vec![EnvelopeStatus::Queued]),
                kinds: Some(vec![EnvelopeKind::Request]),
                tags: Some(vec!["alpha".into(), "beta".into()]),
                since_ns: Some(200),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].created_at, 300);
    }

    #[tokio::test]
    async fn envelope_filter_limit_actually_limits() {
        let (_store, env_store) = setup().await;
        let inbox = "agent:worker";

        for ts in 1..=5 {
            env_store
                .enqueue_envelope(make_envelope(
                    "a",
                    inbox,
                    inbox,
                    EnvelopeKind::Message,
                    EnvelopePriority::Normal,
                    ts,
                ))
                .await
                .unwrap();
        }

        let limited = env_store
            .query_inbox(&EnvelopeFilter {
                inbox_address: Some(inbox.into()),
                limit: Some(2),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(limited.len(), 2);
    }

    #[tokio::test]
    async fn list_pending_limit_actually_limits() {
        let (_store, env_store) = setup().await;
        let inbox = "agent:worker";

        for ts in 1..=5 {
            env_store
                .enqueue_envelope(make_envelope(
                    "a",
                    inbox,
                    inbox,
                    EnvelopeKind::Message,
                    EnvelopePriority::Normal,
                    ts,
                ))
                .await
                .unwrap();
        }

        let pending = env_store.list_pending(inbox, None, Some(3)).await.unwrap();
        assert_eq!(pending.len(), 3);
    }

    #[tokio::test]
    async fn vec_fields_roundtrip_through_db_with_multiple_entries() {
        let (_store, env_store) = setup().await;

        let mut env = make_envelope(
            "a",
            "b",
            "b",
            EnvelopeKind::Message,
            EnvelopePriority::Normal,
            1,
        );
        env.tags = vec!["one".into(), "two".into(), "three".into()];
        env.project_refs = vec!["proj:a".into(), "proj:b".into()];
        env.team_refs = vec!["team:x".into(), "team:y".into()];

        let id = env_store.enqueue_envelope(env.clone()).await.unwrap();
        let fetched = env_store.get_envelope(&id).await.unwrap().unwrap();

        // Order may not be guaranteed by SurrealDB; compare as sets.
        let mut got_tags = fetched.tags.clone();
        got_tags.sort();
        let mut want_tags = env.tags.clone();
        want_tags.sort();
        assert_eq!(got_tags, want_tags);

        let mut got_projects = fetched.project_refs.clone();
        got_projects.sort();
        let mut want_projects = env.project_refs.clone();
        want_projects.sort();
        assert_eq!(got_projects, want_projects);

        let mut got_teams = fetched.team_refs.clone();
        got_teams.sort();
        let mut want_teams = env.team_refs.clone();
        want_teams.sort();
        assert_eq!(got_teams, want_teams);
    }

    #[tokio::test]
    async fn non_object_payload_is_rejected_with_clear_error() {
        // Documented constraint: payload schema is `option<object> FLEXIBLE`
        // so non-object JSON values (arrays, scalars) fail at write time.
        // This test pins the constraint so future schema changes have to
        // explicitly re-evaluate it.
        let (_store, env_store) = setup().await;

        let mut env = make_envelope(
            "a",
            "b",
            "b",
            EnvelopeKind::Message,
            EnvelopePriority::Normal,
            1,
        );
        env.payload = Some(json!([1, 2, 3]));

        let err = env_store
            .enqueue_envelope(env)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.to_ascii_lowercase().contains("payload")
                || err.to_ascii_lowercase().contains("object"),
            "expected schema error mentioning payload/object, got: {err}"
        );
    }

    #[tokio::test]
    async fn standalone_envelope_store_init_schema_is_self_sufficient() {
        // SurrealEnvelopeStore constructed against a raw Surreal<Db> (without
        // SurrealStore::init_schema running first) must still be able to
        // bootstrap by calling EnvelopeStore::init_schema directly.
        use surrealdb::Surreal;
        use surrealdb::engine::local::Mem;

        let db = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns("daemon8").use_db("observations").await.unwrap();

        let store = SurrealEnvelopeStore::new(db);
        store.init_schema().await.unwrap();

        let id = store
            .enqueue_envelope(make_envelope(
                "a",
                "b",
                "b",
                EnvelopeKind::Message,
                EnvelopePriority::Normal,
                1,
            ))
            .await
            .unwrap();
        assert!(store.get_envelope(&id).await.unwrap().is_some());
    }
}
