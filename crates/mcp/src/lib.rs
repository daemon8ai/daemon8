// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use daemon8_store::{
    ActiveSessionState, AwarenessEvidence, AwarenessFilter, AwarenessStore, AwarenessSync,
    AwarenessTraversalFilter, DebugSessionStore, LensManager, LibrarianFilter, LibrarianStore,
    MemoryStore, StateModel,
};
use daemon8_types::{
    AwarenessAuthority, AwarenessNodeKind, AwarenessOperation, Checkpoint, DevicePlatform, Filter,
    LibrarianNodeKind, LocatorKind, Observation, ProjectClassification, SourceActivator,
};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerInfo, Tool};
use rmcp::schemars::{self, JsonSchema};
use rmcp::{RoleServer, ServerHandler, tool, tool_router};
use serde::Deserialize;
use tokio::sync::RwLock;
use tokio::sync::broadcast;
use tracing::Instrument;

pub mod envelope;
pub mod help;
pub mod hints;
use envelope::{ActiveSessionEcho, DaemonMeta};
use help::FeatureGate;

/// Shared handle to the daemon's most recently classified project. The
/// scanner (D3) writes here on every serve; MCP handlers read from it
/// to scope librarian-aware hints. None means no project has been
/// classified this run (CI/non-project use; first invocation before
/// the scanner finishes).
pub type ActiveProjectHandle = Arc<RwLock<Option<ProjectClassification>>>;

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
        description = "Filter by observation kind: log, query, http_exchange, exception, js_exception, lifecycle, state_snapshot, metric, custom, tool_call. Browser console output is 'log', browser JS errors are 'js_exception', page load events are 'lifecycle', network requests are 'http_exchange'. Observations with kind=custom and channel='discovery_hint' are project-onboarding hints; see librarian_index for the response shape expected."
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

    #[schemars(
        description = "Include system/infrastructure observations (tagged '_system'). These are excluded by default to reduce noise from internal tooling."
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HelpParams {
    #[schemars(
        description = "Help topic: index, awareness, debug_session, checkpoint, setup, lens, observations, envelope, librarian. Omit for index."
    )]
    pub topic: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct AwarenessStatusParams {
    #[schemars(description = "Optional awareness path to inspect. Omit for a compact manifest.")]
    pub focus_path: Option<String>,
    #[schemars(description = "Traversal depth for focused reads. Default 1, max 5.")]
    pub depth: Option<u32>,
    #[schemars(description = "Include notes and Redex notation in focused output. Default false.")]
    pub include_notes: Option<bool>,
    #[schemars(description = "Include evidence refs in focused output. Default false.")]
    pub include_evidence: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AwarenessEvidenceParams {
    #[schemars(description = "Observation sequence ids that support this awareness update.")]
    pub observation_ids: Option<Vec<u64>>,
    #[schemars(description = "Debug session ids related to this awareness update.")]
    pub debug_session_ids: Option<Vec<String>>,
    #[schemars(description = "Checkpoint ids related to this awareness update.")]
    pub checkpoint_ids: Option<Vec<String>>,
    #[schemars(description = "Librarian node ids related to this awareness update.")]
    pub librarian_node_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AwarenessSyncParams {
    #[schemars(
        description = "Reasoning operation: capture, update, question, resolve, verify, or retire."
    )]
    pub operation: String,
    #[schemars(description = "Hierarchical awareness path, e.g. debug/frontend/root-cause.")]
    pub path: String,
    #[schemars(
        description = "Node kind: objective, question, hypothesis, fact, decision, constraint, risk, or blocker."
    )]
    pub kind: String,
    #[schemars(
        description = "Project slug. Defaults to active debug session project when omitted."
    )]
    pub project_slug: Option<String>,
    #[schemars(description = "Authority: verified, accepted, inferred, hypothesis, or question.")]
    pub authority: Option<String>,
    #[schemars(description = "Confidence from 0.0 to 1.0.")]
    pub confidence: Option<f64>,
    #[schemars(description = "Compact user-facing summary of the awareness node.")]
    pub summary: Option<String>,
    #[schemars(description = "Short internal note. Not returned by default status reads.")]
    pub note: Option<String>,
    #[schemars(
        description = "Optional compact Redex notation. Stored raw and lightly validated later."
    )]
    pub redex: Option<String>,
    #[schemars(description = "Free-form tags for focused retrieval.")]
    pub tags: Option<Vec<String>>,
    #[schemars(description = "Debug session id. Defaults to active debug session when omitted.")]
    pub debug_session_id: Option<String>,
    #[schemars(description = "Checkpoint id connected to this update.")]
    pub checkpoint_id: Option<String>,
    #[schemars(description = "Evidence references for this awareness update.")]
    pub evidence: Option<AwarenessEvidenceParams>,
    #[schemars(description = "Existing awareness node id for update/resolve/verify/retire.")]
    pub target_node_id: Option<String>,
    #[schemars(description = "Existing node ids this node supersedes.")]
    pub supersedes: Option<Vec<String>>,
    #[schemars(description = "Question/blocker node ids this node answers.")]
    pub answers: Option<Vec<String>>,
    #[schemars(description = "Existing node ids this node contradicts.")]
    pub contradicts: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LibrarianIndexEdge {
    #[schemars(
        description = "Edge kind: has_source | documented_by | fixes | supersedes | child_of"
    )]
    pub kind: String,
    #[schemars(description = "Target catalog node ID for this edge")]
    pub target_node_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LibrarianIndexParams {
    #[schemars(description = "Node kind: doc | source_template | fix | project")]
    pub kind: String,
    #[schemars(description = "Human-readable name for this reference")]
    pub label: String,
    #[schemars(description = "Locator type: file | url | vault")]
    pub locator_kind: String,
    #[schemars(description = "The actual pointer — a file path, URL, or vault note path")]
    pub locator: String,
    #[schemars(description = "Free-form retrieval tags")]
    pub tags: Option<Vec<String>>,
    #[schemars(description = "Project slug to scope this reference")]
    pub project_slug: Option<String>,
    #[schemars(description = "Place under an existing catalog node for hierarchy")]
    pub parent_id: Option<String>,
    #[schemars(description = "Optional edge to create at index time")]
    pub edge: Option<LibrarianIndexEdge>,
    #[schemars(
        description = "Mark as authoritative reference — canonicalized nodes are never flagged as stale"
    )]
    pub canonicalize: Option<bool>,
    #[schemars(
        description = "Kind-specific payload (snake_case JSON). For kind=source_template pass a SourceTemplateData shape; for kind=project pass a ProjectNodeData shape. See daemon8-types for the exact fields."
    )]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LibrarianLookupParams {
    #[schemars(description = "Look up a single node by ID (returns node + edges)")]
    pub id: Option<String>,
    #[schemars(description = "Filter by kind: doc | source_template | fix | project")]
    pub kinds: Option<Vec<String>>,
    #[schemars(description = "Filter by tags")]
    pub tags: Option<Vec<String>>,
    #[schemars(description = "Scope to a project")]
    pub project_slug: Option<String>,
    #[schemars(description = "Case-insensitive search across label and locator")]
    pub text: Option<String>,
    #[schemars(description = "Max results. Default 20, max 500.")]
    pub limit: Option<u32>,
    #[schemars(description = "Include superseded/deprecated entries. Default false.")]
    pub include_deprecated: Option<bool>,
    #[schemars(description = "Find nodes not accessed in N days")]
    pub stale_before_days: Option<u32>,
    #[schemars(description = "Browse children of a specific catalog node")]
    pub parent_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LibrarianForgetParams {
    #[schemars(description = "Catalog node ID to remove or deprecate")]
    pub id: String,
    #[schemars(
        description = "Required for hard delete (deprecate=false). Must be true to proceed."
    )]
    pub confirm: Option<bool>,
    #[schemars(
        description = "Default true. When true: soft-delete (deprecated_at set). When false and confirm=true: permanent removal."
    )]
    pub deprecate: Option<bool>,
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

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SetupToolAction {
    #[schemars(description = "Setup action: status or apply.")]
    pub action: String,
    #[schemars(description = "Project working directory. Defaults to daemon current directory.")]
    pub cwd: Option<String>,
    #[schemars(description = "Required to confirm mutating setup_apply.")]
    pub yes: Option<bool>,
    #[schemars(
        description = "Comma-separated providers to configure (e.g. \"claude-code,gemini,codex\"). Omit for auto-detection."
    )]
    pub providers: Option<String>,
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
    #[schemars(
        description = "Comma-separated providers to configure (e.g. \"claude-code,gemini,codex\"). Omit for auto-detection."
    )]
    pub providers: Option<String>,
}

