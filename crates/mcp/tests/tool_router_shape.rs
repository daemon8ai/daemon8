// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{borrow::Cow, sync::Arc};

use daemon8_chrome::ConnectionState;
use daemon8_mcp::{
    ActParams, BuildContextSnapshotParams, CreateCheckpointParams, Daemon8ConnectParams,
    Daemon8InitParams, DaemonMcp, DaemonMcpConfig, DebugAction, IngestParams,
    LinkConversationParams, ListDebugSessionsParams, ObserveParams, StartDebugSessionParams,
    TOOL_POLICY_TABLE, ToolPolicy, tool_policy,
};
use daemon8_store::StateModel;
use daemon8_types::{Filter, Observation};
use rmcp::ServiceExt as _;
use tokio_util::sync::CancellationToken;

const EXPECTED_TOOLS: [&str; 15] = [
    "read_live_feed",
    "daemon8_connect",
    "daemon8_init",
    "daemon8_status",
    "list_connections",
    "link_conversation",
    "build_context_snapshot",
    "write_to_live_feed",
    "watch_live_feed",
    "issue_command",
    "connect_browser",
    "set_lens",
    "clear_lens",
    "lens_status",
    "daemon8_help",
];

const DEBUG_TOOLS: [&str; 5] = [
    "create_checkpoint",
    "start_debug_session",
    "list_debug_sessions",
    "resolve_debug_session",
    "end_debug_session",
];

fn tool_names(router: &rmcp::handler::server::router::tool::ToolRouter<DaemonMcp>) -> Vec<String> {
    router
        .list_all()
        .iter()
        .map(|t| t.name.to_string())
        .collect()
}

async fn make_mcp() -> DaemonMcp {
    make_mcp_with_cancel(CancellationToken::new()).await
}

async fn make_mcp_with_cancel(cancel: CancellationToken) -> DaemonMcp {
    make_mcp_with_cancel_and_home(cancel, test_home_dir()).await
}

async fn make_mcp_with_cancel_and_home(cancel: CancellationToken, home_dir: PathBuf) -> DaemonMcp {
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let memory_store = store.memory_store();
    memory_store.init_schema().await.unwrap();
    let scope_ledger_store = store.scope_ledger_store();
    let (obs_tx, _) = tokio::sync::mpsc::unbounded_channel();
    let (chrome_tx, _) = tokio::sync::mpsc::channel(16);
    let (_, chrome_state_rx) = tokio::sync::watch::channel(ConnectionState::Disconnected);
    let (broadcast_tx, _) = tokio::sync::broadcast::channel(16);
    let lens = Arc::new(daemon8_store::LensManager::new(broadcast_tx.subscribe()));
    DaemonMcp::new(DaemonMcpConfig {
        store,
        memory_store: Some(Arc::new(memory_store)),
        debug_session_store: None,
        scope_ledger_store: Some(Arc::new(scope_ledger_store)),
        obs_tx,
        chrome_tx,
        chrome_state: chrome_state_rx,
        chrome_endpoint: Arc::new(std::sync::Mutex::new(None)),
        device_screenshot_fn: None,
        screenshot_dir: std::env::temp_dir().join("daemon8-test-screenshots"),
        home_dir,
        broadcast_tx,
        lens,
        cursor_store: None,
        cancel,
    })
}

async fn make_mcp_with_debug() -> DaemonMcp {
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let memory_store = store.memory_store();
    memory_store.init_schema().await.unwrap();
    let debug_session_store = store.debug_session_store();
    debug_session_store.init_schema().await.unwrap();
    let scope_ledger_store = store.scope_ledger_store();
    let (obs_tx, _) = tokio::sync::mpsc::unbounded_channel();
    let (chrome_tx, _) = tokio::sync::mpsc::channel(16);
    let (_, chrome_state_rx) = tokio::sync::watch::channel(ConnectionState::Disconnected);
    let (broadcast_tx, _) = tokio::sync::broadcast::channel(16);
    let lens = Arc::new(daemon8_store::LensManager::new(broadcast_tx.subscribe()));
    DaemonMcp::new(DaemonMcpConfig {
        store,
        memory_store: Some(Arc::new(memory_store)),
        debug_session_store: Some(Arc::new(debug_session_store)),
        scope_ledger_store: Some(Arc::new(scope_ledger_store)),
        obs_tx,
        chrome_tx,
        chrome_state: chrome_state_rx,
        chrome_endpoint: Arc::new(std::sync::Mutex::new(None)),
        device_screenshot_fn: None,
        screenshot_dir: std::env::temp_dir().join("daemon8-test-screenshots"),
        home_dir: test_home_dir(),
        broadcast_tx,
        lens,
        cursor_store: None,
        cancel: CancellationToken::new(),
    })
}

async fn make_mcp_with_debug_and_writer() -> DaemonMcp {
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let memory_store = store.memory_store();
    memory_store.init_schema().await.unwrap();
    let debug_session_store = store.debug_session_store();
    debug_session_store.init_schema().await.unwrap();
    let scope_ledger_store = store.scope_ledger_store();
    let (obs_tx, mut obs_rx) = tokio::sync::mpsc::unbounded_channel();
    let store_for_writer = store.clone();
    tokio::spawn(async move {
        while let Some(obs) = obs_rx.recv().await {
            let _ = store_for_writer.insert(&obs).await;
        }
    });

    let (chrome_tx, _) = tokio::sync::mpsc::channel(16);
    let (_, chrome_state_rx) = tokio::sync::watch::channel(ConnectionState::Disconnected);
    let (broadcast_tx, _) = tokio::sync::broadcast::channel(16);
    let lens = Arc::new(daemon8_store::LensManager::new(broadcast_tx.subscribe()));
    DaemonMcp::new(DaemonMcpConfig {
        store,
        memory_store: Some(Arc::new(memory_store)),
        debug_session_store: Some(Arc::new(debug_session_store)),
        scope_ledger_store: Some(Arc::new(scope_ledger_store)),
        obs_tx,
        chrome_tx,
        chrome_state: chrome_state_rx,
        chrome_endpoint: Arc::new(std::sync::Mutex::new(None)),
        device_screenshot_fn: None,
        screenshot_dir: std::env::temp_dir().join("daemon8-test-screenshots"),
        home_dir: test_home_dir(),
        broadcast_tx,
        lens,
        cursor_store: None,
        cancel: CancellationToken::new(),
    })
}

async fn make_mcp_with_debug_without_memory() -> DaemonMcp {
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let debug_session_store = store.debug_session_store();
    debug_session_store.init_schema().await.unwrap();
    let scope_ledger_store = store.scope_ledger_store();
    let (obs_tx, _) = tokio::sync::mpsc::unbounded_channel();
    let (chrome_tx, _) = tokio::sync::mpsc::channel(16);
    let (_, chrome_state_rx) = tokio::sync::watch::channel(ConnectionState::Disconnected);
    let (broadcast_tx, _) = tokio::sync::broadcast::channel(16);
    let lens = Arc::new(daemon8_store::LensManager::new(broadcast_tx.subscribe()));
    DaemonMcp::new(DaemonMcpConfig {
        store,
        memory_store: None,
        debug_session_store: Some(Arc::new(debug_session_store)),
        scope_ledger_store: Some(Arc::new(scope_ledger_store)),
        obs_tx,
        chrome_tx,
        chrome_state: chrome_state_rx,
        chrome_endpoint: Arc::new(std::sync::Mutex::new(None)),
        device_screenshot_fn: None,
        screenshot_dir: std::env::temp_dir().join("daemon8-test-screenshots"),
        home_dir: test_home_dir(),
        broadcast_tx,
        lens,
        cursor_store: None,
        cancel: CancellationToken::new(),
    })
}

async fn make_mcp_with_debug_in_home(home_dir: PathBuf) -> DaemonMcp {
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let memory_store = store.memory_store();
    memory_store.init_schema().await.unwrap();
    let debug_session_store = store.debug_session_store();
    debug_session_store.init_schema().await.unwrap();
    let scope_ledger_store = store.scope_ledger_store();
    let (obs_tx, _) = tokio::sync::mpsc::unbounded_channel();
    let (chrome_tx, _) = tokio::sync::mpsc::channel(16);
    let (_, chrome_state_rx) = tokio::sync::watch::channel(ConnectionState::Disconnected);
    let (broadcast_tx, _) = tokio::sync::broadcast::channel(16);
    let lens = Arc::new(daemon8_store::LensManager::new(broadcast_tx.subscribe()));
    DaemonMcp::new(DaemonMcpConfig {
        store,
        memory_store: Some(Arc::new(memory_store)),
        debug_session_store: Some(Arc::new(debug_session_store)),
        scope_ledger_store: Some(Arc::new(scope_ledger_store)),
        obs_tx,
        chrome_tx,
        chrome_state: chrome_state_rx,
        chrome_endpoint: Arc::new(std::sync::Mutex::new(None)),
        device_screenshot_fn: None,
        screenshot_dir: std::env::temp_dir().join("daemon8-test-screenshots"),
        home_dir,
        broadcast_tx,
        lens,
        cursor_store: None,
        cancel: CancellationToken::new(),
    })
}

async fn make_mcp_with_writer() -> DaemonMcp {
    make_mcp_with_writer_in_home(test_home_dir()).await
}

async fn make_mcp_with_writer_in_home(home_dir: PathBuf) -> DaemonMcp {
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let memory_store = store.memory_store();
    memory_store.init_schema().await.unwrap();
    let scope_ledger_store = store.scope_ledger_store();
    let (obs_tx, mut obs_rx) = tokio::sync::mpsc::unbounded_channel();
    let store_for_writer = store.clone();
    tokio::spawn(async move {
        while let Some(obs) = obs_rx.recv().await {
            let _ = store_for_writer.insert(&obs).await;
        }
    });

    let (chrome_tx, _) = tokio::sync::mpsc::channel(16);
    let (_, chrome_state_rx) = tokio::sync::watch::channel(ConnectionState::Disconnected);
    let (broadcast_tx, _) = tokio::sync::broadcast::channel(16);
    let lens = Arc::new(daemon8_store::LensManager::new(broadcast_tx.subscribe()));
    DaemonMcp::new(DaemonMcpConfig {
        store,
        memory_store: Some(Arc::new(memory_store)),
        debug_session_store: None,
        scope_ledger_store: Some(Arc::new(scope_ledger_store)),
        obs_tx,
        chrome_tx,
        chrome_state: chrome_state_rx,
        chrome_endpoint: Arc::new(std::sync::Mutex::new(None)),
        device_screenshot_fn: None,
        screenshot_dir: std::env::temp_dir().join("daemon8-test-screenshots"),
        home_dir,
        broadcast_tx,
        lens,
        cursor_store: None,
        cancel: CancellationToken::new(),
    })
}

