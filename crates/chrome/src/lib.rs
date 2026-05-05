// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

// Chrome CDP bridge -- connects to a running Chrome instance via raw WebSocket,
// subscribes to console and network events, normalizes them into Observations.
// Supports bidirectional browser actions (eval, screenshot, CSS injection).
// Handles reconnection with exponential backoff.
//
// Uses a raw CDP client instead of typed codegen libraries. All CDP events are
// parsed from serde_json::Value, extracting only the fields we need. This makes
// the bridge immune to Chrome version drift -- unknown fields are ignored.

mod actions;
mod cdp_client;
mod connection;
mod error;
mod events;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;
use std::time::Duration;

use daemon8_types::{Observation, ObservationKind, Origin, Severity};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc::{Receiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

use cdp_client::{CdpClient, CdpEvent};
pub use connection::{ConnectionState, ConnectionStatus};
pub use error::{ChromeError, Result};

#[derive(Debug, Clone, Copy)]
pub struct ReconnectPolicy {
    pub initial: Duration,
    pub max: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(5),
            max: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabInfo {
    pub id: String,
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerfMetric {
    pub name: String,
    pub value: f64,
}

impl std::fmt::Debug for BrowserAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EvalJs {
                tab_id, expression, ..
            } => f
                .debug_struct("EvalJs")
                .field("tab_id", tab_id)
                .field("expression", expression)
                .finish_non_exhaustive(),
            Self::Screenshot {
                tab_id, selector, ..
            } => f
                .debug_struct("Screenshot")
                .field("tab_id", tab_id)
                .field("selector", selector)
                .finish_non_exhaustive(),
            Self::InjectCss {
                tab_id,
                css,
                temporary,
                ..
            } => f
                .debug_struct("InjectCss")
                .field("tab_id", tab_id)
                .field("css", css)
                .field("temporary", temporary)
                .finish_non_exhaustive(),
            Self::RevertCss { tab_id, .. } => f
                .debug_struct("RevertCss")
                .field("tab_id", tab_id)
                .finish_non_exhaustive(),
            Self::ListTabs { .. } => f.debug_struct("ListTabs").finish_non_exhaustive(),
            Self::GetPerformanceMetrics { tab_id, .. } => f
                .debug_struct("GetPerformanceMetrics")
                .field("tab_id", tab_id)
                .finish_non_exhaustive(),
            Self::GetDom {
                tab_id, selector, ..
            } => f
                .debug_struct("GetDom")
                .field("tab_id", tab_id)
                .field("selector", selector)
                .finish_non_exhaustive(),
            Self::SetViewport {
                tab_id,
                width,
                height,
                ..
            } => f
                .debug_struct("SetViewport")
                .field("tab_id", tab_id)
                .field("width", width)
                .field("height", height)
                .finish_non_exhaustive(),
            Self::ClearViewport { tab_id, .. } => f
                .debug_struct("ClearViewport")
                .field("tab_id", tab_id)
                .finish_non_exhaustive(),
            Self::SetNetworkConditions { tab_id, preset, .. } => f
                .debug_struct("SetNetworkConditions")
                .field("tab_id", tab_id)
                .field("preset", preset)
                .finish_non_exhaustive(),
            Self::Navigate { tab_id, url, .. } => f
                .debug_struct("Navigate")
                .field("tab_id", tab_id)
                .field("url", url)
                .finish_non_exhaustive(),
            Self::StorageClear {
                tab_id,
                storage_types,
                ..
            } => f
                .debug_struct("StorageClear")
                .field("tab_id", tab_id)
                .field("storage_types", storage_types)
                .finish_non_exhaustive(),
            Self::StorageInspect { tab_id, .. } => f
                .debug_struct("StorageInspect")
                .field("tab_id", tab_id)
                .finish_non_exhaustive(),
            Self::StorageSet {
                tab_id,
                store_type,
                key,
                ..
            } => f
                .debug_struct("StorageSet")
                .field("tab_id", tab_id)
                .field("store_type", store_type)
                .field("key", key)
                .finish_non_exhaustive(),
            Self::ElementAtPoint { tab_id, x, y, .. } => f
                .debug_struct("ElementAtPoint")
                .field("tab_id", tab_id)
                .field("x", x)
                .field("y", y)
                .finish_non_exhaustive(),
            Self::NewTab { url, .. } => f
                .debug_struct("NewTab")
                .field("url", url)
                .finish_non_exhaustive(),
            Self::CloseTab { tab_id, .. } => f
                .debug_struct("CloseTab")
                .field("tab_id", tab_id)
                .finish_non_exhaustive(),
        }
    }
}

pub enum BrowserAction {
    EvalJs {
        tab_id: Option<String>,
        expression: String,
        reply: tokio::sync::oneshot::Sender<Result<String>>,
    },
    Screenshot {
        tab_id: Option<String>,
        selector: Option<String>,
        reply: tokio::sync::oneshot::Sender<Result<Vec<u8>>>,
    },
    InjectCss {
        tab_id: Option<String>,
        css: String,
        temporary: bool,
        reply: tokio::sync::oneshot::Sender<Result<String>>,
    },
    RevertCss {
        tab_id: Option<String>,
        reply: tokio::sync::oneshot::Sender<Result<u32>>,
    },
    ListTabs {
        reply: tokio::sync::oneshot::Sender<Result<Vec<TabInfo>>>,
    },
    GetPerformanceMetrics {
        tab_id: Option<String>,
        reply: tokio::sync::oneshot::Sender<Result<Vec<PerfMetric>>>,
    },
    GetDom {
        tab_id: Option<String>,
        selector: Option<String>,
        reply: tokio::sync::oneshot::Sender<Result<String>>,
    },
    SetViewport {
        tab_id: Option<String>,
        width: u32,
        height: u32,
        device_scale_factor: f64,
        mobile: bool,
        user_agent: Option<String>,
        reply: tokio::sync::oneshot::Sender<Result<()>>,
    },
    ClearViewport {
        tab_id: Option<String>,
        reply: tokio::sync::oneshot::Sender<Result<()>>,
    },
    SetNetworkConditions {
        tab_id: Option<String>,
        preset: String,
        reply: tokio::sync::oneshot::Sender<Result<()>>,
    },
    Navigate {
        tab_id: Option<String>,
        url: String,
        reply: tokio::sync::oneshot::Sender<Result<String>>,
    },
    StorageClear {
        tab_id: Option<String>,
        storage_types: String,
        reply: tokio::sync::oneshot::Sender<Result<()>>,
    },
    StorageInspect {
        tab_id: Option<String>,
        reply: tokio::sync::oneshot::Sender<Result<serde_json::Value>>,
    },
    StorageSet {
        tab_id: Option<String>,
        store_type: String,
        key: String,
        value: String,
        reply: tokio::sync::oneshot::Sender<Result<()>>,
    },
    ElementAtPoint {
        tab_id: Option<String>,
        x: f64,
        y: f64,
        reply: tokio::sync::oneshot::Sender<Result<serde_json::Value>>,
    },
    NewTab {
        url: String,
        reply: tokio::sync::oneshot::Sender<Result<String>>,
    },
    CloseTab {
        tab_id: String,
        reply: tokio::sync::oneshot::Sender<Result<()>>,
    },
}

