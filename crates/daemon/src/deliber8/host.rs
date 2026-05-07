// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! In-process specialist host. Runs one tokio task per `Specialist` agent
//! card inside `daemon8 serve`, sharing the open `SurrealStore` handle so the
//! exclusive SurrealKV lock is honored without a separate process.
//!
//! Lifecycle parity with the existing Chrome bridge:
//! - Each task gets a `CancellationToken` derived from a registry-owned token
//!   (which itself is a child of the serve root cancel).
//! - `stop()` enqueues a graceful Control(stop) envelope, then awaits the
//!   `JoinHandle` with a 5s timeout and aborts on overrun.
//! - On `serve` shutdown, `shutdown_all()` cancels every child token before
//!   the JoinSet sweep, so each loop's `select!` exits cleanly.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use daemon8_api::{SpecialistControlError, SpecialistController};
use daemon8_deliber8_llm::{CallOpts, OpenAiCompatClient, parse_from_card};
use daemon8_store::{AgentCardFilter, CardStore, EnvelopeStore, SurrealStore};
use daemon8_types::{AgentKind, AgentStatus};
use thiserror::Error;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{SpecialistConfig, build_stop_envelope, now_ns, run_specialist};

#[derive(Debug, Error)]
pub enum HostError {
    #[error("agent '{slug}' is not in the card registry")]
    NotFound { slug: String },
    #[error("agent '{slug}' is not a Specialist (kind={kind:?})")]
    NotASpecialist { slug: String, kind: AgentKind },
    #[error("agent '{slug}' has no usable model configuration: {reason}")]
    BadModelConfig { slug: String, reason: String },
    #[error("missing API key for env var {var}")]
    MissingApiKey { var: String },
    #[error("agent '{slug}' is already running")]
    AlreadyRunning { slug: String },
    #[error("agent '{slug}' is not running")]
    NotRunning { slug: String },
    #[error("internal store error: {0}")]
    Store(String),
}

/// Lightweight diagnostics about a running specialist task.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RunningInfo {
    pub slug: String,
    pub started_at_ns: u64,
    pub provider: String,
    pub model: String,
}

#[allow(dead_code)]
struct SpecialistTask {
    handle: JoinHandle<Result<super::SpecialistOutcome>>,
    cancel: CancellationToken,
    started_at_ns: u64,
    provider: String,
    model: String,
}

/// Registry of in-process specialist tasks. Cheap to clone: holds an `Arc`
/// to the inner mutex-protected map.
#[derive(Clone)]
pub struct SpecialistRegistry {
    inner: Arc<RegistryInner>,
}

struct RegistryInner {
    /// Synchronized container for both the running tasks and the set of slugs
    /// that have a `start_specialist` in flight. Held together so `start`
    /// can atomically reserve a slug under a single lock without ever
    /// awaiting while holding it.
    state: Mutex<RegistryState>,
    store: Arc<SurrealStore>,
    root_cancel: CancellationToken,
}

#[derive(Default)]
struct RegistryState {
    tasks: HashMap<String, SpecialistTask>,
    starting: HashSet<String>,
}

impl SpecialistRegistry {
    pub fn new(store: Arc<SurrealStore>, root_cancel: CancellationToken) -> Self {
        Self {
            inner: Arc::new(RegistryInner {
                state: Mutex::new(RegistryState::default()),
                store,
                root_cancel,
            }),
        }
    }

