// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use daemon8_types::Severity;

use crate::{ParsedLine, Parser};

pub struct LogfmtParser;

impl Parser for LogfmtParser {
    fn name(&self) -> &str {
        "logfmt"
    }

    fn parse(&self, line: &str) -> Option<ParsedLine> {
        let pairs = parse_logfmt(line);
        if pairs.is_empty() {
            return None;
        }

        let mut fields = serde_json::Map::new();
        let mut severity = None;
        let mut message = None;
        let mut timestamp = None;

        for (key, value) in pairs {
            match key.as_str() {
                "level" | "lvl" => severity = text_to_severity(&value),
                "msg" | "message" => message = Some(value),
                "ts" | "time" | "timestamp" => timestamp = Some(value),
                _ => {
                    fields.insert(key, serde_json::Value::String(value));
                }
            }
        }

        Some(ParsedLine {
            timestamp,
            severity,
            channel: None,
            message: message.unwrap_or_default(),
            fields,
        })
    }
}

fn parse_logfmt(line: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut chars = line.chars().peekable();

    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }

        if chars.peek().is_none() {
            break;
        }

        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' || c.is_whitespace() {
                break;
            }
            key.push(c);
            chars.next();
        }

        if key.is_empty() {
            break;
        }

        if chars.peek() != Some(&'=') {
            pairs.push((key, String::new()));
            continue;
        }
        chars.next();

        let value = if chars.peek() == Some(&'"') {
            chars.next();
            let mut val = String::new();
            let mut escaped = false;
            for c in chars.by_ref() {
                if escaped {
                    val.push(c);
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    break;
                } else {
                    val.push(c);
                }
            }
            val
        } else {
            let mut val = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                val.push(c);
                chars.next();
            }
            val
        };

        pairs.push((key, value));
    }

    pairs
}

fn text_to_severity(s: &str) -> Option<Severity> {
    match s.to_ascii_lowercase().as_str() {
        "trace" => Some(Severity::Trace),
        "debug" | "dbg" => Some(Severity::Debug),
        "info" | "information" => Some(Severity::Info),
        "warn" | "warning" => Some(Severity::Warn),
        "error" | "err" | "fatal" | "critical" | "panic" => Some(Severity::Error),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("logfmt")
            .join(name)
    }

    #[test]
    fn parse_basic_logfmt() {
        let parser = LogfmtParser;
        let line =
            r#"ts=2024-01-15T14:32:01Z level=error msg="connection refused" host=db-01 port=5432"#;
        let result = parser.parse(line).expect("should parse");
        assert_eq!(result.severity, Some(Severity::Error));
        assert_eq!(result.message, "connection refused");
        assert_eq!(result.timestamp.as_deref(), Some("2024-01-15T14:32:01Z"));
        assert_eq!(
            result.fields.get("host"),
            Some(&serde_json::Value::String("db-01".to_string()))
        );
        assert_eq!(
            result.fields.get("port"),
            Some(&serde_json::Value::String("5432".to_string()))
        );
    }

    #[test]
    fn parse_unquoted_values() {
        let parser = LogfmtParser;
        let line = "level=info msg=started service=api version=1.2.3";
        let result = parser.parse(line).expect("should parse");
        assert_eq!(result.severity, Some(Severity::Info));
        assert_eq!(result.message, "started");
    }

    #[test]
    fn parse_quoted_values_with_spaces() {
        let parser = LogfmtParser;
        let line = r#"level=warn msg="disk space low on /var/data" remaining=5%"#;
        let result = parser.parse(line).expect("should parse");
        assert_eq!(result.severity, Some(Severity::Warn));
        assert_eq!(result.message, "disk space low on /var/data");
    }

    #[test]
    fn empty_line_returns_none() {
        let parser = LogfmtParser;
        assert!(parser.parse("").is_none());
        assert!(parser.parse("   ").is_none());
    }

    #[test]
    fn parse_fixtures() {
        let parser = LogfmtParser;
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
                parsed_count += 1;
            }
        }
        assert!(parsed_count > 0, "should parse at least one line");
    }

    #[test]
    fn parse_escaped_quotes() {
        let parser = LogfmtParser;
        let line = r#"level=info msg="said \"hello\" to server" service=api"#;
        let result = parser.parse(line).expect("should parse");
        assert!(result.message.contains("hello"));
    }
}