async fn make_mcp_with_read_through() -> DaemonMcp {
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let memory_store = store.memory_store();
    memory_store.init_schema().await.unwrap();
    let scope_ledger_store = store.scope_ledger_store();
    let cursor_store: Arc<dyn daemon8_store::CursorStore> = Arc::new(store.cursor_store());
    let (obs_tx, mut obs_rx) = tokio::sync::mpsc::unbounded_channel();
    let store_for_writer = store.clone();
    tokio::spawn(async move {
        while let Some(obs) = obs_rx.recv().await {
            let _ = store_for_writer.insert(&obs).await;
        }
    });
    let (chrome_tx, _) = tokio::sync::mpsc::channel(16);
    let (_, chrome_state_rx) = tokio::sync::watch::channel(ConnectionState::Disconnected);
    let (broadcast_tx, _) = tokio::sync::broadcast::channel(16);
    let lens = Arc::new(daemon8_store::LensManager::new(broadcast_tx.subscribe()));
    DaemonMcp::new(DaemonMcpConfig {
        store,
        memory_store: Some(Arc::new(memory_store)),
        debug_session_store: None,
        scope_ledger_store: Some(Arc::new(scope_ledger_store)),
        obs_tx,
        chrome_tx,
        chrome_state: chrome_state_rx,
        chrome_endpoint: Arc::new(std::sync::Mutex::new(None)),
        device_screenshot_fn: None,
        screenshot_dir: std::env::temp_dir().join("daemon8-test-screenshots"),
        home_dir: test_home_dir(),
        broadcast_tx,
        lens,
        cursor_store: Some(cursor_store),
        cancel: CancellationToken::new(),
    })
}

async fn make_mcp_with_shared_store(
    store: Arc<dyn StateModel>,
    cursor_store: Arc<dyn daemon8_store::CursorStore>,
    obs_tx: tokio::sync::mpsc::UnboundedSender<Observation>,
    broadcast_tx: tokio::sync::broadcast::Sender<(Arc<Observation>, Arc<str>)>,
    home_dir: PathBuf,
) -> DaemonMcp {
    let (chrome_tx, _) = tokio::sync::mpsc::channel(16);
    let (_, chrome_state_rx) = tokio::sync::watch::channel(ConnectionState::Disconnected);
    let lens = Arc::new(daemon8_store::LensManager::new(broadcast_tx.subscribe()));
    DaemonMcp::new(DaemonMcpConfig {
        store,
        memory_store: None,
        debug_session_store: None,
        scope_ledger_store: None,
        obs_tx,
        chrome_tx,
        chrome_state: chrome_state_rx,
        chrome_endpoint: Arc::new(std::sync::Mutex::new(None)),
        device_screenshot_fn: None,
        screenshot_dir: std::env::temp_dir().join("daemon8-test-screenshots"),
        home_dir,
        broadcast_tx,
        lens,
        cursor_store: Some(cursor_store),
        cancel: CancellationToken::new(),
    })
}

fn test_home_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("daemon8-test-home-{nanos}"));
    std::fs::create_dir_all(&path).unwrap();
    path
}

use daemon8_store::SurrealStore;

#[derive(Clone, Default)]
struct TestClient;

impl rmcp::handler::client::ClientHandler for TestClient {}

fn act_params(action: DebugAction) -> ActParams {
    ActParams {
        action,
        tab_id: None,
        expression: None,
        selector: None,
        css: None,
        temporary: None,
        device_serial: None,
        device_platform: None,
        viewport_width: None,
        viewport_height: None,
        viewport_scale: None,
        viewport_mobile: None,
        viewport_ua: None,
        network_preset: None,
        store_type: None,
        storage_key: None,
        storage_value: None,
        storage_types: None,
        x: None,
        y: None,
        url: None,
    }
}

fn mark_project(root: &std::path::Path) {
    std::fs::create_dir(root.join(".git")).unwrap();
}

fn write_file_source_config(root: &std::path::Path, source_id: &str, log_name: &str) {
    let daemon_dir = root.join(".daemon8");
    std::fs::create_dir_all(&daemon_dir).unwrap();
    std::fs::write(
        daemon_dir.join("config.md"),
        format!(
            r#"---
daemon8_schema: 1
created_at: "2026-05-17T00:00:00Z"
updated_at: "2026-05-17T00:00:00Z"
project:
  name: mcp-project
  stack:
    languages: [rust]
    frameworks: [tokio]
    tools: [cargo]
vars:
  PRJ_ROOT: "{}"
sources:
  - id: {source_id}
    service: cargo
    kind: file
    parser: line
    path: "$PRJ_ROOT/{log_name}"
---
# daemon8
"#,
            root.display()
        ),
    )
    .unwrap();
}

fn write_empty_project_config(root: &std::path::Path) {
    let daemon_dir = root.join(".daemon8");
    std::fs::create_dir_all(&daemon_dir).unwrap();
    std::fs::write(
        daemon_dir.join("config.md"),
        format!(
            r#"---
daemon8_schema: 1
created_at: "2026-05-17T00:00:00Z"
updated_at: "2026-05-17T00:00:00Z"
project:
  name: mcp-project
  stack:
    languages: [rust]
    frameworks: [tokio]
    tools: [cargo]
vars:
  PRJ_ROOT: "{}"
sources: []
---
# daemon8
"#,
            root.display()
        ),
    )
    .unwrap();
}

fn write_conversation_source_config(root: &std::path::Path, source_id: &str, path_name: &str) {
    let daemon_dir = root.join(".daemon8");
    std::fs::create_dir_all(&daemon_dir).unwrap();
    std::fs::write(
        daemon_dir.join("config.md"),
        format!(
            r#"---
daemon8_schema: 1
created_at: "2026-05-17T00:00:00Z"
updated_at: "2026-05-17T00:00:00Z"
project:
  name: mcp-project
  stack:
    languages: [rust]
    frameworks: [tokio]
    tools: [cargo]
vars:
  PRJ_ROOT: "{}"
sources:
  - id: {source_id}
    service: codex
    kind: conversation
    provider: codex
    path: "$PRJ_ROOT/{path_name}"
---
# daemon8
"#,
            root.display()
        ),
    )
    .unwrap();
}

fn tool_request(
    name: impl Into<Cow<'static, str>>,
    arguments: serde_json::Value,
) -> rmcp::model::CallToolRequestParams {
    let mut request = rmcp::model::CallToolRequestParams::new(name);
    request.arguments = match arguments {
        serde_json::Value::Object(map) => Some(map),
        serde_json::Value::Null => None,
        other => panic!("tool arguments must be a JSON object or null, got {other}"),
    };
    request
}

fn result_text(result: &rmcp::model::CallToolResult) -> &str {
    result
        .content
        .first()
        .and_then(|content| content.raw.as_text())
        .map(|text| text.text.as_str())
        .expect("tool response should contain text content")
}

fn result_json(result: &rmcp::model::CallToolResult) -> serde_json::Value {
    serde_json::from_str(result_text(result)).expect("tool text must be JSON")
}

#[test]
fn current_tool_policy_is_explicit() {
    for tool in [
        "daemon8_connect",
        "daemon8_init",
        "daemon8_status",
        "daemon8_help",
    ] {
        assert_eq!(
            tool_policy(tool),
            Some(ToolPolicy::PreConnectAllowed),
            "{tool}"
        );
    }

    for tool in [
        "read_live_feed",
        "list_connections",
        "write_to_live_feed",
        "watch_live_feed",
        "issue_command",
        "connect_browser",
        "set_lens",
        "clear_lens",
        "lens_status",
        "list_debug_sessions",
    ] {
        assert_eq!(tool_policy(tool), Some(ToolPolicy::GeneralSafe), "{tool}");
    }

    for tool in [
        "start_debug_session",
        "create_checkpoint",
        "resolve_debug_session",
        "end_debug_session",
        "link_conversation",
        "build_context_snapshot",
    ] {
        assert_eq!(tool_policy(tool), Some(ToolPolicy::ProjectOnly), "{tool}");
    }

    assert_eq!(tool_policy("definitely_unknown"), None);
}

#[tokio::test]
async fn tool_policy_table_covers_the_public_tool_surface() {
    let mcp = make_mcp_with_debug().await;
    let names: Vec<String> = mcp
        .tools_for_client()
        .iter()
        .map(|t| t.name.to_string())
        .collect();

    assert_eq!(
        TOOL_POLICY_TABLE.len(),
        names.len(),
        "policy table and debug-enabled public surface must have the same size"
    );

    for name in &names {
        assert!(
            tool_policy(name).is_some(),
            "public tool {name} must have an explicit policy"
        );
    }

    for (name, _) in TOOL_POLICY_TABLE {
        assert!(
            names.iter().any(|tool| tool == name),
            "policy table contains non-public tool {name}"
        );
    }
}

#[tokio::test]
async fn every_public_tool_has_a_file_backed_description() {
    let mcp = make_mcp_with_debug().await;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tool_descriptions");

    for tool in mcp.tools_for_client() {
        let path = dir.join(format!("{}.md", tool.name));
        assert!(
            path.exists(),
            "public tool {} must have a source-reviewed description at {}",
            tool.name,
            path.display()
        );
    }
}

#[test]
fn composed_router_has_full_tool_surface() {
    let router =
        DaemonMcp::tool_router() + DaemonMcp::action_tool_router() + DaemonMcp::lens_tool_router();
    let names = tool_names(&router);

    assert_eq!(
        names.len(),
        EXPECTED_TOOLS.len(),
        "router must expose {} tools, got {}: {:?}",
        EXPECTED_TOOLS.len(),
        names.len(),
        names
    );

    for expected in EXPECTED_TOOLS {
        assert!(
            names.iter().any(|n| n == expected),
            "router missing expected tool '{}'. Present: {:?}",
            expected,
            names
        );
    }
}

#[tokio::test]
async fn memory_tools_are_not_public_on_alpha_surface() {
    let mcp = make_mcp().await;
    let names: Vec<String> = mcp
        .tools_for_client()
        .iter()
        .map(|t| t.name.to_string())
        .collect();

    for removed in [
        "save_memory",
        "search_memory",
        "query_memory",
        "forget_memory",
    ] {
        assert!(
            !names.iter().any(|name| name == removed),
            "{removed} must remain absent from the alpha public MCP surface"
        );
    }
}

#[tokio::test]
async fn live_mcp_exposes_full_tool_surface() {
    let mcp = make_mcp().await;
    let names: Vec<String> = mcp
        .tools_for_client()
        .iter()
        .map(|t| t.name.to_string())
        .collect();

    assert_eq!(
        names.len(),
        EXPECTED_TOOLS.len(),
        "tools_for_client() must expose {} tools, got {}: {:?}",
        EXPECTED_TOOLS.len(),
        names.len(),
        names
    );
    for expected in EXPECTED_TOOLS {
        assert!(
            names.iter().any(|n| n == expected),
            "tools_for_client() missing '{}'. Present: {:?}",
            expected,
            names
        );
    }
}

#[tokio::test]
async fn debug_enabled_mcp_exposes_debug_tool_surface() {
    let mcp = make_mcp_with_debug().await;
    let names: Vec<String> = mcp
        .tools_for_client()
        .iter()
        .map(|t| t.name.to_string())
        .collect();

    assert_eq!(
        names.len(),
        EXPECTED_TOOLS.len() + DEBUG_TOOLS.len(),
        "debug-enabled tools_for_client() must expose core + debug tools, got {}: {:?}",
        names.len(),
        names
    );
    for expected in EXPECTED_TOOLS.into_iter().chain(DEBUG_TOOLS) {
        assert!(
            names.iter().any(|n| n == expected),
            "tools_for_client() missing '{}'. Present: {:?}",
            expected,
            names
        );
    }
}

