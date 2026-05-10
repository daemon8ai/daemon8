// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use daemon8_types::Severity;
use syslog_loose::{ProcId, SyslogSeverity, Variant, parse_message};

use crate::{ParsedLine, Parser};

pub struct SyslogParser;

impl Parser for SyslogParser {
    fn name(&self) -> &str {
        "syslog"
    }

    fn parse(&self, line: &str) -> Option<ParsedLine> {
        let msg = parse_message(line, Variant::Either);

        if msg.facility.is_none() && msg.severity.is_none() && msg.appname.is_none() {
            return None;
        }

        let severity = msg
            .severity
            .map(loose_to_severity)
            .unwrap_or(Severity::Info);

        let timestamp = msg.timestamp.map(|ts| ts.to_rfc3339());

        let mut fields = serde_json::Map::new();
        if let Some(host) = msg.hostname {
            fields.insert(
                "hostname".into(),
                serde_json::Value::String(host.to_string()),
            );
        }
        if let Some(app) = msg.appname {
            fields.insert("app".into(), serde_json::Value::String(app.to_string()));
        }
        if let Some(pid) = msg.procid {
            let pid_str = match pid {
                ProcId::PID(n) => n.to_string(),
                ProcId::Name(s) => s.to_string(),
            };
            fields.insert("pid".into(), serde_json::Value::String(pid_str));
        }
        if let Some(fac) = msg.facility {
            fields.insert(
                "facility".into(),
                serde_json::Value::Number((fac as u8).into()),
            );
        }
        if let Some(mid) = msg.msgid
            && mid != "-"
        {
            fields.insert("msgid".into(), serde_json::Value::String(mid.to_string()));
        }

        let channel = msg.appname.map(|a| a.to_string()).filter(|a| a != "-");

        Some(ParsedLine {
            timestamp,
            severity: Some(severity),
            channel,
            message: msg.msg.to_string(),
            fields,
        })
    }
}

fn loose_to_severity(s: SyslogSeverity) -> Severity {
    match s {
        SyslogSeverity::SEV_EMERG
        | SyslogSeverity::SEV_ALERT
        | SyslogSeverity::SEV_CRIT
        | SyslogSeverity::SEV_ERR => Severity::Error,
        SyslogSeverity::SEV_WARNING => Severity::Warn,
        SyslogSeverity::SEV_NOTICE | SyslogSeverity::SEV_INFO => Severity::Info,
        SyslogSeverity::SEV_DEBUG => Severity::Debug,
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
        let line =
            "<34>Oct 11 22:14:15 mymachine su[12345]: 'su root' failed for lonvick on /dev/pts/8";
        let result = parser.parse(line).expect("should parse");
        assert_eq!(result.severity, Some(Severity::Error));
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
        assert_eq!(result.message, "Application started");
    }

    #[test]
    fn priority_decoding() {
        assert_eq!(
            loose_to_severity(SyslogSeverity::SEV_EMERG),
            Severity::Error
        );
        assert_eq!(
            loose_to_severity(SyslogSeverity::SEV_WARNING),
            Severity::Warn
        );
        assert_eq!(loose_to_severity(SyslogSeverity::SEV_INFO), Severity::Info);
        assert_eq!(
            loose_to_severity(SyslogSeverity::SEV_DEBUG),
            Severity::Debug
        );
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
