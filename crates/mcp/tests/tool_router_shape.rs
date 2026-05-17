// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::sync::Arc;

use daemon8_chrome::ConnectionState;
use daemon8_mcp::{
    ActParams, Daemon8ConnectParams, Daemon8InitParams, DaemonMcp, DaemonMcpConfig, DebugAction,
    ObserveParams,
};
use daemon8_types::Filter;
use rmcp::ServiceExt as _;
use tokio_util::sync::CancellationToken;

const EXPECTED_TOOLS: [&str; 14] = [
    "read_live_feed",
    "daemon8_connect",
    "daemon8_init",
    "daemon8_status",
    "create_checkpoint",
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
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let memory_store = store.memory_store();
    memory_store.init_schema().await.unwrap();
    let (obs_tx, _) = tokio::sync::mpsc::unbounded_channel();
    let (chrome_tx, _) = tokio::sync::mpsc::channel(16);
    let (_, chrome_state_rx) = tokio::sync::watch::channel(ConnectionState::Disconnected);
    let (broadcast_tx, _) = tokio::sync::broadcast::channel(16);
    let lens = Arc::new(daemon8_store::LensManager::new(broadcast_tx.subscribe()));
    DaemonMcp::new(DaemonMcpConfig {
        store,
        memory_store: Some(Arc::new(memory_store)),
        debug_session_store: None,
        obs_tx,
        chrome_tx,
        chrome_state: chrome_state_rx,
        chrome_endpoint: Arc::new(std::sync::Mutex::new(None)),
        device_screenshot_fn: None,
        screenshot_dir: std::env::temp_dir().join("daemon8-test-screenshots"),
        broadcast_tx,
        lens,
        cancel,
    })
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
}

#[tokio::test]
async fn runtime_tools_require_connect_first() {
    let mcp = make_mcp().await;

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
}

#[tokio::test]
async fn general_mode_blocks_project_only_tools() {
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

    let blocked = mcp
        .connect_preflight_for_tests("create_checkpoint")
        .expect("create_checkpoint should require project scope in general mode");
    let parsed: serde_json::Value = serde_json::from_str(&blocked).unwrap();
    assert_eq!(parsed["status"], "blocked");
    assert_eq!(parsed["code"], "project_required");
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
