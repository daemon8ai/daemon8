// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::sync::Arc;

use daemon8_chrome::ConnectionState;
use daemon8_mcp::{DaemonMcp, DaemonMcpConfig};
use daemon8_types::Filter;
use tokio_util::sync::CancellationToken;

const EXPECTED_TOOLS: [&str; 15] = [
    "read_live_feed",
    "status",
    "create_checkpoint",
    "list_connections",
    "write_to_live_feed",
    "watch_live_feed",
    "issue_command",
    "connect_browser",
    "set_lens",
    "clear_lens",
    "lens_status",
    "setup_status",
    "setup_plan",
    "setup_apply",
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
    let lens = Arc::new(daemon8_store::LensManager::new(
        broadcast_tx.subscribe(),
        None,
    ));
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
        setup_tool_fn: Some(Arc::new(|action| {
            Box::pin(async move {
                serde_json::to_string(&serde_json::json!({
                    "action": action.action,
                    "ok": true
                }))
                .unwrap()
            })
        })),
        source_activator: None,
        cancel,
    })
}

use daemon8_store::SurrealStore;

#[test]
fn composed_router_has_full_tool_surface() {
    let router = DaemonMcp::tool_router()
        + DaemonMcp::action_tool_router()
        + DaemonMcp::lens_tool_router()
        + DaemonMcp::setup_tool_router();
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

async fn make_mcp_minimal() -> DaemonMcp {
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let (obs_tx, _) = tokio::sync::mpsc::unbounded_channel();
    let (chrome_tx, _) = tokio::sync::mpsc::channel(16);
    let (_, chrome_state_rx) = tokio::sync::watch::channel(ConnectionState::Disconnected);
    let (broadcast_tx, _) = tokio::sync::broadcast::channel(16);
    let lens = Arc::new(daemon8_store::LensManager::new(
        broadcast_tx.subscribe(),
        None,
    ));
    DaemonMcp::new(DaemonMcpConfig {
        store,
        memory_store: None,
        debug_session_store: None,
        obs_tx,
        chrome_tx,
        chrome_state: chrome_state_rx,
        chrome_endpoint: Arc::new(std::sync::Mutex::new(None)),
        device_screenshot_fn: None,
        screenshot_dir: std::env::temp_dir().join("daemon8-test-screenshots"),
        broadcast_tx,
        lens,
        setup_tool_fn: None,
        source_activator: None,
        cancel: CancellationToken::new(),
    })
}

#[tokio::test]
async fn help_index_includes_core_topics() {
    let mcp = make_mcp().await;
    let index = mcp.help_index_body();
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