#[tokio::test]
async fn debug_lifecycle_tools_require_memory_for_summary_capture() {
    let mcp = make_mcp_with_debug_without_memory().await;
    let general = tempfile::tempdir().unwrap();
    let path = general.path().display().to_string();
    let names: Vec<String> = mcp
        .tools_for_client()
        .iter()
        .map(|t| t.name.to_string())
        .collect();

    for expected in DEBUG_TOOLS {
        assert!(
            !names.iter().any(|n| n == expected),
            "debug lifecycle tool '{expected}' must not be exposed without summary memory storage"
        );
    }

    let (server_transport, client_transport) = tokio::io::duplex(8192);
    let server = tokio::spawn(async move {
        mcp.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });
    let client = TestClient.serve(client_transport).await.unwrap();

    let pre_connect = client
        .call_tool(tool_request(
            "start_debug_session",
            serde_json::json!({
                "project": "daemon8",
                "description": "hidden without memory",
                "agent_id": ":host/codex+agent>",
                "feature": null,
            }),
        ))
        .await;
    assert!(
        pre_connect.is_err(),
        "hidden debug tools must not be masked by connect preflight"
    );

    let connect = client
        .call_tool(tool_request(
            "daemon8_connect",
            serde_json::json!({
                "provider": "codex",
                "project_path": path,
                "agent_name": null,
                "transcript_path": null,
            }),
        ))
        .await
        .unwrap();
    let parsed = result_json(&connect);
    assert_eq!(parsed["status"], "success");

    let post_connect = client
        .call_tool(tool_request(
            "start_debug_session",
            serde_json::json!({
                "project": "daemon8",
                "description": "hidden without memory",
                "agent_id": ":host/codex+agent>",
                "feature": null,
            }),
        ))
        .await;
    assert!(
        post_connect.is_err(),
        "hidden debug tools must stay owned by the MCP router after connect"
    );

    client.cancel().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn daemon8_connect_missing_config_guides_to_init() {
    let mcp = make_mcp().await;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

    let body = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["status"], "setup_required");
    assert_eq!(parsed["code"], "missing_config");
    assert!(parsed["data"]["session_id"].is_string());
    assert!(parsed["data"]["connection"].is_null());
    assert_eq!(parsed["next_actions"][0]["tool"], "daemon8_init");

    let body = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["status"], "setup_required");

    let status = mcp.daemon8_status_for_tests().await;
    let parsed: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert_eq!(
        parsed["data"]["scope_ledger"]["recent_failures"][0]["code"],
        "missing_config"
    );
    assert_eq!(
        parsed["data"]["scope_ledger"]["recent_failures"][0]["attempt_count"],
        2
    );
}

#[tokio::test]
async fn daemon8_init_then_connect_sets_session_connection() {
    let mcp = make_mcp().await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());

    let init = mcp
        .daemon8_init_for_tests(Daemon8InitParams {
            project_path: tmp.path().display().to_string(),
            name: Some("mcp-project".into()),
            overwrite: None,
            ignore: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&init).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["code"], "initialized");

    let connect = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["mode"], "project");
    assert_eq!(parsed["data"]["connection"]["mode"], "project");
    assert_eq!(parsed["data"]["connection"]["provider"], "codex");

    let status = mcp.daemon8_status_for_tests().await;
    let parsed: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["connection"]["mode"], "project");
    assert_eq!(parsed["data"]["connection"]["provider"], "codex");
    assert_eq!(
        parsed["data"]["scope_ledger"]["recent_scopes"][0]["scope_root"],
        tmp.path().canonicalize().unwrap().display().to_string()
    );
    assert_eq!(
        parsed["data"]["scope_ledger"]["recent_scopes"][0]["provider"],
        "codex"
    );
}

#[tokio::test]
async fn daemon8_connect_succeeds_with_file_source_config() {
    let mcp = make_mcp_with_writer().await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());
    std::fs::write(tmp.path().join("cargo.log"), "cargo check one\n").unwrap();
    write_file_source_config(tmp.path(), "cargo.check", "cargo.log");

    let connect = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "success");
    assert!(parsed["data"]["connected_at"].is_u64());
    assert_eq!(
        parsed["data"]["connection"]["connected_at"],
        parsed["data"]["connected_at"]
    );

    let status = mcp.daemon8_status_for_tests().await;
    let status: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert_eq!(
        status["data"]["connection"]["connected_at"],
        parsed["data"]["connected_at"]
    );
}

#[tokio::test]
async fn daemon8_connect_guides_generated_body_replacement() {
    let mcp = make_mcp_with_writer().await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());
    std::fs::write(tmp.path().join("artisan"), "#!/usr/bin/env php").unwrap();
    std::fs::write(tmp.path().join("composer.json"), "{}").unwrap();

    let init = mcp
        .daemon8_init_for_tests(Daemon8InitParams {
            project_path: tmp.path().display().to_string(),
            name: Some("laravel-project".into()),
            overwrite: None,
            ignore: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&init).unwrap();
    assert_eq!(parsed["status"], "success");

    let connect = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "success");
    assert!(parsed["data"]["source_count"].as_u64().unwrap() > 0);
    assert_eq!(
        parsed["data"]["config_body_status"],
        "generated_setup_instructions_present"
    );
    assert_eq!(
        parsed["data"]["config_body_action"],
        "replace_with_project_notes"
    );
    assert!(
        parsed["requirements"]
            .as_array()
            .unwrap()
            .iter()
            .any(|req| req
                .as_str()
                .unwrap()
                .contains("Do not repeat log paths or sources"))
    );
}

#[tokio::test]
async fn daemon8_connect_succeeds_with_conversation_source_config() {
    let mcp = make_mcp_with_writer().await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());
    let sessions = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("one.jsonl"),
        "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"hello\"}]}}\n",
    )
    .unwrap();
    write_conversation_source_config(tmp.path(), "codex.sessions", "sessions");

    let connect = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "success");
}

#[tokio::test]
async fn daemon8_connect_binds_explicit_runtime_transcript() {
    let home = test_home_dir();
    let mcp = make_mcp_with_writer_in_home(home.clone()).await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());
    write_empty_project_config(tmp.path());
    let sessions = home.join(".codex/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let transcript = sessions.join(format!(
        "active-{}.jsonl",
        tmp.path().file_name().unwrap().to_string_lossy()
    ));
    std::fs::write(
        &transcript,
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"s1\",\"cwd\":\"{}\"}}}}\n",
            tmp.path().display()
        ),
    )
    .unwrap();

    let connect = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex-cli".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: Some(transcript.display().to_string()),
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["provider"], "codex");
    assert_eq!(parsed["data"]["transcript"]["status"], "bound");

    let status = mcp.daemon8_status_for_tests().await;
    let parsed: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert_eq!(parsed["data"]["connection"]["mode"], "project");
    assert_eq!(parsed["data"]["connection"]["provider"], "codex");
    assert_eq!(
        parsed["data"]["connection"]["transcript_path"],
        std::fs::canonicalize(&transcript)
            .unwrap()
            .display()
            .to_string()
    );
}

#[tokio::test]
async fn daemon8_connect_rejects_transcript_provider_mismatch() {
    let home = test_home_dir();
    let mcp = make_mcp_with_cancel_and_home(CancellationToken::new(), home.clone()).await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());
    write_empty_project_config(tmp.path());
    let sessions = home.join(".claude/projects/-tmp-project");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::create_dir_all(home.join(".codex/sessions")).unwrap();
    let transcript = sessions.join(format!(
        "claude-{}.jsonl",
        tmp.path().file_name().unwrap().to_string_lossy()
    ));
    std::fs::write(
        &transcript,
        "{\"type\":\"permission-mode\",\"sessionId\":\"c1\",\"cwd\":\"/tmp/project\"}\n",
    )
    .unwrap();

    let connect = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: Some(transcript.display().to_string()),
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "error");
    assert_eq!(parsed["code"], "transcript_provider_mismatch");

    let status = mcp.daemon8_status_for_tests().await;
    let parsed: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert!(parsed["data"]["connection"].is_null());
}

#[tokio::test]
async fn daemon8_connect_rejects_transcript_scope_mismatch() {
    let home = test_home_dir();
    let mcp = make_mcp_with_cancel_and_home(CancellationToken::new(), home.clone()).await;
    let tmp = tempfile::tempdir().unwrap();
    let other_project = tmp.path().join("other-project");
    mark_project(tmp.path());
    write_empty_project_config(tmp.path());
    std::fs::create_dir_all(&other_project).unwrap();
    let sessions = home.join(".codex/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let transcript = sessions.join("other.jsonl");
    std::fs::write(
        &transcript,
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"s1\",\"cwd\":\"{}\"}}}}\n",
            other_project.display()
        ),
    )
    .unwrap();

    let connect = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: Some(transcript.display().to_string()),
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "error");
    assert_eq!(parsed["code"], "transcript_scope_mismatch");

    let status = mcp.daemon8_status_for_tests().await;
    let parsed: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert!(parsed["data"]["connection"].is_null());
}

#[tokio::test]
async fn daemon8_connect_invalid_provider_clears_previous_connection() {
    let mcp = make_mcp().await;
    let general = tempfile::tempdir().unwrap();

    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: general.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["status"], "success");

    let failed = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "unknown-provider".into(),
            project_path: general.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&failed).unwrap();
    assert_eq!(parsed["status"], "error");
    assert_eq!(parsed["code"], "invalid_provider");

    let status = mcp.daemon8_status_for_tests().await;
    let parsed: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert!(parsed["data"]["connection"].is_null());
    assert_eq!(
        parsed["data"]["scope_ledger"]["recent_failures"][0]["code"],
        "invalid_provider"
    );
}

#[tokio::test]
async fn runtime_tools_require_connect_first() {
    let mcp = make_mcp().await;
    let debug_mcp = make_mcp_with_debug().await;

    let body = mcp
        .connect_preflight_for_tests("read_live_feed")
        .expect("read_live_feed should require daemon8_connect");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["status"], "connect_required");
    assert_eq!(parsed["code"], "connect_required");
    assert!(parsed["data"]["session_id"].is_string());
    assert!(parsed["data"]["connection"].is_null());
    assert_eq!(parsed["next_actions"][0]["tool"], "daemon8_connect");
    assert_eq!(
        parsed["next_actions"][0]["reason"],
        "bind this MCP session to a project or general scope"
    );
    assert_eq!(
        parsed["next_actions"][0]["params"]["project_path"],
        "<path>"
    );

    for tool in [
        "daemon8_connect",
        "daemon8_init",
        "daemon8_status",
        "daemon8_help",
    ] {
        assert!(
            mcp.connect_preflight_for_tests(tool).is_none(),
            "{tool} should be a connect-first exception"
        );
    }

    assert!(
        debug_mcp
            .connect_preflight_for_tests("list_debug_sessions")
            .is_some(),
        "list_debug_sessions should require daemon8_connect"
    );
}

