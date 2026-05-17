// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use daemon8_core::control::{
    AlphaEnvelope, AlphaStatus, ConnectRequest, NextAction, ScopeMode, SessionConnection,
    connect as connect_scope, status_envelope,
};
use daemon8_core::init::{InitRequest, init_project};
use daemon8_ingest::source_sync::{SourceSyncReport, SourceTrigger, SourceTriggerRequest};
use daemon8_store::{
    ActiveSessionState, DebugSessionStore, LensManager, MemoryStore, RecentScopeRecord,
    ScopeConnectFailureRecord, ScopeLedgerStore, ScopeSessionRecord, StateModel,
};
use daemon8_types::{Checkpoint, DevicePlatform, Filter, Observation};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerInfo, Tool};
use rmcp::schemars::{self, JsonSchema};
use rmcp::{RoleServer, ServerHandler, tool, tool_router};
use serde::Deserialize;
use tokio::sync::broadcast;
use tracing::Instrument;

pub mod help;
use help::FeatureGate;

const INSTRUCTIONS: &str = include_str!("../tool_descriptions/instructions.md");
static MCP_SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_mcp_session_id() -> String {
    let id = MCP_SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("mcp-{id}")
}

pub struct DeviceScreenshotResult {
    pub png_bytes: Vec<u8>,
    pub source: String,
}

/// Callback type for device screenshot capture. Receives (serial, platform) and
/// returns PNG bytes + source label. Constructed by the daemon crate with access
/// to ADB transport and xcap.
pub type DeviceScreenshotFn = Arc<
    dyn Fn(
            String,
            DevicePlatform,
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<DeviceScreenshotResult>> + Send>>
        + Send
        + Sync,
>;

#[derive(Debug)]
pub enum ChromeCommand {
    Connect { endpoint: String },
    Action(daemon8_chrome::BrowserAction),
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ObserveParams {
    #[schemars(
        description = "Filter by observation kind: log, query, http_exchange, exception, js_exception, lifecycle, state_snapshot, metric, custom, tool_call. Browser console output is 'log', browser JS errors are 'js_exception', page load events are 'lifecycle', and network requests are 'http_exchange'."
    )]
    pub kinds: Option<Vec<String>>,

    #[schemars(description = "Minimum severity threshold: trace, debug, info, warn, error")]
    pub severity_min: Option<String>,

    #[schemars(
        description = "Filter by origin pattern: 'app' or 'app:name' for applications, 'browser' or 'browser:tab_id' for browser tabs, 'device' or 'device:serial' for devices. Omit to see all origins."
    )]
    pub origins: Option<Vec<String>>,

    #[schemars(description = "Search across materialized observation text")]
    pub text_match: Option<String>,

    #[schemars(description = "Return only observations after this checkpoint id")]
    pub since_checkpoint: Option<u64>,

    #[schemars(description = "Maximum number of results to return (default 50)")]
    pub limit: Option<usize>,

    #[schemars(description = "Filter by correlation ID (exact match)")]
    pub correlation_id: Option<String>,

    #[schemars(description = "Filter by tags (all listed tags must be present)")]
    pub tags: Option<Vec<String>>,

    #[schemars(description = "Filter by service provenance, such as cargo, claude, or app.")]
    pub service: Option<Vec<String>>,

    #[schemars(description = "Filter by logical source id from .daemon8/config.md.")]
    pub source: Option<Vec<String>>,

    #[schemars(
        description = "Filter by concrete source instance, such as a file path or transcript id."
    )]
    pub source_instance: Option<Vec<String>>,

    #[schemars(
        description = "Include system/infrastructure observations (tagged '_system'). These are excluded by default to reduce noise from internal tooling."
    )]
    pub include_system: Option<bool>,
}

impl ObserveParams {
    fn has_narrowing_filter(&self) -> bool {
        self.kinds.as_ref().is_some_and(|items| !items.is_empty())
            || self
                .severity_min
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty())
            || self.origins.as_ref().is_some_and(|items| !items.is_empty())
            || self
                .text_match
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty())
            || self.since_checkpoint.is_some()
            || self
                .correlation_id
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty())
            || self.tags.as_ref().is_some_and(|items| !items.is_empty())
            || self.service.as_ref().is_some_and(|items| !items.is_empty())
            || self.source.as_ref().is_some_and(|items| !items.is_empty())
            || self
                .source_instance
                .as_ref()
                .is_some_and(|items| !items.is_empty())
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConnectParams {
    #[schemars(description = "Browser DevTools endpoint URL (default http://localhost:9222)")]
    pub endpoint: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Daemon8ConnectParams {
    #[schemars(description = "Calling provider name, e.g. codex, claude, gemini.")]
    pub provider: String,

    #[schemars(description = "Explicit project or general directory path for this MCP session.")]
    pub project_path: String,

    #[schemars(description = "Optional human-readable agent name.")]
    pub agent_name: Option<String>,

    #[schemars(description = "Optional provider transcript path for conversation-source binding.")]
    pub transcript_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Daemon8InitParams {
    #[schemars(
        description = "Explicit project directory where .daemon8/config.md should be written."
    )]
    pub project_path: String,

    #[schemars(description = "Optional project name. Defaults to the directory basename.")]
    pub name: Option<String>,

    #[schemars(description = "Replace an existing .daemon8/config.md when true.")]
    pub overwrite: Option<bool>,
}

pub use daemon8_types::DebugAction;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPreset {
    Offline,
    #[serde(rename = "slow-3g")]
    Slow3g,
    #[serde(rename = "fast-3g")]
    Fast3g,
    Restore,
}

impl NetworkPreset {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Offline => "offline",
            Self::Slow3g => "slow-3g",
            Self::Fast3g => "fast-3g",
            Self::Restore => "restore",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StoreType {
    Localstorage,
    Sessionstorage,
    Cookie,
}

impl StoreType {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Localstorage => "localstorage",
            Self::Sessionstorage => "sessionstorage",
            Self::Cookie => "cookie",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ActParams {
    pub action: DebugAction,
    #[schemars(description = "Target tab ID (omit for first/default tab)")]
    pub tab_id: Option<String>,
    #[schemars(description = "JavaScript expression to evaluate (for eval_js)")]
    pub expression: Option<String>,
    #[schemars(description = "CSS selector for element screenshot (for screenshot)")]
    pub selector: Option<String>,
    #[schemars(description = "CSS text to inject (for inject_css)")]
    pub css: Option<String>,
    #[schemars(description = "Track injected CSS for later revert (for inject_css, default true)")]
    pub temporary: Option<bool>,
    #[schemars(
        description = "Device serial for device screenshot (e.g. 'emulator-5554'). When provided with action='screenshot', captures from the device instead of the browser. Uses host window capture for emulators, ADB for physical devices."
    )]
    pub device_serial: Option<String>,
    #[schemars(
        description = "Device platform hint: 'android' or 'vega'. Used with device_serial to select the right capture method. Defaults to 'android'."
    )]
    pub device_platform: Option<String>,
    #[schemars(
        description = "Viewport width in CSS pixels (for set_viewport). iPhone 15=390, Pixel 8=412, iPad=820, desktop=1280"
    )]
    pub viewport_width: Option<u32>,
    #[schemars(
        description = "Viewport height in CSS pixels (for set_viewport). iPhone 15=844, Pixel 8=915, iPad=1180, desktop=800"
    )]
    pub viewport_height: Option<u32>,
    #[schemars(
        description = "Device pixel ratio / scale factor (for set_viewport). iPhone 15=3.0, Pixel 8=2.625, iPad=2.0, desktop=1.0"
    )]
    pub viewport_scale: Option<f64>,
    #[schemars(
        description = "Enable mobile emulation with touch events (for set_viewport). true for mobile devices, false for desktop"
    )]
    pub viewport_mobile: Option<bool>,
    #[schemars(description = "User-agent string override (for set_viewport, optional)")]
    pub viewport_ua: Option<String>,
    #[schemars(
        description = "Network preset for network_conditions. offline=no connectivity, slow-3g=400ms/780Kbps, fast-3g=150ms/1.6Mbps, restore=remove throttling"
    )]
    pub network_preset: Option<NetworkPreset>,
    #[schemars(description = "Storage type for storage_set")]
    pub store_type: Option<StoreType>,
    #[schemars(description = "Storage key to read or write (for storage_set)")]
    pub storage_key: Option<String>,
    #[schemars(description = "Storage value to write (for storage_set)")]
    pub storage_value: Option<String>,
    #[schemars(
        description = "Comma-separated storage types to clear (for storage_clear): 'cookies', 'local_storage', 'session_storage', 'indexeddb', 'cache_storage', 'service_workers', 'all'. Default: 'all'"
    )]
    pub storage_types: Option<String>,
    #[schemars(description = "X coordinate in CSS pixels (for element_at_point)")]
    pub x: Option<f64>,
    #[schemars(description = "Y coordinate in CSS pixels (for element_at_point)")]
    pub y: Option<f64>,
    #[schemars(description = "URL to navigate to (for navigate)")]
    pub url: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct IngestParams {
    #[schemars(
        description = "Your agent or application name (e.g. 'my-agent'). Used for filtering with origins=['app:name']."
    )]
    pub app: Option<String>,

    #[schemars(
        description = "Observation kind: log, query, http_exchange, exception, state_snapshot, metric, custom, js_exception, lifecycle, tool_call. Defaults to log."
    )]
    pub kind: Option<String>,

    #[schemars(
        description = "Severity: trace, debug, info, warn, error. Defaults to debug. Setting warn or error triggers a real-time alert push to all connected agent sessions."
    )]
    pub severity: Option<String>,

    #[schemars(
        description = "The observation payload (JSON object). Use a 'message' key for clean alert formatting: {\"message\": \"what happened\"}. Additional fields are preserved."
    )]
    pub data: serde_json::Value,

    #[schemars(description = "Channel name for custom kind observations.")]
    pub channel: Option<String>,

    #[schemars(description = "Correlation ID to group related observations across sources")]
    pub correlation_id: Option<String>,

    #[schemars(description = "Parent observation ID for causal chains")]
    pub parent_id: Option<u64>,

    #[schemars(
        description = "Tags for categorization (e.g. [\"reasoning\", \"high-confidence\"])"
    )]
    pub tags: Option<Vec<String>>,

    #[schemars(description = "Agent session ID that produced this observation")]
    pub session_id: Option<String>,

    #[schemars(description = "Daemon instance node ID")]
    pub node_id: Option<String>,

    #[schemars(description = "Service provenance for this observation.")]
    pub service: Option<String>,

    #[schemars(description = "Logical source id for this observation.")]
    pub source: Option<String>,

    #[schemars(description = "Concrete source instance for this observation.")]
    pub source_instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubscribeParams {
    #[schemars(
        description = "Filter by observation kind: log, query, http_exchange, exception, state_snapshot, js_exception, lifecycle, metric, custom. Omit for all kinds."
    )]
    pub kinds: Option<Vec<String>>,

    #[schemars(
        description = "Minimum severity threshold: trace, debug, info, warn, error. Default: warn (only warn and error push alerts)."
    )]
    pub severity_min: Option<String>,

    #[schemars(
        description = "Filter by origin patterns: 'app', 'app:name', 'browser', 'browser:tab_id', 'device', or 'device:serial'. Omit for all origins."
    )]
    pub origins: Option<Vec<String>>,

    #[schemars(
        description = "Search across materialized observation text. Omit for no text filtering."
    )]
    pub text_match: Option<String>,

    #[schemars(description = "Filter by correlation ID (exact match)")]
    pub correlation_id: Option<String>,

    #[schemars(description = "Filter by tags (all listed tags must be present)")]
    pub tags: Option<Vec<String>>,

    #[schemars(description = "Filter by service provenance.")]
    pub service: Option<Vec<String>>,

    #[schemars(description = "Filter by logical source id.")]
    pub source: Option<Vec<String>>,

    #[schemars(description = "Filter by concrete source instance.")]
    pub source_instance: Option<Vec<String>>,

    #[schemars(
        description = "Include system/infrastructure observations (tagged '_system'). Excluded by default."
    )]
    pub include_system: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LensParams {
    #[schemars(
        description = "Filter by observation kind: log, query, http_exchange, exception, state_snapshot, js_exception, lifecycle, metric, custom"
    )]
    pub kinds: Option<Vec<String>>,

    #[schemars(description = "Minimum severity threshold: trace, debug, info, warn, error")]
    pub severity_min: Option<String>,

    #[schemars(description = "Filter by origin pattern: 'app:name', 'browser', 'device:serial'")]
    pub origins: Option<Vec<String>>,

    #[schemars(description = "Search across materialized observation text")]
    pub text_match: Option<String>,

    #[schemars(description = "Filter by correlation ID (exact match)")]
    pub correlation_id: Option<String>,

    #[schemars(description = "Filter by tags (all listed tags must be present)")]
    pub tags: Option<Vec<String>>,

    #[schemars(description = "Filter by service provenance.")]
    pub service: Option<Vec<String>>,

    #[schemars(description = "Filter by logical source id.")]
    pub source: Option<Vec<String>>,

    #[schemars(description = "Filter by concrete source instance.")]
    pub source_instance: Option<Vec<String>>,

    #[schemars(
        description = "Include system/infrastructure observations (tagged '_system'). Excluded by default."
    )]
    pub include_system: Option<bool>,

    #[schemars(description = "Maximum observations to buffer (default 200)")]
    pub capacity: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HelpParams {
    #[schemars(
        description = "Help topic: index, debug_session, checkpoint, lens, observations, envelope. Omit for index."
    )]
    pub topic: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateCheckpointParams {
    #[schemars(
        description = "Optional human-readable note about why this checkpoint exists (e.g. \"before applying retry patch\")."
    )]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StartDebugSessionParams {
    #[schemars(description = "Project slug to scope the session to (e.g. \"daemon8\").")]
    pub project: Option<String>,
    #[schemars(description = "One-line description of what is being investigated.")]
    pub description: Option<String>,
    #[schemars(
        description = "Required. Agent identity in format :host/tool+role> (e.g. :mbp/claude+plan-agent>). Identifies who is running this investigation."
    )]
    pub agent_id: String,
    #[schemars(
        description = "Optional. Feature being investigated (e.g. 'auth', 'search'). Used by other agents to discover overlapping work."
    )]
    pub feature: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EndDebugSessionParams {
    #[schemars(
        description = "Outcome string. Defaults to \"abandoned\". Use resolve_debug_session for \"resolved\"."
    )]
    pub outcome: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResolveDebugSessionParams {
    #[schemars(description = "Required: human summary of what broke and what fixed it.")]
    pub summary: String,
    #[schemars(description = "Optional: one-sentence root cause.")]
    pub root_cause: Option<String>,
    #[schemars(description = "Optional: unified diff or short patch text.")]
    pub fix_diff: Option<String>,
    #[schemars(description = "Optional: CLI commands that mattered to the fix.")]
    pub commands_used: Option<Vec<String>>,
    #[schemars(description = "Optional: error_hash strings this fix resolves.")]
    pub related_errors: Option<Vec<String>>,
    #[schemars(description = "Optional: extra tags for retrieval.")]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListDebugSessionsParams {
    #[schemars(description = "Filter by status: active, completed, abandoned. Omit for all.")]
    pub status: Option<String>,
    #[schemars(
        description = "Optional. Filter by feature name (e.g. 'auth', 'search'). Returns only sessions investigating that feature."
    )]
    pub feature: Option<String>,
}

