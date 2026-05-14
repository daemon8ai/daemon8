// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

use surrealdb::Surreal;
use surrealdb::engine::local::Db;

use daemon8_types::{
    AwarenessAuthority, AwarenessEdgeKind, AwarenessNodeKind, AwarenessNodeState,
    AwarenessOperation,
};

use crate::{
    AwarenessConflict, AwarenessEdge, AwarenessFilter, AwarenessManifest, AwarenessNode,
    AwarenessRedex, AwarenessRef, AwarenessSignal, AwarenessSignalInput, AwarenessStore,
    AwarenessSync, AwarenessSyncResult, AwarenessTraversalFilter, AwarenessTree, StoreError,
};

const NAMESPACE: &str = "daemon8";
const DATABASE: &str = "observations";

pub struct SurrealAwarenessStore {
    db: Surreal<Db>,
}

impl SurrealAwarenessStore {
    pub fn new(db: Surreal<Db>) -> Self {
        Self { db }
    }

    pub async fn init_schema(&self) -> Result<(), StoreError> {
        self.db
            .use_ns(NAMESPACE)
            .use_db(DATABASE)
            .await
            .map_err(|e| StoreError::Db(format!("selecting namespace/database: {e}")))?;

        self.db
            .query(
                "DEFINE TABLE IF NOT EXISTS awareness_node SCHEMAFULL;
                 DEFINE FIELD IF NOT EXISTS project_slug       ON awareness_node TYPE string;
                 DEFINE FIELD IF NOT EXISTS path               ON awareness_node TYPE string;
                 DEFINE FIELD IF NOT EXISTS kind               ON awareness_node TYPE string;
                 DEFINE FIELD IF NOT EXISTS state              ON awareness_node TYPE string;
                 DEFINE FIELD IF NOT EXISTS authority          ON awareness_node TYPE string;
                 DEFINE FIELD IF NOT EXISTS confidence         ON awareness_node TYPE float;
                 DEFINE FIELD IF NOT EXISTS summary            ON awareness_node TYPE string;
                 DEFINE FIELD IF NOT EXISTS note               ON awareness_node TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS redex              ON awareness_node TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS redex_derived      ON awareness_node TYPE option<object>;
                 DEFINE FIELD IF NOT EXISTS redex_derived.state      ON awareness_node TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS redex_derived.recovery   ON awareness_node TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS redex_derived.implication ON awareness_node TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS redex_derived.temporal   ON awareness_node TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS redex_derived.persistent ON awareness_node TYPE option<bool>;
                 DEFINE FIELD IF NOT EXISTS redex_derived.conflict   ON awareness_node TYPE option<bool>;
                 DEFINE FIELD IF NOT EXISTS tags               ON awareness_node TYPE array<string>;
                 DEFINE FIELD IF NOT EXISTS debug_session_id   ON awareness_node TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS checkpoint_id      ON awareness_node TYPE option<string>;
                 DEFINE FIELD IF NOT EXISTS debug_session_ids  ON awareness_node TYPE array<string>;
                 DEFINE FIELD IF NOT EXISTS checkpoint_ids     ON awareness_node TYPE array<string>;
                 DEFINE FIELD IF NOT EXISTS evidence_refs      ON awareness_node TYPE array<any>;
                 DEFINE FIELD IF NOT EXISTS evidence_refs.*.kind ON awareness_node TYPE string;
                 DEFINE FIELD IF NOT EXISTS evidence_refs.*.id   ON awareness_node TYPE string;
                 DEFINE FIELD IF NOT EXISTS signal_refs        ON awareness_node TYPE array<any>;
                 DEFINE FIELD IF NOT EXISTS signal_refs.*.kind ON awareness_node TYPE string;
                 DEFINE FIELD IF NOT EXISTS signal_refs.*.id   ON awareness_node TYPE string;
                 DEFINE FIELD IF NOT EXISTS observation_ids    ON awareness_node TYPE array<int>;
                 DEFINE FIELD IF NOT EXISTS librarian_node_ids ON awareness_node TYPE array<string>;
                 DEFINE FIELD IF NOT EXISTS created_at         ON awareness_node TYPE int;
                 DEFINE FIELD IF NOT EXISTS updated_at         ON awareness_node TYPE int;
                 DEFINE FIELD IF NOT EXISTS resolved_at        ON awareness_node TYPE option<int>;
                 DEFINE FIELD IF NOT EXISTS retired_at         ON awareness_node TYPE option<int>;
                 DEFINE FIELD IF NOT EXISTS stale_at           ON awareness_node TYPE option<int>;
                 DEFINE INDEX IF NOT EXISTS idx_aw_project    ON awareness_node FIELDS project_slug;
                 DEFINE INDEX IF NOT EXISTS idx_aw_kind       ON awareness_node FIELDS kind;
                 DEFINE INDEX IF NOT EXISTS idx_aw_state      ON awareness_node FIELDS state;
                 DEFINE INDEX IF NOT EXISTS idx_aw_authority  ON awareness_node FIELDS authority;
                 DEFINE INDEX IF NOT EXISTS idx_aw_path       ON awareness_node FIELDS path;
                 DEFINE INDEX IF NOT EXISTS idx_aw_debug      ON awareness_node FIELDS debug_session_id;
                 DEFINE INDEX IF NOT EXISTS idx_aw_checkpoint ON awareness_node FIELDS checkpoint_id;
                 DEFINE INDEX IF NOT EXISTS idx_aw_updated    ON awareness_node FIELDS updated_at;

                 DEFINE TABLE IF NOT EXISTS awareness_edge SCHEMAFULL TYPE RELATION
                   FROM awareness_node TO awareness_node;
                 DEFINE FIELD IF NOT EXISTS kind       ON awareness_edge TYPE string;
                 DEFINE FIELD IF NOT EXISTS created_at ON awareness_edge TYPE int;
                 DEFINE INDEX IF NOT EXISTS idx_ae_kind ON awareness_edge FIELDS kind;

                 DEFINE TABLE IF NOT EXISTS awareness_signal SCHEMAFULL;
                 DEFINE FIELD IF NOT EXISTS project_slug               ON awareness_signal TYPE string;
                 DEFINE FIELD IF NOT EXISTS signal_kind                ON awareness_signal TYPE string;
                 DEFINE FIELD IF NOT EXISTS signal_key                 ON awareness_signal TYPE string;
                 DEFINE FIELD IF NOT EXISTS severity                   ON awareness_signal TYPE string;
                 DEFINE FIELD IF NOT EXISTS summary                    ON awareness_signal TYPE string;
                 DEFINE FIELD IF NOT EXISTS signal_refs                ON awareness_signal TYPE array<any>;
                 DEFINE FIELD IF NOT EXISTS signal_refs.*.kind         ON awareness_signal TYPE string;
                 DEFINE FIELD IF NOT EXISTS signal_refs.*.id           ON awareness_signal TYPE string;
                 DEFINE FIELD IF NOT EXISTS related_awareness_node_ids ON awareness_signal TYPE array<string>;
                 DEFINE FIELD IF NOT EXISTS score                      ON awareness_signal TYPE float;
                 DEFINE FIELD IF NOT EXISTS surfaced_at                ON awareness_signal TYPE option<int>;
                 DEFINE FIELD IF NOT EXISTS expires_at                 ON awareness_signal TYPE int;
                 DEFINE FIELD IF NOT EXISTS created_at                 ON awareness_signal TYPE int;
                 DEFINE INDEX IF NOT EXISTS idx_as_project ON awareness_signal FIELDS project_slug;
                 DEFINE INDEX IF NOT EXISTS idx_as_key     ON awareness_signal FIELDS project_slug, signal_kind, signal_key;
                 DEFINE INDEX IF NOT EXISTS idx_as_expires ON awareness_signal FIELDS expires_at;
                 DEFINE INDEX IF NOT EXISTS idx_as_score   ON awareness_signal FIELDS score;",
            )
            .await
            .map_err(|e| StoreError::Db(format!("awareness schema init: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("awareness schema init check: {e}")))?;

        Ok(())
    }

    async fn active_nodes_for_path(
        &self,
        project_slug: &str,
        path: &str,
    ) -> Result<Vec<AwarenessNode>, StoreError> {
        let mut result = self
            .db
            .query(
                "SELECT * FROM awareness_node
                 WHERE project_slug = $project_slug
                   AND path = $path
                   AND state = 'active'
                 ORDER BY updated_at DESC",
            )
            .bind(("project_slug", serde_json::json!(project_slug)))
            .bind(("path", serde_json::json!(path)))
            .await
            .map_err(|e| StoreError::Db(format!("active awareness lookup: {e}")))?;
        parse_nodes(
            result
                .take(0)
                .map_err(|e| StoreError::Db(format!("active awareness lookup read: {e}")))?,
        )
    }

    async fn latest_node_for_path(
        &self,
        project_slug: &str,
        path: &str,
    ) -> Result<Option<AwarenessNode>, StoreError> {
        let mut result = self
            .db
            .query(
                "SELECT * FROM awareness_node
                 WHERE project_slug = $project_slug AND path = $path
                 ORDER BY updated_at DESC
                 LIMIT 1",
            )
            .bind(("project_slug", serde_json::json!(project_slug)))
            .bind(("path", serde_json::json!(path)))
            .await
            .map_err(|e| StoreError::Db(format!("latest awareness lookup: {e}")))?;
        let rows = parse_nodes(
            result
                .take(0)
                .map_err(|e| StoreError::Db(format!("latest awareness lookup read: {e}")))?,
        )?;
        Ok(rows.into_iter().next())
    }

    async fn find_target(
        &self,
        input: &AwarenessSync,
    ) -> Result<Option<AwarenessNode>, StoreError> {
        if let Some(ref id) = input.target_node_id {
            return self.get_node(id).await;
        }
        self.latest_node_for_path(&input.project_slug, &input.path)
            .await
    }

    async fn create_node(&self, node: &AwarenessNode) -> Result<String, StoreError> {
        let mut result = self
            .db
            .query("CREATE awareness_node CONTENT $content")
            .bind(("content", serde_json::to_value(node)?))
            .await
            .map_err(|e| StoreError::Db(format!("create awareness node: {e}")))?;
        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("create awareness node read: {e}")))?;
        row.as_ref()
            .and_then(|v| v.get("id"))
            .and_then(|id| extract_record_id(id, "awareness_node"))
            .ok_or_else(|| StoreError::Db("create awareness node: no id returned".into()))
    }

    async fn update_node(&self, node: &AwarenessNode) -> Result<(), StoreError> {
        let id = node
            .id
            .as_deref()
            .ok_or_else(|| StoreError::Other("awareness update requires id".into()))?;
        self.db
            .query("UPDATE type::record('awareness_node', $id) CONTENT $content")
            .bind(("id", serde_json::json!(id)))
            .bind(("content", serde_json::to_value(node)?))
            .await
            .map_err(|e| StoreError::Db(format!("update awareness node: {e}")))?
            .check()
            .map_err(|e| StoreError::Db(format!("update awareness node check: {e}")))?;
        Ok(())
    }

    async fn create_edge(
        &self,
        from_node: &str,
        to_node: &str,
        kind: AwarenessEdgeKind,
        now: u64,
    ) -> Result<AwarenessEdge, StoreError> {
        validate_record_key(from_node)?;
        validate_record_key(to_node)?;
        let sql = format!(
            "RELATE awareness_node:{from_node}->awareness_edge->awareness_node:{to_node}
             SET kind = $kind, created_at = $created_at"
        );
        let mut result = self
            .db
            .query(&sql)
            .bind(("kind", serde_json::json!(kind.to_string())))
            .bind(("created_at", serde_json::json!(now)))
            .await
            .map_err(|e| StoreError::Db(format!("create awareness edge: {e}")))?;
        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("create awareness edge read: {e}")))?;
        row.map(parse_edge)
            .transpose()?
            .ok_or_else(|| StoreError::Db("create awareness edge: no row returned".into()))
    }

    async fn create_edges_for_input(
        &self,
        node_id: &str,
        input: &AwarenessSync,
        now: u64,
    ) -> Result<Vec<AwarenessEdge>, StoreError> {
        let mut edges = Vec::new();
        for id in &input.supersedes {
            edges.push(
                self.create_edge(node_id, id, AwarenessEdgeKind::Supersedes, now)
                    .await?,
            );
        }
        for id in &input.answers {
            edges.push(
                self.create_edge(node_id, id, AwarenessEdgeKind::Answers, now)
                    .await?,
            );
        }
        for id in &input.contradicts {
            edges.push(
                self.create_edge(node_id, id, AwarenessEdgeKind::Contradicts, now)
                    .await?,
            );
        }
        Ok(edges)
    }

    async fn validate_edge_targets(&self, input: &AwarenessSync) -> Result<(), StoreError> {
        let mut ids = BTreeSet::new();
        ids.extend(input.supersedes.iter().map(String::as_str));
        ids.extend(input.answers.iter().map(String::as_str));
        ids.extend(input.contradicts.iter().map(String::as_str));

        for id in ids {
            validate_record_key(id)?;
            if self.get_node(id).await?.is_none() {
                return Err(StoreError::Other(format!(
                    "awareness edge target does not exist: {id}"
                )));
            }
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl AwarenessStore for SurrealAwarenessStore {
    async fn sync_node(&self, input: AwarenessSync) -> Result<AwarenessSyncResult, StoreError> {
        let input = normalize_sync(input)?;
        if input.operation == AwarenessOperation::Verify && input.refs.evidence_refs.is_empty() {
            return Err(StoreError::Other(
                "awareness verify requires at least one durable evidence_ref".into(),
            ));
        }
        self.validate_edge_targets(&input).await?;

        let now = current_ns();
        let mut conflict = None;
        if input.operation == AwarenessOperation::Capture && !input.contradicts.is_empty() {
            let existing = self
                .active_nodes_for_path(&input.project_slug, &input.path)
                .await?;
            let superseded: BTreeSet<&str> = input.supersedes.iter().map(String::as_str).collect();
            let target = input.target_node_id.as_deref();
            let conflicts = existing
                .into_iter()
                .filter(|node| {
                    node.id
                        .as_deref()
                        .is_none_or(|id| !superseded.contains(id) && Some(id) != target)
                })
                .collect::<Vec<_>>();
            if !conflicts.is_empty() {
                conflict = Some(AwarenessConflict {
                    reason:
                        "incoming node explicitly contradicts active awareness on the same path"
                            .into(),
                    incoming_path: input.path.clone(),
                    existing_nodes: conflicts,
                });
            }
        }

        let node = match input.operation {
            AwarenessOperation::Capture | AwarenessOperation::Question => {
                let mut node = new_node_from_input(&input, now);
                if conflict.is_some() {
                    node.state = AwarenessNodeState::Conflicted;
                }
                if input.operation == AwarenessOperation::Question {
                    node.kind = AwarenessNodeKind::Question;
                    node.authority = AwarenessAuthority::Question;
                }
                let id = self.create_node(&node).await?;
                node.id = Some(id);
                node
            }
            AwarenessOperation::Update
            | AwarenessOperation::Resolve
            | AwarenessOperation::Verify
            | AwarenessOperation::Retire => {
                let mut node = self.find_target(&input).await?.ok_or_else(|| {
                    StoreError::Other(format!(
                        "awareness {} requires an existing node",
                        input.operation
                    ))
                })?;
                apply_input_to_node(&mut node, &input, now);
                self.update_node(&node).await?;
                node
            }
        };

        let node_id = node
            .id
            .as_deref()
            .ok_or_else(|| StoreError::Other("awareness node id missing after sync".into()))?;
        let edges = self.create_edges_for_input(node_id, &input, now).await?;
        Ok(AwarenessSyncResult {
            node: Some(node),
            edges,
            conflict,
        })
    }

    async fn get_node(&self, id: &str) -> Result<Option<AwarenessNode>, StoreError> {
        let mut result = self
            .db
            .query("SELECT * FROM type::record('awareness_node', $id)")
            .bind(("id", serde_json::json!(id)))
            .await
            .map_err(|e| StoreError::Db(format!("get awareness node: {e}")))?;
        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("get awareness node read: {e}")))?;
        row.map(parse_node).transpose()
    }

    async fn manifest(&self, filter: &AwarenessFilter) -> Result<AwarenessManifest, StoreError> {
        let states = if filter.include_inactive {
            Vec::new()
        } else {
            vec![AwarenessNodeState::Active, AwarenessNodeState::Conflicted]
        };
        let nodes = query_nodes(
            &self.db,
            &filter.project_slug,
            None,
            &states,
            filter.limit.unwrap_or(500),
        )
        .await?;

        let mut counts_by_kind = BTreeMap::new();
        let mut active_objectives = Vec::new();
        let mut open_questions = Vec::new();
        let mut active_hypotheses = Vec::new();
        let mut suggested = BTreeSet::new();
        let mut stale_risk_count = 0;
        let mut conflict_count = 0;

        for node in &nodes {
            *counts_by_kind.entry(node.kind.to_string()).or_insert(0) += 1;
            match node.state {
                AwarenessNodeState::Conflicted => {
                    conflict_count += 1;
                    suggested.insert(node.path.clone());
                }
                AwarenessNodeState::Stale => {
                    stale_risk_count += 1;
                    suggested.insert(node.path.clone());
                }
                AwarenessNodeState::Active => match node.kind {
                    AwarenessNodeKind::Objective => {
                        if active_objectives.len() < 5 {
                            active_objectives.push(node.clone());
                        }
                        suggested.insert(node.path.clone());
                    }
                    AwarenessNodeKind::Question | AwarenessNodeKind::Blocker => {
                        if open_questions.len() < 5 {
                            open_questions.push(node.clone());
                        }
                        suggested.insert(node.path.clone());
                    }
                    AwarenessNodeKind::Hypothesis => {
                        if active_hypotheses.len() < 5 {
                            active_hypotheses.push(node.clone());
                        }
                    }
                    AwarenessNodeKind::Risk => {
                        stale_risk_count += 1;
                        suggested.insert(node.path.clone());
                    }
                    _ => {}
                },
                AwarenessNodeState::Resolved | AwarenessNodeState::Retired => {}
            }
        }

        Ok(AwarenessManifest {
            project_slug: filter.project_slug.clone(),
            counts_by_kind,
            active_objectives,
            open_questions,
            active_hypotheses,
            stale_risk_count,
            conflict_count,
            suggested_focus_paths: suggested.into_iter().take(8).collect(),
        })
    }

    async fn record_signal(
        &self,
        input: AwarenessSignalInput,
    ) -> Result<AwarenessSignal, StoreError> {
        let signal_refs = normalize_refs(input.signal_refs, RefRole::Signal)?;
        if signal_refs.is_empty() {
            return Err(StoreError::Other(
                "awareness signal requires at least one signal_ref".into(),
            ));
        }
        let signal_key = refs_key_string(&signal_refs);
        let id = signal_record_id(&input.project_slug, &input.signal_kind, &signal_key);
        let signal = AwarenessSignal {
            id: Some(id.clone()),
            project_slug: input.project_slug,
            signal_kind: input.signal_kind,
            signal_key,
            severity: input.severity,
            summary: input.summary,
            signal_refs,
            related_awareness_node_ids: input.related_awareness_node_ids,
            score: input.score.clamp(0.0, 1.0),
            surfaced_at: None,
            expires_at: input.expires_at,
            created_at: current_ns(),
        };
        let mut result = self
            .db
            .query("UPSERT type::record('awareness_signal', $id) CONTENT $content")
            .bind(("id", serde_json::json!(id)))
            .bind(("content", serde_json::to_value(&signal)?))
            .await
            .map_err(|e| StoreError::Db(format!("upsert awareness signal: {e}")))?;
        let row: Option<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("upsert awareness signal read: {e}")))?;
        row.map(parse_signal)
            .transpose()?
            .ok_or_else(|| StoreError::Db("upsert awareness signal: no row returned".into()))
    }

    async fn active_signals(
        &self,
        project_slug: &str,
        limit: usize,
    ) -> Result<Vec<AwarenessSignal>, StoreError> {
        self.prune_expired_signals(current_ns()).await?;

        let mut result = self
            .db
            .query(
                "SELECT * FROM awareness_signal
                 WHERE project_slug = $project_slug AND expires_at > $now
                 ORDER BY score DESC, created_at DESC
                 LIMIT $limit",
            )
            .bind(("project_slug", serde_json::json!(project_slug)))
            .bind(("now", serde_json::json!(current_ns())))
            .bind(("limit", serde_json::json!(limit)))
            .await
            .map_err(|e| StoreError::Db(format!("query awareness signals: {e}")))?;
        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("query awareness signals read: {e}")))?;
        rows.into_iter().map(parse_signal).collect()
    }

    async fn prune_expired_signals(&self, now: u64) -> Result<u64, StoreError> {
        let mut result = self
            .db
            .query("DELETE awareness_signal WHERE expires_at <= $now RETURN BEFORE")
            .bind(("now", serde_json::json!(now)))
            .await
            .map_err(|e| StoreError::Db(format!("delete expired awareness signals: {e}")))?;

        let rows: Vec<serde_json::Value> = result
            .take(0)
            .map_err(|e| StoreError::Db(format!("delete expired awareness signals read: {e}")))?;

        Ok(rows.len() as u64)
    }

    async fn traverse(
        &self,
        filter: &AwarenessTraversalFilter,
    ) -> Result<AwarenessTree, StoreError> {
        let states = if filter.include_inactive {
            Vec::new()
        } else {
            vec![AwarenessNodeState::Active, AwarenessNodeState::Conflicted]
        };
        let mut nodes = query_nodes(
            &self.db,
            &filter.project_slug,
            Some(&filter.focus_path),
            &states,
            filter.limit.unwrap_or(100),
        )
        .await?;
        nodes.retain(|node| path_depth_within(&filter.focus_path, &node.path, filter.depth));
        if !filter.include_notes {
            for node in &mut nodes {
                node.note = None;
                node.redex = None;
                node.redex_derived = None;
            }
        }
        if !filter.include_evidence {
            for node in &mut nodes {
                node.observation_ids.clear();
                node.librarian_node_ids.clear();
                node.debug_session_ids.clear();
                node.checkpoint_ids.clear();
                node.evidence_refs.clear();
                node.signal_refs.clear();
                node.debug_session_id = None;
                node.checkpoint_id = None;
            }
        }

        let mut edges = Vec::new();
        let ids: BTreeSet<&str> = nodes.iter().filter_map(|node| node.id.as_deref()).collect();
        for id in &ids {
            edges.extend(query_edges_for_node(&self.db, id).await?);
        }
        edges.retain(|edge| {
            ids.contains(edge.from_node.as_str()) && ids.contains(edge.to_node.as_str())
        });

        Ok(AwarenessTree {
            project_slug: filter.project_slug.clone(),
            focus_path: filter.focus_path.clone(),
            nodes,
            edges,
        })
    }
}

