// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::error::{ChromeError, Result};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub(crate) struct CdpEvent {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct TargetInfo {
    pub id: String,
    pub target_type: String,
    pub url: String,
}

pub(crate) async fn discover_ws_url(endpoint: &str) -> Result<String> {
    let url = format!("{}/json/version", endpoint.trim_end_matches('/'));
    let resp: Value = reqwest::get(&url).await?.json().await?;

    resp["webSocketDebuggerUrl"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| ChromeError::Cdp("Chrome did not provide a WebSocket debugger URL".into()))
}

pub(crate) async fn list_targets(endpoint: &str) -> Result<Vec<TargetInfo>> {
    let url = format!("{}/json/list", endpoint.trim_end_matches('/'));
    let resp: Vec<Value> = reqwest::get(&url).await?.json().await?;

    Ok(resp
        .into_iter()
        .filter_map(|v| {
            Some(TargetInfo {
                id: v["id"].as_str()?.to_string(),
                target_type: v["type"].as_str().unwrap_or("unknown").to_string(),
                url: v["url"].as_str().unwrap_or("").to_string(),
            })
        })
        .collect())
}

type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;
type WsSink = Arc<
    Mutex<
        futures::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            Message,
        >,
    >,
>;

pub(crate) struct CdpClient {
    sink: WsSink,
    next_id: Arc<AtomicU64>,
    pending: PendingMap,
    // The read half is consumed by `run` -- Option so we can take it once.
    stream: Mutex<
        Option<
            futures::stream::SplitStream<
                tokio_tungstenite::WebSocketStream<
                    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
                >,
            >,
        >,
    >,
}

impl CdpClient {
    pub async fn connect(ws_url: &str) -> Result<Self> {
        let (ws, _resp) = tokio_tungstenite::connect_async(ws_url).await?;

        let (sink, stream) = ws.split();

        Ok(Self {
            sink: Arc::new(Mutex::new(sink)),
            next_id: Arc::new(AtomicU64::new(1)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            stream: Mutex::new(Some(stream)),
        })
    }

    /// Send a CDP command and await the response.
    ///
    /// Returns the `result` field from the CDP response, or an error if:
    /// - The WebSocket send fails
    /// - The response carries a CDP `error` field
    /// - The command times out (10s default)
    pub async fn send_command(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let mut msg = serde_json::json!({
            "id": id,
            "method": method,
            "params": params,
        });
        if let Some(sid) = session_id {
            msg["sessionId"] = Value::String(sid.to_string());
        }

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let send_result = {
            let mut sink = self.sink.lock().await;
            sink.send(Message::Text(msg.to_string().into())).await
        };

        if let Err(e) = send_result {
            self.pending.lock().await.remove(&id);
            return Err(ChromeError::WebSocket(format!(
                "failed to send CDP command '{method}': {e}"
            )));
        }

        match tokio::time::timeout(COMMAND_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(ChromeError::Cdp(format!(
                "CDP command '{method}' cancelled (connection closed)"
            ))),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(ChromeError::Cdp(format!(
                    "CDP command '{method}' timed out after {}s",
                    COMMAND_TIMEOUT.as_secs()
                )))
            }
        }
    }

