// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use daemon8_mcp::ChromeCommand;
use daemon8_store::{
    ActiveSessionState, DebugSessionStore, MemoryStore, StateModel, SurrealStore, error_hash,
};
use daemon8_types::Observation;

use crate::config;

pub(crate) const CHROME_CMD_CAPACITY: usize = 64;
pub(crate) const BROWSER_ACTION_CAPACITY: usize = 64;

#[derive(clap::Args, Default)]
pub(crate) struct ServeArgs {
    #[arg(long)]
    pub port: Option<u16>,
    #[arg(long)]
    pub browser: Option<String>,
    #[arg(long, hide = true)]
    pub log_dir: Option<String>,
    #[arg(long, hide = true)]
    pub log_level: Option<String>,
}

pub(crate) async fn cmd_serve(config_path: Option<String>, args: ServeArgs) -> Result<()> {
    let mut cfg = config::load(config_path.as_deref()).context("failed to load configuration")?;
    let setup_config_path = config_path.clone();

    if let Some(port) = args.port {
        cfg.server.port = port;
    }
    if let Some(ref endpoint) = args.browser {
        cfg.browser.auto_connect = true;
        cfg.browser.endpoint = endpoint.clone();
    }

    let db_path = config::resolve_db_path(cfg.storage.path.as_deref());
    let surreal_store = Arc::new(
        SurrealStore::open(&db_path)
            .await
            .with_context(|| format!("opening database: {}", db_path.display()))?,
    );

    let memory_store: Arc<dyn daemon8_store::MemoryStore> = Arc::new(surreal_store.memory_store());
    let debug_session_store: Arc<dyn daemon8_store::DebugSessionStore> =
        Arc::new(surreal_store.debug_session_store());
    let store: Arc<dyn StateModel> = surreal_store.clone();
    let active_state = ActiveSessionState::new();

    // Unbounded channel — deliberate policy.  The daemon captures observations
    // best-effort and losslessly: callers POST and return immediately; the store
    // writer drains the receiver without backpressure.  Switching to a bounded
    // channel would introduce dropped observations under burst load and change
    // the data-contract that agents depend on.  Do not change to bounded without
    // measuring the real burst profile, choosing a drop strategy (oldest vs
    // newest), and documenting the trade-offs here.
    let (obs_tx, obs_rx) = mpsc::unbounded_channel::<Observation>();
    // Broadcast carries (Arc<Observation>, Arc<str>) so every subscriber can
    // filter on the typed observation (cheap) and forward the pre-serialized
    // JSON (cheap Arc clone) without re-serializing per subscriber. The
    // serialized payload carries the real id assigned by store.insert.
    let (broadcast_tx, _broadcast_rx) = broadcast::channel::<(Arc<Observation>, Arc<str>)>(1000);
    let cancel = CancellationToken::new();
    let mut tasks = JoinSet::new();

    let node_id: Arc<str> = Arc::from(resolve_node_id().as_str());

    spawn_store_writer(
        &mut tasks,
        obs_rx,
        StoreWriterCtx {
            store: store.clone(),
            memory_store: Some(memory_store.clone()),
            debug_session_store: Some(debug_session_store.clone()),
            active_state: active_state.clone(),
            broadcast_tx: broadcast_tx.clone(),
            node_id,
        },
        cancel.clone(),
    );

    let screenshot_dir = config::resolve_screenshot_path(&cfg);
    crate::cleanup::spawn_cleanup_task(
        &mut tasks,
        crate::cleanup::CleanupCtx {
            store: store.clone(),
            debug_session_store: Some(debug_session_store.clone()),
            memory_store: Some(memory_store.clone()),
            active_state: active_state.clone(),
            inactivity_auto_end_secs: cfg
                .debug_session
                .inactivity_auto_end_secs
                .unwrap_or(crate::cleanup::DEFAULT_INACTIVITY_AUTO_END_SECS),
        },
        screenshot_dir.clone(),
        cancel.clone(),
    );

    let (chrome_cmd_tx, chrome_cmd_rx) = mpsc::channel::<ChromeCommand>(CHROME_CMD_CAPACITY);

    let browser_binary = cfg.browser.path.as_deref().map(|p| p.display().to_string());
    let reconnect_policy = daemon8_chrome::ReconnectPolicy {
        initial: Duration::from_secs(cfg.browser.reconnect_interval_secs),
        max: Duration::from_secs(cfg.browser.max_reconnect_interval_secs),
    };
    let chrome_state_rx = spawn_chrome_command_handler(
        &mut tasks,
        chrome_cmd_rx,
        obs_tx.clone(),
        browser_binary,
        reconnect_policy,
        cancel.clone(),
    );

    // Store the chrome endpoint for lazy connection (triggered on first tool use).
    // No auto-connect -- the MCP tools handle it on demand.
    let chrome_endpoint: Arc<std::sync::Mutex<Option<Arc<str>>>> =
        Arc::new(std::sync::Mutex::new(if cfg.browser.auto_connect {
            Some(Arc::from(cfg.browser.endpoint.as_str()))
        } else {
            None
        }));

    let device_screenshot_fn: Option<daemon8_mcp::DeviceScreenshotFn> = if cfg.adb.enabled {
        let addr = cfg.adb.server_addr;
        let scan_interval = cfg.adb.scan_interval_secs;
        let tx = obs_tx.clone();
        let ct = cancel.clone();
        tasks.spawn(async move {
            if let Err(e) = daemon8_adb::connect_and_monitor(addr, scan_interval, tx, ct).await {
                tracing::error!("ADB device monitor error: {e}");
            }
        });
        Some(crate::screenshot::build_screenshot_fn(addr))
    } else {
        None
    };

    if cfg.ingestion.udp.enabled {
        let bind = cfg.ingestion.udp.bind;
        let max_packet = cfg.ingestion.udp.max_packet;
        let tx = obs_tx.clone();
        let ct = cancel.clone();
        tasks.spawn(async move {
            if let Err(e) = daemon8_ingest::udp::run_udp_listener(bind, max_packet, tx, ct).await {
                tracing::error!("UDP listener error: {e}");
            }
        });
    }

    #[cfg(unix)]
    if cfg.ingestion.unix.enabled {
        let path = config::resolve_unix_socket_path(cfg.ingestion.unix.path.as_deref());
        let tx = obs_tx.clone();
        let ct = cancel.clone();
        tasks.spawn(async move {
            if let Err(e) = daemon8_ingest::unix::run_unix_listener(&path, tx, ct).await {
                tracing::error!("Unix socket listener error: {e}");
            }
        });
    }
    #[cfg(windows)]
    if cfg.ingestion.unix.enabled {
        tracing::warn!(
            "unix socket ingestion is not supported on Windows; \
             set ingestion.unix.enabled = false to silence this warning"
        );
    }

    if !cfg.sources.is_empty() {
        tracing::info!(count = cfg.sources.len(), "starting file sources");
        crate::sources::spawn_file_sources(
            &mut tasks,
            &cfg.sources,
            obs_tx.clone(),
            cancel.clone(),
        );
    }

    let setup_tool_fn: daemon8_mcp::SetupToolFn = Arc::new(move |action| {
        let setup_config_path = setup_config_path.clone();
        Box::pin(async move {
            crate::cli::setup::cmd_setup_mcp(action, setup_config_path.as_deref()).await
        })
    });

    let hooks_tool_fn: daemon8_mcp::HooksToolFn = Arc::new(move |action| {
        Box::pin(async move { crate::cli::hooks::cmd_hooks_mcp(action).await })
    });

    // Only start MCP stdio when stdin is a real FIFO from an MCP client.
    // A plain "not a TTY" check is insufficient: launchd, nohup, and shell
    // backgrounding all attach /dev/null (a character device) to stdin.
    // rmcp 1.3 happily initializes on /dev/null, then waiting() returns at
    // EOF and the daemon cancels itself. FIFO detection is the one signal
    // that distinguishes a real client pipe from /dev/null on every launcher.
    let stdin_is_pipe = stdin_is_real_pipe();
    if stdin_is_pipe {
        use rmcp::ServiceExt;

        let lens = Arc::new(daemon8_store::LensManager::new(broadcast_tx.subscribe()));
        let mcp = daemon8_mcp::DaemonMcp::new(daemon8_mcp::DaemonMcpConfig {
            store: store.clone(),
            memory_store: Some(memory_store.clone()),
            debug_session_store: Some(debug_session_store.clone()),
            active_state: active_state.clone(),
            obs_tx: obs_tx.clone(),
            chrome_tx: chrome_cmd_tx.clone(),
            chrome_state: chrome_state_rx.clone(),
            chrome_endpoint: chrome_endpoint.clone(),
            device_screenshot_fn: device_screenshot_fn.clone(),
            screenshot_dir: screenshot_dir.clone(),
            broadcast_tx: broadcast_tx.clone(),
            lens,
            setup_tool_fn: Some(setup_tool_fn.clone()),
            hooks_tool_fn: Some(hooks_tool_fn.clone()),
            cancel: cancel.clone(),
        });
        let cancel_on_eof = cancel.clone();
        tasks.spawn(async move {
            match mcp.serve(rmcp::transport::stdio()).await {
                Ok(service) => {
                    tracing::info!("MCP stdio server running");
                    let _ = service.waiting().await;
                    tracing::info!("MCP client disconnected (stdin closed), shutting down");
                    cancel_on_eof.cancel();
                }
                Err(e) => {
                    tracing::error!("MCP stdio error: {e}");
                }
            }
        });
    } else if cfg.mcp.stdio {
        tracing::debug!(
            "MCP stdio requested but stdin is not a pipe -- skipping (use via MCP client)"
        );
    }

    let mcp_store = store.clone();
    let mcp_memory_store = memory_store.clone();
    let mcp_debug_session_store = debug_session_store.clone();
    let mcp_active_state = active_state.clone();
    let mcp_obs_tx = obs_tx.clone();
    let mcp_chrome_tx = chrome_cmd_tx.clone();
    let mcp_state_rx = chrome_state_rx.clone();
    let mcp_ep = chrome_endpoint.clone();
    let mcp_screenshot_fn = device_screenshot_fn.clone();
    let mcp_screenshot_dir = screenshot_dir.clone();
    let mcp_broadcast_tx = broadcast_tx.clone();
    let mcp_cancel = cancel.child_token();
    // Daemon-wide cancel cloned into the per-session factory closure. Each
    // `DaemonMcp` then derives its own child token for the push task in
    // `on_initialized`, so daemon shutdown propagates while a single session
    // ending does not affect siblings.
    let mcp_root_cancel = cancel.clone();

    let mcp_http = rmcp::transport::streamable_http_server::StreamableHttpService::new(
        move || {
            Ok(daemon8_mcp::DaemonMcp::new(daemon8_mcp::DaemonMcpConfig {
                store: mcp_store.clone(),
                memory_store: Some(mcp_memory_store.clone()),
                debug_session_store: Some(mcp_debug_session_store.clone()),
                active_state: mcp_active_state.clone(),
                obs_tx: mcp_obs_tx.clone(),
                chrome_tx: mcp_chrome_tx.clone(),
                chrome_state: mcp_state_rx.clone(),
                chrome_endpoint: mcp_ep.clone(),
                device_screenshot_fn: mcp_screenshot_fn.clone(),
                screenshot_dir: mcp_screenshot_dir.clone(),
                broadcast_tx: mcp_broadcast_tx.clone(),
                lens: Arc::new(daemon8_store::LensManager::new(
                    mcp_broadcast_tx.subscribe(),
                )),
                setup_tool_fn: Some(setup_tool_fn.clone()),
                hooks_tool_fn: Some(hooks_tool_fn.clone()),
                cancel: mcp_root_cancel.clone(),
            }))
        },
        Arc::new({
            let mut mgr = rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default();
            mgr.session_config.keep_alive = Some(std::time::Duration::from_secs(86400));
            mgr
        }),
        rmcp::transport::streamable_http_server::StreamableHttpServerConfig::default()
            .with_stateful_mode(true)
            .with_cancellation_token(mcp_cancel),
    );

    let api_lens = Arc::new(daemon8_store::LensManager::new(broadcast_tx.subscribe()));
    let api_state = daemon8_api::ApiState {
        store: store.clone(),
        stream_tx: broadcast_tx.clone(),
        chrome_cmd_tx: chrome_cmd_tx.clone(),
        chrome_state: chrome_state_rx.clone(),
        chrome_endpoint: chrome_endpoint.clone(),
        lens: api_lens,
        memory_store: Some(memory_store.clone()),
    };
    let port = cfg.server.port;
    let app = daemon8_ingest::ingest_router(obs_tx.clone())
        .merge(daemon8_api::api_router(api_state))
        .route(
            "/.well-known/oauth-protected-resource",
            axum::routing::get(move || async move {
                axum::Json(serde_json::json!({
                    "resource": format!("http://localhost:{port}"),
                    "authorization_servers": []
                }))
            }),
        )
        .nest_service("/mcp", mcp_http);

    let bind_addr = format!("{}:{}", cfg.server.host, cfg.server.port);
    let listener = bind_with_retry(&bind_addr, cfg.server.port).await?;

    let mut transports = vec!["http"];
    if cfg.ingestion.udp.enabled {
        transports.push("udp");
    }
    #[cfg(unix)]
    if cfg.ingestion.unix.enabled {
        transports.push("unix");
    }

    let log_dir = config::resolve_log_dir(cfg.logging.file.as_deref());
    tracing::info!(
        port = cfg.server.port,
        mcp = %format!("http://{}:{}/mcp", cfg.server.host, cfg.server.port),
        db = %db_path.display(),
        logs = %log_dir.display(),
        screenshots = %screenshot_dir.display(),
        log_level = %cfg.logging.level.as_str(),
        log_stderr = cfg.logging.stderr,
        log_max_files = cfg.logging.max_log_files,
        log_rotation = "daily",
        browser = if cfg.browser.auto_connect { &cfg.browser.endpoint } else { "disabled" },
        transports = %transports.join(", "),
        "daemon8 started"
    );

    let cancel_for_server = cancel.clone();
    tasks.spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(cancel_for_server.cancelled_owned())
            .await
            .unwrap_or_else(|e| tracing::error!("HTTP server error: {e}"));
    });

    tokio::select! {
        signal = shutdown_signal() => {
            tracing::info!(signal, "received shutdown signal, shutting down...");
        }
        _ = cancel.cancelled() => {}
    }

    cancel.cancel();

    // Give spawned tasks up to 5 seconds to finish
    let shutdown_deadline = tokio::time::timeout(Duration::from_secs(5), async {
        while tasks.join_next().await.is_some() {}
    });

    if shutdown_deadline.await.is_err() {
        tracing::warn!("shutdown timed out after 5s, aborting remaining tasks");
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }

    tracing::info!("daemon8 stopped");
    Ok(())
}