pub struct DaemonMcp {
    store: Arc<dyn StateModel>,
    memory_store: Option<Arc<dyn MemoryStore>>,
    debug_session_store: Option<Arc<dyn DebugSessionStore>>,
    scope_ledger_store: Option<Arc<dyn ScopeLedgerStore>>,
    active_state: ActiveSessionState,
    obs_tx: tokio::sync::mpsc::UnboundedSender<Observation>,
    chrome_tx: tokio::sync::mpsc::Sender<ChromeCommand>,
    chrome_state: tokio::sync::watch::Receiver<daemon8_chrome::ConnectionState>,
    chrome_endpoint: Arc<Mutex<Option<Arc<str>>>>,
    last_checkpoint: Mutex<Checkpoint>,
    device_screenshot_fn: Option<DeviceScreenshotFn>,
    screenshot_dir: std::path::PathBuf,
    subscription_tx: tokio::sync::watch::Sender<Option<Filter>>,
    broadcast_tx: broadcast::Sender<(Arc<Observation>, Arc<str>)>,
    source_trigger: Option<Arc<dyn SourceTrigger>>,
    lens: Arc<LensManager>,
    cancel: tokio_util::sync::CancellationToken,
    enabled_features: Vec<FeatureGate>,
    session_id: String,
    connection: Arc<Mutex<Option<SessionConnection>>>,
    tool_router: ToolRouter<Self>,
}

pub struct DaemonMcpConfig {
    pub store: Arc<dyn StateModel>,
    pub memory_store: Option<Arc<dyn MemoryStore>>,
    pub debug_session_store: Option<Arc<dyn DebugSessionStore>>,
    pub scope_ledger_store: Option<Arc<dyn ScopeLedgerStore>>,
    pub obs_tx: tokio::sync::mpsc::UnboundedSender<Observation>,
    pub chrome_tx: tokio::sync::mpsc::Sender<ChromeCommand>,
    pub chrome_state: tokio::sync::watch::Receiver<daemon8_chrome::ConnectionState>,
    pub chrome_endpoint: Arc<Mutex<Option<Arc<str>>>>,
    pub device_screenshot_fn: Option<DeviceScreenshotFn>,
    pub screenshot_dir: std::path::PathBuf,
    pub broadcast_tx: broadcast::Sender<(Arc<Observation>, Arc<str>)>,
    pub source_trigger: Option<Arc<dyn SourceTrigger>>,
    pub lens: Arc<LensManager>,
    pub cancel: tokio_util::sync::CancellationToken,
}

#[tool_router(vis = "pub")]
impl DaemonMcp {
    pub fn new(cfg: DaemonMcpConfig) -> Self {
        let mut router = Self::tool_router();
        router += Self::action_tool_router();
        router += Self::lens_tool_router();
        if cfg.debug_session_store.is_some() && cfg.memory_store.is_some() {
            router += Self::debug_session_tool_router();
        }
        let mut enabled_features = Vec::new();
        if cfg.debug_session_store.is_some() && cfg.memory_store.is_some() {
            enabled_features.push(FeatureGate::DebugSession);
        }
        let (subscription_tx, _) = tokio::sync::watch::channel::<Option<Filter>>(None);
        Self {
            store: cfg.store,
            memory_store: cfg.memory_store,
            debug_session_store: cfg.debug_session_store,
            scope_ledger_store: cfg.scope_ledger_store,
            active_state: ActiveSessionState::new(),
            obs_tx: cfg.obs_tx,
            chrome_tx: cfg.chrome_tx,
            chrome_state: cfg.chrome_state,
            chrome_endpoint: cfg.chrome_endpoint,
            last_checkpoint: Mutex::new(Checkpoint(0)),
            device_screenshot_fn: cfg.device_screenshot_fn,
            screenshot_dir: cfg.screenshot_dir,
            subscription_tx,
            broadcast_tx: cfg.broadcast_tx,
            source_trigger: cfg.source_trigger,
            lens: cfg.lens,
            cancel: cfg.cancel,
            enabled_features,
            session_id: next_mcp_session_id(),
            connection: Arc::new(Mutex::new(None)),
            tool_router: router,
        }
    }

    pub fn subscription_rx(&self) -> tokio::sync::watch::Receiver<Option<Filter>> {
        self.subscription_tx.subscribe()
    }

    /// Set this session's subscription filter directly. Equivalent to invoking
    /// the `watch_live_feed` tool — exposed for integration tests that
    /// need to verify per-session subscription scoping without driving the
    /// rmcp tool router. Gated behind the `test-util` feature so it does not
    /// appear in the public API of release builds.
    #[cfg(feature = "test-util")]
    pub fn set_subscription(&self, filter: Option<Filter>) {
        self.subscription_tx.send_replace(filter);
    }

    /// Derive a child cancellation token from this session's stored parent
    /// token. Mirrors what `on_initialized` does for the per-session push
    /// task; exposed so integration tests can prove daemon-shutdown
    /// propagates into per-session work.
    #[cfg(feature = "test-util")]
    pub fn child_cancel_token(&self) -> tokio_util::sync::CancellationToken {
        self.cancel.child_token()
    }

    #[cfg(feature = "test-util")]
    pub async fn read_live_feed_for_tests(&self) -> String {
        self.read_live_feed(Parameters(ObserveParams::default()))
            .await
    }

    #[cfg(feature = "test-util")]
    pub async fn read_live_feed_for_tests_with(&self, params: ObserveParams) -> String {
        self.read_live_feed(Parameters(params)).await
    }

    #[cfg(feature = "test-util")]
    pub async fn write_to_live_feed_for_tests(&self, params: IngestParams) -> String {
        self.write_to_live_feed(Parameters(params)).await
    }

    #[cfg(feature = "test-util")]
    pub async fn daemon8_connect_for_tests(&self, params: Daemon8ConnectParams) -> String {
        self.daemon8_connect(Parameters(params)).await
    }

    #[cfg(feature = "test-util")]
    pub async fn daemon8_init_for_tests(&self, params: Daemon8InitParams) -> String {
        self.daemon8_init(Parameters(params)).await
    }

    #[cfg(feature = "test-util")]
    pub async fn daemon8_status_for_tests(&self) -> String {
        self.daemon8_status().await
    }

    #[cfg(feature = "test-util")]
    pub async fn issue_command_for_tests(&self, params: ActParams) -> String {
        self.issue_command(Parameters(params)).await
    }

    #[cfg(feature = "test-util")]
    pub fn connect_preflight_for_tests(&self, tool: &str) -> Option<String> {
        self.connect_preflight(tool)
    }

    #[cfg(feature = "test-util")]
    pub async fn create_checkpoint_for_tests(&self, params: CreateCheckpointParams) -> String {
        self.create_checkpoint(Parameters(params)).await
    }

    #[cfg(feature = "test-util")]
    pub async fn start_debug_session_for_tests(&self, params: StartDebugSessionParams) -> String {
        self.start_debug_session(Parameters(params)).await
    }

    #[cfg(feature = "test-util")]
    pub fn help_topic_body(&self, topic: &str) -> (String, String) {
        match help::find_topic(topic, &self.enabled_features) {
            Some(t) => (t.name.to_string(), t.body.to_string()),
            None => (
                "index".to_string(),
                help::build_dynamic_index(&self.enabled_features),
            ),
        }
    }

    /// Ensure Chrome is connected, waiting up to `timeout` for the connection.
    /// Returns Ok if connected, Err with a user-facing error message if not.
    async fn ensure_chrome_connected(&self, timeout: std::time::Duration) -> Result<(), String> {
        use daemon8_chrome::ConnectionState;

        let state = *self.chrome_state.borrow();
        match state {
            ConnectionState::Connected => Ok(()),
            ConnectionState::Disconnected => {
                let endpoint = self
                    .chrome_endpoint
                    .lock()
                    .expect("chrome_endpoint mutex poisoned")
                    .as_ref()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "http://localhost:9222".to_string());
                let _ = self
                    .chrome_tx
                    .send(ChromeCommand::Connect { endpoint })
                    .await;
                let result = self.wait_for_connected(timeout).await;
                if result.is_ok() {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
                result
            }
            ConnectionState::Connecting | ConnectionState::Reconnecting => {
                self.wait_for_connected(timeout).await
            }
        }
    }

    async fn wait_for_connected(&self, timeout: std::time::Duration) -> Result<(), String> {
        use daemon8_chrome::ConnectionState;

        let mut rx = self.chrome_state.clone();
        let result = tokio::time::timeout(timeout, async {
            loop {
                if *rx.borrow_and_update() == ConnectionState::Connected {
                    return true;
                }
                if rx.changed().await.is_err() {
                    return false;
                }
            }
        })
        .await;
        match result {
            Ok(true) => Ok(()),
            _ => Err(
                "Browser connection timed out. The daemon will keep retrying in the background."
                    .into(),
            ),
        }
    }

    #[doc = include_str!("../tool_descriptions/read_live_feed.md")]
    #[tool(name = "read_live_feed")]
    async fn read_live_feed(&self, Parameters(params): Parameters<ObserveParams>) -> String {
        if self.connection_mode() == Some(ScopeMode::General) && !params.has_narrowing_filter() {
            return self.blocked(
                "narrow_filter_required",
                "general mode read_live_feed requires a narrowing filter",
                Some("add kinds, severity_min, origins, service, source, source_instance, text_match, since_checkpoint, correlation_id, or tags"),
                None,
            );
        }

        // If the caller wants browser observations, ensure Chrome is connected.
        let wants_browser = params
            .origins
            .as_ref()
            .is_some_and(|origins| origins.iter().any(|o| o.starts_with("browser")));
        if wants_browser
            && let Err(e) = self
                .ensure_chrome_connected(std::time::Duration::from_secs(10))
                .await
        {
            // Don't fail -- still query the store for whatever's there,
            // but include the connection error in the response.
            tracing::warn!("Browser not available for observation: {e}");
        }

        let kinds = params.kinds.map(Filter::kinds_from_vec);

        let severity_min = params.severity_min.and_then(|s| Filter::parse_severity(&s));

        let origins = params.origins.map(Filter::origins_from_vec);

        let since = params.since_checkpoint.map(Checkpoint);

        let filter = Filter {
            kinds,
            severity_min,
            origins,
            text_match: params.text_match,
            since,
            limit: Some(params.limit.unwrap_or(50).min(500)),
            correlation_id: params.correlation_id,
            tags: params.tags,
            service: params.service,
            source: params.source,
            source_instance: params.source_instance,
            include_system: params.include_system,
        };

        let source_report = self.trigger_project_sources().await;

        match self.store.query(&filter).await {
            Ok(slice) => {
                let mut result = serde_json::to_value(&slice).unwrap_or_default();
                if let Some(report) = source_report {
                    result["triggered_ingestion"] = source_report_value(&report);
                }

                if wants_browser {
                    let chrome_state = *self.chrome_state.borrow();
                    result["browser_state"] = serde_json::json!(format!("{chrome_state}"));
                }

                let lens_obs = self.lens.drain().await;
                if !lens_obs.is_empty() {
                    result["lens_observations"] =
                        serde_json::to_value(&lens_obs).unwrap_or_default();
                    result["lens_count"] = serde_json::json!(lens_obs.len());
                }

                let warned_since_checkpoint = filter.since.is_some()
                    && slice
                        .observations
                        .iter()
                        .any(|obs| obs.severity.level() >= daemon8_types::Severity::Warn.level());
                if warned_since_checkpoint {
                    return self.ok_with(
                        result,
                        vec!["read_live_feed", "resolve_debug_session"],
                        Some("runtime signal found; interpret the live-feed entries before recording any durable conclusion"),
                    );
                }

                self.ok(result)
            }
            Err(e) => self.err("query_failed", &e.to_string(), None, None),
        }
    }

    #[doc = include_str!("../tool_descriptions/daemon8_connect.md")]
    #[tool(name = "daemon8_connect")]
    async fn daemon8_connect(
        &self,
        Parameters(params): Parameters<Daemon8ConnectParams>,
    ) -> String {
        let provider = params.provider;
        let project_path = params.project_path;
        let agent_name = params.agent_name;
        let transcript_path = params.transcript_path;
        let outcome = connect_scope(ConnectRequest {
            session_id: self.session_id.clone(),
            provider: provider.clone(),
            project_path: PathBuf::from(&project_path),
            agent_name: agent_name.clone(),
            transcript_path: transcript_path.clone().map(PathBuf::from),
        });

        *self.connection.lock().expect("connection mutex poisoned") = outcome.connection.clone();
        self.record_connect_outcome(
            &provider,
            &project_path,
            agent_name.as_deref(),
            transcript_path.as_deref(),
            &outcome,
        )
        .await;

        let mut envelope = outcome.envelope;
        if envelope.status == AlphaStatus::Success
            && let Some(report) = self.trigger_project_sources().await
        {
            let mut data = envelope
                .data
                .take()
                .unwrap_or_else(|| serde_json::json!({}));
            data["triggered_ingestion"] = source_report_value(&report);
            envelope.data = Some(data);
        }

        envelope.render()
    }