fn new_node_from_input(input: &AwarenessSync, now: u64) -> AwarenessNode {
    let signal_refs = awareness_signal_refs(input);
    AwarenessNode {
        id: None,
        project_slug: input.project_slug.clone(),
        path: input.path.clone(),
        kind: input.kind,
        state: AwarenessNodeState::Active,
        authority: input
            .authority
            .unwrap_or_else(|| default_authority(input.kind)),
        confidence: input.confidence.unwrap_or(0.5).clamp(0.0, 1.0),
        summary: input.summary.clone().unwrap_or_else(|| input.path.clone()),
        note: input.note.clone(),
        redex: input.redex.clone(),
        redex_derived: input.redex.as_deref().map(derive_redex),
        tags: input.tags.clone(),
        debug_session_id: input.debug_session_id.clone(),
        checkpoint_id: input.checkpoint_id.clone(),
        debug_session_ids: legacy_ref_ids(&signal_refs, "debug_session"),
        checkpoint_ids: legacy_ref_ids(&signal_refs, "checkpoint"),
        evidence_refs: input.refs.evidence_refs.clone(),
        signal_refs: signal_refs.clone(),
        observation_ids: legacy_observation_ids(&signal_refs),
        librarian_node_ids: legacy_ref_ids(&input.refs.evidence_refs, "librarian_node"),
        created_at: now,
        updated_at: now,
        resolved_at: None,
        retired_at: None,
        stale_at: None,
    }
}

