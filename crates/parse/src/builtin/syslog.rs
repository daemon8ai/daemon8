// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::sync::LazyLock;

use daemon8_types::Severity;
use regex::Regex;

use crate::{ParsedLine, Parser};

// RFC 3164: <PRI>TIMESTAMP HOSTNAME APP[PID]: MSG
static RFC3164_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^<(\d{1,3})>(\w{3}\s+\d{1,2}\s+\d{2}:\d{2}:\d{2})\s+(\S+)\s+(\S+?)(?:\[(\d+)\])?:\s*(.*)",
    )
    .expect("rfc3164 regex is valid")
});

// RFC 5424: <PRI>VERSION TIMESTAMP HOSTNAME APP-NAME PROCID MSGID SD MSG
static RFC5424_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^<(\d{1,3})>(\d+)\s+(\S+)\s+(\S+)\s+(\S+)\s+(\S+)\s+(\S+)\s+(?:\[.*?\]|-)\s*(.*)",
    )
    .expect("rfc5424 regex is valid")
});

pub struct SyslogParser;

impl Parser for SyslogParser {
    fn name(&self) -> &str {
        "syslog"
    }

    fn parse(&self, line: &str) -> Option<ParsedLine> {
        self.parse_rfc5424(line).or_else(|| self.parse_rfc3164(line))
    }
}

impl SyslogParser {
    fn parse_rfc3164(&self, line: &str) -> Option<ParsedLine> {
        let caps = RFC3164_RE.captures(line)?;

        let pri: u8 = caps.get(1)?.as_str().parse().ok()?;
        let severity = pri_to_severity(pri);
        let facility = pri / 8;

        let timestamp = caps.get(2).map(|m| m.as_str().to_string());
        let hostname = caps.get(3).map(|m| m.as_str().to_string());
        let app = caps.get(4).map(|m| m.as_str().to_string());
        let pid = caps.get(5).map(|m| m.as_str().to_string());
        let message = caps.get(6).map_or("", |m| m.as_str()).to_string();

        let mut fields = serde_json::Map::new();
        if let Some(h) = hostname {
            fields.insert("hostname".to_string(), serde_json::Value::String(h));
        }
        if let Some(a) = &app {
            fields.insert("app".to_string(), serde_json::Value::String(a.clone()));
        }
        if let Some(p) = pid {
            fields.insert("pid".to_string(), serde_json::Value::String(p));
        }
        fields.insert(
            "facility".to_string(),
            serde_json::Value::Number(facility.into()),
        );

        Some(ParsedLine {
            timestamp,
            severity: Some(severity),
            channel: app,
            message,
            fields,
        })
    }

    fn parse_rfc5424(&self, line: &str) -> Option<ParsedLine> {
        let caps = RFC5424_RE.captures(line)?;

        let pri: u8 = caps.get(1)?.as_str().parse().ok()?;
        let severity = pri_to_severity(pri);
        let facility = pri / 8;

        let _version = caps.get(2).map(|m| m.as_str());
        let timestamp = caps.get(3).map(|m| m.as_str().to_string());
        let hostname = caps.get(4).map(|m| m.as_str().to_string());
        let app = caps.get(5).map(|m| m.as_str().to_string());
        let procid = caps.get(6).map(|m| m.as_str().to_string());
        let msgid = caps.get(7).map(|m| m.as_str().to_string());
        let message = caps.get(8).map_or("", |m| m.as_str()).to_string();

        let mut fields = serde_json::Map::new();
        if let Some(h) = hostname {
            fields.insert("hostname".to_string(), serde_json::Value::String(h));
        }
        if let Some(a) = &app {
            fields.insert("app".to_string(), serde_json::Value::String(a.clone()));
        }
        if let Some(p) = procid
            && p != "-"
        {
            fields.insert("procid".to_string(), serde_json::Value::String(p));
        }
        if let Some(m) = msgid
            && m != "-"
        {
            fields.insert("msgid".to_string(), serde_json::Value::String(m));
        }
        fields.insert(
            "facility".to_string(),
            serde_json::Value::Number(facility.into()),
        );

        let channel = app.as_deref().filter(|a| *a != "-").map(String::from);

        Some(ParsedLine {
            timestamp,
            severity: Some(severity),
            channel,
            message,
            fields,
        })
    }
}

fn pri_to_severity(pri: u8) -> Severity {
    match pri % 8 {
        0..=3 => Severity::Error,
        4 => Severity::Warn,
        5 => Severity::Info,
        6 => Severity::Info,
        7 => Severity::Debug,
        _ => Severity::Info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("syslog")
            .join(name)
    }

    #[test]
    fn parse_rfc3164() {
        let parser = SyslogParser;
        let line = "<34>Oct 11 22:14:15 mymachine su[12345]: 'su root' failed for lonvick on /dev/pts/8";
        let result = parser.parse(line).expect("should parse");
        assert_eq!(result.severity, Some(Severity::Error));
        assert_eq!(result.timestamp.as_deref(), Some("Oct 11 22:14:15"));
        assert_eq!(
            result.fields.get("hostname"),
            Some(&serde_json::Value::String("mymachine".to_string()))
        );
        assert_eq!(
            result.fields.get("pid"),
            Some(&serde_json::Value::String("12345".to_string()))
        );
        assert!(result.message.contains("su root"));
    }

    #[test]
    fn parse_rfc5424() {
        let parser = SyslogParser;
        let line = "<165>1 2024-01-15T14:32:01.123Z host app 1234 ID47 - Application started";
        let result = parser.parse(line).expect("should parse");
        assert_eq!(result.severity, Some(Severity::Info));
        assert_eq!(
            result.timestamp.as_deref(),
            Some("2024-01-15T14:32:01.123Z")
        );
        assert_eq!(result.message, "Application started");
    }

    #[test]
    fn priority_decoding() {
        assert_eq!(pri_to_severity(0) as u8, Severity::Error as u8);
        assert_eq!(pri_to_severity(4) as u8, Severity::Warn as u8);
        assert_eq!(pri_to_severity(6) as u8, Severity::Info as u8);
        assert_eq!(pri_to_severity(7) as u8, Severity::Debug as u8);
    }

    #[test]
    fn reject_non_syslog() {
        let parser = SyslogParser;
        assert!(parser.parse("just a plain string").is_none());
        assert!(parser.parse("").is_none());
    }

    #[test]
    fn parse_rfc3164_fixtures() {
        let parser = SyslogParser;
        let fixture = fixture_path("rfc3164.log");
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
    fn parse_rfc5424_fixtures() {
        let parser = SyslogParser;
        let fixture = fixture_path("rfc5424.log");
        let content = std::fs::read_to_string(&fixture)
            .unwrap_or_else(|e| panic!("read fixture {}: {e}", fixture.display()));
        let mut parsed_count = 0;
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Some(result) = parser.parse(line) {
                assert!(result.severity.is_some());
                parsed_count += 1;
            }
        }
        assert!(parsed_count > 0, "should parse at least one line");
    }
}