    #[doc = include_str!("../tool_descriptions/daemon8_init.md")]
    #[tool(name = "daemon8_init")]
    async fn daemon8_init(&self, Parameters(params): Parameters<Daemon8InitParams>) -> String {
        let outcome = init_project(InitRequest {
            project_path: PathBuf::from(params.project_path),
            name: params.name,
            overwrite: params.overwrite.unwrap_or(false),
        });
        self.record_init_outcome(&outcome.envelope).await;
        outcome.envelope.render()
    }

    #[doc = include_str!("../tool_descriptions/daemon8_status.md")]
    #[tool(name = "daemon8_status")]
    async fn daemon8_status(&self) -> String {
        match self.store.summary().await {
            Ok(summary) => match serde_json::to_value(&summary) {
                Ok(mut val) => {
                    if let Some(obj) = val.as_object_mut() {
                        obj.insert(
                            "daemon_version".into(),
                            serde_json::Value::String(env!("CARGO_PKG_VERSION").to_string()),
                        );
                    }
                    val["session_id"] = serde_json::json!(self.session_id);
                    let connection = self
                        .connection
                        .lock()
                        .expect("connection mutex poisoned")
                        .clone();
                    val["connection"] = serde_json::to_value(connection).unwrap_or_default();
                    if let Some(ledger) = &self.scope_ledger_store {
                        match ledger.scope_ledger_summary(5).await {
                            Ok(summary) => {
                                val["scope_ledger"] =
                                    serde_json::to_value(summary).unwrap_or_default();
                            }
                            Err(err) => {
                                tracing::warn!(error = %err, "scope ledger summary failed");
                                val["scope_ledger_error"] = serde_json::json!(err.to_string());
                            }
                        }
                    }
                    status_envelope(self.with_session_context(val)).render()
                }
                Err(e) => self.err("serialization_failed", &e.to_string(), None, None),
            },
            Err(e) => self.err("summary_failed", &e.to_string(), None, None),
        }
    }

    #[tool(
        name = "daemon8_help",
        description = "Narrative documentation for daemon8 protocols. Pass topic='index' (or omit) for the topic list. Returns markdown."
    )]
    async fn daemon8_help(&self, Parameters(params): Parameters<HelpParams>) -> String {
        let topic = params.topic.as_deref().unwrap_or("index");
        if topic == "index" {
            let body = help::build_dynamic_index(&self.enabled_features);
            return self.ok(serde_json::json!({ "topic": "index", "body": body }));
        }
        match help::find_topic(topic, &self.enabled_features) {
            Some(t) => self.ok(serde_json::json!({ "topic": t.name, "body": t.body })),
            None => {
                let body = help::build_dynamic_index(&self.enabled_features);
                self.ok(serde_json::json!({ "topic": "index", "body": body }))
            }
        }
    }

    #[doc = include_str!("../tool_descriptions/list_connections.md")]
    #[tool(name = "list_connections")]
    async fn list_connections(&self) -> String {
        wrap_inner_result(self, &self.connections_json().await)
    }

    #[doc = include_str!("../tool_descriptions/write_to_live_feed.md")]
    #[tool(name = "write_to_live_feed")]
    async fn write_to_live_feed(&self, Parameters(params): Parameters<IngestParams>) -> String {
        let mut body = serde_json::Map::new();
        if let Some(app) = params.app {
            body.insert("app".into(), serde_json::Value::String(app));
        } else {
            body.insert("app".into(), serde_json::Value::String("agent".into()));
        }
        if let Some(kind) = params.kind {
            body.insert("kind".into(), serde_json::Value::String(kind));
        }
        if let Some(severity) = params.severity {
            body.insert("severity".into(), serde_json::Value::String(severity));
        }
        if let Some(channel) = params.channel {
            body.insert("channel".into(), serde_json::Value::String(channel));
        }
        if let Some(cid) = params.correlation_id {
            body.insert("correlation_id".into(), serde_json::Value::String(cid));
        }
        if let Some(pid) = params.parent_id {
            body.insert("parent_id".into(), serde_json::Value::Number(pid.into()));
        }
        if let Some(tags) = params.tags {
            body.insert(
                "tags".into(),
                serde_json::Value::Array(tags.into_iter().map(serde_json::Value::String).collect()),
            );
        }
        if let Some(sid) = params.session_id {
            body.insert("session_id".into(), serde_json::Value::String(sid));
        }
        if let Some(nid) = params.node_id {
            body.insert("node_id".into(), serde_json::Value::String(nid));
        }
        if let Some(service) = params.service {
            body.insert("service".into(), serde_json::Value::String(service));
        }
        if let Some(source) = params.source {
            body.insert("source".into(), serde_json::Value::String(source));
        }
        if let Some(source_instance) = params.source_instance {
            body.insert(
                "source_instance".into(),
                serde_json::Value::String(source_instance),
            );
        }
        body.insert("data".into(), params.data);

        let mut obs = daemon8_ingest::normalize::normalize(serde_json::Value::Object(body));

        // Stamp per-session debug-session and checkpoint links. Each DaemonMcp
        // instance owns its own ActiveSessionState, so concurrent MCP sessions
        // do not interfere with each other's observation stamping.
        if let Some(ref session) = self.active_state.current_session() {
            obs.debug_session_id = Some(session.id.clone());
            let slug_tag = format!("project:{}", session.project_slug);
            obs.tags = Some(match obs.tags {
                Some(mut existing) => {
                    if !existing.contains(&slug_tag) {
                        existing.push(slug_tag);
                    }
                    existing
                }
                None => vec![slug_tag],
            });
        }
        if let Some(cp) = self.active_state.current_checkpoint() {
            obs.checkpoint_id = Some(cp);
        }

        if let Err(e) = self.obs_tx.send(obs) {
            tracing::warn!(
                origin = ?e.0.origin,
                kind = %e.0.kind.tag(),
                severity = %e.0.severity,
                "MCP ingest failed: observation channel closed"
            );
            return self.err(
                "daemon_shutting_down",
                "Daemon is shutting down.",
                None,
                None,
            );
        }

        self.ok(serde_json::json!({"ok": true}))
    }

    #[doc = include_str!("../tool_descriptions/watch_live_feed.md")]
    #[tool(name = "watch_live_feed")]
    async fn watch_live_feed(&self, Parameters(params): Parameters<SubscribeParams>) -> String {
        let kinds = params.kinds.map(Filter::kinds_from_vec);

        let severity_min = params.severity_min.and_then(|s| Filter::parse_severity(&s));

        let origins = params.origins.map(Filter::origins_from_vec);

        let filter = Filter {
            kinds,
            severity_min,
            origins,
            text_match: params.text_match,
            since: None,
            limit: None,
            correlation_id: params.correlation_id,
            tags: params.tags,
            service: params.service,
            source: params.source,
            source_instance: params.source_instance,
            include_system: params.include_system,
        };

        let is_default = filter.kinds.is_none()
            && filter.severity_min.is_none()
            && filter.origins.is_none()
            && filter.text_match.is_none()
            && filter.correlation_id.is_none()
            && filter.tags.is_none()
            && filter.service.is_none()
            && filter.source.is_none()
            && filter.source_instance.is_none()
            && filter.include_system.is_none();

        if is_default {
            self.subscription_tx.send_replace(None);
            self.ok(serde_json::json!({
                "subscribed": true,
                "filter": "default (severity >= warn)"
            }))
        } else {
            self.subscription_tx.send_replace(Some(filter));
            self.ok(serde_json::json!({
                "subscribed": true,
                "filter": "custom"
            }))
        }
    }
}

#[tool_router(router = action_tool_router, vis = "pub")]
impl DaemonMcp {
    #[doc = include_str!("../tool_descriptions/connect_browser.md")]
    #[tool(name = "connect_browser")]
    async fn connect_browser(&self, Parameters(params): Parameters<ConnectParams>) -> String {
        self.connect_browser_inner(params).await
    }

    #[doc = include_str!("../tool_descriptions/issue_command.md")]
    #[tool(name = "issue_command")]
    async fn issue_command(&self, Parameters(params): Parameters<ActParams>) -> String {
        let raw = self.issue_command_inner(params).await;
        wrap_inner_result(self, &raw)
    }
}

#[tool_router(router = lens_tool_router, vis = "pub")]
impl DaemonMcp {
    #[doc = include_str!("../tool_descriptions/set_lens.md")]
    #[tool(name = "set_lens")]
    async fn set_lens(&self, Parameters(params): Parameters<LensParams>) -> String {
        let filter = Filter {
            kinds: params.kinds.map(Filter::kinds_from_vec),
            severity_min: params.severity_min.and_then(|s| Filter::parse_severity(&s)),
            origins: params.origins.map(Filter::origins_from_vec),
            text_match: params.text_match,
            since: None,
            limit: None,
            correlation_id: params.correlation_id,
            tags: params.tags,
            service: params.service,
            source: params.source,
            source_instance: params.source_instance,
            include_system: params.include_system,
        };

        let capacity = params.capacity.unwrap_or(200).min(1000);
        self.lens.set_with_capacity(filter, capacity).await;

        let status = self.lens.status().await;
        self.ok(serde_json::to_value(&status).unwrap_or(serde_json::Value::Null))
    }

    #[doc = include_str!("../tool_descriptions/clear_lens.md")]
    #[tool(name = "clear_lens")]
    async fn clear_lens(&self) -> String {
        self.lens.clear().await;
        self.ok(serde_json::json!({"cleared": true}))
    }

    #[doc = include_str!("../tool_descriptions/lens_status.md")]
    #[tool(name = "lens_status")]
    async fn lens_status(&self) -> String {
        let status = self.lens.status().await;
        self.ok(serde_json::to_value(&status).unwrap_or(serde_json::Value::Null))
    }
}

/// Wrap a JSON string from an inner helper (no envelope) into the standard
/// envelope shape. Pure best-effort: malformed JSON falls back to a string
/// payload so the LLM still gets *something* readable instead of an opaque
/// error.
fn wrap_inner_result(daemon: &DaemonMcp, raw: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(v) => {
            // If the inner function already produced an alpha envelope
            // (i.e. `error_json(...)` from inside the inner), pass it through.
            // Otherwise it's a raw success payload to wrap as `data`.
            if v.get("status").is_some() && v.get("code").is_some() {
                return serde_json::to_string_pretty(&v)
                    .unwrap_or_else(|_| daemon.err("serialization_failed", raw, None, None));
            }
            if let Some(err_obj) = v.get("error").and_then(|e| e.as_object()) {
                let code = err_obj
                    .get("code")
                    .and_then(|c| c.as_str())
                    .unwrap_or("internal_error");
                let message = err_obj
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("(no message)");
                return daemon.err(code, message, None, None);
            }
            daemon.ok(v)
        }
        Err(_) => daemon.ok(serde_json::json!({"raw": raw})),
    }
}