fn apply_input_to_node(node: &mut AwarenessNode, input: &AwarenessSync, now: u64) {
    if let Some(authority) = input.authority {
        node.authority = authority;
    }
    if let Some(confidence) = input.confidence {
        node.confidence = confidence.clamp(0.0, 1.0);
    }
    if let Some(summary) = &input.summary {
        node.summary = summary.clone();
    }
    if input.note.is_some() {
        node.note = input.note.clone();
    }
    if input.redex.is_some() {
        node.redex = input.redex.clone();
        node.redex_derived = input.redex.as_deref().map(derive_redex);
    }
    let signal_refs = awareness_signal_refs(input);
    extend_unique(&mut node.tags, &input.tags);
    extend_unique_refs(&mut node.evidence_refs, &input.refs.evidence_refs);
    extend_unique_refs(&mut node.signal_refs, &signal_refs);
    extend_unique_u64(
        &mut node.observation_ids,
        &legacy_observation_ids(&signal_refs),
    );
    extend_unique(
        &mut node.debug_session_ids,
        &legacy_ref_ids(&signal_refs, "debug_session"),
    );
    extend_unique(
        &mut node.checkpoint_ids,
        &legacy_ref_ids(&signal_refs, "checkpoint"),
    );
    extend_unique(
        &mut node.librarian_node_ids,
        &legacy_ref_ids(&input.refs.evidence_refs, "librarian_node"),
    );
    if node.debug_session_id.is_none() {
        node.debug_session_id = input.debug_session_id.clone();
    }
    if node.checkpoint_id.is_none() {
        node.checkpoint_id = input.checkpoint_id.clone();
    }
    match input.operation {
        AwarenessOperation::Resolve => {
            node.state = AwarenessNodeState::Resolved;
            node.resolved_at = Some(now);
        }
        AwarenessOperation::Verify => {
            node.authority = AwarenessAuthority::Verified;
            node.confidence = input.confidence.unwrap_or(1.0).clamp(0.0, 1.0);
            node.state = AwarenessNodeState::Active;
        }
        AwarenessOperation::Retire => {
            node.state = AwarenessNodeState::Retired;
            node.retired_at = Some(now);
        }
        AwarenessOperation::Update | AwarenessOperation::Capture | AwarenessOperation::Question => {
        }
    }
    node.updated_at = now;
}