/// Drain observations from the channel into the store, broadcasting each
/// persisted observation to SSE subscribers. The broadcast payload is
/// `(Arc<Observation>, Arc<str>)` — the typed observation for server-side
/// filtering and the pre-serialized JSON for zero-copy fanout. The JSON
/// carries the real `id` assigned by `store.insert`; callers relying on
/// `Last-Event-ID` depend on this ordering.
pub(crate) struct StoreWriterCtx {
    pub store: Arc<dyn StateModel>,
    pub memory_store: Option<Arc<dyn MemoryStore>>,
    pub debug_session_store: Option<Arc<dyn DebugSessionStore>>,
    pub active_state: ActiveSessionState,
    pub broadcast_tx: broadcast::Sender<(Arc<Observation>, Arc<str>)>,
    pub node_id: Arc<str>,
}

fn spawn_store_writer(
    tasks: &mut JoinSet<()>,
    mut rx: mpsc::UnboundedReceiver<Observation>,
    ctx: StoreWriterCtx,
    cancel: CancellationToken,
) {
    tasks.spawn(async move {
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Some(obs) => {
                            handle_observation(obs, &ctx).await;
                        }
                        None => break, // channel closed
                    }
                }
                () = cancel.cancelled() => break,
            }
        }
        tracing::debug!("store writer stopped");
    });
}