pub type SetupToolFn =
    Arc<dyn Fn(SetupToolAction) -> Pin<Box<dyn Future<Output = String> + Send>> + Send + Sync>;

pub struct DaemonMcp {
    store: Arc<dyn StateModel>,
    memory_store: Option<Arc<dyn MemoryStore>>,
    debug_session_store: Option<Arc<dyn DebugSessionStore>>,
    librarian_store: Option<Arc<dyn LibrarianStore>>,
    awareness_store: Option<Arc<dyn AwarenessStore>>,
    active_state: ActiveSessionState,
    obs_tx: tokio::sync::mpsc::UnboundedSender<Observation>,
    chrome_tx: tokio::sync::mpsc::Sender<ChromeCommand>,
    chrome_state: tokio::sync::watch::Receiver<daemon8_chrome::ConnectionState>,
    chrome_endpoint: Arc<Mutex<Option<Arc<str>>>>,
    last_checkpoint: Mutex<Checkpoint>,
    device_screenshot_fn: Option<DeviceScreenshotFn>,
    screenshot_dir: std::path::PathBuf,
    /// Per-session subscription filter. Each `DaemonMcp` instance owns its own
    /// channel so concurrent MCP sessions do not overwrite each other's
    /// `subscribe_observations` filter.
    subscription_tx: tokio::sync::watch::Sender<Option<Filter>>,
    broadcast_tx: broadcast::Sender<(Arc<Observation>, Arc<str>)>,
    lens: Arc<LensManager>,
    setup_tool_fn: Option<SetupToolFn>,
    source_activator: Option<Arc<dyn SourceActivator>>,
    /// Parent cancellation token. The push task spawned in `on_initialized`
    /// uses a child token so daemon shutdown propagates cleanly.
    cancel: tokio_util::sync::CancellationToken,
    enabled_features: Vec<FeatureGate>,
    /// Shared with the discovery scanner — read by `query_observations`
    /// to scope librarian-aware path hints. `None` when no project has
    /// been classified this run.
    active_project: ActiveProjectHandle,
    tool_router: ToolRouter<Self>,
}

