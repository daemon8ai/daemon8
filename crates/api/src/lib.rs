// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use daemon8_chrome::BrowserAction;
use daemon8_ingest::source_sync::{SourceSyncReport, SourceTrigger, SourceTriggerRequest};
use daemon8_mcp::ChromeCommand;
use daemon8_store::{LensManager, StateModel};
use daemon8_types::{Checkpoint, Filter, Observation};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;
use tokio_stream::wrappers::ReceiverStream;
use tracing::warn;

#[derive(Clone)]
pub struct ApiState {
    pub store: Arc<dyn StateModel>,
    pub stream_tx: tokio::sync::broadcast::Sender<(Arc<Observation>, Arc<str>)>,
    pub chrome_cmd_tx: tokio::sync::mpsc::Sender<ChromeCommand>,
    pub chrome_state: tokio::sync::watch::Receiver<daemon8_chrome::ConnectionState>,
    pub chrome_endpoint: Arc<std::sync::Mutex<Option<Arc<str>>>>,
    pub lens: Arc<LensManager>,
    pub source_trigger: Option<Arc<dyn SourceTrigger>>,
}

pub fn api_router(state: ApiState) -> Router {
    Router::new()
        .route("/api/observe", get(handle_observe))
        .route("/api/checkpoint", get(handle_checkpoint))
        .route("/api/summary", get(handle_summary))
        .route("/api/connections", get(handle_connections))
        .route("/api/connect", post(handle_connect))
        .route("/api/stream", get(handle_stream))
        .route(
            "/api/lens",
            get(handle_lens_status)
                .put(handle_lens_set)
                .delete(handle_lens_clear),
        )
        .route("/api/browser/act", post(handle_chrome_act))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
pub struct ObserveQueryParams {
    pub kinds: Option<String>,
    pub severity_min: Option<String>,
    pub origins: Option<String>,
    pub text_match: Option<String>,
    pub since: Option<u64>,
    pub limit: Option<u64>,
    pub correlation_id: Option<String>,
    pub tags: Option<String>,
    pub service: Option<String>,
    pub source: Option<String>,
    pub source_instance: Option<String>,
    pub include_system: Option<bool>,
    pub project_path: Option<String>,
}

/// Query params for the live SSE stream. `since` and `limit` are deliberately
/// omitted — stream replay is driven by the `Last-Event-ID` header, and the
/// stream itself is unbounded.
#[derive(Debug, Deserialize)]
pub struct StreamQueryParams {
    pub kinds: Option<String>,
    pub severity_min: Option<String>,
    pub origins: Option<String>,
    pub text_match: Option<String>,
    pub correlation_id: Option<String>,
    pub tags: Option<String>,
    pub service: Option<String>,
    pub source: Option<String>,
    pub source_instance: Option<String>,
    pub include_system: Option<bool>,
    pub project_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConnectBody {
    pub endpoint: String,
}

/// Tagged sum of all browser-automation requests. Each variant carries
/// exactly the fields relevant to its action. Serde rejects the request
/// at deserialize time if a required field is missing or the `action`
/// tag is unknown, so the handler below never has to defensively unwrap.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ActRequest {
    EvalJs {
        tab_id: Option<String>,
        expression: String,
    },
    Screenshot {
        tab_id: Option<String>,
        selector: Option<String>,
    },
    InjectCss {
        tab_id: Option<String>,
        css: String,
        #[serde(default = "default_temporary")]
        temporary: bool,
    },
    RevertCss {
        tab_id: Option<String>,
    },
    ListTabs,
    GetPerfMetrics {
        tab_id: Option<String>,
    },
    GetDom {
        tab_id: Option<String>,
        selector: Option<String>,
    },
    SetViewport {
        tab_id: Option<String>,
        #[serde(default = "default_viewport_width")]
        viewport_width: u32,
        #[serde(default = "default_viewport_height")]
        viewport_height: u32,
        #[serde(default = "default_viewport_scale")]
        viewport_scale: f64,
        #[serde(default = "default_viewport_mobile")]
        viewport_mobile: bool,
        viewport_ua: Option<String>,
    },
    ClearViewport {
        tab_id: Option<String>,
    },
    NetworkConditions {
        tab_id: Option<String>,
        #[serde(default = "default_network_preset")]
        network_preset: String,
    },
    Navigate {
        tab_id: Option<String>,
        url: String,
    },
    StorageClear {
        tab_id: Option<String>,
        #[serde(default = "default_storage_types")]
        storage_types: String,
    },
    StorageInspect {
        tab_id: Option<String>,
    },
    StorageSet {
        tab_id: Option<String>,
        #[serde(default = "default_store_type")]
        store_type: String,
        #[serde(default)]
        storage_key: String,
        #[serde(default)]
        storage_value: String,
    },
    ElementAtPoint {
        tab_id: Option<String>,
        #[serde(default)]
        x: f64,
        #[serde(default)]
        y: f64,
    },
    NewTab {
        #[serde(default = "default_new_tab_url")]
        url: String,
    },
    CloseTab {
        tab_id: String,
    },
}

fn default_temporary() -> bool {
    true
}
fn default_viewport_width() -> u32 {
    390
}
fn default_viewport_height() -> u32 {
    844
}
fn default_viewport_scale() -> f64 {
    2.0
}
fn default_viewport_mobile() -> bool {
    true
}
fn default_network_preset() -> String {
    "restore".into()
}
fn default_new_tab_url() -> String {
    "about:blank".into()
}
fn default_storage_types() -> String {
    "all".into()
}
fn default_store_type() -> String {
    "localstorage".into()
}

/// Shared parser used by `handle_observe` and `handle_stream`. Unknown values
/// inside each comma-separated list are logged and skipped; the overall parse
/// never fails.
struct FilterInput {
    kinds: Option<String>,
    severity_min: Option<String>,
    origins: Option<String>,
    text_match: Option<String>,
    since: Option<u64>,
    limit: Option<usize>,
    correlation_id: Option<String>,
    tags: Option<String>,
    service: Option<String>,
    source: Option<String>,
    source_instance: Option<String>,
    include_system: Option<bool>,
}

fn parse_filter(input: FilterInput) -> Filter {
    let FilterInput {
        kinds,
        severity_min,
        origins,
        text_match,
        since,
        limit,
        correlation_id,
        tags,
        service,
        source,
        source_instance,
        include_system,
    } = input;

    Filter {
        kinds: kinds.map(|raw| Filter::parse_kinds(&raw)),
        severity_min: severity_min.and_then(|raw| Filter::parse_severity(&raw)),
        origins: origins.map(|raw| Filter::parse_origins(&raw)),
        text_match,
        since: since.map(Checkpoint),
        limit,
        correlation_id,
        tags: tags.map(|raw| Filter::parse_tags(&raw)),
        service: service.map(|raw| Filter::parse_string_list(&raw)),
        source: source.map(|raw| Filter::parse_string_list(&raw)),
        source_instance: source_instance.map(|raw| Filter::parse_string_list(&raw)),
        include_system,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use daemon8_ingest::source_sync::{
        ConfiguredSourceTrigger, ObservationWriteResult, ObservationWriteStatus, ObservationWriter,
    };
    use daemon8_store::SurrealStore;
    use http_body_util::BodyExt;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tower::ServiceExt;

    struct DirectStoreWriter {
        store: Arc<dyn StateModel>,
    }

    #[async_trait::async_trait]
    impl ObservationWriter for DirectStoreWriter {
        async fn write_observation(
            &self,
            obs: Observation,
        ) -> Result<ObservationWriteResult, String> {
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

    async fn app_with_source_trigger() -> (Router, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join("cargo.log"), "cargo check one\n").unwrap();
        std::fs::create_dir(tmp.path().join(".daemon8")).unwrap();
        std::fs::write(
            tmp.path().join(".daemon8/config.md"),
            format!(
                r#"---
daemon8_schema: 1
created_at: "2026-05-17T00:00:00Z"
updated_at: "2026-05-17T00:00:00Z"
project:
  name: api-project
  stack:
    languages: [rust]
    frameworks: [tokio]
    tools: [cargo]
vars:
  PRJ_ROOT: "{}"
sources:
  - id: cargo.check
    service: cargo
    kind: file
    parser: line
    path: "$PRJ_ROOT/cargo.log"
---
# daemon8
"#,
                tmp.path().display()
            ),
        )
        .unwrap();

        let store = Arc::new(SurrealStore::memory().await.unwrap());
        let store_model: Arc<dyn StateModel> = store.clone();
        let source_trigger = ConfiguredSourceTrigger::new(
            Arc::new(store.cursor_store()),
            Arc::new(DirectStoreWriter {
                store: store_model.clone(),
            }),
        );
        let (stream_tx, _) = tokio::sync::broadcast::channel(16);
        let (chrome_cmd_tx, _) = tokio::sync::mpsc::channel(16);
        let (_, chrome_state) =
            tokio::sync::watch::channel(daemon8_chrome::ConnectionState::Disconnected);
        let state = ApiState {
            store: store_model,
            stream_tx: stream_tx.clone(),
            chrome_cmd_tx,
            chrome_state,
            chrome_endpoint: Arc::new(std::sync::Mutex::new(None)),
            lens: Arc::new(LensManager::new(stream_tx.subscribe())),
            source_trigger: Some(Arc::new(source_trigger)),
        };
        (api_router(state), tmp)
    }

    fn drain_sse_frames(buf: &mut String, frames: &mut Vec<(Option<String>, String)>) {
        while let Some((idx, sep_len)) = sse_frame_end(buf) {
            let frame = buf[..idx].to_string();
            buf.drain(..idx + sep_len);
            let mut event = None;
            let mut data = String::new();
            for line in frame.lines() {
                let line = line.trim_end_matches('\r');
                if let Some(rest) = line.strip_prefix("event:") {
                    event = Some(rest.trim().to_string());
                } else if let Some(rest) = line.strip_prefix("data:") {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                }
            }
            if !data.is_empty() {
                frames.push((event, data));
            }
        }
    }

    fn sse_frame_end(buf: &str) -> Option<(usize, usize)> {
        buf.find("\r\n\r\n")
            .map(|idx| (idx, 4))
            .or_else(|| buf.find("\n\n").map(|idx| (idx, 2)))
    }

    #[tokio::test]
    async fn observe_triggers_sources_when_project_path_is_explicit() {
        let (app, tmp) = app_with_source_trigger().await;
        let uri = format!(
            "/api/observe?project_path={}&source=cargo.check",
            tmp.path().display()
        );
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(body["triggered_ingestion"]["observations_written"], 1);
        assert_eq!(body["observations"].as_array().unwrap().len(), 1);
        assert_eq!(body["observations"][0]["source"], "cargo.check");
    }

    #[tokio::test]
    async fn observe_without_project_path_does_not_trigger_sources() {
        let (app, _tmp) = app_with_source_trigger().await;
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/observe?source=cargo.check")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert!(body.get("triggered_ingestion").is_none());
        assert_eq!(body["observations"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn stream_emits_triggered_ingestion_before_replay() {
        let (app, tmp) = app_with_source_trigger().await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let request = format!(
            "GET /api/stream?project_path={}&source=cargo.check HTTP/1.1\r\nHost: {addr}\r\nAccept: text/event-stream\r\nLast-Event-ID: 0\r\n\r\n",
            tmp.path().display()
        );
        stream.write_all(request.as_bytes()).await.unwrap();

        let mut headers = String::new();
        let mut body = String::new();
        let mut frames = Vec::new();
        let mut headers_seen = false;
        let mut bytes = [0u8; 4096];
        while frames.len() < 2 {
            let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut bytes))
                .await
                .expect("timed out waiting for SSE bytes")
                .expect("failed to read SSE bytes");
            assert!(read > 0, "SSE stream ended before expected frames");
            let chunk = String::from_utf8_lossy(&bytes[..read]);
            if headers_seen {
                body.push_str(&chunk);
            } else {
                headers.push_str(&chunk);
                if let Some(idx) = headers.find("\r\n\r\n") {
                    assert!(headers.starts_with("HTTP/1.1 200"), "{headers}");
                    body.push_str(&headers[idx + 4..]);
                    headers_seen = true;
                }
            }
            drain_sse_frames(&mut body, &mut frames);
        }
        server.abort();

        assert_eq!(frames[0].0.as_deref(), Some("triggered_ingestion"));
        let report: serde_json::Value = serde_json::from_str(&frames[0].1).unwrap();
        assert_eq!(report["observations_written"], 1);
        let obs: serde_json::Value = serde_json::from_str(&frames[1].1).unwrap();
        assert_eq!(obs["source"], "cargo.check");
    }
}

fn error_json(status: StatusCode, message: impl Into<String>) -> Response {
    let body = serde_json::json!({ "error": message.into() });
    (status, Json(body)).into_response()
}

async fn trigger_project_sources(
    state: &ApiState,
    project_path: Option<&str>,
) -> Option<SourceSyncReport> {
    let project_path = project_path?;
    let trigger = state.source_trigger.as_ref()?;
    Some(
        trigger
            .trigger_sources(SourceTriggerRequest::project(std::path::PathBuf::from(
                project_path,
            )))
            .await,
    )
}

fn source_report_value(report: &SourceSyncReport) -> serde_json::Value {
    serde_json::to_value(report).unwrap_or_else(|_| serde_json::json!({}))
}

async fn handle_observe(
    State(state): State<ApiState>,
    Query(params): Query<ObserveQueryParams>,
) -> Response {
    let source_report = trigger_project_sources(&state, params.project_path.as_deref()).await;
    let filter = parse_filter(FilterInput {
        kinds: params.kinds,
        severity_min: params.severity_min,
        origins: params.origins,
        text_match: params.text_match,
        since: params.since,
        limit: Some(params.limit.unwrap_or(50).min(500) as usize),
        correlation_id: params.correlation_id,
        tags: params.tags,
        service: params.service,
        source: params.source,
        source_instance: params.source_instance,
        include_system: params.include_system,
    });

    match state.store.query(&filter).await {
        Ok(slice) => {
            let mut body = serde_json::to_value(slice).unwrap_or_default();
            if let Some(report) = source_report {
                body["triggered_ingestion"] = source_report_value(&report);
            }
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("query failed: {e}"),
        ),
    }
}

async fn handle_checkpoint(State(state): State<ApiState>) -> Response {
    let cp = state.store.checkpoint().await;
    (
        StatusCode::OK,
        Json(serde_json::json!({ "checkpoint": cp.0 })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct LensSetBody {
    pub kinds: Option<String>,
    pub severity_min: Option<String>,
    pub origins: Option<String>,
    pub text_match: Option<String>,
    pub correlation_id: Option<String>,
    pub tags: Option<String>,
    pub service: Option<String>,
    pub source: Option<String>,
    pub source_instance: Option<String>,
    pub include_system: Option<bool>,
    pub capacity: Option<usize>,
}

async fn handle_lens_set(State(state): State<ApiState>, Json(body): Json<LensSetBody>) -> Response {
    let filter = parse_filter(FilterInput {
        kinds: body.kinds,
        severity_min: body.severity_min,
        origins: body.origins,
        text_match: body.text_match,
        since: None,
        limit: None,
        correlation_id: body.correlation_id,
        tags: body.tags,
        service: body.service,
        source: body.source,
        source_instance: body.source_instance,
        include_system: body.include_system,
    });
    let capacity = body.capacity.unwrap_or(200).min(1000);
    state.lens.set_with_capacity(filter, capacity).await;

    let status = state.lens.status().await;
    (StatusCode::OK, Json(status)).into_response()
}

async fn handle_lens_clear(State(state): State<ApiState>) -> Response {
    state.lens.clear().await;
    (StatusCode::OK, Json(serde_json::json!({"cleared": true}))).into_response()
}

async fn handle_lens_status(State(state): State<ApiState>) -> Response {
    let status = state.lens.status().await;
    (StatusCode::OK, Json(status)).into_response()
}

async fn handle_summary(State(state): State<ApiState>) -> Response {
    match state.store.summary().await {
        Ok(summary) => (StatusCode::OK, Json(summary)).into_response(),
        Err(e) => error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("summary failed: {e}"),
        ),
    }
}

async fn handle_connections(State(state): State<ApiState>) -> Response {
    let chrome_state = *state.chrome_state.borrow();
    let chrome_endpoint = state
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

    if let Ok(summary) = state.store.summary().await
        && !summary.connections.is_empty()
    {
        result["applications"] = serde_json::to_value(&summary.connections).unwrap_or_default();
    }

    (StatusCode::OK, Json(result)).into_response()
}

async fn handle_connect(State(state): State<ApiState>, Json(body): Json<ConnectBody>) -> Response {
    let endpoint = body.endpoint.clone();
    match state
        .chrome_cmd_tx
        .send(ChromeCommand::Connect {
            endpoint: body.endpoint,
        })
        .await
    {
        Ok(()) => {
            *state
                .chrome_endpoint
                .lock()
                .expect("chrome_endpoint mutex poisoned") = Some(Arc::from(endpoint.as_str()));
            let resp = serde_json::json!({
                "status": "connecting",
                "endpoint": endpoint,
            });
            (StatusCode::ACCEPTED, Json(resp)).into_response()
        }
        Err(e) => error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to send connect command: {e}"),
        ),
    }
}

/// Replay cap for `Last-Event-ID` resume and `Lagged` recovery. Clients with
/// deeper gaps are expected to fall back to `GET /api/observe`.
const STREAM_REPLAY_LIMIT: usize = 1000;

/// Emit stored observations to the stream, stamping the correct SSE id on each.
/// Returns the highest id emitted, or `start_from` if nothing matched. A failure
/// to send means the client disconnected — caller should terminate.
async fn emit_replay(
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
    observations: Vec<Observation>,
) -> Result<u64, ()> {
    let mut highest = 0;
    for obs in observations {
        let id = obs.id;
        let json = match serde_json::to_string(&obs) {
            Ok(s) => s,
            Err(e) => {
                warn!("failed to serialize replayed observation {id}: {e}");
                continue;
            }
        };
        let event = Event::default().id(id.to_string()).data(json);
        if tx.send(Ok(event)).await.is_err() {
            return Err(());
        }
        highest = id;
    }
    Ok(highest)
}

async fn handle_stream(
    State(state): State<ApiState>,
    Query(params): Query<StreamQueryParams>,
    headers: HeaderMap,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    // Subscribe BEFORE querying the store so the live window is captured while
    // we replay history. Any frames that arrive between subscribe and the first
    // live recv() buffer in the broadcast receiver; Lagged recovery handles
    // overflow.
    let mut broadcast_rx = state.stream_tx.subscribe();
    let project_path = params.project_path.clone();

    let filter = parse_filter(FilterInput {
        kinds: params.kinds,
        severity_min: params.severity_min,
        origins: params.origins,
        text_match: params.text_match,
        since: None,
        limit: None,
        correlation_id: params.correlation_id,
        tags: params.tags,
        service: params.service,
        source: params.source,
        source_instance: params.source_instance,
        include_system: params.include_system,
    });

    let last_event_id: Option<u64> = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse().ok());

    let store = state.store.clone();
    let source_trigger = state.source_trigger.clone();
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(256);

    tokio::spawn(async move {
        let mut max_replayed_id: u64 = 0;
        let mut last_sent_id: u64 = 0;

        if let Some(project_path) = project_path.as_deref()
            && let Some(trigger) = source_trigger.as_ref()
        {
            let report = trigger
                .trigger_sources(SourceTriggerRequest::project(std::path::PathBuf::from(
                    project_path,
                )))
                .await;
            let event = Event::default()
                .event("triggered_ingestion")
                .data(source_report_value(&report).to_string());
            if tx.send(Ok(event)).await.is_err() {
                return;
            }
        }

        // Phase 1 — historical replay driven by Last-Event-ID.
        if let Some(since_id) = last_event_id {
            // Gap detection: if the store's oldest retained id is greater than
            // since_id + 1, some observations the client expects are gone.
            // Emit a synthetic `event: gap` frame so gap-aware clients can
            // surface the loss; clients that don't handle the event type
            // ignore it per SSE spec.
            if let Some(oldest) = store.oldest_id().await
                && oldest > since_id + 1
            {
                let gap = Event::default().event("gap").data(
                    serde_json::json!({
                        "since": since_id,
                        "oldest_available": oldest,
                    })
                    .to_string(),
                );
                if tx.send(Ok(gap)).await.is_err() {
                    return;
                }
            }

            let replay_filter = Filter {
                since: Some(Checkpoint(since_id)),
                limit: Some(STREAM_REPLAY_LIMIT),
                ..filter.clone()
            };
            match store.query(&replay_filter).await {
                Ok(slice) => match emit_replay(&tx, slice.observations).await {
                    Ok(highest) => {
                        if highest > 0 {
                            max_replayed_id = highest;
                            last_sent_id = highest;
                        }
                    }
                    Err(()) => return,
                },
                Err(e) => warn!("stream replay query failed: {e}"),
            }
        }

        // Phase 2 — live streaming with handoff dedup and lagged recovery.
        loop {
            match broadcast_rx.recv().await {
                Ok((arc_obs, arc_json)) => {
                    // Dedup frames already covered by replay.
                    if arc_obs.id <= max_replayed_id {
                        continue;
                    }
                    if !filter.matches(&arc_obs) {
                        continue;
                    }
                    let event = Event::default()
                        .id(arc_obs.id.to_string())
                        .data(arc_json.as_ref());
                    if tx.send(Ok(event)).await.is_err() {
                        return;
                    }
                    last_sent_id = arc_obs.id;
                }
                Err(RecvError::Lagged(skipped)) => {
                    warn!(
                        skipped,
                        last_sent_id, "SSE subscriber lagged; healing via store replay"
                    );
                    let heal_filter = Filter {
                        since: Some(Checkpoint(last_sent_id)),
                        limit: Some(STREAM_REPLAY_LIMIT),
                        ..filter.clone()
                    };
                    match store.query(&heal_filter).await {
                        Ok(slice) => match emit_replay(&tx, slice.observations).await {
                            Ok(highest) => {
                                if highest > last_sent_id {
                                    last_sent_id = highest;
                                }
                                if highest > max_replayed_id {
                                    max_replayed_id = highest;
                                }
                            }
                            Err(()) => return,
                        },
                        Err(e) => warn!("stream lagged-recovery query failed: {e}"),
                    }
                }
                Err(RecvError::Closed) => return,
            }
        }
    });

    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

const ACTION_TIMEOUT: Duration = Duration::from_secs(30);

/// Send a BrowserAction to the chrome command channel, await the reply,
/// and map the success payload to an HTTP response. Every variant handler
/// below delegates here, so the "channel closed, timed out, action error"
/// error paths are centralized.
async fn dispatch_action<T, F, R>(
    chrome_cmd_tx: &tokio::sync::mpsc::Sender<ChromeCommand>,
    build: F,
    on_success: R,
) -> Response
where
    F: FnOnce(tokio::sync::oneshot::Sender<daemon8_chrome::Result<T>>) -> BrowserAction,
    R: FnOnce(T) -> Response,
{
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let action = build(reply_tx);
    if let Err(e) = chrome_cmd_tx.send(ChromeCommand::Action(action)).await {
        return error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("send failed: {e}"),
        );
    }
    match tokio::time::timeout(ACTION_TIMEOUT, reply_rx).await {
        Err(_) => error_json(
            StatusCode::GATEWAY_TIMEOUT,
            "browser action timed out (30s)",
        ),
        Ok(Err(_)) => error_json(StatusCode::INTERNAL_SERVER_ERROR, "reply channel closed"),
        Ok(Ok(Err(e))) => error_json(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")),
        Ok(Ok(Ok(value))) => on_success(value),
    }
}

fn ok_json(body: serde_json::Value) -> Response {
    (StatusCode::OK, Json(body)).into_response()
}

async fn ensure_chrome_connected(state: &ApiState) -> Result<(), Response> {
    use daemon8_chrome::ConnectionState;

    let current = *state.chrome_state.borrow();
    match current {
        ConnectionState::Connected => return Ok(()),
        ConnectionState::Disconnected => {
            let endpoint = state
                .chrome_endpoint
                .lock()
                .expect("chrome_endpoint mutex poisoned")
                .as_ref()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "http://localhost:9222".to_string());
            let _ = state
                .chrome_cmd_tx
                .send(ChromeCommand::Connect { endpoint })
                .await;
        }
        ConnectionState::Connecting | ConnectionState::Reconnecting => {}
    }

    let mut rx = state.chrome_state.clone();
    match tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if *rx.borrow_and_update() == ConnectionState::Connected {
                return;
            }
            if rx.changed().await.is_err() {
                return;
            }
        }
    })
    .await
    {
        Ok(()) if *rx.borrow() == ConnectionState::Connected => {
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok(())
        }
        _ => Err(error_json(
            StatusCode::SERVICE_UNAVAILABLE,
            "cdp: Browser not connected (timed out waiting for connection)",
        )),
    }
}

async fn handle_chrome_act(
    State(state): State<ApiState>,
    body: Result<Json<ActRequest>, JsonRejection>,
) -> Response {
    let Json(req) = match body {
        Ok(j) => j,
        Err(rejection) => return error_json(StatusCode::BAD_REQUEST, rejection.body_text()),
    };

    if let Err(resp) = ensure_chrome_connected(&state).await {
        return resp;
    }

    let tx = &state.chrome_cmd_tx;

    match req {
        ActRequest::ListTabs => {
            dispatch_action(
                tx,
                |reply| BrowserAction::ListTabs { reply },
                |tabs| ok_json(serde_json::json!({ "tabs": tabs })),
            )
            .await
        }
        ActRequest::EvalJs { tab_id, expression } => {
            dispatch_action(
                tx,
                |reply| BrowserAction::EvalJs {
                    tab_id,
                    expression,
                    reply,
                },
                |result| ok_json(serde_json::json!({ "result": result })),
            )
            .await
        }
        ActRequest::Screenshot { tab_id, selector } => {
            dispatch_action(
                tx,
                |reply| BrowserAction::Screenshot {
                    tab_id,
                    selector,
                    reply,
                },
                |bytes: Vec<u8>| {
                    let tmp = std::env::temp_dir().join(format!(
                        "daemon8-screenshot-{}.png",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis()
                    ));
                    if let Err(e) = std::fs::write(&tmp, &bytes) {
                        return error_json(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("failed to write screenshot: {e}"),
                        );
                    }
                    ok_json(serde_json::json!({
                        "screenshot": tmp.to_string_lossy(),
                        "size_bytes": bytes.len(),
                    }))
                },
            )
            .await
        }
        ActRequest::InjectCss {
            tab_id,
            css,
            temporary,
        } => {
            dispatch_action(
                tx,
                |reply| BrowserAction::InjectCss {
                    tab_id,
                    css,
                    temporary,
                    reply,
                },
                |element_id| ok_json(serde_json::json!({ "injected": true, "element_id": element_id, "temporary": temporary })),
            )
            .await
        }
        ActRequest::RevertCss { tab_id } => {
            dispatch_action(
                tx,
                |reply| BrowserAction::RevertCss { tab_id, reply },
                |count| ok_json(serde_json::json!({ "reverted_count": count })),
            )
            .await
        }
        ActRequest::GetPerfMetrics { tab_id } => {
            dispatch_action(
                tx,
                |reply| BrowserAction::GetPerformanceMetrics { tab_id, reply },
                |metrics| ok_json(serde_json::json!({ "metrics": metrics })),
            )
            .await
        }
        ActRequest::GetDom { tab_id, selector } => {
            dispatch_action(
                tx,
                |reply| BrowserAction::GetDom {
                    tab_id,
                    selector,
                    reply,
                },
                |html| ok_json(serde_json::json!({ "html": html })),
            )
            .await
        }
        ActRequest::SetViewport {
            tab_id,
            viewport_width,
            viewport_height,
            viewport_scale,
            viewport_mobile,
            viewport_ua,
        } => {
            dispatch_action(
                tx,
                |reply| BrowserAction::SetViewport {
                    tab_id,
                    width: viewport_width,
                    height: viewport_height,
                    device_scale_factor: viewport_scale,
                    mobile: viewport_mobile,
                    user_agent: viewport_ua,
                    reply,
                },
                |()| {
                    ok_json(serde_json::json!({
                        "viewport_set": true,
                        "width": viewport_width,
                        "height": viewport_height,
                        "scale": viewport_scale,
                        "mobile": viewport_mobile,
                    }))
                },
            )
            .await
        }
        ActRequest::ClearViewport { tab_id } => {
            dispatch_action(
                tx,
                |reply| BrowserAction::ClearViewport { tab_id, reply },
                |()| ok_json(serde_json::json!({ "viewport_cleared": true })),
            )
            .await
        }
        ActRequest::Navigate { tab_id, url } => {
            dispatch_action(
                tx,
                |reply| BrowserAction::Navigate { tab_id, url, reply },
                |title| ok_json(serde_json::json!({ "navigated": true, "title": title })),
            )
            .await
        }
        ActRequest::NetworkConditions {
            tab_id,
            network_preset,
        } => {
            dispatch_action(
                tx,
                |reply| BrowserAction::SetNetworkConditions {
                    tab_id,
                    preset: network_preset,
                    reply,
                },
                |()| ok_json(serde_json::json!({ "status": "ok" })),
            )
            .await
        }
        ActRequest::StorageClear {
            tab_id,
            storage_types,
        } => {
            dispatch_action(
                tx,
                |reply| BrowserAction::StorageClear {
                    tab_id,
                    storage_types,
                    reply,
                },
                |()| ok_json(serde_json::json!({ "cleared": true })),
            )
            .await
        }
        ActRequest::StorageInspect { tab_id } => {
            dispatch_action(
                tx,
                |reply| BrowserAction::StorageInspect { tab_id, reply },
                ok_json,
            )
            .await
        }
        ActRequest::StorageSet {
            tab_id,
            store_type,
            storage_key,
            storage_value,
        } => {
            dispatch_action(
                tx,
                |reply| BrowserAction::StorageSet {
                    tab_id,
                    store_type,
                    key: storage_key,
                    value: storage_value,
                    reply,
                },
                |()| ok_json(serde_json::json!({ "set": true })),
            )
            .await
        }
        ActRequest::ElementAtPoint { tab_id, x, y } => {
            dispatch_action(
                tx,
                |reply| BrowserAction::ElementAtPoint {
                    tab_id,
                    x,
                    y,
                    reply,
                },
                ok_json,
            )
            .await
        }
        ActRequest::NewTab { url } => {
            dispatch_action(
                tx,
                |reply| BrowserAction::NewTab { url, reply },
                |target_id| ok_json(serde_json::json!({ "tab_id": target_id })),
            )
            .await
        }
        ActRequest::CloseTab { tab_id } => {
            dispatch_action(
                tx,
                |reply| BrowserAction::CloseTab { tab_id, reply },
                |()| ok_json(serde_json::json!({ "closed": true })),
            )
            .await
        }
    }
}
