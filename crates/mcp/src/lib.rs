// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use daemon8_store::{
    BookkeeperStore, CardStore, EmbeddingProfileStore, EnvelopeStore, LensManager, MemoryLongStore,
    MemoryReferenceStore, MemoryShortStore, MemoryStore, StateModel,
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

const INSTRUCTIONS: &str = include_str!("../tool_descriptions/instructions.txt");
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ObserveParams {
    #[schemars(
        description = "Filter by observation kind: log, query, http_exchange, exception, js_exception, lifecycle, state_snapshot, metric, custom. Browser console output is 'log', browser JS errors are 'js_exception', page load events are 'lifecycle', network requests are 'http_exchange'."
    )]
    pub kinds: Option<Vec<String>>,

    #[schemars(description = "Minimum severity threshold: trace, debug, info, warn, error")]
    pub severity_min: Option<String>,

    #[schemars(
        description = "Filter by origin pattern: 'app' or 'app:name' for applications, 'browser' or 'browser:tab_id' for browser tabs, 'device' or 'device:serial' for devices. Omit to see all origins."
    )]
    pub origins: Option<Vec<String>>,

    #[schemars(description = "Substring search across observation data")]
    pub text_match: Option<String>,

    #[schemars(description = "Return only observations after this checkpoint id")]
    pub since_checkpoint: Option<u64>,

    #[schemars(description = "Maximum number of results to return (default 50)")]
    pub limit: Option<usize>,

    #[schemars(description = "Filter by correlation ID (exact match)")]
    pub correlation_id: Option<String>,

    #[schemars(description = "Filter by tags (all listed tags must be present)")]
    pub tags: Option<Vec<String>>,

    #[schemars(
        description = "Include system/infrastructure observations (tagged '_system'). These are excluded by default to reduce noise from CLI hooks and internal tooling."
    )]
    pub include_system: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConnectParams {
    #[schemars(description = "Browser DevTools endpoint URL (default http://localhost:9222)")]
    pub endpoint: String,
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
        description = "Observation kind: log, metric, query, exception, custom. Defaults to log."
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
        description = "Filter by origin: 'app' or 'app:name' for applications. Omit for all origins."
    )]
    pub origins: Option<Vec<String>>,

    #[schemars(description = "Substring match in observation data. Omit for no text filtering.")]
    pub text_match: Option<String>,

    #[schemars(description = "Filter by correlation ID (exact match)")]
    pub correlation_id: Option<String>,

    #[schemars(description = "Filter by tags (all listed tags must be present)")]
    pub tags: Option<Vec<String>>,

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

    #[schemars(description = "Substring search across observation data")]
    pub text_match: Option<String>,

    #[schemars(description = "Filter by correlation ID (exact match)")]
    pub correlation_id: Option<String>,

    #[schemars(description = "Filter by tags (all listed tags must be present)")]
    pub tags: Option<Vec<String>>,

    #[schemars(description = "Maximum observations to buffer (default 200)")]
    pub capacity: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SaveMemoryParams {
    #[schemars(description = "The memory content to persist")]
    pub content: String,

    #[schemars(
        description = "Memory kind: pattern, decision, error_signature, session_summary, user_flagged. Defaults to user_flagged."
    )]
    pub kind: Option<String>,

    #[schemars(description = "Tags for categorization and retrieval")]
    pub tags: Option<Vec<String>>,

    #[schemars(description = "Observation IDs that informed this memory")]
    pub source_observations: Option<Vec<u64>>,

    #[schemars(description = "Project slug to scope this memory to")]
    pub project_slug: Option<String>,

    #[schemars(description = "Session ID that produced this memory")]
    pub session_id: Option<String>,

    #[schemars(description = "Confidence score from 0.0 to 1.0 (default 1.0)")]
    pub confidence: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QueryMemoryParams {
    #[schemars(description = "Substring search across memory content")]
    pub text: Option<String>,

    #[schemars(
        description = "Filter by kind: pattern, decision, error_signature, session_summary, user_flagged"
    )]
    pub kinds: Option<Vec<String>>,

    #[schemars(description = "Filter by tags (all listed tags must be present)")]
    pub tags: Option<Vec<String>>,

    #[schemars(description = "Filter by project slug")]
    pub project_slug: Option<String>,

    #[schemars(description = "Maximum number of results (default 20)")]
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ForgetMemoryParams {
    #[schemars(description = "The memory ID to delete")]
    pub id: String,
    #[schemars(
        description = "Required to confirm deletion. Must be true to delete the memory; any other value (including absent) returns an error and leaves the memory intact."
    )]
    pub confirm: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SetupToolAction {
    #[schemars(description = "Setup action: status, plan, or apply.")]
    pub action: String,
    #[schemars(description = "Project working directory. Defaults to daemon current directory.")]
    pub cwd: Option<String>,
    #[schemars(description = "Required to confirm mutating setup_apply.")]
    pub yes: Option<bool>,
    #[schemars(description = "Comma-separated providers to configure during setup_apply.")]
    pub providers: Option<String>,
    #[schemars(description = "Hook install scope for setup_apply: local, shared, or global.")]
    pub install_hooks: Option<String>,
    #[schemars(description = "Replace stale daemon8 hook entries during setup_apply.")]
    pub force_hooks: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetupStatusParams {
    #[schemars(description = "Project working directory. Defaults to daemon current directory.")]
    pub cwd: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetupPlanParams {
    #[schemars(description = "Project working directory. Defaults to daemon current directory.")]
    pub cwd: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetupApplyParams {
    #[schemars(description = "Project working directory. Defaults to daemon current directory.")]
    pub cwd: Option<String>,
    #[schemars(description = "Required to confirm setup_apply writes.")]
    pub yes: bool,
    #[schemars(description = "Comma-separated providers to configure.")]
    pub providers: Option<String>,
    #[schemars(description = "Hook install scope: local, shared, or global.")]
    pub install_hooks: Option<String>,
    #[schemars(description = "Replace stale daemon8 hook entries.")]
    pub force_hooks: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Deliber8InboxParams {
    #[schemars(description = "Inbox address to query, e.g. 'agent:my-specialist'")]
    pub address: String,
    #[schemars(
        description = "Filter by envelope status (queued, delivered, read, failed, cancelled). Omit for all statuses."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statuses: Option<Vec<String>>,
    #[schemars(description = "Maximum envelopes to return (default 20).")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Deliber8RosterParams {
    #[schemars(
        description = "Filter by agent kind (specialist, steward, bookkeeper). Applied in-process after the store query. Omit for all kinds."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<String>>,
    #[schemars(
        description = "Filter by agent status (alive, retired). Defaults to alive only when omitted."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statuses: Option<Vec<String>>,
    #[schemars(description = "Scope to one project ref (exact match).")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_ref: Option<String>,
    #[schemars(description = "Scope to one team ref (exact match).")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_ref: Option<String>,
    #[schemars(description = "Maximum cards to return (default 50, capped at 500).")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemorySweepShortParams {
    #[schemars(
        description = "Restrict the sweep to one agent's working memory. Omit to sweep every agent's expired short-tier rows in one pass."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[schemars(
        description = "Required to actually delete. When false (default), the sweep runs as a dry-run and only reports what would be removed."
    )]
    #[serde(default)]
    pub apply: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemoryDedupeLongParams {
    #[schemars(
        description = "Optional memory scope (e.g. \"global\", \"project:slug\", \"agent:slug\"). Omit to dedupe across all scopes."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[schemars(
        description = "Required to actually delete duplicate rows. When false (default), the dedupe runs as a dry-run and only reports redundant ids."
    )]
    #[serde(default)]
    pub apply: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QueryMemoryTierParams {
    #[schemars(
        description = "Tier to query: \"short\" (TTL-bound working memory), \"reference\" (durable external sources), or \"long\" (durable distilled knowledge)."
    )]
    pub tier: String,
    #[schemars(description = "Filter by agent_id (short tier only).")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[schemars(
        description = "Filter by memory scope (e.g. \"global\", \"project:slug\", \"agent:slug\")."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[schemars(description = "Filter by tags (any-match).")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags_any: Option<Vec<String>>,
    #[schemars(description = "Filter by exact embedding profile id.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_profile_id: Option<String>,
    #[schemars(
        description = "Short tier only: include rows whose `expires_at` has passed. Default false."
    )]
    #[serde(default)]
    pub include_expired: bool,
    #[schemars(
        description = "Long tier only: include rows whose `revoked_at` is set. Default false."
    )]
    #[serde(default)]
    pub include_revoked: bool,
    #[schemars(description = "Maximum rows to return. Default 20, capped at 500.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

pub type SetupToolFn =
    Arc<dyn Fn(SetupToolAction) -> Pin<Box<dyn Future<Output = String> + Send>> + Send + Sync>;

pub struct DaemonMcp {
    store: Arc<dyn StateModel>,
    memory_store: Option<Arc<dyn MemoryStore>>,
    envelope_store: Option<Arc<dyn EnvelopeStore>>,
    card_store: Option<Arc<dyn CardStore>>,
    memory_short_store: Option<Arc<dyn MemoryShortStore>>,
    memory_reference_store: Option<Arc<dyn MemoryReferenceStore>>,
    memory_long_store: Option<Arc<dyn MemoryLongStore>>,
    bookkeeper_store: Option<Arc<dyn BookkeeperStore>>,
    #[allow(dead_code)] // consumed by embedding-profile MCP tools in the next slice
    embedding_profile_store: Option<Arc<dyn EmbeddingProfileStore>>,
    obs_tx: tokio::sync::mpsc::UnboundedSender<Observation>,
    chrome_tx: tokio::sync::mpsc::Sender<ChromeCommand>,
    chrome_state: tokio::sync::watch::Receiver<daemon8_chrome::ConnectionState>,
    chrome_endpoint: Arc<Mutex<Option<Arc<str>>>>,
    last_checkpoint: Mutex<Checkpoint>,
    device_screenshot_fn: Option<DeviceScreenshotFn>,
    screenshot_dir: std::path::PathBuf,
    subscription_tx: Arc<tokio::sync::watch::Sender<Option<Filter>>>,
    broadcast_tx: broadcast::Sender<(Arc<Observation>, Arc<str>)>,
    lens: Arc<LensManager>,
    embedder: Option<Arc<dyn daemon8_embed::Embedder>>,
    setup_tool_fn: Option<SetupToolFn>,
    tool_router: ToolRouter<Self>,
}

pub struct DaemonMcpConfig {
    pub store: Arc<dyn StateModel>,
    pub memory_store: Option<Arc<dyn MemoryStore>>,
    pub envelope_store: Option<Arc<dyn EnvelopeStore>>,
    pub card_store: Option<Arc<dyn CardStore>>,
    pub memory_short_store: Option<Arc<dyn MemoryShortStore>>,
    pub memory_reference_store: Option<Arc<dyn MemoryReferenceStore>>,
    pub memory_long_store: Option<Arc<dyn MemoryLongStore>>,
    pub bookkeeper_store: Option<Arc<dyn BookkeeperStore>>,
    pub embedding_profile_store: Option<Arc<dyn EmbeddingProfileStore>>,
    pub obs_tx: tokio::sync::mpsc::UnboundedSender<Observation>,
    pub chrome_tx: tokio::sync::mpsc::Sender<ChromeCommand>,
    pub chrome_state: tokio::sync::watch::Receiver<daemon8_chrome::ConnectionState>,
    pub chrome_endpoint: Arc<Mutex<Option<Arc<str>>>>,
    pub device_screenshot_fn: Option<DeviceScreenshotFn>,
    pub screenshot_dir: std::path::PathBuf,
    pub subscription_tx: Arc<tokio::sync::watch::Sender<Option<Filter>>>,
    pub broadcast_tx: broadcast::Sender<(Arc<Observation>, Arc<str>)>,
    pub lens: Arc<LensManager>,
    pub embedder: Option<Arc<dyn daemon8_embed::Embedder>>,
    pub setup_tool_fn: Option<SetupToolFn>,
}

#[tool_router(vis = "pub")]
impl DaemonMcp {
    pub fn new(cfg: DaemonMcpConfig) -> Self {
        let mut router = Self::tool_router();
        router += Self::action_tool_router();
        router += Self::lens_tool_router();
        if cfg.memory_store.is_some() {
            router += Self::memory_tool_router();
        }
        // Either store on its own is sufficient — each tool method checks
        // its own store and returns error_json if unset, so the surface
        // degrades gracefully when only one is wired.
        if cfg.envelope_store.is_some() || cfg.card_store.is_some() {
            router += Self::deliber8_tool_router();
        }
        // Tier surface: any of the tier stores is sufficient. Each tool method
        // checks the specific store(s) it needs and returns error_json otherwise.
        if cfg.bookkeeper_store.is_some()
            || cfg.memory_short_store.is_some()
            || cfg.memory_reference_store.is_some()
            || cfg.memory_long_store.is_some()
        {
            router += Self::tier_tool_router();
        }
        if cfg.setup_tool_fn.is_some() {
            router += Self::setup_tool_router();
        }
        Self {
            store: cfg.store,
            memory_store: cfg.memory_store,
            envelope_store: cfg.envelope_store,
            card_store: cfg.card_store,
            memory_short_store: cfg.memory_short_store,
            memory_reference_store: cfg.memory_reference_store,
            memory_long_store: cfg.memory_long_store,
            bookkeeper_store: cfg.bookkeeper_store,
            embedding_profile_store: cfg.embedding_profile_store,
            obs_tx: cfg.obs_tx,
            chrome_tx: cfg.chrome_tx,
            chrome_state: cfg.chrome_state,
            chrome_endpoint: cfg.chrome_endpoint,
            last_checkpoint: Mutex::new(Checkpoint(0)),
            device_screenshot_fn: cfg.device_screenshot_fn,
            screenshot_dir: cfg.screenshot_dir,
            subscription_tx: cfg.subscription_tx,
            broadcast_tx: cfg.broadcast_tx,
            lens: cfg.lens,
            embedder: cfg.embedder,
            setup_tool_fn: cfg.setup_tool_fn,
            tool_router: router,
        }
    }

    pub fn subscription_rx(&self) -> tokio::sync::watch::Receiver<Option<Filter>> {
        self.subscription_tx.subscribe()
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

    #[doc = include_str!("../tool_descriptions/query_observations.txt")]
    #[tool(name = "query_observations")]
    async fn query_observations(&self, Parameters(params): Parameters<ObserveParams>) -> String {
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
            include_system: params.include_system,
        };

        match self.store.query(&filter).await {
            Ok(slice) => {
                let mut result = serde_json::to_value(&slice).unwrap_or_default();

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

                serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|e| error_json(&format!("serialization failed: {e}")))
            }
            Err(e) => error_json(&format!("query failed: {e}")),
        }
    }

    #[doc = include_str!("../tool_descriptions/status.txt")]
    #[tool(name = "status")]
    async fn status(&self) -> String {
        match self.store.summary().await {
            Ok(summary) => match serde_json::to_value(&summary) {
                Ok(mut val) => {
                    if let Some(obj) = val.as_object_mut() {
                        obj.insert(
                            "daemon_version".into(),
                            serde_json::Value::String(env!("CARGO_PKG_VERSION").to_string()),
                        );
                    }
                    serde_json::to_string_pretty(&val)
                        .unwrap_or_else(|e| error_json(&format!("serialization failed: {e}")))
                }
                Err(e) => error_json(&format!("serialization failed: {e}")),
            },
            Err(e) => error_json(&format!("summary failed: {e}")),
        }
    }

    #[doc = include_str!("../tool_descriptions/create_checkpoint.txt")]
    #[tool(name = "create_checkpoint")]
    async fn create_checkpoint(&self) -> String {
        let current = self.store.checkpoint().await;
        *self
            .last_checkpoint
            .lock()
            .expect("last_checkpoint mutex poisoned") = current;
        serde_json::to_string_pretty(&serde_json::json!({ "checkpoint": current.0 }))
            .unwrap_or_else(|e| error_json(&format!("serialization failed: {e}")))
    }

    #[doc = include_str!("../tool_descriptions/list_connections.txt")]
    #[tool(name = "list_connections")]
    async fn list_connections(&self) -> String {
        self.connections_json().await
    }

    #[doc = include_str!("../tool_descriptions/ingest_observation.txt")]
    #[tool(name = "ingest_observation")]
    async fn ingest_observation(&self, Parameters(params): Parameters<IngestParams>) -> String {
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
        body.insert("data".into(), params.data);

        let obs = daemon8_ingest::normalize::normalize(serde_json::Value::Object(body));
        if let Err(e) = self.obs_tx.send(obs) {
            tracing::warn!(
                origin = ?e.0.origin,
                kind = %e.0.kind.tag(),
                severity = %e.0.severity,
                "MCP ingest failed: observation channel closed"
            );
            return error_json("Daemon is shutting down.");
        }

        serde_json::to_string(&serde_json::json!({"ok": true})).unwrap_or_default()
    }

    #[doc = include_str!("../tool_descriptions/subscribe_observations.txt")]
    #[tool(name = "subscribe_observations")]
    async fn subscribe_observations(
        &self,
        Parameters(params): Parameters<SubscribeParams>,
    ) -> String {
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
            include_system: params.include_system,
        };

        let is_default = filter.kinds.is_none()
            && filter.severity_min.is_none()
            && filter.origins.is_none()
            && filter.text_match.is_none()
            && filter.correlation_id.is_none()
            && filter.tags.is_none()
            && filter.include_system.is_none();

        if is_default {
            self.subscription_tx.send_replace(None);
            serde_json::to_string(&serde_json::json!({
                "subscribed": true,
                "filter": "default (severity >= warn)"
            }))
            .unwrap_or_default()
        } else {
            self.subscription_tx.send_replace(Some(filter));
            serde_json::to_string(&serde_json::json!({
                "subscribed": true,
                "filter": "custom"
            }))
            .unwrap_or_default()
        }
    }
}

