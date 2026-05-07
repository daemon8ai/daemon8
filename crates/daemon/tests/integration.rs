// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

// Integration tests that exercise cross-feature interactions.
// Each test spins up the actual Axum server stack (ingest + API) backed by
// a real SQLite store, then exercises the pipeline end-to-end.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};

use daemon8_chrome::BrowserAction;
use daemon8_mcp::ChromeCommand;
use daemon8_store::{Memory, MemoryStore, StateModel, SurrealStore};
use daemon8_types::{MemoryKind, Observation};

type StreamFrame = (Arc<Observation>, Arc<str>);

struct ChildGuard(std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn toml_string(value: &std::path::Path) -> String {
    value
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn remove_daemon8_env(command: &mut std::process::Command) {
    for (key, _) in std::env::vars() {
        if key.starts_with("DAEMON8_") {
            command.env_remove(key);
        }
    }
}

fn daemon8_command() -> std::process::Command {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_daemon8"));
    remove_daemon8_env(&mut command);
    command
}

fn write_logging_config(
    config_path: &std::path::Path,
    store_path: &std::path::Path,
    screenshot_dir: &std::path::Path,
    log_dir: &std::path::Path,
    max_log_files: usize,
) {
    std::fs::write(
        config_path,
        format!(
            r#"[storage]
path = "{}"
screenshot_path = "{}"

[adb]
enabled = false

[logging]
file = "{}"
level = "debug"
stderr = false
max_log_files = {}
"#,
            toml_string(store_path),
            toml_string(screenshot_dir),
            toml_string(log_dir),
            max_log_files,
        ),
    )
    .unwrap();
}

async fn wait_for_health(base: &str) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);

    loop {
        if let Ok(resp) = client.get(format!("{base}/health")).send().await
            && resp.status().is_success()
            && resp.text().await.unwrap_or_default() == "ok"
        {
            return;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "daemon8 serve did not become healthy at {base}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn daemon_log_paths(log_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Ok(entries) = std::fs::read_dir(log_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_daemon_log = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("daemon8.") && name.ends_with(".log"));
            if is_daemon_log {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths
}

async fn wait_for_log_contains(log_dir: &std::path::Path, needle: &str) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    loop {
        let mut body = String::new();
        for path in daemon_log_paths(log_dir) {
            body.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
        }

        if body.contains(needle) {
            return body;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "log output in {} never contained {needle:?}; body was: {body}",
            log_dir.display()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// Spin up the full HTTP stack (ingest + api) on a random port.
// Returns the base URL and a cancellation handle.
async fn start_server(
    store: Arc<dyn StateModel>,
) -> (
    String,
    mpsc::UnboundedSender<Observation>,
    tokio::task::JoinHandle<()>,
) {
    let (obs_tx, mut obs_rx) = mpsc::unbounded_channel::<Observation>();
    let (broadcast_tx, _) = broadcast::channel::<StreamFrame>(100);
    let (chrome_cmd_tx, _) = mpsc::channel(16);

    let store_for_writer = store.clone();
    let btx = broadcast_tx.clone();
    // Store writer task — mirrors production: insert first so id is assigned,
    // stamp id on obs, serialize once, broadcast the tuple.
    tokio::spawn(async move {
        while let Some(mut obs) = obs_rx.recv().await {
            let insert_copy = obs.clone();
            if let Ok(id) = store_for_writer.insert(insert_copy).await {
                obs.id = id;
                let json = serde_json::to_string(&obs).unwrap_or_default();
                let arc_obs = Arc::new(obs);
                let arc_json: Arc<str> = Arc::from(json);
                let _ = btx.send((arc_obs, arc_json));
            }
        }
    });

    let (_, chrome_state_rx) =
        tokio::sync::watch::channel(daemon8_chrome::ConnectionState::Connected);
    let api_state = daemon8_api::ApiState {
        store,
        stream_tx: broadcast_tx.clone(),
        chrome_cmd_tx,
        chrome_state: chrome_state_rx,
        chrome_endpoint: std::sync::Arc::new(std::sync::Mutex::new(None)),
        lens: std::sync::Arc::new(daemon8_store::LensManager::new(broadcast_tx.subscribe())),
        embedder: None,
        memory_store: None,
        card_store: None,
        envelope_store: None,
        memory_short_store: None,
        memory_reference_store: None,
        memory_long_store: None,
        bookkeeper_store: None,
        embedding_profile_store: None,
        specialist_controller: None,
    };

    let app =
        daemon8_ingest::ingest_router(obs_tx.clone()).merge(daemon8_api::api_router(api_state));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Give the server a moment to start
    tokio::time::sleep(Duration::from_millis(50)).await;

    (base, obs_tx, handle)
}

// -----------------------------------------------------------------------
// Ingest → Store → Query API (the core pipeline)
// -----------------------------------------------------------------------

#[tokio::test]
async fn ingest_to_query_pipeline() {
    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::memory().await.unwrap());
    let (base, _tx, _handle) = start_server(store).await;

    // Ingest an exception
    let resp = reqwest::Client::new()
        .post(format!("{base}/ingest"))
        .json(&json!({
            "kind": "exception",
            "data": {"message": "null pointer"},
            "severity": "error",
            "app": "test-app"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);

    // Ingest a query
    let resp = reqwest::Client::new()
        .post(format!("{base}/ingest"))
        .json(&json!({
            "kind": "query",
            "data": {"sql": "SELECT 1", "duration_ms": 5.0},
            "severity": "info",
            "app": "test-app"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);

    // Small delay for the writer task to drain
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Query all observations via API
    let resp: Value = reqwest::get(format!("{base}/api/observe?limit=10"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let obs = resp["observations"].as_array().unwrap();
    assert_eq!(obs.len(), 2, "expected 2 observations, got {}", obs.len());

    // Query errors only
    let resp: Value = reqwest::get(format!("{base}/api/observe?severity_min=error"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let obs = resp["observations"].as_array().unwrap();
    assert_eq!(obs.len(), 1, "expected 1 error observation");
    assert_eq!(obs[0]["severity"], "error");

    // Query by kind
    let resp: Value = reqwest::get(format!("{base}/api/observe?kinds=query"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let obs = resp["observations"].as_array().unwrap();
    assert_eq!(obs.len(), 1, "expected 1 query observation");
}

// -----------------------------------------------------------------------
// Batch ingest → query
// -----------------------------------------------------------------------

#[tokio::test]
async fn batch_ingest_to_query() {
    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::memory().await.unwrap());
    let (base, _tx, _handle) = start_server(store).await;

    let resp = reqwest::Client::new()
        .post(format!("{base}/ingest/batch"))
        .json(&json!([
            {"kind": "log", "data": {"msg": "one"}, "severity": "info"},
            {"kind": "log", "data": {"msg": "two"}, "severity": "info"},
            {"kind": "exception", "data": {"message": "boom"}, "severity": "error"},
        ]))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["count"], 3);

    tokio::time::sleep(Duration::from_millis(100)).await;

    // All 3 should be queryable
    let resp: Value = reqwest::get(format!("{base}/api/observe?limit=10"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["observations"].as_array().unwrap().len(), 3);

    // Filter to errors only
    let resp: Value = reqwest::get(format!("{base}/api/observe?severity_min=error"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["observations"].as_array().unwrap().len(), 1);
}

// -----------------------------------------------------------------------
// Summary endpoint reflects ingested data
// -----------------------------------------------------------------------

#[tokio::test]
async fn summary_reflects_state() {
    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::memory().await.unwrap());
    let (base, _tx, _handle) = start_server(store).await;

    // Empty state
    let resp: Value = reqwest::get(format!("{base}/api/summary"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["observation_count"], 0);
    assert_eq!(resp["health"], "no_sources");

    // Ingest some data
    reqwest::Client::new()
        .post(format!("{base}/ingest"))
        .json(&json!({"kind": "exception", "data": {"message": "err"}, "severity": "error"}))
        .send()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let resp: Value = reqwest::get(format!("{base}/api/summary"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["observation_count"], 1);
    assert!(resp["error_count_last_60s"].as_u64().unwrap() >= 1);
}

// -----------------------------------------------------------------------
// Checkpoint-based pagination
// -----------------------------------------------------------------------

#[tokio::test]
async fn checkpoint_pagination() {
    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::memory().await.unwrap());
    let (base, _tx, _handle) = start_server(store).await;

    // Ingest first batch
    reqwest::Client::new()
        .post(format!("{base}/ingest/batch"))
        .json(&json!([
            {"kind": "log", "data": {"msg": "old-1"}},
            {"kind": "log", "data": {"msg": "old-2"}},
        ]))
        .send()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Get checkpoint
    let resp: Value = reqwest::get(format!("{base}/api/observe?limit=10"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let checkpoint = resp["checkpoint"].as_u64().unwrap();
    assert_eq!(resp["observations"].as_array().unwrap().len(), 2);

    // Ingest second batch
    reqwest::Client::new()
        .post(format!("{base}/ingest/batch"))
        .json(&json!([
            {"kind": "log", "data": {"msg": "new-1"}},
        ]))
        .send()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Query since checkpoint -- should only get the new one
    let resp: Value = reqwest::get(format!("{base}/api/observe?since={checkpoint}&limit=10"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let obs = resp["observations"].as_array().unwrap();
    assert_eq!(obs.len(), 1, "expected 1 new observation after checkpoint");
}

#[tokio::test]
async fn memory_export_endpoint_streams_paged_ndjson_for_memory_rows() {
    let store = SurrealStore::memory().await.unwrap();
    let memory_store = store.memory_store();
    for (idx, content) in ["one", "two", "three"].iter().enumerate() {
        memory_store
            .save_memory(Memory {
                id: None,
                created_at: (idx + 1) as u64,
                updated_at: (idx + 1) as u64,
                kind: MemoryKind::Pattern,
                content: (*content).into(),
                embedding: None,
                source_observations: Vec::new(),
                tags: vec!["project:daemon8".into()],
                project_slug: "daemon8".into(),
                session_id: None,
                confidence: 1.0,
            })
            .await
            .unwrap();
    }

    let (base, _tx, _handle) = start_server(Arc::new(store)).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/api/memory/export"))
        .json(&json!({
            "query": "SELECT * FROM memory ORDER BY created_at ASC",
            "page_size": 2
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert!(
        resp.headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/x-ndjson"))
    );
    let text = resp.text().await.unwrap();
    let rows: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["content"], "one");
    assert_eq!(rows[1]["content"], "two");
    assert_eq!(rows[2]["content"], "three");

    let resp = client
        .post(format!("{base}/api/memory/export"))
        .json(&json!({
            "query": "SELECT * FROM observation ORDER BY seq ASC",
            "page_size": 2
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);

    let resp = client
        .post(format!("{base}/api/memory/export"))
        .json(&json!({
            "query": "SELECT * FROM memory ORDER BY created_at ASC",
            "page_size": 0
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
}

// -----------------------------------------------------------------------
// SSE streaming receives observations in real-time
// -----------------------------------------------------------------------

#[tokio::test]
async fn sse_stream_receives_observations() {
    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::memory().await.unwrap());
    let (base, _tx, _handle) = start_server(store).await;

    // Connect SSE client
    let (result_tx, mut result_rx) = tokio::sync::mpsc::channel::<String>(10);

    let sse_base = base.clone();
    let sse_task = tokio::spawn(async move {
        use reqwest_eventsource::{Event, EventSource};
        use tokio_stream::StreamExt;

        let mut es = EventSource::get(format!("{sse_base}/api/stream"));
        while let Some(event) = es.next().await {
            match event {
                Ok(Event::Message(msg)) => {
                    let _ = result_tx.send(msg.data).await;
                }
                Ok(Event::Open) => {}
                Err(_) => break,
            }
        }
    });

    // Give SSE time to connect
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Ingest while SSE is listening
    reqwest::Client::new()
        .post(format!("{base}/ingest"))
        .json(&json!({"kind": "log", "data": {"msg": "streamed"}, "severity": "info"}))
        .send()
        .await
        .unwrap();

    // Should receive via SSE within a reasonable time
    let received = tokio::time::timeout(Duration::from_secs(3), result_rx.recv())
        .await
        .expect("SSE timed out")
        .expect("SSE channel closed");

    let obs: Value = serde_json::from_str(&received).unwrap();
    assert_eq!(obs["severity"], "info");
    // Regression: broadcast used to serialize before store.insert assigned id,
    // so every SSE frame shipped id:0. Guard against it returning.
    assert!(
        obs["id"].as_u64().unwrap_or(0) > 0,
        "SSE frame must carry the id assigned by store.insert, got {}",
        obs["id"]
    );

    sse_task.abort();
}

// -----------------------------------------------------------------------
// SSE streaming: server-side filtering
// -----------------------------------------------------------------------

// Minimal SSE reader built on raw reqwest streaming so the test suite can
// attach custom headers (`Last-Event-ID`) without pulling in a second reqwest
// major version via reqwest-eventsource 0.6 (which is pinned to reqwest 0.12).
async fn collect_sse(
    base: String,
    query: &str,
    last_event_id: Option<u64>,
) -> (
    tokio::sync::mpsc::Receiver<(Option<String>, String)>,
    tokio::task::JoinHandle<()>,
) {
    use futures::StreamExt;

    let (tx, rx) = tokio::sync::mpsc::channel::<(Option<String>, String)>(32);
    let url = format!("{base}/api/stream{query}");
    let task = tokio::spawn(async move {
        let client = reqwest::Client::new();
        let mut req = client.get(&url);
        if let Some(id) = last_event_id {
            req = req.header("Last-Event-ID", id.to_string());
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(_) => return,
        };
        let mut body = resp.bytes_stream();
        let mut buf = String::new();
        while let Some(chunk) = body.next().await {
            let bytes = match chunk {
                Ok(b) => b,
                Err(_) => break,
            };
            buf.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(idx) = buf.find("\n\n") {
                let frame = buf[..idx].to_string();
                buf.drain(..idx + 2);
                let mut id: Option<String> = None;
                let mut data = String::new();
                for line in frame.lines() {
                    if let Some(rest) = line.strip_prefix("id:") {
                        id = Some(rest.trim().to_string());
                    } else if let Some(rest) = line.strip_prefix("id: ") {
                        id = Some(rest.to_string());
                    } else if let Some(rest) = line.strip_prefix("data:") {
                        if !data.is_empty() {
                            data.push('\n');
                        }
                        data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                    }
                }
                if !data.is_empty() && tx.send((id, data)).await.is_err() {
                    return;
                }
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    (rx, task)
}

async fn drain_for(
    rx: &mut tokio::sync::mpsc::Receiver<(Option<String>, String)>,
    window: Duration,
) -> Vec<(Option<String>, Value)> {
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + window;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        match tokio::time::timeout(deadline - now, rx.recv()).await {
            Ok(Some((id, data))) => {
                let v: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
                out.push((id, v));
            }
            _ => break,
        }
    }
    out
}

#[tokio::test]
async fn stream_filters_by_kind() {
    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::memory().await.unwrap());
    let (base, _tx, _handle) = start_server(store).await;

    let (mut rx, task) = collect_sse(base.clone(), "?kinds=query", None).await;

    let client = reqwest::Client::new();
    client
        .post(format!("{base}/ingest"))
        .json(&json!({"kind": "log", "data": {"msg": "log1"}, "severity": "info"}))
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/ingest"))
        .json(&json!({
            "kind": "query",
            "data": {"sql": "SELECT 1", "duration_ms": 1.0},
            "severity": "info"
        }))
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/ingest"))
        .json(&json!({"kind": "log", "data": {"msg": "log2"}, "severity": "info"}))
        .send()
        .await
        .unwrap();

    let events = drain_for(&mut rx, Duration::from_millis(800)).await;
    task.abort();

    assert_eq!(events.len(), 1, "expected 1 query event, got {events:?}");
    assert_eq!(events[0].1["kind"]["type"], "query");
}

#[tokio::test]
async fn stream_filters_by_severity() {
    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::memory().await.unwrap());
    let (base, _tx, _handle) = start_server(store).await;

    let (mut rx, task) = collect_sse(base.clone(), "?severity_min=warn", None).await;

    let client = reqwest::Client::new();
    for severity in ["debug", "info", "warn", "error"] {
        client
            .post(format!("{base}/ingest"))
            .json(&json!({
                "kind": "log",
                "data": {"msg": severity},
                "severity": severity,
            }))
            .send()
            .await
            .unwrap();
    }

    let events = drain_for(&mut rx, Duration::from_millis(800)).await;
    task.abort();

    assert_eq!(events.len(), 2, "expected warn + error, got {events:?}");
    let severities: Vec<_> = events
        .iter()
        .map(|(_, v)| v["severity"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(severities.contains(&"warn".to_string()));
    assert!(severities.contains(&"error".to_string()));
}

#[tokio::test]
async fn stream_filters_by_materialized_text() {
    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::memory().await.unwrap());
    let (base, _tx, _handle) = start_server(store).await;

    let (mut rx, task) = collect_sse(base.clone(), "?text_match=domain:device", None).await;

    let client = reqwest::Client::new();
    client
        .post(format!("{base}/ingest"))
        .json(&json!({
            "kind": "log",
            "data": {"msg": "plain payload"},
            "severity": "info",
            "tags": ["domain:device"],
        }))
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/ingest"))
        .json(&json!({
            "kind": "log",
            "data": {"msg": "plain payload"},
            "severity": "info",
            "tags": ["domain:browser"],
        }))
        .send()
        .await
        .unwrap();

    let events = drain_for(&mut rx, Duration::from_millis(800)).await;
    task.abort();

    assert_eq!(
        events.len(),
        1,
        "expected materialized text match, got {events:?}"
    );
    assert_eq!(events[0].1["tags"][0], "domain:device");
}

#[tokio::test]
async fn stream_no_filter_is_firehose() {
    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::memory().await.unwrap());
    let (base, _tx, _handle) = start_server(store).await;

    let (mut rx, task) = collect_sse(base.clone(), "", None).await;

    let client = reqwest::Client::new();
    for severity in ["debug", "info", "warn", "error"] {
        client
            .post(format!("{base}/ingest"))
            .json(&json!({
                "kind": "log",
                "data": {"msg": severity},
                "severity": severity,
            }))
            .send()
            .await
            .unwrap();
    }

    let events = drain_for(&mut rx, Duration::from_millis(800)).await;
    task.abort();

    assert_eq!(events.len(), 4, "no filter should pass every event");
}

#[tokio::test]
async fn stream_last_event_id_replay() {
    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::memory().await.unwrap());
    let (base, _tx, _handle) = start_server(store).await;

    let client = reqwest::Client::new();
    // Seed five observations BEFORE any subscriber connects.
    for i in 1..=5 {
        client
            .post(format!("{base}/ingest"))
            .json(&json!({
                "kind": "log",
                "data": {"msg": format!("seed-{i}")},
                "severity": "info",
            }))
            .send()
            .await
            .unwrap();
    }
    // Let the store writer drain everything.
    tokio::time::sleep(Duration::from_millis(150)).await;

    let (mut rx, task) = collect_sse(base.clone(), "", Some(2)).await;

    let replayed = drain_for(&mut rx, Duration::from_millis(400)).await;
    assert_eq!(
        replayed.len(),
        3,
        "expected replay of ids 3,4,5, got {replayed:?}"
    );
    let replay_ids: Vec<u64> = replayed
        .iter()
        .map(|(id, _)| id.clone().unwrap_or_default().parse().unwrap_or(0))
        .collect();
    assert_eq!(replay_ids, vec![3, 4, 5]);

    // Live frame after replay should continue from id 6.
    client
        .post(format!("{base}/ingest"))
        .json(&json!({
            "kind": "log",
            "data": {"msg": "live-after-replay"},
            "severity": "info",
        }))
        .send()
        .await
        .unwrap();

    let live = drain_for(&mut rx, Duration::from_millis(600)).await;
    task.abort();

    assert_eq!(live.len(), 1, "expected one live frame, got {live:?}");
    let live_id: u64 = live[0].0.clone().unwrap_or_default().parse().unwrap_or(0);
    assert_eq!(live_id, 6);
}

#[tokio::test]
async fn stream_emits_gap_frame_when_resume_below_retention() {
    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::memory().await.unwrap());
    let (base, _tx, _handle) = start_server(store.clone()).await;
    let client = reqwest::Client::new();

    for i in 1..=5 {
        client
            .post(format!("{base}/ingest"))
            .json(&json!({
                "kind": "log",
                "data": {"msg": format!("seed-{i}")},
                "severity": "info",
            }))
            .send()
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(150)).await;

    let future_ts_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
        + 1_000_000_000;
    let pruned = store.cleanup_before(future_ts_ns).await.unwrap();
    assert!(pruned >= 5, "expected to prune >=5, pruned {pruned}");

    client
        .post(format!("{base}/ingest"))
        .json(&json!({
            "kind": "log",
            "data": {"msg": "post-prune"},
            "severity": "info",
        }))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let (mut rx, task) = collect_sse(base, "", Some(2)).await;
    let frames = drain_for(&mut rx, Duration::from_millis(500)).await;
    task.abort();

    let gap = frames
        .iter()
        .find(|(id, data)| id.is_none() && data.get("oldest_available").is_some());
    assert!(
        gap.is_some(),
        "expected gap frame (no id, oldest_available in data), got: {frames:?}"
    );

    let (_id, gap_data) = gap.unwrap();
    assert_eq!(gap_data["since"], 2);
    assert!(
        gap_data["oldest_available"].as_u64().unwrap_or(0) >= 6,
        "oldest_available should be >= 6 after prune+insert, got {gap_data:?}"
    );
}

// -----------------------------------------------------------------------
// Normalization: various JSON shapes produce correct observations
// -----------------------------------------------------------------------

#[tokio::test]
async fn normalization_edge_cases() {
    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::memory().await.unwrap());
    let (base, _tx, _handle) = start_server(store).await;
    let client = reqwest::Client::new();

    // Bare JSON with no meta fields → kind=log, severity=debug
    client
        .post(format!("{base}/ingest"))
        .json(&json!({"message": "bare"}))
        .send()
        .await
        .unwrap();

    // Channel without kind → custom
    client
        .post(format!("{base}/ingest"))
        .json(&json!({"channel": "events", "data": {"x": 1}}))
        .send()
        .await
        .unwrap();

    // Query missing duration_ms → falls back to custom
    client
        .post(format!("{base}/ingest"))
        .json(&json!({"kind": "query", "data": {"sql": "SELECT 1"}}))
        .send()
        .await
        .unwrap();

    // Full query with all fields
    client
        .post(format!("{base}/ingest"))
        .json(&json!({"kind": "query", "data": {"sql": "SELECT 1", "duration_ms": 2.5}}))
        .send()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let resp: Value = reqwest::get(format!("{base}/api/observe?limit=10"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let obs = resp["observations"].as_array().unwrap();
    assert_eq!(obs.len(), 4);

    // Verify kinds
    assert_eq!(obs[0]["kind"]["type"], "log");
    assert_eq!(obs[1]["kind"]["type"], "custom");
    assert_eq!(obs[1]["kind"]["channel"], "events");
    assert_eq!(obs[2]["kind"]["type"], "custom"); // query fallback
    assert_eq!(obs[3]["kind"]["type"], "query"); // proper query
}

// -----------------------------------------------------------------------
// SurrealDB store: same pipeline with real persistence
// -----------------------------------------------------------------------

#[tokio::test]
async fn surreal_store_pipeline() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::open(tmp.path()).await.unwrap());
    let (base, _tx, _handle) = start_server(store).await;

    // Ingest
    reqwest::Client::new()
        .post(format!("{base}/ingest"))
        .json(
            &json!({"kind": "metric", "data": {"name": "cpu", "value": 42.5}, "severity": "info"}),
        )
        .send()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Query
    let resp: Value = reqwest::get(format!("{base}/api/observe?kinds=metric"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let obs = resp["observations"].as_array().unwrap();
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0]["kind"]["type"], "metric");
}

// -----------------------------------------------------------------------
// Health endpoint always works
// -----------------------------------------------------------------------

#[tokio::test]
async fn health_endpoint() {
    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::memory().await.unwrap());
    let (base, _tx, _handle) = start_server(store).await;

    let resp = reqwest::get(format!("{base}/health")).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "ok");
}

#[tokio::test]
async fn serve_writes_rotating_operational_log_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let log_dir = tmp.path().join("logs");
    let store_path = tmp.path().join("store");
    let screenshot_dir = tmp.path().join("screenshots");
    let config_path = tmp.path().join("config.toml");

    write_logging_config(&config_path, &store_path, &screenshot_dir, &log_dir, 3);

    let port = free_port();
    let mut command = daemon8_command();
    let child = command
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "serve",
            "--port",
            &port.to_string(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut child = ChildGuard(child);

    let base = format!("http://127.0.0.1:{port}");
    wait_for_health(&base).await;

    let logs = wait_for_log_contains(&log_dir, "daemon8 started").await;
    assert!(
        logs.contains("logging initialized"),
        "missing logging initialization line; logs were: {logs}"
    );
    assert!(
        logs.contains("log_rotation=\"daily\""),
        "missing configured rotation field; logs were: {logs}"
    );
    assert!(
        logs.contains("log_max_files=3"),
        "missing configured retention field; logs were: {logs}"
    );
    assert!(
        logs.contains("log_stderr=false"),
        "missing configured stderr field; logs were: {logs}"
    );

    child.0.kill().unwrap();
    let _ = child.0.wait();
}

// -----------------------------------------------------------------------
// Text search across observations
// -----------------------------------------------------------------------

#[tokio::test]
async fn text_search_filter() {
    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::memory().await.unwrap());
    let (base, _tx, _handle) = start_server(store).await;
    let client = reqwest::Client::new();

    client
        .post(format!("{base}/ingest"))
        .json(&json!({"kind": "log", "data": {"msg": "connection refused"}}))
        .send()
        .await
        .unwrap();

    client
        .post(format!("{base}/ingest"))
        .json(&json!({"kind": "log", "data": {"msg": "request completed"}}))
        .send()
        .await
        .unwrap();

    client
        .post(format!("{base}/ingest"))
        .json(&json!({
            "kind": "log",
            "data": {"msg": "plain payload"},
            "tags": ["domain:device"],
        }))
        .send()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let resp: Value = reqwest::get(format!("{base}/api/observe?text_match=fused"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let obs = resp["observations"].as_array().unwrap();
    assert_eq!(obs.len(), 1);

    let resp: Value = reqwest::get(format!("{base}/api/observe?text_match=domain:device"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let obs = resp["observations"].as_array().unwrap();
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0]["tags"][0], "domain:device");
}

// -----------------------------------------------------------------------
// /api/browser/act — each variant of ActRequest deserializes into the
// correct BrowserAction with the correct fields.
//
// Spins up a minimal api_router with a captured chrome_cmd channel.
// A spawned task receives the first ChromeCommand::Action, replies with
// a canned success, and returns the BrowserAction so the test can assert
// its variant and fields.
// -----------------------------------------------------------------------

async fn start_act_server() -> (
    String,
    mpsc::Receiver<ChromeCommand>,
    tokio::task::JoinHandle<()>,
) {
    let (chrome_cmd_tx, chrome_cmd_rx) = mpsc::channel::<ChromeCommand>(16);
    let (stream_tx, _) = broadcast::channel::<StreamFrame>(16);
    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::memory().await.unwrap());
    let (_, chrome_state_rx) =
        tokio::sync::watch::channel(daemon8_chrome::ConnectionState::Connected);

    let api_state = daemon8_api::ApiState {
        store,
        stream_tx: stream_tx.clone(),
        chrome_cmd_tx,
        chrome_state: chrome_state_rx,
        chrome_endpoint: std::sync::Arc::new(std::sync::Mutex::new(None)),
        lens: std::sync::Arc::new(daemon8_store::LensManager::new(stream_tx.subscribe())),
        embedder: None,
        memory_store: None,
        card_store: None,
        envelope_store: None,
        memory_short_store: None,
        memory_reference_store: None,
        memory_long_store: None,
        bookkeeper_store: None,
        embedding_profile_store: None,
        specialist_controller: None,
    };
    let app = daemon8_api::api_router(api_state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (base, chrome_cmd_rx, handle)
}

/// Take the next ChromeCommand::Action off the channel, send a canned
/// success reply into its oneshot, and return the BrowserAction variant
/// so the test can assert on its fields.
async fn capture_action(rx: &mut mpsc::Receiver<ChromeCommand>) -> BrowserAction {
    let cmd = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("no chrome command received within 5s")
        .expect("chrome channel closed");
    let action = match cmd {
        ChromeCommand::Action(a) => a,
        ChromeCommand::Connect { endpoint } => {
            panic!("expected Action, got Connect {{ endpoint: {endpoint} }}")
        }
    };

    // Reply so the HTTP handler unblocks.
    match action {
        BrowserAction::EvalJs {
            tab_id,
            expression,
            reply,
        } => {
            let _ = reply.send(Ok("captured".into()));
            BrowserAction::EvalJs {
                tab_id,
                expression,
                reply: tokio::sync::oneshot::channel().0,
            }
        }
        BrowserAction::ListTabs { reply } => {
            let _ = reply.send(Ok(vec![]));
            BrowserAction::ListTabs {
                reply: tokio::sync::oneshot::channel().0,
            }
        }
        BrowserAction::Screenshot {
            tab_id,
            selector,
            reply,
        } => {
            let _ = reply.send(Ok(vec![0x89, 0x50, 0x4E, 0x47]));
            BrowserAction::Screenshot {
                tab_id,
                selector,
                reply: tokio::sync::oneshot::channel().0,
            }
        }
        BrowserAction::InjectCss {
            tab_id,
            css,
            temporary,
            reply,
        } => {
            let _ = reply.send(Ok("style-xyz".into()));
            BrowserAction::InjectCss {
                tab_id,
                css,
                temporary,
                reply: tokio::sync::oneshot::channel().0,
            }
        }
        BrowserAction::RevertCss { tab_id, reply } => {
            let _ = reply.send(Ok(0));
            BrowserAction::RevertCss {
                tab_id,
                reply: tokio::sync::oneshot::channel().0,
            }
        }
        BrowserAction::GetPerformanceMetrics { tab_id, reply } => {
            let _ = reply.send(Ok(vec![]));
            BrowserAction::GetPerformanceMetrics {
                tab_id,
                reply: tokio::sync::oneshot::channel().0,
            }
        }
        BrowserAction::GetDom {
            tab_id,
            selector,
            reply,
        } => {
            let _ = reply.send(Ok("<html/>".into()));
            BrowserAction::GetDom {
                tab_id,
                selector,
                reply: tokio::sync::oneshot::channel().0,
            }
        }
        BrowserAction::SetViewport {
            tab_id,
            width,
            height,
            device_scale_factor,
            mobile,
            user_agent,
            reply,
        } => {
            let _ = reply.send(Ok(()));
            BrowserAction::SetViewport {
                tab_id,
                width,
                height,
                device_scale_factor,
                mobile,
                user_agent,
                reply: tokio::sync::oneshot::channel().0,
            }
        }
        BrowserAction::ClearViewport { tab_id, reply } => {
            let _ = reply.send(Ok(()));
            BrowserAction::ClearViewport {
                tab_id,
                reply: tokio::sync::oneshot::channel().0,
            }
        }
        BrowserAction::SetNetworkConditions {
            tab_id,
            preset,
            reply,
        } => {
            let _ = reply.send(Ok(()));
            BrowserAction::SetNetworkConditions {
                tab_id,
                preset,
                reply: tokio::sync::oneshot::channel().0,
            }
        }
        BrowserAction::Navigate { tab_id, url, reply } => {
            let _ = reply.send(Ok("page title".into()));
            BrowserAction::Navigate {
                tab_id,
                url,
                reply: tokio::sync::oneshot::channel().0,
            }
        }
        BrowserAction::StorageClear {
            tab_id,
            storage_types,
            reply,
        } => {
            let _ = reply.send(Ok(()));
            BrowserAction::StorageClear {
                tab_id,
                storage_types,
                reply: tokio::sync::oneshot::channel().0,
            }
        }
        BrowserAction::StorageInspect { tab_id, reply } => {
            let _ = reply.send(Ok(
                json!({"local_storage": {}, "session_storage": {}, "cookies": []}),
            ));
            BrowserAction::StorageInspect {
                tab_id,
                reply: tokio::sync::oneshot::channel().0,
            }
        }
        BrowserAction::StorageSet {
            tab_id,
            store_type,
            key,
            value,
            reply,
        } => {
            let _ = reply.send(Ok(()));
            BrowserAction::StorageSet {
                tab_id,
                store_type,
                key,
                value,
                reply: tokio::sync::oneshot::channel().0,
            }
        }
        BrowserAction::ElementAtPoint {
            tab_id,
            x,
            y,
            reply,
        } => {
            let _ = reply.send(Ok(json!({"tag": "DIV"})));
            BrowserAction::ElementAtPoint {
                tab_id,
                x,
                y,
                reply: tokio::sync::oneshot::channel().0,
            }
        }
        BrowserAction::NewTab { url, reply } => {
            let _ = reply.send(Ok("FAKE-TARGET-ID".into()));
            BrowserAction::NewTab {
                url,
                reply: tokio::sync::oneshot::channel().0,
            }
        }
        BrowserAction::CloseTab { tab_id, reply } => {
            let _ = reply.send(Ok(()));
            BrowserAction::CloseTab {
                tab_id,
                reply: tokio::sync::oneshot::channel().0,
            }
        }
    }
}

async fn post_act(base: &str, body: Value) -> (reqwest::StatusCode, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/browser/act"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn act_list_tabs_dispatches_list_tabs() {
    let (base, mut rx, _h) = start_act_server().await;
    let capture = tokio::spawn(async move { capture_action(&mut rx).await });

    let (status, _body) = post_act(&base, json!({"action": "list_tabs"})).await;
    assert_eq!(status, 200);

    let action = capture.await.unwrap();
    assert!(matches!(action, BrowserAction::ListTabs { .. }));
}

#[tokio::test]
async fn act_eval_js_extracts_expression_and_tab_id() {
    let (base, mut rx, _h) = start_act_server().await;
    let capture = tokio::spawn(async move { capture_action(&mut rx).await });

    let (status, _body) = post_act(
        &base,
        json!({"action": "eval_js", "expression": "1+1", "tab_id": "tab-a"}),
    )
    .await;
    assert_eq!(status, 200);

    let action = capture.await.unwrap();
    match action {
        BrowserAction::EvalJs {
            tab_id, expression, ..
        } => {
            assert_eq!(expression, "1+1");
            assert_eq!(tab_id.as_deref(), Some("tab-a"));
        }
        other => panic!("expected EvalJs, got {other:?}"),
    }
}

#[tokio::test]
async fn act_eval_js_rejects_missing_expression() {
    let (base, _rx, _h) = start_act_server().await;
    let (status, body) = post_act(&base, json!({"action": "eval_js"})).await;
    assert_eq!(status, 400);
    assert!(
        body["error"].as_str().unwrap_or("").contains("expression"),
        "expected error about missing 'expression', got {body:?}"
    );
}

#[tokio::test]
async fn act_set_viewport_applies_defaults_for_omitted_fields() {
    let (base, mut rx, _h) = start_act_server().await;
    let capture = tokio::spawn(async move { capture_action(&mut rx).await });

    // Only viewport_width provided — rest fall back to defaults.
    let (status, _body) = post_act(
        &base,
        json!({"action": "set_viewport", "viewport_width": 1280}),
    )
    .await;
    assert_eq!(status, 200);

    let action = capture.await.unwrap();
    match action {
        BrowserAction::SetViewport {
            width,
            height,
            device_scale_factor,
            mobile,
            user_agent,
            ..
        } => {
            assert_eq!(width, 1280);
            assert_eq!(height, 844, "default height");
            assert_eq!(device_scale_factor, 2.0, "default scale");
            assert!(mobile, "default mobile = true");
            assert!(user_agent.is_none());
        }
        other => panic!("expected SetViewport, got {other:?}"),
    }
}

#[tokio::test]
async fn act_storage_set_passes_all_three_fields() {
    let (base, mut rx, _h) = start_act_server().await;
    let capture = tokio::spawn(async move { capture_action(&mut rx).await });

    let (status, _body) = post_act(
        &base,
        json!({
            "action": "storage_set",
            "store_type": "localstorage",
            "storage_key": "token",
            "storage_value": "abc123",
        }),
    )
    .await;
    assert_eq!(status, 200);

    let action = capture.await.unwrap();
    match action {
        BrowserAction::StorageSet {
            store_type,
            key,
            value,
            ..
        } => {
            assert_eq!(store_type, "localstorage");
            assert_eq!(key, "token");
            assert_eq!(value, "abc123");
        }
        other => panic!("expected StorageSet, got {other:?}"),
    }
}

#[tokio::test]
async fn act_navigate_requires_url() {
    let (base, _rx, _h) = start_act_server().await;
    let (status, body) = post_act(&base, json!({"action": "navigate"})).await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap_or("").contains("url"));
}

#[tokio::test]
async fn act_inject_css_requires_css() {
    let (base, _rx, _h) = start_act_server().await;
    let (status, body) = post_act(&base, json!({"action": "inject_css"})).await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap_or("").contains("css"));
}

// -----------------------------------------------------------------------
// SSE under concurrent writers: id monotonicity + no-drop + no-duplicate
// invariants after a burst of writes from multiple tasks.
// -----------------------------------------------------------------------

#[tokio::test]
async fn stream_preserves_id_monotonicity_under_concurrent_writers() {
    const WRITERS: usize = 4;
    const PER_WRITER: usize = 250;
    const TOTAL: usize = WRITERS * PER_WRITER;

    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::memory().await.unwrap());
    let (base, _tx, _handle) = start_server(store).await;

    // Subscribe BEFORE writers start so all frames broadcast live.
    let (mut rx, task) = collect_sse(base.clone(), "", None).await;

    let client = reqwest::Client::new();
    let mut writers = Vec::with_capacity(WRITERS);
    for w in 0..WRITERS {
        let client = client.clone();
        let base = base.clone();
        writers.push(tokio::spawn(async move {
            for i in 0..PER_WRITER {
                client
                    .post(format!("{base}/ingest"))
                    .json(&json!({
                        "kind": "log",
                        "data": {"writer": w, "seq": i},
                        "severity": "info",
                    }))
                    .send()
                    .await
                    .unwrap();
            }
        }));
    }

    for w in writers {
        w.await.unwrap();
    }

    // Drain frames until silent for 500ms, or we've seen TOTAL.
    let frames = drain_for(&mut rx, Duration::from_secs(3)).await;
    task.abort();

    let ids: Vec<u64> = frames
        .iter()
        .map(|(id, _)| id.clone().unwrap_or_default().parse().unwrap_or(0))
        .collect();

    assert_eq!(
        ids.len(),
        TOTAL,
        "expected {TOTAL} broadcast frames, got {}",
        ids.len()
    );

    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        ids.len(),
        "broadcast frames must have unique ids; found duplicates among {} frames",
        ids.len()
    );

    for window in ids.windows(2) {
        assert!(
            window[0] < window[1],
            "SSE frame ids must arrive in strictly ascending order; got {} then {}",
            window[0],
            window[1]
        );
    }

    assert_eq!(*sorted.first().unwrap(), 1, "ids should start at 1");
    assert_eq!(
        *sorted.last().unwrap(),
        TOTAL as u64,
        "ids should end at {TOTAL}"
    );
}

#[tokio::test]
async fn act_inject_css_extracts_css_and_temporary_flag() {
    let (base, mut rx, _h) = start_act_server().await;
    let capture = tokio::spawn(async move { capture_action(&mut rx).await });

    let (status, _body) = post_act(
        &base,
        json!({"action": "inject_css", "css": "body{color:red}", "temporary": false}),
    )
    .await;
    assert_eq!(status, 200);

    let action = capture.await.unwrap();
    match action {
        BrowserAction::InjectCss { css, temporary, .. } => {
            assert_eq!(css, "body{color:red}");
            assert!(!temporary);
        }
        other => panic!("expected InjectCss, got {other:?}"),
    }
}
