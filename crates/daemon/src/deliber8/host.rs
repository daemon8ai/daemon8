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

use std::collections::HashMap;
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
    tasks: Mutex<HashMap<String, SpecialistTask>>,
    store: Arc<SurrealStore>,
    root_cancel: CancellationToken,
}

impl SpecialistRegistry {
    pub fn new(store: Arc<SurrealStore>, root_cancel: CancellationToken) -> Self {
        Self {
            inner: Arc::new(RegistryInner {
                tasks: Mutex::new(HashMap::new()),
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
                Err(e) => {
                    tracing::warn!(slug = %card.slug, error = %e, "boot skip");
                }
            }
        }
        tracing::info!(spawned, "specialist registry boot complete");
        Ok(spawned)
    }

    /// Spawn a task for the named agent. Idempotent: if the agent is already
    /// running, returns `AlreadyRunning` without disturbing the existing task.
    pub async fn start_specialist(&self, slug: &str) -> Result<(), HostError> {
        {
            let map = self.inner.tasks.lock().unwrap();
            if map.contains_key(slug) {
                return Err(HostError::AlreadyRunning {
                    slug: slug.to_string(),
                });
            }
        }

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
                let _ = card_store
                    .update_agent_status(&card.id, AgentStatus::Failed, now_ns())
                    .await;
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

        let provider_label = provider_cfg.provider.clone();
        let model_label = provider_cfg.model.clone();
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
        self.inner.tasks.lock().unwrap().insert(
            card.slug.clone(),
            SpecialistTask {
                handle,
                cancel,
                started_at_ns,
                provider: provider_label.clone(),
                model: model_label.clone(),
            },
        );

        tracing::info!(
            slug = %card.slug,
            provider = %provider_label,
            model = %model_label,
            "specialist task started"
        );
        Ok(())
    }

    /// Send a graceful stop envelope, then await the task with a 5s timeout
    /// and abort on overrun. Returns `NotRunning` if no task was registered.
    pub async fn stop_specialist(&self, slug: &str) -> Result<(), HostError> {
        let task = self
            .inner
            .tasks
            .lock()
            .unwrap()
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
    pub async fn shutdown_all(&self) {
        let drained: Vec<(String, SpecialistTask)> = {
            let mut map = self.inner.tasks.lock().unwrap();
            map.drain().collect()
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
            .tasks
            .lock()
            .unwrap()
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
        self.inner.tasks.lock().unwrap().contains_key(slug)
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
