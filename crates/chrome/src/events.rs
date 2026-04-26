// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use daemon8_types::{Severity, SourceLocation};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Parsed event types -- only the fields we need from CDP
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct ConsoleEvent {
    pub severity: Severity,
    pub message: String,
    pub console_type: String,
    pub source_location: Option<SourceLocation>,
    pub timestamp_ns: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct RequestEvent {
    pub request_id: String,
    pub method: String,
    pub url: String,
    pub timestamp: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct ResponseEvent {
    pub request_id: String,
    pub status: u16,
    pub url: String,
    pub mime_type: String,
    pub timestamp: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct LoadingFailedEvent {
    pub request_id: String,
    pub error_text: String,
    pub canceled: bool,
    pub timestamp: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct ExceptionEvent {
    pub message: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub url: String,
    pub trace: Option<String>,
    pub source_location: Option<SourceLocation>,
    pub timestamp_ns: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct LifecycleEvent {
    pub name: String,
    pub frame_id: String,
    pub timestamp_ns: u64,
}

// ---------------------------------------------------------------------------
// Parsers -- extract fields from CDP event params (serde_json::Value)
// ---------------------------------------------------------------------------

fn cdp_timestamp_to_ns(ts: &Value) -> u64 {
    let secs = ts.as_f64().unwrap_or(0.0);
    if secs <= 0.0 || !secs.is_finite() {
        return 0;
    }
    // CDP timestamps are seconds since Unix epoch. Reject values that
    // are clearly not epoch timestamps (before 2000 or after 2100).
    // Use wall clock as fallback for relative/invalid timestamps.
    if !(946_684_800.0..=4_102_444_800.0).contains(&secs) {
        return std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
    }
    let ns = secs * 1_000_000_000.0;
    ns.min(u64::MAX as f64 - 1.0) as u64
}

fn severity_from_console_type(t: &str) -> Severity {
    match t {
        "error" | "assert" => Severity::Error,
        "warning" => Severity::Warn,
        "info" => Severity::Info,
        "log"
        | "debug"
        | "dir"
        | "dirxml"
        | "table"
        | "trace"
        | "clear"
        | "startGroup"
        | "startGroupCollapsed"
        | "endGroup"
        | "profile"
        | "timeEnd"
        | "count" => Severity::Debug,
        _ => Severity::Debug,
    }
}

fn extract_source_location(stack_trace: &Value) -> Option<SourceLocation> {
    let frame = stack_trace.get("callFrames")?.as_array()?.first()?;
    let file = frame["url"].as_str().unwrap_or("").to_string();
    let line = frame["lineNumber"].as_i64().unwrap_or(0).max(0) as u32;
    let function = frame["functionName"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(String::from);
    Some(SourceLocation {
        file,
        line,
        function,
    })
}

fn format_stack_trace(st: &Value) -> Option<String> {
    let frames = st.get("callFrames")?.as_array()?;
    if frames.is_empty() {
        return None;
    }
    let lines: Vec<String> = frames
        .iter()
        .map(|f| {
            let func = f["functionName"].as_str().unwrap_or("<anonymous>");
            let url = f["url"].as_str().unwrap_or("");
            let line = f["lineNumber"].as_i64().unwrap_or(0);
            let col = f["columnNumber"].as_i64().unwrap_or(0);
            format!("  at {func} ({url}:{line}:{col})")
        })
        .collect();
    Some(lines.join("\n"))
}

pub(crate) fn parse_console(params: &Value) -> Option<ConsoleEvent> {
    let console_type = params["type"].as_str()?.to_string();
    let severity = severity_from_console_type(&console_type);

    let args = params["args"].as_array();
    let message = match args {
        Some(args) => args
            .iter()
            .map(|arg| {
                if let Some(val) = arg.get("value") {
                    match val {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    }
                } else if let Some(desc) = arg["description"].as_str() {
                    desc.to_string()
                } else {
                    "[object]".to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
        None => String::new(),
    };

    let source_location = params.get("stackTrace").and_then(extract_source_location);

    let timestamp_ns = cdp_timestamp_to_ns(&params["timestamp"]);

    Some(ConsoleEvent {
        severity,
        message,
        console_type,
        source_location,
        timestamp_ns,
    })
}

pub(crate) fn parse_request(params: &Value) -> Option<RequestEvent> {
    let request_id = params["requestId"].as_str()?.to_string();
    let request = params.get("request")?;
    let method = request["method"].as_str().unwrap_or("?").to_string();
    let url = request["url"].as_str().unwrap_or("").to_string();
    let timestamp = params["timestamp"].as_f64().unwrap_or(0.0);

    Some(RequestEvent {
        request_id,
        method,
        url,
        timestamp,
    })
}

pub(crate) fn parse_response(params: &Value) -> Option<ResponseEvent> {
    let request_id = params["requestId"].as_str()?.to_string();
    let response = params.get("response")?;
    let status = response["status"].as_u64().unwrap_or(0) as u16;
    let url = response["url"].as_str().unwrap_or("").to_string();
    let mime_type = response["mimeType"].as_str().unwrap_or("").to_string();
    let timestamp = params["timestamp"].as_f64().unwrap_or(0.0);

    Some(ResponseEvent {
        request_id,
        status,
        url,
        mime_type,
        timestamp,
    })
}

pub(crate) fn parse_loading_failed(params: &Value) -> Option<LoadingFailedEvent> {
    let request_id = params["requestId"].as_str()?.to_string();
    let error_text = params["errorText"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let canceled = params["canceled"].as_bool().unwrap_or(false);
    let timestamp = params["timestamp"].as_f64().unwrap_or(0.0);

    Some(LoadingFailedEvent {
        request_id,
        error_text,
        canceled,
        timestamp,
    })
}

pub(crate) fn parse_exception(params: &Value) -> Option<ExceptionEvent> {
    let details = params.get("exceptionDetails")?;
    let message = details["text"]
        .as_str()
        .unwrap_or("unknown exception")
        .to_string();
    let line = details["lineNumber"].as_u64().map(|n| n as u32);
    let column = details["columnNumber"].as_u64().map(|n| n as u32);
    let url = details["url"].as_str().unwrap_or("").to_string();

    let trace = details.get("stackTrace").and_then(format_stack_trace);
    let source_location = details.get("stackTrace").and_then(extract_source_location);
    let timestamp_ns = cdp_timestamp_to_ns(&params["timestamp"]);

    Some(ExceptionEvent {
        message,
        line,
        column,
        url,
        trace,
        source_location,
        timestamp_ns,
    })
}

pub(crate) fn parse_lifecycle(params: &Value) -> Option<LifecycleEvent> {
    let name = params["name"].as_str()?.to_string();
    let frame_id = params["frameId"].as_str().unwrap_or("").to_string();
    let timestamp_ns = cdp_timestamp_to_ns(&params["timestamp"]);

    Some(LifecycleEvent {
        name,
        frame_id,
        timestamp_ns,
    })
}

// ---------------------------------------------------------------------------
// Log.entryAdded -- browser-level log entries (CORS, CSP, deprecation, etc.)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct LogEntryEvent {
    pub severity: Severity,
    pub message: String,
    pub source: String,
    pub url: String,
    pub timestamp_ns: u64,
}

fn severity_from_log_level(level: &str) -> Severity {
    match level {
        "error" => Severity::Error,
        "warning" => Severity::Warn,
        "info" => Severity::Info,
        "verbose" => Severity::Debug,
        _ => Severity::Debug,
    }
}

pub(crate) fn parse_log_entry(params: &Value) -> Option<LogEntryEvent> {
    let entry = params.get("entry")?;
    let text = entry["text"].as_str()?.to_string();
    let level = entry["level"].as_str().unwrap_or("info");
    let source = entry["source"].as_str().unwrap_or("other").to_string();
    let url = entry["url"].as_str().unwrap_or("").to_string();
    let timestamp_ns = cdp_timestamp_to_ns(&entry["timestamp"]);

    Some(LogEntryEvent {
        severity: severity_from_log_level(level),
        message: text,
        source,
        url,
        timestamp_ns,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_log_basic() {
        let params = serde_json::json!({
            "type": "log",
            "args": [{"type": "string", "value": "hello world"}],
            "timestamp": 1711300000.123
        });

        let event = parse_console(&params).unwrap();
        assert_eq!(event.severity, Severity::Debug);
        assert_eq!(event.message, "hello world");
        assert_eq!(event.console_type, "log");
        assert!(event.source_location.is_none());
        assert!(event.timestamp_ns > 0);
    }

    #[test]
    fn console_error_with_stack() {
        let params = serde_json::json!({
            "type": "error",
            "args": [{"type": "string", "value": "something failed"}],
            "timestamp": 1711300000.0,
            "stackTrace": {
                "callFrames": [{
                    "url": "https://example.com/app.js",
                    "lineNumber": 42,
                    "columnNumber": 10,
                    "functionName": "handleClick"
                }]
            }
        });

        let event = parse_console(&params).unwrap();
        assert_eq!(event.severity, Severity::Error);
        assert_eq!(event.message, "something failed");
        let loc = event.source_location.unwrap();
        assert_eq!(loc.file, "https://example.com/app.js");
        assert_eq!(loc.line, 42);
        assert_eq!(loc.function.as_deref(), Some("handleClick"));
    }

    #[test]
    fn console_warning() {
        let params = serde_json::json!({
            "type": "warning",
            "args": [{"type": "string", "value": "deprecated API"}],
            "timestamp": 0.0
        });
        let event = parse_console(&params).unwrap();
        assert_eq!(event.severity, Severity::Warn);
    }

    #[test]
    fn console_info() {
        let params = serde_json::json!({
            "type": "info",
            "args": [{"type": "string", "value": "server started"}],
            "timestamp": 0.0
        });
        let event = parse_console(&params).unwrap();
        assert_eq!(event.severity, Severity::Info);
    }

    #[test]
    fn console_with_object_args() {
        let params = serde_json::json!({
            "type": "log",
            "args": [
                {"type": "string", "value": "user:"},
                {"type": "object", "description": "Object {name: 'Alice'}"}
            ],
            "timestamp": 0.0
        });

        let event = parse_console(&params).unwrap();
        assert_eq!(event.message, "user: Object {name: 'Alice'}");
    }

    #[test]
    fn console_with_numeric_value() {
        let params = serde_json::json!({
            "type": "log",
            "args": [{"type": "number", "value": 42}],
            "timestamp": 0.0
        });
        let event = parse_console(&params).unwrap();
        assert_eq!(event.message, "42");
    }

    #[test]
    fn console_unknown_type_defaults_to_debug() {
        let params = serde_json::json!({
            "type": "futureNewType",
            "args": [{"type": "string", "value": "test"}],
            "timestamp": 0.0
        });
        let event = parse_console(&params).unwrap();
        assert_eq!(event.severity, Severity::Debug);
    }

    #[test]
    fn console_missing_type_returns_none() {
        let params = serde_json::json!({"args": [], "timestamp": 0.0});
        assert!(parse_console(&params).is_none());
    }

    #[test]
    fn request_basic() {
        let params = serde_json::json!({
            "requestId": "req-1",
            "request": {
                "method": "GET",
                "url": "https://api.example.com/users"
            },
            "timestamp": 1711300000.5
        });

        let event = parse_request(&params).unwrap();
        assert_eq!(event.request_id, "req-1");
        assert_eq!(event.method, "GET");
        assert_eq!(event.url, "https://api.example.com/users");
        assert!((event.timestamp - 1711300000.5).abs() < 0.001);
    }

    #[test]
    fn request_missing_id_returns_none() {
        let params = serde_json::json!({"request": {"method": "GET", "url": "x"}});
        assert!(parse_request(&params).is_none());
    }

    #[test]
    fn response_basic() {
        let params = serde_json::json!({
            "requestId": "req-1",
            "response": {
                "status": 200,
                "url": "https://api.example.com/users",
                "mimeType": "application/json"
            },
            "timestamp": 1711300001.0
        });

        let event = parse_response(&params).unwrap();
        assert_eq!(event.request_id, "req-1");
        assert_eq!(event.status, 200);
        assert_eq!(event.mime_type, "application/json");
    }

    #[test]
    fn response_404() {
        let params = serde_json::json!({
            "requestId": "req-2",
            "response": {"status": 404, "url": "/missing", "mimeType": "text/html"},
            "timestamp": 0.0
        });
        let event = parse_response(&params).unwrap();
        assert_eq!(event.status, 404);
    }

    #[test]
    fn loading_failed_basic() {
        let params = serde_json::json!({
            "requestId": "req-3",
            "errorText": "net::ERR_CONNECTION_REFUSED",
            "canceled": false,
            "timestamp": 0.0
        });

        let event = parse_loading_failed(&params).unwrap();
        assert_eq!(event.request_id, "req-3");
        assert_eq!(event.error_text, "net::ERR_CONNECTION_REFUSED");
        assert!(!event.canceled);
    }

    #[test]
    fn loading_failed_canceled() {
        let params = serde_json::json!({
            "requestId": "req-4",
            "errorText": "canceled",
            "canceled": true,
            "timestamp": 0.0
        });
        let event = parse_loading_failed(&params).unwrap();
        assert!(event.canceled);
    }

    #[test]
    fn exception_with_stack() {
        let params = serde_json::json!({
            "timestamp": 1711300000.0,
            "exceptionDetails": {
                "text": "Uncaught TypeError: x is not a function",
                "lineNumber": 15,
                "columnNumber": 8,
                "url": "https://example.com/app.js",
                "stackTrace": {
                    "callFrames": [
                        {"url": "https://example.com/app.js", "lineNumber": 15, "columnNumber": 8, "functionName": "init"},
                        {"url": "https://example.com/app.js", "lineNumber": 100, "columnNumber": 1, "functionName": ""}
                    ]
                }
            }
        });

        let event = parse_exception(&params).unwrap();
        assert_eq!(event.message, "Uncaught TypeError: x is not a function");
        assert_eq!(event.line, Some(15));
        assert_eq!(event.column, Some(8));
        assert!(event.trace.as_ref().unwrap().contains("init"));
        let loc = event.source_location.unwrap();
        assert_eq!(loc.line, 15);
        assert_eq!(loc.function.as_deref(), Some("init"));
    }

    #[test]
    fn exception_without_stack() {
        let params = serde_json::json!({
            "timestamp": 0.0,
            "exceptionDetails": {
                "text": "Script error.",
                "lineNumber": 0,
                "columnNumber": 0
            }
        });
        let event = parse_exception(&params).unwrap();
        assert_eq!(event.message, "Script error.");
        assert!(event.trace.is_none());
        assert!(event.source_location.is_none());
    }

    #[test]
    fn exception_missing_details_returns_none() {
        let params = serde_json::json!({"timestamp": 0.0});
        assert!(parse_exception(&params).is_none());
    }

    #[test]
    fn lifecycle_load() {
        let params = serde_json::json!({
            "name": "load",
            "frameId": "frame-abc",
            "timestamp": 1711300002.0
        });

        let event = parse_lifecycle(&params).unwrap();
        assert_eq!(event.name, "load");
        assert_eq!(event.frame_id, "frame-abc");
        assert!(event.timestamp_ns > 0);
    }

    #[test]
    fn lifecycle_dom_content_loaded() {
        let params = serde_json::json!({
            "name": "DOMContentLoaded",
            "frameId": "frame-xyz",
            "timestamp": 0.0
        });
        let event = parse_lifecycle(&params).unwrap();
        assert_eq!(event.name, "DOMContentLoaded");
    }

    #[test]
    fn lifecycle_missing_name_returns_none() {
        let params = serde_json::json!({"frameId": "x", "timestamp": 0.0});
        assert!(parse_lifecycle(&params).is_none());
    }

    #[test]
    fn timestamp_clamps_negative() {
        let ts = serde_json::json!(-1.0);
        assert_eq!(cdp_timestamp_to_ns(&ts), 0);
    }

    #[test]
    fn timestamp_handles_missing() {
        let ts = Value::Null;
        assert_eq!(cdp_timestamp_to_ns(&ts), 0);
    }

    #[test]
    fn timestamp_out_of_epoch_range_uses_wall_clock() {
        // A relative timestamp (like 10.5ms from timeEnd) is not epoch-based
        let ts = serde_json::json!(0.0105);
        let ns = cdp_timestamp_to_ns(&ts);
        // Should be a recent wall clock time, not 10.5ms since epoch
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        assert!(ns > now_ns - 1_000_000_000); // within 1 second of now
    }

    #[test]
    fn console_assert_is_error() {
        let params = serde_json::json!({
            "type": "assert",
            "args": [{"type": "string", "value": "assertion failed"}],
            "timestamp": 0.0
        });
        let event = parse_console(&params).unwrap();
        assert_eq!(event.severity, Severity::Error);
    }

    #[test]
    fn console_empty_function_name_excluded() {
        let params = serde_json::json!({
            "type": "log",
            "args": [{"type": "string", "value": "x"}],
            "timestamp": 0.0,
            "stackTrace": {
                "callFrames": [{
                    "url": "test.js",
                    "lineNumber": 1,
                    "columnNumber": 0,
                    "functionName": ""
                }]
            }
        });
        let event = parse_console(&params).unwrap();
        let loc = event.source_location.unwrap();
        assert!(loc.function.is_none());
    }
}