#[tokio::test]
async fn runtime_tools_require_connect_first_through_real_mcp_call() -> anyhow::Result<()> {
    let mcp = make_mcp().await;
    let (server_transport, client_transport) = tokio::io::duplex(8192);

    let server = tokio::spawn(async move {
        mcp.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = TestClient.serve(client_transport).await?;
    let result = client
        .call_tool(rmcp::model::CallToolRequestParams::new("read_live_feed"))
        .await?;

    let text = result
        .content
        .first()
        .and_then(|content| content.raw.as_text())
        .map(|text| text.text.as_str())
        .expect("connect-first response should be text content");
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["status"], "connect_required");
    assert_eq!(parsed["code"], "connect_required");
    assert_eq!(parsed["next_actions"][0]["tool"], "daemon8_connect");

    client.cancel().await?;
    server.await??;
    Ok(())
}

#[tokio::test]
async fn unknown_tools_are_not_masked_by_connect_preflight() -> anyhow::Result<()> {
    let mcp = make_mcp().await;
    let general = tempfile::tempdir().unwrap();
    let path = general.path().display().to_string();
    let (server_transport, client_transport) = tokio::io::duplex(8192);

    let server = tokio::spawn(async move {
        mcp.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = TestClient.serve(client_transport).await?;
    let pre_connect = client
        .call_tool(rmcp::model::CallToolRequestParams::new(
            "definitely_unknown",
        ))
        .await;
    assert!(
        pre_connect.is_err(),
        "unknown pre-connect tools must stay owned by the MCP router"
    );

    let connect = client
        .call_tool(tool_request(
            "daemon8_connect",
            serde_json::json!({
                "provider": "codex",
                "project_path": path,
                "agent_name": null,
                "transcript_path": null,
            }),
        ))
        .await?;
    let parsed = result_json(&connect);
    assert_eq!(parsed["status"], "success");

    let post_connect = client
        .call_tool(rmcp::model::CallToolRequestParams::new(
            "definitely_unknown",
        ))
        .await;
    assert!(
        post_connect.is_err(),
        "unknown post-connect tools must stay owned by the MCP router"
    );

    client.cancel().await?;
    server.await??;
    Ok(())
}

#[tokio::test]
async fn pre_connect_status_exception_runs_through_real_mcp_call() -> anyhow::Result<()> {
    let mcp = make_mcp().await;
    let (server_transport, client_transport) = tokio::io::duplex(8192);

    let server = tokio::spawn(async move {
        mcp.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = TestClient.serve(client_transport).await?;
    let result = client
        .call_tool(rmcp::model::CallToolRequestParams::new("daemon8_status"))
        .await?;

    let text = result
        .content
        .first()
        .and_then(|content| content.raw.as_text())
        .map(|text| text.text.as_str())
        .expect("status response should be text content");
    let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["code"], "status");
    assert_eq!(parsed["data"]["daemon_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        parsed["data"]["schema_version"],
        daemon8_store::SCHEMA_VERSION
    );
    assert!(parsed["data"]["session_id"].is_string());
    assert!(parsed["data"]["connection"].is_null());
    assert!(parsed["data"].get("session_context").is_none());

    client.cancel().await?;
    server.await??;
    Ok(())
}

#[tokio::test]
async fn pre_connect_help_exception_runs_through_real_mcp_call() -> anyhow::Result<()> {
    let mcp = make_mcp().await;
    let (server_transport, client_transport) = tokio::io::duplex(8192);

    let server = tokio::spawn(async move {
        mcp.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = TestClient.serve(client_transport).await?;
    let result = client
        .call_tool(tool_request(
            "daemon8_help",
            serde_json::json!({"topic": "envelope"}),
        ))
        .await?;
    let parsed = result_json(&result);
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["code"], "ok");
    assert_eq!(parsed["data"]["topic"], "envelope");
    assert!(parsed["data"]["session_id"].is_string());
    assert!(parsed["data"]["connection"].is_null());
    assert!(parsed["data"].get("session_context").is_none());
    assert!(
        parsed["data"]["body"]
            .as_str()
            .unwrap()
            .contains("connect_required")
    );

    client.cancel().await?;
    server.await??;
    Ok(())
}

#[tokio::test]
async fn connected_session_passes_runtime_tool_preflight() {
    let mcp = make_mcp().await;
    let general = tempfile::tempdir().unwrap();
    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: general.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["status"], "success");
    assert!(mcp.connect_preflight_for_tests("read_live_feed").is_none());
}

#[tokio::test]
async fn general_mode_blocks_unfiltered_live_feed_reads() {
    let mcp = make_mcp().await;
    let general = tempfile::tempdir().unwrap();
    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: general.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["data"]["mode"], "general");

    let blocked = mcp.read_live_feed_for_tests().await;
    let parsed: serde_json::Value = serde_json::from_str(&blocked).unwrap();
    assert_eq!(parsed["status"], "blocked");
    assert_eq!(parsed["code"], "narrow_filter_required");
    assert_eq!(parsed["data"]["mode"], "general");
    assert!(parsed["data"]["scope_root"].is_string());
    assert_eq!(parsed["next_actions"][0]["tool"], "read_live_feed");
    assert_eq!(
        parsed["next_actions"][0]["reason"],
        "general mode scopes the entire daemon -- add a narrowing filter (severity_min, kinds, origins, service, source, tags, or text_match) to focus observations"
    );
    assert_eq!(parsed["next_actions"][0]["params"]["severity_min"], "warn");

    let narrowed = mcp
        .read_live_feed_for_tests_with(ObserveParams {
            severity_min: Some("warn".into()),
            ..Default::default()
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&narrowed).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["mode"], "general");
    assert!(parsed["data"]["connection"]["scope_root"].is_string());

    let narrowed = mcp
        .read_live_feed_for_tests_with(ObserveParams {
            service: Some(vec!["cargo".into()]),
            ..Default::default()
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&narrowed).unwrap();
    assert_eq!(parsed["status"], "success");
}

#[tokio::test]
async fn mcp_live_feed_filters_by_provenance() {
    let mcp = make_mcp_with_writer().await;
    let general = tempfile::tempdir().unwrap();
    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: general.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["status"], "success");

    let written = mcp
        .write_to_live_feed_for_tests(IngestParams {
            kind: Some("log".into()),
            severity: Some("info".into()),
            app: Some("cargo".into()),
            channel: None,
            correlation_id: None,
            parent_id: None,
            tags: None,
            session_id: None,
            node_id: None,
            service: Some("cargo".into()),
            source: Some("cargo.check".into()),
            source_instance: Some("target/daemon8/cargo-check.log".into()),
            data: serde_json::json!({"msg": "cargo check failed"}),
        })
        .await;
    let parsed_write: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(parsed_write["data"]["queued"], true);
    mcp.write_to_live_feed_for_tests(IngestParams {
        kind: Some("log".into()),
        severity: Some("info".into()),
        app: Some("claude".into()),
        channel: None,
        correlation_id: None,
        parent_id: None,
        tags: None,
        session_id: None,
        node_id: None,
        service: Some("claude".into()),
        source: Some("claude.conversations".into()),
        source_instance: Some("session.jsonl".into()),
        data: serde_json::json!({"msg": "assistant turn"}),
    })
    .await;
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let filtered = mcp
        .read_live_feed_for_tests_with(ObserveParams {
            service: Some(vec!["cargo".into()]),
            source: Some(vec!["cargo.check".into()]),
            source_instance: Some(vec!["target/daemon8/cargo-check.log".into()]),
            ..Default::default()
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&filtered).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["observations"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["data"]["observations"][0]["service"], "cargo");
}

#[tokio::test]
async fn live_feed_warning_since_checkpoint_without_active_debug_session_stays_on_feed() {
    let mcp = make_mcp_with_writer().await;
    let general = tempfile::tempdir().unwrap();
    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: general.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["status"], "success");

    mcp.write_to_live_feed_for_tests(IngestParams {
        kind: Some("log".into()),
        severity: Some("warn".into()),
        app: Some("agent".into()),
        channel: None,
        correlation_id: None,
        parent_id: None,
        tags: None,
        session_id: None,
        node_id: None,
        service: Some("agent".into()),
        source: Some("agent.notes".into()),
        source_instance: Some("mcp".into()),
        data: serde_json::json!({"message": "warn after checkpoint"}),
    })
    .await;
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let feed = mcp
        .read_live_feed_for_tests_with(ObserveParams {
            since_checkpoint: Some(0),
            service: Some(vec!["agent".into()]),
            ..Default::default()
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&feed).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["next_actions"][0]["tool"], "read_live_feed");
    assert_eq!(parsed["next_actions"][0]["params"]["since_checkpoint"], 0);
    assert_eq!(parsed["next_actions"][0]["params"]["severity_min"], "warn");
    assert_eq!(parsed["next_actions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn live_feed_warning_since_checkpoint_in_active_project_debug_session_can_resolve() {
    let mcp = make_mcp_with_debug_and_writer().await;
    let project = tempfile::tempdir().unwrap();
    mark_project(project.path());
    let init = mcp
        .daemon8_init_for_tests(Daemon8InitParams {
            project_path: project.path().display().to_string(),
            name: Some("mcp-project".into()),
            overwrite: None,
            ignore: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&init).unwrap();
    assert_eq!(parsed["status"], "success");

    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: project.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["data"]["mode"], "project");

    let started = mcp
        .start_debug_session_for_tests(StartDebugSessionParams {
            project: Some("daemon8".into()),
            description: Some("warning follow-up".into()),
            agent_id: ":host/codex+agent>".into(),
            feature: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&started).unwrap();
    assert_eq!(parsed["status"], "success");

    mcp.write_to_live_feed_for_tests(IngestParams {
        kind: Some("log".into()),
        severity: Some("warn".into()),
        app: Some("agent".into()),
        channel: None,
        correlation_id: None,
        parent_id: None,
        tags: None,
        session_id: None,
        node_id: None,
        service: Some("agent".into()),
        source: Some("agent.notes".into()),
        source_instance: Some("mcp".into()),
        data: serde_json::json!({"message": "warn during active debug"}),
    })
    .await;
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let feed = mcp
        .read_live_feed_for_tests_with(ObserveParams {
            since_checkpoint: Some(0),
            service: Some(vec!["agent".into()]),
            ..Default::default()
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&feed).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["next_actions"][0]["tool"], "read_live_feed");
    assert_eq!(parsed["next_actions"][1]["tool"], "resolve_debug_session");
    assert_eq!(
        parsed["next_actions"][1]["params"]["summary"],
        "<durable conclusion after interpreting the runtime signal>"
    );
}

#[tokio::test]
async fn read_live_feed_skips_read_through_without_cursor_store() {
    let mcp = make_mcp_with_writer().await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());
    let log = tmp.path().join("cargo.log");
    std::fs::write(&log, "first\n").unwrap();
    write_file_source_config(tmp.path(), "cargo.check", "cargo.log");

    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["status"], "success");

    std::fs::write(&log, "first\nsecond\n").unwrap();
    let feed = mcp
        .read_live_feed_for_tests_with(ObserveParams {
            source: Some(vec!["cargo.check".into()]),
            ..Default::default()
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&feed).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["observations"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn read_live_feed_returns_file_source_observations_via_read_through() {
    let mcp = make_mcp_with_read_through().await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());
    let log = tmp.path().join("cargo.log");
    std::fs::write(&log, "first\nsecond\n").unwrap();
    write_file_source_config(tmp.path(), "cargo.check", "cargo.log");

    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["status"], "success");

    let feed = mcp
        .read_live_feed_for_tests_with(ObserveParams {
            source: Some(vec!["cargo.check".into()]),
            ..Default::default()
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&feed).unwrap();
    assert_eq!(parsed["status"], "success");
    let observations = parsed["data"]["observations"].as_array().unwrap();
    assert_eq!(observations.len(), 2);
    assert_eq!(observations[0]["data"]["message"], "first");
    assert_eq!(observations[1]["data"]["message"], "second");
    assert_eq!(observations[0]["service"], "cargo");
    assert_eq!(observations[0]["source"], "cargo.check");
}

#[tokio::test]
async fn read_through_advances_cursor_between_reads() {
    let mcp = make_mcp_with_read_through().await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());
    let log = tmp.path().join("cargo.log");
    std::fs::write(&log, "first\n").unwrap();
    write_file_source_config(tmp.path(), "cargo.check", "cargo.log");

    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["status"], "success");

    let feed1 = mcp
        .read_live_feed_for_tests_with(ObserveParams {
            source: Some(vec!["cargo.check".into()]),
            ..Default::default()
        })
        .await;
    let parsed1: serde_json::Value = serde_json::from_str(&feed1).unwrap();
    assert_eq!(parsed1["data"]["observations"].as_array().unwrap().len(), 1);

    std::fs::write(&log, "first\nsecond\n").unwrap();
    let feed2 = mcp
        .read_live_feed_for_tests_with(ObserveParams {
            source: Some(vec!["cargo.check".into()]),
            ..Default::default()
        })
        .await;
    let parsed2: serde_json::Value = serde_json::from_str(&feed2).unwrap();
    let observations = parsed2["data"]["observations"].as_array().unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0]["data"]["message"], "second");
}

#[tokio::test]
async fn read_through_merges_with_surreal_observations() {
    let mcp = make_mcp_with_read_through().await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());
    let log = tmp.path().join("cargo.log");
    std::fs::write(&log, "file-line\n").unwrap();
    write_file_source_config(tmp.path(), "cargo.check", "cargo.log");

    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["status"], "success");

    mcp.write_to_live_feed_for_tests(IngestParams {
        kind: Some("log".into()),
        severity: Some("info".into()),
        app: Some("test-app".into()),
        data: serde_json::json!({"message": "surreal-line"}),
        channel: None,
        correlation_id: None,
        parent_id: None,
        tags: None,
        session_id: None,
        node_id: None,
        service: None,
        source: None,
        source_instance: None,
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let feed = mcp
        .read_live_feed_for_tests_with(ObserveParams::default())
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&feed).unwrap();
    assert_eq!(parsed["status"], "success");
    let observations = parsed["data"]["observations"].as_array().unwrap();
    assert!(
        observations.len() >= 2,
        "expected surreal + file observations, got {}",
        observations.len()
    );
    let messages: Vec<&str> = observations
        .iter()
        .filter_map(|o| o["data"]["message"].as_str())
        .collect();
    assert!(
        messages.contains(&"surreal-line"),
        "missing SurrealDB observation"
    );
    assert!(
        messages.contains(&"file-line"),
        "missing file-source observation"
    );

    let timestamps: Vec<u64> = observations
        .iter()
        .filter_map(|o| o["timestamp_ns"].as_u64())
        .collect();
    let mut sorted = timestamps.clone();
    sorted.sort();
    assert_eq!(
        timestamps, sorted,
        "observations must be sorted by timestamp_ns"
    );
}

#[tokio::test]
async fn read_through_merge_respects_limit() {
    let mcp = make_mcp_with_read_through().await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());
    let log = tmp.path().join("cargo.log");
    std::fs::write(&log, "a\nb\nc\n").unwrap();
    write_file_source_config(tmp.path(), "cargo.check", "cargo.log");

    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["status"], "success");

    let feed = mcp
        .read_live_feed_for_tests_with(ObserveParams {
            limit: Some(2),
            source: Some(vec!["cargo.check".into()]),
            ..Default::default()
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&feed).unwrap();
    assert_eq!(parsed["status"], "success");
    let observations = parsed["data"]["observations"].as_array().unwrap();
    assert_eq!(observations.len(), 2, "limit should cap at 2");
    assert_eq!(
        observations[0]["data"]["message"], "b",
        "should keep newest 2"
    );
    assert_eq!(observations[1]["data"]["message"], "c");
}

#[tokio::test]
async fn general_mode_blocks_project_only_tools() {
    let mcp = make_mcp_with_debug().await;
    let general = tempfile::tempdir().unwrap();
    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: general.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["data"]["mode"], "general");

    assert!(
        mcp.connect_preflight_for_tests("list_debug_sessions")
            .is_none()
    );

    for tool in [
        "start_debug_session",
        "create_checkpoint",
        "resolve_debug_session",
        "end_debug_session",
        "link_conversation",
        "build_context_snapshot",
    ] {
        let blocked = mcp
            .connect_preflight_for_tests(tool)
            .unwrap_or_else(|| panic!("{tool} should require project scope in general mode"));
        let parsed: serde_json::Value = serde_json::from_str(&blocked).unwrap();
        assert_eq!(parsed["status"], "blocked");
        assert_eq!(parsed["code"], "project_required");
        assert_eq!(parsed["next_actions"][0]["tool"], "daemon8_connect");
    }
}