#[tool_router(router = action_tool_router, vis = "pub")]
impl DaemonMcp {
    #[doc = include_str!("../tool_descriptions/connect_browser.txt")]
    #[tool(name = "connect_browser")]
    async fn connect_browser(&self, Parameters(params): Parameters<ConnectParams>) -> String {
        self.connect_browser_inner(params).await
    }

    #[doc = include_str!("../tool_descriptions/issue_command.txt")]
    #[tool(name = "issue_command")]
    async fn issue_command(&self, Parameters(params): Parameters<ActParams>) -> String {
        self.issue_command_inner(params).await
    }
}

#[tool_router(router = lens_tool_router, vis = "pub")]
impl DaemonMcp {
    #[doc = include_str!("../tool_descriptions/set_lens.txt")]
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
            include_system: None,
        };

        let capacity = params.capacity.unwrap_or(200).min(1000);
        self.lens.set_with_capacity(filter, capacity).await;

        let status = self.lens.status().await;
        serde_json::to_string_pretty(&status)
            .unwrap_or_else(|e| error_json(&format!("serialization failed: {e}")))
    }

    #[doc = include_str!("../tool_descriptions/clear_lens.txt")]
    #[tool(name = "clear_lens")]
    async fn clear_lens(&self) -> String {
        self.lens.clear().await;
        serde_json::to_string(&serde_json::json!({"cleared": true})).unwrap_or_default()
    }

    #[doc = include_str!("../tool_descriptions/lens_status.txt")]
    #[tool(name = "lens_status")]
    async fn lens_status(&self) -> String {
        let status = self.lens.status().await;
        serde_json::to_string_pretty(&status)
            .unwrap_or_else(|e| error_json(&format!("serialization failed: {e}")))
    }
}