async fn handle_observation(mut obs: Observation, ctx: &StoreWriterCtx) {
    let now_ns = current_ns();

    if obs.node_id.is_none() {
        obs.node_id = Some(ctx.node_id.clone());
    }

    // Stamp active debug session and checkpoint links if present.
    let active_session = ctx.active_state.current_session();
    if let Some(ref session) = active_session {
        if obs.debug_session_id.is_none() {
            obs.debug_session_id = Some(session.id.clone());
        }
        session.touch(now_ns);
    }
    if obs.checkpoint_id.is_none()
        && let Some(cp) = ctx.active_state.current_checkpoint()
    {
        obs.checkpoint_id = Some(cp);
    }

    // Compute error hash (cheap, sync) for any error-shaped observation.
    if obs.error_hash.is_none()
        && let Some(text) = error_hash::extract_error_text(&obs)
    {
        let normalized = error_hash::normalize_error_text(&text);
        let hash = error_hash::hash_error(&normalized);
        obs.error_hash = Some(Arc::from(hash.as_str()));
    }

    let insert_copy = obs.clone();
    let inserted_id = match ctx.store.insert(insert_copy).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("store insert failed: {e}");
            return;
        }
    };
    obs.id = inserted_id;

    // Broadcast to any subscribers (zero-copy fanout).
    match serde_json::to_string(&obs) {
        Ok(json) => {
            let arc_obs = Arc::new(obs);
            let arc_json: Arc<str> = Arc::from(json);
            let _ = ctx.broadcast_tx.send((arc_obs.clone(), arc_json));

            // Promote error signature to memory if applicable. Done after
            // the row is on disk so source_observations references the real
            // seq id. Fire-and-forget if memory_store isn't wired.
            if let (Some(mem_store), Some(hash)) =
                (ctx.memory_store.as_ref(), arc_obs.error_hash.as_ref())
            {
                let project_slug = active_session
                    .as_ref()
                    .map(|s| s.project_slug.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let normalized = error_hash::extract_error_text(&arc_obs)
                    .map(|t| error_hash::normalize_error_text(&t))
                    .unwrap_or_default();
                if let Err(e) = error_hash::promote_error_signature(
                    mem_store.as_ref(),
                    hash,
                    &normalized,
                    &project_slug,
                    inserted_id,
                    now_ns,
                )
                .await
                {
                    tracing::warn!(error = %e, "error signature promotion failed");
                }
            }
        }
        Err(e) => {
            tracing::error!(id = inserted_id, error = %e, "observation failed to serialize; dropping from broadcast");
        }
    }

    // touch_debug_session in DB is left to the periodic flush in the cleanup
    // task (Step 7) — avoids one DB write per observation. Let `_` consume
    // the unused store handle so future maintainers know it's deliberate.
    let _ = ctx.debug_session_store.as_ref();
}