#[tokio::test]
async fn create_checkpoint_is_pure_sequence_bookmark() {
    let mcp = make_mcp_with_debug().await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());
    write_file_source_config(tmp.path(), "cargo.check", "cargo.log");

    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["status"], "success");

    let started = mcp
        .start_debug_session_for_tests(StartDebugSessionParams {
            project: Some("daemon8".into()),
            description: Some("source checkpoint".into()),
            agent_id: ":host/codex+agent>".into(),
            feature: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&started).unwrap();
    assert_eq!(parsed["status"], "success");

    let checkpoint = mcp
        .create_checkpoint_for_tests(CreateCheckpointParams {
            description: Some("pure sequence bookmark".into()),
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&checkpoint).unwrap();
    assert_eq!(parsed["status"], "success");
    assert!(parsed["data"]["seq_at_creation"].as_u64().is_some());
    assert!(parsed["data"]["checkpoint_id"].as_str().is_some());
}

#[tokio::test]
async fn debug_tools_obey_policy_through_real_mcp_calls() -> anyhow::Result<()> {
    let mcp = make_mcp_with_debug().await;
    let general = tempfile::tempdir().unwrap();
    let path = general.path().display().to_string();
    let (server_transport, client_transport) = tokio::io::duplex(8192);

    let server = tokio::spawn(async move {
        mcp.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = TestClient.serve(client_transport).await?;
    let pre_connect = client
        .call_tool(tool_request(
            "start_debug_session",
            serde_json::json!({
                "project": "daemon8",
                "description": "pre-connect guard",
                "agent_id": ":host/codex+agent>",
                "feature": null,
            }),
        ))
        .await?;
    let parsed = result_json(&pre_connect);
    assert_eq!(parsed["status"], "connect_required");
    assert_eq!(parsed["code"], "connect_required");

    let connect = client
        .call_tool(tool_request(
            "daemon8_connect",
            serde_json::json!({
                "provider": "codex",
                "project_path": path,
                "agent_name": null,
                "transcript_path": null,
            }),
        ))
        .await?;
    let parsed = result_json(&connect);
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["mode"], "general");

    let listed = client
        .call_tool(tool_request(
            "list_debug_sessions",
            serde_json::json!({
                "status": null,
                "feature": null,
            }),
        ))
        .await?;
    let parsed = result_json(&listed);
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["code"], "debug_sessions_listed");

    let blocked = client
        .call_tool(tool_request(
            "start_debug_session",
            serde_json::json!({
                "project": "daemon8",
                "description": "general guard",
                "agent_id": ":host/codex+agent>",
                "feature": null,
            }),
        ))
        .await?;
    let parsed = result_json(&blocked);
    assert_eq!(parsed["status"], "blocked");
    assert_eq!(parsed["code"], "project_required");
    assert_eq!(
        parsed["next_actions"][0]["reason"],
        "bind this MCP session to a project scope"
    );
    assert_eq!(
        parsed["next_actions"][0]["params"]["provider"],
        "<provider>"
    );

    client.cancel().await?;
    server.await??;
    Ok(())
}

#[tokio::test]
async fn debug_lifecycle_codes_are_stable_through_real_mcp_calls() -> anyhow::Result<()> {
    let mcp = make_mcp_with_debug().await;
    let project = tempfile::tempdir().unwrap();
    mark_project(project.path());
    let path = project.path().display().to_string();
    let (server_transport, client_transport) = tokio::io::duplex(8192);

    let server = tokio::spawn(async move {
        mcp.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });

    let client = TestClient.serve(client_transport).await?;
    let init = client
        .call_tool(tool_request(
            "daemon8_init",
            serde_json::json!({
                "project_path": path.clone(),
                "name": "daemon8-test",
                "overwrite": null,
            }),
        ))
        .await?;
    let parsed = result_json(&init);
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["code"], "initialized");

    let connect = client
        .call_tool(tool_request(
            "daemon8_connect",
            serde_json::json!({
                "provider": "codex",
                "project_path": path,
                "agent_name": null,
                "transcript_path": null,
            }),
        ))
        .await?;
    let parsed = result_json(&connect);
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["mode"], "project");

    let started = client
        .call_tool(tool_request(
            "start_debug_session",
            serde_json::json!({
                "project": "daemon8",
                "description": "router lifecycle",
                "agent_id": ":host/codex+agent>",
                "feature": "mcp",
            }),
        ))
        .await?;
    let parsed = result_json(&started);
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["code"], "debug_session_started");
    assert_eq!(parsed["data"]["mode"], "project");
    assert!(parsed["data"]["scope_root"].is_string());
    let first_session_id = parsed["data"]["debug_session_id"]
        .as_str()
        .expect("start response must include debug_session_id")
        .to_string();

    let checkpoint = client
        .call_tool(tool_request(
            "create_checkpoint",
            serde_json::json!({
                "description": "before resolve",
            }),
        ))
        .await?;
    let parsed = result_json(&checkpoint);
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["code"], "checkpoint_created");
    assert_eq!(parsed["data"]["debug_session_id"], first_session_id);
    assert_eq!(parsed["next_actions"][0]["tool"], "read_live_feed");
    assert!(parsed["next_actions"][0]["params"]["since_checkpoint"].is_u64());

    let resolved = client
        .call_tool(tool_request(
            "resolve_debug_session",
            serde_json::json!({
                "summary": "router lifecycle verified",
                "root_cause": null,
                "fix_diff": null,
                "commands_used": null,
                "related_errors": null,
                "tags": ["test"]
            }),
        ))
        .await?;
    let parsed = result_json(&resolved);
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["code"], "debug_session_resolved");
    assert_eq!(parsed["data"]["debug_session_id"], first_session_id);
    assert_eq!(parsed["data"]["checkpoint_count"], 1);
    assert_eq!(parsed["data"]["evidence_ref"]["kind"], "session_summary");
    assert_eq!(parsed["next_actions"][0]["tool"], "start_debug_session");
    assert_eq!(
        parsed["next_actions"][0]["reason"],
        "open a new debug session so follow-up work gets its own checkpoints and retrievable summary"
    );
    assert_eq!(parsed["next_actions"][1]["tool"], "list_debug_sessions");
    assert_eq!(
        parsed["next_actions"][1]["reason"],
        "review recent sessions to check for overlapping work before starting a new investigation"
    );

    let completed = client
        .call_tool(tool_request(
            "list_debug_sessions",
            serde_json::json!({
                "status": "completed",
                "feature": "mcp",
            }),
        ))
        .await?;
    let parsed = result_json(&completed);
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["code"], "debug_sessions_listed");
    assert_eq!(parsed["data"]["count"], 1);

    let restarted = client
        .call_tool(tool_request(
            "start_debug_session",
            serde_json::json!({
                "project": "daemon8",
                "description": "router end lifecycle",
                "agent_id": ":host/codex+agent>",
                "feature": "mcp",
            }),
        ))
        .await?;
    let parsed = result_json(&restarted);
    assert_eq!(parsed["code"], "debug_session_started");
    let second_session_id = parsed["data"]["debug_session_id"]
        .as_str()
        .expect("restart response must include debug_session_id")
        .to_string();

    let rejected = client
        .call_tool(tool_request(
            "end_debug_session",
            serde_json::json!({
                "outcome": "resolved",
            }),
        ))
        .await?;
    let parsed = result_json(&rejected);
    assert_eq!(parsed["status"], "error");
    assert_eq!(parsed["code"], "bad_outcome");
    assert_eq!(parsed["next_actions"][0]["tool"], "resolve_debug_session");
    assert_eq!(
        parsed["next_actions"][0]["params"]["summary"],
        "<what changed and why it fixed the issue>"
    );

    let ended = client
        .call_tool(tool_request(
            "end_debug_session",
            serde_json::json!({
                "outcome": null,
            }),
        ))
        .await?;
    let parsed = result_json(&ended);
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["code"], "debug_session_ended");
    assert_eq!(parsed["data"]["debug_session_id"], second_session_id);

    let abandoned = client
        .call_tool(tool_request(
            "list_debug_sessions",
            serde_json::json!({
                "status": "abandoned",
                "feature": "mcp",
            }),
        ))
        .await?;
    let parsed = result_json(&abandoned);
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["code"], "debug_sessions_listed");
    assert_eq!(parsed["data"]["count"], 1);

    client.cancel().await?;
    server.await??;
    Ok(())
}