/// Validate agent_id against the `:host/tool+role>` convention.
/// All lowercase, bounded by `:` prefix and `>` suffix, `/` separates host from tool,
/// `+` separates tool from role. Max 64 chars total.
fn validate_agent_id(id: &str) -> Result<(), String> {
    if id.len() > 64 {
        return Err("agent_id must be at most 64 characters".into());
    }
    let body = id
        .strip_prefix(':')
        .and_then(|s| s.strip_suffix('>'))
        .ok_or("agent_id must start with ':' and end with '>'")?;
    let (host_rest, role) = body
        .split_once('+')
        .ok_or("agent_id must contain '+' separating tool from role")?;
    let (host, tool) = host_rest
        .split_once('/')
        .ok_or("agent_id must contain '/' separating host from tool")?;
    if host.is_empty() || tool.is_empty() || role.is_empty() {
        return Err("host, tool, and role must be non-empty".into());
    }
    let valid_segment = |s: &str| {
        s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    if !valid_segment(host) || !valid_segment(tool) || !valid_segment(role) {
        return Err("agent_id segments must be lowercase alphanumeric with hyphens only".into());
    }
    Ok(())
}

#[tool_router(router = debug_session_tool_router, vis = "pub")]
impl DaemonMcp {
    #[doc = include_str!("../tool_descriptions/create_checkpoint.md")]
    #[tool(name = "create_checkpoint")]
    async fn create_checkpoint(
        &self,
        Parameters(params): Parameters<CreateCheckpointParams>,
    ) -> String {
        let active = match self.active_state.current_session() {
            Some(s) => s,
            None => {
                return self.err(
                    "no_active_debug_session",
                    "create_checkpoint requires an active debug session",
                    Some("call start_debug_session first"),
                    Some("start_debug_session"),
                );
            }
        };
        let ds_store = match &self.debug_session_store {
            Some(s) => s,
            None => {
                return self.err(
                    "debug_session_unavailable",
                    "debug_session store not available",
                    None,
                    None,
                );
            }
        };

        let source_report = self.trigger_project_sources().await;
        if let Some(report) = &source_report
            && report.has_failures()
        {
            let mut data = self.session_context();
            data["triggered_ingestion"] = source_report_value(report);
            return AlphaEnvelope::non_success(
                AlphaStatus::Blocked,
                "checkpoint_source_refresh_failed",
                "checkpoint source refresh failed",
                "configured project sources must refresh before daemon8 can create a checkpoint",
            )
            .with_data(data)
            .with_next_action(NextAction::new(
                "read_live_feed",
                "inspect source refresh failures before creating a checkpoint",
                serde_json::json!({}),
            ))
            .render();
        }

        let seq = self.store.checkpoint().await;
        let now = current_ns();
        let cp = daemon8_store::DebugCheckpoint {
            id: None,
            debug_session_id: active.id.to_string(),
            description: params.description,
            created_at: now,
            seq_at_creation: seq.0,
        };
        let cp_id = match ds_store.create_checkpoint(cp).await {
            Ok(id) => id,
            Err(e) => return self.err("create_checkpoint_failed", &e.to_string(), None, None),
        };
        self.active_state
            .set_checkpoint(Some(Arc::from(cp_id.as_str())));
        active.touch(now);
        *self
            .last_checkpoint
            .lock()
            .expect("last_checkpoint mutex poisoned") = seq;
        let mut data = serde_json::json!({
            "checkpoint_id": cp_id,
            "debug_session_id": active.id.as_ref(),
            "seq_at_creation": seq.0,
            "created_at": now
        });
        if let Some(report) = source_report {
            data["triggered_ingestion"] = source_report_value(&report);
        }
        self.ok_with_code(
            "checkpoint_created",
            "checkpoint created",
            data,
            vec!["read_live_feed"],
            Some("checkpoint set; read_live_feed(since_checkpoint=...) returns new entries since this point"),
        )
    }

    #[doc = include_str!("../tool_descriptions/start_debug_session.md")]
    #[tool(name = "start_debug_session")]
    async fn start_debug_session(
        &self,
        Parameters(params): Parameters<StartDebugSessionParams>,
    ) -> String {
        let ds_store = match &self.debug_session_store {
            Some(s) => s,
            None => {
                return self.err(
                    "debug_session_unavailable",
                    "debug_session store not available",
                    Some("start the daemon with debug session storage enabled"),
                    None,
                );
            }
        };
        if let Some(existing) = self.active_state.current_session() {
            return self.err(
                "already_active_debug_session",
                &format!("session {} is already active", existing.id),
                Some(
                    "call end_debug_session(outcome=\"abandoned\") or resolve_debug_session first",
                ),
                Some("end_debug_session"),
            );
        }
        if let Err(msg) = validate_agent_id(&params.agent_id) {
            return self.err(
                "invalid_agent_id",
                &msg,
                Some("agent_id format: :host/tool+role> (e.g. :mbp/claude+plan-agent>)"),
                None,
            );
        }
        let now = current_ns();
        let session = daemon8_store::DebugSession {
            id: None,
            started_at: now,
            ended_at: None,
            last_activity: now,
            project_slug: params.project.unwrap_or_else(|| "unknown".into()),
            description: params.description,
            status: daemon8_types::DebugSessionStatus::Active,
            outcome: None,
            summary_memory_id: None,
            agent_id: params.agent_id.clone(),
            feature: params.feature.clone(),
        };
        match ds_store.start_debug_session(session.clone()).await {
            Ok(id) => {
                self.active_state
                    .set_session(Some(daemon8_store::ActiveDebugSession {
                        id: Arc::from(id.as_str()),
                        project_slug: Arc::from(session.project_slug.as_str()),
                        started_at_ns: now,
                        last_activity_ns: Arc::new(AtomicU64::new(now)),
                        agent_id: Arc::from(params.agent_id.as_str()),
                        feature: params.feature.as_deref().map(Arc::from),
                    }));
                self.ok_with_code(
                    "debug_session_started",
                    "debug session started",
                    serde_json::json!({
                        "debug_session_id": id,
                        "started_at": now,
                    }),
                    vec!["create_checkpoint"],
                    Some("debug session opened; create a checkpoint before the action you want to verify"),
                )
            }
            Err(e) => self.err("start_debug_session_failed", &e.to_string(), None, None),
        }
    }

    #[doc = include_str!("../tool_descriptions/end_debug_session.md")]
    #[tool(name = "end_debug_session")]
    async fn end_debug_session(
        &self,
        Parameters(params): Parameters<EndDebugSessionParams>,
    ) -> String {
        end_or_resolve_inner(
            self,
            EndIntent::Abandon {
                outcome_str: params.outcome,
            },
        )
        .await
    }

    #[doc = include_str!("../tool_descriptions/resolve_debug_session.md")]
    #[tool(name = "resolve_debug_session")]
    async fn resolve_debug_session(
        &self,
        Parameters(params): Parameters<ResolveDebugSessionParams>,
    ) -> String {
        end_or_resolve_inner(self, EndIntent::Resolve(params)).await
    }

    #[doc = include_str!("../tool_descriptions/list_debug_sessions.md")]
    #[tool(name = "list_debug_sessions")]
    async fn list_debug_sessions(
        &self,
        Parameters(params): Parameters<ListDebugSessionsParams>,
    ) -> String {
        let ds_store = match &self.debug_session_store {
            Some(s) => s,
            None => {
                return self.err(
                    "debug_session_unavailable",
                    "debug_session store not available",
                    Some("start the daemon with debug session storage enabled"),
                    None,
                );
            }
        };
        let status = match params.status.as_deref() {
            Some(s) => match s.parse::<daemon8_types::DebugSessionStatus>() {
                Ok(v) => Some(v),
                Err(e) => return self.err("bad_status", &e, None, None),
            },
            None => None,
        };
        match ds_store.list_debug_sessions(status).await {
            Ok(mut sessions) => {
                if let Some(ref feat) = params.feature {
                    sessions.retain(|s| s.feature.as_deref() == Some(feat.as_str()));
                }
                self.ok_code(
                    "debug_sessions_listed",
                    "debug sessions listed",
                    serde_json::json!({
                        "count": sessions.len(),
                        "sessions": sessions,
                    }),
                )
            }
            Err(e) => self.err("list_debug_sessions_failed", &e.to_string(), None, None),
        }
    }
}

enum EndIntent {
    Abandon { outcome_str: Option<String> },
    Resolve(ResolveDebugSessionParams),
}

async fn end_or_resolve_inner(daemon: &DaemonMcp, intent: EndIntent) -> String {
    let ds_store = match &daemon.debug_session_store {
        Some(s) => s,
        None => {
            return daemon.err(
                "debug_session_unavailable",
                "debug_session store not available",
                None,
                None,
            );
        }
    };
    let mem_store = match &daemon.memory_store {
        Some(s) => s,
        None => {
            return daemon.err(
                "memory_store_unavailable",
                "memory store not available",
                None,
                None,
            );
        }
    };
    let active = match daemon.active_state.current_session() {
        Some(s) => s,
        None => {
            return daemon.err(
                "no_active_debug_session",
                "no active debug session to end/resolve",
                Some("call start_debug_session first"),
                Some("start_debug_session"),
            );
        }
    };
    let now = current_ns();

    let checkpoints = ds_store
        .list_checkpoints(active.id.as_ref())
        .await
        .unwrap_or_default();
    let source_observations: Vec<u64> = Vec::new();

    let (outcome, summary_text, tags, data_blob) = match intent {
        EndIntent::Abandon { outcome_str } => {
            let outcome = match outcome_str.as_deref() {
                None => daemon8_types::DebugSessionOutcome::Abandoned,
                Some(s) => match s.parse::<daemon8_types::DebugSessionOutcome>() {
                    Ok(daemon8_types::DebugSessionOutcome::Resolved) => {
                        return daemon.err(
                            "bad_outcome",
                            "end_debug_session cannot resolve a session",
                            Some("use resolve_debug_session when you have a captured fix"),
                            Some("resolve_debug_session"),
                        );
                    }
                    Ok(outcome) => outcome,
                    Err(e) => {
                        return daemon.err(
                            "bad_outcome",
                            &e,
                            Some("allowed outcomes are abandoned and in_progress"),
                            None,
                        );
                    }
                },
            };
            let summary = format!(
                "Debug session abandoned. Project: {}, started_at_ns: {}, checkpoints: {}.",
                active.project_slug,
                active.started_at_ns,
                checkpoints.len()
            );
            let tags = vec![
                "kind:debug_session_summary".to_string(),
                format!("project:{}", active.project_slug),
                format!("outcome:{}", outcome),
            ];
            (outcome, summary, tags, None)
        }
        EndIntent::Resolve(params) => {
            let mut tags = vec![
                "kind:debug_session_summary".to_string(),
                format!("project:{}", active.project_slug),
                "outcome:resolved".to_string(),
            ];
            if let Some(extra) = &params.tags {
                tags.extend(extra.iter().cloned());
            }
            // Add error_hash tags so typed session/error lookup can find
            // the resolution alongside the ErrorSignature memory.
            if let Some(errs) = &params.related_errors {
                tags.extend(errs.iter().map(|h| format!("hash:{h}")));
            }
            let mut data = serde_json::Map::new();
            if let Some(rc) = &params.root_cause {
                data.insert("root_cause".into(), serde_json::json!(rc));
            }
            if let Some(diff) = &params.fix_diff {
                data.insert("fix_diff".into(), serde_json::json!(diff));
            }
            if let Some(cmds) = &params.commands_used {
                data.insert("commands_used".into(), serde_json::json!(cmds));
            }
            if let Some(errs) = &params.related_errors {
                data.insert("related_errors".into(), serde_json::json!(errs));
            }
            data.insert(
                "checkpoint_count".into(),
                serde_json::json!(checkpoints.len()),
            );
            data.insert(
                "checkpoint_seq_refs".into(),
                serde_json::json!(
                    checkpoints
                        .iter()
                        .map(|cp| cp.seq_at_creation)
                        .collect::<Vec<_>>()
                ),
            );
            data.insert(
                "started_at_ns".into(),
                serde_json::json!(active.started_at_ns),
            );
            data.insert("ended_at_ns".into(), serde_json::json!(now));
            (
                daemon8_types::DebugSessionOutcome::Resolved,
                params.summary,
                tags,
                Some(serde_json::Value::Object(data)),
            )
        }
    };

    let mem = daemon8_store::Memory {
        id: None,
        created_at: now,
        updated_at: now,
        kind: daemon8_types::MemoryKind::SessionSummary,
        content: summary_text,
        source_observations,
        tags,
        project_slug: active.project_slug.to_string(),
        session_id: Some(active.id.to_string()),
        confidence: 1.0,
        data: data_blob,
    };

    let summary_memory_id = match mem_store.save_memory(mem).await {
        Ok(id) => id,
        Err(e) => {
            return daemon.err("session_summary_save_failed", &e.to_string(), None, None);
        }
    };

    let status = match outcome {
        daemon8_types::DebugSessionOutcome::Resolved => {
            daemon8_types::DebugSessionStatus::Completed
        }
        daemon8_types::DebugSessionOutcome::Abandoned
        | daemon8_types::DebugSessionOutcome::InProgress => {
            daemon8_types::DebugSessionStatus::Abandoned
        }
    };

    if let Err(e) = ds_store
        .end_debug_session(
            active.id.as_ref(),
            status,
            Some(outcome),
            Some(summary_memory_id.clone()),
            now,
        )
        .await
    {
        return daemon.err(
            "end_debug_session_db_update_failed",
            &e.to_string(),
            None,
            None,
        );
    }

    daemon.active_state.clear();

    let evidence_ref = serde_json::json!({
        "kind": "session_summary",
        "id": summary_memory_id.clone(),
    });
    let (code, message) = match outcome {
        daemon8_types::DebugSessionOutcome::Resolved => {
            ("debug_session_resolved", "debug session resolved")
        }
        daemon8_types::DebugSessionOutcome::Abandoned
        | daemon8_types::DebugSessionOutcome::InProgress => {
            ("debug_session_ended", "debug session ended")
        }
    };
    let next_actions = vec!["start_debug_session", "list_debug_sessions"];
    let hint = "session closed; start_debug_session for the next investigation";

    daemon.ok_with_code(
        code,
        message,
        serde_json::json!({
            "debug_session_id": active.id.as_ref(),
            "summary_memory_id": summary_memory_id,
            "project_slug": active.project_slug.as_ref(),
            "evidence_ref": evidence_ref,
            "checkpoint_count": checkpoints.len(),
        }),
        next_actions,
        Some(hint),
    )
}

fn current_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

fn source_report_value(report: &SourceSyncReport) -> serde_json::Value {
    serde_json::to_value(report).unwrap_or_else(|_| serde_json::json!({}))
}

fn scope_session_record(
    connection: &SessionConnection,
    envelope: &AlphaEnvelope,
    now: u64,
) -> ScopeSessionRecord {
    ScopeSessionRecord {
        id: None,
        session_id: connection.session_id.clone(),
        provider: connection.provider.clone(),
        agent_name: connection.agent_name.clone(),
        mode: connection.mode.as_str().into(),
        requested_path: connection.requested_path.clone(),
        scope_root: connection.scope_root.clone(),
        transcript_path: connection.transcript_path.clone(),
        project_name: envelope_data_str(envelope, "project_name"),
        source_count: envelope_data_u64(envelope, "source_count"),
        connected_at: now,
        last_seen_at: now,
    }
}

fn scope_failure_record(
    session_id: &str,
    provider: &str,
    requested_path: &str,
    agent_name: Option<&str>,
    transcript_path: Option<&str>,
    envelope: &AlphaEnvelope,
    now: u64,
) -> ScopeConnectFailureRecord {
    ScopeConnectFailureRecord {
        id: None,
        session_id: session_id.into(),
        provider: provider.into(),
        agent_name: agent_name.map(Into::into),
        requested_path: requested_path.into(),
        scope_root: envelope_data_str(envelope, "scope_root"),
        transcript_path: transcript_path.map(Into::into),
        mode: envelope_data_str(envelope, "mode")
            .unwrap_or_else(|| ScopeMode::Invalid.as_str().into()),
        status: alpha_status_str(envelope.status).into(),
        code: envelope.code.clone(),
        message: envelope.message.clone(),
        why: envelope.why.clone(),
        attempt_count: 1,
        first_seen_at: now,
        last_seen_at: now,
    }
}

fn envelope_data_str(envelope: &AlphaEnvelope, key: &str) -> Option<String> {
    envelope
        .data
        .as_ref()
        .and_then(|data| data.get(key))
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
}

fn envelope_data_u64(envelope: &AlphaEnvelope, key: &str) -> Option<u64> {
    envelope
        .data
        .as_ref()
        .and_then(|data| data.get(key))
        .and_then(|value| value.as_u64())
}

fn alpha_status_str(status: AlphaStatus) -> &'static str {
    match status {
        AlphaStatus::Success => "success",
        AlphaStatus::Error => "error",
        AlphaStatus::ConnectRequired => "connect_required",
        AlphaStatus::SetupRequired => "setup_required",
        AlphaStatus::Blocked => "blocked",
    }
}

impl DaemonMcp {
    pub(crate) fn ok(&self, value: serde_json::Value) -> String {
        self.ok_code("ok", "ok", value)
    }

