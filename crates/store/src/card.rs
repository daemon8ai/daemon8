// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use surrealdb::Surreal;
use surrealdb::engine::local::Db;

use daemon8_types::{ActorCard, AgentCard, AgentStatus, ProjectCard, TeamCard, UserCard};

use crate::{AgentCardFilter, CardStore, StoreError};

const NAMESPACE: &str = "daemon8";
const DATABASE: &str = "observations";

pub struct SurrealCardStore {
    db: Surreal<Db>,
}

impl SurrealCardStore {
    pub fn new(db: Surreal<Db>) -> Self {
        Self { db }
    }
}

pub const CARD_DDL: &str = "DEFINE TABLE IF NOT EXISTS actor_card SCHEMAFULL;
                 DEFINE FIELD IF NOT EXISTS address      ON actor_card TYPE string;
                 DEFINE FIELD IF NOT EXISTS actor_kind   ON actor_card TYPE string;
                 DEFINE FIELD IF NOT EXISTS slug         ON actor_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS display_name ON actor_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS status       ON actor_card TYPE string;
                 DEFINE FIELD IF NOT EXISTS origin       ON actor_card TYPE object FLEXIBLE;
                 DEFINE FIELD IF NOT EXISTS refs         ON actor_card TYPE object FLEXIBLE;
                 DEFINE FIELD IF NOT EXISTS created_at   ON actor_card TYPE int;
                 DEFINE FIELD IF NOT EXISTS updated_at   ON actor_card TYPE int;
                 DEFINE INDEX IF NOT EXISTS actor_address     ON actor_card FIELDS address UNIQUE;
                 DEFINE INDEX IF NOT EXISTS actor_kind_status ON actor_card FIELDS actor_kind, status;

                 DEFINE TABLE IF NOT EXISTS user_card SCHEMAFULL;
                 DEFINE FIELD IF NOT EXISTS actor_ref           ON user_card TYPE string;
                 DEFINE FIELD IF NOT EXISTS address             ON user_card TYPE string;
                 DEFINE FIELD IF NOT EXISTS display_name        ON user_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS communication       ON user_card TYPE object FLEXIBLE;
                 DEFINE FIELD IF NOT EXISTS current_cwd         ON user_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS current_project_ref ON user_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS inbox_address       ON user_card TYPE string;
                 DEFINE FIELD IF NOT EXISTS last_read_cursor    ON user_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS preferences         ON user_card TYPE object FLEXIBLE;
                 DEFINE FIELD IF NOT EXISTS created_at          ON user_card TYPE int;
                 DEFINE FIELD IF NOT EXISTS updated_at          ON user_card TYPE int;
                 DEFINE INDEX IF NOT EXISTS user_address ON user_card FIELDS address UNIQUE;

                 DEFINE TABLE IF NOT EXISTS agent_card SCHEMAFULL;
                 DEFINE FIELD IF NOT EXISTS actor_ref                  ON agent_card TYPE string;
                 DEFINE FIELD IF NOT EXISTS address                    ON agent_card TYPE string;
                 DEFINE FIELD IF NOT EXISTS slug                       ON agent_card TYPE string;
                 DEFINE FIELD IF NOT EXISTS display_name               ON agent_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS agent_kind                 ON agent_card TYPE string;
                 DEFINE FIELD IF NOT EXISTS status                     ON agent_card TYPE string;
                 DEFINE FIELD IF NOT EXISTS persona                    ON agent_card TYPE object FLEXIBLE;
                 DEFINE FIELD IF NOT EXISTS model                      ON agent_card TYPE object FLEXIBLE;
                 DEFINE FIELD IF NOT EXISTS capabilities               ON agent_card TYPE array<string>;
                 DEFINE FIELD IF NOT EXISTS subjects_handled           ON agent_card TYPE array<string>;
                 DEFINE FIELD IF NOT EXISTS project_refs               ON agent_card TYPE array<string>;
                 DEFINE FIELD IF NOT EXISTS team_refs                  ON agent_card TYPE array<string>;
                 DEFINE FIELD IF NOT EXISTS primary_team_ref           ON agent_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS spawned_by_actor_ref       ON agent_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS spawned_from_cwd           ON agent_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS spawned_from_project_ref   ON agent_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS host_id                    ON agent_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS pid                        ON agent_card TYPE option<int>;
                 DEFINE FIELD IF NOT EXISTS parent_pid                 ON agent_card TYPE option<int>;
                 DEFINE FIELD IF NOT EXISTS process_group_id           ON agent_card TYPE option<int>;
                 DEFINE FIELD IF NOT EXISTS executable_path            ON agent_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS argv_hash                  ON agent_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS runtime_kind               ON agent_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS runtime_version            ON agent_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS launch_nonce               ON agent_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS started_at                 ON agent_card TYPE option<int>;
                 DEFINE FIELD IF NOT EXISTS last_seen_at               ON agent_card TYPE option<int>;
                 DEFINE FIELD IF NOT EXISTS heartbeat_interval_ms      ON agent_card TYPE option<int>;
                 DEFINE FIELD IF NOT EXISTS stop_state                 ON agent_card TYPE object FLEXIBLE;
                 DEFINE FIELD IF NOT EXISTS last_stop_request_at       ON agent_card TYPE option<int>;
                 DEFINE FIELD IF NOT EXISTS last_exit_code             ON agent_card TYPE option<int>;
                 DEFINE FIELD IF NOT EXISTS last_signal                ON agent_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS cost_window_usd            ON agent_card TYPE float;
                 DEFINE FIELD IF NOT EXISTS cost_total_usd             ON agent_card TYPE float;
                 DEFINE FIELD IF NOT EXISTS budget_daily_usd           ON agent_card TYPE option<float>;
                 DEFINE FIELD IF NOT EXISTS failure_reason             ON agent_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS created_at                 ON agent_card TYPE int;
                 DEFINE FIELD IF NOT EXISTS updated_at                 ON agent_card TYPE int;
                 DEFINE INDEX IF NOT EXISTS agent_slug      ON agent_card FIELDS slug;
                 DEFINE INDEX IF NOT EXISTS agent_status    ON agent_card FIELDS status;
                 DEFINE INDEX IF NOT EXISTS agent_project   ON agent_card FIELDS project_refs, status;
                 DEFINE INDEX IF NOT EXISTS agent_team      ON agent_card FIELDS team_refs, status;
                 DEFINE INDEX IF NOT EXISTS agent_last_seen ON agent_card FIELDS last_seen_at;

                 DEFINE TABLE IF NOT EXISTS project_card SCHEMAFULL;
                 DEFINE FIELD IF NOT EXISTS actor_ref        ON project_card TYPE string;
                 DEFINE FIELD IF NOT EXISTS slug             ON project_card TYPE string;
                 DEFINE FIELD IF NOT EXISTS name             ON project_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS root_path        ON project_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS config_path      ON project_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS policy           ON project_card TYPE object FLEXIBLE;
                 DEFINE FIELD IF NOT EXISTS team_refs        ON project_card TYPE array<string>;
                 DEFINE FIELD IF NOT EXISTS default_user_ref ON project_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS created_at       ON project_card TYPE int;
                 DEFINE FIELD IF NOT EXISTS updated_at       ON project_card TYPE int;
                 DEFINE INDEX IF NOT EXISTS project_slug ON project_card FIELDS slug UNIQUE;
                 DEFINE INDEX IF NOT EXISTS project_root ON project_card FIELDS root_path;

                 DEFINE TABLE IF NOT EXISTS team_card SCHEMAFULL;
                 DEFINE FIELD IF NOT EXISTS actor_ref   ON team_card TYPE string;
                 DEFINE FIELD IF NOT EXISTS slug        ON team_card TYPE string;
                 DEFINE FIELD IF NOT EXISTS project_ref ON team_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS steward_ref ON team_card TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS member_refs ON team_card TYPE array<string>;
                 DEFINE FIELD IF NOT EXISTS policy      ON team_card TYPE object FLEXIBLE;
                 DEFINE FIELD IF NOT EXISTS created_at  ON team_card TYPE int;
                 DEFINE FIELD IF NOT EXISTS updated_at  ON team_card TYPE int;
                 DEFINE INDEX IF NOT EXISTS team_slug_project ON team_card FIELDS project_ref, slug;
                 DEFINE INDEX IF NOT EXISTS team_steward      ON team_card FIELDS steward_ref;";