#[tokio::test]
async fn already_active_debug_session_prefers_resolution_before_abandoning() {
    let mcp = make_mcp_with_debug().await;
    let project = tempfile::tempdir().unwrap();
    mark_project(project.path());

    let init = mcp
        .daemon8_init_for_tests(Daemon8InitParams {
            project_path: project.path().display().to_string(),
            name: None,
            overwrite: None,
            ignore: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&init).unwrap();
    assert_eq!(parsed["status"], "success");

    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: project.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["data"]["mode"], "project");

    let first = mcp
        .start_debug_session_for_tests(StartDebugSessionParams {
            project: Some("daemon8".into()),
            description: Some("first".into()),
            agent_id: ":host/codex+agent>".into(),
            feature: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(parsed["status"], "success");

    let second = mcp
        .start_debug_session_for_tests(StartDebugSessionParams {
            project: Some("daemon8".into()),
            description: Some("second".into()),
            agent_id: ":host/codex+agent>".into(),
            feature: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&second).unwrap();
    assert_eq!(parsed["status"], "error");
    assert_eq!(parsed["code"], "already_active_debug_session");
    assert_eq!(parsed["next_actions"][0]["tool"], "resolve_debug_session");
    assert_eq!(parsed["next_actions"][1]["tool"], "end_debug_session");
}

#[tokio::test]
async fn active_debug_session_flush_persists_touched_activity() {
    let mcp = make_mcp_with_debug_and_writer().await;
    let start = mcp
        .start_debug_session_for_tests(StartDebugSessionParams {
            project: Some("daemon8".into()),
            description: Some("touch coverage".into()),
            agent_id: ":host/codex+agent>".into(),
            feature: Some("flush".into()),
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&start).unwrap();
    assert_eq!(parsed["status"], "success");

    let before = mcp
        .list_debug_sessions_for_tests(ListDebugSessionsParams {
            status: Some("active".into()),
            feature: Some("flush".into()),
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&before).unwrap();
    let before_activity = parsed["data"]["sessions"][0]["last_activity"]
        .as_u64()
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    mcp.write_to_live_feed_for_tests(IngestParams {
        kind: Some("log".into()),
        severity: Some("info".into()),
        app: Some("agent".into()),
        channel: None,
        correlation_id: None,
        parent_id: None,
        tags: None,
        session_id: None,
        node_id: None,
        service: Some("agent".into()),
        source: Some("agent.notes".into()),
        source_instance: Some("mcp".into()),
        data: serde_json::json!({"message": "activity"}),
    })
    .await;
    mcp.flush_active_debug_session_for_tests().await.unwrap();

    let after = mcp
        .list_debug_sessions_for_tests(ListDebugSessionsParams {
            status: Some("active".into()),
            feature: Some("flush".into()),
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&after).unwrap();
    let after_activity = parsed["data"]["sessions"][0]["last_activity"]
        .as_u64()
        .unwrap();

    assert!(after_activity > before_activity);
}

#[tokio::test]
async fn project_mode_allows_project_only_debug_tools() {
    let mcp = make_mcp_with_debug().await;
    let project = tempfile::tempdir().unwrap();
    mark_project(project.path());

    let init = mcp
        .daemon8_init_for_tests(Daemon8InitParams {
            project_path: project.path().display().to_string(),
            name: None,
            overwrite: None,
            ignore: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&init).unwrap();
    assert_eq!(parsed["status"], "success");

    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: project.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["data"]["mode"], "project");

    for tool in DEBUG_TOOLS {
        assert!(
            mcp.connect_preflight_for_tests(tool).is_none(),
            "{tool} should pass preflight in project mode"
        );
    }
}

#[tokio::test]
async fn issue_command_missing_param_uses_alpha_envelope() {
    let mcp = make_mcp().await;
    let body = mcp
        .issue_command_for_tests(act_params(DebugAction::EvalJs))
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["status"], "error");
    assert_eq!(parsed["code"], "missing_param");
    assert!(parsed["data"]["session_id"].is_string());
    assert!(parsed["data"]["connection"].is_null());
    assert!(parsed.get("result").is_none());
    assert!(parsed.get("error").is_none());
}

#[tokio::test]
async fn daemon8_failed_reconnect_clears_previous_session_connection() {
    let mcp = make_mcp().await;
    let general = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname = \"x\"\n",
    )
    .unwrap();

    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: general.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["mode"], "general");

    let blocked = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: project.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&blocked).unwrap();
    assert_eq!(parsed["status"], "setup_required");
    assert_eq!(parsed["code"], "missing_config");

    let status = mcp.daemon8_status_for_tests().await;
    let parsed: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert!(parsed["data"]["connection"].is_null());
}

#[tokio::test]
async fn read_live_feed_description_mentions_full_surface() {
    let mcp = make_mcp().await;
    let tools = mcp.tools_for_client();
    let observe = tools
        .iter()
        .find(|t| t.name == "read_live_feed")
        .expect("read_live_feed must be present in tools_for_client()");
    let desc = observe.description.as_deref().unwrap_or("");
    for term in ["browser", "device", "js_exception"] {
        assert!(
            desc.contains(term),
            "read_live_feed description must contain '{term}'. Got: {:?}",
            &desc[..desc.len().min(300)]
        );
    }
}

#[tokio::test]
async fn server_instructions_mention_action_surface() {
    use rmcp::ServerHandler as _;

    let mcp = make_mcp().await;
    let text = mcp
        .get_info()
        .instructions
        .as_deref()
        .unwrap_or("")
        .to_string();

    assert!(
        text.contains("browser"),
        "instructions must mention browser. Got: {:?}",
        &text[..text.len().min(300)]
    );
    assert!(
        text.contains("issue_command"),
        "instructions must mention issue_command. Got: {:?}",
        &text[..text.len().min(300)]
    );
    for term in [
        "Do not run conversation recall automatically",
        "ask once",
        "build_context_snapshot` with no `facets` filter",
        "link_conversation",
        "Do not rely on the visible chat as the whole project history",
    ] {
        assert!(
            text.contains(term),
            "instructions must contain '{term}'. Got: {:?}",
            &text[..text.len().min(600)]
        );
    }
}

#[tokio::test]
async fn parent_cancel_propagates_to_session_child_token() {
    let parent = CancellationToken::new();
    let mcp = make_mcp_with_cancel(parent.clone()).await;
    let session = mcp.child_cancel_token();

    assert!(
        !session.is_cancelled(),
        "child token must start uncancelled"
    );
    parent.cancel();
    assert!(
        session.is_cancelled(),
        "parent cancellation must propagate to session-derived child"
    );
}

#[tokio::test]
async fn list_connections_browser_key_visible() {
    let mcp = make_mcp().await;
    let raw = mcp.connections_json().await;
    let val: serde_json::Value =
        serde_json::from_str(&raw).expect("connections_json must return valid JSON");
    assert!(
        val.get("browser").is_some(),
        "connections_json must contain 'browser' key. Got: {val}"
    );
}

#[tokio::test]
async fn subscription_filters_are_per_session() {
    use daemon8_types::Severity;

    let mcp_a = make_mcp().await;
    let mcp_b = make_mcp().await;

    let mut rx_a = mcp_a.subscription_rx();
    let mut rx_b = mcp_b.subscription_rx();

    let _ = rx_a.borrow_and_update();
    let _ = rx_b.borrow_and_update();

    let filter_a = Filter {
        severity_min: Some(Severity::Warn),
        ..Filter::default()
    };
    let filter_b = Filter {
        severity_min: Some(Severity::Error),
        ..Filter::default()
    };

    mcp_a.set_subscription(Some(filter_a.clone()));
    assert!(
        rx_a.has_changed().expect("rx_a still alive"),
        "session A receiver should observe its own write"
    );
    assert!(
        !rx_b.has_changed().expect("rx_b still alive"),
        "session A write must not perturb session B"
    );

    mcp_b.set_subscription(Some(filter_b));
    let a = rx_a.borrow_and_update().clone().expect("session A filter");
    let b = rx_b.borrow_and_update().clone().expect("session B filter");

    assert_eq!(a.severity_min, Some(Severity::Warn));
    assert_eq!(b.severity_min, Some(Severity::Error));
}

#[tokio::test]
async fn help_index_includes_core_topics() {
    let mcp = make_mcp().await;
    let index = mcp.help_topic_body("index").1;
    assert!(
        index.contains("observations"),
        "index must always contain observations"
    );
    assert!(
        index.contains("envelope"),
        "index must always contain envelope"
    );
    assert!(
        index.contains("getting_started"),
        "index must always contain getting_started"
    );
    assert!(index.contains("setup"), "index must always contain setup");
    assert!(
        index.contains("sources"),
        "index must always contain sources"
    );
}

#[tokio::test]
async fn help_unknown_topic_falls_back_to_index() {
    let mcp = make_mcp().await;
    let result = mcp.help_topic_body("nonexistent_topic");
    assert!(result.0 == "index", "unknown topic must resolve to 'index'");
    assert!(
        result.1.contains("## Topics"),
        "fallback body must be the index"
    );
}

#[test]
fn tool_descriptions_do_not_describe_removed_envelope() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tool_descriptions");
    let mut stack = vec![dir];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }
            let body = std::fs::read_to_string(&path).unwrap();
            for stale in [
                "daemon8.active_debug_session",
                "daemon8.next_actions",
                "fix.tool",
                "error.message",
                "result:",
                "result.",
            ] {
                assert!(
                    !body.contains(stale),
                    "{} still mentions removed envelope fragment {stale:?}",
                    path.display()
                );
            }
        }
    }
}