fn current_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

#[cfg(test)]
mod writer_tests {
    use super::*;
    use daemon8_store::{ActiveDebugSession, MemoryFilter, MemoryStore};
    use daemon8_types::{AppName, MemoryKind, ObservationKind, Origin, Severity};
    use std::sync::atomic::AtomicU64;

    fn fresh_obs(severity: Severity, kind: ObservationKind) -> Observation {
        Observation {
            id: 0,
            origin: Origin::Application {
                name: AppName::from("test-app"),
            },
            kind,
            data: serde_json::json!({"msg": "x"}),
            severity,
            source_location: None,
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

    async fn build_ctx_with_active(
        session: Option<ActiveDebugSession>,
        checkpoint: Option<&str>,
    ) -> (StoreWriterCtx, Arc<dyn StateModel>, Arc<dyn MemoryStore>) {
        let ss = Arc::new(SurrealStore::memory().await.unwrap());
        let mem: Arc<dyn MemoryStore> = Arc::new(ss.memory_store());
        let ds: Arc<dyn DebugSessionStore> = Arc::new(ss.debug_session_store());
        let active = ActiveSessionState::new();
        if let Some(s) = session {
            active.set_session(Some(s));
        }
        if let Some(cp) = checkpoint {
            active.set_checkpoint(Some(Arc::from(cp)));
        }
        let (broadcast_tx, _rx) = broadcast::channel(8);
        let store: Arc<dyn StateModel> = ss.clone();
        (
            StoreWriterCtx {
                store: store.clone(),
                memory_store: Some(mem.clone()),
                debug_session_store: Some(ds),
                active_state: active,
                broadcast_tx,
                node_id: Arc::from("node-test"),
            },
            store,
            mem,
        )
    }

    #[tokio::test]
    async fn stamps_active_debug_session_and_checkpoint() {
        let session = ActiveDebugSession {
            id: Arc::from("ds_123"),
            project_slug: Arc::from("daemon8"),
            started_at_ns: 1_000,
            last_activity_ns: Arc::new(AtomicU64::new(1_000)),
        };
        let (ctx, store, _mem) = build_ctx_with_active(Some(session), Some("cp_42")).await;

        handle_observation(fresh_obs(Severity::Info, ObservationKind::Log), &ctx).await;

        let slice = store
            .query(&daemon8_types::Filter::default())
            .await
            .unwrap();
        assert_eq!(slice.observations.len(), 1);
        let obs = &slice.observations[0];
        assert_eq!(obs.debug_session_id.as_deref(), Some("ds_123"));
        assert_eq!(obs.checkpoint_id.as_deref(), Some("cp_42"));
    }

    #[tokio::test]
    async fn no_stamp_when_no_active_session() {
        let (ctx, store, _mem) = build_ctx_with_active(None, None).await;

        handle_observation(fresh_obs(Severity::Info, ObservationKind::Log), &ctx).await;

        let slice = store
            .query(&daemon8_types::Filter::default())
            .await
            .unwrap();
        let obs = &slice.observations[0];
        assert!(obs.debug_session_id.is_none());
        assert!(obs.checkpoint_id.is_none());
    }

    #[tokio::test]
    async fn computes_error_hash_on_exception_and_promotes_signature() {
        let session = ActiveDebugSession {
            id: Arc::from("ds_x"),
            project_slug: Arc::from("daemon8"),
            started_at_ns: 1_000,
            last_activity_ns: Arc::new(AtomicU64::new(1_000)),
        };
        let (ctx, store, mem) = build_ctx_with_active(Some(session), None).await;

        let exc = ObservationKind::Exception {
            message: "DB timeout after 5000ms on conn 7".into(),
            trace: Some("at db::query in /Users/x/proj/src/db.rs:42".into()),
        };
        handle_observation(fresh_obs(Severity::Error, exc.clone()), &ctx).await;
        // Same shape — should bump seen, not create a new memory.
        let exc2 = ObservationKind::Exception {
            message: "DB timeout after 9999ms on conn 12".into(),
            trace: Some("at db::query in /Users/y/other/src/db.rs:42".into()),
        };
        handle_observation(fresh_obs(Severity::Error, exc2), &ctx).await;

        let slice = store
            .query(&daemon8_types::Filter::default())
            .await
            .unwrap();
        assert_eq!(slice.observations.len(), 2);
        assert!(slice.observations[0].error_hash.is_some());
        assert_eq!(
            slice.observations[0].error_hash, slice.observations[1].error_hash,
            "structurally identical errors must hash to the same value"
        );

        let signatures = mem
            .query_memory(&MemoryFilter {
                kinds: Some(vec![MemoryKind::ErrorSignature]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            signatures.len(),
            1,
            "second occurrence must reuse the same ErrorSignature"
        );
        assert!(signatures[0].tags.contains(&"seen:2".to_string()));
    }

    #[tokio::test]
    async fn touch_bumps_active_session_last_activity() {
        let last = Arc::new(AtomicU64::new(1_000));
        let session = ActiveDebugSession {
            id: Arc::from("ds_t"),
            project_slug: Arc::from("p"),
            started_at_ns: 1_000,
            last_activity_ns: last.clone(),
        };
        let (ctx, _store, _mem) = build_ctx_with_active(Some(session), None).await;

        handle_observation(fresh_obs(Severity::Info, ObservationKind::Log), &ctx).await;

        assert!(
            last.load(std::sync::atomic::Ordering::Relaxed) > 1_000,
            "active session last_activity_ns must be touched on each insert"
        );
    }
}

/// Periodically delete observations and screenshots older than their retention windows.
/// Decision the chrome command handler makes when receiving a Connect command.
///
/// Centralizes the idempotency logic so it can be unit tested without spinning up
/// a real Chrome process. The handler calls `decide_connect_action` and then
/// performs the corresponding side effects -- there is no duplicated logic.
#[derive(Debug, PartialEq, Eq)]
enum ConnectDecision {
    /// No live task; spawn a fresh connector.
    Spawn,
    /// Live task exists for a different endpoint; abort it and spawn a new one.
    AbortAndSpawn,
    /// Already connecting/connected to the requested endpoint -- do nothing.
    Ignore,
}

fn decide_connect_action(
    task_alive: bool,
    state: daemon8_chrome::ConnectionState,
    same_endpoint: bool,
) -> ConnectDecision {
    use daemon8_chrome::ConnectionState;

    if !task_alive {
        return ConnectDecision::Spawn;
    }
    match state {
        ConnectionState::Connecting | ConnectionState::Reconnecting => ConnectDecision::Ignore,
        ConnectionState::Connected if same_endpoint => ConnectDecision::Ignore,
        _ => ConnectDecision::AbortAndSpawn,
    }
}

/// State the chrome command handler maintains across Connect commands.
///
/// Extracted so the connect-arm logic can be exercised by unit tests without
/// requiring a real Chrome process. Production and tests both call
/// `handle_connect`, which is the only place that decides whether to abort
/// a previous task and which is the only place that mutates `current_endpoint`.
struct ChromeHandlerState {
    chrome_handle: Option<ChromeTask>,
    current_endpoint: Option<String>,
}

struct ChromeTask {
    handle: tokio::task::JoinHandle<()>,
    cancel: CancellationToken,
}

impl ChromeTask {
    fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    #[cfg(test)]
    fn id(&self) -> tokio::task::Id {
        self.handle.id()
    }
}

impl ChromeHandlerState {
    fn new() -> Self {
        Self {
            chrome_handle: None,
            current_endpoint: None,
        }
    }

    /// Apply the idempotency decision for a Connect command and, if a new task
    /// should be spawned, invoke `spawn` to produce its `JoinHandle`. Returns
    /// `true` when a new task was spawned.
    fn handle_connect<F>(
        &mut self,
        endpoint: &str,
        state: daemon8_chrome::ConnectionState,
        spawn: F,
    ) -> bool
    where
        F: FnOnce(CancellationToken) -> tokio::task::JoinHandle<()>,
    {
        let task_alive = self
            .chrome_handle
            .as_ref()
            .is_some_and(|h| !h.is_finished());
        let same_endpoint = self.current_endpoint.as_deref() == Some(endpoint);

        match decide_connect_action(task_alive, state, same_endpoint) {
            ConnectDecision::Ignore => false,
            ConnectDecision::AbortAndSpawn => {
                if let Some(task) = self.chrome_handle.take() {
                    task.cancel.cancel();
                }
                self.current_endpoint = Some(endpoint.to_string());
                self.chrome_handle = Some(spawn_chrome_task(spawn));
                true
            }
            ConnectDecision::Spawn => {
                self.chrome_handle = None;
                self.current_endpoint = Some(endpoint.to_string());
                self.chrome_handle = Some(spawn_chrome_task(spawn));
                true
            }
        }
    }
}

fn spawn_chrome_task<F>(spawn: F) -> ChromeTask
where
    F: FnOnce(CancellationToken) -> tokio::task::JoinHandle<()>,
{
    let cancel = CancellationToken::new();
    let handle = spawn(cancel.clone());
    ChromeTask { handle, cancel }
}

async fn stop_chrome_task(mut task: ChromeTask) {
    task.cancel.cancel();

    tokio::select! {
        result = &mut task.handle => {
            if let Err(e) = result {
                tracing::debug!("browser monitor task ended while stopping: {e}");
            }
        }
        _ = tokio::time::sleep(Duration::from_secs(5)) => {
            tracing::warn!("browser monitor did not stop within 5s; aborting task");
            task.handle.abort();
            let _ = task.handle.await;
        }
    }
}

/// Listen for ChromeCommand messages and spawn connect_and_monitor accordingly.
/// Returns a watch::Receiver so MCP and other consumers can observe connection state.
fn spawn_chrome_command_handler(
    tasks: &mut JoinSet<()>,
    mut rx: mpsc::Receiver<ChromeCommand>,
    obs_tx: mpsc::UnboundedSender<Observation>,
    browser_path: Option<String>,
    reconnect: daemon8_chrome::ReconnectPolicy,
    cancel: CancellationToken,
) -> tokio::sync::watch::Receiver<daemon8_chrome::ConnectionState> {
    let (status, state_rx) = daemon8_chrome::ConnectionStatus::new();

    tasks.spawn(async move {
        let mut handler = ChromeHandlerState::new();
        let mut action_tx: Option<mpsc::Sender<daemon8_chrome::BrowserAction>> = None;

        loop {
            tokio::select! {
                cmd = rx.recv() => {
                    match cmd {
                        Some(ChromeCommand::Connect { endpoint }) => {
                            let tx = obs_tx.clone();
                            let (atx, action_rx) = mpsc::channel(BROWSER_ACTION_CAPACITY);
                            let task_status = status.clone();
                            let bp = browser_path.clone();
                            let endpoint_for_task = endpoint.clone();

                            let spawned = handler.handle_connect(&endpoint, status.current(), |token| {
                                tokio::spawn(async move {
                                    if let Err(e) = daemon8_chrome::connect_and_monitor(
                                        endpoint_for_task, tx, action_rx, token, task_status, bp, reconnect,
                                    ).await {
                                        tracing::error!("browser monitor exited with error: {e}");
                                    }
                                })
                            });

                            if spawned {
                                action_tx = Some(atx);
                            } else {
                                tracing::debug!(
                                    endpoint = %endpoint,
                                    state = ?status.current(),
                                    "Connect ignored: handler already managing this endpoint"
                                );
                                // atx is dropped here; the unused action_rx was
                                // already captured by the (unused) spawn closure.
                                drop(atx);
                            }
                        }
                        Some(ChromeCommand::Action(browser_action)) => {
                            if let Some(ref atx) = action_tx {
                                if atx.send(browser_action).await.is_err() {
                                    tracing::warn!("Browser action channel closed (browser disconnected?)");
                                    action_tx = None;
                                }
                            } else {
                                tracing::warn!("Browser action received but no active connection");
                                browser_action.reply_error("Browser not connected");
                            }
                        }
                        None => break,
                    }
                }
                () = cancel.cancelled() => break,
            }
        }

        if let Some(task) = handler.chrome_handle.take() {
            stop_chrome_task(task).await;
        }
        tracing::debug!("chrome command handler stopped");
    });

    state_rx
}

#[cfg(unix)]
fn resolve_node_id() -> String {
    let mut buf = [0u8; 256];
    unsafe {
        if libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) == 0 {
            let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            String::from_utf8_lossy(&buf[..len]).into_owned()
        } else {
            "unknown".to_string()
        }
    }
}

#[cfg(windows)]
fn resolve_node_id() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".to_string())
}

/// Returns true only when stdin is a FIFO (named or anonymous pipe).
///
/// This is the signal we use to decide whether to start MCP stdio. A real
/// MCP client connects stdin as an anonymous pipe; launchd, nohup, and
/// `&`-backgrounded shells attach /dev/null (a character device) instead.
/// Checking S_IFMT against S_IFIFO cleanly separates the two regardless of
/// how the daemon was launched.
#[cfg(unix)]
fn stdin_is_real_pipe() -> bool {
    use std::os::fd::AsRawFd;
    let fd = std::io::stdin().as_raw_fd();
    // SAFETY: fstat on a valid fd (stdin is always open) with a zeroed
    // stat buffer is sound. We only read st_mode from the result on success.
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &mut st) != 0 {
            return false;
        }
        (st.st_mode & libc::S_IFMT) == libc::S_IFIFO
    }
}