    pub(crate) fn ok_code(&self, code: &str, message: &str, value: serde_json::Value) -> String {
        AlphaEnvelope::success(code, message, self.with_session_context(value)).render()
    }

    pub(crate) fn ok_with(
        &self,
        value: serde_json::Value,
        next_actions: Vec<&str>,
        hint: Option<&str>,
    ) -> String {
        self.ok_with_code("ok", "ok", value, next_actions, hint)
    }

    pub(crate) fn ok_with_code(
        &self,
        code: &str,
        message: &str,
        value: serde_json::Value,
        next_actions: Vec<&str>,
        hint: Option<&str>,
    ) -> String {
        let mut envelope = AlphaEnvelope::success(code, message, self.with_session_context(value));
        for action in next_actions {
            envelope = envelope.with_next_action(NextAction::new(
                action,
                "recommended next tool",
                serde_json::json!({}),
            ));
        }
        if let Some(hint) = hint {
            envelope = envelope.with_hint(hint);
        }
        envelope.render()
    }

    pub(crate) fn err(
        &self,
        code: &str,
        message: &str,
        hint: Option<&str>,
        next_tool: Option<&str>,
    ) -> String {
        let mut envelope =
            AlphaEnvelope::non_success(AlphaStatus::Error, code, message, hint.unwrap_or(message))
                .with_data(self.session_context());
        if let Some(tool) = next_tool {
            envelope = envelope.with_next_action(NextAction::new(
                tool,
                "call this tool to continue",
                serde_json::json!({}),
            ));
        }
        envelope.render()
    }

    fn with_session_context(&self, mut value: serde_json::Value) -> serde_json::Value {
        if let Some(object) = value.as_object_mut() {
            let context = self.session_context();
            if let Some(context) = context.as_object() {
                for (key, value) in context {
                    object.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
        }
        value
    }

    fn session_context(&self) -> serde_json::Value {
        let mut data = serde_json::json!({
            "session_id": self.session_id,
        });

        let connection = self
            .connection
            .lock()
            .expect("connection mutex poisoned")
            .clone();
        if let Some(connection) = connection {
            data["connection"] = serde_json::to_value(connection).unwrap_or_default();
        }

        if let Some(s) = self.active_state.current_session() {
            data["active_debug_session"] = serde_json::json!({
                "id": s.id.to_string(),
                "project_slug": s.project_slug.to_string(),
                "started_at_ns": s.started_at_ns,
            });
        }

        data
    }

    async fn record_connect_outcome(
        &self,
        provider: &str,
        requested_path: &str,
        agent_name: Option<&str>,
        transcript_path: Option<&str>,
        outcome: &daemon8_core::control::ConnectOutcome,
    ) {
        let Some(ledger) = &self.scope_ledger_store else {
            return;
        };
        let now = current_ns();

        let result = match &outcome.connection {
            Some(connection) => {
                ledger
                    .record_connect_success(scope_session_record(
                        connection,
                        &outcome.envelope,
                        now,
                    ))
                    .await
            }
            None => {
                ledger
                    .record_connect_failure(scope_failure_record(
                        &self.session_id,
                        provider,
                        requested_path,
                        agent_name,
                        transcript_path,
                        &outcome.envelope,
                        now,
                    ))
                    .await
            }
        };

        if let Err(err) = result {
            tracing::warn!(error = %err, "scope ledger connect recording failed");
        }
    }

    async fn record_init_outcome(&self, envelope: &AlphaEnvelope) {
        if envelope.status != AlphaStatus::Success {
            return;
        }
        let Some(ledger) = &self.scope_ledger_store else {
            return;
        };
        let Some(scope_root) = envelope_data_str(envelope, "scope_root") else {
            return;
        };
        let now = current_ns();
        let record = RecentScopeRecord {
            id: None,
            mode: envelope_data_str(envelope, "mode").unwrap_or_else(|| "project".into()),
            requested_path: envelope_data_str(envelope, "requested_path")
                .unwrap_or_else(|| scope_root.clone()),
            scope_root,
            provider: None,
            agent_name: None,
            session_id: Some(self.session_id.clone()),
            project_name: envelope_data_str(envelope, "project_name"),
            source_count: envelope_data_u64(envelope, "source_count"),
            first_seen_at: now,
            last_seen_at: now,
        };

        if let Err(err) = ledger.record_recent_scope(record).await {
            tracing::warn!(error = %err, "scope ledger init recording failed");
        }
    }

    fn has_connection(&self) -> bool {
        self.connection
            .lock()
            .expect("connection mutex poisoned")
            .is_some()
    }

    fn connection_mode(&self) -> Option<ScopeMode> {
        self.connection
            .lock()
            .expect("connection mutex poisoned")
            .as_ref()
            .map(|connection| connection.mode)
    }

    async fn trigger_project_sources(&self) -> Option<SourceSyncReport> {
        let trigger = self.source_trigger.as_ref()?;
        let scope_root = {
            let connection = self.connection.lock().expect("connection mutex poisoned");
            let connection = connection.as_ref()?;
            if connection.mode != ScopeMode::Project {
                return None;
            }
            PathBuf::from(connection.scope_root.as_ref()?)
        };

        Some(
            trigger
                .trigger_sources(SourceTriggerRequest { scope_root })
                .await,
        )
    }

    fn connect_preflight(&self, tool: &str) -> Option<String> {
        self.tool_router.get(tool)?;
        let policy = tool_policy(tool)?;
        match policy {
            ToolPolicy::PreConnectAllowed => None,
            ToolPolicy::GeneralSafe if self.has_connection() => None,
            ToolPolicy::ProjectOnly if self.connection_mode() == Some(ScopeMode::Project) => None,
            ToolPolicy::ProjectOnly if self.connection_mode() == Some(ScopeMode::General) => Some(
                AlphaEnvelope::non_success(
                    AlphaStatus::Blocked,
                    "project_required",
                    "project scope required",
                    "reconnect with daemon8_connect using a project path before using this tool",
                )
                .with_data(self.session_context())
                .with_next_action(NextAction::new(
                    "daemon8_connect",
                    "bind this MCP session to a project scope",
                    serde_json::json!({}),
                ))
                .render(),
            ),
            ToolPolicy::GeneralSafe | ToolPolicy::ProjectOnly => Some(
                AlphaEnvelope::non_success(
                    AlphaStatus::ConnectRequired,
                    "connect_required",
                    "daemon8_connect required",
                    "call daemon8_connect before using runtime tools in this MCP session",
                )
                .with_data(self.session_context())
                .with_next_action(NextAction::new(
                    "daemon8_connect",
                    "bind this MCP session to a project or general scope",
                    serde_json::json!({}),
                ))
                .render(),
            ),
        }
    }

    fn blocked(
        &self,
        code: &str,
        message: &str,
        hint: Option<&str>,
        next_tool: Option<&str>,
    ) -> String {
        let mut envelope = AlphaEnvelope::non_success(
            AlphaStatus::Blocked,
            code,
            message,
            hint.unwrap_or(message),
        )
        .with_data(self.session_context());
        if let Some(tool) = next_tool {
            envelope = envelope.with_next_action(NextAction::new(
                tool,
                "call this tool to continue",
                serde_json::json!({}),
            ));
        }
        envelope.render()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPolicy {
    PreConnectAllowed,
    GeneralSafe,
    ProjectOnly,
}

pub const TOOL_POLICY_TABLE: &[(&str, ToolPolicy)] = &[
    ("daemon8_connect", ToolPolicy::PreConnectAllowed),
    ("daemon8_init", ToolPolicy::PreConnectAllowed),
    ("daemon8_status", ToolPolicy::PreConnectAllowed),
    ("read_live_feed", ToolPolicy::GeneralSafe),
    ("list_connections", ToolPolicy::GeneralSafe),
    ("write_to_live_feed", ToolPolicy::GeneralSafe),
    ("watch_live_feed", ToolPolicy::GeneralSafe),
    ("issue_command", ToolPolicy::GeneralSafe),
    ("connect_browser", ToolPolicy::GeneralSafe),
    ("set_lens", ToolPolicy::GeneralSafe),
    ("clear_lens", ToolPolicy::GeneralSafe),
    ("lens_status", ToolPolicy::GeneralSafe),
    ("daemon8_help", ToolPolicy::GeneralSafe),
    ("list_debug_sessions", ToolPolicy::GeneralSafe),
    ("start_debug_session", ToolPolicy::ProjectOnly),
    ("create_checkpoint", ToolPolicy::ProjectOnly),
    ("resolve_debug_session", ToolPolicy::ProjectOnly),
    ("end_debug_session", ToolPolicy::ProjectOnly),
];

pub fn tool_policy(tool: &str) -> Option<ToolPolicy> {
    TOOL_POLICY_TABLE
        .iter()
        .find_map(|(name, policy)| (*name == tool).then_some(*policy))
}

// Command handler implementations (inner methods, not registered with tool_router).
impl DaemonMcp {
    async fn connect_browser_inner(&self, params: ConnectParams) -> String {
        let endpoint = params.endpoint.clone();
        *self
            .chrome_endpoint
            .lock()
            .expect("chrome_endpoint mutex poisoned") = Some(Arc::from(endpoint.as_str()));
        match self
            .chrome_tx
            .send(ChromeCommand::Connect {
                endpoint: params.endpoint,
            })
            .await
        {
            Ok(()) => {
                tracing::info!(endpoint = %endpoint, "MCP requested browser connection");
                self.ok(serde_json::json!({
                    "status": "connecting",
                    "endpoint": endpoint,
                }))
            }
            Err(_) => {
                tracing::warn!(endpoint = %endpoint, "browser connect command rejected: daemon shutting down");
                self.err(
                    "daemon_shutting_down",
                    "Daemon is shutting down.",
                    None,
                    None,
                )
            }
        }
    }

    async fn issue_command_inner(&self, params: ActParams) -> String {
        use daemon8_chrome::BrowserAction;

        // Device screenshot: bypass Chrome entirely
        if params.action == DebugAction::Screenshot && params.device_serial.is_some() {
            return self.handle_device_screenshot(&params).await;
        }

        if let Some(error) = validate_action_params(&params) {
            return error;
        }

        if let Err(e) = self
            .ensure_chrome_connected(std::time::Duration::from_secs(10))
            .await
        {
            return error_json("browser_not_connected", &e);
        }

        let (reply_tx, reply_rx) =
            tokio::sync::oneshot::channel::<Result<serde_json::Value, anyhow::Error>>();

        // Build the BrowserAction and a mapper that converts the typed reply
        // into a uniform serde_json::Value result for the wrapper oneshot.
        let action: BrowserAction = match params.action {
            DebugAction::EvalJs => {
                let expression = match params.expression {
                    Some(expr) => expr,
                    None => return missing_param_json("eval_js requires 'expression' parameter"),
                };
                let (tx, rx) = tokio::sync::oneshot::channel();
                let reply_tx = reply_tx;
                tokio::spawn(async move {
                    let result = rx
                        .await
                        .map_err(|_| anyhow::anyhow!("browser task died"))
                        .and_then(|r: daemon8_chrome::Result<_>| r.map_err(anyhow::Error::from))
                        .map(|s| serde_json::json!({ "result": s }));
                    let _ = reply_tx.send(result);
                });
                BrowserAction::EvalJs {
                    tab_id: params.tab_id,
                    expression,
                    reply: tx,
                }
            }
            DebugAction::Screenshot => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                let reply_tx = reply_tx;
                let selector = params.selector.clone();
                let shot_dir = self.screenshot_dir.clone();
                tokio::spawn(async move {
                    let result = rx
                        .await
                        .map_err(|_| anyhow::anyhow!("browser task died"))
                        .and_then(|r: daemon8_chrome::Result<_>| r.map_err(anyhow::Error::from))
                        .and_then(|bytes: Vec<u8>| {
                            let path = screenshot_path(&shot_dir, "browser", selector.as_deref());
                            std::fs::write(&path, &bytes)
                                .map_err(|e| anyhow::anyhow!("failed to write screenshot: {e}"))?;
                            Ok(serde_json::json!({
                                "screenshot": path.display().to_string(),
                                "size_bytes": bytes.len(),
                                "selector": selector,
                            }))
                        });
                    let _ = reply_tx.send(result);
                });
                BrowserAction::Screenshot {
                    tab_id: params.tab_id,
                    selector: params.selector,
                    reply: tx,
                }
            }
            DebugAction::InjectCss => {
                let css = match params.css {
                    Some(css) => css,
                    None => return missing_param_json("inject_css requires 'css' parameter"),
                };
                let temporary = params.temporary.unwrap_or(true);
                let (tx, rx) = tokio::sync::oneshot::channel();
                let reply_tx = reply_tx;
                tokio::spawn(async move {
                    let result = rx
                        .await
                        .map_err(|_| anyhow::anyhow!("browser task died"))
                        .and_then(|r: daemon8_chrome::Result<_>| r.map_err(anyhow::Error::from))
                        .map(|element_id| {
                            serde_json::json!({
                                "injected": true,
                                "element_id": element_id,
                                "temporary": temporary,
                            })
                        });
                    let _ = reply_tx.send(result);
                });
                BrowserAction::InjectCss {
                    tab_id: params.tab_id,
                    css,
                    temporary,
                    reply: tx,
                }
            }
            DebugAction::RevertCss => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                let reply_tx = reply_tx;
                tokio::spawn(async move {
                    let result = rx
                        .await
                        .map_err(|_| anyhow::anyhow!("browser task died"))
                        .and_then(|r: daemon8_chrome::Result<_>| r.map_err(anyhow::Error::from))
                        .map(|count| serde_json::json!({ "reverted_count": count }));
                    let _ = reply_tx.send(result);
                });
                BrowserAction::RevertCss {
                    tab_id: params.tab_id,
                    reply: tx,
                }
            }
            DebugAction::ListTabs => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                let reply_tx = reply_tx;
                tokio::spawn(async move {
                    let result = rx
                        .await
                        .map_err(|_| anyhow::anyhow!("browser task died"))
                        .and_then(|r: daemon8_chrome::Result<_>| r.map_err(anyhow::Error::from))
                        .map(|tabs| serde_json::json!({ "tabs": tabs }));
                    let _ = reply_tx.send(result);
                });
                BrowserAction::ListTabs { reply: tx }
            }
            DebugAction::GetPerfMetrics => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                let reply_tx = reply_tx;
                tokio::spawn(async move {
                    let result = rx
                        .await
                        .map_err(|_| anyhow::anyhow!("browser task died"))
                        .and_then(|r: daemon8_chrome::Result<_>| r.map_err(anyhow::Error::from))
                        .and_then(|metrics| {
                            let json = serde_json::to_value(&metrics)
                                .map_err(|e| anyhow::anyhow!("serialization failed: {e}"))?;
                            Ok(serde_json::json!({ "metrics": json }))
                        });
                    let _ = reply_tx.send(result);
                });
                BrowserAction::GetPerformanceMetrics {
                    tab_id: params.tab_id,
                    reply: tx,
                }
            }
            DebugAction::GetDom => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                let reply_tx = reply_tx;
                tokio::spawn(async move {
                    let result = rx
                        .await
                        .map_err(|_| anyhow::anyhow!("browser task died"))
                        .and_then(|r: daemon8_chrome::Result<_>| r.map_err(anyhow::Error::from))
                        .map(|html| serde_json::json!({ "html": html }));
                    let _ = reply_tx.send(result);
                });
                BrowserAction::GetDom {
                    tab_id: params.tab_id,
                    selector: params.selector,
                    reply: tx,
                }
            }
            DebugAction::SetViewport => {
                let width = params.viewport_width.unwrap_or(390);
                let height = params.viewport_height.unwrap_or(844);
                let scale = params.viewport_scale.unwrap_or(2.0);
                let mobile = params.viewport_mobile.unwrap_or(true);
                let (tx, rx) = tokio::sync::oneshot::channel();
                let reply_tx = reply_tx;
                tokio::spawn(async move {
                    let result = rx
                        .await
                        .map_err(|_| anyhow::anyhow!("browser task died"))
                        .and_then(|r: daemon8_chrome::Result<_>| r.map_err(anyhow::Error::from))
                        .map(|()| {
                            serde_json::json!({
                                "viewport_set": true,
                                "width": width,
                                "height": height,
                                "scale": scale,
                                "mobile": mobile,
                            })
                        });
                    let _ = reply_tx.send(result);
                });
                BrowserAction::SetViewport {
                    tab_id: params.tab_id,
                    width,
                    height,
                    device_scale_factor: scale,
                    mobile,
                    user_agent: params.viewport_ua,
                    reply: tx,
                }
            }
            DebugAction::ClearViewport => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                let reply_tx = reply_tx;
                tokio::spawn(async move {
                    let result = rx
                        .await
                        .map_err(|_| anyhow::anyhow!("browser task died"))
                        .and_then(|r: daemon8_chrome::Result<_>| r.map_err(anyhow::Error::from))
                        .map(|()| serde_json::json!({ "viewport_cleared": true }));
                    let _ = reply_tx.send(result);
                });
                BrowserAction::ClearViewport {
                    tab_id: params.tab_id,
                    reply: tx,
                }
            }
            DebugAction::NetworkConditions => {
                let preset = params.network_preset.unwrap_or(NetworkPreset::Restore);
                let preset_str = preset.as_str();
                let (tx, rx) = tokio::sync::oneshot::channel();
                let reply_tx = reply_tx;
                tokio::spawn(async move {
                    let result = rx
                        .await
                        .map_err(|_| anyhow::anyhow!("browser task died"))
                        .and_then(|r: daemon8_chrome::Result<_>| r.map_err(anyhow::Error::from))
                        .map(|()| serde_json::json!({ "network_conditions": preset_str }));
                    let _ = reply_tx.send(result);
                });
                BrowserAction::SetNetworkConditions {
                    tab_id: params.tab_id,
                    preset: preset.as_str().to_string(),
                    reply: tx,
                }
            }
            DebugAction::Navigate => {
                let url = match params.url {
                    Some(u) => u,
                    None => return missing_param_json("navigate requires 'url' parameter"),
                };
                let (tx, rx) = tokio::sync::oneshot::channel();
                let reply_tx = reply_tx;
                tokio::spawn(async move {
                    let result = rx
                        .await
                        .map_err(|_| anyhow::anyhow!("browser task died"))
                        .and_then(|r: daemon8_chrome::Result<_>| r.map_err(anyhow::Error::from))
                        .map(|title| serde_json::json!({ "navigated": true, "title": title }));
                    let _ = reply_tx.send(result);
                });
                BrowserAction::Navigate {
                    tab_id: params.tab_id,
                    url,
                    reply: tx,
                }
            }
            DebugAction::StorageClear => {
                let types = params.storage_types.unwrap_or_else(|| "all".to_string());
                let (tx, rx) = tokio::sync::oneshot::channel();
                let reply_tx = reply_tx;
                tokio::spawn(async move {
                    let result = rx
                        .await
                        .map_err(|_| anyhow::anyhow!("browser task died"))
                        .and_then(|r: daemon8_chrome::Result<_>| r.map_err(anyhow::Error::from))
                        .map(|()| serde_json::json!({ "cleared": true }));
                    let _ = reply_tx.send(result);
                });
                BrowserAction::StorageClear {
                    tab_id: params.tab_id,
                    storage_types: types,
                    reply: tx,
                }
            }
            DebugAction::StorageInspect => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                let reply_tx = reply_tx;
                tokio::spawn(async move {
                    let result = rx
                        .await
                        .map_err(|_| anyhow::anyhow!("browser task died"))
                        .and_then(|r: daemon8_chrome::Result<_>| r.map_err(anyhow::Error::from));
                    let _ = reply_tx.send(result);
                });
                BrowserAction::StorageInspect {
                    tab_id: params.tab_id,
                    reply: tx,
                }
            }
            DebugAction::StorageSet => {
                let store_type = match params.store_type {
                    Some(t) => t.as_str().to_string(),
                    None => {
                        return missing_param_json("storage_set requires 'store_type' parameter");
                    }
                };
                let key = match params.storage_key {
                    Some(k) => k,
                    None => {
                        return missing_param_json("storage_set requires 'storage_key' parameter");
                    }
                };
                let value = params.storage_value.unwrap_or_default();
                let (tx, rx) = tokio::sync::oneshot::channel();
                let reply_tx = reply_tx;
                tokio::spawn(async move {
                    let result = rx
                        .await
                        .map_err(|_| anyhow::anyhow!("browser task died"))
                        .and_then(|r: daemon8_chrome::Result<_>| r.map_err(anyhow::Error::from))
                        .map(|()| serde_json::json!({ "set": true }));
                    let _ = reply_tx.send(result);
                });
                BrowserAction::StorageSet {
                    tab_id: params.tab_id,
                    store_type,
                    key,
                    value,
                    reply: tx,
                }
            }
            DebugAction::ElementAtPoint => {
                let x = match params.x {
                    Some(v) => v,
                    None => return missing_param_json("element_at_point requires 'x' parameter"),
                };
                let y = match params.y {
                    Some(v) => v,
                    None => return missing_param_json("element_at_point requires 'y' parameter"),
                };
                let (tx, rx) = tokio::sync::oneshot::channel();
                let reply_tx = reply_tx;
                tokio::spawn(async move {
                    let result = rx
                        .await
                        .map_err(|_| anyhow::anyhow!("browser task died"))
                        .and_then(|r: daemon8_chrome::Result<_>| r.map_err(anyhow::Error::from));
                    let _ = reply_tx.send(result);
                });
                BrowserAction::ElementAtPoint {
                    tab_id: params.tab_id,
                    x,
                    y,
                    reply: tx,
                }
            }
            DebugAction::NewTab => {
                let url = params.url.unwrap_or_else(|| "about:blank".to_string());
                let (tx, rx) = tokio::sync::oneshot::channel();
                let reply_tx = reply_tx;
                tokio::spawn(async move {
                    let result = rx
                        .await
                        .map_err(|_| anyhow::anyhow!("browser task died"))
                        .and_then(|r: daemon8_chrome::Result<_>| r.map_err(anyhow::Error::from))
                        .map(|target_id| serde_json::json!({ "tab_id": target_id }));
                    let _ = reply_tx.send(result);
                });
                BrowserAction::NewTab { url, reply: tx }
            }
            DebugAction::CloseTab => {
                let tab_id = match params.tab_id {
                    Some(id) => id,
                    None => return missing_param_json("close_tab requires 'tab_id' parameter"),
                };
                let (tx, rx) = tokio::sync::oneshot::channel();
                let reply_tx = reply_tx;
                tokio::spawn(async move {
                    let result = rx
                        .await
                        .map_err(|_| anyhow::anyhow!("browser task died"))
                        .and_then(|r: daemon8_chrome::Result<_>| r.map_err(anyhow::Error::from))
                        .map(|()| serde_json::json!({ "closed": true }));
                    let _ = reply_tx.send(result);
                });
                BrowserAction::CloseTab { tab_id, reply: tx }
            }
        };

        if self
            .chrome_tx
            .send(ChromeCommand::Action(action))
            .await
            .is_err()
        {
            tracing::warn!("browser action command rejected: daemon shutting down");
            return error_json("daemon_shutting_down", "Daemon is shutting down.");
        }

        match tokio::time::timeout(std::time::Duration::from_secs(30), reply_rx).await {
            Err(_) => error_json(
                "action_failed",
                "Browser action timed out (30s). The daemon is still connected and will recover.",
            ),
            Ok(Ok(Ok(value))) => serde_json::to_string(&value).unwrap_or_default(),
            Ok(Ok(Err(e))) => error_json("action_failed", &format!("{e}")),
            Ok(Err(_)) => error_json(
                "browser_not_connected",
                "Browser connection lost during action. The daemon is reconnecting automatically.",
            ),
        }
    }
}