#[tokio::test]
async fn daemon8_init_ignore_true_returns_project_ignored() {
    let mcp = make_mcp().await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());

    let init = mcp
        .daemon8_init_for_tests(Daemon8InitParams {
            project_path: tmp.path().display().to_string(),
            name: None,
            overwrite: None,
            ignore: Some(true),
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&init).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["code"], "project_ignored");
    assert!(parsed["data"]["scope_root"].is_string());
    assert!(!tmp.path().join(".daemon8").join("config.md").exists());
}

#[tokio::test]
async fn daemon8_connect_ignored_project_returns_blocked() {
    let mcp = make_mcp().await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());

    let init = mcp
        .daemon8_init_for_tests(Daemon8InitParams {
            project_path: tmp.path().display().to_string(),
            name: None,
            overwrite: None,
            ignore: Some(true),
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&init).unwrap();
    assert_eq!(parsed["code"], "project_ignored");

    let connect = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "blocked");
    assert_eq!(parsed["code"], "project_ignored");
    assert!(parsed["data"]["connection"].is_null());
}

#[tokio::test]
async fn daemon8_init_ignore_false_then_connect_returns_setup_required() {
    let mcp = make_mcp().await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());

    mcp.daemon8_init_for_tests(Daemon8InitParams {
        project_path: tmp.path().display().to_string(),
        name: None,
        overwrite: None,
        ignore: Some(true),
    })
    .await;

    let unignore = mcp
        .daemon8_init_for_tests(Daemon8InitParams {
            project_path: tmp.path().display().to_string(),
            name: None,
            overwrite: None,
            ignore: Some(false),
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&unignore).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["code"], "project_unignored");
    assert_eq!(parsed["next_actions"][0]["tool"], "daemon8_connect");

    let connect = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "setup_required");
    assert_eq!(parsed["code"], "missing_config");
}

#[tokio::test]
async fn build_context_snapshot_requires_project_mode() {
    let mcp = make_mcp_with_debug().await;
    let home = test_home_dir();
    let project = home.join("snap-general");
    std::fs::create_dir_all(&project).unwrap();
    mark_project(&project);

    let connect = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            project_path: project.display().to_string(),
            provider: "claude".into(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let connect_parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(
        connect_parsed["status"], "setup_required",
        "connect without config should be setup_required"
    );
    // Without config, session stays unconnected. build_context_snapshot is
    // ProjectOnly, so connect_preflight rejects with connect_required.
    let snap = mcp
        .build_context_snapshot_for_tests(BuildContextSnapshotParams {
            since: None,
            facets: None,
            providers: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&snap).unwrap();
    assert_eq!(
        parsed["status"], "connect_required",
        "snapshot should require a connected project scope"
    );
}

#[tokio::test]
async fn build_context_snapshot_with_transcript() {
    let home = test_home_dir();
    let project = home.join("snap-project");
    std::fs::create_dir_all(&project).unwrap();
    mark_project(&project);
    write_empty_project_config(&project);

    // Write transcript in the Claude provider directory so transcript
    // validation during connect accepts it. Use canonical paths to avoid
    // /var vs /private/var mismatch on macOS.
    let canonical_project = std::fs::canonicalize(&project).unwrap();
    let slug = canonical_project.to_string_lossy().replace('/', "-");
    let canonical_home = std::fs::canonicalize(&home).unwrap();
    let claude_project_dir = canonical_home.join(".claude/projects").join(&slug);
    std::fs::create_dir_all(&claude_project_dir).unwrap();
    let transcript = claude_project_dir.join("test-session.jsonl");

    // Include cwd in the permission-mode line so the scope check matches
    let cwd = canonical_project.display().to_string();
    let transcript_content = format!(
        r#"{{"type":"permission-mode","permissionMode":"bypassPermissions","isSidechain":false,"sessionId":"s1","cwd":"{cwd}"}}
{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"fix the login bug"}}]}},"timestamp":"2026-05-22T10:00:00.000Z"}}
{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"call_1","name":"Read","input":{{"file_path":"/src/auth.rs"}}}}]}},"timestamp":"2026-05-22T10:00:01.000Z"}}
{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"I found the bug."}}]}},"timestamp":"2026-05-22T10:00:05.000Z"}}
"#
    );
    std::fs::write(&transcript, &transcript_content).unwrap();

    let mcp = make_mcp_with_cancel_and_home(CancellationToken::new(), canonical_home.clone()).await;

    let connect = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            project_path: canonical_project.display().to_string(),
            provider: "claude".into(),
            agent_name: None,
            transcript_path: Some(transcript.display().to_string()),
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(
        parsed["status"], "success",
        "connect should succeed: {connect}"
    );

    let snap = mcp
        .build_context_snapshot_for_tests(BuildContextSnapshotParams {
            since: None,
            facets: None,
            providers: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&snap).unwrap();
    assert_eq!(
        parsed["status"], "success",
        "snapshot should succeed: {snap}"
    );
    assert_eq!(parsed["code"], "snapshot_built");

    let data = &parsed["data"];
    assert!(!data["sources_read"].as_array().unwrap().is_empty());
    assert!(!data["facets"].as_object().unwrap().is_empty());

    let snapshot_path = data["snapshot_path"].as_str().unwrap();
    let snapshot_dir = std::path::PathBuf::from(snapshot_path);
    assert!(
        snapshot_dir.join("user-messages.md").exists(),
        "user-messages.md should exist"
    );
    assert!(
        snapshot_dir.join("summary.md").exists(),
        "summary.md should exist"
    );

    // Error path: invalid facet
    let snap_err = mcp
        .build_context_snapshot_for_tests(BuildContextSnapshotParams {
            since: None,
            facets: Some(vec!["nonexistent".into()]),
            providers: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&snap_err).unwrap();
    assert_eq!(parsed["code"], "invalid_facet");

    // Error path: invalid since
    let snap_err = mcp
        .build_context_snapshot_for_tests(BuildContextSnapshotParams {
            since: Some("garbage".into()),
            facets: None,
            providers: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&snap_err).unwrap();
    assert_eq!(parsed["code"], "invalid_since_param");

    // Subset facets through handler
    let snap_subset = mcp
        .build_context_snapshot_for_tests(BuildContextSnapshotParams {
            since: None,
            facets: Some(vec!["summary".into()]),
            providers: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&snap_subset).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["facets"].as_object().unwrap().len(), 1);
    let second_snapshot_dir =
        std::path::PathBuf::from(parsed["data"]["snapshot_path"].as_str().unwrap());
    assert_ne!(
        snapshot_dir, second_snapshot_dir,
        "each snapshot build should get a unique run directory"
    );
    assert!(
        snapshot_dir.exists(),
        "first snapshot should not be overwritten"
    );
    assert!(
        second_snapshot_dir.exists(),
        "second snapshot should be written separately"
    );
}

#[tokio::test]
async fn build_context_snapshot_no_transcript_sources() {
    let mcp = make_mcp_with_debug().await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());
    write_empty_project_config(tmp.path());

    let connect = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            project_path: tmp.path().display().to_string(),
            provider: "codex".into(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "success", "connect: {connect}");

    let snap = mcp
        .build_context_snapshot_for_tests(BuildContextSnapshotParams {
            since: None,
            facets: None,
            providers: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&snap).unwrap();
    assert_eq!(parsed["code"], "no_transcript_sources");
    assert_eq!(parsed["next_actions"][0]["tool"], "link_conversation");
}

#[tokio::test]
async fn build_context_snapshot_checkpoint_without_debug_session() {
    let home = test_home_dir();
    let project = home.join("snap-cp-nosess");
    std::fs::create_dir_all(&project).unwrap();
    mark_project(&project);
    write_empty_project_config(&project);

    let canonical_project = std::fs::canonicalize(&project).unwrap();
    let slug = canonical_project.to_string_lossy().replace('/', "-");
    let canonical_home = std::fs::canonicalize(&home).unwrap();
    let claude_dir = canonical_home.join(".claude/projects").join(&slug);
    std::fs::create_dir_all(&claude_dir).unwrap();
    let transcript = claude_dir.join("cp-test.jsonl");
    let cwd = canonical_project.display().to_string();
    std::fs::write(
        &transcript,
        format!(
            r#"{{"type":"permission-mode","permissionMode":"bypassPermissions","isSidechain":false,"sessionId":"s1","cwd":"{cwd}"}}
{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"test"}}]}},"timestamp":"2026-05-22T10:00:00.000Z"}}
"#
        ),
    )
    .unwrap();

    let mcp = make_mcp_with_cancel_and_home(CancellationToken::new(), canonical_home.clone()).await;

    let connect = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            project_path: canonical_project.display().to_string(),
            provider: "claude".into(),
            agent_name: None,
            transcript_path: Some(transcript.display().to_string()),
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "success", "connect: {connect}");

    let snap = mcp
        .build_context_snapshot_for_tests(BuildContextSnapshotParams {
            since: Some("checkpoint".into()),
            facets: None,
            providers: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&snap).unwrap();
    assert_eq!(parsed["code"], "no_active_checkpoint");
}