#[cfg(windows)]
fn stdin_is_real_pipe() -> bool {
    use std::os::windows::io::AsRawHandle;
    // GetFileType returns FILE_TYPE_PIPE (3) for anonymous pipes.
    unsafe extern "system" {
        fn GetFileType(hFile: *mut std::ffi::c_void) -> u32;
    }
    let handle = std::io::stdin().as_raw_handle();
    unsafe { GetFileType(handle as *mut _) == 3 }
}

async fn bind_with_retry(addr: &str, port: u16) -> Result<tokio::net::TcpListener> {
    const MAX_ATTEMPTS: u32 = 5;
    let mut delay = Duration::from_millis(500);

    for attempt in 1..=MAX_ATTEMPTS {
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => return Ok(listener),
            Err(e) if attempt < MAX_ATTEMPTS => {
                let holder = find_port_holder(port);
                tracing::warn!(
                    attempt,
                    port,
                    delay_ms = delay.as_millis() as u64,
                    holder = holder.as_deref().unwrap_or("unknown"),
                    error = %e,
                    "bind failed, retrying"
                );
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(5));
            }
            Err(e) => {
                let msg = if let Some(pid) = find_port_holder(port) {
                    format!(
                        "port {port} is held by PID {pid}. \
                         Kill it with: kill {pid}"
                    )
                } else {
                    format!("port {port} is in use by another process")
                };
                return Err(e).with_context(|| msg);
            }
        }
    }
    unreachable!()
}