pub struct DaemonMcpConfig {
    pub store: Arc<dyn StateModel>,
    pub memory_store: Option<Arc<dyn MemoryStore>>,
    pub debug_session_store: Option<Arc<dyn DebugSessionStore>>,
    pub librarian_store: Option<Arc<dyn LibrarianStore>>,
    pub awareness_store: Option<Arc<dyn AwarenessStore>>,
    pub obs_tx: tokio::sync::mpsc::UnboundedSender<Observation>,
    pub chrome_tx: tokio::sync::mpsc::Sender<ChromeCommand>,
    pub chrome_state: tokio::sync::watch::Receiver<daemon8_chrome::ConnectionState>,
    pub chrome_endpoint: Arc<Mutex<Option<Arc<str>>>>,
    pub device_screenshot_fn: Option<DeviceScreenshotFn>,
    pub screenshot_dir: std::path::PathBuf,
    pub broadcast_tx: broadcast::Sender<(Arc<Observation>, Arc<str>)>,
    pub lens: Arc<LensManager>,
    pub setup_tool_fn: Option<SetupToolFn>,
    pub source_activator: Option<Arc<dyn SourceActivator>>,
    /// Parent cancellation token. Use the daemon-wide token; the MCP push task
    /// derives a child from it so daemon shutdown stops per-session work.
    pub cancel: tokio_util::sync::CancellationToken,
    /// Shared with the discovery scanner. The daemon constructs one
    /// `Arc<RwLock<Option<ProjectClassification>>>` and hands clones
    /// to both the scanner (writer) and every MCP session (reader).
    pub active_project: ActiveProjectHandle,
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
        if cfg.setup_tool_fn.is_some() {
            router += Self::setup_tool_router();
        }
        if cfg.librarian_store.is_some() {
            router += Self::librarian_tool_router();
        }
        if cfg.awareness_store.is_some() {
            router += Self::awareness_tool_router();
        }
        let mut enabled_features = Vec::new();
        if cfg.debug_session_store.is_some() && cfg.memory_store.is_some() {
            enabled_features.push(FeatureGate::DebugSession);
        }
        if cfg.setup_tool_fn.is_some() {
            enabled_features.push(FeatureGate::Setup);
        }
        if cfg.librarian_store.is_some() {
            enabled_features.push(FeatureGate::Librarian);
        }
        let (subscription_tx, _) = tokio::sync::watch::channel::<Option<Filter>>(None);
        Self {
            store: cfg.store,
            memory_store: cfg.memory_store,
            debug_session_store: cfg.debug_session_store,
            librarian_store: cfg.librarian_store,
            awareness_store: cfg.awareness_store,
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
            lens: cfg.lens,
            setup_tool_fn: cfg.setup_tool_fn,
            source_activator: cfg.source_activator,
            cancel: cfg.cancel,
            enabled_features,
            active_project: cfg.active_project,
            tool_router: router,
        }
    }

    pub fn subscription_rx(&self) -> tokio::sync::watch::Receiver<Option<Filter>> {
        self.subscription_tx.subscribe()
    }

    /// Set this session's subscription filter directly. Equivalent to invoking
    /// the `subscribe_observations` tool — exposed for integration tests that
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
    pub fn help_index_body(&self) -> String {
        help::build_dynamic_index(&self.enabled_features, self.librarian_store.is_some())
    }

    /// Drive `query_observations` with empty parameters. Exposed for
    /// integration tests that need to inspect the rendered envelope
    /// (e.g. asserting that `daemon8.hints` is set when a path-pattern
    /// matches and absent otherwise).
    #[cfg(feature = "test-util")]
    pub async fn query_observations_for_tests(&self) -> String {
        self.query_observations(Parameters(ObserveParams::default()))
            .await
    }

    #[cfg(feature = "test-util")]
    pub fn help_topic_body(&self, topic: &str) -> (String, String) {
        match help::find_topic(topic, &self.enabled_features) {
            Some(t) => (t.name.to_string(), t.body.to_string()),
            None => (
                "index".to_string(),
                help::build_dynamic_index(&self.enabled_features, self.librarian_store.is_some()),
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

    #[doc = include_str!("../tool_descriptions/query_observations.md")]
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

        if let Some(ref sa) = self.source_activator {
            sa.touch_matching(&filter);
        }

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

                let path_hint = self.compute_path_hint(&slice.observations).await;
                if let Some(hint) = path_hint {
                    let mut meta = self.current_meta();
                    meta.hints.push(hint.hint_text);
                    if self.awareness_store.is_some()
                        && filter.since.is_some()
                        && slice.observations.iter().any(|obs| {
                            obs.severity.level() >= daemon8_types::Severity::Warn.level()
                        })
                    {
                        meta.next_actions = Some(vec!["awareness_sync".into()]);
                        meta.hint = Some("meaningful checkpoint evidence found; verify/refine/retire the related awareness node if this changes the investigation state".into());
                    }
                    return envelope::ok_value(result, meta);
                }

                if self.awareness_store.is_some()
                    && filter.since.is_some()
                    && slice
                        .observations
                        .iter()
                        .any(|obs| obs.severity.level() >= daemon8_types::Severity::Warn.level())
                {
                    return self.ok_with(
                        result,
                        vec!["awareness_sync"],
                        Some("meaningful checkpoint evidence found; verify/refine/retire the related awareness node if this changes the investigation state"),
                    );
                }

                self.ok(result)
            }
            Err(e) => self.err("query_failed", &e.to_string(), None, None),
        }
    }

    async fn compute_path_hint(
        &self,
        observations: &[Observation],
    ) -> Option<hints::PathPatternHint> {
        let (project_tags, project_root) = match self.active_project.read().await.as_ref() {
            Some(c) => (c.tags.clone(), c.root.to_str().map(|s| s.to_string())),
            None => (Vec::new(), None),
        };
        let obs_values: Vec<serde_json::Value> = observations
            .iter()
            .filter_map(|o| serde_json::to_value(o).ok())
            .collect();
        hints::maybe_emit_path_hint(
            &obs_values,
            self.librarian_store.as_ref(),
            &project_tags,
            project_root.as_deref(),
        )
        .await
    }

    #[doc = include_str!("../tool_descriptions/status.md")]
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
                    self.ok(val)
                }
                Err(e) => self.err("serialization_failed", &e.to_string(), None, None),
            },
            Err(e) => self.err("summary_failed", &e.to_string(), None, None),
        }
    }

    #[doc = include_str!("../tool_descriptions/awareness_status.md")]
    #[tool(name = "awareness_status")]
    async fn awareness_status(
        &self,
        Parameters(params): Parameters<AwarenessStatusParams>,
    ) -> String {
        let active_session = self.active_state.current_session();
        let project = self.active_project.read().await.clone();
        let project_slug = active_session
            .as_ref()
            .map(|s| s.project_slug.to_string())
            .filter(|s| s != "unknown")
            .or_else(|| {
                project
                    .as_ref()
                    .and_then(|p| derive_slug_from_root(&p.root))
            });

        let (source_awareness, source_hint) = match self
            .source_awareness(project.as_ref(), project_slug.as_deref())
            .await
        {
            Ok(v) => v,
            Err(e) => return self.err("awareness_lookup_failed", &e.to_string(), None, None),
        };

        let project_awareness = match (&self.awareness_store, project_slug.as_deref()) {
            (None, _) => serde_json::json!({
                "available": false,
                "reason": "awareness store unavailable",
            }),
            (Some(_), None) => serde_json::json!({
                "available": false,
                "reason": "project slug unavailable",
            }),
            (Some(store), Some(slug)) => {
                if let Some(focus_path) = params.focus_path.as_deref() {
                    match store
                        .traverse(&AwarenessTraversalFilter {
                            project_slug: slug.to_string(),
                            focus_path: focus_path.to_string(),
                            depth: params.depth.unwrap_or(1).min(5) as usize,
                            include_inactive: false,
                            include_notes: params.include_notes.unwrap_or(false),
                            include_evidence: params.include_evidence.unwrap_or(false),
                            limit: Some(100),
                        })
                        .await
                    {
                        Ok(tree) => serde_json::json!({
                            "available": true,
                            "mode": "focused",
                            "tree": tree,
                        }),
                        Err(e) => {
                            return self.err(
                                "awareness_traverse_failed",
                                &e.to_string(),
                                None,
                                None,
                            );
                        }
                    }
                } else {
                    match store
                        .manifest(&AwarenessFilter {
                            project_slug: slug.to_string(),
                            include_inactive: false,
                            limit: Some(500),
                        })
                        .await
                    {
                        Ok(manifest) => serde_json::json!({
                            "available": true,
                            "mode": "manifest",
                            "manifest": manifest,
                        }),
                        Err(e) => {
                            return self.err(
                                "awareness_manifest_failed",
                                &e.to_string(),
                                None,
                                None,
                            );
                        }
                    }
                }
            }
        };

        let next_actions = match (&self.awareness_store, params.focus_path.is_some()) {
            (Some(_), true) => vec!["awareness_sync", "create_checkpoint"],
            (Some(_), false) => vec!["awareness_sync", "create_checkpoint", "librarian_lookup"],
            (None, true) => vec!["create_checkpoint"],
            (None, false) => vec!["create_checkpoint", "librarian_lookup"],
        };

        self.ok_with(
            serde_json::json!({
                "project_slug": project_slug,
                "source_awareness": source_awareness,
                "project_awareness": project_awareness,
            }),
            next_actions,
            Some(source_hint.as_str()),
        )
    }

    async fn source_awareness(
        &self,
        project: Option<&ProjectClassification>,
        project_slug: Option<&str>,
    ) -> Result<(serde_json::Value, String), String> {
        let Some(lib_store) = &self.librarian_store else {
            return Ok((
                serde_json::json!({
                    "level": "limited",
                    "reason": "librarian unavailable",
                    "known_source_templates": 0,
                    "known_source_instances": 0,
                    "accessible_source_instances": 0,
                    "inaccessible_source_instances": 0,
                }),
                "limited awareness: librarian topology is unavailable, so use broad observation queries and register reusable sources when found".into(),
            ));
        };

        let Some(classification) = project else {
            return Ok((
                serde_json::json!({
                    "level": "unknown",
                    "reason": "no active project classification",
                    "known_source_templates": 0,
                    "known_source_instances": 0,
                    "accessible_source_instances": 0,
                    "inaccessible_source_instances": 0,
                }),
                "unknown awareness: check the project sitrep/librarian before trusting debug-session deltas".into(),
            ));
        };

        let templates = lib_store
            .lookup(&LibrarianFilter {
                kinds: Some(vec![LibrarianNodeKind::SourceTemplate]),
                limit: Some(500),
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|node| source_template_matches(node.data.as_ref(), classification))
            .collect::<Vec<_>>();

        let instances = if let Some(slug) = project_slug {
            lib_store
                .lookup(&LibrarianFilter {
                    kinds: Some(vec![LibrarianNodeKind::SourceInstance]),
                    project_slug: Some(slug.to_string()),
                    limit: Some(500),
                    ..Default::default()
                })
                .await
                .map_err(|e| e.to_string())?
        } else {
            Vec::new()
        };

        let inaccessible = instances
            .iter()
            .filter(|node| {
                node.locator_kind == LocatorKind::File
                    && !std::path::Path::new(&node.locator).exists()
            })
            .count();
        let accessible = instances.len().saturating_sub(inaccessible);

        let (level, reason, hint) = if !instances.is_empty() && inaccessible == 0 {
            (
                "optimal",
                "all librarian-known project source instances are accessible",
                "optimal awareness: create a checkpoint, run the action, then query observations since the checkpoint",
            )
        } else if !instances.is_empty() || !templates.is_empty() {
            (
                "partial",
                "librarian knows relevant sources, but coverage is not fully accessible for this session",
                "partial awareness: inspect source drift and activate/query relevant source tags before relying on checkpoint deltas",
            )
        } else {
            (
                "limited",
                "no matching librarian source templates or project source instances were found",
                "limited awareness: use broad observation queries and teach reusable source locations with librarian_index when discovered",
            )
        };

        Ok((
            serde_json::json!({
                "level": level,
                "reason": reason,
                "classification_tags": classification.tags,
                "platform": classification.platform,
                "known_source_templates": templates.len(),
                "known_source_instances": instances.len(),
                "accessible_source_instances": accessible,
                "inaccessible_source_instances": inaccessible,
                "cursor_status": "checkpoint_seq_available; durable source cursor ledger pending",
            }),
            hint.into(),
        ))
    }

    #[tool(
        name = "daemon8_help",
        description = "Narrative documentation for daemon8 protocols. Pass topic='index' (or omit) for the topic list. Returns markdown."
    )]
    async fn daemon8_help(&self, Parameters(params): Parameters<HelpParams>) -> String {
        let topic = params.topic.as_deref().unwrap_or("index");
        if topic == "index" {
            let body =
                help::build_dynamic_index(&self.enabled_features, self.librarian_store.is_some());
            return self.ok(serde_json::json!({ "topic": "index", "body": body }));
        }
        match help::find_topic(topic, &self.enabled_features) {
            Some(t) => self.ok(serde_json::json!({ "topic": t.name, "body": t.body })),
            None => {
                let body = help::build_dynamic_index(
                    &self.enabled_features,
                    self.librarian_store.is_some(),
                );
                self.ok(serde_json::json!({ "topic": "index", "body": body }))
            }
        }
    }

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
                    "internal_error",
                    "debug_session store not available",
                    None,
                    None,
                );
            }
        };

        let seq = self.store.checkpoint().await;
        let now = current_ns();
        let checkpoint_has_semantic_anchor = params
            .description
            .as_deref()
            .is_some_and(|desc| !desc.trim().is_empty());
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
        let next_actions = if checkpoint_has_semantic_anchor && self.awareness_store.is_some() {
            vec!["awareness_sync", "query_observations"]
        } else {
            vec!["query_observations"]
        };
        let hint = if checkpoint_has_semantic_anchor && self.awareness_store.is_some() {
            "checkpoint set; if this checkpoint tests an active question/hypothesis, link it with awareness_sync before querying observations"
        } else {
            "checkpoint set; query_observations(since_checkpoint=...) shows what comes next"
        };

        self.ok_with(
            serde_json::json!({
                "checkpoint_id": cp_id,
                "debug_session_id": active.id.as_ref(),
                "seq_at_creation": seq.0,
                "created_at": now
            }),
            next_actions,
            Some(hint),
        )
    }

    #[doc = include_str!("../tool_descriptions/list_connections.md")]
    #[tool(name = "list_connections")]
    async fn list_connections(&self) -> String {
        wrap_inner_result(self, &self.connections_json().await)
    }

    #[doc = include_str!("../tool_descriptions/ingest_observation.md")]
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

    #[doc = include_str!("../tool_descriptions/subscribe_observations.md")]
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

        if let Some(ref sa) = self.source_activator {
            sa.touch_matching(&filter);
        }

        let is_default = filter.kinds.is_none()
            && filter.severity_min.is_none()
            && filter.origins.is_none()
            && filter.text_match.is_none()
            && filter.correlation_id.is_none()
            && filter.tags.is_none()
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
        self.issue_command_inner(params).await
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
            include_system: None,
        };

        if let Some(ref sa) = self.source_activator {
            sa.touch_matching(&filter);
        }
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
            // If the inner function already produced an envelope-shaped value
            // (i.e. `error_json(...)` from inside the inner), pass it through
            // by extracting the error and rewrapping. Otherwise it's a raw
            // success payload to wrap as `result`.
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