#[tokio::test]
async fn build_context_snapshot_checkpoint_without_created_checkpoint() {
    let home = test_home_dir();
    let project = home.join("snap-cp-nocp");
    std::fs::create_dir_all(&project).unwrap();
    mark_project(&project);
    write_empty_project_config(&project);

    let canonical_project = std::fs::canonicalize(&project).unwrap();
    let slug = canonical_project.to_string_lossy().replace('/', "-");
    let canonical_home = std::fs::canonicalize(&home).unwrap();
    let claude_dir = canonical_home.join(".claude/projects").join(&slug);
    std::fs::create_dir_all(&claude_dir).unwrap();
    let transcript = claude_dir.join("cp-nocp-test.jsonl");
    let cwd = canonical_project.display().to_string();
    std::fs::write(
        &transcript,
        format!(
            r#"{{"type":"permission-mode","permissionMode":"bypassPermissions","isSidechain":false,"sessionId":"s1","cwd":"{cwd}"}}
{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"test"}}]}},"timestamp":"2026-05-22T10:00:00.000Z"}}
"#
        ),
    )
    .unwrap();

    let mcp = make_mcp_with_debug_in_home(canonical_home.clone()).await;

    let connect = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            project_path: canonical_project.display().to_string(),
            provider: "claude".into(),
            agent_name: None,
            transcript_path: Some(transcript.display().to_string()),
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "success", "connect: {connect}");

    let started = mcp
        .start_debug_session_for_tests(StartDebugSessionParams {
            project: Some("test".into()),
            description: Some("checkpoint test".into()),
            agent_id: ":host/claude+test>".into(),
            feature: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&started).unwrap();
    assert_eq!(parsed["status"], "success", "start: {started}");

    let snap = mcp
        .build_context_snapshot_for_tests(BuildContextSnapshotParams {
            since: Some("checkpoint".into()),
            facets: None,
            providers: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&snap).unwrap();
    assert_eq!(parsed["code"], "no_active_checkpoint");
}

#[tokio::test]
async fn link_conversation_requires_connection() {
    let mcp = make_mcp().await;
    let result = mcp
        .link_conversation_for_tests(LinkConversationParams {
            provider: "claude".into(),
            project_path: Some("/tmp/test".into()),
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["status"], "connect_required");
    assert_eq!(parsed["next_actions"][0]["tool"], "daemon8_connect");
}

#[tokio::test]
async fn link_conversation_missing_params() {
    let mcp = make_mcp().await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());
    write_empty_project_config(tmp.path());

    let connect = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "claude".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "success", "connect: {connect}");

    let result = mcp
        .link_conversation_for_tests(LinkConversationParams {
            provider: "codex".into(),
            project_path: None,
            transcript_path: None,
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["status"], "error");
    assert_eq!(parsed["code"], "missing_params");
}

#[tokio::test]
async fn link_conversation_happy_path() {
    let home = test_home_dir();
    let project = home.join("link-project");
    std::fs::create_dir_all(&project).unwrap();
    mark_project(&project);
    write_empty_project_config(&project);

    let canonical_project = std::fs::canonicalize(&project).unwrap();
    let slug = canonical_project.to_string_lossy().replace('/', "-");
    let canonical_home = std::fs::canonicalize(&home).unwrap();

    let claude_dir = canonical_home.join(".claude/projects").join(&slug);
    std::fs::create_dir_all(&claude_dir).unwrap();
    let claude_transcript = claude_dir.join("primary.jsonl");
    let cwd = canonical_project.display().to_string();
    std::fs::write(
        &claude_transcript,
        format!(
            r#"{{"type":"permission-mode","permissionMode":"bypassPermissions","isSidechain":false,"sessionId":"s1","cwd":"{cwd}"}}"#
        ),
    )
    .unwrap();

    let codex_dir = canonical_home.join(".codex/sessions");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let codex_transcript = codex_dir.join("session.jsonl");
    std::fs::write(
        &codex_transcript,
        format!(r#"{{"type":"session_meta","payload":{{"id":"s2","cwd":"{cwd}"}}}}"#),
    )
    .unwrap();

    let mcp = make_mcp_with_cancel_and_home(CancellationToken::new(), canonical_home).await;

    let connect = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            project_path: canonical_project.display().to_string(),
            provider: "claude".into(),
            agent_name: None,
            transcript_path: Some(claude_transcript.display().to_string()),
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "success", "connect: {connect}");

    let result = mcp
        .link_conversation_for_tests(LinkConversationParams {
            provider: "codex".into(),
            project_path: None,
            transcript_path: Some(codex_transcript.display().to_string()),
            conversation_lookback_hours: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["status"], "success", "link: {result}");
    assert!(parsed["data"]["path"].as_str().is_some());
    assert_eq!(parsed["data"]["provider"], "codex");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_sessions_read_live_feed_without_wedge() {
    let surreal = Arc::new(SurrealStore::memory().await.unwrap());
    let cursor_store: Arc<dyn daemon8_store::CursorStore> = Arc::new(surreal.cursor_store());
    let store: Arc<dyn StateModel> = surreal;
    let (obs_tx, mut obs_rx) = tokio::sync::mpsc::unbounded_channel();
    let store_for_writer = store.clone();
    tokio::spawn(async move {
        while let Some(obs) = obs_rx.recv().await {
            let _ = store_for_writer.insert(&obs).await;
        }
    });
    let (broadcast_tx, _) = tokio::sync::broadcast::channel::<(Arc<Observation>, Arc<str>)>(16);

    let home = test_home_dir();
    let project = home.join("concurrent-read");
    std::fs::create_dir_all(&project).unwrap();
    mark_project(&project);
    std::fs::write(
        project.join("cargo.log"),
        "line-one\nline-two\nline-three\n",
    )
    .unwrap();
    write_file_source_config(&project, "cargo.check", "cargo.log");
    let project_path = project.display().to_string();

    const SESSION_COUNT: usize = 6;
    let mut sessions = Vec::with_capacity(SESSION_COUNT);
    for _ in 0..SESSION_COUNT {
        let mcp = make_mcp_with_shared_store(
            store.clone(),
            cursor_store.clone(),
            obs_tx.clone(),
            broadcast_tx.clone(),
            home.clone(),
        )
        .await;
        let connected = mcp
            .daemon8_connect_for_tests(Daemon8ConnectParams {
                provider: "codex".into(),
                project_path: project_path.clone(),
                agent_name: None,
                transcript_path: None,
                conversation_lookback_hours: None,
            })
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
        assert_eq!(parsed["status"], "success", "connect: {connected}");
        sessions.push(Arc::new(mcp));
    }

    let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut handles = Vec::new();
        for mcp in &sessions {
            let mcp = Arc::clone(mcp);
            handles.push(tokio::spawn(async move {
                for _ in 0..3 {
                    let feed = mcp
                        .read_live_feed_for_tests_with(ObserveParams {
                            source: Some(vec!["cargo.check".into()]),
                            ..Default::default()
                        })
                        .await;
                    let parsed: serde_json::Value = serde_json::from_str(&feed).unwrap();
                    assert_eq!(parsed["status"], "success", "feed: {feed}");
                }
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "concurrent read_live_feed deadlocked (timed out after 10s)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_read_and_write_live_feed() {
    let surreal = Arc::new(SurrealStore::memory().await.unwrap());
    let cursor_store: Arc<dyn daemon8_store::CursorStore> = Arc::new(surreal.cursor_store());
    let store: Arc<dyn StateModel> = surreal;
    let (obs_tx, mut obs_rx) = tokio::sync::mpsc::unbounded_channel();
    let store_for_writer = store.clone();
    tokio::spawn(async move {
        while let Some(obs) = obs_rx.recv().await {
            let _ = store_for_writer.insert(&obs).await;
        }
    });
    let (broadcast_tx, _) = tokio::sync::broadcast::channel::<(Arc<Observation>, Arc<str>)>(16);

    let home = test_home_dir();
    let project = home.join("concurrent-rw");
    std::fs::create_dir_all(&project).unwrap();
    mark_project(&project);
    write_file_source_config(&project, "cargo.check", "cargo.log");
    std::fs::write(project.join("cargo.log"), "baseline\n").unwrap();
    let project_path = project.display().to_string();

    const SESSION_COUNT: usize = 5;
    let mut sessions = Vec::with_capacity(SESSION_COUNT);
    for _ in 0..SESSION_COUNT {
        let mcp = make_mcp_with_shared_store(
            store.clone(),
            cursor_store.clone(),
            obs_tx.clone(),
            broadcast_tx.clone(),
            home.clone(),
        )
        .await;
        let connected = mcp
            .daemon8_connect_for_tests(Daemon8ConnectParams {
                provider: "codex".into(),
                project_path: project_path.clone(),
                agent_name: None,
                transcript_path: None,
                conversation_lookback_hours: None,
            })
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
        assert_eq!(parsed["status"], "success", "connect: {connected}");
        sessions.push(Arc::new(mcp));
    }

    let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut handles = Vec::new();

        let writer = Arc::clone(&sessions[0]);
        handles.push(tokio::spawn(async move {
            for i in 0..10 {
                let result = writer
                    .write_to_live_feed_for_tests(IngestParams {
                        kind: Some("log".into()),
                        severity: Some("info".into()),
                        app: Some("writer".into()),
                        data: serde_json::json!({"message": format!("write-{i}")}),
                        channel: None,
                        correlation_id: None,
                        parent_id: None,
                        tags: None,
                        session_id: None,
                        node_id: None,
                        service: None,
                        source: None,
                        source_instance: None,
                    })
                    .await;
                let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
                assert_eq!(parsed["status"], "success", "write: {result}");
            }
        }));

        for mcp in sessions.iter().skip(1) {
            let mcp = Arc::clone(mcp);
            handles.push(tokio::spawn(async move {
                for _ in 0..5 {
                    let feed = mcp
                        .read_live_feed_for_tests_with(ObserveParams::default())
                        .await;
                    let parsed: serde_json::Value = serde_json::from_str(&feed).unwrap();
                    assert_eq!(parsed["status"], "success", "feed: {feed}");
                }
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "concurrent read+write deadlocked (timed out after 10s)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_sessions_with_growing_file() {
    let surreal = Arc::new(SurrealStore::memory().await.unwrap());
    let cursor_store: Arc<dyn daemon8_store::CursorStore> = Arc::new(surreal.cursor_store());
    let store: Arc<dyn StateModel> = surreal;
    let (obs_tx, mut obs_rx) = tokio::sync::mpsc::unbounded_channel();
    let store_for_writer = store.clone();
    tokio::spawn(async move {
        while let Some(obs) = obs_rx.recv().await {
            let _ = store_for_writer.insert(&obs).await;
        }
    });
    let (broadcast_tx, _) = tokio::sync::broadcast::channel::<(Arc<Observation>, Arc<str>)>(16);

    let home = test_home_dir();
    let project = home.join("concurrent-grow");
    std::fs::create_dir_all(&project).unwrap();
    mark_project(&project);
    write_file_source_config(&project, "cargo.check", "cargo.log");
    let log_path = project.join("cargo.log");
    std::fs::write(&log_path, "initial\n").unwrap();
    let project_path = project.display().to_string();

    const SESSION_COUNT: usize = 4;
    let mut sessions = Vec::with_capacity(SESSION_COUNT);
    for _ in 0..SESSION_COUNT {
        let mcp = make_mcp_with_shared_store(
            store.clone(),
            cursor_store.clone(),
            obs_tx.clone(),
            broadcast_tx.clone(),
            home.clone(),
        )
        .await;
        let connected = mcp
            .daemon8_connect_for_tests(Daemon8ConnectParams {
                provider: "codex".into(),
                project_path: project_path.clone(),
                agent_name: None,
                transcript_path: None,
                conversation_lookback_hours: None,
            })
            .await;
        let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
        assert_eq!(parsed["status"], "success", "connect: {connected}");
        sessions.push(Arc::new(mcp));
    }

    let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut handles = Vec::new();

        let appender_path = log_path.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..5 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                use std::io::Write;
                let mut f = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&appender_path)
                    .unwrap();
                writeln!(f, "appended-{i}").unwrap();
            }
        }));

        for mcp in &sessions {
            let mcp = Arc::clone(mcp);
            handles.push(tokio::spawn(async move {
                for _ in 0..8 {
                    let feed = mcp
                        .read_live_feed_for_tests_with(ObserveParams {
                            source: Some(vec!["cargo.check".into()]),
                            ..Default::default()
                        })
                        .await;
                    let parsed: serde_json::Value = serde_json::from_str(&feed).unwrap();
                    assert_eq!(parsed["status"], "success", "feed: {feed}");
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "concurrent read with growing file deadlocked (timed out after 10s)"
    );
}