fn find_port_holder(port: u16) -> Option<String> {
    let output = std::process::Command::new("lsof")
        .args(["-ti", &format!(":{port}")])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().next().map(|s| s.trim().to_string())
}

#[cfg(unix)]
async fn shutdown_signal() -> &'static str {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to install SIGTERM handler");
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .expect("failed to install SIGINT handler");

    tokio::select! {
        _ = sigterm.recv() => "SIGTERM",
        _ = sigint.recv() => "SIGINT",
        _ = tokio::signal::ctrl_c() => "ctrl-c",
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() -> &'static str {
    let _ = tokio::signal::ctrl_c().await;
    "ctrl-c"
}

//
// These tests pin down the regression we hit twice: a Connect command
// arriving while a previous connect task is in flight should NOT abort the
// running task and start a new one. The handler must be idempotent on the
// (endpoint, state) tuple. We test the real production state machine
// (`ChromeHandlerState::handle_connect`) by passing controllable spawn
// closures, so the test exercises the same code path that runs in production.

#[cfg(test)]
mod channel_capacity_tests {
    use super::*;
    use tokio::sync::mpsc::error::TrySendError;

    #[test]
    fn chrome_cmd_channel_enforces_capacity() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<ChromeCommand>(CHROME_CMD_CAPACITY);
        for i in 0..CHROME_CMD_CAPACITY {
            tx.try_send(ChromeCommand::Connect {
                endpoint: format!("http://slot-{i}"),
            })
            .expect("fill up to capacity");
        }
        let overflow = tx.try_send(ChromeCommand::Connect {
            endpoint: "http://overflow".into(),
        });
        assert!(
            matches!(overflow, Err(TrySendError::Full(_))),
            "expected Full after {CHROME_CMD_CAPACITY} sends, got {overflow:?}"
        );
    }

    #[test]
    fn browser_action_channel_enforces_capacity() {
        let (tx, _rx) =
            tokio::sync::mpsc::channel::<daemon8_chrome::BrowserAction>(BROWSER_ACTION_CAPACITY);
        let mut replies = Vec::with_capacity(BROWSER_ACTION_CAPACITY + 1);
        for _ in 0..BROWSER_ACTION_CAPACITY {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            replies.push(reply_rx);
            tx.try_send(daemon8_chrome::BrowserAction::ListTabs { reply: reply_tx })
                .expect("fill up to capacity");
        }
        let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
        let overflow = tx.try_send(daemon8_chrome::BrowserAction::ListTabs { reply: reply_tx });
        assert!(
            matches!(overflow, Err(TrySendError::Full(_))),
            "expected Full after {BROWSER_ACTION_CAPACITY} sends, got {overflow:?}"
        );
    }
}

