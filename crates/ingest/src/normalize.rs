// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

use std::sync::Arc;

use daemon8_types::{Observation, ObservationKind, Origin, Severity, SourceLocation};

/// Normalize arbitrary JSON into an Observation. Extracts recognized meta
/// fields (kind, channel, severity, file, line, function, app, data) and
/// maps the remainder into the observation payload.
pub fn normalize(mut raw: Value) -> Observation {
    let obj = raw.as_object_mut();

    let (kind_str, channel_str, severity_str, file, line, function, app, explicit_data, meta) =
        match obj {
            Some(map) => extract_meta(map),
            None => (
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                ObsMeta::default(),
            ),
        };

    let severity = parse_severity(severity_str.as_deref());

    let source_location = file.map(|f| SourceLocation {
        file: f,
        line: line.unwrap_or(0),
        function,
    });

    let origin = Origin::Application {
        name: app.map(Into::into).unwrap_or_else(|| "unknown".into()),
    };

    let data = unwrap_stringified_json(explicit_data.unwrap_or(raw));
    let kind = resolve_kind(kind_str.as_deref(), channel_str, &data);

    let timestamp_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;

    Observation {
        id: 0,
        origin,
        kind,
        data,
        severity,
        source_location,
        timestamp_ns,
        correlation_id: meta.correlation_id.map(Arc::from),
        parent_id: meta.parent_id,
        tags: meta.tags,
        session_id: meta.session_id.map(Arc::from),
        node_id: meta.node_id.map(Arc::from),
        debug_session_id: None,
        checkpoint_id: None,
        error_hash: None,
    }
}

#[derive(Default)]
struct ObsMeta {
    correlation_id: Option<String>,
    parent_id: Option<u64>,
    tags: Option<Vec<String>>,
    session_id: Option<String>,
    node_id: Option<String>,
}

type MetaFields = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<u32>,
    Option<String>,
    Option<String>,
    Option<Value>,
    ObsMeta,
);

fn extract_meta(map: &mut Map<String, Value>) -> MetaFields {
    let kind = map
        .remove("kind")
        .and_then(|v| v.as_str().map(String::from));
    let channel = map
        .remove("channel")
        .and_then(|v| v.as_str().map(String::from));
    let severity = map
        .remove("severity")
        .and_then(|v| v.as_str().map(String::from));
    let file = map
        .remove("file")
        .and_then(|v| v.as_str().map(String::from));
    let line = map
        .remove("line")
        .and_then(|v| v.as_u64().map(|n| n as u32));
    let function = map
        .remove("function")
        .and_then(|v| v.as_str().map(String::from));
    let app = map.remove("app").and_then(|v| v.as_str().map(String::from));
    let data = map.remove("data");

    let correlation_id = map
        .remove("correlation_id")
        .and_then(|v| v.as_str().map(String::from));
    let parent_id = map.remove("parent_id").and_then(|v| v.as_u64());
    let tags = map.remove("tags").and_then(|v| {
        v.as_array().map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str().map(String::from))
                .collect()
        })
    });
    let session_id = map
        .remove("session_id")
        .and_then(|v| v.as_str().map(String::from));
    let node_id = map
        .remove("node_id")
        .and_then(|v| v.as_str().map(String::from));

    let meta = ObsMeta {
        correlation_id,
        parent_id,
        tags,
        session_id,
        node_id,
    };

    (
        kind, channel, severity, file, line, function, app, data, meta,
    )
}

pub fn parse_severity(s: Option<&str>) -> Severity {
    match s {
        Some(v) => match v.to_ascii_lowercase().as_str() {
            "trace" => Severity::Trace,
            "debug" => Severity::Debug,
            "info" => Severity::Info,
            "warn" | "warning" => Severity::Warn,
            "error" => Severity::Error,
            _ => Severity::Debug,
        },
        None => Severity::Debug,
    }
}

fn unwrap_stringified_json(v: Value) -> Value {
    match &v {
        Value::String(s) if s.starts_with('{') || s.starts_with('[') => {
            serde_json::from_str(s).unwrap_or(v)
        }
        _ => v,
    }
}

fn resolve_kind(kind_str: Option<&str>, channel: Option<String>, data: &Value) -> ObservationKind {
    match kind_str {
        Some(k) => match k.to_ascii_lowercase().as_str() {
            "log" => ObservationKind::Log,
            "query" => try_query(data),
            "http" | "http_exchange" => try_http(data),
            "exception" => try_exception(data),
            "metric" => try_metric(data),
            "state_snapshot" => try_state_snapshot(data),
            "tool_call" => try_tool_call(data),
            "js_exception" => try_js_exception(data),
            "lifecycle" => try_lifecycle(data),
            "custom" => ObservationKind::Custom {
                channel: channel.unwrap_or_else(|| "custom".into()),
            },
            _ => ObservationKind::Custom {
                channel: channel.unwrap_or_else(|| k.to_string()),
            },
        },
        None => match channel {
            Some(ch) => ObservationKind::Custom { channel: ch },
            None => ObservationKind::Log,
        },
    }
}

