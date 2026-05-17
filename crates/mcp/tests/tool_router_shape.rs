// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{borrow::Cow, sync::Arc};

use daemon8_chrome::ConnectionState;
use daemon8_ingest::source_sync::{
    ConfiguredSourceTrigger, ObservationWriteResult, ObservationWriteStatus, ObservationWriter,
};
use daemon8_mcp::{
    ActParams, CreateCheckpointParams, Daemon8ConnectParams, Daemon8InitParams, DaemonMcp,
    DaemonMcpConfig, DebugAction, IngestParams, ObserveParams, StartDebugSessionParams,
    TOOL_POLICY_TABLE, ToolPolicy, tool_policy,
};
use daemon8_types::{Filter, Observation};
use rmcp::ServiceExt as _;
use tokio_util::sync::CancellationToken;

const EXPECTED_TOOLS: [&str; 13] = [
    "read_live_feed",
    "daemon8_connect",
    "daemon8_init",
    "daemon8_status",
    "list_connections",
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
        source_trigger: None,
        lens,
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
    let source_trigger = ConfiguredSourceTrigger::new(
        Arc::new(store.cursor_store()),
        Arc::new(DirectStoreWriter {
            store: store.clone(),
        }),
    );
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
        source_trigger: Some(Arc::new(source_trigger)),
        lens,
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
        source_trigger: None,
        lens,
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
    let source_trigger = ConfiguredSourceTrigger::new(
        Arc::new(store.cursor_store()),
        Arc::new(DirectStoreWriter {
            store: store.clone(),
        }),
    );
    let (obs_tx, mut obs_rx) = tokio::sync::mpsc::unbounded_channel();
    let store_for_writer = store.clone();
    tokio::spawn(async move {
        while let Some(obs) = obs_rx.recv().await {
            let _ = store_for_writer.insert(obs).await;
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
        source_trigger: Some(Arc::new(source_trigger)),
        lens,
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

use daemon8_store::{StateModel, SurrealStore};

struct DirectStoreWriter {
    store: Arc<dyn StateModel>,
}

#[async_trait::async_trait]
impl ObservationWriter for DirectStoreWriter {
    async fn write_observation(&self, obs: Observation) -> Result<ObservationWriteResult, String> {
        let id = self
            .store
            .insert(obs)
            .await
            .map_err(|err| err.to_string())?;
        Ok(ObservationWriteResult {
            status: ObservationWriteStatus::Inserted,
            id: Some(id),
        })
    }
}

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
    for tool in ["daemon8_connect", "daemon8_init", "daemon8_status"] {
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
        "daemon8_help",
        "list_debug_sessions",
    ] {
        assert_eq!(tool_policy(tool), Some(ToolPolicy::GeneralSafe), "{tool}");
    }

    for tool in [
        "start_debug_session",
        "create_checkpoint",
        "resolve_debug_session",
        "end_debug_session",
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
async fn memory_tools_are_not_public_in_afl_02d() {
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
            "{removed} must remain absent from the AFL-02d public MCP surface"
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
                "agent_id": ":host/codex+worker>",
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
                "agent_id": ":host/codex+worker>",
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
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["status"], "setup_required");
    assert_eq!(parsed["code"], "missing_config");
    assert_eq!(parsed["next_actions"][0]["tool"], "daemon8_init");

    let body = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
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
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["mode"], "project");

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
async fn daemon8_connect_triggers_configured_file_source_ingestion() {
    let mcp = make_mcp_with_writer().await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());
    std::fs::write(tmp.path().join("cargo.log"), "cargo check one\n").unwrap();
    write_file_source_config(tmp.path(), "cargo.check", "cargo.log");

    let config_before = std::fs::read_to_string(tmp.path().join(".daemon8/config.md")).unwrap();
    let connect = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(
        parsed["data"]["triggered_ingestion"]["observations_written"],
        1
    );

    let filtered = mcp
        .read_live_feed_for_tests_with(ObserveParams {
            service: Some(vec!["cargo".into()]),
            source: Some(vec!["cargo.check".into()]),
            ..Default::default()
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&filtered).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["observations"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["data"]["observations"][0]["service"], "cargo");
    assert_eq!(parsed["data"]["observations"][0]["source"], "cargo.check");
    assert_eq!(
        parsed["data"]["observations"][0]["source_instance"],
        std::fs::canonicalize(tmp.path().join("cargo.log"))
            .unwrap()
            .display()
            .to_string()
    );
    let config_after = std::fs::read_to_string(tmp.path().join(".daemon8/config.md")).unwrap();
    assert_eq!(config_after, config_before);
}

#[tokio::test]
async fn daemon8_connect_triggers_configured_conversation_source_ingestion() {
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
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(
        parsed["data"]["triggered_ingestion"]["observations_written"],
        1
    );

    let filtered = mcp
        .read_live_feed_for_tests_with(ObserveParams {
            source: Some(vec!["codex.sessions".into()]),
            ..Default::default()
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&filtered).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["observations"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["data"]["observations"][0]["service"], "codex");
    assert_eq!(
        parsed["data"]["observations"][0]["source"],
        "codex.sessions"
    );
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
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connect).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["provider"], "codex");
    assert_eq!(parsed["data"]["transcript"]["status"], "bound");
    assert_eq!(
        parsed["data"]["triggered_ingestion"]["observations_written"],
        1
    );

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

    let filtered = mcp
        .read_live_feed_for_tests_with(ObserveParams {
            source: Some(vec!["runtime.transcript.codex".into()]),
            ..Default::default()
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&filtered).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["observations"].as_array().unwrap().len(), 1);
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
async fn daemon8_connect_invalid_provider_clears_previous_connection() {
    let mcp = make_mcp().await;
    let general = tempfile::tempdir().unwrap();

    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: general.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
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
    assert_eq!(parsed["next_actions"][0]["tool"], "daemon8_connect");

    for tool in ["daemon8_connect", "daemon8_init", "daemon8_status"] {
        assert!(
            mcp.connect_preflight_for_tests(tool).is_none(),
            "{tool} should be a connect-first exception"
        );
    }

    assert!(
        mcp.connect_preflight_for_tests("daemon8_help").is_some(),
        "daemon8_help should require daemon8_connect"
    );
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
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["data"]["mode"], "general");

    let blocked = mcp.read_live_feed_for_tests().await;
    let parsed: serde_json::Value = serde_json::from_str(&blocked).unwrap();
    assert_eq!(parsed["status"], "blocked");
    assert_eq!(parsed["code"], "narrow_filter_required");

    let narrowed = mcp
        .read_live_feed_for_tests_with(ObserveParams {
            severity_min: Some("warn".into()),
            ..Default::default()
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&narrowed).unwrap();
    assert_eq!(parsed["status"], "success");

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
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["status"], "success");

    mcp.write_to_live_feed_for_tests(IngestParams {
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
async fn read_live_feed_triggers_configured_file_source_append() {
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
    assert_eq!(
        parsed["data"]["triggered_ingestion"]["observations_written"],
        1
    );
    assert_eq!(parsed["data"]["observations"].as_array().unwrap().len(), 2);
    assert!(
        parsed["data"]["observations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|obs| obs["data"]["message"] == "second")
    );
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
async fn create_checkpoint_refreshes_project_sources_before_sequence_capture() {
    let mcp = make_mcp_with_debug().await;
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
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["status"], "success");

    let started = mcp
        .start_debug_session_for_tests(StartDebugSessionParams {
            project: Some("daemon8".into()),
            description: Some("source checkpoint".into()),
            agent_id: ":host/codex+worker>".into(),
            feature: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&started).unwrap();
    assert_eq!(parsed["status"], "success");

    std::fs::write(&log, "first\nsecond\n").unwrap();
    let checkpoint = mcp
        .create_checkpoint_for_tests(CreateCheckpointParams {
            description: Some("after source append".into()),
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&checkpoint).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(
        parsed["data"]["triggered_ingestion"]["observations_written"],
        1
    );
    let seq_at_creation = parsed["data"]["seq_at_creation"].as_u64().unwrap();

    let all = mcp
        .read_live_feed_for_tests_with(ObserveParams {
            source: Some(vec!["cargo.check".into()]),
            ..Default::default()
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&all).unwrap();
    let observations = parsed["data"]["observations"].as_array().unwrap();
    assert_eq!(observations.len(), 2);
    assert!(
        observations
            .iter()
            .all(|obs| obs["id"].as_u64().unwrap() <= seq_at_creation)
    );
}

#[tokio::test]
async fn create_checkpoint_blocks_when_source_refresh_fails() {
    let mcp = make_mcp_with_debug().await;
    let tmp = tempfile::tempdir().unwrap();
    mark_project(tmp.path());
    write_file_source_config(tmp.path(), "cargo.check", "missing.log");

    let connected = mcp
        .daemon8_connect_for_tests(Daemon8ConnectParams {
            provider: "codex".into(),
            project_path: tmp.path().display().to_string(),
            agent_name: None,
            transcript_path: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&connected).unwrap();
    assert_eq!(parsed["status"], "success");

    let started = mcp
        .start_debug_session_for_tests(StartDebugSessionParams {
            project: Some("daemon8".into()),
            description: Some("source checkpoint failure".into()),
            agent_id: ":host/codex+worker>".into(),
            feature: None,
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&started).unwrap();
    assert_eq!(parsed["status"], "success");

    let checkpoint = mcp
        .create_checkpoint_for_tests(CreateCheckpointParams {
            description: Some("blocked by missing source".into()),
        })
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&checkpoint).unwrap();
    assert_eq!(parsed["status"], "blocked");
    assert_eq!(parsed["code"], "checkpoint_source_refresh_failed");
    assert_eq!(
        parsed["data"]["triggered_ingestion"]["failures"][0]["code"],
        "read_failed"
    );
    assert!(parsed["data"].get("checkpoint_id").is_none());
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
                "agent_id": ":host/codex+worker>",
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
                "agent_id": ":host/codex+worker>",
                "feature": null,
            }),
        ))
        .await?;
    let parsed = result_json(&blocked);
    assert_eq!(parsed["status"], "blocked");
    assert_eq!(parsed["code"], "project_required");

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
                "agent_id": ":host/codex+worker>",
                "feature": "mcp",
            }),
        ))
        .await?;
    let parsed = result_json(&started);
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["code"], "debug_session_started");
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
                "agent_id": ":host/codex+worker>",
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
async fn project_mode_allows_project_only_debug_tools() {
    let mcp = make_mcp_with_debug().await;
    let project = tempfile::tempdir().unwrap();
    mark_project(project.path());

    let init = mcp
        .daemon8_init_for_tests(Daemon8InitParams {
            project_path: project.path().display().to_string(),
            name: None,
            overwrite: None,
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