#[tool_router(router = memory_tool_router, vis = "pub")]
impl DaemonMcp {
    #[doc = include_str!("../tool_descriptions/save_memory.txt")]
    #[tool(name = "save_memory")]
    async fn save_memory(&self, Parameters(params): Parameters<SaveMemoryParams>) -> String {
        let mem_store = match &self.memory_store {
            Some(s) => s,
            None => return error_json("memory store not available"),
        };

        let kind = params
            .kind
            .as_deref()
            .and_then(|s| s.parse::<daemon8_types::MemoryKind>().ok())
            .unwrap_or(daemon8_types::MemoryKind::UserFlagged);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        let embedding = match &self.embedder {
            Some(embedder) => match embedder.embed(&params.content).await {
                Ok(v) if v.is_empty() => None,
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(error = %e, "embedding failed, saving without");
                    None
                }
            },
            None => None,
        };

        let memory = daemon8_store::Memory {
            id: None,
            created_at: now,
            updated_at: now,
            kind,
            content: params.content,
            embedding,
            source_observations: params.source_observations.unwrap_or_default(),
            tags: params.tags.unwrap_or_default(),
            project_slug: params.project_slug.unwrap_or_default(),
            session_id: params.session_id,
            confidence: params.confidence.unwrap_or(1.0),
        };

