// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::sync::LazyLock;

use daemon8_types::Severity;
use regex::Regex;

use crate::{ParsedLine, Parser};

// Common Log Format, optionally followed by Combined fields (referrer + user-agent)
static CLF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^(\S+)\s+(\S+)\s+(\S+)\s+\[([^\]]+)\]\s+"(\S+)\s+(\S+)\s+(\S+)"\s+(\d{3})\s+(\S+)(?:\s+"([^"]*?)"\s+"([^"]*?)")?\s*$"#,
    )
    .expect("clf regex is valid")
});

pub struct ClfParser;

impl Parser for ClfParser {
    fn name(&self) -> &str {
        "clf"
    }

    fn parse(&self, line: &str) -> Option<ParsedLine> {
        let caps = CLF_RE.captures(line)?;

        let client_ip = caps.get(1)?.as_str();
        let _ident = caps.get(2)?.as_str();
        let _user = caps.get(3)?.as_str();
        let timestamp = caps.get(4).map(|m| m.as_str().to_string());
        let method = caps.get(5)?.as_str();
        let path = caps.get(6)?.as_str();
        let protocol = caps.get(7)?.as_str();
        let status: u16 = caps.get(8)?.as_str().parse().ok()?;
        let bytes = caps.get(9)?.as_str();

        let severity = status_to_severity(status);
        let message = format!("{method} {path} {status}");

        let mut fields = serde_json::Map::new();
        fields.insert(
            "client_ip".to_string(),
            serde_json::Value::String(client_ip.to_string()),
        );
        fields.insert(
            "method".to_string(),
            serde_json::Value::String(method.to_string()),
        );
        fields.insert(
            "path".to_string(),
            serde_json::Value::String(path.to_string()),
        );
        fields.insert(
            "protocol".to_string(),
            serde_json::Value::String(protocol.to_string()),
        );
        fields.insert(
            "status".to_string(),
            serde_json::Value::Number(status.into()),
        );

        if bytes != "-"
            && let Ok(n) = bytes.parse::<u64>()
        {
            fields.insert("bytes".to_string(), serde_json::Value::Number(n.into()));
        }

        if let Some(referrer) = caps.get(10) {
            let r = referrer.as_str();
            if r != "-" {
                fields.insert(
                    "referrer".to_string(),
                    serde_json::Value::String(r.to_string()),
                );
            }
        }

        if let Some(ua) = caps.get(11) {
            fields.insert(
                "user_agent".to_string(),
                serde_json::Value::String(ua.as_str().to_string()),
            );
        }

        Some(ParsedLine {
            timestamp,
            severity: Some(severity),
            channel: None,
            message,
            fields,
        })
    }
}

fn status_to_severity(status: u16) -> Severity {
    match status {
        200..=399 => Severity::Info,
        400..=499 => Severity::Warn,
        500..=599 => Severity::Error,
        _ => Severity::Info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("clf")
            .join(name)
    }

    #[test]
    fn parse_common_log_format() {
        let parser = ClfParser;
        let line =
            r#"127.0.0.1 - frank [10/Oct/2000:13:55:36 -0700] "GET /apache_pb.gif HTTP/1.0" 200 2326"#;
        let result = parser.parse(line).expect("should parse");
        assert_eq!(result.severity, Some(Severity::Info));
        assert_eq!(result.message, "GET /apache_pb.gif 200");
        assert_eq!(result.timestamp.as_deref(), Some("10/Oct/2000:13:55:36 -0700"));
        assert_eq!(
            result.fields.get("client_ip"),
            Some(&serde_json::Value::String("127.0.0.1".to_string()))
        );
        assert_eq!(
            result.fields.get("status"),
            Some(&serde_json::Value::Number(200.into()))
        );
        assert_eq!(
            result.fields.get("bytes"),
            Some(&serde_json::Value::Number(2326.into()))
        );
    }

    #[test]
    fn parse_combined_log_format() {
        let parser = ClfParser;
        let line = r#"127.0.0.1 - frank [10/Oct/2000:13:55:36 -0700] "GET /apache_pb.gif HTTP/1.0" 200 2326 "http://www.example.com/start.html" "Mozilla/4.08 [en] (Win98; I ;Nav)""#;
        let result = parser.parse(line).expect("should parse");
        assert_eq!(
            result.fields.get("referrer"),
            Some(&serde_json::Value::String(
                "http://www.example.com/start.html".to_string()
            ))
        );
        assert!(result.fields.contains_key("user_agent"));
    }

    #[test]
    fn status_severity_mapping() {
        assert_eq!(status_to_severity(200), Severity::Info);
        assert_eq!(status_to_severity(301), Severity::Info);
        assert_eq!(status_to_severity(404), Severity::Warn);
        assert_eq!(status_to_severity(500), Severity::Error);
    }

    #[test]
    fn reject_non_clf() {
        let parser = ClfParser;
        assert!(parser.parse("just a plain string").is_none());
        assert!(parser.parse("").is_none());
    }

    #[test]
    fn parse_common_fixtures() {
        let parser = ClfParser;
        let fixture = fixture_path("common.log");
        let content = std::fs::read_to_string(&fixture)
            .unwrap_or_else(|e| panic!("read fixture {}: {e}", fixture.display()));
        let mut parsed_count = 0;
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Some(result) = parser.parse(line) {
                assert!(!result.message.is_empty());
                assert!(result.severity.is_some());
                parsed_count += 1;
            }
        }
        assert!(parsed_count > 0, "should parse at least one line");
    }

    #[test]
    fn parse_combined_fixtures() {
        let parser = ClfParser;
        let fixture = fixture_path("combined.log");
        let content = std::fs::read_to_string(&fixture)
            .unwrap_or_else(|e| panic!("read fixture {}: {e}", fixture.display()));
        let mut parsed_count = 0;
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Some(result) = parser.parse(line) {
                assert!(result.fields.contains_key("user_agent"));
                parsed_count += 1;
            }
        }
        assert!(parsed_count > 0, "should parse at least one line");
    }
}