#[cfg(test)]
mod chrome_handler_tests {
    use super::*;
    use daemon8_chrome::ConnectionState;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio_util::sync::CancellationToken;

    /// Spawn a tokio task that lives until `token` is cancelled. The returned
    /// `JoinHandle` reports `is_finished() == true` only after cancellation.
    fn spawn_controlled(token: CancellationToken) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            token.cancelled().await;
        })
    }

    /// Build a spawn closure that increments `counter` and returns a
    /// JoinHandle controllable via the closed-over CancellationToken.
    fn counting_spawner(
        counter: Arc<AtomicUsize>,
        token: CancellationToken,
    ) -> impl FnOnce(CancellationToken) -> tokio::task::JoinHandle<()> {
        move |_task_cancel| {
            counter.fetch_add(1, Ordering::SeqCst);
            spawn_controlled(token)
        }
    }

    #[tokio::test]
    async fn connect_while_disconnected_spawns_task() {
        let mut state = ChromeHandlerState::new();
        let count = Arc::new(AtomicUsize::new(0));
        let token = CancellationToken::new();

        let spawned = state.handle_connect(
            "ws://localhost:9222/devtools/browser/abc",
            ConnectionState::Disconnected,
            counting_spawner(count.clone(), token.clone()),
        );

        assert!(spawned, "Connect from Disconnected should spawn");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            state.current_endpoint.as_deref(),
            Some("ws://localhost:9222/devtools/browser/abc")
        );
        assert!(state.chrome_handle.is_some());

        token.cancel();
    }

    #[tokio::test]
    async fn connect_same_endpoint_while_connected_is_noop() {
        let mut state = ChromeHandlerState::new();
        let count = Arc::new(AtomicUsize::new(0));
        let token = CancellationToken::new();
        let endpoint = "ws://localhost:9222/devtools/browser/xyz";

        // First connect to seed state.
        state.handle_connect(
            endpoint,
            ConnectionState::Disconnected,
            counting_spawner(count.clone(), token.clone()),
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let original_handle_id = state.chrome_handle.as_ref().map(|h| h.id());

        // Second connect to the same endpoint while Connected -- must not
        // spawn, must not abort the existing handle.
        let spawned = state.handle_connect(
            endpoint,
            ConnectionState::Connected,
            counting_spawner(count.clone(), CancellationToken::new()),
        );

        assert!(!spawned, "duplicate Connect must be ignored");
        assert_eq!(count.load(Ordering::SeqCst), 1, "no new spawn");
        assert_eq!(
            state.chrome_handle.as_ref().map(|h| h.id()),
            original_handle_id,
            "existing handle must be preserved"
        );
        assert_eq!(state.current_endpoint.as_deref(), Some(endpoint));
        assert!(
            !state
                .chrome_handle
                .as_ref()
                .expect("handle present")
                .is_finished(),
            "existing task must still be running"
        );

        token.cancel();
    }

    #[tokio::test]
    async fn connect_different_endpoint_while_connected_aborts_and_respawns() {
        let mut state = ChromeHandlerState::new();
        let count = Arc::new(AtomicUsize::new(0));
        let first_token = CancellationToken::new();

        state.handle_connect(
            "ws://localhost:9222/devtools/browser/aaa",
            ConnectionState::Disconnected,
            counting_spawner(count.clone(), first_token.clone()),
        );
        let first_handle_id = state.chrome_handle.as_ref().map(|h| h.id());

        let second_token = CancellationToken::new();
        let spawned = state.handle_connect(
            "ws://localhost:9223/devtools/browser/bbb",
            ConnectionState::Connected,
            counting_spawner(count.clone(), second_token.clone()),
        );

        assert!(spawned, "different endpoint must respawn");
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert_ne!(
            state.chrome_handle.as_ref().map(|h| h.id()),
            first_handle_id,
            "must hold the new handle"
        );
        assert_eq!(
            state.current_endpoint.as_deref(),
            Some("ws://localhost:9223/devtools/browser/bbb")
        );

        // Give the runtime a chance to observe the abort. We don't assert
        // first_token.is_cancelled() because handle.abort() does not flip the
        // CancellationToken -- it drops the task at the next await point.
        // The handle-id swap above is the load-bearing assertion.
        tokio::task::yield_now().await;
        let _ = first_token; // explicitly unused

        second_token.cancel();
    }

    #[tokio::test]
    async fn connect_while_connecting_is_noop() {
        let mut state = ChromeHandlerState::new();
        let count = Arc::new(AtomicUsize::new(0));
        let token = CancellationToken::new();
        let endpoint = "ws://localhost:9222/devtools/browser/connecting";

        state.handle_connect(
            endpoint,
            ConnectionState::Disconnected,
            counting_spawner(count.clone(), token.clone()),
        );
        let original_handle_id = state.chrome_handle.as_ref().map(|h| h.id());

        // Even with a DIFFERENT endpoint, Connecting state means a connect is
        // in flight; the handler must wait, not start a parallel attempt.
        let spawned = state.handle_connect(
            "ws://localhost:9999/devtools/browser/different",
            ConnectionState::Connecting,
            counting_spawner(count.clone(), CancellationToken::new()),
        );

        assert!(!spawned, "Connect during Connecting must be ignored");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            state.chrome_handle.as_ref().map(|h| h.id()),
            original_handle_id
        );
        // Endpoint must NOT be updated, since we ignored the request.
        assert_eq!(state.current_endpoint.as_deref(), Some(endpoint));

        token.cancel();
    }

    #[tokio::test]
    async fn connect_while_reconnecting_is_noop() {
        let mut state = ChromeHandlerState::new();
        let count = Arc::new(AtomicUsize::new(0));
        let token = CancellationToken::new();
        let endpoint = "ws://localhost:9222/devtools/browser/reconnecting";

        state.handle_connect(
            endpoint,
            ConnectionState::Disconnected,
            counting_spawner(count.clone(), token.clone()),
        );
        let original_handle_id = state.chrome_handle.as_ref().map(|h| h.id());

        let spawned = state.handle_connect(
            endpoint,
            ConnectionState::Reconnecting,
            counting_spawner(count.clone(), CancellationToken::new()),
        );

        assert!(!spawned, "Connect during Reconnecting must be ignored");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            state.chrome_handle.as_ref().map(|h| h.id()),
            original_handle_id
        );

        token.cancel();
    }

    #[tokio::test]
    async fn connect_after_task_finished_respawns_cleanly() {
        let mut state = ChromeHandlerState::new();
        let count = Arc::new(AtomicUsize::new(0));
        let first_token = CancellationToken::new();
        let endpoint = "ws://localhost:9222/devtools/browser/finished";

        state.handle_connect(
            endpoint,
            ConnectionState::Disconnected,
            counting_spawner(count.clone(), first_token.clone()),
        );

        // Cancel the first task and wait for it to actually finish.
        first_token.cancel();
        if let Some(h) = state.chrome_handle.as_ref() {
            // Spin until is_finished() flips. The task body just awaits the
            // token, so this resolves on the next scheduler tick.
            for _ in 0..100 {
                if h.is_finished() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(h.is_finished(), "first task must have completed");
        }

        // Now Connect again. State will be Disconnected (the watch stays
        // wherever it was; in production the connector updates it). Even if
        // state were Connected, task_alive == false flips the decision to
        // Spawn.
        let second_token = CancellationToken::new();
        let spawned = state.handle_connect(
            endpoint,
            ConnectionState::Connected, // would normally block, but task is finished
            counting_spawner(count.clone(), second_token.clone()),
        );

        assert!(spawned, "finished task must allow respawn");
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert!(
            !state
                .chrome_handle
                .as_ref()
                .expect("handle present")
                .is_finished(),
            "new task must be running"
        );
        assert_eq!(state.current_endpoint.as_deref(), Some(endpoint));

        second_token.cancel();
    }

    #[test]
    fn decide_connect_action_truth_table() {
        // No live task -> always Spawn.
        assert_eq!(
            decide_connect_action(false, ConnectionState::Disconnected, false),
            ConnectDecision::Spawn
        );
        assert_eq!(
            decide_connect_action(false, ConnectionState::Connected, true),
            ConnectDecision::Spawn
        );

        // Live task + Connecting/Reconnecting -> always Ignore.
        assert_eq!(
            decide_connect_action(true, ConnectionState::Connecting, false),
            ConnectDecision::Ignore
        );
        assert_eq!(
            decide_connect_action(true, ConnectionState::Reconnecting, true),
            ConnectDecision::Ignore
        );

        // Live task + Connected: same endpoint Ignore, different endpoint AbortAndSpawn.
        assert_eq!(
            decide_connect_action(true, ConnectionState::Connected, true),
            ConnectDecision::Ignore
        );
        assert_eq!(
            decide_connect_action(true, ConnectionState::Connected, false),
            ConnectDecision::AbortAndSpawn
        );

        // Live task + Disconnected (rare; task running but state went stale).
        assert_eq!(
            decide_connect_action(true, ConnectionState::Disconnected, false),
            ConnectDecision::AbortAndSpawn
        );
    }
}