        match mem_store.save_memory(memory).await {
            Ok(id) => serde_json::to_string(&serde_json::json!({ "id": id })).unwrap_or_default(),
            Err(e) => error_json(&format!("save_memory failed: {e}")),
        }
    }

    #[doc = include_str!("../tool_descriptions/query_memory.txt")]
    #[tool(name = "query_memory")]
    async fn query_memory(&self, Parameters(params): Parameters<QueryMemoryParams>) -> String {
        let mem_store = match &self.memory_store {
            Some(s) => s,
            None => return error_json("memory store not available"),
        };

        let kinds = params.kinds.map(|v| {
            v.into_iter()
                .filter_map(|s| s.parse::<daemon8_types::MemoryKind>().ok())
                .collect()
        });

        let filter = daemon8_store::MemoryFilter {
            kinds,
            tags: params.tags,
            project_slug: params.project_slug,
            session_id: None,
            text_match: params.text,
            limit: Some(params.limit.unwrap_or(20).min(500) as usize),
        };

        match mem_store.query_memory(&filter).await {
            Ok(memories) => serde_json::to_string_pretty(&memories)
                .unwrap_or_else(|e| error_json(&format!("serialization failed: {e}"))),
            Err(e) => error_json(&format!("query_memory failed: {e}")),
        }
    }

    #[doc = include_str!("../tool_descriptions/forget_memory.txt")]
    #[tool(name = "forget_memory")]
    async fn forget_memory(&self, Parameters(params): Parameters<ForgetMemoryParams>) -> String {
        if let Err(msg) = check_forget_memory_confirm(params.confirm) {
            return msg;
        }

        let mem_store = match &self.memory_store {
            Some(s) => s,
            None => return error_json("memory store not available"),
        };

        match mem_store.forget_memory(&params.id).await {
            Ok(existed) => serde_json::to_string(&serde_json::json!({ "deleted": existed }))
                .unwrap_or_default(),
            Err(e) => error_json(&format!("forget_memory failed: {e}")),
        }
    }
}