impl BrowserAction {
    pub fn reply_error(self, msg: &str) {
        let msg = msg.to_string();
        match self {
            Self::EvalJs { reply, .. } => {
                let _ = reply.send(Err(ChromeError::Cdp(msg)));
            }
            Self::Screenshot { reply, .. } => {
                let _ = reply.send(Err(ChromeError::Cdp(msg)));
            }
            Self::InjectCss { reply, .. } => {
                let _ = reply.send(Err(ChromeError::Cdp(msg)));
            }
            Self::RevertCss { reply, .. } => {
                let _ = reply.send(Err(ChromeError::Cdp(msg)));
            }
            Self::ListTabs { reply, .. } => {
                let _ = reply.send(Err(ChromeError::Cdp(msg)));
            }
            Self::GetPerformanceMetrics { reply, .. } => {
                let _ = reply.send(Err(ChromeError::Cdp(msg)));
            }
            Self::GetDom { reply, .. } => {
                let _ = reply.send(Err(ChromeError::Cdp(msg)));
            }
            Self::SetViewport { reply, .. } => {
                let _ = reply.send(Err(ChromeError::Cdp(msg)));
            }
            Self::ClearViewport { reply, .. } => {
                let _ = reply.send(Err(ChromeError::Cdp(msg)));
            }
            Self::SetNetworkConditions { reply, .. } => {
                let _ = reply.send(Err(ChromeError::Cdp(msg)));
            }
            Self::Navigate { reply, .. } => {
                let _ = reply.send(Err(ChromeError::Cdp(msg)));
            }
            Self::StorageClear { reply, .. } => {
                let _ = reply.send(Err(ChromeError::Cdp(msg)));
            }
            Self::StorageInspect { reply, .. } => {
                let _ = reply.send(Err(ChromeError::Cdp(msg)));
            }
            Self::StorageSet { reply, .. } => {
                let _ = reply.send(Err(ChromeError::Cdp(msg)));
            }
            Self::ElementAtPoint { reply, .. } => {
                let _ = reply.send(Err(ChromeError::Cdp(msg)));
            }
            Self::NewTab { reply, .. } => {
                let _ = reply.send(Err(ChromeError::Cdp(msg)));
            }
            Self::CloseTab { reply, .. } => {
                let _ = reply.send(Err(ChromeError::Cdp(msg)));
            }
        }
    }
}

struct SessionState {
    target_id: String,
    url: String,
}

struct InjectedStyle {
    session_id: String,
    element_id: String,
}

struct PendingRequest {
    method: String,
    url: String,
    timestamp: f64,
    wall_clock: std::time::Instant,
}

struct BrowserLaunch {
    endpoint: String,
    managed: Option<ManagedBrowser>,
}

struct ManagedBrowser {
    pid: u32,
    user_data_dir: PathBuf,
    child: Option<Child>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MonitorDisconnectAction {
    WaitForNextRequest,
    RetryEndpoint,
}

fn monitor_disconnect_action(has_managed_browser: bool) -> MonitorDisconnectAction {
    if has_managed_browser {
        MonitorDisconnectAction::WaitForNextRequest
    } else {
        MonitorDisconnectAction::RetryEndpoint
    }
}

pub async fn connect_and_monitor(
    endpoint: String,
    obs_tx: UnboundedSender<Observation>,
    mut action_rx: Receiver<BrowserAction>,
    cancel: CancellationToken,
    status: ConnectionStatus,
    browser_path: Option<String>,
    reconnect: ReconnectPolicy,
) -> Result<()> {
    let mut backoff = reconnect.initial;
    let max_backoff = reconnect.max;
    let mut launch_attempted = false;
    let mut endpoint = endpoint;
    let mut managed_browser: Option<ManagedBrowser> = None;

    loop {
        if cancel.is_cancelled() {
            if let Some(mut browser) = managed_browser.take() {
                browser
                    .terminate("daemon shutdown; cleaning up managed browser")
                    .await;
            }
            status.transition(ConnectionState::Disconnected);
            return Ok(());
        }

        status.transition(ConnectionState::Disconnected);

        let ws_url = match cdp_client::discover_ws_url(&endpoint).await {
            Ok(url) => url,
            Err(e) => {
                if !launch_attempted {
                    launch_attempted = true;
                    match launch_chrome(browser_path.as_deref()).await {
                        Ok(launch) => {
                            managed_browser = launch.managed;
                            endpoint = launch.endpoint;
                            backoff = Duration::from_secs(1);
                            continue;
                        }
                        Err(launch_err) => {
                            tracing::warn!("Browser auto-launch failed: {launch_err}");
                        }
                    }
                }
                tracing::warn!(
                    "Browser not reachable: {e}, retrying in {}s...",
                    backoff.as_secs()
                );
                wait_or_cancel(backoff, &cancel).await;
                backoff = (backoff * 2).min(max_backoff);
                continue;
            }
        };

        status.transition(ConnectionState::Connecting);

        match CdpClient::connect(&ws_url).await {
            Ok(client) => {
                backoff = reconnect.initial;
                launch_attempted = true; // Connection worked; don't fallback to managed launch immediately on next error
                tracing::info!("Connected to browser at {endpoint}");
                let client = Arc::new(client);
                status.transition(ConnectionState::Connected);

                monitor_browser(
                    client,
                    &endpoint,
                    obs_tx.clone(),
                    &mut action_rx,
                    cancel.clone(),
                )
                .await;

                if cancel.is_cancelled() {
                    if let Some(mut browser) = managed_browser.take() {
                        browser
                            .terminate("daemon shutdown; cleaning up managed browser")
                            .await;
                    }
                    status.transition(ConnectionState::Disconnected);
                    return Ok(());
                }

                status.transition(ConnectionState::Reconnecting);
                let action = monitor_disconnect_action(managed_browser.is_some());
                if let Some(mut browser) = managed_browser.take() {
                    browser
                        .terminate("browser monitor disconnected; waiting for next browser request")
                        .await;
                }

                if action == MonitorDisconnectAction::WaitForNextRequest {
                    status.transition(ConnectionState::Disconnected);
                    tracing::info!(
                        "Managed browser disconnected; waiting for next browser request"
                    );
                    return Ok(());
                }
                tracing::warn!(
                    "Browser disconnected, reconnecting in {}s...",
                    backoff.as_secs()
                );
            }
            Err(e) => {
                tracing::warn!(
                    "Browser connect failed: {e}, retrying in {}s...",
                    backoff.as_secs()
                );
            }
        }

        wait_or_cancel(backoff, &cancel).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

impl ManagedBrowser {
    async fn terminate(&mut self, reason: &str) {
        if self.child.is_none() {
            tracing::info!(
                pid = self.pid,
                profile = %self.user_data_dir.display(),
                reason,
                "releasing hold on reattached browser (not daemon-spawned)"
            );
            return;
        }

        tracing::warn!(
            pid = self.pid,
            profile = %self.user_data_dir.display(),
            reason,
            "terminating managed browser"
        );

        terminate_pid(self.pid);
        if browser_exited(self, Duration::from_secs(2)).await {
            return;
        }

        force_kill_pid(self.pid, self.child.as_mut());
        let _ = browser_exited(self, Duration::from_secs(2)).await;
    }
}

async fn browser_exited(browser: &mut ManagedBrowser, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(child) = browser.child.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) => {}
                Err(e) => {
                    tracing::debug!(pid = browser.pid, error = %e, "failed to reap browser child");
                    return true;
                }
            }
        } else if !pid_alive(browser.pid) {
            return true;
        }

        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(unix)]
fn terminate_pid(pid: u32) {
    // SAFETY: kill is called with a concrete PID discovered from Chrome's own
    // profile lock or from the child process we spawned. SIGTERM requests
    // normal shutdown; errors are harmless because the process may already exit.
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if rc != 0 {
        tracing::debug!(pid, "SIGTERM failed or process already exited");
    }
}

#[cfg(windows)]
fn terminate_pid(_pid: u32) {}

#[cfg(not(any(unix, windows)))]
fn terminate_pid(_pid: u32) {}

#[cfg(unix)]
fn force_kill_pid(pid: u32, _child: Option<&mut Child>) {
    // SAFETY: same PID provenance as terminate_pid. SIGKILL is only used after
    // a bounded graceful shutdown wait for daemon-managed browser instances.
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    if rc != 0 {
        tracing::debug!(pid, "SIGKILL failed or process already exited");
    }
}