fn try_query(data: &Value) -> ObservationKind {
    let sql = data.get("sql").and_then(Value::as_str);
    let duration_ms = data.get("duration_ms").and_then(Value::as_f64);
    match (sql, duration_ms) {
        (Some(sql), Some(ms)) => ObservationKind::Query {
            sql: sql.to_string(),
            duration_ms: ms,
        },
        _ => ObservationKind::Custom {
            channel: "query".into(),
        },
    }
}

fn try_http(data: &Value) -> ObservationKind {
    let method = data.get("method").and_then(Value::as_str);
    let url = data.get("url").and_then(Value::as_str);
    match (method, url) {
        (Some(m), Some(u)) => ObservationKind::HttpExchange {
            method: m.to_string(),
            url: u.to_string(),
            status: data.get("status").and_then(Value::as_u64).map(|s| s as u16),
            duration_ms: data.get("duration_ms").and_then(Value::as_f64),
        },
        _ => ObservationKind::Custom {
            channel: "http".into(),
        },
    }
}

fn try_exception(data: &Value) -> ObservationKind {
    let message = data.get("message").and_then(Value::as_str);
    match message {
        Some(msg) => ObservationKind::Exception {
            message: msg.to_string(),
            trace: data.get("trace").and_then(Value::as_str).map(String::from),
        },
        None => ObservationKind::Custom {
            channel: "exception".into(),
        },
    }
}

fn try_tool_call(data: &Value) -> ObservationKind {
    let tool = data
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let input = data.get("input").cloned().unwrap_or(Value::Null);
    let output = data.get("output").cloned();
    let exit_code = data
        .get("exit_code")
        .and_then(Value::as_i64)
        .map(|n| n as i32);
    let duration_ms = data.get("duration_ms").and_then(Value::as_f64);
    ObservationKind::ToolCall {
        tool,
        input,
        output,
        exit_code,
        duration_ms,
    }
}

fn try_metric(data: &Value) -> ObservationKind {
    let name = data.get("name").and_then(Value::as_str);
    let value = data.get("value").and_then(Value::as_f64);
    match (name, value) {
        (Some(n), Some(v)) => ObservationKind::Metric {
            name: n.to_string(),
            value: v,
        },
        _ => ObservationKind::Custom {
            channel: "metric".into(),
        },
    }
}

fn try_state_snapshot(data: &Value) -> ObservationKind {
    let label = data.get("label").and_then(Value::as_str);
    match label {
        Some(l) => ObservationKind::StateSnapshot {
            label: l.to_string(),
        },
        None => ObservationKind::Custom {
            channel: "state_snapshot".into(),
        },
    }
}

fn try_js_exception(data: &Value) -> ObservationKind {
    let message = data.get("message").and_then(Value::as_str);
    match message {
        Some(message) => ObservationKind::JsException {
            message: message.to_string(),
            line: data.get("line").and_then(Value::as_u64).map(|n| n as u32),
            column: data.get("column").and_then(Value::as_u64).map(|n| n as u32),
        },
        None => ObservationKind::Custom {
            channel: "js_exception".into(),
        },
    }
}

fn try_lifecycle(data: &Value) -> ObservationKind {
    let event_name = data.get("event_name").and_then(Value::as_str);
    let frame_id = data.get("frame_id").and_then(Value::as_str);
    match (event_name, frame_id) {
        (Some(event_name), Some(frame_id)) => ObservationKind::Lifecycle {
            event_name: event_name.to_string(),
            frame_id: frame_id.to_string(),
        },
        _ => ObservationKind::Custom {
            channel: "lifecycle".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwrap_stringified_object() {
        let input = Value::String(r#"{"message":"hello"}"#.into());
        let out = unwrap_stringified_json(input);
        assert_eq!(out, serde_json::json!({"message": "hello"}));
    }

    #[test]
    fn unwrap_stringified_array() {
        let input = Value::String(r#"[1,2,3]"#.into());
        let out = unwrap_stringified_json(input);
        assert_eq!(out, serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn passthrough_already_object() {
        let input = serde_json::json!({"message": "hello"});
        let out = unwrap_stringified_json(input.clone());
        assert_eq!(out, input);
    }

    #[test]
    fn passthrough_plain_string() {
        let input = Value::String("just a message".into());
        let out = unwrap_stringified_json(input.clone());
        assert_eq!(out, input);
    }

    #[test]
    fn passthrough_invalid_json_string() {
        let input = Value::String("{not valid json".into());
        let out = unwrap_stringified_json(input.clone());
        assert_eq!(out, input);
    }

    #[test]
    fn resolves_documented_live_feed_kinds() {
        let http = resolve_kind(
            Some("http_exchange"),
            None,
            &serde_json::json!({"method": "GET", "url": "/health"}),
        );
        assert!(matches!(http, ObservationKind::HttpExchange { .. }));

        let js = resolve_kind(
            Some("js_exception"),
            None,
            &serde_json::json!({"message": "boom", "line": 12, "column": 3}),
        );
        assert!(matches!(js, ObservationKind::JsException { .. }));

        let lifecycle = resolve_kind(
            Some("lifecycle"),
            None,
            &serde_json::json!({"event_name": "load", "frame_id": "frame-1"}),
        );
        assert!(matches!(lifecycle, ObservationKind::Lifecycle { .. }));
    }
}