// MVP-12-A1: confirmation gate parallel to setup_apply's `yes=true` requirement.
// Without this, an MCP client that misroutes a delete intent (or hallucinates a
// memory id) silently destroys data. The gate forces an explicit boolean.
fn check_forget_memory_confirm(confirm: Option<bool>) -> Result<(), String> {
    match confirm {
        Some(true) => Ok(()),
        _ => Err(error_json(
            "forget_memory requires confirm=true to delete the memory",
        )),
    }
}

#[tool_router(router = setup_tool_router, vis = "pub")]
impl DaemonMcp {
    #[doc = include_str!("../tool_descriptions/setup_status.txt")]
    #[tool(name = "setup_status")]
    async fn setup_status(&self, Parameters(params): Parameters<SetupStatusParams>) -> String {
        self.call_setup_tool(SetupToolAction {
            action: "status".into(),
            cwd: params.cwd,
            yes: None,
            providers: None,
            install_hooks: None,
            force_hooks: None,
        })
        .await
    }

    #[doc = include_str!("../tool_descriptions/setup_plan.txt")]
    #[tool(name = "setup_plan")]
    async fn setup_plan(&self, Parameters(params): Parameters<SetupPlanParams>) -> String {
        self.call_setup_tool(SetupToolAction {
            action: "plan".into(),
            cwd: params.cwd,
            yes: None,
            providers: None,
            install_hooks: None,
            force_hooks: None,
        })
        .await
    }

    #[doc = include_str!("../tool_descriptions/setup_apply.txt")]
    #[tool(name = "setup_apply")]
    async fn setup_apply(&self, Parameters(params): Parameters<SetupApplyParams>) -> String {
        self.call_setup_tool(SetupToolAction {
            action: "apply".into(),
            cwd: params.cwd,
            yes: Some(params.yes),
            providers: params.providers,
            install_hooks: params.install_hooks,
            force_hooks: params.force_hooks,
        })
        .await
    }
}

#[tool_router(router = deliber8_tool_router, vis = "pub")]
impl DaemonMcp {
    #[doc = include_str!("../tool_descriptions/deliber8_inbox.txt")]
    #[tool(name = "deliber8_inbox")]
    async fn deliber8_inbox(&self, Parameters(params): Parameters<Deliber8InboxParams>) -> String {
        let Some(envelope_store) = self.envelope_store.as_ref() else {
            return error_json("deliber8 tools not available: envelope store not configured");
        };
        deliber8_inbox_inner(envelope_store.as_ref(), params).await
    }

    #[doc = include_str!("../tool_descriptions/deliber8_roster.txt")]
    #[tool(name = "deliber8_roster")]
    async fn deliber8_roster(
        &self,
        Parameters(params): Parameters<Deliber8RosterParams>,
    ) -> String {
        let Some(card_store) = self.card_store.as_ref() else {
            return error_json("deliber8 tools not available: card store not configured");
        };
        deliber8_roster_inner(card_store.as_ref(), params).await
    }
}

#[tool_router(router = tier_tool_router, vis = "pub")]
impl DaemonMcp {
    #[doc = include_str!("../tool_descriptions/memory_sweep_short.txt")]
    #[tool(name = "memory_sweep_short")]
    async fn memory_sweep_short(
        &self,
        Parameters(params): Parameters<MemorySweepShortParams>,
    ) -> String {
        let Some(bookkeeper) = self.bookkeeper_store.as_ref() else {
            return error_json("memory_sweep_short unavailable: bookkeeper store not configured");
        };
        memory_sweep_short_inner(bookkeeper.as_ref(), params).await
    }

    #[doc = include_str!("../tool_descriptions/memory_dedupe_long.txt")]
    #[tool(name = "memory_dedupe_long")]
    async fn memory_dedupe_long(
        &self,
        Parameters(params): Parameters<MemoryDedupeLongParams>,
    ) -> String {
        let Some(bookkeeper) = self.bookkeeper_store.as_ref() else {
            return error_json("memory_dedupe_long unavailable: bookkeeper store not configured");
        };
        memory_dedupe_long_inner(bookkeeper.as_ref(), params).await
    }