// MVP-12-A1: confirmation gate parallel to setup_apply's `yes=true` requirement.
// Without this, an MCP client that misroutes a delete intent (or hallucinates a
// memory id) silently destroys data. The gate forces an explicit boolean.
#[cfg(test)]
fn check_forget_memory_confirm(confirm: Option<bool>) -> Result<(), String> {
    match confirm {
        Some(true) => Ok(()),
        _ => Err("forget_memory requires confirm=true to delete the memory".into()),
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
                    Some("ensure setup_apply has run"),
                    Some("setup_apply"),
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
                self.ok_with(
                    serde_json::json!({
                        "debug_session_id": id,
                        "started_at": now,
                    }),
                    vec!["awareness_status", "awareness_sync", "create_checkpoint"],
                    Some("debug session opened; check source posture and capture the objective/open questions/hypotheses if they are not already clear"),
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
                    Some("ensure setup_apply has run"),
                    Some("setup_apply"),
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
                self.ok(serde_json::json!({
                    "count": sessions.len(),
                    "sessions": sessions,
                }))
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

    // Gather source observations from this session's checkpoints. Falls back
    // to the bare seq range from start..now if checkpoint listing fails.
    let checkpoints = ds_store
        .list_checkpoints(active.id.as_ref())
        .await
        .unwrap_or_default();
    let mut source_observations: Vec<u64> =
        checkpoints.iter().map(|cp| cp.seq_at_creation).collect();

    let (outcome, summary_text, tags, data_blob) = match intent {
        EndIntent::Abandon { outcome_str } => {
            let outcome = outcome_str
                .as_deref()
                .and_then(|s| s.parse::<daemon8_types::DebugSessionOutcome>().ok())
                .unwrap_or(daemon8_types::DebugSessionOutcome::Abandoned);
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

    // Cap source_observations to the most recent 50 to avoid unbounded blobs.
    if source_observations.len() > 50 {
        let drop = source_observations.len() - 50;
        source_observations.drain(0..drop);
    }

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

    let (next_actions, hint) = if daemon.awareness_store.is_some() {
        (
            vec!["awareness_sync", "start_debug_session"],
            "session closed; use awareness_sync to close questions, verify facts, or preserve unresolved blockers before starting the next investigation",
        )
    } else {
        (
            vec!["start_debug_session", "list_debug_sessions"],
            "session closed; start_debug_session for the next investigation",
        )
    };

    daemon.ok_with(
        serde_json::json!({
            "debug_session_id": active.id.as_ref(),
            "summary_memory_id": summary_memory_id,
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

impl DaemonMcp {
    /// Build the per-tool-call meta block. Currently echoes the active debug
    /// session so the LLM sees its lifecycle context on every response.
    /// Setup-status surfacing lands in v0.4 once the synchronous probe is
    /// wired across crate boundaries; until then `setup_status` remains the
    /// explicit tool for setup state.
    pub(crate) fn current_meta(&self) -> DaemonMeta {
        let mut meta = DaemonMeta::default();
        if let Some(s) = self.active_state.current_session() {
            meta.active_debug_session = Some(ActiveSessionEcho {
                id: s.id.to_string(),
                project_slug: s.project_slug.to_string(),
                started_at_ns: s.started_at_ns,
            });
        }
        meta
    }

    pub(crate) fn current_meta_with(
        &self,
        next_actions: Vec<&str>,
        hint: Option<&str>,
    ) -> DaemonMeta {
        let mut meta = self.current_meta();
        if !next_actions.is_empty() {
            meta.next_actions = Some(next_actions.into_iter().map(String::from).collect());
        }
        if let Some(h) = hint {
            meta.hint = Some(h.to_string());
        }
        meta
    }

    pub(crate) fn ok(&self, value: serde_json::Value) -> String {
        envelope::ok_value(value, self.current_meta())
    }

    pub(crate) fn ok_with(
        &self,
        value: serde_json::Value,
        next_actions: Vec<&str>,
        hint: Option<&str>,
    ) -> String {
        envelope::ok_value(value, self.current_meta_with(next_actions, hint))
    }

    pub(crate) fn err(
        &self,
        code: &str,
        message: &str,
        hint: Option<&str>,
        fix_tool: Option<&str>,
    ) -> String {
        envelope::err(code, message, hint, fix_tool, self.current_meta())
    }
}

#[tool_router(router = setup_tool_router, vis = "pub")]
impl DaemonMcp {
    #[doc = include_str!("../tool_descriptions/setup_status.md")]
    #[tool(name = "setup_status")]
    async fn setup_status(&self, Parameters(params): Parameters<SetupStatusParams>) -> String {
        let inner = self
            .call_setup_tool(SetupToolAction {
                action: "status".into(),
                cwd: params.cwd,
                yes: None,
                providers: None,
            })
            .await;
        wrap_inner_result(self, &inner)
    }

    #[doc = include_str!("../tool_descriptions/setup_plan.md")]
    #[tool(name = "setup_plan")]
    async fn setup_plan(&self, Parameters(params): Parameters<SetupPlanParams>) -> String {
        let inner = self
            .call_setup_tool(SetupToolAction {
                action: "status".into(),
                cwd: params.cwd,
                yes: None,
                providers: None,
            })
            .await;
        wrap_inner_result(self, &inner)
    }

    #[doc = include_str!("../tool_descriptions/setup_apply.md")]
    #[tool(name = "setup_apply")]
    async fn setup_apply(&self, Parameters(params): Parameters<SetupApplyParams>) -> String {
        let inner = self
            .call_setup_tool(SetupToolAction {
                action: "apply".into(),
                cwd: params.cwd,
                yes: Some(params.yes),
                providers: params.providers,
            })
            .await;
        wrap_inner_result(self, &inner)
    }
}

#[tool_router(router = awareness_tool_router, vis = "pub")]
impl DaemonMcp {
    #[doc = include_str!("../tool_descriptions/awareness_sync.md")]
    #[tool(name = "awareness_sync")]
    async fn awareness_sync(&self, Parameters(params): Parameters<AwarenessSyncParams>) -> String {
        let Some(store) = &self.awareness_store else {
            return self.err(
                "awareness_store_unavailable",
                "awareness store not configured",
                Some("use awareness_status for source posture; project-state sync requires awareness store"),
                None,
            );
        };

        let operation = match params.operation.parse::<AwarenessOperation>() {
            Ok(v) => v,
            Err(e) => return self.err("bad_awareness_operation", &e, None, None),
        };
        let kind = match params.kind.parse::<AwarenessNodeKind>() {
            Ok(v) => v,
            Err(e) => return self.err("bad_awareness_kind", &e, None, None),
        };
        let authority = match params.authority.as_deref() {
            Some(raw) => match raw.parse::<AwarenessAuthority>() {
                Ok(v) => Some(v),
                Err(e) => return self.err("bad_awareness_authority", &e, None, None),
            },
            None => None,
        };
        let active = self.active_state.current_session();
        let project_slug = params
            .project_slug
            .or_else(|| active.as_ref().map(|s| s.project_slug.to_string()))
            .filter(|s| s != "unknown");
        let Some(project_slug) = project_slug else {
            return self.err(
                "missing_project_slug",
                "awareness_sync requires project_slug or an active debug session project",
                Some("pass project_slug explicitly or start_debug_session with project"),
                Some("start_debug_session"),
            );
        };
        let evidence = params
            .evidence
            .map_or_else(AwarenessEvidence::default, |e| AwarenessEvidence {
                observation_ids: e.observation_ids.unwrap_or_default(),
                debug_session_ids: e.debug_session_ids.unwrap_or_default(),
                checkpoint_ids: e.checkpoint_ids.unwrap_or_default(),
                librarian_node_ids: e.librarian_node_ids.unwrap_or_default(),
            });

        let input = AwarenessSync {
            operation,
            project_slug,
            path: params.path,
            kind,
            authority,
            confidence: params.confidence,
            summary: params.summary,
            note: params.note,
            redex: params.redex,
            tags: params.tags.unwrap_or_default(),
            debug_session_id: params
                .debug_session_id
                .or_else(|| active.as_ref().map(|s| s.id.to_string())),
            checkpoint_id: params.checkpoint_id,
            evidence,
            target_node_id: params.target_node_id,
            supersedes: params.supersedes.unwrap_or_default(),
            answers: params.answers.unwrap_or_default(),
            contradicts: params.contradicts.unwrap_or_default(),
        };

        match store.sync_node(input).await {
            Ok(result) => {
                if result.conflict.is_some() {
                    self.ok_with(
                        serde_json::json!({
                            "status": "conflict_detected",
                            "result": result,
                        }),
                        vec!["awareness_status"],
                        Some("conflict detected; ask the user which source of truth should remain active before continuing"),
                    )
                } else {
                    self.ok_with(
                        serde_json::json!({
                            "status": "synced",
                            "result": result,
                        }),
                        vec!["awareness_status"],
                        Some("awareness updated; use awareness_status for the compact current manifest"),
                    )
                }
            }
            Err(e) => self.err("awareness_sync_failed", &e.to_string(), None, None),
        }
    }
}

#[tool_router(router = librarian_tool_router, vis = "pub")]
impl DaemonMcp {
    #[doc = include_str!("../tool_descriptions/librarian_index.md")]
    #[tool(name = "librarian_index")]
    async fn librarian_index(
        &self,
        Parameters(params): Parameters<LibrarianIndexParams>,
    ) -> String {
        let lib_store = match &self.librarian_store {
            Some(s) => s,
            None => {
                return self.err(
                    "librarian_store_unavailable",
                    "librarian catalog not configured",
                    None,
                    None,
                );
            }
        };
        let inner = librarian_index_inner(lib_store.as_ref(), params).await;
        match serde_json::from_str::<serde_json::Value>(&inner) {
            Ok(v) if v.get("error").is_some() => wrap_inner_result(self, &inner),
            Ok(v) => {
                let mut hints = Vec::new();
                let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                let version = v.get("version").and_then(|v| v.as_str()).unwrap_or("");
                match kind {
                    "project" => hints.push(
                        "Next: index its documentation and source configs with edges linking back.",
                    ),
                    "fix" => hints.push(
                        "Consider linking this fix to the error it resolves with edge kind 'fixes'.",
                    ),
                    _ => {}
                }
                if version.matches('.').count() > 2 {
                    hints.push("Previous version deprecated and linked via supersedes edge.");
                }
                if v.get("parent_id").and_then(|p| p.as_str()).is_none() && kind != "project" {
                    hints.push("Consider organizing under a parent node for hierarchy.");
                }
                let hint = if hints.is_empty() {
                    None
                } else {
                    Some(hints.join(" "))
                };
                self.ok_with(v, vec!["librarian_lookup"], hint.as_deref())
            }
            Err(_) => wrap_inner_result(self, &inner),
        }
    }

    #[doc = include_str!("../tool_descriptions/librarian_lookup.md")]
    #[tool(name = "librarian_lookup")]
    async fn librarian_lookup(
        &self,
        Parameters(params): Parameters<LibrarianLookupParams>,
    ) -> String {
        let lib_store = match &self.librarian_store {
            Some(s) => s,
            None => {
                return self.err(
                    "librarian_store_unavailable",
                    "librarian catalog not configured",
                    None,
                    None,
                );
            }
        };
        let inner = librarian_lookup_inner(lib_store.as_ref(), params).await;
        match serde_json::from_str::<serde_json::Value>(&inner) {
            Ok(v) if v.get("error").is_some() => wrap_inner_result(self, &inner),
            Ok(v) => {
                let mut hints = Vec::new();
                if let Some(nodes) = v.get("nodes").and_then(|n| n.as_array()) {
                    let thirty_days_ago_ns = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos() as u64
                        - 30 * 86_400_000_000_000;
                    let stale_count = nodes
                        .iter()
                        .filter(|n| {
                            if n.get("canonicalized_at").and_then(|c| c.as_u64()).is_some() {
                                return false;
                            }
                            n.get("last_read_at")
                                .and_then(|r| r.as_u64())
                                .is_none_or(|ts| ts < thirty_days_ago_ns)
                        })
                        .count();
                    if stale_count > 0 {
                        hints.push(
                            "Some results haven't been accessed in over 30 days. Consider reviewing and deprecating stale entries with librarian_forget(deprecate=true).",
                        );
                    }
                }
                let hint = if hints.is_empty() {
                    None
                } else {
                    Some(hints.join(" "))
                };
                self.ok_with(v, vec![], hint.as_deref())
            }
            Err(_) => wrap_inner_result(self, &inner),
        }
    }

    #[doc = include_str!("../tool_descriptions/librarian_forget.md")]
    #[tool(name = "librarian_forget")]
    async fn librarian_forget(
        &self,
        Parameters(params): Parameters<LibrarianForgetParams>,
    ) -> String {
        let lib_store = match &self.librarian_store {
            Some(s) => s,
            None => {
                return self.err(
                    "librarian_store_unavailable",
                    "librarian catalog not configured",
                    None,
                    None,
                );
            }
        };
        let deprecate = params.deprecate.unwrap_or(true);
        if deprecate {
            match lib_store.deprecate_node(&params.id).await {
                Ok(existed) => self.ok(serde_json::json!({ "deprecated": existed })),
                Err(e) => self.err("librarian_forget_failed", &e.to_string(), None, None),
            }
        } else {
            if params.confirm != Some(true) {
                return self.err(
                    "missing_confirm",
                    "hard delete requires confirm=true",
                    Some("pass confirm=true to permanently delete, or use deprecate=true (default) for soft-delete"),
                    None,
                );
            }
            match lib_store.forget_node(&params.id).await {
                Ok(existed) => self.ok(serde_json::json!({ "deleted": existed })),
                Err(e) => self.err("librarian_forget_failed", &e.to_string(), None, None),
            }
        }
    }
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

/// Standalone error builder for code paths without &self access (free
/// functions, inner helpers). Produces an envelope-shaped error with no
/// `daemon8` meta — tool methods that have &self should prefer
/// `self.err(...)` so the active debug session echoes correctly.
fn error_json(msg: &str) -> String {
    envelope::err(
        "internal_error",
        msg,
        None,
        None,
        envelope::DaemonMeta::default(),
    )
}

fn source_template_matches(
    data: Option<&serde_json::Value>,
    classification: &ProjectClassification,
) -> bool {
    let Some(data) = data else {
        return false;
    };
    let Ok(template) = serde_json::from_value::<daemon8_types::SourceTemplateData>(data.clone())
    else {
        return false;
    };
    if !template.platforms.contains(&classification.platform) {
        return false;
    }
    template
        .project_types
        .iter()
        .any(|project_type| classification.tags.contains(project_type))
}

fn derive_slug_from_root(root: &std::path::Path) -> Option<String> {
    root.file_name().and_then(|name| name.to_str()).map(|raw| {
        let slug = raw
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .split('-')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        if slug.is_empty() {
            "project".to_string()
        } else {
            slug
        }
    })
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
        let push_source_activator = self.source_activator.clone();
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

                if let (Some(f), Some(sa)) = (&filter, &push_source_activator) {
                    sa.touch_matching(f);
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

/// Internal handler for typed persistence. Extracted so API/tests can drive it
/// against an in-memory `MemoryStore` without spinning up DaemonMcp.
pub async fn save_memory_inner(mem_store: &dyn MemoryStore, params: SaveMemoryParams) -> String {
    let kind = params
        .kind
        .as_deref()
        .and_then(|s| s.parse::<daemon8_types::MemoryKind>().ok())
        .unwrap_or(daemon8_types::MemoryKind::UserFlagged);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;

    let memory = daemon8_store::Memory {
        id: None,
        created_at: now,
        updated_at: now,
        kind,
        content: params.content,
        source_observations: params.source_observations.unwrap_or_default(),
        tags: params.tags.unwrap_or_default(),
        project_slug: params.project_slug.unwrap_or_default(),
        session_id: params.session_id,
        confidence: params.confidence.unwrap_or(1.0),
        data: None,
    };

    match mem_store.save_memory(memory).await {
        Ok(id) => serde_json::to_string(&serde_json::json!({ "id": id })).unwrap_or_default(),
        Err(e) => error_json(&format!("save_memory failed: {e}")),
    }
}

/// Internal handler for typed persistence lookup. Extracted so API/tests can drive it
/// against an in-memory `MemoryStore` without spinning up DaemonMcp.
pub async fn query_memory_inner(mem_store: &dyn MemoryStore, params: QueryMemoryParams) -> String {
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

pub async fn librarian_index_inner(
    lib_store: &dyn LibrarianStore,
    params: LibrarianIndexParams,
) -> String {
    let kind = match params.kind.parse::<daemon8_types::LibrarianNodeKind>() {
        Ok(k) => k,
        Err(_) => {
            return error_json(&format!(
                "invalid kind '{}'. Use: doc, source_template, fix, project",
                params.kind
            ));
        }
    };
    let locator_kind = match params.locator_kind.parse::<daemon8_types::LocatorKind>() {
        Ok(k) => k,
        Err(_) => {
            return error_json(&format!(
                "invalid locator_kind '{}'. Use: file, url, vault",
                params.locator_kind
            ));
        }
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;

    let node = daemon8_store::LibrarianNode {
        id: None,
        kind,
        label: params.label,
        locator_kind,
        locator: params.locator,
        tags: params.tags.unwrap_or_default(),
        project_slug: params.project_slug.unwrap_or_default(),
        version: String::new(),
        parent_id: params.parent_id.clone(),
        created_at: now,
        updated_at: now,
        last_read_at: None,
        deprecated_at: None,
        canonicalized_at: if params.canonicalize.unwrap_or(false) {
            Some(now)
        } else {
            None
        },
        data: params.data,
    };

    let id = match lib_store.index_node(node).await {
        Ok(id) => id,
        Err(e) => return error_json(&format!("librarian_index failed: {e}")),
    };

    let indexed_node = match lib_store.get_node(&id).await {
        Ok(Some(n)) => n,
        _ => {
            return serde_json::to_string(&serde_json::json!({
                "id": id, "version": "unknown", "kind": kind.to_string()
            }))
            .unwrap_or_default();
        }
    };

    if let Some(ref edge) = params.edge {
        let edge_kind = match edge.kind.parse::<daemon8_types::LibrarianEdgeKind>() {
            Ok(k) => k,
            Err(_) => {
                return serde_json::to_string(&serde_json::json!({
                    "id": id,
                    "version": indexed_node.version,
                    "kind": kind.to_string(),
                    "edge_error": format!("invalid edge kind '{}'. Use: has_source, documented_by, fixes, supersedes, child_of", edge.kind)
                }))
                .unwrap_or_default();
            }
        };
        let lib_edge = daemon8_store::LibrarianEdge {
            id: None,
            kind: edge_kind,
            from_node: id.clone(),
            to_node: edge.target_node_id.clone(),
            created_at: now,
        };
        if let Err(e) = lib_store.index_edge(lib_edge).await {
            return serde_json::to_string(&serde_json::json!({
                "id": id,
                "version": indexed_node.version,
                "kind": kind.to_string(),
                "edge_error": format!("edge creation failed: {e}")
            }))
            .unwrap_or_default();
        }
    }

    serde_json::to_string(&serde_json::json!({
        "id": id,
        "version": indexed_node.version,
        "kind": kind.to_string(),
        "parent_id": params.parent_id,
    }))
    .unwrap_or_default()
}

pub async fn librarian_lookup_inner(
    lib_store: &dyn LibrarianStore,
    params: LibrarianLookupParams,
) -> String {
    if let Some(ref id) = params.id {
        let node = match lib_store.get_node(id).await {
            Ok(Some(n)) => n,
            Ok(None) => return error_json(&format!("node '{id}' not found")),
            Err(e) => return error_json(&format!("librarian_lookup failed: {e}")),
        };
        let edges = match lib_store.get_edges(id).await {
            Ok(e) => e,
            Err(e) => return error_json(&format!("librarian_lookup edges: {e}")),
        };
        return serde_json::to_string_pretty(&serde_json::json!({
            "node": node,
            "edges": edges,
        }))
        .unwrap_or_default();
    }

    let kinds = params.kinds.map(|v| {
        v.into_iter()
            .filter_map(|s| s.parse::<daemon8_types::LibrarianNodeKind>().ok())
            .collect()
    });

    let stale_before = params.stale_before_days.map(|days| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        now.saturating_sub(u64::from(days) * 86_400_000_000_000)
    });

    let filter = daemon8_store::LibrarianFilter {
        kinds,
        tags: params.tags,
        project_slug: params.project_slug,
        text_match: params.text,
        limit: Some(params.limit.unwrap_or(20).min(500) as usize),
        include_deprecated: params.include_deprecated.unwrap_or(false),
        stale_before,
        parent_id: params.parent_id,
    };

    match lib_store.lookup(&filter).await {
        Ok(nodes) => serde_json::to_string_pretty(&serde_json::json!({ "nodes": nodes }))
            .unwrap_or_else(|e| error_json(&format!("serialization failed: {e}"))),
        Err(e) => error_json(&format!("librarian_lookup failed: {e}")),
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

    async fn build_mcp_with_debug_session() -> DaemonMcp {
        let store = Arc::new(daemon8_store::SurrealStore::memory().await.unwrap());
        let memory_store: Arc<dyn MemoryStore> = Arc::new(store.memory_store());
        let debug_session_store: Arc<dyn DebugSessionStore> = Arc::new(store.debug_session_store());
        let awareness_store: Arc<dyn daemon8_store::AwarenessStore> =
            Arc::new(store.awareness_store());
        let (obs_tx, _obs_rx) = tokio::sync::mpsc::unbounded_channel();
        let (chrome_tx, _chrome_rx) = tokio::sync::mpsc::channel(8);
        let (_, chrome_state) =
            tokio::sync::watch::channel(daemon8_chrome::ConnectionState::Disconnected);
        let (broadcast_tx, _broadcast_rx) = broadcast::channel(8);
        let lens = Arc::new(LensManager::new(broadcast_tx.subscribe(), None));
        DaemonMcp::new(DaemonMcpConfig {
            store: store.clone(),
            memory_store: Some(memory_store),
            debug_session_store: Some(debug_session_store),
            librarian_store: None,
            awareness_store: Some(awareness_store),
            obs_tx,
            chrome_tx,
            chrome_state,
            chrome_endpoint: Arc::new(Mutex::new(None)),
            device_screenshot_fn: None,
            screenshot_dir: std::env::temp_dir().join("daemon8-test"),
            broadcast_tx,
            lens,
            setup_tool_fn: None,
            source_activator: None,
            cancel: tokio_util::sync::CancellationToken::new(),
            active_project: Arc::new(RwLock::new(None)),
        })
    }

    #[tokio::test]
    async fn awareness_sync_populates_status_manifest() {
        let mcp = build_mcp_with_debug_session().await;
        let _ = mcp
            .start_debug_session(Parameters(StartDebugSessionParams {
                project: Some("daemon8".into()),
                description: Some("awareness tree alpha".into()),
                agent_id: ":test/codex+runtime-agent>".into(),
                feature: Some("awareness".into()),
            }))
            .await;

        let synced = mcp
            .awareness_sync(Parameters(AwarenessSyncParams {
                operation: "capture".into(),
                path: "alpha.awareness.objective".into(),
                kind: "objective".into(),
                authority: Some("accepted".into()),
                confidence: Some(0.8),
                summary: Some("Keep debug-session state visible without flooding context.".into()),
                note: None,
                redex: None,
                tags: Some(vec!["alpha".into(), "cadence".into()]),
                project_slug: None,
                debug_session_id: None,
                checkpoint_id: None,
                evidence: None,
                target_node_id: None,
                supersedes: None,
                answers: None,
                contradicts: None,
            }))
            .await;
        let synced: serde_json::Value = serde_json::from_str(&synced).unwrap();
        assert_eq!(synced["result"]["status"], "synced");

        let status = mcp
            .awareness_status(Parameters(AwarenessStatusParams {
                focus_path: None,
                depth: None,
                include_notes: None,
                include_evidence: None,
            }))
            .await;
        let status: serde_json::Value = serde_json::from_str(&status).unwrap();
        let objectives = status["result"]["project_awareness"]["manifest"]["active_objectives"]
            .as_array()
            .expect("manifest must include active objectives");
        assert_eq!(objectives.len(), 1);
        assert_eq!(objectives[0]["path"], "alpha.awareness.objective");
        assert!(
            status["daemon8"]["next_actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "awareness_sync"),
            "status should suggest awareness_sync when the graph is writable"
        );
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
        let session_id = started["result"]["debug_session_id"]
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
        assert_eq!(resolved["result"]["debug_session_id"], session_id);
        let memory_id = resolved["result"]["summary_memory_id"].as_str().unwrap();

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
        assert_eq!(parsed["error"]["code"], "no_active_debug_session");
        assert_eq!(parsed["error"]["fix"]["tool"], "start_debug_session");
        assert!(parsed["result"].is_null());
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
        let result = &parsed["result"];
        let cp_id = result["checkpoint_id"].as_str().unwrap();
        assert!(result["seq_at_creation"].is_number());
        // Envelope echoes the active session.
        assert!(parsed["daemon8"]["active_debug_session"].is_object());

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
        assert_eq!(parsed["result"]["count"], 1);

        let all = mcp
            .list_debug_sessions(Parameters(ListDebugSessionsParams {
                status: None,
                feature: None,
            }))
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&all).unwrap();
        assert_eq!(parsed["result"]["count"], 2);
    }

    #[tokio::test]
    async fn save_memory_inner_persists_curated_memory() {
        let store = daemon8_store::SurrealStore::memory().await.unwrap();
        let mem_store = store.memory_store();
        mem_store.init_schema().await.unwrap();

        let result = save_memory_inner(
            &mem_store,
            SaveMemoryParams {
                content: "Prefer checkpoints before runtime checks.".into(),
                kind: Some("decision".into()),
                tags: Some(vec!["project:daemon8".into(), "kind:test".into()]),
                source_observations: Some(vec![42]),
                project_slug: Some("daemon8".into()),
                session_id: Some("test-session".into()),
                confidence: Some(0.9),
            },
        )
        .await;

        let value: serde_json::Value = serde_json::from_str(&result).unwrap();
        let id = value["id"]
            .as_str()
            .expect("save_memory should return an id");
        let saved = mem_store.get_memory(id).await.unwrap().unwrap();

        assert_eq!(saved.kind, daemon8_types::MemoryKind::Decision);
        assert_eq!(saved.content, "Prefer checkpoints before runtime checks.");
        assert_eq!(saved.source_observations, vec![42]);
        assert_eq!(saved.tags, vec!["project:daemon8", "kind:test"]);
        assert_eq!(saved.project_slug, "daemon8");
        assert_eq!(saved.session_id.as_deref(), Some("test-session"));
        assert_eq!(saved.confidence, 0.9);
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

        // Keep receivers alive so observations sent via ingest_observation
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
            let lens = Arc::new(LensManager::new(broadcast_tx.subscribe(), None));
            DaemonMcp::new(DaemonMcpConfig {
                store: shared_store.clone(),
                memory_store: Some(shared_mem.clone()),
                debug_session_store: Some(shared_ds.clone()),
                librarian_store: None,
                awareness_store: None,
                obs_tx: shared_obs_tx.clone(),
                chrome_tx,
                chrome_state,
                chrome_endpoint: Arc::new(Mutex::new(None)),
                device_screenshot_fn: None,
                screenshot_dir: std::env::temp_dir().join("daemon8-test"),
                broadcast_tx,
                lens,
                setup_tool_fn: None,
                source_activator: None,
                cancel: tokio_util::sync::CancellationToken::new(),
                active_project: Arc::new(RwLock::new(None)),
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
            a_parsed["result"]["debug_session_id"].is_string(),
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
            b_parsed["result"]["debug_session_id"].is_string(),
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
        let a_sid = a_start["result"]["debug_session_id"].as_str().unwrap();

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
        let b_sid = b_start["result"]["debug_session_id"].as_str().unwrap();

        assert_ne!(a_sid, b_sid, "each agent must get a distinct session id");

        // Each ingests an observation through their own MCP instance
        a.ingest_observation(Parameters(IngestParams {
            kind: Some("log".into()),
            severity: Some("info".into()),
            app: Some("test-a".into()),
            channel: None,
            correlation_id: None,
            parent_id: None,
            node_id: None,
            session_id: None,
            tags: None,
            data: serde_json::json!({"msg": "from agent A"}),
        }))
        .await;

        b.ingest_observation(Parameters(IngestParams {
            kind: Some("log".into()),
            severity: Some("info".into()),
            app: Some("test-b".into()),
            channel: None,
            correlation_id: None,
            parent_id: None,
            node_id: None,
            session_id: None,
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
        assert!(
            end_res["result"]["debug_session_id"].is_string(),
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
            auth_only["result"]["count"], 1,
            "feature filter must return only the auth session"
        );
        assert_eq!(
            auth_only["result"]["sessions"][0]["feature"], "auth",
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
            search_only["result"]["count"], 1,
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
            none["result"]["count"], 0,
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
