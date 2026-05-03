// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use surrealdb::Surreal;
use surrealdb::engine::local::Db;

use crate::{EnvelopeFilter, EnvelopeRecord, EnvelopeStore, StoreError};

pub struct SurrealEnvelopeStore {
    db: Surreal<Db>,
}

impl SurrealEnvelopeStore {
    pub fn new(db: Surreal<Db>) -> Self {
        Self { db }
    }

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
    async fn init_schema(&self) -> Result<(), StoreError> {
        // Schema is owned by SurrealStore::init_schema so the envelope table
        // shares the same namespace/database lifecycle as observations and
        // memories. Calling this is a no-op kept for trait symmetry.
        Ok(())
    }

    async fn enqueue_envelope(&self, record: EnvelopeRecord) -> Result<String, StoreError> {
        let mut content = serde_json::to_value(&record)?;
        if let serde_json::Value::Object(ref mut obj) = content {
            obj.remove("id");
        }

        let explicit_id = if record.id.is_empty() {
            None
        } else {
            Some(record.id.clone())
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

    async fn mark_delivered(&self, id: &str, at_ns: u64) -> Result<(), StoreError> {
        if self.get_envelope(id).await?.is_none() {
            return Err(StoreError::Other(format!("envelope '{id}' not found")));
        }

        self.db
            .query(
                "UPDATE type::record('envelope', $id) \
                 SET status = 'delivered', delivered_at = $at, updated_at = $at",
            )
            .bind(("id", serde_json::json!(id)))
            .bind(("at", serde_json::json!(at_ns)))
            .await
            .map_err(|e| StoreError::Db(format!("mark_delivered: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("mark_delivered check: {e}")))?;

        Ok(())
    }

    async fn mark_read(&self, id: &str, at_ns: u64) -> Result<(), StoreError> {
        let existing = self
            .get_envelope(id)
            .await?
            .ok_or_else(|| StoreError::Other(format!("envelope '{id}' not found")))?;

        let delivered_at = existing.delivered_at.unwrap_or(at_ns);

        self.db
            .query(
                "UPDATE type::record('envelope', $id) \
                 SET status = 'read', read_at = $at, delivered_at = $delivered, updated_at = $at",
            )
            .bind(("id", serde_json::json!(id)))
            .bind(("at", serde_json::json!(at_ns)))
            .bind(("delivered", serde_json::json!(delivered_at)))
            .await
            .map_err(|e| StoreError::Db(format!("mark_read: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("mark_read check: {e}")))?;

        Ok(())
    }

    async fn mark_failed(&self, id: &str, reason: &str, at_ns: u64) -> Result<(), StoreError> {
        if self.get_envelope(id).await?.is_none() {
            return Err(StoreError::Other(format!("envelope '{id}' not found")));
        }

        self.db
            .query(
                "UPDATE type::record('envelope', $id) \
                 SET status = 'failed', failed_at = $at, failure_reason = $reason, updated_at = $at",
            )
            .bind(("id", serde_json::json!(id)))
            .bind(("at", serde_json::json!(at_ns)))
            .bind(("reason", serde_json::json!(reason)))
            .await
            .map_err(|e| StoreError::Db(format!("mark_failed: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("mark_failed check: {e}")))?;

        Ok(())
    }

    async fn cancel_envelope(&self, id: &str, at_ns: u64) -> Result<(), StoreError> {
        let existing = self
            .get_envelope(id)
            .await?
            .ok_or_else(|| StoreError::Other(format!("envelope '{id}' not found")))?;

        if existing.status.is_terminal() {
            return Err(StoreError::Other(format!(
                "envelope '{id}' is in terminal state {} and cannot be cancelled",
                existing.status
            )));
        }

        self.db
            .query(
                "UPDATE type::record('envelope', $id) \
                 SET status = 'cancelled', updated_at = $at",
            )
            .bind(("id", serde_json::json!(id)))
            .bind(("at", serde_json::json!(at_ns)))
            .await
            .map_err(|e| StoreError::Db(format!("cancel_envelope: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("cancel_envelope check: {e}")))?;

        Ok(())
    }
}

// SurrealStore::init_schema creates the envelope table because all daemon8
// tables share one namespace/database. EnvelopeStore::init_schema is a no-op
// kept for trait symmetry; do not also call it from the SurrealStore bootstrap.
const _: () = ();

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
}