    #[doc = include_str!("../tool_descriptions/query_memory_tier.txt")]
    #[tool(name = "query_memory_tier")]
    async fn query_memory_tier(
        &self,
        Parameters(params): Parameters<QueryMemoryTierParams>,
    ) -> String {
        query_memory_tier_inner(
            self.memory_short_store.as_deref(),
            self.memory_reference_store.as_deref(),
            self.memory_long_store.as_deref(),
            params,
        )
        .await
    }
}

/// Pure handler for `memory_sweep_short`. Extracted so tests can drive it
/// against an in-memory `BookkeeperStore` without spinning up DaemonMcp.
pub async fn memory_sweep_short_inner(
    bookkeeper: &dyn BookkeeperStore,
    params: MemorySweepShortParams,
) -> String {
    let scope = daemon8_store::SweepShortScope {
        agent_id: params.agent_id,
        now_ns: None,
        apply: params.apply,
    };
    match bookkeeper.sweep_short(scope).await {
        Ok(report) => serde_json::to_string(&report)
            .unwrap_or_else(|e| error_json(&format!("sweep_short serialize failed: {e}"))),
        Err(e) => error_json(&format!("sweep_short failed: {e}")),
    }
}

/// Pure handler for `memory_dedupe_long`. Extracted so tests can drive it
/// against an in-memory `BookkeeperStore` without spinning up DaemonMcp.
pub async fn memory_dedupe_long_inner(
    bookkeeper: &dyn BookkeeperStore,
    params: MemoryDedupeLongParams,
) -> String {
    let scope = match params.scope.as_deref() {
        Some(s) => match s.parse::<daemon8_types::MemoryScope>() {
            Ok(v) => Some(v),
            Err(e) => return error_json(&e),
        },
        None => None,
    };
    let dedupe_scope = daemon8_store::DedupLongScope {
        scope,
        apply: params.apply,
    };
    match bookkeeper.dedupe_long(dedupe_scope).await {
        Ok(report) => serde_json::to_string(&report)
            .unwrap_or_else(|e| error_json(&format!("dedupe_long serialize failed: {e}"))),
        Err(e) => error_json(&format!("dedupe_long failed: {e}")),
    }
}

/// Pure handler for `query_memory_tier`. Dispatches to the right tier store
/// based on `tier`. Extracted so tests can drive it against any concrete tier
/// store implementation without spinning up DaemonMcp.
pub async fn query_memory_tier_inner(
    short: Option<&dyn MemoryShortStore>,
    reference: Option<&dyn MemoryReferenceStore>,
    long: Option<&dyn MemoryLongStore>,
    params: QueryMemoryTierParams,
) -> String {
    let scope = match params.scope.as_deref() {
        Some(s) => match s.parse::<daemon8_types::MemoryScope>() {
            Ok(v) => Some(v),
            Err(e) => return error_json(&e),
        },
        None => None,
    };
    let limit = Some(params.limit.unwrap_or(20).min(500));

    match params.tier.to_ascii_lowercase().as_str() {
        "short" => {
            let Some(store) = short else {
                return error_json("query_memory_tier: memory_short_store not configured");
            };
            let filter = daemon8_store::MemoryShortFilter {
                agent_id: params.agent_id.clone(),
                thread_id: None,
                scope,
                content_kinds: None,
                tags_any: params.tags_any.clone(),
                embedding_profile_id: params.embedding_profile_id.clone(),
                include_expired: params.include_expired,
                now_ns: None,
                limit,
            };
            match store.list(&filter).await {
                Ok(rows) => serde_json::json!({
                    "tier": "short",
                    "total": rows.len(),
                    "records": rows,
                })
                .to_string(),
                Err(e) => error_json(&format!("memory_short list failed: {e}")),
            }
        }
        "reference" => {
            let Some(store) = reference else {
                return error_json("query_memory_tier: memory_reference_store not configured");
            };
            let filter = daemon8_store::MemoryReferenceFilter {
                source_kinds: None,
                scope,
                tags_any: params.tags_any.clone(),
                embedding_profile_id: params.embedding_profile_id.clone(),
                limit,
            };
            match store.list(&filter).await {
                Ok(rows) => serde_json::json!({
                    "tier": "reference",
                    "total": rows.len(),
                    "records": rows,
                })
                .to_string(),
                Err(e) => error_json(&format!("memory_reference list failed: {e}")),
            }
        }
        "long" => {
            let Some(store) = long else {
                return error_json("query_memory_tier: memory_long_store not configured");
            };
            let filter = daemon8_store::MemoryLongFilter {
                scope,
                content_kinds: None,
                tags_any: params.tags_any.clone(),
                embedding_profile_id: params.embedding_profile_id.clone(),
                include_revoked: params.include_revoked,
                limit,
            };
            match store.list(&filter).await {
                Ok(rows) => serde_json::json!({
                    "tier": "long",
                    "total": rows.len(),
                    "records": rows,
                })
                .to_string(),
                Err(e) => error_json(&format!("memory_long list failed: {e}")),
            }
        }
        other => error_json(&format!(
            "unknown tier: {other} (expected short, reference, or long)"
        )),
    }
}