impl DaemonMcp {
    async fn handle_device_screenshot(&self, params: &ActParams) -> String {
        let screenshot_fn = match &self.device_screenshot_fn {
            Some(f) => f,
            None => {
                tracing::warn!(
                    "device screenshot requested but ADB screenshot support is unavailable"
                );
                return error_json(
                    "device_screenshot_unavailable",
                    "device screenshots not available (ADB not enabled)",
                );
            }
        };

        let serial = params.device_serial.clone().unwrap_or_default();
        let platform = match params.device_platform.as_deref() {
            Some("vega") => DevicePlatform::Vega,
            _ => DevicePlatform::Android,
        };

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            (screenshot_fn)(serial.clone(), platform.clone()),
        )
        .await;

        match result {
            Err(_) => {
                tracing::warn!(serial, platform = ?platform, "device screenshot timed out");
                error_json("action_failed", "device screenshot timed out (15s)")
            }
            Ok(Err(e)) => {
                tracing::warn!(serial, platform = ?platform, error = %e, "device screenshot failed");
                error_json(
                    "action_failed",
                    &format!("device screenshot failed for {serial}: {e}"),
                )
            }
            Ok(Ok(shot)) => {
                let path = screenshot_path(&self.screenshot_dir, &serial, Some(&shot.source));
                if let Err(e) = std::fs::write(&path, &shot.png_bytes) {
                    tracing::warn!(serial, path = %path.display(), error = %e, "failed to write device screenshot");
                    return error_json(
                        "action_failed",
                        &format!("failed to write screenshot: {e}"),
                    );
                }
                tracing::info!(
                    serial,
                    source = %shot.source,
                    path = %path.display(),
                    size_bytes = shot.png_bytes.len(),
                    "device screenshot captured"
                );
                serde_json::to_string(&serde_json::json!({
                    "screenshot": path.display().to_string(),
                    "size_bytes": shot.png_bytes.len(),
                    "source": shot.source,
                    "serial": serial,
                }))
                .unwrap_or_default()
            }
        }
    }
}

fn screenshot_path(dir: &std::path::Path, target: &str, label: Option<&str>) -> std::path::PathBuf {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let safe_target = target.replace(['/', '\\', ':'], "-");
    let suffix = label.map(|l| format!("-{l}")).unwrap_or_default();
    dir.join(format!("daemon8-screenshot-{ts}-{safe_target}{suffix}.png"))
}

/// Standalone error builder for code paths without &self access.
fn validate_action_params(params: &ActParams) -> Option<String> {
    match params.action {
        DebugAction::EvalJs if params.expression.is_none() => Some(missing_param_json(
            "eval_js requires 'expression' parameter",
        )),
        DebugAction::InjectCss if params.css.is_none() => {
            Some(missing_param_json("inject_css requires 'css' parameter"))
        }
        DebugAction::Navigate if params.url.is_none() => {
            Some(missing_param_json("navigate requires 'url' parameter"))
        }
        DebugAction::StorageSet if params.store_type.is_none() => Some(missing_param_json(
            "storage_set requires 'store_type' parameter",
        )),
        DebugAction::StorageSet if params.storage_key.is_none() => Some(missing_param_json(
            "storage_set requires 'storage_key' parameter",
        )),
        DebugAction::ElementAtPoint if params.x.is_none() => Some(missing_param_json(
            "element_at_point requires 'x' parameter",
        )),
        DebugAction::ElementAtPoint if params.y.is_none() => Some(missing_param_json(
            "element_at_point requires 'y' parameter",
        )),
        DebugAction::CloseTab if params.tab_id.is_none() => {
            Some(missing_param_json("close_tab requires 'tab_id' parameter"))
        }
        _ => None,
    }
}

fn missing_param_json(msg: &str) -> String {
    error_json("missing_param", msg)
}

fn error_json(code: &str, msg: &str) -> String {
    AlphaEnvelope::non_success(AlphaStatus::Error, code, msg, msg).render()
}

impl DaemonMcp {
    /// Returns the connection-state JSON with full state including browser.
    pub async fn connections_json(&self) -> String {
        let chrome_state = *self.chrome_state.borrow();
        let chrome_endpoint = self
            .chrome_endpoint
            .lock()
            .expect("chrome_endpoint mutex poisoned")
            .clone();
        let mut result = serde_json::json!({
            "browser": {
                "state": format!("{chrome_state}"),
                "endpoint": chrome_endpoint,
            }
        });

        if let Ok(summary) = self.store.summary().await
            && !summary.connections.is_empty()
        {
            result["applications"] = serde_json::to_value(&summary.connections).unwrap_or_default();
        }

        serde_json::to_string_pretty(&result).unwrap_or_else(|e| {
            error_json(
                "serialization_failed",
                &format!("serialization failed: {e}"),
            )
        })
    }