async fn query_nodes(
    db: &Surreal<Db>,
    project_slug: &str,
    focus_path: Option<&str>,
    states: &[AwarenessNodeState],
    limit: usize,
) -> Result<Vec<AwarenessNode>, StoreError> {
    let mut conditions = vec!["project_slug = $project_slug".to_string()];
    if !states.is_empty() {
        conditions.push("state IN $states".to_string());
    }
    if focus_path.is_some() {
        conditions.push("(path = $focus_path OR string::starts_with(path, $path_prefix))".into());
    }
    let sql = format!(
        "SELECT * FROM awareness_node WHERE {} ORDER BY updated_at DESC LIMIT {limit}",
        conditions.join(" AND ")
    );
    let mut query = db
        .query(&sql)
        .bind(("project_slug", serde_json::json!(project_slug)));
    if !states.is_empty() {
        let states = states.iter().map(ToString::to_string).collect::<Vec<_>>();
        query = query.bind(("states", serde_json::json!(states)));
    }
    if let Some(path) = focus_path {
        query = query
            .bind(("focus_path", serde_json::json!(path)))
            .bind(("path_prefix", serde_json::json!(format!("{path}/"))));
    }
    let mut result = query
        .await
        .map_err(|e| StoreError::Db(format!("query awareness nodes: {e}")))?;
    parse_nodes(
        result
            .take(0)
            .map_err(|e| StoreError::Db(format!("query awareness nodes read: {e}")))?,
    )
}

