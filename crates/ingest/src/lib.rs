// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

mod error;
pub mod normalize;
pub mod source_sync;
pub mod udp;
#[cfg(unix)]
pub mod unix;

pub use error::{IngestError, Result};

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use daemon8_types::Observation;

#[derive(Clone)]
struct IngestState {
    tx: UnboundedSender<Observation>,
}

pub fn ingest_router(tx: UnboundedSender<Observation>) -> Router {
    let state = IngestState { tx };

    Router::new()
        .route("/ingest", post(handle_ingest))
        .route("/ingest/batch", post(handle_batch))
        .route("/health", get(handle_health))
        .with_state(state)
}

async fn handle_health() -> &'static str {
    "ok"
}

async fn handle_ingest(
    State(state): State<IngestState>,
    axum::Json(body): axum::Json<Value>,
) -> (StatusCode, axum::Json<Value>) {
    let obs = normalize::normalize(body);

    tracing::debug!(
        kind = ?obs.kind,
        severity = ?obs.severity,
        origin = ?obs.origin,
        "ingested observation"
    );

    if state.tx.send(obs).is_err() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({"ok": false, "error": "ingest channel closed"})),
        );
    }

    (StatusCode::ACCEPTED, axum::Json(json!({"ok": true})))
}

async fn handle_batch(
    State(state): State<IngestState>,
    axum::Json(items): axum::Json<Vec<Value>>,
) -> (StatusCode, axum::Json<Value>) {
    let count = items.len();
    let mut dropped = 0usize;

    for item in items {
        let obs = normalize::normalize(item);
        if state.tx.send(obs).is_err() {
            dropped += 1;
        }
    }

    tracing::debug!(count, dropped, "ingested batch");

    if dropped > 0 {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({
                "ok": false,
                "error": "ingest channel closed",
                "count": count,
                "dropped": dropped,
            })),
        );
    }

    (
        StatusCode::ACCEPTED,
        axum::Json(json!({"ok": true, "count": count})),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use daemon8_types::{ObservationKind, Origin, Severity};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn router() -> (Router, tokio::sync::mpsc::UnboundedReceiver<Observation>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (ingest_router(tx), rx)
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let (app, _rx) = router();

        let req = axum::http::Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn ingest_returns_202_and_sends_observation() {
        let (app, mut rx) = router();

        let payload = json!({
            "kind": "exception",
            "severity": "error",
            "data": { "message": "boom", "trace": "at line 1" },
            "app": "test-app",
            "file": "/src/main.rs",
            "line": 42,
        });

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/ingest")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&payload).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let obs = rx.try_recv().expect("should have received an observation");
        assert!(matches!(obs.kind, ObservationKind::Exception { .. }));
        assert!(matches!(obs.severity, Severity::Error));
        assert!(matches!(obs.origin, Origin::Application { ref name } if name == "test-app"));
        assert_eq!(obs.source_location.as_ref().unwrap().file, "/src/main.rs");
        assert_eq!(obs.source_location.as_ref().unwrap().line, 42);
    }

    #[tokio::test]
    async fn ingest_with_channel_becomes_custom() {
        let (app, mut rx) = router();

        let payload = json!({
            "channel": "my-events",
            "data": { "foo": "bar" },
        });

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/ingest")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&payload).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let obs = rx.try_recv().unwrap();
        assert!(
            matches!(obs.kind, ObservationKind::Custom { ref channel } if channel == "my-events")
        );
    }

    #[tokio::test]
    async fn ingest_bare_json_defaults_to_log() {
        let (app, mut rx) = router();

        let payload = json!({ "message": "hello world" });

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/ingest")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&payload).unwrap()))
            .unwrap();

        let _ = app.oneshot(req).await.unwrap();

        let obs = rx.try_recv().unwrap();
        assert!(matches!(obs.kind, ObservationKind::Log));
        assert!(matches!(obs.severity, Severity::Debug));
        assert!(matches!(obs.origin, Origin::Application { ref name } if name == "unknown"));
    }

    #[test]
    fn parse_severity_variants() {
        assert!(matches!(
            normalize::parse_severity(Some("trace")),
            Severity::Trace
        ));
        assert!(matches!(
            normalize::parse_severity(Some("INFO")),
            Severity::Info
        ));
        assert!(matches!(
            normalize::parse_severity(Some("warning")),
            Severity::Warn
        ));
        assert!(matches!(
            normalize::parse_severity(Some("garbage")),
            Severity::Debug
        ));
        assert!(matches!(normalize::parse_severity(None), Severity::Debug));
    }

    #[tokio::test]
    async fn ingest_returns_503_when_channel_closed() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Observation>();
        let app = ingest_router(tx);
        drop(rx);

        let payload = json!({
            "kind": "log",
            "data": { "msg": "shutdown probe" },
            "app": "test-app",
        });

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/ingest")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&payload).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body: Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(body["ok"], false);
        assert_eq!(body["error"], "ingest channel closed");
    }

    #[tokio::test]
    async fn batch_returns_503_when_channel_closed() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Observation>();
        let app = ingest_router(tx);
        drop(rx);

        let payload = json!([
            {"kind": "log", "data": {"msg": "one"}, "app": "test"},
            {"kind": "log", "data": {"msg": "two"}, "app": "test"},
        ]);

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/ingest/batch")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&payload).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body: Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(body["ok"], false);
        assert_eq!(body["error"], "ingest channel closed");
        assert_eq!(body["count"], 2);
        assert_eq!(body["dropped"], 2);
    }

    #[tokio::test]
    async fn batch_ingests_multiple() {
        let (app, mut rx) = router();

        let payload = json!([
            {"kind": "log", "data": {"msg": "one"}, "app": "test"},
            {"kind": "log", "data": {"msg": "two"}, "app": "test"},
            {"kind": "exception", "data": {"message": "boom"}, "severity": "error"},
        ]);

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/ingest/batch")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&payload).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let body: Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();

        assert_eq!(body["count"], 3);

        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_ok());
        let third = rx.try_recv().unwrap();
        assert!(matches!(third.kind, ObservationKind::Exception { .. }));
    }
}