#[cfg(windows)]
fn force_kill_pid(pid: u32, child: Option<&mut Child>) {
    if let Some(child) = child {
        let _ = child.kill();
        return;
    }

    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(not(any(unix, windows)))]
fn force_kill_pid(_pid: u32, child: Option<&mut Child>) {
    if let Some(child) = child {
        let _ = child.kill();
    }
}

async fn wait_or_cancel(duration: Duration, cancel: &CancellationToken) {
    tokio::select! {
        () = tokio::time::sleep(duration) => {}
        () = cancel.cancelled() => {}
    }
}

async fn monitor_browser(
    client: Arc<CdpClient>,
    endpoint: &str,
    obs_tx: UnboundedSender<Observation>,
    action_rx: &mut Receiver<BrowserAction>,
    cancel: CancellationToken,
) {
    // Session registry: session_id -> SessionState
    let mut sessions: HashMap<String, SessionState> = HashMap::new();
    // Reverse lookup: target_id -> session_id
    let mut target_to_session: HashMap<String, String> = HashMap::new();
    // Pending network requests: (session_id, request_id) -> PendingRequest
    let mut pending_requests: HashMap<(String, String), PendingRequest> = HashMap::new();
    // Injected CSS styles
    let mut injected_styles: Vec<InjectedStyle> = Vec::new();
    let mut css_counter: u64 = 0;

    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<CdpEvent>(4096);
    let pump_cancel = cancel.clone();
    let pump_client = client.clone();
    tokio::spawn(async move {
        pump_client.run(event_tx, pump_cancel).await;
    });

    // Initial target discovery via HTTP /json/list
    discover_and_attach(&client, endpoint, &mut sessions, &mut target_to_session).await;

    // Enable push-based target discovery so new tabs are detected immediately
    // instead of waiting for the fallback scan timer.
    if let Err(e) = client
        .send_command("Target.setDiscoverTargets", json!({"discover": true}), None)
        .await
    {
        tracing::warn!("failed to enable target discovery: {e}");
    }

    // Auto-attach to child targets (workers, OOPIFs, popups) so their
    // console and network activity is captured without manual attachment.
    if let Err(e) = client
        .send_command(
            "Target.setAutoAttach",
            json!({
                "autoAttach": true,
                "waitForDebuggerOnStart": false,
                "flatten": true,
            }),
            None,
        )
        .await
    {
        tracing::warn!("failed to enable auto-attach: {e}");
    }

    let mut scan_timer = tokio::time::interval(Duration::from_secs(60));
    scan_timer.tick().await;

    let mut health_timer = tokio::time::interval(Duration::from_secs(20));
    health_timer.tick().await;

    let mut sweep_timer = tokio::time::interval(Duration::from_secs(10));
    sweep_timer.tick().await;

    loop {
        tokio::select! {
            _ = sweep_timer.tick() => {
                let before = pending_requests.len();
                if before > 0 {
                    let now = std::time::Instant::now();
                    pending_requests.retain(|_, v| now.duration_since(v.wall_clock).as_secs() < 10);
                    let removed = before - pending_requests.len();
                    if removed > 0 {
                        tracing::debug!(removed, remaining = pending_requests.len(), "swept stale pending requests");
                    }
                }
            }
            _ = scan_timer.tick() => {
                scan_for_new_targets(
                    &client,
                    &mut sessions,
                    &mut target_to_session,
                ).await;
            }
            _ = health_timer.tick() => {
                let health = tokio::time::timeout(
                    Duration::from_secs(5),
                    client.send_command("Browser.getVersion", json!({}), None),
                ).await;
                match health {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        tracing::warn!("health check failed: {e}");
                        return;
                    }
                    Err(_) => {
                        tracing::warn!("health check timed out (5s), triggering reconnect");
                        return;
                    }
                }
            }
            event = event_rx.recv() => {
                match event {
                    Some(ev) => {
                        handle_cdp_event(
                            &ev,
                            &mut sessions,
                            &mut target_to_session,
                            &mut pending_requests,
                            &obs_tx,
                            &client,
                        ).await;
                    }
                    None => {
                        tracing::debug!("CDP event pump closed (browser disconnected)");
                        return;
                    }
                }
            }
            action = action_rx.recv() => {
                match action {
                    Some(action) => {
                        execute_action(
                            action,
                            &client,
                            &sessions,
                            &mut injected_styles,
                            &mut css_counter,
                        ).await;
                    }
                    None => {
                        tracing::debug!("Action channel closed");
                        return;
                    }
                }
            }
            () = cancel.cancelled() => {
                tracing::debug!("Browser monitor cancelled");
                return;
            }
        }
    }
}

async fn discover_and_attach(
    client: &Arc<CdpClient>,
    endpoint: &str,
    sessions: &mut HashMap<String, SessionState>,
    target_to_session: &mut HashMap<String, String>,
) {
    let targets = match cdp_client::list_targets(endpoint).await {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!("failed to list targets: {e}");
            return;
        }
    };

    for target in targets
        .iter()
        .filter(|t| t.target_type == "page" && !is_internal_url(&t.url))
    {
        if target_to_session.contains_key(&target.id) {
            continue;
        }

        match actions::attach_target(client, &target.id).await {
            Ok(session_id) => {
                tracing::info!("Now monitoring tab: {} ({})", target.url, target.id);
                actions::enable_domains(client, &session_id).await;
                sessions.insert(
                    session_id.clone(),
                    SessionState {
                        target_id: target.id.clone(),
                        url: target.url.clone(),
                    },
                );
                target_to_session.insert(target.id.clone(), session_id);
            }
            Err(e) => {
                tracing::debug!("failed to attach to {}: {e}", target.url);
            }
        }
    }
}

async fn scan_for_new_targets(
    client: &Arc<CdpClient>,
    sessions: &mut HashMap<String, SessionState>,
    target_to_session: &mut HashMap<String, String>,
) {
    let targets = match actions::get_targets(client).await {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!("target scan failed: {e}");
            return;
        }
    };

    // Collect live user-facing page targets. Filters out Chrome's internal
    // CDP targets (chrome://, chrome-extension://, devtools://, chrome-error://,
    // chrome-untrusted://) which expose themselves as type=page but are not
    // user-addressable tabs. Omnibox suggestion popups in particular flash in
    // and out of existence and would otherwise pollute list_tabs output.
    let live_ids: std::collections::HashSet<String> = targets
        .iter()
        .filter(|t| is_user_target(t))
        .filter_map(|t| t["targetId"].as_str().map(String::from))
        .collect();

    // Prune stale sessions
    let stale: Vec<String> = target_to_session
        .keys()
        .filter(|id| !live_ids.contains(id.as_str()))
        .cloned()
        .collect();
    for id in stale {
        if let Some(sid) = target_to_session.remove(&id) {
            sessions.remove(&sid);
        }
    }

    // Attach new targets
    for target in &targets {
        if !is_user_target(target) {
            continue;
        }
        let target_id = match target["targetId"].as_str() {
            Some(id) => id,
            None => continue,
        };
        let url = target["url"].as_str().unwrap_or("");
        if target_to_session.contains_key(target_id) {
            continue;
        }

        match actions::attach_target(client, target_id).await {
            Ok(session_id) => {
                tracing::info!("Now monitoring tab: {url} ({target_id})");
                actions::enable_domains(client, &session_id).await;
                sessions.insert(
                    session_id.clone(),
                    SessionState {
                        target_id: target_id.to_string(),
                        url: url.to_string(),
                    },
                );
                target_to_session.insert(target_id.to_string(), session_id);
            }
            Err(e) => {
                tracing::debug!("failed to attach to {url}: {e}");
            }
        }
    }
}

