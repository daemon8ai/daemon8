// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use daemon8_types::Severity;
use serde_json::Value;

use crate::{ParsedLine, Parser};

pub struct JsonParser;

impl Parser for JsonParser {
    fn name(&self) -> &str {
        "json"
    }

    fn parse(&self, line: &str) -> Option<ParsedLine> {
        let trimmed = line.trim();
        let mut obj: serde_json::Map<String, Value> = serde_json::from_str(trimmed).ok()?;

        let severity = extract_first(&mut obj, &["level", "severity", "lvl"])
            .and_then(|v| value_to_severity(&v));

        let message = extract_first(&mut obj, &["msg", "message", "text"])
            .map(|v| value_to_string(&v))
            .unwrap_or_default();

        let timestamp = extract_first(&mut obj, &["timestamp", "ts", "time", "@timestamp"])
            .map(|v| value_to_string(&v));

        let channel =
            extract_first(&mut obj, &["channel", "logger", "name"]).map(|v| value_to_string(&v));

        Some(ParsedLine {
            timestamp,
            severity,
            channel,
            message,
            fields: obj,
        })
    }
}

fn extract_first(obj: &mut serde_json::Map<String, Value>, keys: &[&str]) -> Option<Value> {
    for key in keys {
        if let Some(val) = obj.remove(*key) {
            return Some(val);
        }
    }
    None
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn value_to_severity(v: &Value) -> Option<Severity> {
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => return n.as_u64().map(numeric_to_severity),
        _ => return None,
    };
    match s.to_ascii_lowercase().as_str() {
        "trace" => Some(Severity::Trace),
        "debug" | "dbg" => Some(Severity::Debug),
        "info" | "information" | "informational" => Some(Severity::Info),
        "warn" | "warning" => Some(Severity::Warn),
        "error" | "err" | "fatal" | "critical" | "crit" | "alert" | "emergency" | "emerg" => {
            Some(Severity::Error)
        }
        _ => None,
    }
}

fn numeric_to_severity(n: u64) -> Severity {
    match n {
        0..=99 => Severity::Trace,
        100..=199 => Severity::Debug,
        200..=299 => Severity::Info,
        300..=399 => Severity::Warn,
        _ => Severity::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("json")
            .join(name)
    }

    #[test]
    fn parse_structured_log() {
        let parser = JsonParser;
        let line = r#"{"timestamp":"2024-01-15T14:32:01Z","level":"error","msg":"connection refused","host":"db-01"}"#;
        let result = parser.parse(line).expect("should parse");
        assert_eq!(result.severity, Some(Severity::Error));
        assert_eq!(result.message, "connection refused");
        assert_eq!(result.timestamp.as_deref(), Some("2024-01-15T14:32:01Z"));
        assert_eq!(
            result.fields.get("host"),
            Some(&Value::String("db-01".to_string()))
        );
    }

    #[test]
    fn parse_alternative_field_names() {
        let parser = JsonParser;
        let line = r#"{"ts":"2024-01-15","severity":"warn","message":"disk full"}"#;
        let result = parser.parse(line).expect("should parse");
        assert_eq!(result.severity, Some(Severity::Warn));
        assert_eq!(result.message, "disk full");
        assert_eq!(result.timestamp.as_deref(), Some("2024-01-15"));
    }

    #[test]
    fn non_json_returns_none() {
        let parser = JsonParser;
        assert!(parser.parse("plain text").is_none());
        assert!(parser.parse("[1, 2, 3]").is_none());
        assert!(parser.parse("").is_none());
    }

    #[test]
    fn remaining_fields_preserved() {
        let parser = JsonParser;
        let line = r#"{"msg":"hi","level":"info","service":"api","region":"us-east-1"}"#;
        let result = parser.parse(line).expect("should parse");
        assert_eq!(result.fields.len(), 2);
        assert!(result.fields.contains_key("service"));
        assert!(result.fields.contains_key("region"));
    }

    #[test]
    fn parse_fixtures() {
        let parser = JsonParser;
        let fixture = fixture_path("structured.log");
        let content = std::fs::read_to_string(&fixture)
            .unwrap_or_else(|e| panic!("read fixture {}: {e}", fixture.display()));
        let mut parsed_count = 0;
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Some(result) = parser.parse(line) {
                assert!(!result.message.is_empty());
                parsed_count += 1;
            }
        }
        assert!(
            parsed_count > 0,
            "should parse at least one line from fixture"
        );
    }

    #[test]
    fn parse_various_formats() {
        let parser = JsonParser;
        let fixture = fixture_path("various_formats.log");
        let content = std::fs::read_to_string(&fixture)
            .unwrap_or_else(|e| panic!("read fixture {}: {e}", fixture.display()));
        let mut parsed_count = 0;
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Some(_result) = parser.parse(line) {
                parsed_count += 1;
            }
        }
        assert!(
            parsed_count >= 5,
            "should parse at least 5 lines from various formats fixture"
        );
    }
}