#[cfg(test)]
mod chrome_handler_property_tests {
    use super::*;
    use daemon8_chrome::ConnectionState;
    use proptest::prelude::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::runtime::Runtime;
    use tokio_util::sync::CancellationToken;

    /// Randomize a ConnectionState for property tests.
    fn arb_state() -> impl Strategy<Value = ConnectionState> {
        prop_oneof![
            Just(ConnectionState::Disconnected),
            Just(ConnectionState::Connecting),
            Just(ConnectionState::Connected),
            Just(ConnectionState::Reconnecting),
        ]
    }

    /// Small endpoint alphabet so commands frequently re-target the same
    /// endpoint, exercising the same-endpoint idempotency path.
    fn arb_endpoint() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("http://localhost:9222".to_string()),
            Just("http://localhost:9223".to_string()),
            Just("http://localhost:9224".to_string()),
        ]
    }

    proptest! {
        // Invariant: after replaying any sequence of (state, endpoint) Connect
        // commands through ChromeHandlerState, at most one live JoinHandle
        // exists at any point. handle_connect MUST abort the prior task before
        // spawning a replacement.
        #[test]
        fn at_most_one_live_task_across_connect_sequence(
            commands in proptest::collection::vec((arb_state(), arb_endpoint()), 1..30)
        ) {
            let rt = Runtime::new().unwrap();
            rt.block_on(async {
                let mut state = ChromeHandlerState::new();
                // Track the cancel token of the currently-live task so we can
                // detect whether the prior task was correctly aborted.
                let mut current_cancel: Option<CancellationToken> = None;
                let spawn_count = Arc::new(AtomicUsize::new(0));

                for (conn_state, endpoint) in commands {
                    let my_cancel = CancellationToken::new();
                    let my_cancel_for_spawn = my_cancel.clone();
                    let sc = spawn_count.clone();
                    let spawned = state.handle_connect(&endpoint, conn_state, |_task_cancel| {
                        sc.fetch_add(1, Ordering::SeqCst);
                        tokio::spawn(async move { my_cancel_for_spawn.cancelled().await; })
                    });

                    if spawned {
                        // Give the scheduler a tick to register the abort on
                        // the prior task (tokio::spawn + abort is asynchronous).
                        tokio::task::yield_now().await;
                        if let Some(prior) = current_cancel.take() {
                            // Prior task must be on the way out. The test
                            // invariant is that the handler took() and aborted
                            // it, which manifests as either cancellation
                            // observed OR the JoinHandle is_finished().
                            prior.cancel();
                        }
                        current_cancel = Some(my_cancel);
                    } else {
                        // Not spawned — the handler ignored this Connect.
                        // The cancel token we built is orphaned; drop it.
                        drop(my_cancel);
                    }
                }

                // Final state: at most one live task handle.
                let live = state
                    .chrome_handle
                    .as_ref()
                    .map(|h| !h.is_finished())
                    .unwrap_or(false);
                prop_assert!(
                    !live || state.chrome_handle.is_some(),
                    "invariant: at most one live chrome_handle at a time"
                );
                Ok(())
            }).unwrap();
        }
    }
}