/// Pure handler for `deliber8_roster`. Same shape as `deliber8_inbox_inner` —
/// extracted so tests can drive it without the full DaemonMcp harness.
pub async fn deliber8_roster_inner(
    card_store: &dyn CardStore,
    params: Deliber8RosterParams,
) -> String {
    let parsed_kinds = match parse_agent_kinds(params.kinds.as_deref()) {
        Ok(v) => v,
        Err(e) => return error_json(&e),
    };
    let parsed_statuses = match parse_agent_statuses(params.statuses.as_deref()) {
        Ok(v) => v.or_else(|| Some(vec![daemon8_types::AgentStatus::Alive])),
        Err(e) => return error_json(&e),
    };

    // Kind filter runs in-process AFTER the store query, so we pull a wider slice
    // from the store and then narrow + truncate here. Otherwise a kind=bookkeeper
    // query with limit=50 could return zero rows even when bookkeepers exist —
    // the store would yield 50 specialists first.
    let filter = daemon8_store::AgentCardFilter {
        statuses: parsed_statuses,
        project_ref: params.project_ref.clone(),
        team_ref: params.team_ref.clone(),
        limit: None,
    };

    let cards = match card_store.list_agents(&filter).await {
        Ok(v) => v,
        Err(e) => return error_json(&format!("list_agents failed: {e}")),
    };

    let cap = params.limit.unwrap_or(50).min(500);
    let agents: Vec<&daemon8_types::AgentCard> = match parsed_kinds.as_ref() {
        Some(kinds) => cards
            .iter()
            .filter(|c| kinds.contains(&c.agent_kind))
            .take(cap)
            .collect(),
        None => cards.iter().take(cap).collect(),
    };

    serde_json::json!({
        "total": agents.len(),
        "agents": agents,
    })
    .to_string()
}

fn parse_agent_kinds(
    raw: Option<&[String]>,
) -> Result<Option<Vec<daemon8_types::AgentKind>>, String> {
    let Some(list) = raw else { return Ok(None) };
    if list.is_empty() {
        return Ok(None);
    }
    let mut parsed = Vec::with_capacity(list.len());
    for s in list {
        match s.parse::<daemon8_types::AgentKind>() {
            Ok(v) => parsed.push(v),
            Err(_) => return Err(format!("unknown agent kind: {s}")),
        }
    }
    Ok(Some(parsed))
}

fn parse_agent_statuses(
    raw: Option<&[String]>,
) -> Result<Option<Vec<daemon8_types::AgentStatus>>, String> {
    let Some(list) = raw else { return Ok(None) };
    if list.is_empty() {
        return Ok(None);
    }
    let mut parsed = Vec::with_capacity(list.len());
    for s in list {
        match s.parse::<daemon8_types::AgentStatus>() {
            Ok(v) => parsed.push(v),
            Err(_) => return Err(format!("unknown agent status: {s}")),
        }
    }
    Ok(Some(parsed))
}

/// Pure handler for `deliber8_inbox`. Extracted from the MCP tool method so
/// tests can drive it against an in-memory `EnvelopeStore` without spinning
/// up the full DaemonMcp harness.
pub async fn deliber8_inbox_inner(
    envelope_store: &dyn EnvelopeStore,
    params: Deliber8InboxParams,
) -> String {
    if params.address.trim().is_empty() {
        return error_json("deliber8_inbox requires a non-empty address");
    }

    let parsed_statuses = match parse_envelope_statuses(params.statuses.as_deref()) {
        Ok(v) => v,
        Err(e) => return error_json(&e),
    };

    let filter = daemon8_store::EnvelopeFilter {
        inbox_address: Some(params.address.clone()),
        statuses: parsed_statuses,
        limit: Some(params.limit.unwrap_or(20).min(500)),
        ..Default::default()
    };

    let envelopes = match envelope_store.query_inbox(&filter).await {
        Ok(v) => v,
        Err(e) => return error_json(&format!("query_inbox failed: {e}")),
    };

    let mut queued = 0usize;
    let mut delivered = 0usize;
    let mut read = 0usize;
    let mut failed = 0usize;
    let mut expired = 0usize;
    let mut cancelled = 0usize;
    for env in &envelopes {
        match env.status {
            daemon8_types::EnvelopeStatus::Queued => queued += 1,
            daemon8_types::EnvelopeStatus::Delivered => delivered += 1,
            daemon8_types::EnvelopeStatus::Read => read += 1,
            daemon8_types::EnvelopeStatus::Failed => failed += 1,
            daemon8_types::EnvelopeStatus::Expired => expired += 1,
            daemon8_types::EnvelopeStatus::Cancelled => cancelled += 1,
        }
    }

    serde_json::json!({
        "address": params.address,
        "total": envelopes.len(),
        "queued": queued,
        "delivered": delivered,
        "read": read,
        "failed": failed,
        "expired": expired,
        "cancelled": cancelled,
        "envelopes": envelopes,
    })
    .to_string()
}

fn parse_envelope_statuses(
    raw: Option<&[String]>,
) -> Result<Option<Vec<daemon8_types::EnvelopeStatus>>, String> {
    let Some(list) = raw else { return Ok(None) };
    if list.is_empty() {
        return Ok(None);
    }
    let mut parsed = Vec::with_capacity(list.len());
    for s in list {
        match s.parse::<daemon8_types::EnvelopeStatus>() {
            Ok(v) => parsed.push(v),
            Err(_) => return Err(format!("unknown envelope status: {s}")),
        }
    }
    Ok(Some(parsed))
}