    pub fn tools_for_client(&self) -> Vec<Tool> {
        self.tool_router.list_all()
    }
}

impl ServerHandler for DaemonMcp {
    fn get_info(&self) -> ServerInfo {
        let instructions = String::from(INSTRUCTIONS);

        let mut capabilities = ServerCapabilities::builder().enable_tools().build();
        capabilities.experimental = Some(std::collections::BTreeMap::from([(
            "claude/channel".to_string(),
            serde_json::Map::new(),
        )]));

        ServerInfo::new(capabilities)
            .with_server_info(Implementation::new("daemon8", env!("CARGO_PKG_VERSION")))
            .with_instructions(instructions)
    }

    async fn on_initialized(&self, context: rmcp::service::NotificationContext<RoleServer>) {
        let session_id = self.session_id.clone();
        tracing::info!(
            session_id,
            "MCP session initialized, starting observation push"
        );
        let peer = context.peer;
        let mut rx = self.broadcast_tx.subscribe();
        let sub_rx = self.subscription_tx.subscribe();
        let session_cancel = self.cancel.child_token();
        let span = tracing::info_span!("mcp_session", session_id = %session_id);

        tokio::spawn(async move {
            let mut last_push = std::time::Instant::now() - Duration::from_secs(2);
            loop {
                let (arc_obs, _json) = tokio::select! {
                    biased;
                    _ = session_cancel.cancelled() => {
                        tracing::debug!("MCP observation push cancelled");
                        break;
                    }
                    recv = rx.recv() => match recv {
                        Ok(payload) => payload,
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "MCP observation push receiver lagged; continuing with newest observations");
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            tracing::debug!("MCP observation push broadcast closed");
                            break;
                        }
                    },
                };

                let obs: &Observation = &arc_obs;
                let filter = sub_rx.borrow().clone();
                let should_push = match filter.as_ref() {
                    Some(f) => f.matches(obs),
                    None => obs.severity.level() >= daemon8_types::Severity::Warn.level(),
                };

                if !should_push {
                    continue;
                }

                if last_push.elapsed() < Duration::from_secs(1) {
                    tracing::debug!(
                        observation_id = obs.id,
                        severity = %obs.severity,
                        kind = %obs.kind.tag(),
                        "MCP observation push throttled"
                    );
                    continue;
                }

                let param = logging_notification(obs);
                let send = tokio::time::timeout(Duration::from_secs(5), peer.notify_logging_message(param)).await;

                match send {
                    Ok(Ok(())) => {
                        last_push = std::time::Instant::now();
                        tracing::debug!(
                            observation_id = obs.id,
                            severity = %obs.severity,
                            kind = %obs.kind.tag(),
                            "MCP observation pushed to client"
                        );
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(error = ?e, "MCP observation push failed; ending session push task");
                        break;
                    }
                    Err(_) => {
                        tracing::warn!(
                            observation_id = obs.id,
                            severity = %obs.severity,
                            kind = %obs.kind.tag(),
                            "MCP observation push timed out"
                        );
                    }
                }
            }
            tracing::debug!("MCP observation push task ended");
        }
        .instrument(span));

        // Per-session debug session flush: periodically writes the in-memory
        // last_activity to the DB so the cleanup task's find_stale_active
        // sees current data. Each MCP session flushes its own active session.
        if let Some(ds_store) = self.debug_session_store.clone() {
            let flush_state = self.active_state.clone();
            let flush_cancel = self.cancel.child_token();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        () = tokio::time::sleep(Duration::from_secs(60)) => {}
                        () = flush_cancel.cancelled() => break,
                    }
                    if let Some(session) = flush_state.current_session() {
                        let last = session.last_activity();
                        if let Err(e) = ds_store
                            .touch_debug_session(session.id.as_ref(), last)
                            .await
                        {
                            tracing::warn!(
                                session_id = %session.id,
                                error = %e,
                                "per-session debug session flush failed"
                            );
                        }
                    }
                }
                tracing::debug!("per-session debug session flush task ended");
            });
        }
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, rmcp::ErrorData> {
        Ok(rmcp::model::ListToolsResult {
            tools: self.tools_for_client(),
            meta: None,
            next_cursor: None,
        })
    }

    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResult, rmcp::ErrorData> {
        let tool = request.name.to_string();
        let started = Instant::now();
        tracing::debug!(tool = %tool, "MCP tool call started");

        if let Some(body) = self.connect_preflight(&tool) {
            tracing::info!(tool = %tool, "MCP tool call blocked until daemon8_connect");
            return Ok(rmcp::model::CallToolResult::success(vec![
                rmcp::model::Content::text(body),
            ]));
        }

        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        let result = self.tool_router.call(tcc).await;
        let duration_ms = started.elapsed().as_millis();

        match &result {
            Ok(result) if result.is_error.unwrap_or(false) => {
                tracing::warn!(tool = %tool, duration_ms, "MCP tool call returned error result");
            }
            Ok(_) => {
                tracing::info!(tool = %tool, duration_ms, "MCP tool call completed");
            }
            Err(e) => {
                tracing::warn!(tool = %tool, duration_ms, error = ?e, "MCP tool call failed");
            }
        }

        result
    }

    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        self.tool_router.get(name).cloned()
    }
}

fn logging_notification(obs: &Observation) -> rmcp::model::LoggingMessageNotificationParam {
    let severity_str = obs.severity.to_string();
    let kind_str = obs.kind.tag().to_string();
    let origin_str = match &obs.origin {
        daemon8_types::Origin::Application { name } => format!("app:{name}"),
        daemon8_types::Origin::Browser { tab_id, .. } => format!("browser:{tab_id}"),
        daemon8_types::Origin::Device { serial, .. } => format!("device:{serial}"),
    };
    let msg = obs.data["message"]
        .as_str()
        .or_else(|| obs.data["msg"].as_str())
        .unwrap_or("(no message)");
    let level = match obs.severity {
        daemon8_types::Severity::Trace | daemon8_types::Severity::Debug => {
            rmcp::model::LoggingLevel::Debug
        }
        daemon8_types::Severity::Info => rmcp::model::LoggingLevel::Info,
        daemon8_types::Severity::Warn => rmcp::model::LoggingLevel::Warning,
        daemon8_types::Severity::Error => rmcp::model::LoggingLevel::Error,
    };
    let data = serde_json::json!({
        "message": format!("[{severity_str}] {kind_str} from {origin_str}: {msg}"),
        "severity": severity_str,
        "kind": kind_str,
        "origin": origin_str,
        "observation_id": obs.id,
    });

    rmcp::model::LoggingMessageNotificationParam::new(level, data)
        .with_logger("daemon8".to_string())
}

#[cfg(test)]
mod logging_tests {
    use daemon8_types::{ObservationKind, Origin, Severity};

    use super::*;

    #[test]
    fn mcp_session_ids_are_stable_and_prefixed() {
        let id = next_mcp_session_id();
        assert!(id.starts_with("mcp-"));
    }

    async fn build_mcp_with_debug_session() -> DaemonMcp {
        let store = Arc::new(daemon8_store::SurrealStore::memory().await.unwrap());
        let memory_store: Arc<dyn MemoryStore> = Arc::new(store.memory_store());
        let debug_session_store: Arc<dyn DebugSessionStore> = Arc::new(store.debug_session_store());
        let scope_ledger_store: Arc<dyn ScopeLedgerStore> = Arc::new(store.scope_ledger_store());
        let (obs_tx, _obs_rx) = tokio::sync::mpsc::unbounded_channel();
        let (chrome_tx, _chrome_rx) = tokio::sync::mpsc::channel(8);
        let (_, chrome_state) =
            tokio::sync::watch::channel(daemon8_chrome::ConnectionState::Disconnected);
        let (broadcast_tx, _broadcast_rx) = broadcast::channel(8);
        let lens = Arc::new(LensManager::new(broadcast_tx.subscribe()));
        DaemonMcp::new(DaemonMcpConfig {
            store: store.clone(),
            memory_store: Some(memory_store),
            debug_session_store: Some(debug_session_store),
            scope_ledger_store: Some(scope_ledger_store),
            obs_tx,
            chrome_tx,
            chrome_state,
            chrome_endpoint: Arc::new(Mutex::new(None)),
            device_screenshot_fn: None,
            screenshot_dir: std::env::temp_dir().join("daemon8-test"),
            broadcast_tx,
            source_trigger: None,
            lens,
            cancel: tokio_util::sync::CancellationToken::new(),
        })
    }

    #[tokio::test]
    async fn debug_session_lifecycle_resolved_writes_rich_summary() {
        let mcp = build_mcp_with_debug_session().await;

        // start
        let start_res = mcp
            .start_debug_session(Parameters(StartDebugSessionParams {
                project: Some("daemon8".into()),
                description: Some("flaky login test".into()),
                agent_id: ":test/claude+plan-agent>".into(),
                feature: None,
            }))
            .await;
        let started: serde_json::Value = serde_json::from_str(&start_res).unwrap();
        assert_eq!(started["code"], "debug_session_started");
        let session_id = started["data"]["debug_session_id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(mcp.active_state.current_session().is_some());

        // resolve with rich fields
        let resolve_res = mcp
            .resolve_debug_session(Parameters(ResolveDebugSessionParams {
                summary: "Cookie domain mismatch dropped session on subdomain switch.".into(),
                root_cause: Some("Set-Cookie missing Domain attr".into()),
                fix_diff: Some(
                    "- res.cookie('s', tok)\n+ res.cookie('s', tok, {domain: '.x'})".into(),
                ),
                commands_used: Some(vec!["cargo test login".into()]),
                related_errors: Some(vec!["abcd1234deadbeef".into()]),
                tags: Some(vec!["auth".into(), "regression".into()]),
            }))
            .await;
        let resolved: serde_json::Value = serde_json::from_str(&resolve_res).unwrap();
        assert_eq!(resolved["code"], "debug_session_resolved");
        assert_eq!(resolved["data"]["debug_session_id"], session_id);
        assert_eq!(resolved["data"]["project_slug"], "daemon8");
        let memory_id = resolved["data"]["summary_memory_id"].as_str().unwrap();
        assert_eq!(resolved["data"]["evidence_ref"]["kind"], "session_summary");
        assert_eq!(resolved["data"]["evidence_ref"]["id"], memory_id);

        // active state cleared
        assert!(mcp.active_state.current_session().is_none());

        // SessionSummary memory landed with rich data
        let mem_store = mcp.memory_store.clone().unwrap();
        let mem = mem_store.get_memory(memory_id).await.unwrap().unwrap();
        assert_eq!(mem.kind, daemon8_types::MemoryKind::SessionSummary);
        assert!(mem.content.contains("Cookie domain"));
        let data = mem.data.expect("resolved session must carry rich data");
        assert_eq!(data["root_cause"], "Set-Cookie missing Domain attr");
        assert!(mem.tags.contains(&"outcome:resolved".to_string()));
        assert!(mem.tags.contains(&"hash:abcd1234deadbeef".to_string()));
        assert!(mem.tags.contains(&"auth".to_string()));

        // session row updated to completed
        let ds_store = mcp.debug_session_store.clone().unwrap();
        let session = ds_store
            .get_debug_session(&session_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(session.status, daemon8_types::DebugSessionStatus::Completed);
        assert_eq!(
            session.outcome,
            Some(daemon8_types::DebugSessionOutcome::Resolved)
        );
        assert_eq!(session.summary_memory_id.as_deref(), Some(memory_id));
    }

    #[tokio::test]
    async fn debug_session_double_start_rejected() {
        let mcp = build_mcp_with_debug_session().await;
        let _ = mcp
            .start_debug_session(Parameters(StartDebugSessionParams {
                project: None,
                description: None,
                agent_id: ":test/claude+plan-agent>".into(),
                feature: None,
            }))
            .await;
        let second = mcp
            .start_debug_session(Parameters(StartDebugSessionParams {
                project: None,
                description: None,
                agent_id: ":test/claude+plan-agent>".into(),
                feature: None,
            }))
            .await;
        assert!(
            second.contains("already_active_debug_session"),
            "second start must be rejected: {second}"
        );
    }

    #[tokio::test]
    async fn create_checkpoint_without_active_session_returns_structured_error() {
        let mcp = build_mcp_with_debug_session().await;
        let res = mcp
            .create_checkpoint(Parameters(CreateCheckpointParams { description: None }))
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&res).unwrap();
        assert_eq!(parsed["status"], "error");
        assert_eq!(parsed["code"], "no_active_debug_session");
        assert_eq!(parsed["next_actions"][0]["tool"], "start_debug_session");
    }

    #[tokio::test]
    async fn create_checkpoint_inside_active_session_writes_row_and_updates_active_state() {
        let mcp = build_mcp_with_debug_session().await;
        let _ = mcp
            .start_debug_session(Parameters(StartDebugSessionParams {
                project: Some("daemon8".into()),
                description: None,
                agent_id: ":test/claude+plan-agent>".into(),
                feature: None,
            }))
            .await;
        let res = mcp
            .create_checkpoint(Parameters(CreateCheckpointParams {
                description: Some("before fix".into()),
            }))
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&res).unwrap();
        let result = &parsed["data"];
        assert_eq!(parsed["code"], "checkpoint_created");
        let cp_id = result["checkpoint_id"].as_str().unwrap();
        assert!(result["seq_at_creation"].is_number());
        // Envelope echoes the active session.
        assert!(parsed["data"]["active_debug_session"].is_object());

        // active_state.checkpoint should now match
        let active_cp = mcp.active_state.current_checkpoint().unwrap();
        assert_eq!(active_cp.as_ref(), cp_id);

        // Row exists in store with our description
        let ds_store = mcp.debug_session_store.clone().unwrap();
        let cp = ds_store.get_checkpoint(cp_id).await.unwrap().unwrap();
        assert_eq!(cp.description.as_deref(), Some("before fix"));
    }