    /// Start the event pump. Reads WebSocket messages and:
    /// - Routes responses (messages with `id`) to pending command waiters
    /// - Routes events (messages with `method`, no `id`) to `event_tx`
    ///
    /// Runs until the WebSocket closes, cancellation fires, or an
    /// unrecoverable error occurs.
    pub async fn run(&self, event_tx: mpsc::Sender<CdpEvent>, cancel: CancellationToken) {
        let mut stream = {
            let mut guard = self.stream.lock().await;
            match guard.take() {
                Some(s) => s,
                None => {
                    tracing::error!("CDP event pump started twice -- read stream already consumed");
                    return;
                }
            }
        };

        loop {
            tokio::select! {
                msg = stream.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            self.handle_message(&text, &event_tx);
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            tracing::debug!("Chrome WebSocket closed");
                            break;
                        }
                        Some(Ok(_)) => {} // ping/pong/binary -- ignore
                        Some(Err(e)) => {
                            tracing::debug!("WebSocket read error: {e}");
                            break;
                        }
                    }
                }
                () = cancel.cancelled() => {
                    tracing::debug!("CDP event pump cancelled");
                    break;
                }
            }
        }

        // Drain pending commands so no caller hangs forever.
        self.drain_pending().await;
    }

    fn handle_message(&self, text: &str, event_tx: &mpsc::Sender<CdpEvent>) {
        let msg: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => {
                tracing::trace!("skipping non-JSON WebSocket message");
                return;
            }
        };

        // Response to a command we sent (has `id` field).
        if let Some(id) = msg["id"].as_u64() {
            let result = if let Some(err) = msg.get("error") {
                Err(ChromeError::Cdp(format!(
                    "CDP error {}: {}",
                    err["code"].as_i64().unwrap_or(-1),
                    err["message"].as_str().unwrap_or("unknown")
                )))
            } else {
                Ok(msg.get("result").cloned().unwrap_or(Value::Null))
            };

            // Try to resolve the pending command. If nobody is waiting
            // (timed out or cancelled), the result is silently dropped.
            let pending = self.pending.clone();
            tokio::spawn(async move {
                if let Some(tx) = pending.lock().await.remove(&id) {
                    let _ = tx.send(result);
                }
            });
            return;
        }

        // Event from Chrome (has `method` field, no `id`).
        if let Some(method) = msg["method"].as_str() {
            let event = CdpEvent {
                method: method.to_string(),
                params: msg.get("params").cloned().unwrap_or(Value::Null),
                session_id: msg["sessionId"].as_str().map(String::from),
            };
            match event_tx.try_send(event) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!("CDP event channel full, dropping event {method}");
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {}
            }
        }

        // Messages with neither `id` nor `method` are silently ignored.
        // Chrome occasionally sends protocol-level messages we don't need.
    }

    async fn drain_pending(&self) {
        let mut pending = self.pending.lock().await;
        let count = pending.len();
        for (_id, tx) in pending.drain() {
            let _ = tx.send(Err(ChromeError::Disconnected));
        }
        if count > 0 {
            tracing::debug!(count, "drained pending CDP commands on disconnect");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc as tokio_mpsc;

    // Helper: spin up a local WebSocket server that echoes/responds to CDP
    // messages. Returns the WS URL and a channel to control server behavior.
    async fn mock_cdp_server() -> (
        String,
        tokio_mpsc::UnboundedSender<String>, // inject messages from "Chrome"
        tokio_mpsc::UnboundedReceiver<String>, // messages sent by client
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_url = format!("ws://127.0.0.1:{}", addr.port());

        let (inject_tx, mut inject_rx) = tokio_mpsc::unbounded_channel::<String>();
        let (capture_tx, capture_rx) = tokio_mpsc::unbounded_channel::<String>();

        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            let (mut sink, mut stream) = ws.split();

            loop {
                tokio::select! {
                    // Forward injected messages to the client
                    msg = inject_rx.recv() => {
                        match msg {
                            Some(text) => {
                                let _ = sink.send(Message::Text(text.into())).await;
                            }
                            None => break,
                        }
                    }
                    // Capture messages from the client
                    msg = stream.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                let _ = capture_tx.send(text.to_string());
                            }
                            Some(Ok(Message::Close(_))) | None => break,
                            _ => {}
                        }
                    }
                }
            }
        });

        (ws_url, inject_tx, capture_rx)
    }

    #[tokio::test]
    async fn send_command_produces_correct_json_rpc() {
        let (ws_url, inject, mut capture) = mock_cdp_server().await;
        let client = CdpClient::connect(&ws_url).await.unwrap();

        let (event_tx, _event_rx) = mpsc::channel(4096);
        let cancel = CancellationToken::new();

        // Start the pump so responses get routed
        let pump_cancel = cancel.clone();
        let pump_client = Arc::new(client);
        let cmd_client = pump_client.clone();
        tokio::spawn({
            let c = pump_client.clone();
            async move {
                c.run(event_tx, pump_cancel).await;
            }
        });

        // send_command in a spawned task; inject the response from "Chrome"
        let cmd = tokio::spawn(async move {
            cmd_client
                .send_command("Runtime.enable", serde_json::json!({}), Some("session-1"))
                .await
        });

        // Wait for the command to arrive at the mock server
        let sent = tokio::time::timeout(Duration::from_secs(2), capture.recv())
            .await
            .unwrap()
            .unwrap();
        let parsed: Value = serde_json::from_str(&sent).unwrap();

        assert_eq!(parsed["method"], "Runtime.enable");
        assert_eq!(parsed["sessionId"], "session-1");
        assert!(parsed["params"].is_object());
        let id = parsed["id"].as_u64().unwrap();

        // Respond from "Chrome"
        inject
            .send(serde_json::json!({"id": id, "result": {}}).to_string())
            .unwrap();

        let result = tokio::time::timeout(Duration::from_secs(2), cmd)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(result.is_object());

        cancel.cancel();
    }

    #[tokio::test]
    async fn command_response_correlation() {
        let (ws_url, inject, mut capture) = mock_cdp_server().await;
        let client = Arc::new(CdpClient::connect(&ws_url).await.unwrap());

        let (event_tx, _event_rx) = mpsc::channel(4096);
        let cancel = CancellationToken::new();

        let pump_cancel = cancel.clone();
        tokio::spawn({
            let c = client.clone();
            async move {
                c.run(event_tx, pump_cancel).await;
            }
        });

        // Send two commands
        let c1 = client.clone();
        let cmd1 =
            tokio::spawn(async move { c1.send_command("cmd1", serde_json::json!({}), None).await });

        let c2 = client.clone();
        let cmd2 =
            tokio::spawn(async move { c2.send_command("cmd2", serde_json::json!({}), None).await });

        // Capture both commands from the wire
        let sent1 = tokio::time::timeout(Duration::from_secs(2), capture.recv())
            .await
            .unwrap()
            .unwrap();
        let sent2 = tokio::time::timeout(Duration::from_secs(2), capture.recv())
            .await
            .unwrap()
            .unwrap();

        let p1: Value = serde_json::from_str(&sent1).unwrap();
        let p2: Value = serde_json::from_str(&sent2).unwrap();

        let id1 = p1["id"].as_u64().unwrap();
        let id2 = p2["id"].as_u64().unwrap();

        // Respond out of order: cmd2 first, then cmd1
        inject
            .send(serde_json::json!({"id": id2, "result": {"val": "two"}}).to_string())
            .unwrap();
        inject
            .send(serde_json::json!({"id": id1, "result": {"val": "one"}}).to_string())
            .unwrap();

        let r1 = tokio::time::timeout(Duration::from_secs(2), cmd1)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let r2 = tokio::time::timeout(Duration::from_secs(2), cmd2)
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        // Correlation: each response matches its command regardless of order
        let method1 = p1["method"].as_str().unwrap();
        let _method2 = p2["method"].as_str().unwrap();
        if method1 == "cmd1" {
            assert_eq!(r1["val"], "one");
            assert_eq!(r2["val"], "two");
        } else {
            assert_eq!(r1["val"], "two");
            assert_eq!(r2["val"], "one");
        }

        cancel.cancel();
    }

    #[tokio::test]
    async fn events_route_to_channel() {
        let (ws_url, inject, _capture) = mock_cdp_server().await;
        let client = Arc::new(CdpClient::connect(&ws_url).await.unwrap());

        let (event_tx, mut event_rx) = mpsc::channel(4096);
        let cancel = CancellationToken::new();

        let pump_cancel = cancel.clone();
        tokio::spawn({
            let c = client.clone();
            async move {
                c.run(event_tx, pump_cancel).await;
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Inject a CDP event (has method, no id)
        inject
            .send(
                serde_json::json!({
                    "method": "Runtime.consoleAPICalled",
                    "params": {"type": "log", "args": []},
                    "sessionId": "abc123"
                })
                .to_string(),
            )
            .unwrap();

        let event = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(event.method, "Runtime.consoleAPICalled");
        assert_eq!(event.session_id.as_deref(), Some("abc123"));
        assert_eq!(event.params["type"], "log");

        cancel.cancel();
    }

    #[tokio::test]
    async fn event_without_session_id() {
        let (ws_url, inject, _capture) = mock_cdp_server().await;
        let client = Arc::new(CdpClient::connect(&ws_url).await.unwrap());

        let (event_tx, mut event_rx) = mpsc::channel(4096);
        let cancel = CancellationToken::new();

        let pump_cancel = cancel.clone();
        tokio::spawn({
            let c = client.clone();
            async move {
                c.run(event_tx, pump_cancel).await;
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        inject
            .send(
                serde_json::json!({
                    "method": "Target.targetCreated",
                    "params": {"targetInfo": {"targetId": "t1", "type": "page"}}
                })
                .to_string(),
            )
            .unwrap();

        let event = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(event.method, "Target.targetCreated");
        assert!(event.session_id.is_none());

        cancel.cancel();
    }

    #[tokio::test]
    async fn cdp_error_response_becomes_err() {
        let (ws_url, inject, mut capture) = mock_cdp_server().await;
        let client = CdpClient::connect(&ws_url).await.unwrap();

        let (event_tx, _event_rx) = mpsc::channel(4096);
        let cancel = CancellationToken::new();

        let client = Arc::new(client);
        let pump_cancel = cancel.clone();
        tokio::spawn({
            let c = client.clone();
            async move {
                c.run(event_tx, pump_cancel).await;
            }
        });

        let c = client.clone();
        let cmd = tokio::spawn(async move {
            c.send_command("BadMethod", serde_json::json!({}), None)
                .await
        });

        let sent = tokio::time::timeout(Duration::from_secs(2), capture.recv())
            .await
            .unwrap()
            .unwrap();
        let parsed: Value = serde_json::from_str(&sent).unwrap();
        let id = parsed["id"].as_u64().unwrap();

        inject
            .send(
                serde_json::json!({
                    "id": id,
                    "error": {"code": -32601, "message": "method not found"}
                })
                .to_string(),
            )
            .unwrap();

        let result = tokio::time::timeout(Duration::from_secs(2), cmd)
            .await
            .unwrap()
            .unwrap();

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("-32601"));
        assert!(err_msg.contains("method not found"));

        cancel.cancel();
    }

    #[tokio::test]
    async fn pump_drains_pending_on_close() {
        let (ws_url, inject, _capture) = mock_cdp_server().await;
        let client = Arc::new(CdpClient::connect(&ws_url).await.unwrap());

        let (event_tx, _event_rx) = mpsc::channel(4096);
        let cancel = CancellationToken::new();

        // Register a pending command that will never get a response
        let (tx, rx) = oneshot::channel();
        client.pending.lock().await.insert(999, tx);

        // Start the pump, then drop the inject channel to close the WS
        let pump_cancel = cancel.clone();
        let pump_handle = tokio::spawn({
            let c = client.clone();
            async move {
                c.run(event_tx, pump_cancel).await;
            }
        });

        // Close the server side by dropping inject_tx
        drop(inject);

        // The pump should exit and drain pending
        tokio::time::timeout(Duration::from_secs(2), pump_handle)
            .await
            .unwrap()
            .unwrap();

        // The pending command should resolve with an error
        let result = rx.await.unwrap();
        assert!(result.is_err());
        assert!(matches!(result, Err(ChromeError::Disconnected)));
    }
}