async fn handle_cdp_event(
    event: &CdpEvent,
    sessions: &mut HashMap<String, SessionState>,
    target_to_session: &mut HashMap<String, String>,
    pending_requests: &mut HashMap<(String, String), PendingRequest>,
    obs_tx: &UnboundedSender<Observation>,
    client: &Arc<CdpClient>,
) {
    let session_id = event.session_id.as_deref().unwrap_or("");

    tracing::trace!(
        method = %event.method,
        session = session_id,
        known = sessions.contains_key(session_id),
        "CDP event received"
    );

    // Look up the tab info for this session
    let (tab_id, page_url) = sessions
        .get(session_id)
        .map(|s| (s.target_id.as_str(), s.url.as_str()))
        .unwrap_or(("", ""));

    match event.method.as_str() {
        "Runtime.consoleAPICalled" => {
            if let Some(ev) = events::parse_console(&event.params) {
                tracing::debug!(message = %ev.message, severity = %ev.severity, "console event captured");
                let obs = Observation {
                    id: 0,
                    origin: Origin::Browser {
                        tab_id: tab_id.into(),
                        url: page_url.to_string(),
                    },
                    kind: ObservationKind::Log,
                    data: json!({
                        "message": ev.message,
                        "console_type": ev.console_type,
                    }),
                    severity: ev.severity,
                    source_location: ev.source_location,
                    timestamp_ns: ev.timestamp_ns,
                    correlation_id: None,
                    parent_id: None,
                    tags: None,
                    session_id: None,
                    node_id: None,
                };
                let _ = obs_tx.send(obs);
            }
        }

        "Log.entryAdded" => {
            if let Some(ev) = events::parse_log_entry(&event.params) {
                let obs = Observation {
                    id: 0,
                    origin: Origin::Browser {
                        tab_id: tab_id.into(),
                        url: page_url.to_string(),
                    },
                    kind: ObservationKind::Log,
                    data: json!({
                        "message": ev.message,
                        "source": ev.source,
                        "url": ev.url,
                    }),
                    severity: ev.severity,
                    source_location: None,
                    timestamp_ns: ev.timestamp_ns,
                    correlation_id: None,
                    parent_id: None,
                    tags: None,
                    session_id: None,
                    node_id: None,
                };
                let _ = obs_tx.send(obs);
            }
        }

        "Network.requestWillBeSent" => {
            if let Some(ev) = events::parse_request(&event.params) {
                pending_requests.insert(
                    (session_id.to_string(), ev.request_id),
                    PendingRequest {
                        method: ev.method,
                        url: ev.url,
                        timestamp: ev.timestamp,
                        wall_clock: std::time::Instant::now(),
                    },
                );
            }
        }

        "Network.responseReceived" => {
            if let Some(ev) = events::parse_response(&event.params) {
                let key = (session_id.to_string(), ev.request_id.clone());
                let pending = pending_requests.remove(&key);

                let (method, req_url, duration_ms) = match pending {
                    Some(p) => (p.method, p.url, Some((ev.timestamp - p.timestamp) * 1000.0)),
                    None => ("?".to_string(), ev.url.clone(), None),
                };

                let severity = match ev.status {
                    500..=599 => Severity::Error,
                    400..=499 => Severity::Warn,
                    _ => Severity::Debug,
                };

                let obs = Observation {
                    id: 0,
                    origin: Origin::Browser {
                        tab_id: tab_id.into(),
                        url: page_url.to_string(),
                    },
                    kind: ObservationKind::HttpExchange {
                        method: method.to_string(),
                        url: req_url.to_string(),
                        status: Some(ev.status),
                        duration_ms,
                    },
                    data: json!({"mime_type": ev.mime_type}),
                    severity,
                    source_location: None,
                    timestamp_ns: (ev.timestamp * 1_000_000_000.0).clamp(0.0, u64::MAX as f64)
                        as u64,
                    correlation_id: None,
                    parent_id: None,
                    tags: None,
                    session_id: None,
                    node_id: None,
                };
                let _ = obs_tx.send(obs);

                sweep_pending(pending_requests);
            }
        }

        "Network.loadingFailed" => {
            if let Some(ev) = events::parse_loading_failed(&event.params) {
                let key = (session_id.to_string(), ev.request_id.clone());
                let pending = pending_requests.remove(&key);

                let (method, req_url) = match &pending {
                    Some(p) => (p.method.as_str(), p.url.as_str()),
                    None => ("?", ""),
                };

                let obs = Observation {
                    id: 0,
                    origin: Origin::Browser {
                        tab_id: tab_id.into(),
                        url: page_url.to_string(),
                    },
                    kind: ObservationKind::HttpExchange {
                        method: method.to_string(),
                        url: req_url.to_string(),
                        status: None,
                        duration_ms: None,
                    },
                    data: json!({"error": ev.error_text, "canceled": ev.canceled}),
                    severity: Severity::Error,
                    source_location: None,
                    timestamp_ns: (ev.timestamp * 1_000_000_000.0).clamp(0.0, u64::MAX as f64)
                        as u64,
                    correlation_id: None,
                    parent_id: None,
                    tags: None,
                    session_id: None,
                    node_id: None,
                };
                let _ = obs_tx.send(obs);

                sweep_pending(pending_requests);
            }
        }

        "Runtime.exceptionThrown" => {
            if let Some(ev) = events::parse_exception(&event.params) {
                let obs = Observation {
                    id: 0,
                    origin: Origin::Browser {
                        tab_id: tab_id.into(),
                        url: page_url.to_string(),
                    },
                    kind: ObservationKind::JsException {
                        message: ev.message.clone(),
                        line: ev.line,
                        column: ev.column,
                    },
                    data: json!({
                        "message": ev.message,
                        "url": ev.url,
                        "trace": ev.trace,
                    }),
                    severity: Severity::Error,
                    source_location: ev.source_location,
                    timestamp_ns: ev.timestamp_ns,
                    correlation_id: None,
                    parent_id: None,
                    tags: None,
                    session_id: None,
                    node_id: None,
                };
                let _ = obs_tx.send(obs);
            }
        }

        "Page.lifecycleEvent" => {
            if let Some(ev) = events::parse_lifecycle(&event.params) {
                let obs = Observation {
                    id: 0,
                    origin: Origin::Browser {
                        tab_id: tab_id.into(),
                        url: page_url.to_string(),
                    },
                    kind: ObservationKind::Lifecycle {
                        event_name: ev.name.clone(),
                        frame_id: ev.frame_id,
                    },
                    data: json!({"event": ev.name}),
                    severity: Severity::Info,
                    source_location: None,
                    timestamp_ns: ev.timestamp_ns,
                    correlation_id: None,
                    parent_id: None,
                    tags: None,
                    session_id: None,
                    node_id: None,
                };
                let _ = obs_tx.send(obs);
            }
        }

        "Target.targetCreated" => {
            let info = &event.params["targetInfo"];
            if info["type"].as_str() == Some("page")
                && let Some(target_id) = info["targetId"].as_str()
            {
                let url = info["url"].as_str().unwrap_or("");
                if is_internal_url(url) {
                    tracing::debug!("Skipping internal target: {url}");
                } else if !target_to_session.contains_key(target_id) {
                    match actions::attach_target(client, target_id).await {
                        Ok(sid) => {
                            tracing::info!("Now monitoring new tab: {url} ({target_id})");
                            actions::enable_domains(client, &sid).await;
                            sessions.insert(
                                sid.clone(),
                                SessionState {
                                    target_id: target_id.to_string(),
                                    url: url.to_string(),
                                },
                            );
                            target_to_session.insert(target_id.to_string(), sid);
                        }
                        Err(e) => {
                            tracing::debug!("failed to attach to new tab {url}: {e}");
                        }
                    }
                }
            }
        }

        "Target.targetDestroyed" => {
            if let Some(target_id) = event.params["targetId"].as_str()
                && let Some(sid) = target_to_session.remove(target_id)
            {
                sessions.remove(&sid);
                tracing::debug!("Tab closed: {target_id}");
            }
        }

        "Target.attachedToTarget" => {
            // Auto-attached child target (worker, OOPIF, popup).
            // The child session ID is in params.sessionId, target info in params.targetInfo.
            let child_sid = event.params["sessionId"].as_str().unwrap_or("");
            let info = &event.params["targetInfo"];
            let target_id = info["targetId"].as_str().unwrap_or("");
            let target_type = info["type"].as_str().unwrap_or("");
            let url = info["url"].as_str().unwrap_or("");

            if !child_sid.is_empty()
                && !target_id.is_empty()
                && !target_to_session.contains_key(target_id)
                && target_type == "page"
                && !is_internal_url(url)
            {
                tracing::debug!("Auto-attached {target_type}: {url} ({target_id})");
                actions::enable_domains(client, child_sid).await;
                sessions.insert(
                    child_sid.to_string(),
                    SessionState {
                        target_id: target_id.to_string(),
                        url: url.to_string(),
                    },
                );
                target_to_session.insert(target_id.to_string(), child_sid.to_string());
            }
        }

        "Target.targetInfoChanged" => {
            let info = &event.params["targetInfo"];
            if let Some(target_id) = info["targetId"].as_str()
                && let Some(url) = info["url"].as_str()
                && let Some(sid) = target_to_session.get(target_id)
                && let Some(state) = sessions.get_mut(sid)
            {
                state.url = url.to_string();
            }
        }

        "Target.targetCrashed" => {
            if let Some(target_id) = event.params["targetId"].as_str() {
                tracing::error!("Tab crashed: {target_id}");
                if let Some(sid) = target_to_session.remove(target_id) {
                    sessions.remove(&sid);
                }
                let now_ns = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64;
                let obs = Observation {
                    id: 0,
                    origin: Origin::Browser {
                        tab_id: target_id.into(),
                        url: String::new(),
                    },
                    kind: ObservationKind::Lifecycle {
                        event_name: "crashed".to_string(),
                        frame_id: String::new(),
                    },
                    data: json!({"event": "tab_crashed", "target_id": target_id}),
                    severity: Severity::Error,
                    source_location: None,
                    timestamp_ns: now_ns,
                    correlation_id: None,
                    parent_id: None,
                    tags: None,
                    session_id: None,
                    node_id: None,
                };
                let _ = obs_tx.send(obs);
            }
        }

        "Inspector.detached" => {
            let reason = event.params["reason"].as_str().unwrap_or("unknown");
            tracing::warn!("Inspector detached from session {session_id}: {reason}");
            if let Some(state) = sessions.remove(session_id) {
                target_to_session.remove(&state.target_id);
            }
        }

        "Page.frameNavigated" => {
            let frame = &event.params["frame"];
            let parent_id = frame["parentFrameId"].as_str().unwrap_or("");
            if parent_id.is_empty() && !session_id.is_empty() && sessions.contains_key(session_id) {
                tracing::debug!(
                    "Main frame navigated, re-enabling domains for session {session_id}"
                );
                actions::enable_domains(client, session_id).await;
                if let Some(state) = sessions.get_mut(session_id)
                    && let Some(url) = frame["url"].as_str()
                {
                    state.url = url.to_string();
                }
            }
        }

        // All other CDP events are silently ignored.
        // This is the Chrome-version-immunity guarantee.
        _ => {}
    }
}

