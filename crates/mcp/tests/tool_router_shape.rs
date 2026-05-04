// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::sync::Arc;

use daemon8_chrome::ConnectionState;
use daemon8_mcp::{DaemonMcp, DaemonMcpConfig};
use daemon8_store::SurrealStore;

const EXPECTED_TOOLS: [&str; 18] = [
    "query_observations",
    "status",
    "create_checkpoint",
    "list_connections",
    "ingest_observation",
    "subscribe_observations",
    "issue_command",
    "connect_browser",
    "set_lens",
    "clear_lens",
    "lens_status",
    "save_memory",
    "query_memory",
    "forget_memory",
    "setup_status",
    "setup_plan",
    "setup_apply",
    "deliber8_inbox",
];

fn tool_names(router: &rmcp::handler::server::router::tool::ToolRouter<DaemonMcp>) -> Vec<String> {
    router
        .list_all()
        .iter()
        .map(|t| t.name.to_string())
        .collect()
}

async fn make_mcp() -> DaemonMcp {
    let store = Arc::new(SurrealStore::memory().await.unwrap());
    let memory_store = store.memory_store();
    memory_store.init_schema().await.unwrap();
    let (obs_tx, _) = tokio::sync::mpsc::unbounded_channel();
    let (chrome_tx, _) = tokio::sync::mpsc::channel(16);
    let (_, chrome_state_rx) = tokio::sync::watch::channel(ConnectionState::Disconnected);
    let (sub_tx, _) = tokio::sync::watch::channel(None);
    let sub_tx = Arc::new(sub_tx);
    let (broadcast_tx, _) = tokio::sync::broadcast::channel(16);
    let lens = Arc::new(daemon8_store::LensManager::new(broadcast_tx.subscribe()));
    let envelope_store: Arc<dyn daemon8_store::EnvelopeStore> =
        Arc::new(SurrealStore::memory().await.unwrap().envelope_store());
    let card_store: Arc<dyn daemon8_store::CardStore> =
        Arc::new(SurrealStore::memory().await.unwrap().card_store());
    DaemonMcp::new(DaemonMcpConfig {
        store,
        memory_store: Some(Arc::new(memory_store)),
        envelope_store: Some(envelope_store),
        card_store: Some(card_store),
        obs_tx,
        chrome_tx,
        chrome_state: chrome_state_rx,
        chrome_endpoint: Arc::new(std::sync::Mutex::new(None)),
        device_screenshot_fn: None,
        screenshot_dir: std::env::temp_dir().join("daemon8-test-screenshots"),
        subscription_tx: sub_tx,
        broadcast_tx,
        lens,
        embedder: None,
        setup_tool_fn: Some(Arc::new(|action| {
            Box::pin(async move {
                serde_json::to_string(&serde_json::json!({
                    "action": action.action,
                    "ok": true
                }))
                .unwrap()
            })
        })),
    })
}

#[test]
fn composed_router_has_all_eighteen_tools() {
    let router = DaemonMcp::tool_router()
        + DaemonMcp::action_tool_router()
        + DaemonMcp::lens_tool_router()
        + DaemonMcp::memory_tool_router()
        + DaemonMcp::setup_tool_router()
        + DaemonMcp::deliber8_tool_router();
    let names = tool_names(&router);

    assert_eq!(
        names.len(),
        18,
        "router must expose all 18 tools, got {}: {:?}",
        names.len(),
        names
    );

    for expected in EXPECTED_TOOLS.iter() {
        assert!(
            names.iter().any(|n| n == expected),
            "router missing expected tool '{}'. Present: {:?}",
            expected,
            names
        );
    }
}

#[tokio::test]
async fn live_mcp_exposes_all_eighteen_tools() {
    let mcp = make_mcp().await;
    let names: Vec<String> = mcp
        .tools_for_client()
        .iter()
        .map(|t| t.name.to_string())
        .collect();

    assert_eq!(
        names.len(),
        18,
        "tools_for_client() must expose all 18 tools, got {}: {:?}",
        names.len(),
        names
    );
    for expected in EXPECTED_TOOLS.iter() {
        assert!(
            names.iter().any(|n| n == expected),
            "tools_for_client() missing '{}'. Present: {:?}",
            expected,
            names
        );
    }
}

#[tokio::test]
async fn query_observations_description_mentions_full_surface() {
    let mcp = make_mcp().await;
    let tools = mcp.tools_for_client();
    let observe = tools
        .iter()
        .find(|t| t.name == "query_observations")
        .expect("query_observations must be present in tools_for_client()");
    let desc = observe.description.as_deref().unwrap_or("");
    for term in ["browser", "device", "js_exception"] {
        assert!(
            desc.contains(term),
            "query_observations description must contain '{term}'. Got: {:?}",
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
