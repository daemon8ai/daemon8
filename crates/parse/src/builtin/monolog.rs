// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::sync::LazyLock;

use daemon8_types::Severity;
use regex::Regex;

use crate::{ParsedLine, Parser};

static MONOLOG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\[(\d{4}-\d{2}-\d{2}\s+\d{2}:\d{2}:\d{2})\]\s+(\w+)\.(\w+):\s+(.*?)(\s+\{.*\})?\s*(\[.*\])?\s*$",
    )
    .expect("monolog regex is valid")
});

pub struct MonologParser;

impl Parser for MonologParser {
    fn name(&self) -> &str {
        "monolog"
    }

    fn parse(&self, line: &str) -> Option<ParsedLine> {
        let caps = MONOLOG_RE.captures(line)?;

        let timestamp = caps.get(1).map(|m| m.as_str().to_string());
        let channel = caps.get(2).map(|m| m.as_str().to_string());
        let severity = caps.get(3).map(|m| psr3_to_severity(m.as_str()));
        let message = caps.get(4).map_or("", |m| m.as_str()).to_string();

        let mut fields = serde_json::Map::new();

        if let Some(ctx) = caps.get(5) {
            let trimmed = ctx.as_str().trim();
            if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(trimmed) {
                fields = map;
            }
        }

        if let Some(extra) = caps.get(6) {
            let trimmed = extra.as_str().trim();
            if trimmed != "[]"
                && let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed)
            {
                fields.insert("extra".to_string(), val);
            }
        }

        Some(ParsedLine {
            timestamp,
            severity,
            channel,
            message,
            fields,
        })
    }
}

fn psr3_to_severity(level: &str) -> Severity {
    match level.to_ascii_uppercase().as_str() {
        "DEBUG" => Severity::Debug,
        "INFO" => Severity::Info,
        "NOTICE" => Severity::Info,
        "WARNING" => Severity::Warn,
        "ERROR" => Severity::Error,
        "CRITICAL" => Severity::Error,
        "ALERT" => Severity::Error,
        "EMERGENCY" => Severity::Error,
        _ => Severity::Info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("monolog")
            .join(name)
    }

    #[test]
    fn parse_basic_error() {
        let parser = MonologParser;
        let line =
            "[2024-01-15 14:32:01] app.ERROR: Something went wrong {\"context\":\"data\"} []";
        let result = parser.parse(line).expect("should parse");
        assert_eq!(result.timestamp.as_deref(), Some("2024-01-15 14:32:01"));
        assert_eq!(result.severity, Some(Severity::Error));
        assert_eq!(result.channel.as_deref(), Some("app"));
        assert_eq!(result.message, "Something went wrong");
        assert_eq!(
            result.fields.get("context"),
            Some(&serde_json::Value::String("data".to_string()))
        );
    }

    #[test]
    fn parse_all_severity_levels() {
        let parser = MonologParser;
        let cases = [
            ("DEBUG", Severity::Debug),
            ("INFO", Severity::Info),
            ("NOTICE", Severity::Info),
            ("WARNING", Severity::Warn),
            ("ERROR", Severity::Error),
            ("CRITICAL", Severity::Error),
            ("ALERT", Severity::Error),
            ("EMERGENCY", Severity::Error),
        ];
        for (level, expected) in cases {
            let line = format!("[2024-01-15 10:00:00] test.{level}: message []");
            let result = parser.parse(&line).expect("should parse");
            assert_eq!(result.severity, Some(expected), "level {level}");
        }
    }

    #[test]
    fn reject_non_monolog_lines() {
        let parser = MonologParser;
        assert!(parser.parse("just a plain string").is_none());
        assert!(parser.parse("").is_none());
        assert!(parser.parse("{\"json\": true}").is_none());
    }

    #[test]
    fn parse_fixtures() {
        let parser = MonologParser;
        let fixture = fixture_path("basic.log");
        let content = std::fs::read_to_string(&fixture)
            .unwrap_or_else(|e| panic!("read fixture {}: {e}", fixture.display()));
        let mut parsed_count = 0;
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Some(result) = parser.parse(line) {
                assert!(!result.message.is_empty());
                assert!(result.timestamp.is_some());
                assert!(result.severity.is_some());
                parsed_count += 1;
            }
        }
        assert!(
            parsed_count > 0,
            "should parse at least one line from fixture"
        );
    }

    #[test]
    fn parse_context_fixtures() {
        let parser = MonologParser;
        let fixture = fixture_path("with_context.log");
        let content = std::fs::read_to_string(&fixture)
            .unwrap_or_else(|e| panic!("read fixture {}: {e}", fixture.display()));
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let result = parser
                .parse(line)
                .expect("context fixture lines should parse");
            assert!(
                !result.fields.is_empty(),
                "context fixtures should have fields"
            );
        }
    }
}