impl SurrealCardStore {
    pub async fn init_schema(&self) -> Result<(), StoreError> {
        self.db
            .use_ns(NAMESPACE)
            .use_db(DATABASE)
            .await
            .map_err(|e| StoreError::Db(format!("selecting namespace/database: {e}")))?;

        self.db
            .query(CARD_DDL)
            .await
            .map_err(|e| StoreError::Db(format!("card schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("card schema init check: {e}")))?;

        Ok(())
    }

    async fn upsert_card<T>(&self, table: &str, id: &str, card: &T) -> Result<(), StoreError>
    where
        T: serde::Serialize + Send + Sync,
    {
        let mut content = serde_json::to_value(card)?;
        if let serde_json::Value::Object(ref mut obj) = content {
            obj.remove("id");
        }
        remove_top_level_null_fields(&mut content);

        let sql = format!("UPSERT type::record('{table}', $id) CONTENT $content");
        self.db
            .query(sql)
            .bind(("id", serde_json::json!(id)))
            .bind(("content", content))
            .await
            .map_err(|e| StoreError::Db(format!("upsert {table}: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("upsert {table} check: {e}")))?;

        Ok(())
    }

    async fn get_one<T>(
        &self,
        sql: &str,
        binds: Vec<(&str, serde_json::Value)>,
    ) -> Result<Option<T>, StoreError>
    where
        T: serde::de::DeserializeOwned,
    {
        let mut query = self.db.query(sql);
        for (name, value) in binds {
            query = query.bind((name.to_string(), value));
        }

        let mut result = query
            .await
            .map_err(|e| StoreError::Db(format!("card query: {e}")))?;
        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("card query read result: {e}")))?;

        row.map(decode_card).transpose()
    }

    async fn get_many<T>(
        &self,
        sql: &str,
        binds: Vec<(String, serde_json::Value)>,
    ) -> Result<Vec<T>, StoreError>
    where
        T: serde::de::DeserializeOwned,
    {
        let mut query = self.db.query(sql);
        for (name, value) in binds {
            query = query.bind((name, value));
        }

        let mut result = query
            .await
            .map_err(|e| StoreError::Db(format!("card list query: {e}")))?;
        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("card list read result: {e}")))?;

        rows.into_iter().map(decode_card).collect()
    }

    async fn active_agent_for_slug(&self, slug: &str) -> Result<Option<AgentCard>, StoreError> {
        self.get_one(
            "SELECT * FROM agent_card WHERE slug = $slug AND status != 'retired' LIMIT 1",
            vec![("slug", serde_json::json!(slug))],
        )
        .await
    }

    async fn agent_by_id(&self, id: &str) -> Result<Option<AgentCard>, StoreError> {
        self.get_one(
            "SELECT * FROM agent_card WHERE id = type::record('agent_card', $id) LIMIT 1",
            vec![("id", serde_json::json!(id))],
        )
        .await
    }
}

fn decode_card<T>(mut val: serde_json::Value) -> Result<T, StoreError>
where
    T: serde::de::DeserializeOwned,
{
    if let Some(id_val) = val.get("id")
        && let Some(id) = extract_record_id(id_val)
    {
        val["id"] = serde_json::Value::String(id);
    }

    serde_json::from_value(val).map_err(StoreError::from)
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

fn strip_table_prefix(raw: &str) -> &str {
    raw.split_once(':')
        .map_or(raw, |(_, id)| id)
        .trim_matches('`')
}

fn remove_top_level_null_fields(value: &mut serde_json::Value) {
    if let serde_json::Value::Object(obj) = value {
        obj.retain(|_, value| !value.is_null());
    }
}

#[async_trait::async_trait]
impl CardStore for SurrealCardStore {
    async fn init_schema(&self) -> Result<(), StoreError> {
        Self::init_schema(self).await
    }

    async fn upsert_actor(&self, card: ActorCard) -> Result<(), StoreError> {
        self.upsert_card("actor_card", &card.id, &card).await
    }

    async fn get_actor_by_address(&self, address: &str) -> Result<Option<ActorCard>, StoreError> {
        self.get_one(
            "SELECT * FROM actor_card WHERE address = $address LIMIT 1",
            vec![("address", serde_json::json!(address))],
        )
        .await
    }

    async fn list_actors(&self) -> Result<Vec<ActorCard>, StoreError> {
        self.get_many(
            "SELECT * FROM actor_card ORDER BY created_at ASC",
            Vec::new(),
        )
        .await
    }

    async fn upsert_user(&self, card: UserCard) -> Result<(), StoreError> {
        self.upsert_card("user_card", &card.id, &card).await
    }

    async fn get_user_by_address(&self, address: &str) -> Result<Option<UserCard>, StoreError> {
        self.get_one(
            "SELECT * FROM user_card WHERE address = $address LIMIT 1",
            vec![("address", serde_json::json!(address))],
        )
        .await
    }

    async fn upsert_agent(&self, card: AgentCard) -> Result<(), StoreError> {
        if let Some(existing) = self.active_agent_for_slug(&card.slug).await?
            && existing.id != card.id
            && !card.status.is_retired()
        {
            return Err(StoreError::Other(format!(
                "active agent slug '{}' already belongs to {}",
                card.slug, existing.id
            )));
        }

        self.upsert_card("agent_card", &card.id, &card).await
    }

    async fn get_agent_by_slug(&self, slug: &str) -> Result<Option<AgentCard>, StoreError> {
        self.active_agent_for_slug(slug).await
    }

    async fn list_agents(&self, filter: &AgentCardFilter) -> Result<Vec<AgentCard>, StoreError> {
        let mut conditions = Vec::new();
        let mut binds = Vec::new();

        if let Some(ref statuses) = filter.statuses
            && !statuses.is_empty()
        {
            let status_values: Vec<String> = statuses.iter().map(ToString::to_string).collect();
            conditions.push("status IN $statuses".to_string());
            binds.push(("statuses".into(), serde_json::json!(status_values)));
        }

        if let Some(ref project_ref) = filter.project_ref {
            conditions.push("project_refs CONTAINS $project_ref".to_string());
            binds.push(("project_ref".into(), serde_json::json!(project_ref)));
        }

        if let Some(ref team_ref) = filter.team_ref {
            conditions.push("team_refs CONTAINS $team_ref".to_string());
            binds.push(("team_ref".into(), serde_json::json!(team_ref)));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };
        let limit_clause = filter
            .limit
            .map_or(String::new(), |n| format!(" LIMIT {n}"));
        let sql =
            format!("SELECT * FROM agent_card{where_clause} ORDER BY created_at ASC{limit_clause}");

        self.get_many(&sql, binds).await
    }

    async fn update_agent_status(
        &self,
        id: &str,
        status: AgentStatus,
        updated_at: u64,
    ) -> Result<(), StoreError> {
        if self.agent_by_id(id).await?.is_none() {
            return Err(StoreError::Other(format!(
                "agent card '{id}' does not exist"
            )));
        }

        self.db
            .query("UPDATE type::record('agent_card', $id) SET status = $status, updated_at = $updated_at")
            .bind(("id", serde_json::json!(id)))
            .bind(("status", serde_json::json!(status.to_string())))
            .bind(("updated_at", serde_json::json!(updated_at)))
            .await
            .map_err(|e| StoreError::Db(format!("update agent status: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("update agent status check: {e}")))?;

        Ok(())
    }

    async fn update_agent_persona(
        &self,
        id: &str,
        persona: serde_json::Value,
        updated_at: u64,
    ) -> Result<(), StoreError> {
        if self.agent_by_id(id).await?.is_none() {
            return Err(StoreError::Other(format!(
                "agent card '{id}' does not exist"
            )));
        }
        self.db
            .query("UPDATE type::record('agent_card', $id) SET persona = $persona, updated_at = $updated_at")
            .bind(("id", serde_json::json!(id)))
            .bind(("persona", persona))
            .bind(("updated_at", serde_json::json!(updated_at)))
            .await
            .map_err(|e| StoreError::Db(format!("update agent persona: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("update agent persona check: {e}")))?;
        Ok(())
    }

    async fn update_agent_model(
        &self,
        id: &str,
        model: serde_json::Value,
        updated_at: u64,
    ) -> Result<(), StoreError> {
        if self.agent_by_id(id).await?.is_none() {
            return Err(StoreError::Other(format!(
                "agent card '{id}' does not exist"
            )));
        }
        self.db
            .query("UPDATE type::record('agent_card', $id) SET model = $model, updated_at = $updated_at")
            .bind(("id", serde_json::json!(id)))
            .bind(("model", model))
            .bind(("updated_at", serde_json::json!(updated_at)))
            .await
            .map_err(|e| StoreError::Db(format!("update agent model: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("update agent model check: {e}")))?;
        Ok(())
    }

    async fn record_agent_failure(
        &self,
        id: &str,
        reason: &str,
        at: u64,
    ) -> Result<(), StoreError> {
        if self.agent_by_id(id).await?.is_none() {
            return Err(StoreError::Other(format!(
                "agent card '{id}' does not exist"
            )));
        }
        self.db
            .query("UPDATE type::record('agent_card', $id) SET failure_reason = $reason, status = $status, updated_at = $at")
            .bind(("id", serde_json::json!(id)))
            .bind(("reason", serde_json::json!(reason)))
            .bind(("status", serde_json::json!(AgentStatus::Failed.to_string())))
            .bind(("at", serde_json::json!(at)))
            .await
            .map_err(|e| StoreError::Db(format!("record agent failure: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("record agent failure check: {e}")))?;
        Ok(())
    }

    async fn record_agent_heartbeat(&self, id: &str, seen_at: u64) -> Result<(), StoreError> {
        if self.agent_by_id(id).await?.is_none() {
            return Err(StoreError::Other(format!(
                "agent card '{id}' does not exist"
            )));
        }

        self.db
            .query("UPDATE type::record('agent_card', $id) SET last_seen_at = $seen_at, updated_at = $seen_at")
            .bind(("id", serde_json::json!(id)))
            .bind(("seen_at", serde_json::json!(seen_at)))
            .await
            .map_err(|e| StoreError::Db(format!("record agent heartbeat: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("record agent heartbeat check: {e}")))?;

        Ok(())
    }

    async fn upsert_project(&self, card: ProjectCard) -> Result<(), StoreError> {
        self.upsert_card("project_card", &card.id, &card).await
    }

    async fn get_project_by_slug(&self, slug: &str) -> Result<Option<ProjectCard>, StoreError> {
        self.get_one(
            "SELECT * FROM project_card WHERE slug = $slug LIMIT 1",
            vec![("slug", serde_json::json!(slug))],
        )
        .await
    }

    async fn upsert_team(&self, card: TeamCard) -> Result<(), StoreError> {
        if let Some(existing) = self
            .get_team_by_slug(card.project_ref.as_deref(), &card.slug)
            .await?
            && existing.id != card.id
        {
            return Err(StoreError::Other(format!(
                "team slug '{}' already belongs to {}",
                card.slug, existing.id
            )));
        }

        self.upsert_card("team_card", &card.id, &card).await
    }

    async fn get_team_by_slug(
        &self,
        project_ref: Option<&str>,
        slug: &str,
    ) -> Result<Option<TeamCard>, StoreError> {
        let mut binds = vec![("slug", serde_json::json!(slug))];
        let project_clause = match project_ref {
            Some(project_ref) => {
                binds.push(("project_ref", serde_json::json!(project_ref)));
                "project_ref = $project_ref"
            }
            None => "project_ref IS NONE",
        };
        let sql =
            format!("SELECT * FROM team_card WHERE slug = $slug AND {project_clause} LIMIT 1");

        self.get_one(&sql, binds).await
    }
}

#[cfg(test)]
mod tests {
    use daemon8_types::{ActorKind, AgentKind};
    use serde_json::json;

    use super::*;
    use crate::SurrealStore;

    async fn setup() -> (SurrealStore, SurrealCardStore) {
        let store = SurrealStore::memory().await.unwrap();
        let card_store = store.card_store();
        card_store.init_schema().await.unwrap();
        (store, card_store)
    }

    fn actor(id: &str, address: &str, actor_kind: ActorKind) -> ActorCard {
        ActorCard {
            id: id.into(),
            address: address.into(),
            actor_kind,
            slug: Some(id.into()),
            display_name: Some(address.into()),
            status: "active".into(),
            origin: json!({"surface": "test"}),
            refs: json!({"missing_ref": "project:missing"}),
            created_at: 1,
            updated_at: 1,
        }
    }

    fn user() -> UserCard {
        UserCard {
            id: "user-local".into(),
            actor_ref: "user.local".into(),
            address: "user.local".into(),
            display_name: Some("Local User".into()),
            communication: json!({"tool": "codex-cli"}),
            current_cwd: Some("/tmp/project".into()),
            current_project_ref: Some("project:daemon8".into()),
            inbox_address: "user.local".into(),
            last_read_cursor: Some("42".into()),
            preferences: json!({"tone": "brief"}),
            created_at: 1,
            updated_at: 2,
        }
    }

    fn agent(id: &str, slug: &str, status: AgentStatus) -> AgentCard {
        AgentCard {
            id: id.into(),
            actor_ref: format!("agent.{id}"),
            address: format!("agent.{id}"),
            slug: slug.into(),
            display_name: Some(slug.into()),
            agent_kind: AgentKind::Specialist,
            status,
            persona: json!({"ref": "persona:rust"}),
            model: json!({"provider": "test", "name": "small"}),
            capabilities: vec!["reads_code".into()],
            subjects_handled: vec!["rust".into()],
            project_refs: vec!["project:daemon8".into()],
            team_refs: vec!["team:core".into()],
            primary_team_ref: Some("team:core".into()),
            spawned_by_actor_ref: Some("user.local".into()),
            spawned_from_cwd: Some("/tmp/daemon8".into()),
            spawned_from_project_ref: Some("project:daemon8".into()),
            host_id: Some("host-1".into()),
            pid: Some(1234),
            parent_pid: Some(1000),
            process_group_id: Some(1234),
            executable_path: Some("/bin/daemon8".into()),
            argv_hash: Some("argv-hash".into()),
            runtime_kind: Some("rust".into()),
            runtime_version: Some("0.1.0".into()),
            launch_nonce: Some("nonce".into()),
            started_at: Some(10),
            last_seen_at: Some(20),
            heartbeat_interval_ms: Some(30_000),
            stop_state: json!({}),
            last_stop_request_at: None,
            last_exit_code: None,
            last_signal: None,
            cost_window_usd: 0.0,
            cost_total_usd: 0.0,
            budget_daily_usd: Some(2.0),
            failure_reason: None,
            created_at: 1,
            updated_at: 2,
        }
    }

    fn project() -> ProjectCard {
        ProjectCard {
            id: "project-daemon8".into(),
            actor_ref: "project.daemon8".into(),
            slug: "daemon8".into(),
            name: Some("daemon8".into()),
            root_path: Some("/tmp/daemon8".into()),
            config_path: Some("/tmp/daemon8/.daemon8.toml".into()),
            policy: json!({"hooks": "enabled"}),
            team_refs: vec!["team:core".into()],
            default_user_ref: Some("user.local".into()),
            created_at: 1,
            updated_at: 2,
        }
    }

    fn team() -> TeamCard {
        TeamCard {
            id: "team-core".into(),
            actor_ref: "team.core".into(),
            slug: "core".into(),
            project_ref: Some("project:daemon8".into()),
            steward_ref: Some("agent.steward".into()),
            member_refs: vec!["agent.rust".into()],
            policy: json!({"routing": "manual"}),
            created_at: 1,
            updated_at: 2,
        }
    }

    #[tokio::test]
    async fn actor_card_roundtrips_by_address() {
        let (_store, cards) = setup().await;
        cards
            .upsert_actor(actor("actor-user-local", "user.local", ActorKind::User))
            .await
            .unwrap();

        let found = cards
            .get_actor_by_address("user.local")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, "actor-user-local");
        assert_eq!(found.refs["missing_ref"], "project:missing");

        let all = cards.list_actors().await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn user_project_and_team_cards_roundtrip() {
        let (_store, cards) = setup().await;
        cards.upsert_user(user()).await.unwrap();
        cards.upsert_project(project()).await.unwrap();
        cards.upsert_team(team()).await.unwrap();

        assert_eq!(
            cards
                .get_user_by_address("user.local")
                .await
                .unwrap()
                .unwrap()
                .inbox_address,
            "user.local"
        );
        assert_eq!(
            cards
                .get_project_by_slug("daemon8")
                .await
                .unwrap()
                .unwrap()
                .team_refs,
            vec!["team:core"]
        );
        assert_eq!(
            cards
                .get_team_by_slug(Some("project:daemon8"), "core")
                .await
                .unwrap()
                .unwrap()
                .member_refs,
            vec!["agent.rust"]
        );
    }

    #[tokio::test]
    async fn agent_card_preserves_process_identity_and_loose_refs() {
        let (_store, cards) = setup().await;
        cards
            .upsert_agent(agent("agent-rust", "rust", AgentStatus::Alive))
            .await
            .unwrap();

        let found = cards.get_agent_by_slug("rust").await.unwrap().unwrap();
        assert_eq!(found.host_id.as_deref(), Some("host-1"));
        assert_eq!(found.pid, Some(1234));
        assert_eq!(found.started_at, Some(10));
        assert_eq!(found.executable_path.as_deref(), Some("/bin/daemon8"));
        assert_eq!(found.project_refs, vec!["project:daemon8"]);
    }

    #[tokio::test]
    async fn agents_can_be_filtered_and_heartbeat_updates_last_seen() {
        let (_store, cards) = setup().await;
        cards
            .upsert_agent(agent("agent-rust", "rust", AgentStatus::Alive))
            .await
            .unwrap();
        cards
            .upsert_agent(agent("agent-ts", "typescript", AgentStatus::Paused))
            .await
            .unwrap();

        let alive = cards
            .list_agents(&AgentCardFilter {
                statuses: Some(vec![AgentStatus::Alive]),
                project_ref: Some("project:daemon8".into()),
                team_ref: Some("team:core".into()),
                limit: None,
            })
            .await
            .unwrap();
        assert_eq!(alive.len(), 1);
        assert_eq!(alive[0].slug, "rust");

        let limited = cards
            .list_agents(&AgentCardFilter {
                statuses: None,
                project_ref: Some("project:daemon8".into()),
                team_ref: None,
                limit: Some(1),
            })
            .await
            .unwrap();
        assert_eq!(limited.len(), 1);

        cards
            .record_agent_heartbeat("agent-rust", 99)
            .await
            .unwrap();
        let found = cards.get_agent_by_slug("rust").await.unwrap().unwrap();
        assert_eq!(found.last_seen_at, Some(99));
        assert_eq!(found.updated_at, 99);
    }

    #[tokio::test]
    async fn status_update_keeps_identity_fields() {
        let (_store, cards) = setup().await;
        cards
            .upsert_agent(agent("agent-rust", "rust", AgentStatus::Alive))
            .await
            .unwrap();

        cards
            .update_agent_status("agent-rust", AgentStatus::Degraded, 88)
            .await
            .unwrap();
        let found = cards.get_agent_by_slug("rust").await.unwrap().unwrap();
        assert_eq!(found.status, AgentStatus::Degraded);
        assert_eq!(found.pid, Some(1234));
        assert_eq!(found.executable_path.as_deref(), Some("/bin/daemon8"));
        assert_eq!(found.updated_at, 88);
    }

    #[tokio::test]
    async fn lifecycle_updates_fail_for_missing_agents() {
        let (_store, cards) = setup().await;

        let status = cards
            .update_agent_status("missing-agent", AgentStatus::Degraded, 88)
            .await;
        assert!(status.is_err());

        let heartbeat = cards.record_agent_heartbeat("missing-agent", 99).await;
        assert!(heartbeat.is_err());

        let persona = cards
            .update_agent_persona(
                "missing-agent",
                serde_json::json!({"identity_prompt": "x"}),
                100,
            )
            .await;
        assert!(persona.is_err());

        let model = cards
            .update_agent_model(
                "missing-agent",
                serde_json::json!({"provider": "ollama"}),
                101,
            )
            .await;
        assert!(model.is_err());

        let failure = cards
            .record_agent_failure("missing-agent", "boom", 102)
            .await;
        assert!(failure.is_err());
    }

    #[tokio::test]
    async fn update_agent_persona_replaces_field_and_bumps_updated_at() {
        let (_store, cards) = setup().await;
        cards
            .upsert_agent(agent("agent-rust", "rust", AgentStatus::Alive))
            .await
            .unwrap();

        let new_persona = serde_json::json!({
            "identity_prompt": "you are a rust borrow-checker specialist",
            "extra": [1, 2, 3],
        });
        cards
            .update_agent_persona("agent-rust", new_persona.clone(), 4242)
            .await
            .unwrap();

        let found = cards.get_agent_by_slug("rust").await.unwrap().unwrap();
        assert_eq!(found.persona, new_persona);
        assert_eq!(found.updated_at, 4242);
        // Identity fields preserved.
        assert_eq!(found.pid, Some(1234));
        assert_eq!(found.executable_path.as_deref(), Some("/bin/daemon8"));
    }

    #[tokio::test]
    async fn update_agent_model_replaces_model_block() {
        let (_store, cards) = setup().await;
        cards
            .upsert_agent(agent("agent-rust", "rust", AgentStatus::Alive))
            .await
            .unwrap();

        let new_model = serde_json::json!({
            "provider": "openrouter",
            "model": "openai/gpt-4o-mini",
            "temperature": 0.4,
        });
        cards
            .update_agent_model("agent-rust", new_model.clone(), 5151)
            .await
            .unwrap();

        let found = cards.get_agent_by_slug("rust").await.unwrap().unwrap();
        assert_eq!(found.model, new_model);
        assert_eq!(found.updated_at, 5151);
        assert_eq!(found.status, AgentStatus::Alive);
    }

    #[tokio::test]
    async fn record_agent_failure_drives_status_to_failed_and_writes_reason() {
        let (_store, cards) = setup().await;
        cards
            .upsert_agent(agent("agent-rust", "rust", AgentStatus::Alive))
            .await
            .unwrap();

        cards
            .record_agent_failure("agent-rust", "missing API key for env var FOO", 6262)
            .await
            .unwrap();

        let found = cards.get_agent_by_slug("rust").await.unwrap().unwrap();
        assert_eq!(found.status, AgentStatus::Failed);
        assert_eq!(found.updated_at, 6262);
        assert_eq!(
            found.failure_reason.as_deref(),
            Some("missing API key for env var FOO")
        );
    }

    #[tokio::test]
    async fn active_agent_slug_is_unique_but_retired_slug_can_be_reused() {
        let (_store, cards) = setup().await;
        cards
            .upsert_agent(agent("agent-rust-1", "rust", AgentStatus::Alive))
            .await
            .unwrap();

        let duplicate = cards
            .upsert_agent(agent("agent-rust-2", "rust", AgentStatus::Created))
            .await;
        assert!(duplicate.is_err());

        cards
            .update_agent_status("agent-rust-1", AgentStatus::Retired, 100)
            .await
            .unwrap();
        cards
            .upsert_agent(agent("agent-rust-2", "rust", AgentStatus::Created))
            .await
            .unwrap();

        let found = cards.get_agent_by_slug("rust").await.unwrap().unwrap();
        assert_eq!(found.id, "agent-rust-2");
    }

    #[tokio::test]
    async fn nested_nulls_in_flexible_json_are_preserved() {
        let (_store, cards) = setup().await;
        let mut card = agent("agent-rust", "rust", AgentStatus::Alive);
        card.display_name = None;
        card.persona = json!({
            "style": null,
            "nested": {
                "preference": null,
                "name": "rust"
            }
        });

        cards.upsert_agent(card).await.unwrap();

        let found = cards.get_agent_by_slug("rust").await.unwrap().unwrap();
        assert!(found.display_name.is_none());
        assert!(found.persona["style"].is_null());
        assert!(found.persona["nested"]["preference"].is_null());
        assert_eq!(found.persona["nested"]["name"], "rust");
    }

    #[tokio::test]
    async fn team_slug_is_unique_within_project_scope() {
        let (_store, cards) = setup().await;
        cards.upsert_team(team()).await.unwrap();

        let mut duplicate = team();
        duplicate.id = "team-core-duplicate".into();
        let result = cards.upsert_team(duplicate).await;
        assert!(result.is_err());

        let mut other_project = team();
        other_project.id = "team-core-other-project".into();
        other_project.project_ref = Some("project:other".into());
        cards.upsert_team(other_project).await.unwrap();

        let mut global_team = team();
        global_team.id = "team-core-global".into();
        global_team.project_ref = None;
        cards.upsert_team(global_team).await.unwrap();

        let found = cards.get_team_by_slug(None, "core").await.unwrap().unwrap();
        assert_eq!(found.id, "team-core-global");
    }
}