// Command handler implementations (inner methods, not registered with tool_router).
impl DaemonMcp {
    async fn call_setup_tool(&self, action: SetupToolAction) -> String {
        match &self.setup_tool_fn {
            Some(f) => f(action).await,
            None => error_json("setup tools not available"),
        }
    }

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
                serde_json::to_string(&serde_json::json!({
                    "status": "connecting",
                    "endpoint": endpoint,
                }))
                .unwrap_or_default()
            }
            Err(_) => {
                tracing::warn!(endpoint = %endpoint, "browser connect command rejected: daemon shutting down");
                error_json("Daemon is shutting down.")
            }
        }
    }

    async fn issue_command_inner(&self, params: ActParams) -> String {
        use daemon8_chrome::BrowserAction;

        // Device screenshot: bypass Chrome entirely
        if params.action == DebugAction::Screenshot && params.device_serial.is_some() {
            return self.handle_device_screenshot(&params).await;
        }

        if let Err(e) = self
            .ensure_chrome_connected(std::time::Duration::from_secs(10))
            .await
        {
            return error_json(&e);
        }

        let (reply_tx, reply_rx) =
            tokio::sync::oneshot::channel::<Result<serde_json::Value, anyhow::Error>>();

        // Build the BrowserAction and a mapper that converts the typed reply
        // into a uniform serde_json::Value result for the wrapper oneshot.
        let action: BrowserAction = match params.action {
            DebugAction::EvalJs => {
                let expression = match params.expression {
                    Some(expr) => expr,
                    None => return error_json("eval_js requires 'expression' parameter"),
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
                    None => return error_json("inject_css requires 'css' parameter"),
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
                    None => return error_json("navigate requires 'url' parameter"),
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
                    None => return error_json("storage_set requires 'store_type' parameter"),
                };
                let key = match params.storage_key {
                    Some(k) => k,
                    None => return error_json("storage_set requires 'storage_key' parameter"),
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
                    None => return error_json("element_at_point requires 'x' parameter"),
                };
                let y = match params.y {
                    Some(v) => v,
                    None => return error_json("element_at_point requires 'y' parameter"),
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
                    None => return error_json("close_tab requires 'tab_id' parameter"),
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
            return error_json("Daemon is shutting down.");
        }

        match tokio::time::timeout(std::time::Duration::from_secs(30), reply_rx).await {
            Err(_) => error_json(
                "Browser action timed out (30s). The daemon is still connected and will recover.",
            ),
            Ok(Ok(Ok(value))) => serde_json::to_string(&value).unwrap_or_default(),
            Ok(Ok(Err(e))) => error_json(&format!("{e}")),
            Ok(Err(_)) => error_json(
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
                return error_json("device screenshots not available (ADB not enabled)");
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
                error_json("device screenshot timed out (15s)")
            }
            Ok(Err(e)) => {
                tracing::warn!(serial, platform = ?platform, error = %e, "device screenshot failed");
                error_json(&format!("device screenshot failed for {serial}: {e}"))
            }
            Ok(Ok(shot)) => {
                let path = screenshot_path(&self.screenshot_dir, &serial, Some(&shot.source));
                if let Err(e) = std::fs::write(&path, &shot.png_bytes) {
                    tracing::warn!(serial, path = %path.display(), error = %e, "failed to write device screenshot");
                    return error_json(&format!("failed to write screenshot: {e}"));
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

fn error_json(msg: &str) -> String {
    serde_json::to_string(&serde_json::json!({ "error": msg })).unwrap_or_default()
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

        serde_json::to_string_pretty(&result)
            .unwrap_or_else(|e| error_json(&format!("serialization failed: {e}")))
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
        let session_id = next_mcp_session_id();
        tracing::info!(
            session_id,
            "MCP session initialized, starting observation push"
        );
        let peer = context.peer;
        let mut rx = self.broadcast_tx.subscribe();
        let sub_rx = self.subscription_tx.subscribe();
        let span = tracing::info_span!("mcp_session", session_id = %session_id);

        tokio::spawn(async move {
            let mut last_push = std::time::Instant::now() - Duration::from_secs(2);
            loop {
                let (arc_obs, _json) = match rx.recv().await {
                    Ok(payload) => payload,
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "MCP observation push receiver lagged; continuing with newest observations");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::debug!("MCP observation push broadcast closed");
                        break;
                    }
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

    #[test]
    fn forget_memory_gate_rejects_missing_confirm() {
        let result = check_forget_memory_confirm(None);
        let err = result.expect_err("missing confirm should error");
        assert!(
            err.contains("confirm=true"),
            "error message should name the required field: {err}"
        );
    }

    #[test]
    fn forget_memory_gate_rejects_confirm_false() {
        let result = check_forget_memory_confirm(Some(false));
        assert!(result.is_err(), "confirm=false should error");
    }

    #[test]
    fn forget_memory_gate_accepts_confirm_true() {
        let result = check_forget_memory_confirm(Some(true));
        assert!(result.is_ok(), "confirm=true should pass");
    }

    #[test]
    fn forget_memory_params_parses_without_confirm() {
        let p: ForgetMemoryParams = serde_json::from_str(r#"{"id":"abc"}"#).unwrap();
        assert_eq!(p.id, "abc");
        assert_eq!(p.confirm, None);
    }

    #[test]
    fn forget_memory_params_parses_with_confirm_true() {
        let p: ForgetMemoryParams = serde_json::from_str(r#"{"id":"abc","confirm":true}"#).unwrap();
        assert_eq!(p.confirm, Some(true));
    }

    #[test]
    fn forget_memory_params_parses_with_confirm_false() {
        let p: ForgetMemoryParams =
            serde_json::from_str(r#"{"id":"abc","confirm":false}"#).unwrap();
        assert_eq!(p.confirm, Some(false));
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
}