async fn query_edges_for_node(
    db: &Surreal<Db>,
    node_id: &str,
) -> Result<Vec<AwarenessEdge>, StoreError> {
    validate_record_key(node_id)?;
    let mut result = db
        .query(
            "SELECT * FROM awareness_edge
             WHERE in = type::record('awareness_node', $id)
                OR out = type::record('awareness_node', $id)",
        )
        .bind(("id", serde_json::json!(node_id)))
        .await
        .map_err(|e| StoreError::Db(format!("query awareness edges: {e}")))?;
    let rows: Vec<serde_json::Value> = result
        .take(0)
        .map_err(|e| StoreError::Db(format!("query awareness edges read: {e}")))?;
    rows.into_iter().map(parse_edge).collect()
}

fn parse_nodes(rows: Vec<serde_json::Value>) -> Result<Vec<AwarenessNode>, StoreError> {
    rows.into_iter().map(parse_node).collect()
}

fn parse_node(mut val: serde_json::Value) -> Result<AwarenessNode, StoreError> {
    if let Some(id_val) = val.get("id")
        && let Some(bare) = extract_record_id(id_val, "awareness_node")
    {
        val["id"] = serde_json::Value::String(bare);
    }
    serde_json::from_value(val).map_err(StoreError::from)
}

fn parse_edge(mut val: serde_json::Value) -> Result<AwarenessEdge, StoreError> {
    if let Some(id_val) = val.get("id")
        && let Some(bare) = extract_record_id(id_val, "awareness_edge")
    {
        val["id"] = serde_json::Value::String(bare);
    }
    if let Some(in_val) = val.get("in")
        && let Some(bare) = extract_record_id(in_val, "awareness_node")
    {
        val["from_node"] = serde_json::Value::String(bare);
    }
    if let Some(out_val) = val.get("out")
        && let Some(bare) = extract_record_id(out_val, "awareness_node")
    {
        val["to_node"] = serde_json::Value::String(bare);
    }
    if let Some(obj) = val.as_object_mut() {
        obj.remove("in");
        obj.remove("out");
    }
    serde_json::from_value(val).map_err(StoreError::from)
}

fn parse_signal(mut val: serde_json::Value) -> Result<AwarenessSignal, StoreError> {
    if let Some(id_val) = val.get("id")
        && let Some(bare) = extract_record_id(id_val, "awareness_signal")
    {
        val["id"] = serde_json::Value::String(bare);
    }
    serde_json::from_value(val).map_err(StoreError::from)
}