fn sweep_pending(pending: &mut HashMap<(String, String), PendingRequest>) {
    if pending.len() < 50 {
        return;
    }
    let max_ts = pending.values().map(|p| p.timestamp).fold(0.0f64, f64::max);
    let cutoff = max_ts - 30.0;
    pending.retain(|_, v| v.timestamp > cutoff);
}

async fn execute_action(
    action: BrowserAction,
    client: &Arc<CdpClient>,
    sessions: &HashMap<String, SessionState>,
    injected_styles: &mut Vec<InjectedStyle>,
    css_counter: &mut u64,
) {
    match action {
        BrowserAction::EvalJs {
            tab_id,
            expression,
            reply,
        } => {
            let result = match resolve_session(sessions, tab_id.as_deref()) {
                Ok(sid) => actions::eval_js(client, sid, &expression).await,
                Err(e) => Err(e),
            };
            let _ = reply.send(result);
        }

        BrowserAction::Screenshot {
            tab_id,
            selector,
            reply,
        } => {
            let result = match resolve_session(sessions, tab_id.as_deref()) {
                Ok(sid) => actions::capture_screenshot(client, sid, selector.as_deref()).await,
                Err(e) => Err(e),
            };
            let _ = reply.send(result);
        }

        BrowserAction::InjectCss {
            tab_id,
            css,
            temporary,
            reply,
        } => {
            let result = match resolve_session(sessions, tab_id.as_deref()) {
                Ok(sid) => {
                    *css_counter += 1;
                    let element_id = format!("daemon8-css-{css_counter}");
                    match actions::inject_css(client, sid, &css, &element_id).await {
                        Ok(()) => {
                            if temporary {
                                injected_styles.push(InjectedStyle {
                                    session_id: sid.to_string(),
                                    element_id: element_id.clone(),
                                });
                            }
                            Ok(element_id)
                        }
                        Err(e) => Err(e),
                    }
                }
                Err(e) => Err(e),
            };
            let _ = reply.send(result);
        }

        BrowserAction::RevertCss { tab_id, reply } => {
            let result = match resolve_session(sessions, tab_id.as_deref()) {
                Ok(sid) => {
                    let ids: Vec<String> = injected_styles
                        .iter()
                        .filter(|s| s.session_id == sid)
                        .map(|s| s.element_id.clone())
                        .collect();
                    let count = actions::revert_css(client, sid, &ids).await.unwrap_or(0);
                    injected_styles.retain(|s| s.session_id != sid);
                    Ok(count)
                }
                Err(e) => Err(e),
            };
            let _ = reply.send(result);
        }

        BrowserAction::ListTabs { reply } => {
            let mut tabs = Vec::new();
            for (sid, state) in sessions {
                let title = actions::get_tab_title(client, sid).await;
                tabs.push(TabInfo {
                    id: state.target_id.clone(),
                    url: state.url.clone(),
                    title,
                });
            }
            let _ = reply.send(Ok(tabs));
        }

        BrowserAction::GetPerformanceMetrics { tab_id, reply } => {
            let result = match resolve_session(sessions, tab_id.as_deref()) {
                Ok(sid) => actions::get_performance_metrics(client, sid).await,
                Err(e) => Err(e),
            };
            let _ = reply.send(result);
        }

        BrowserAction::GetDom {
            tab_id,
            selector,
            reply,
        } => {
            let result = match resolve_session(sessions, tab_id.as_deref()) {
                Ok(sid) => actions::get_dom(client, sid, selector.as_deref()).await,
                Err(e) => Err(e),
            };
            let _ = reply.send(result);
        }

        BrowserAction::SetViewport {
            tab_id,
            width,
            height,
            device_scale_factor,
            mobile,
            user_agent,
            reply,
        } => {
            let result = match resolve_session(sessions, tab_id.as_deref()) {
                Ok(sid) => {
                    actions::set_viewport(
                        client,
                        sid,
                        width,
                        height,
                        device_scale_factor,
                        mobile,
                        user_agent.as_deref(),
                    )
                    .await
                }
                Err(e) => Err(e),
            };
            let _ = reply.send(result);
        }

        BrowserAction::ClearViewport { tab_id, reply } => {
            let result = match resolve_session(sessions, tab_id.as_deref()) {
                Ok(sid) => actions::clear_viewport(client, sid).await,
                Err(e) => Err(e),
            };
            let _ = reply.send(result);
        }

        BrowserAction::SetNetworkConditions {
            tab_id,
            preset,
            reply,
        } => {
            let result = match resolve_session(sessions, tab_id.as_deref()) {
                Ok(sid) => actions::set_network_conditions(client, sid, &preset).await,
                Err(e) => Err(e),
            };
            let _ = reply.send(result);
        }

        BrowserAction::Navigate { tab_id, url, reply } => {
            let result = match resolve_session(sessions, tab_id.as_deref()) {
                Ok(sid) => actions::navigate(client, sid, &url).await,
                Err(e) => Err(e),
            };
            let _ = reply.send(result);
        }

        BrowserAction::StorageClear {
            tab_id,
            storage_types,
            reply,
        } => {
            let result = match resolve_session(sessions, tab_id.as_deref()) {
                Ok(sid) => actions::storage_clear(client, sid, &storage_types).await,
                Err(e) => Err(e),
            };
            let _ = reply.send(result);
        }

        BrowserAction::StorageInspect { tab_id, reply } => {
            let result = match resolve_session(sessions, tab_id.as_deref()) {
                Ok(sid) => actions::storage_inspect(client, sid).await,
                Err(e) => Err(e),
            };
            let _ = reply.send(result);
        }

        BrowserAction::StorageSet {
            tab_id,
            store_type,
            key,
            value,
            reply,
        } => {
            let result = match resolve_session(sessions, tab_id.as_deref()) {
                Ok(sid) => actions::storage_set(client, sid, &store_type, &key, &value).await,
                Err(e) => Err(e),
            };
            let _ = reply.send(result);
        }

        BrowserAction::ElementAtPoint {
            tab_id,
            x,
            y,
            reply,
        } => {
            let result = match resolve_session(sessions, tab_id.as_deref()) {
                Ok(sid) => actions::element_at_point(client, sid, x, y).await,
                Err(e) => Err(e),
            };
            let _ = reply.send(result);
        }

        BrowserAction::NewTab { url, reply } => {
            let result = client
                .send_command("Target.createTarget", json!({"url": url}), None)
                .await
                .map_err(|e| ChromeError::Cdp(format!("{e}")))
                .and_then(|v| {
                    v["targetId"]
                        .as_str()
                        .map(|s| s.to_string())
                        .ok_or_else(|| ChromeError::Cdp("no targetId in response".into()))
                });
            let _ = reply.send(result);
        }

        BrowserAction::CloseTab { tab_id, reply } => {
            let result = client
                .send_command("Target.closeTarget", json!({"targetId": tab_id}), None)
                .await
                .map_err(|e| ChromeError::Cdp(format!("{e}")))
                .map(|_| ());
            let _ = reply.send(result);
        }
    }
}