    /// Scan the card registry and spawn a task for every Specialist whose
    /// status is in {Created, Starting, Alive}. Agents that fail to start
    /// (missing API key, bad model config) are transitioned to `Failed` with
    /// a clear reason and skipped — boot does not abort.
    ///
    /// Returns the count of successfully spawned tasks.
    pub async fn boot_from_registry(&self) -> Result<usize> {
        let card_store = self.inner.store.card_store();
        let mut spawned = 0usize;
        let mut skipped_no_config = 0usize;
        let mut skipped_other = 0usize;
        let cards = card_store
            .list_agents(&AgentCardFilter::default())
            .await
            .context("listing agent cards on boot")?;

        for card in cards {
            if card.agent_kind != AgentKind::Specialist {
                continue;
            }
            let bootable = matches!(
                card.status,
                AgentStatus::Created | AgentStatus::Starting | AgentStatus::Alive
            );
            if !bootable {
                continue;
            }
            match self.start_specialist(&card.slug).await {
                Ok(()) => spawned += 1,
                Err(HostError::AlreadyRunning { .. }) => {}
                Err(HostError::BadModelConfig { .. }) => {
                    skipped_no_config += 1;
                    tracing::warn!(slug = %card.slug, "boot skip: no model configured");
                }
                Err(e) => {
                    skipped_other += 1;
                    tracing::warn!(slug = %card.slug, error = %e, "boot skip");
                }
            }
        }
        tracing::info!(spawned, "specialist registry boot complete");
        if skipped_no_config > 0 {
            tracing::info!(
                skipped = skipped_no_config,
                "specialists without a model configuration; patch via PATCH /api/deliber8/roster/{{slug}} then start"
            );
        }
        if skipped_other > 0 {
            tracing::info!(
                skipped = skipped_other,
                "specialists skipped on boot due to other errors; see prior warnings"
            );
        }
        Ok(spawned)
    }

    /// Spawn a task for the named agent. Idempotent: if the agent is already
    /// running OR has a concurrent start in flight, returns `AlreadyRunning`
    /// without disturbing the existing task.
    ///
    /// The slug is reserved in `state.starting` under the lock before any
    /// async work, which closes the TOCTOU between `contains_key` and the
    /// final `insert`. The reservation is released either when the
    /// `SpecialistTask` is inserted (success) or when this function returns
    /// `Err` (cleanup helper below).
    pub async fn start_specialist(&self, slug: &str) -> Result<(), HostError> {
        // Reserve the slug atomically. If anyone else is already running or
        // starting it, bail out with AlreadyRunning before doing any IO.
        {
            let mut state = self.inner.state.lock().unwrap();
            if state.tasks.contains_key(slug) || state.starting.contains(slug) {
                return Err(HostError::AlreadyRunning {
                    slug: slug.to_string(),
                });
            }
            state.starting.insert(slug.to_string());
        }

        // From here on, every error path must release the reservation.
        let result = self.start_specialist_inner(slug).await;
        if result.is_err() {
            self.inner.state.lock().unwrap().starting.remove(slug);
        }
        result
    }

