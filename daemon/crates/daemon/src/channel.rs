// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, tool_handler, tool_router};

// ---------------------------------------------------------------------------
// ChannelMcp -- minimal MCP handler, no tools, only claude/channel capability
// ---------------------------------------------------------------------------

struct ChannelMcp {
    tool_router: ToolRouter<Self>,
}

impl ChannelMcp {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl ChannelMcp {}

#[tool_handler]
impl ServerHandler for ChannelMcp {
    fn get_info(&self) -> ServerInfo {
        let mut capabilities = ServerCapabilities::builder().build();
        capabilities.experimental = Some(BTreeMap::from([(
            "claude/channel".to_string(),
            serde_json::Map::new(),
        )]));

        ServerInfo::new(capabilities)
            .with_server_info(Implementation::new("daemon8-channel", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Real-time observation relay from Daemon8. \
                 High-severity observations (warn/error) are pushed into your session automatically. \
                 Use the daemon8 MCP server for tools (debug_observe, debug_act, etc.)."
            )
    }
}

// ---------------------------------------------------------------------------
// cmd_channel -- entry point for `daemon8 channel`
// ---------------------------------------------------------------------------

pub async fn cmd_channel() -> Result<()> {
    let cfg = crate::config::load(None).unwrap_or_default();
    let port = cfg.server.port;
    let stream_url = format!("http://localhost:{port}/api/stream");

    // Probe: is the daemon running?
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;

    let mut connected = false;
    for attempt in 1..=3 {
        if client.get(&stream_url).send().await.is_ok() {
            connected = true;
            break;
        }
        if attempt < 3 {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    if !connected {
        anyhow::bail!("daemon8 is not running — start the daemon with 'daemon8 serve' first");
    }

    // Start MCP stdio server
    use rmcp::ServiceExt;
    let service = ChannelMcp::new()
        .serve(rmcp::transport::stdio())
        .await
        .context("channel MCP stdio transport failed")?;

    let peer: rmcp::service::Peer<rmcp::RoleServer> = (*service).clone();

    let relay = tokio::spawn(sse_relay(peer, port));

    let _ = service.waiting().await;

    relay.abort();
    Ok(())
}

// ---------------------------------------------------------------------------
// sse_relay -- subscribe to daemon SSE, push warn/error as channel notifications
// ---------------------------------------------------------------------------

async fn sse_relay(peer: rmcp::service::Peer<rmcp::RoleServer>, port: u16) {
    use reqwest_eventsource::{Event, EventSource};
    use tokio_stream::StreamExt;

    let mut last_push = Instant::now() - Duration::from_secs(2);
    // Server-side filter replaces the old client-side `severity in {warn,error}` check.
    let mut es = EventSource::get(format!(
        "http://localhost:{port}/api/stream?severity_min=warn"
    ));

    while let Some(event) = es.next().await {
        let json_str = match event {
            Ok(Event::Message(msg)) => msg.data,
            Ok(Event::Open) => continue,
            Err(_) => continue,
        };

        let obs: serde_json::Value = match serde_json::from_str(&json_str) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let severity = obs["severity"].as_str().unwrap_or("info");

        if last_push.elapsed() < Duration::from_secs(1) {
            continue;
        }

        let kind = obs["kind"]["type"].as_str().unwrap_or("log");
        let origin = if let Some(name) = obs["origin"]["name"].as_str() {
            format!("app:{name}")
        } else if let Some(tab) = obs["origin"]["tab_id"].as_str() {
            format!("browser:{tab}")
        } else {
            "unknown".to_string()
        };

        let msg = obs["kind"]["message"]
            .as_str()
            .or_else(|| obs["data"]["message"].as_str())
            .or_else(|| obs["data"]["msg"].as_str())
            .unwrap_or("(no message)");
        let content = format!("[{severity}] {kind} from {origin}: {msg}");

        let mut meta = serde_json::Map::new();
        meta.insert(
            "severity".into(),
            serde_json::Value::String(severity.into()),
        );
        meta.insert("kind".into(), serde_json::Value::String(kind.into()));
        meta.insert("origin".into(), serde_json::Value::String(origin));

        let notification = rmcp::model::ServerNotification::CustomNotification(
            rmcp::model::CustomNotification::new(
                "notifications/claude/channel",
                Some(serde_json::json!({
                    "content": content,
                    "meta": meta,
                })),
            ),
        );

        let send =
            tokio::time::timeout(Duration::from_secs(5), peer.send_notification(notification))
                .await;

        match send {
            Ok(Ok(())) => {
                last_push = Instant::now();
            }
            Ok(Err(_)) => break,
            Err(_) => {}
        }
    }
}