fn resolve_session<'a>(
    sessions: &'a HashMap<String, SessionState>,
    tab_id: Option<&str>,
) -> Result<&'a str> {
    if let Some(tid) = tab_id {
        // Find the session for this target ID
        for (sid, state) in sessions {
            if state.target_id == tid {
                return Ok(sid);
            }
        }
        return Err(ChromeError::Cdp(format!("no session for tab {tid}")));
    }

    sessions
        .keys()
        .next()
        .map(|s| s.as_str())
        .ok_or_else(|| {
            ChromeError::Cdp(
                "No browser tabs open. Navigate to a page or the daemon will discover tabs automatically."
                    .into(),
            )
        })
}

const CHROMIUM_BROWSERS: &[(&str, &[&str])] = &[
    #[cfg(target_os = "macos")]
    (
        "Google Chrome",
        &["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"],
    ),
    #[cfg(target_os = "macos")]
    (
        "Microsoft Edge",
        &["/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"],
    ),
    #[cfg(target_os = "macos")]
    (
        "Brave Browser",
        &["/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"],
    ),
    #[cfg(target_os = "macos")]
    ("Arc", &["/Applications/Arc.app/Contents/MacOS/Arc"]),
    #[cfg(target_os = "macos")]
    (
        "Vivaldi",
        &["/Applications/Vivaldi.app/Contents/MacOS/Vivaldi"],
    ),
    #[cfg(target_os = "macos")]
    (
        "Chromium",
        &["/Applications/Chromium.app/Contents/MacOS/Chromium"],
    ),
    #[cfg(target_os = "linux")]
    ("Google Chrome", &["google-chrome", "google-chrome-stable"]),
    #[cfg(target_os = "linux")]
    (
        "Microsoft Edge",
        &["microsoft-edge", "microsoft-edge-stable"],
    ),
    #[cfg(target_os = "linux")]
    ("Brave Browser", &["brave-browser", "brave-browser-stable"]),
    #[cfg(target_os = "linux")]
    ("Vivaldi", &["vivaldi", "vivaldi-stable"]),
    #[cfg(target_os = "linux")]
    ("Chromium", &["chromium-browser", "chromium"]),
    // System installs (Program Files / x86) come first, then per-user installs
    // under %LOCALAPPDATA%. Env-var references are expanded in resolve_binary.
    #[cfg(windows)]
    (
        "Google Chrome",
        &[
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            r"%LOCALAPPDATA%\Google\Chrome\Application\chrome.exe",
        ],
    ),
    #[cfg(windows)]
    (
        "Microsoft Edge",
        &[
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
            r"%LOCALAPPDATA%\Microsoft\Edge\Application\msedge.exe",
        ],
    ),
    #[cfg(windows)]
    (
        "Brave Browser",
        &[
            r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
            r"C:\Program Files (x86)\BraveSoftware\Brave-Browser\Application\brave.exe",
            r"%LOCALAPPDATA%\BraveSoftware\Brave-Browser\Application\brave.exe",
        ],
    ),
    #[cfg(windows)]
    (
        "Vivaldi",
        &[
            r"C:\Program Files\Vivaldi\Application\vivaldi.exe",
            r"%LOCALAPPDATA%\Vivaldi\Application\vivaldi.exe",
        ],
    ),
    #[cfg(windows)]
    (
        "Chromium",
        &[
            r"%ProgramFiles%\Chromium\Application\chrome.exe",
            r"%LOCALAPPDATA%\Chromium\Application\chrome.exe",
        ],
    ),
];

#[cfg(windows)]
fn expand_windows_env(input: &str) -> String {
    // Tiny expander for `%VAR%` tokens. Unknown or unset vars leave the token
    // in place; the caller's `path.exists()` check will reject it naturally.
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('%') {
            let name = &after[..end];
            match std::env::var(name) {
                Ok(val) => out.push_str(&val),
                Err(_) => {
                    out.push('%');
                    out.push_str(name);
                    out.push('%');
                }
            }
            rest = &after[end + 1..];
        } else {
            out.push_str(&rest[start..]);
            return out;
        }
    }
    out.push_str(rest);
    out
}

fn resolve_binary(candidate: &str) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    // Windows per-user install paths reference %LOCALAPPDATA% / %ProgramFiles%;
    // expand any %VAR% tokens before testing the path.
    #[cfg(windows)]
    let candidate_owned = expand_windows_env(candidate);
    #[cfg(windows)]
    let candidate: &str = &candidate_owned;

    let path = PathBuf::from(candidate);
    if path.is_absolute() {
        return path.exists().then_some(path);
    }
    // Relative name -- look up in PATH
    #[cfg(unix)]
    let lookup = "which";
    #[cfg(windows)]
    let lookup = "where";
    std::process::Command::new(lookup)
        .arg(candidate)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            (!s.is_empty()).then(|| PathBuf::from(s))
        })
}