    #[tokio::test]
    async fn end_without_active_session_returns_error() {
        let mcp = build_mcp_with_debug_session().await;
        let res = mcp
            .end_debug_session(Parameters(EndDebugSessionParams { outcome: None }))
            .await;
        assert!(res.contains("no_active_debug_session"));
    }

    #[tokio::test]
    async fn list_debug_sessions_filters_by_status() {
        let mcp = build_mcp_with_debug_session().await;
        // start + resolve one
        let _ = mcp
            .start_debug_session(Parameters(StartDebugSessionParams {
                project: Some("p".into()),
                description: None,
                agent_id: ":test/claude+plan-agent>".into(),
                feature: None,
            }))
            .await;
        let _ = mcp
            .resolve_debug_session(Parameters(ResolveDebugSessionParams {
                summary: "x".into(),
                root_cause: None,
                fix_diff: None,
                commands_used: None,
                related_errors: None,
                tags: None,
            }))
            .await;
        // start a fresh one (active)
        let _ = mcp
            .start_debug_session(Parameters(StartDebugSessionParams {
                project: Some("p".into()),
                description: None,
                agent_id: ":test/claude+plan-agent>".into(),
                feature: None,
            }))
            .await;

        let active_only = mcp
            .list_debug_sessions(Parameters(ListDebugSessionsParams {
                status: Some("active".into()),
                feature: None,
            }))
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&active_only).unwrap();
        assert_eq!(parsed["code"], "debug_sessions_listed");
        assert_eq!(parsed["data"]["count"], 1);

        let all = mcp
            .list_debug_sessions(Parameters(ListDebugSessionsParams {
                status: None,
                feature: None,
            }))
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&all).unwrap();
        assert_eq!(parsed["data"]["count"], 2);
    }

    #[test]
    fn logging_notification_includes_operational_fields() {
        let mut obs = Observation::new(
            Origin::Application {
                name: "test-app".into(),
            },
            ObservationKind::Log,
            serde_json::json!({"message": "hello"}),
            Severity::Warn,
            None,
        );
        obs.id = 42;

        let param = logging_notification(&obs);
        assert_eq!(param.logger.as_deref(), Some("daemon8"));
        assert_eq!(param.data["severity"], "warn");
        assert_eq!(param.data["kind"], "log");
        assert_eq!(param.data["origin"], "app:test-app");
        assert_eq!(param.data["observation_id"], 42);
    }

    // ── B4: Multi-session + agent ID tests ──────────────────────────

    /// Two MCP instances from a shared SurrealDB store — each gets its own
    /// ActiveSessionState (created internally by DaemonMcp::new), so they
    /// do not interfere with each other's debug sessions.
    async fn build_shared_mcps() -> (DaemonMcp, DaemonMcp) {
        let shared_store = Arc::new(daemon8_store::SurrealStore::memory().await.unwrap());
        let shared_mem: Arc<dyn MemoryStore> = Arc::new(shared_store.memory_store());
        let shared_ds: Arc<dyn DebugSessionStore> = Arc::new(shared_store.debug_session_store());
        let shared_scope_ledger: Arc<dyn ScopeLedgerStore> =
            Arc::new(shared_store.scope_ledger_store());

        // Keep receivers alive so observations sent via write_to_live_feed
        // actually reach the store through the drain tasks.
        let (shared_obs_tx, mut shared_obs_rx) = tokio::sync::mpsc::unbounded_channel();

        // Drain task: reads from the shared channel and inserts into the store.
        // Tokio drops the spawned task when the test runtime shuts down.
        let drain_store = shared_store.clone();
        tokio::spawn(async move {
            while let Some(obs) = shared_obs_rx.recv().await {
                let _ = drain_store.insert(obs).await;
            }
        });

        let make = || {
            let (chrome_tx, _chrome_rx) = tokio::sync::mpsc::channel(8);
            let (_, chrome_state) =
                tokio::sync::watch::channel(daemon8_chrome::ConnectionState::Disconnected);
            let (broadcast_tx, _broadcast_rx) = broadcast::channel(8);
            let lens = Arc::new(LensManager::new(broadcast_tx.subscribe()));
            DaemonMcp::new(DaemonMcpConfig {
                store: shared_store.clone(),
                memory_store: Some(shared_mem.clone()),
                debug_session_store: Some(shared_ds.clone()),
                scope_ledger_store: Some(shared_scope_ledger.clone()),
                obs_tx: shared_obs_tx.clone(),
                chrome_tx,
                chrome_state,
                chrome_endpoint: Arc::new(Mutex::new(None)),
                device_screenshot_fn: None,
                screenshot_dir: std::env::temp_dir().join("daemon8-test"),
                broadcast_tx,
                source_trigger: None,
                lens,
                cancel: tokio_util::sync::CancellationToken::new(),
            })
        };

        (make(), make())
    }

    #[tokio::test]
    async fn multi_session_two_agents_non_conflicting() {
        let (a, b) = build_shared_mcps().await;

        let a_start = a
            .start_debug_session(Parameters(StartDebugSessionParams {
                project: Some("daemon8".into()),
                description: Some("agent A investigating auth".into()),
                agent_id: ":test/claude+plan-agent>".into(),
                feature: Some("auth".into()),
            }))
            .await;
        let a_parsed: serde_json::Value = serde_json::from_str(&a_start).unwrap();
        assert!(
            a_parsed["data"]["debug_session_id"].is_string(),
            "agent A must start successfully: {a_start}"
        );

        let b_start = b
            .start_debug_session(Parameters(StartDebugSessionParams {
                project: Some("daemon8".into()),
                description: Some("agent B investigating search".into()),
                agent_id: ":test/codex+build-agent>".into(),
                feature: Some("search".into()),
            }))
            .await;
        let b_parsed: serde_json::Value = serde_json::from_str(&b_start).unwrap();
        assert!(
            b_parsed["data"]["debug_session_id"].is_string(),
            "agent B must start successfully — no global single-active: {b_start}"
        );

        // Verify B's response does NOT contain an already_active error
        assert!(
            !b_start.contains("already_active_debug_session"),
            "agent B must not be blocked by agent A's active session"
        );
    }

    #[tokio::test]
    async fn multi_session_observations_stamped_independently() {
        let (a, b) = build_shared_mcps().await;

        // Agent A starts auth session
        let a_start: serde_json::Value = serde_json::from_str(
            &a.start_debug_session(Parameters(StartDebugSessionParams {
                project: Some("p".into()),
                description: None,
                agent_id: ":test/claude+plan-agent>".into(),
                feature: None,
            }))
            .await,
        )
        .unwrap();
        let a_sid = a_start["data"]["debug_session_id"].as_str().unwrap();

        // Agent B starts search session
        let b_start: serde_json::Value = serde_json::from_str(
            &b.start_debug_session(Parameters(StartDebugSessionParams {
                project: Some("p".into()),
                description: None,
                agent_id: ":test/codex+build-agent>".into(),
                feature: None,
            }))
            .await,
        )
        .unwrap();
        let b_sid = b_start["data"]["debug_session_id"].as_str().unwrap();

        assert_ne!(a_sid, b_sid, "each agent must get a distinct session id");

        // Each ingests an observation through their own MCP instance
        a.write_to_live_feed(Parameters(IngestParams {
            kind: Some("log".into()),
            severity: Some("info".into()),
            app: Some("test-a".into()),
            channel: None,
            correlation_id: None,
            parent_id: None,
            node_id: None,
            session_id: None,
            service: None,
            source: None,
            source_instance: None,
            tags: None,
            data: serde_json::json!({"msg": "from agent A"}),
        }))
        .await;

        b.write_to_live_feed(Parameters(IngestParams {
            kind: Some("log".into()),
            severity: Some("info".into()),
            app: Some("test-b".into()),
            channel: None,
            correlation_id: None,
            parent_id: None,
            node_id: None,
            session_id: None,
            service: None,
            source: None,
            source_instance: None,
            tags: None,
            data: serde_json::json!({"msg": "from agent B"}),
        }))
        .await;

        // Let the drain task process both observations
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Query the store directly to verify each observation got the right stamp
        let slice = a
            .store
            .query(&daemon8_types::Filter {
                kinds: Some(vec![daemon8_types::ObservationKindTag::Log]),
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(
            slice.observations.len(),
            2,
            "both observations must be in the shared store"
        );

        let has_a = slice
            .observations
            .iter()
            .any(|o| o.debug_session_id.as_deref() == Some(a_sid));
        let has_b = slice
            .observations
            .iter()
            .any(|o| o.debug_session_id.as_deref() == Some(b_sid));
        assert!(
            has_a,
            "observation from agent A must be stamped with A's session"
        );
        assert!(
            has_b,
            "observation from agent B must be stamped with B's session"
        );
    }

    #[tokio::test]
    async fn multi_session_end_one_leaves_other_active() {
        let (a, b) = build_shared_mcps().await;

        // Both start sessions
        let _ = a
            .start_debug_session(Parameters(StartDebugSessionParams {
                project: None,
                description: None,
                agent_id: ":test/claude+plan-agent>".into(),
                feature: None,
            }))
            .await;
        let _ = b
            .start_debug_session(Parameters(StartDebugSessionParams {
                project: None,
                description: None,
                agent_id: ":test/codex+build-agent>".into(),
                feature: None,
            }))
            .await;

        // A ends its session
        let end_res: serde_json::Value = serde_json::from_str(
            &a.end_debug_session(Parameters(EndDebugSessionParams { outcome: None }))
                .await,
        )
        .unwrap();
        assert_eq!(end_res["code"], "debug_session_ended");
        assert!(
            end_res["data"]["debug_session_id"].is_string(),
            "end must succeed: {end_res}"
        );

        // A's active state should be clear
        assert!(a.active_state.current_session().is_none());

        // B's active state should still be present
        assert!(
            b.active_state.current_session().is_some(),
            "agent B must remain active after A ends"
        );
    }

    #[tokio::test]
    async fn list_debug_sessions_filters_by_feature() {
        let (a, _b) = build_shared_mcps().await;

        // Agent A creates two sessions with different features
        let _ = a
            .start_debug_session(Parameters(StartDebugSessionParams {
                project: Some("p".into()),
                description: None,
                agent_id: ":test/claude+plan-agent>".into(),
                feature: Some("auth".into()),
            }))
            .await;
        // End first session so we can start a second (single-active per instance)
        let _ = a
            .end_debug_session(Parameters(EndDebugSessionParams { outcome: None }))
            .await;
        let _ = a
            .start_debug_session(Parameters(StartDebugSessionParams {
                project: Some("p".into()),
                description: None,
                agent_id: ":test/claude+plan-agent>".into(),
                feature: Some("search".into()),
            }))
            .await;

        // Filter by feature
        let auth_only: serde_json::Value = serde_json::from_str(
            &a.list_debug_sessions(Parameters(ListDebugSessionsParams {
                status: None,
                feature: Some("auth".into()),
            }))
            .await,
        )
        .unwrap();
        assert_eq!(
            auth_only["data"]["count"], 1,
            "feature filter must return only the auth session"
        );
        assert_eq!(
            auth_only["data"]["sessions"][0]["feature"], "auth",
            "returned session must have the matching feature"
        );

        let search_only: serde_json::Value = serde_json::from_str(
            &a.list_debug_sessions(Parameters(ListDebugSessionsParams {
                status: None,
                feature: Some("search".into()),
            }))
            .await,
        )
        .unwrap();
        assert_eq!(
            search_only["data"]["count"], 1,
            "feature filter must return only the search session"
        );

        let none: serde_json::Value = serde_json::from_str(
            &a.list_debug_sessions(Parameters(ListDebugSessionsParams {
                status: None,
                feature: Some("nonexistent".into()),
            }))
            .await,
        )
        .unwrap();
        assert_eq!(
            none["data"]["count"], 0,
            "unknown feature must return empty"
        );
    }

    // ── B4: Agent ID validation ──────────────────────────────────────

    #[test]
    fn agent_id_valid_formats() {
        for id in [
            ":mbp/claude+plan-agent>",
            ":linux/codex+build-agent>",
            ":mbp/gemini+researcher>",
            ":mini/copilot+reviewer>",
            ":box/opencode+crawler>",
            ":test-host/my-tool+my-role>",
        ] {
            assert!(
                validate_agent_id(id).is_ok(),
                "valid agent_id must pass: {id}"
            );
        }
    }

    #[test]
    fn agent_id_rejects_missing_colon() {
        assert!(validate_agent_id("mbp/claude+agent>").is_err());
    }

    #[test]
    fn agent_id_rejects_missing_gt() {
        assert!(validate_agent_id(":mbp/claude+agent").is_err());
    }

    #[test]
    fn agent_id_rejects_missing_slash() {
        assert!(validate_agent_id(":mbp-claude+agent>").is_err());
    }

    #[test]
    fn agent_id_rejects_missing_plus() {
        assert!(validate_agent_id(":mbp/claude-agent>").is_err());
    }

    #[test]
    fn agent_id_rejects_uppercase() {
        assert!(validate_agent_id(":MBP/claude+agent>").is_err());
    }

    #[test]
    fn agent_id_rejects_empty_host() {
        assert!(validate_agent_id(":/tool+role>").is_err());
    }

    #[test]
    fn agent_id_rejects_empty_tool() {
        assert!(validate_agent_id(":host/+role>").is_err());
    }

    #[test]
    fn agent_id_rejects_empty_role() {
        assert!(validate_agent_id(":host/tool+>").is_err());
    }

    #[test]
    fn agent_id_rejects_too_long() {
        let long_id = format!(":{}", "x".repeat(65));
        assert!(validate_agent_id(&long_id).is_err());
    }

    #[tokio::test]
    async fn start_debug_session_rejects_invalid_agent_id() {
        let mcp = build_mcp_with_debug_session().await;
        let res = mcp
            .start_debug_session(Parameters(StartDebugSessionParams {
                project: None,
                description: None,
                agent_id: "bad-format".into(),
                feature: None,
            }))
            .await;
        assert!(
            res.contains("invalid_agent_id"),
            "bad agent_id must be rejected: {res}"
        );
    }
}