fn extract_record_id(val: &serde_json::Value, table: &str) -> Option<String> {
    let prefix = format!("{table}:");
    match val {
        serde_json::Value::String(s) => Some(s.strip_prefix(&prefix).unwrap_or(s).to_string()),
        serde_json::Value::Object(obj) => {
            let id_field = obj.get("id")?;
            match id_field {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Object(inner) => {
                    inner.get("String")?.as_str().map(str::to_string)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn validate_record_key(id: &str) -> Result<(), StoreError> {
    if !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        Ok(())
    } else {
        Err(StoreError::Other(format!(
            "invalid awareness record id: {id}"
        )))
    }
}

fn default_authority(kind: AwarenessNodeKind) -> AwarenessAuthority {
    match kind {
        AwarenessNodeKind::Question | AwarenessNodeKind::Blocker => AwarenessAuthority::Question,
        AwarenessNodeKind::Hypothesis => AwarenessAuthority::Hypothesis,
        AwarenessNodeKind::Decision | AwarenessNodeKind::Constraint => AwarenessAuthority::Accepted,
        AwarenessNodeKind::Fact => AwarenessAuthority::Inferred,
        AwarenessNodeKind::Objective | AwarenessNodeKind::Risk => AwarenessAuthority::Inferred,
    }
}

fn extend_unique(target: &mut Vec<String>, incoming: &[String]) {
    let mut seen = target.iter().cloned().collect::<BTreeSet<_>>();
    for item in incoming {
        if seen.insert(item.clone()) {
            target.push(item.clone());
        }
    }
}

fn extend_unique_refs(target: &mut Vec<AwarenessRef>, incoming: &[AwarenessRef]) {
    let mut seen = target
        .iter()
        .map(|r| (r.kind.clone(), r.id.clone()))
        .collect::<BTreeSet<_>>();
    for item in incoming {
        if seen.insert((item.kind.clone(), item.id.clone())) {
            target.push(item.clone());
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum RefRole {
    Evidence,
    Signal,
}

fn normalize_sync(mut input: AwarenessSync) -> Result<AwarenessSync, StoreError> {
    let signal_refs = awareness_signal_refs(&input);
    input.refs.evidence_refs = normalize_refs(input.refs.evidence_refs, RefRole::Evidence)?;
    input.refs.signal_refs = normalize_refs(signal_refs, RefRole::Signal)?;
    Ok(input)
}

fn normalize_refs(refs: Vec<AwarenessRef>, role: RefRole) -> Result<Vec<AwarenessRef>, StoreError> {
    let mut unique = BTreeSet::new();
    let mut normalized = Vec::new();
    for item in refs {
        let kind = item.kind.trim().to_ascii_lowercase();
        let id = item.id.trim().to_string();
        if kind.is_empty() || id.is_empty() {
            return Err(StoreError::Other(
                "awareness refs require non-empty kind and id".into(),
            ));
        }
        match role {
            RefRole::Evidence if !is_durable_ref_kind(&kind) => {
                return Err(StoreError::Other(format!(
                    "awareness evidence_ref kind is not durable: {kind}"
                )));
            }
            RefRole::Signal if !is_signal_ref_kind(&kind) => {
                return Err(StoreError::Other(format!(
                    "awareness signal_ref kind is not ephemeral: {kind}"
                )));
            }
            _ => {}
        }
        if unique.insert((kind.clone(), id.clone())) {
            normalized.push(AwarenessRef { kind, id });
        }
    }
    Ok(normalized)
}

fn is_durable_ref_kind(kind: &str) -> bool {
    matches!(
        kind,
        "session_summary"
            | "librarian_node"
            | "source_verification"
            | "accepted_research"
            | "research_note"
            | "plan_item"
            | "decision"
            | "fixed_bug"
            | "lesson"
            | "business_rule"
            | "user_confirmation"
            | "memory"
            | "error_signature"
            | "debug_resolution"
    )
}

fn is_signal_ref_kind(kind: &str) -> bool {
    matches!(
        kind,
        "observation"
            | "checkpoint"
            | "debug_session"
            | "log_row"
            | "browser_event"
            | "device_event"
            | "tool_output"
            | "tool_call"
            | "http_exchange"
            | "js_exception"
            | "console"
            | "network"
            | "event"
    )
}

fn refs_key_string(refs: &[AwarenessRef]) -> String {
    let mut key = refs
        .iter()
        .map(|r| (r.kind.as_str(), r.id.as_str()))
        .collect::<Vec<_>>();
    key.sort();
    serde_json::to_string(&key).unwrap_or_else(|_| "[]".into())
}

fn signal_record_id(project_slug: &str, signal_kind: &str, signal_key: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    project_slug.hash(&mut hasher);
    signal_kind.hash(&mut hasher);
    signal_key.hash(&mut hasher);
    format!("sig_{:016x}", hasher.finish())
}

fn awareness_signal_refs(input: &AwarenessSync) -> Vec<AwarenessRef> {
    let mut refs = input.refs.signal_refs.clone();
    if let Some(debug_session_id) = &input.debug_session_id {
        extend_unique_refs(
            &mut refs,
            &[AwarenessRef {
                kind: "debug_session".into(),
                id: debug_session_id.clone(),
            }],
        );
    }
    if let Some(checkpoint_id) = &input.checkpoint_id {
        extend_unique_refs(
            &mut refs,
            &[AwarenessRef {
                kind: "checkpoint".into(),
                id: checkpoint_id.clone(),
            }],
        );
    }
    refs
}

fn legacy_ref_ids(refs: &[AwarenessRef], kind: &str) -> Vec<String> {
    refs.iter()
        .filter(|r| r.kind == kind)
        .map(|r| r.id.clone())
        .collect()
}

fn legacy_observation_ids(refs: &[AwarenessRef]) -> Vec<u64> {
    refs.iter()
        .filter(|r| r.kind == "observation")
        .filter_map(|r| r.id.parse::<u64>().ok())
        .collect()
}

fn derive_redex(redex: &str) -> AwarenessRedex {
    let state = if redex.contains("!!") {
        Some("conflict".into())
    } else if redex.contains('!') {
        Some("accepted".into())
    } else if redex.contains('?') {
        Some("question".into())
    } else if redex.contains('~') {
        Some("stale".into())
    } else if redex.contains('_') {
        Some("missing".into())
    } else {
        None
    };
    AwarenessRedex {
        state,
        recovery: token_after(redex, "_>"),
        implication: token_after(redex, "=>"),
        temporal: token_after(redex, "@"),
        persistent: redex.contains('$'),
        conflict: redex.contains("!!"),
    }
}

fn token_after(redex: &str, marker: &str) -> Option<String> {
    let (_, rest) = redex.split_once(marker)?;
    let token = rest
        .trim()
        .split(|ch: char| ch.is_whitespace() || ch == ';' || ch == ',')
        .next()
        .unwrap_or_default();
    (!token.is_empty()).then(|| token.to_string())
}

fn extend_unique_u64(target: &mut Vec<u64>, incoming: &[u64]) {
    let mut seen = target.iter().copied().collect::<BTreeSet<_>>();
    for item in incoming {
        if seen.insert(*item) {
            target.push(*item);
        }
    }
}

fn path_depth_within(focus: &str, path: &str, depth: usize) -> bool {
    if path == focus {
        return true;
    }
    let Some(rest) = path.strip_prefix(&format!("{focus}/")) else {
        return false;
    };
    rest.split('/')
        .filter(|segment| !segment.is_empty())
        .count()
        <= depth
}

fn current_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SurrealStore;

    async fn setup() -> (SurrealStore, SurrealAwarenessStore) {
        let store = SurrealStore::memory().await.unwrap();
        let awareness = store.awareness_store();
        awareness.init_schema().await.unwrap();
        (store, awareness)
    }

    fn sync(path: &str, kind: AwarenessNodeKind, summary: &str) -> AwarenessSync {
        AwarenessSync {
            operation: AwarenessOperation::Capture,
            project_slug: "daemon8".into(),
            path: path.into(),
            kind,
            authority: None,
            confidence: Some(0.7),
            summary: Some(summary.into()),
            note: Some("internal note".into()),
            redex: Some("! @. +".into()),
            tags: vec!["domain:awareness".into()],
            debug_session_id: Some("debug-1".into()),
            checkpoint_id: Some("checkpoint-1".into()),
            refs: crate::AwarenessRefs {
                evidence_refs: vec![AwarenessRef {
                    kind: "librarian_node".into(),
                    id: "catalog-1".into(),
                }],
                signal_refs: vec![
                    AwarenessRef {
                        kind: "observation".into(),
                        id: "42".into(),
                    },
                    AwarenessRef {
                        kind: "debug_session".into(),
                        id: "debug-1".into(),
                    },
                    AwarenessRef {
                        kind: "checkpoint".into(),
                        id: "checkpoint-1".into(),
                    },
                ],
            },
            target_node_id: None,
            supersedes: Vec::new(),
            answers: Vec::new(),
            contradicts: Vec::new(),
        }
    }

    #[tokio::test]
    async fn capture_get_round_trip() {
        let (_store, awareness) = setup().await;
        let result = awareness
            .sync_node(sync(
                "debug/frontend/root-cause",
                AwarenessNodeKind::Hypothesis,
                "Hydration is failing before route mount",
            ))
            .await
            .unwrap();
        let id = result.node.unwrap().id.unwrap();
        let fetched = awareness.get_node(&id).await.unwrap().unwrap();
        assert_eq!(fetched.path, "debug/frontend/root-cause");
        assert_eq!(fetched.kind, AwarenessNodeKind::Hypothesis);
        assert_eq!(fetched.state, AwarenessNodeState::Active);
        assert_eq!(fetched.observation_ids, vec![42]);
        assert_eq!(fetched.signal_refs.len(), 3);
        assert_eq!(fetched.evidence_refs.len(), 1);
    }

    #[tokio::test]
    async fn resolve_question_and_retire_hypothesis_leave_manifest_active_set() {
        let (_store, awareness) = setup().await;
        let q = awareness
            .sync_node(sync(
                "debug/frontend/open-question",
                AwarenessNodeKind::Question,
                "Which source owns the route mount?",
            ))
            .await
            .unwrap()
            .node
            .unwrap();
        let h = awareness
            .sync_node(sync(
                "debug/frontend/bad-hypothesis",
                AwarenessNodeKind::Hypothesis,
                "Backend auth caused hydration failure",
            ))
            .await
            .unwrap()
            .node
            .unwrap();

        let mut resolve = sync(
            "debug/frontend/open-question",
            AwarenessNodeKind::Question,
            "Route mount is owned by the frontend shell",
        );
        resolve.operation = AwarenessOperation::Resolve;
        resolve.target_node_id = q.id.clone();
        awareness.sync_node(resolve).await.unwrap();

        let mut retire = sync(
            "debug/frontend/bad-hypothesis",
            AwarenessNodeKind::Hypothesis,
            "Backend auth was not involved",
        );
        retire.operation = AwarenessOperation::Retire;
        retire.target_node_id = h.id.clone();
        awareness.sync_node(retire).await.unwrap();

        let manifest = awareness
            .manifest(&AwarenessFilter {
                project_slug: "daemon8".into(),
                include_inactive: false,
                limit: None,
            })
            .await
            .unwrap();
        assert!(manifest.open_questions.is_empty());
        assert!(manifest.active_hypotheses.is_empty());
    }

    #[tokio::test]
    async fn verify_fact_updates_authority_confidence_and_evidence() {
        let (_store, awareness) = setup().await;
        let fact = awareness
            .sync_node(sync(
                "debug/frontend/fact",
                AwarenessNodeKind::Fact,
                "Frontend route shell mounts first",
            ))
            .await
            .unwrap()
            .node
            .unwrap();
        let mut verify = sync(
            "debug/frontend/fact",
            AwarenessNodeKind::Fact,
            "Frontend route shell mounts first",
        );
        verify.operation = AwarenessOperation::Verify;
        verify.target_node_id = fact.id.clone();
        verify.confidence = Some(0.95);
        verify.refs.evidence_refs = vec![AwarenessRef {
            kind: "session_summary".into(),
            id: "session-summary-1".into(),
        }];
        let verified = awareness.sync_node(verify).await.unwrap().node.unwrap();
        assert_eq!(verified.authority, AwarenessAuthority::Verified);
        assert_eq!(verified.confidence, 0.95);
        assert!(
            verified
                .evidence_refs
                .iter()
                .any(|r| r.kind == "session_summary" && r.id == "session-summary-1")
        );
    }

    #[tokio::test]
    async fn verify_requires_durable_evidence_refs() {
        let (_store, awareness) = setup().await;
        let fact = awareness
            .sync_node(sync(
                "debug/frontend/fact-without-evidence",
                AwarenessNodeKind::Fact,
                "Frontend route shell mounts first",
            ))
            .await
            .unwrap()
            .node
            .unwrap();
        let mut verify = sync(
            "debug/frontend/fact-without-evidence",
            AwarenessNodeKind::Fact,
            "Frontend route shell mounts first",
        );
        verify.operation = AwarenessOperation::Verify;
        verify.target_node_id = fact.id;
        verify.refs.evidence_refs.clear();
        verify.refs.signal_refs = vec![AwarenessRef {
            kind: "observation".into(),
            id: "43".into(),
        }];

        let err = awareness.sync_node(verify).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("verify requires at least one durable evidence_ref")
        );
    }

    #[tokio::test]
    async fn ephemeral_refs_are_rejected_as_durable_evidence() {
        let (_store, awareness) = setup().await;
        let mut input = sync(
            "debug/frontend/bad-evidence-ref",
            AwarenessNodeKind::Fact,
            "Observation rows are not durable evidence",
        );
        input.refs.evidence_refs = vec![AwarenessRef {
            kind: "observation".into(),
            id: "43".into(),
        }];

        let err = awareness.sync_node(input).await.unwrap_err();
        assert!(err.to_string().contains("evidence_ref kind is not durable"));
    }

    #[tokio::test]
    async fn bad_edge_target_does_not_persist_node() {
        let (_store, awareness) = setup().await;
        let mut input = sync(
            "debug/frontend/bad-edge",
            AwarenessNodeKind::Fact,
            "Bad relation target should not write a fact",
        );
        input.answers = vec!["missing-target".into()];

        let err = awareness.sync_node(input).await.unwrap_err();
        assert!(err.to_string().contains("edge target does not exist"));

        let tree = awareness
            .traverse(&AwarenessTraversalFilter {
                project_slug: "daemon8".into(),
                include_inactive: true,
                focus_path: "debug/frontend/bad-edge".into(),
                depth: 0,
                include_notes: true,
                include_evidence: true,
                limit: None,
            })
            .await
            .unwrap();
        assert!(tree.nodes.is_empty());
    }

    #[tokio::test]
    async fn edges_round_trip_through_traversal() {
        let (_store, awareness) = setup().await;
        let q = awareness
            .sync_node(sync(
                "debug/frontend/question",
                AwarenessNodeKind::Question,
                "why",
            ))
            .await
            .unwrap()
            .node
            .unwrap();
        let mut answer = sync("debug/frontend/answer", AwarenessNodeKind::Fact, "because");
        answer.answers = vec![q.id.clone().unwrap()];
        let answer_id = awareness
            .sync_node(answer)
            .await
            .unwrap()
            .node
            .unwrap()
            .id
            .unwrap();

        let tree = awareness
            .traverse(&AwarenessTraversalFilter {
                project_slug: "daemon8".into(),
                focus_path: "debug/frontend".into(),
                depth: 2,
                include_inactive: false,
                include_notes: true,
                include_evidence: true,
                limit: None,
            })
            .await
            .unwrap();
        assert!(
            tree.nodes
                .iter()
                .any(|node| node.id.as_deref() == Some(&answer_id))
        );
        assert!(
            tree.edges
                .iter()
                .any(|edge| edge.kind == AwarenessEdgeKind::Answers)
        );
    }

    #[tokio::test]
    async fn explicit_contradiction_detects_conflict_on_same_path() {
        let (_store, awareness) = setup().await;
        let first = awareness
            .sync_node(sync(
                "debug/frontend/conflict",
                AwarenessNodeKind::Fact,
                "A",
            ))
            .await
            .unwrap()
            .node
            .unwrap();
        let mut contradiction = sync("debug/frontend/conflict", AwarenessNodeKind::Fact, "not A");
        contradiction.contradicts = vec![first.id.unwrap()];
        let result = awareness.sync_node(contradiction).await.unwrap();
        assert!(result.conflict.is_some());
        let node = result.node.unwrap();
        assert_eq!(node.state, AwarenessNodeState::Conflicted);
    }

    #[tokio::test]
    async fn signals_round_trip_and_expire() {
        let (_store, awareness) = setup().await;
        let now = current_ns();
        let input = AwarenessSignalInput {
            project_slug: "daemon8".into(),
            signal_kind: "runtime_warning".into(),
            severity: "warn".into(),
            summary: "test warning".into(),
            signal_refs: vec![
                AwarenessRef {
                    kind: "observation".into(),
                    id: "101".into(),
                },
                AwarenessRef {
                    kind: "observation".into(),
                    id: "101".into(),
                },
            ],
            related_awareness_node_ids: Vec::new(),
            score: 0.8,
            expires_at: now + 60_000_000_000,
        };
        let created = awareness.record_signal(input.clone()).await.unwrap();
        assert!(created.id.is_some());
        let updated = awareness.record_signal(input).await.unwrap();
        assert_eq!(updated.id, created.id);

        let signals = awareness.active_signals("daemon8", 10).await.unwrap();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].signal_kind, "runtime_warning");
        assert_eq!(signals[0].signal_refs.len(), 1);

        let deleted = awareness
            .prune_expired_signals(now + 120_000_000_000)
            .await
            .unwrap();
        assert_eq!(deleted, 1);
        assert!(
            awareness
                .active_signals("daemon8", 10)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn signal_refs_must_be_present_and_ephemeral() {
        let (_store, awareness) = setup().await;
        let now = current_ns();
        let empty = awareness
            .record_signal(AwarenessSignalInput {
                project_slug: "daemon8".into(),
                signal_kind: "runtime_warning".into(),
                severity: "warn".into(),
                summary: "empty signal".into(),
                signal_refs: Vec::new(),
                related_awareness_node_ids: Vec::new(),
                score: 0.8,
                expires_at: now + 60_000_000_000,
            })
            .await
            .unwrap_err();
        assert!(
            empty
                .to_string()
                .contains("requires at least one signal_ref")
        );

        let durable = awareness
            .record_signal(AwarenessSignalInput {
                project_slug: "daemon8".into(),
                signal_kind: "runtime_warning".into(),
                severity: "warn".into(),
                summary: "durable signal".into(),
                signal_refs: vec![AwarenessRef {
                    kind: "session_summary".into(),
                    id: "summary-1".into(),
                }],
                related_awareness_node_ids: Vec::new(),
                score: 0.8,
                expires_at: now + 60_000_000_000,
            })
            .await
            .unwrap_err();
        assert!(
            durable
                .to_string()
                .contains("signal_ref kind is not ephemeral")
        );
    }

    #[tokio::test]
    async fn signal_dedupe_is_keyed_beyond_recent_scan_limits() {
        let (_store, awareness) = setup().await;
        let now = current_ns();
        for idx in 0..60 {
            awareness
                .record_signal(AwarenessSignalInput {
                    project_slug: "daemon8".into(),
                    signal_kind: "runtime_warning".into(),
                    severity: "warn".into(),
                    summary: format!("warning {idx}"),
                    signal_refs: vec![AwarenessRef {
                        kind: "observation".into(),
                        id: idx.to_string(),
                    }],
                    related_awareness_node_ids: Vec::new(),
                    score: 0.5,
                    expires_at: now + 60_000_000_000,
                })
                .await
                .unwrap();
        }

        let updated = awareness
            .record_signal(AwarenessSignalInput {
                project_slug: "daemon8".into(),
                signal_kind: "runtime_warning".into(),
                severity: "error".into(),
                summary: "warning 0 updated".into(),
                signal_refs: vec![AwarenessRef {
                    kind: "observation".into(),
                    id: "0".into(),
                }],
                related_awareness_node_ids: Vec::new(),
                score: 0.9,
                expires_at: now + 60_000_000_000,
            })
            .await
            .unwrap();

        let signals = awareness.active_signals("daemon8", 100).await.unwrap();
        assert_eq!(signals.len(), 60);
        assert_eq!(updated.summary, "warning 0 updated");
    }
}