    async fn start_specialist_inner(&self, slug: &str) -> Result<(), HostError> {
        let card_store = self.inner.store.card_store();
        let card = card_store
            .get_agent_by_slug(slug)
            .await
            .map_err(|e| HostError::Store(e.to_string()))?
            .ok_or_else(|| HostError::NotFound {
                slug: slug.to_string(),
            })?;

        if card.agent_kind != AgentKind::Specialist {
            return Err(HostError::NotASpecialist {
                slug: slug.to_string(),
                kind: card.agent_kind,
            });
        }

        let provider_cfg = parse_from_card(&card.model).map_err(|e| HostError::BadModelConfig {
            slug: slug.to_string(),
            reason: e.to_string(),
        })?;

        let llm = match OpenAiCompatClient::from_config(&provider_cfg) {
            Ok(c) => Arc::new(c),
            Err(daemon8_deliber8_llm::LlmError::MissingApiKey { var }) => {
                let reason = format!("missing API key for env var {var}");
                // mark_agent_failed sets status=Failed AND failure_reason in
                // a single SQL roundtrip, so no separate update_agent_status
                // call is needed.
                let _ = card_store
                    .record_agent_failure(&card.id, &reason, now_ns())
                    .await;
                return Err(HostError::MissingApiKey { var });
            }
            Err(e) => {
                return Err(HostError::BadModelConfig {
                    slug: slug.to_string(),
                    reason: e.to_string(),
                });
            }
        };

        let persona_prompt = card
            .persona
            .get("identity_prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let cfg = SpecialistConfig::new(card.slug.clone(), card.address.clone(), llm)
            .heartbeat_interval(
                card.heartbeat_interval_ms
                    .unwrap_or(super::DEFAULT_HEARTBEAT_MS),
            )
            .persona_prompt(persona_prompt)
            .call_opts(CallOpts {
                temperature: provider_cfg.temperature,
                max_tokens: provider_cfg.max_tokens,
            });

        let cancel = self.inner.root_cancel.child_token();
        let store = Arc::clone(&self.inner.store);
        let task_cancel = cancel.clone();
        let slug_owned = card.slug.clone();
        let handle = tokio::spawn(async move {
            let res = run_specialist(store, cfg, task_cancel).await;
            match &res {
                Ok(o) => tracing::info!(
                    slug = %slug_owned,
                    processed = o.processed,
                    responded = o.responded,
                    stopped_by_control = o.stopped_by_control,
                    cancelled = o.cancelled,
                    "specialist task exited"
                ),
                Err(e) => tracing::warn!(slug = %slug_owned, error = %e, "specialist task error"),
            }
            res
        });

        let started_at_ns = now_ns();
        let task = SpecialistTask {
            handle,
            cancel,
            started_at_ns,
            provider: provider_cfg.provider.clone(),
            model: provider_cfg.model.clone(),
        };

        // Atomically swap the in-flight reservation for the real task.
        {
            let mut state = self.inner.state.lock().unwrap();
            state.starting.remove(&card.slug);
            state.tasks.insert(card.slug.clone(), task);
        }

        tracing::info!(
            slug = %card.slug,
            provider = %provider_cfg.provider,
            model = %provider_cfg.model,
            "specialist task started"
        );
        Ok(())
    }

    /// Send a graceful stop envelope, then await the task with a 5s timeout
    /// and abort on overrun. Returns `NotRunning` if no task was registered.
    pub async fn stop_specialist(&self, slug: &str) -> Result<(), HostError> {
        let task = self
            .inner
            .state
            .lock()
            .unwrap()
            .tasks
            .remove(slug)
            .ok_or_else(|| HostError::NotRunning {
                slug: slug.to_string(),
            })?;

        // Resolve the agent's inbox address to enqueue a graceful stop.
        let card_store = self.inner.store.card_store();
        if let Ok(Some(card)) = card_store.get_agent_by_slug(slug).await {
            let env = build_stop_envelope(&card.address, "deliber8.host");
            let envelope_store = self.inner.store.envelope_store();
            if let Err(e) = envelope_store.enqueue_envelope(env).await {
                tracing::warn!(slug, error = %e, "stop envelope enqueue failed; cancelling token directly");
            }
        }

        // Wait up to 5s for the task to drain the stop envelope, otherwise
        // cancel + abort.
        let SpecialistTask { handle, cancel, .. } = task;
        let deadline = tokio::time::timeout(Duration::from_secs(5), async {
            let _ = handle.await;
        });
        match deadline.await {
            Ok(()) => {}
            Err(_) => {
                tracing::warn!(slug, "specialist did not stop within 5s; cancelling");
                cancel.cancel();
            }
        }
        Ok(())
    }

    /// Stop then start. Convenience for picking up persona/model edits.
    pub async fn restart_specialist(&self, slug: &str) -> Result<(), HostError> {
        match self.stop_specialist(slug).await {
            Ok(()) | Err(HostError::NotRunning { .. }) => {}
            Err(e) => return Err(e),
        }
        self.start_specialist(slug).await
    }

    /// Cancel every task immediately (no grace) and clear the registry.
    /// Called from `serve` shutdown after the root `cancel.cancel()`.
    ///
    /// This is *not* redundant with the root token cascade. The serve `cancel`
    /// causes each task's `select!` to exit, but those tasks were spawned via
    /// `tokio::spawn` (not into the serve `JoinSet`), so the JoinSet's 5s
    /// shutdown deadline does not see them. `shutdown_all` is what awaits
    /// their JoinHandles within a bounded budget.
    pub async fn shutdown_all(&self) {
        let drained: Vec<(String, SpecialistTask)> = {
            let mut state = self.inner.state.lock().unwrap();
            state.starting.clear();
            state.tasks.drain().collect()
        };
        for (slug, task) in drained {
            task.cancel.cancel();
            let SpecialistTask { handle, .. } = task;
            let deadline = tokio::time::timeout(Duration::from_secs(5), async {
                let _ = handle.await;
            });
            if deadline.await.is_err() {
                tracing::warn!(slug = %slug, "specialist task did not exit within 5s on shutdown");
            }
        }
    }

    /// Snapshot the running specialists for diagnostics.
    #[allow(dead_code)]
    pub fn list(&self) -> Vec<RunningInfo> {
        self.inner
            .state
            .lock()
            .unwrap()
            .tasks
            .iter()
            .map(|(slug, t)| RunningInfo {
                slug: slug.clone(),
                started_at_ns: t.started_at_ns,
                provider: t.provider.clone(),
                model: t.model.clone(),
            })
            .collect()
    }

    pub fn is_running(&self, slug: &str) -> bool {
        self.inner.state.lock().unwrap().tasks.contains_key(slug)
    }
}

impl From<HostError> for SpecialistControlError {
    fn from(e: HostError) -> Self {
        match e {
            HostError::NotFound { slug } => SpecialistControlError::NotFound { slug },
            HostError::NotASpecialist { slug, .. } => {
                SpecialistControlError::NotASpecialist { slug }
            }
            HostError::BadModelConfig { slug, reason } => {
                SpecialistControlError::BadConfig { slug, reason }
            }
            HostError::MissingApiKey { var } => SpecialistControlError::MissingApiKey { var },
            HostError::AlreadyRunning { slug } => SpecialistControlError::AlreadyRunning { slug },
            HostError::NotRunning { slug } => SpecialistControlError::NotRunning { slug },
            HostError::Store(s) => SpecialistControlError::Internal(s),
        }
    }
}

#[async_trait]
impl SpecialistController for SpecialistRegistry {
    async fn start(&self, slug: &str) -> Result<(), SpecialistControlError> {
        self.start_specialist(slug).await.map_err(Into::into)
    }
    async fn stop(&self, slug: &str) -> Result<(), SpecialistControlError> {
        self.stop_specialist(slug).await.map_err(Into::into)
    }
    async fn restart(&self, slug: &str) -> Result<(), SpecialistControlError> {
        self.restart_specialist(slug).await.map_err(Into::into)
    }
    async fn patch(
        &self,
        slug: &str,
        persona: Option<serde_json::Value>,
        model: Option<serde_json::Value>,
    ) -> Result<bool, SpecialistControlError> {
        let card_store = self.inner.store.card_store();
        let card = card_store
            .get_agent_by_slug(slug)
            .await
            .map_err(|e| SpecialistControlError::Internal(e.to_string()))?
            .ok_or_else(|| SpecialistControlError::NotFound {
                slug: slug.to_string(),
            })?;

        if let Some(p) = persona {
            card_store
                .update_agent_persona(&card.id, p, now_ns())
                .await
                .map_err(|e| SpecialistControlError::Internal(e.to_string()))?;
        }
        if let Some(m) = model {
            card_store
                .update_agent_model(&card.id, m, now_ns())
                .await
                .map_err(|e| SpecialistControlError::Internal(e.to_string()))?;
        }

        if self.is_running(slug) {
            self.restart_specialist(slug)
                .await
                .map_err(SpecialistControlError::from)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    //! Lifecycle tests for the registry. The LLM call path is exercised in
    //! `super::tests` via `build_llm_response_*`; these tests focus on the
    //! state machine: reservation, idempotency, error propagation, and
    //! shutdown ordering.
    //!
    //! Test agents use `provider = "ollama"` so `OpenAiCompatClient::from_config`
    //! succeeds without any env var (Ollama's default api_key_env is `None`).
    //! No Request envelopes are dispatched, so the unreachable
    //! `http://127.0.0.1:11434/v1` is never actually hit.
    use super::*;
    use daemon8_store::CardStore;
    use daemon8_types::{AgentCard, AgentKind, AgentStatus};

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }

    fn ollama_card(slug: &str, status: AgentStatus) -> AgentCard {
        AgentCard {
            id: format!("agent_{slug}"),
            actor_ref: format!("actor:{slug}"),
            address: format!("agent:{slug}"),
            slug: slug.to_string(),
            display_name: None,
            agent_kind: AgentKind::Specialist,
            status,
            persona: serde_json::json!({"identity_prompt": "be terse"}),
            model: serde_json::json!({
                "provider": "ollama",
                "model": "llama3.2",
            }),
            capabilities: vec![],
            subjects_handled: vec![],
            project_refs: vec![],
            team_refs: vec![],
            primary_team_ref: None,
            spawned_by_actor_ref: None,
            spawned_from_cwd: None,
            spawned_from_project_ref: None,
            host_id: None,
            pid: None,
            parent_pid: None,
            process_group_id: None,
            executable_path: None,
            argv_hash: None,
            runtime_kind: Some("daemon8.deliber8".into()),
            runtime_version: None,
            launch_nonce: None,
            started_at: None,
            last_seen_at: None,
            // Long heartbeat so the test's loop doesn't churn.
            heartbeat_interval_ms: Some(60_000),
            stop_state: serde_json::json!({}),
            last_stop_request_at: None,
            last_exit_code: None,
            last_signal: None,
            cost_window_usd: 0.0,
            cost_total_usd: 0.0,
            budget_daily_usd: None,
            failure_reason: None,
            created_at: now(),
            updated_at: now(),
        }
    }

    async fn fresh_registry() -> (SpecialistRegistry, Arc<SurrealStore>) {
        let store = Arc::new(SurrealStore::memory().await.unwrap());
        store.card_store().init_schema().await.unwrap();
        let cancel = CancellationToken::new();
        let registry = SpecialistRegistry::new(Arc::clone(&store), cancel);
        (registry, store)
    }

    #[tokio::test]
    async fn start_then_double_start_returns_already_running() {
        let (registry, store) = fresh_registry().await;
        let cards = store.card_store();
        cards
            .upsert_agent(ollama_card("alpha", AgentStatus::Created))
            .await
            .unwrap();

        registry.start_specialist("alpha").await.unwrap();
        let again = registry.start_specialist("alpha").await;
        assert!(matches!(again, Err(HostError::AlreadyRunning { .. })));
        assert!(registry.is_running("alpha"));

        registry.shutdown_all().await;
        assert!(!registry.is_running("alpha"));
    }

    #[tokio::test]
    async fn stop_on_unknown_returns_not_running() {
        let (registry, _store) = fresh_registry().await;
        let err = registry.stop_specialist("ghost").await.unwrap_err();
        assert!(matches!(err, HostError::NotRunning { .. }));
    }

    #[tokio::test]
    async fn start_missing_api_key_records_failure() {
        let (registry, store) = fresh_registry().await;
        let cards = store.card_store();
        let mut card = ollama_card("needs-key", AgentStatus::Created);
        // Switch to openrouter pointing at a never-set env var.
        card.model = serde_json::json!({
            "provider": "openrouter",
            "model": "openai/gpt-4o-mini",
            "api_key_env": format!(
                "DAEMON8_LLM_NEVER_SET_{}_{}",
                std::process::id(),
                now()
            ),
        });
        cards.upsert_agent(card.clone()).await.unwrap();

        let err = registry.start_specialist("needs-key").await.unwrap_err();
        assert!(matches!(err, HostError::MissingApiKey { .. }));

        // The failure must be recorded so the admin UI can surface it.
        let after = cards.get_agent_by_slug("needs-key").await.unwrap().unwrap();
        assert_eq!(after.status, AgentStatus::Failed);
        assert!(
            after
                .failure_reason
                .as_deref()
                .map(|s| s.contains("missing API key"))
                .unwrap_or(false),
            "expected failure_reason to mention missing API key, got {:?}",
            after.failure_reason
        );
        assert!(!registry.is_running("needs-key"));
    }

    #[tokio::test]
    async fn start_with_no_model_returns_bad_config() {
        let (registry, store) = fresh_registry().await;
        let cards = store.card_store();
        let mut card = ollama_card("blank", AgentStatus::Created);
        card.model = serde_json::json!({});
        cards.upsert_agent(card).await.unwrap();

        let err = registry.start_specialist("blank").await.unwrap_err();
        assert!(matches!(err, HostError::BadModelConfig { .. }));
        assert!(!registry.is_running("blank"));
    }

    #[tokio::test]
    async fn start_unknown_slug_returns_not_found() {
        let (registry, _store) = fresh_registry().await;
        let err = registry.start_specialist("ghost").await.unwrap_err();
        assert!(matches!(err, HostError::NotFound { .. }));
    }

    #[tokio::test]
    async fn boot_from_registry_skips_non_specialists_and_retired() {
        let (registry, store) = fresh_registry().await;
        let cards = store.card_store();

        cards
            .upsert_agent(ollama_card("alive-spec", AgentStatus::Alive))
            .await
            .unwrap();
        cards
            .upsert_agent(ollama_card("retired-spec", AgentStatus::Retired))
            .await
            .unwrap();
        let mut steward = ollama_card("steward", AgentStatus::Alive);
        steward.id = "agent_steward".into();
        steward.agent_kind = AgentKind::Steward;
        cards.upsert_agent(steward).await.unwrap();

        let n = registry.boot_from_registry().await.unwrap();
        assert_eq!(n, 1, "only the Alive Specialist should boot");
        assert!(registry.is_running("alive-spec"));
        assert!(!registry.is_running("retired-spec"));
        assert!(!registry.is_running("steward"));

        registry.shutdown_all().await;
    }

    #[tokio::test]
    async fn patch_returns_false_when_not_running() {
        let (registry, store) = fresh_registry().await;
        let cards = store.card_store();
        cards
            .upsert_agent(ollama_card("idle", AgentStatus::Created))
            .await
            .unwrap();

        let restarted = registry
            .patch(
                "idle",
                Some(serde_json::json!({"identity_prompt": "new prompt"})),
                None,
            )
            .await
            .unwrap();
        assert!(!restarted, "agent is not running, should not restart");

        let after = cards.get_agent_by_slug("idle").await.unwrap().unwrap();
        assert_eq!(
            after.persona,
            serde_json::json!({"identity_prompt": "new prompt"})
        );
    }

    #[tokio::test]
    async fn patch_with_empty_body_writes_nothing() {
        let (registry, store) = fresh_registry().await;
        let cards = store.card_store();
        cards
            .upsert_agent(ollama_card("frozen", AgentStatus::Created))
            .await
            .unwrap();

        // Patch with both fields None — handler-level guard rejects empty
        // body, but the controller method itself simply no-ops the writes.
        let restarted = registry.patch("frozen", None, None).await.unwrap();
        assert!(!restarted);
        let after = cards.get_agent_by_slug("frozen").await.unwrap().unwrap();
        assert_eq!(
            after.persona,
            serde_json::json!({"identity_prompt": "be terse"})
        );
    }

    #[tokio::test]
    async fn shutdown_all_clears_registry() {
        let (registry, store) = fresh_registry().await;
        let cards = store.card_store();
        for slug in ["a1", "a2", "a3"] {
            cards
                .upsert_agent(ollama_card(slug, AgentStatus::Created))
                .await
                .unwrap();
            registry.start_specialist(slug).await.unwrap();
        }
        assert_eq!(registry.list().len(), 3);

        registry.shutdown_all().await;
        assert!(registry.list().is_empty());
        assert!(!registry.is_running("a1"));
    }
}