fn find_chromium_binary() -> Option<std::path::PathBuf> {
    for (_name, candidates) in CHROMIUM_BROWSERS {
        for candidate in *candidates {
            if let Some(path) = resolve_binary(candidate) {
                return Some(path);
            }
        }
    }
    None
}

pub fn find_all_chromium_browsers() -> Vec<(String, std::path::PathBuf)> {
    let mut found = Vec::new();
    for (name, candidates) in CHROMIUM_BROWSERS {
        for candidate in *candidates {
            if let Some(path) = resolve_binary(candidate) {
                found.push((name.to_string(), path));
                break; // one path per browser name is enough
            }
        }
    }
    found
}

/// Read Chrome's `SingletonLock` symlink and return the PID of the process
/// currently holding the user-data-dir lock, but only if that process is still
/// alive. Chrome writes the lock as a symlink whose target is `<host>-<pid>`.
///
/// This is the source of truth for "is a Chrome instance currently using this
/// profile dir". `DevToolsActivePort` is not, because Chrome only writes that
/// file once at startup and the daemon used to delete it on transient
/// reattach failures, leaving a live Chrome with no discoverable port.
#[cfg(unix)]
fn live_chrome_pid_for_user_data_dir(user_data_dir: &std::path::Path) -> Option<u32> {
    let target = std::fs::read_link(user_data_dir.join("SingletonLock")).ok()?;
    let target_str = target.to_string_lossy();
    let pid: u32 = target_str.rsplit('-').next()?.parse().ok()?;
    pid_alive(pid).then_some(pid)
}

// macOS Sonoma (14+) gates cross-bundle signals behind the
// kTCCServiceAppManagement privacy permission. A plain `kill -0 PID` against a
// PID owned by another app bundle (Chrome) trips the "App Management" prompt
// on every daemon reinstall. `proc_pidinfo` with PROC_PIDTBSDINFO is a passive
// BSD-level query that returns zero iff the process does not exist, without
// signaling or requiring any TCC grant.
#[cfg(target_os = "macos")]
fn pid_alive(pid: u32) -> bool {
    use std::mem::MaybeUninit;
    const PROC_PIDTBSDINFO: libc::c_int = 3;
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let ret = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    ret == size
}

#[cfg(all(unix, not(target_os = "macos")))]
fn pid_alive(pid: u32) -> bool {
    // SAFETY: kill with signal 0 is the documented POSIX liveness probe and
    // does not deliver a signal. Linux has no TCC equivalent so this path is
    // unchanged from prior behavior.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

// Whether a CDP Target is a user-addressable page. Filters out Chrome's
// internal UI (settings, omnibox suggestion popup, extension service workers,
// devtools frontends, error interstitials). Those targets are real CDP
// sessions but cannot be navigated, screenshotted, or otherwise acted on by
// an agent -- they would only pollute list_tabs and confuse downstream tools.
// URL prefixes identifying CDP targets that are not user-addressable tabs.
// Kept narrow on purpose: `chrome://newtab/`, `chrome://settings/`, etc. ARE
// real user tabs and must stay attachable. Only the omnibox suggestion popup
// (a floating dropdown rendered as a separate page target), extension pages,
// internal untrusted/error surfaces, and devtools frontends are excluded.
fn is_internal_url(url: &str) -> bool {
    url.starts_with("chrome://omnibox-popup")
        || url.starts_with("chrome-extension://")
        || url.starts_with("chrome-untrusted://")
        || url.starts_with("chrome-error://")
        || url.starts_with("devtools://")
}

fn is_user_target(target: &serde_json::Value) -> bool {
    if target["type"].as_str() != Some("page") {
        return false;
    }
    !is_internal_url(target["url"].as_str().unwrap_or(""))
}

#[cfg(windows)]
fn live_chrome_pid_for_user_data_dir(user_data_dir: &std::path::Path) -> Option<u32> {
    // On Windows, SingletonLock is a regular file (not a symlink), so the PID
    // is not embedded in it. Its presence confirms Chrome owns this profile dir.
    // Find the browser process by querying all known Chromium exe names for a
    // process whose CommandLine contains this user-data-dir and is not a
    // renderer/gpu subprocess (those carry --type=).
    if !user_data_dir.join("SingletonLock").exists() {
        return None;
    }
    let dir_str = user_data_dir.to_string_lossy().to_lowercase();
    powershell_chromium_pid(&dir_str)
}

#[cfg(windows)]
fn powershell_chromium_pid(user_data_dir_lower: &str) -> Option<u32> {
    let escaped = user_data_dir_lower.replace('\'', "''");
    let script = format!(
        "Get-CimInstance Win32_Process \
         -Filter \"Name='chrome.exe' Or Name='msedge.exe' Or Name='brave.exe' \
                   Or Name='vivaldi.exe' Or Name='chromium.exe'\" | \
         Where-Object {{ $_.CommandLine -and \
             $_.CommandLine.ToLower() -like '*{escaped}*' -and \
             $_.CommandLine -notlike '*--type=*' }} | \
         Select-Object -First 1 -ExpandProperty ProcessId"
    );
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(not(any(unix, windows)))]
fn live_chrome_pid_for_user_data_dir(_user_data_dir: &std::path::Path) -> Option<u32> {
    None
}

/// Recovery: Chrome is alive but `DevToolsActivePort` is missing. Use `lsof`
/// to find any loopback TCP port the given PID is listening on. This unwedges
/// the daemon when a stale-cleanup pass deleted the port file from under a
/// running Chrome.
#[cfg(unix)]
fn find_chrome_debug_port_for_pid(pid: u32) -> Option<u16> {
    // Absolute path first because launchd-spawned processes inherit a
    // minimal PATH that may not include /usr/sbin on all macOS versions.
    let candidates = ["/usr/sbin/lsof", "/usr/bin/lsof", "lsof"];
    let pid_str = pid.to_string();
    let output = candidates.iter().find_map(|bin| {
        std::process::Command::new(bin)
            .args(["-aP", "-p", &pid_str, "-iTCP", "-sTCP:LISTEN", "-n"])
            .output()
            .ok()
            .filter(|o| o.status.success())
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().skip(1) {
        // lsof columns: COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME
        // NAME is at index 8 when command name has no whitespace (macOS
        // truncates it to a single token).
        let Some(name) = line.split_whitespace().nth(8) else {
            continue;
        };
        let Some((host, port)) = name.rsplit_once(':') else {
            continue;
        };
        if host != "127.0.0.1" && host != "localhost" && host != "[::1]" {
            continue;
        }
        if let Ok(p) = port.parse::<u16>() {
            return Some(p);
        }
    }
    None
}

#[cfg(windows)]
fn find_chrome_debug_port_for_pid(pid: u32) -> Option<u16> {
    use windows_sys::Win32::Foundation::NO_ERROR;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
        TCP_TABLE_OWNER_PID_LISTENER,
    };
    use windows_sys::Win32::Networking::WinSock::AF_INET;

    // Use Vec<u32> so the buffer has 4-byte alignment, matching the DWORD
    // fields in MIB_TCPTABLE_OWNER_PID. A Vec<u8> would be technically UB
    // when cast to a struct pointer on a strict-aliasing-aware compiler.
    let mut size: u32 = 8192;
    let mut buf = vec![0u32; (size as usize + 3) / 4];

    loop {
        let ret = unsafe {
            GetExtendedTcpTable(
                buf.as_mut_ptr() as *mut _,
                &mut size,
                0,
                AF_INET as u32,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if ret == NO_ERROR {
            break;
        }
        // size is set to the required byte count by the API on buffer-too-small.
        // If size hasn't grown past our current capacity (in bytes), something
        // unexpected happened -- bail rather than loop forever.
        if (size as usize) <= buf.len() * 4 {
            return None;
        }
        buf.resize((size as usize + 3) / 4, 0);
    }

    let table = unsafe { &*(buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID) };
    let count = table.dwNumEntries as usize;
    let rows = unsafe {
        std::slice::from_raw_parts(table.table.as_ptr() as *const MIB_TCPROW_OWNER_PID, count)
    };

    let mut candidates: Vec<u16> = rows
        .iter()
        .filter(|row| row.dwOwningPid == pid)
        .filter(|row| {
            // dwLocalAddr is in network byte order; 0x0100007f = 127.0.0.1, 0 = 0.0.0.0
            row.dwLocalAddr == 0x0100007f || row.dwLocalAddr == 0
        })
        .filter_map(|row| {
            // dwLocalPort is in network byte order
            let port = u16::from_be(row.dwLocalPort as u16);
            if port > 0 { Some(port) } else { None }
        })
        .collect();

    candidates.sort_unstable();
    candidates.dedup();
    candidates.into_iter().next()
}

#[cfg(not(any(unix, windows)))]
fn find_chrome_debug_port_for_pid(_pid: u32) -> Option<u16> {
    None
}

/// Launch or reattach to the daemon-managed Chrome instance.
///
/// Reattach order: SingletonLock+PID liveness -> DevToolsActivePort ->
/// lsof recovery. Only spawns a new Chrome process when no live instance
/// holds the user-data-dir, because Chrome's singleton handler would
/// otherwise intercept our launch args and silently open a new tab in the
/// existing window.
async fn launch_chrome(configured_path: Option<&str>) -> Result<BrowserLaunch> {
    let tmp = std::env::temp_dir();
    let user_data_dir = tmp.join("daemon8-browser");
    // Check legacy path for backward compat
    let legacy_dir = tmp.join("daemon8-chrome");
    let (user_data_dir, active_port_file) = if !user_data_dir.exists() && legacy_dir.exists() {
        (legacy_dir.clone(), legacy_dir.join("DevToolsActivePort"))
    } else {
        (
            user_data_dir.clone(),
            user_data_dir.join("DevToolsActivePort"),
        )
    };

    // Authoritative liveness check: does any process currently hold the
    // SingletonLock for this profile dir?
    let live_pid = live_chrome_pid_for_user_data_dir(&user_data_dir);

    // Cheapest reattach path: read the port file Chrome writes at startup.
    if let Ok(contents) = std::fs::read_to_string(&active_port_file) {
        if let Some(port) = contents
            .lines()
            .next()
            .and_then(|s| s.trim().parse::<u16>().ok())
        {
            let endpoint = format!("http://127.0.0.1:{port}");
            if cdp_client::discover_ws_url(&endpoint).await.is_ok() {
                tracing::info!("Reattached to existing daemon Chrome on port {port}");
                return Ok(BrowserLaunch {
                    endpoint,
                    managed: live_pid.map(|pid| ManagedBrowser {
                        pid,
                        user_data_dir: user_data_dir.clone(),
                        child: None,
                    }),
                });
            }
        }
        // Only delete the port file when we have positive evidence Chrome is
        // dead. Deleting it under a live Chrome is what caused the
        // singleton-collision new-tab loop.
        if live_pid.is_none() {
            let _ = std::fs::remove_file(&active_port_file);
        }
    }

    // Recovery path: Chrome is alive but the port file is missing or stale.
    // Find the listening port via lsof and reattach. Without this, falling
    // through to the spawn branch below would hit Chrome's singleton handler
    // and open a new tab on every call.
    if let Some(pid) = live_pid {
        if let Some(port) = find_chrome_debug_port_for_pid(pid) {
            let endpoint = format!("http://127.0.0.1:{port}");
            if cdp_client::discover_ws_url(&endpoint).await.is_ok() {
                tracing::info!(
                    "Reattached to live Chrome PID {pid} on port {port} (DevToolsActivePort recovery)"
                );
                return Ok(BrowserLaunch {
                    endpoint,
                    managed: Some(ManagedBrowser {
                        pid,
                        user_data_dir: user_data_dir.clone(),
                        child: None,
                    }),
                });
            }
        }
        return Err(ChromeError::Cdp(format!(
            "The browser is already running with user-data-dir {} (PID {pid}) but its DevTools port could not be discovered. Quit that browser instance and retry.",
            user_data_dir.display()
        )));
    }

    let chrome = if let Some(p) = configured_path
        && !p.is_empty()
        && std::path::Path::new(p).exists()
    {
        std::path::PathBuf::from(p)
    } else {
        match find_chromium_binary() {
            Some(path) => path,
            None => {
                return Err(ChromeError::Cdp(
                    "No Chromium-based browser found on this system".into(),
                ));
            }
        }
    };

    tracing::info!("Launching browser (random debug port)...");

    let mut cmd = std::process::Command::new(&chrome);
    cmd.arg("--remote-debugging-port=0")
        .arg(format!("--user-data-dir={}", user_data_dir.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-timer-throttling")
        .arg("--disable-renderer-backgrounding")
        .arg("--disable-hang-monitor")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW:
        // Chrome gets its own process group so CTRL+C from the daemon terminal
        // doesn't propagate, runs detached, and is barred from attaching a
        // console window (defensive against the phantom-console class of bugs).
        cmd.creation_flags(0x00000200 | 0x00000008 | 0x08000000);
    }

    let child = cmd
        .spawn()
        .map_err(|e| ChromeError::Cdp(format!("failed to launch browser: {e}")))?;
    let pid = child.id();
    let mut managed = Some(ManagedBrowser {
        pid,
        user_data_dir: user_data_dir.clone(),
        child: Some(child),
    });

    // Chrome writes DevToolsActivePort once the debug server is ready.
    // First line: port number. Second line: browser WebSocket path.
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if let Ok(contents) = std::fs::read_to_string(&active_port_file) {
            let mut lines = contents.lines();
            if let Some(port_str) = lines.next()
                && let Ok(port) = port_str.trim().parse::<u16>()
            {
                let endpoint = format!("http://127.0.0.1:{port}");
                if cdp_client::discover_ws_url(&endpoint).await.is_ok() {
                    tracing::info!("Browser debug instance ready on port {port}");
                    return Ok(BrowserLaunch {
                        endpoint,
                        managed: managed.take(),
                    });
                }
            }
        }
    }

    if let Some(mut browser) = managed.take() {
        browser
            .terminate("browser launch did not expose a DevTools port")
            .await;
    }

    Err(ChromeError::Cdp(
        "Browser launched but DevToolsActivePort not found after 5s".into(),
    ))
}

#[cfg(all(test, unix))]
mod pid_alive_tests {
    use super::pid_alive;

    #[test]
    fn current_process_is_alive() {
        assert!(pid_alive(std::process::id()));
    }

    #[test]
    fn impossible_pid_is_dead() {
        // PID 0 is the kernel scheduler on macOS/Linux; userland procinfo
        // lookups return zero bytes and the libc liveness probe reports dead.
        assert!(!pid_alive(0));
    }

    #[test]
    fn very_high_pid_is_dead() {
        // PIDs above the kernel's pid_max are guaranteed unused.
        assert!(!pid_alive(u32::MAX - 1));
    }
}

#[cfg(test)]
mod monitor_disconnect_tests {
    use super::{MonitorDisconnectAction, monitor_disconnect_action};

    #[test]
    fn managed_browser_waits_for_next_request_after_disconnect() {
        assert_eq!(
            monitor_disconnect_action(true),
            MonitorDisconnectAction::WaitForNextRequest
        );
    }

    #[test]
    fn external_endpoint_keeps_retrying_after_disconnect() {
        assert_eq!(
            monitor_disconnect_action(false),
            MonitorDisconnectAction::RetryEndpoint
        );
    }
}
