// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use crate::{ParsedLine, Parser};

use super::clf::ClfParser;
use super::json::JsonParser;
use super::line::LineParser;
use super::logfmt::LogfmtParser;
use super::monolog::MonologParser;
use super::syslog::SyslogParser;

pub struct AutoParser {
    json: JsonParser,
    syslog: SyslogParser,
    monolog: MonologParser,
    logfmt: LogfmtParser,
    clf: ClfParser,
    line: LineParser,
}

impl Default for AutoParser {
    fn default() -> Self {
        Self {
            json: JsonParser,
            syslog: SyslogParser,
            monolog: MonologParser,
            logfmt: LogfmtParser,
            clf: ClfParser,
            line: LineParser,
        }
    }
}

impl Parser for AutoParser {
    fn name(&self) -> &str {
        "auto"
    }

    fn parse(&self, line: &str) -> Option<ParsedLine> {
        let trimmed = line.trim();

        if trimmed.starts_with('{')
            && let Some(parsed) = self.json.parse(line)
        {
            return Some(parsed);
        }

        if trimmed.starts_with('<')
            && trimmed
                .as_bytes()
                .get(1)
                .is_some_and(|b| b.is_ascii_digit())
            && let Some(parsed) = self.syslog.parse(line)
        {
            return Some(parsed);
        }

        if trimmed.starts_with('[')
            && let Some(parsed) = self.monolog.parse(line)
        {
            return Some(parsed);
        }

        if looks_like_clf(trimmed)
            && let Some(parsed) = self.clf.parse(line)
        {
            return Some(parsed);
        }

        if looks_like_logfmt(trimmed)
            && let Some(parsed) = self.logfmt.parse(line)
        {
            return Some(parsed);
        }

        self.line.parse(line)
    }
}

fn looks_like_clf(s: &str) -> bool {
    s.contains("] \"") && s.contains("HTTP/")
}

fn looks_like_logfmt(s: &str) -> bool {
    let eq_count = s.chars().filter(|&c| c == '=').count();
    eq_count >= 2 && s.contains(' ')
}

#[cfg(test)]
mod tests {
    use daemon8_types::Severity;

    use super::*;

    #[test]
    fn detect_json() {
        let parser = AutoParser::default();
        let line = r#"{"timestamp":"2024-01-15T14:32:01Z","level":"error","msg":"boom"}"#;
        let result = parser.parse(line).unwrap();
        assert_eq!(result.severity, Some(Severity::Error));
        assert_eq!(result.message, "boom");
    }

    #[test]
    fn detect_syslog() {
        let parser = AutoParser::default();
        let line = "<34>Oct 11 22:14:15 myhost su[12345]: su root failed";
        let result = parser.parse(line).unwrap();
        assert_eq!(result.severity, Some(Severity::Error));
        assert!(result.fields.contains_key("hostname"));
    }

    #[test]
    fn detect_monolog() {
        let parser = AutoParser::default();
        let line = "[2024-01-15 10:00:00] app.ERROR: Something broke {} []";
        let result = parser.parse(line).unwrap();
        assert_eq!(result.severity, Some(Severity::Error));
        assert_eq!(result.channel.as_deref(), Some("app"));
    }

    #[test]
    fn detect_logfmt() {
        let parser = AutoParser::default();
        let line = "ts=2024-01-15T14:32:01Z level=warn msg=\"memory pressure\" used_mb=3800";
        let result = parser.parse(line).unwrap();
        assert_eq!(result.severity, Some(Severity::Warn));
    }

    #[test]
    fn detect_clf() {
        let parser = AutoParser::default();
        let line =
            r#"127.0.0.1 - frank [10/Oct/2000:13:55:36 -0700] "GET /page HTTP/1.0" 200 2326"#;
        let result = parser.parse(line).unwrap();
        assert_eq!(result.severity, Some(Severity::Info));
        assert!(result.fields.contains_key("client_ip"));
    }

    #[test]
    fn fallback_to_line() {
        let parser = AutoParser::default();
        let line = "just a plain log line";
        let result = parser.parse(line).unwrap();
        assert_eq!(result.message, line);
        assert_eq!(result.severity, Some(Severity::Info));
    }

    #[test]
    fn fallback_with_severity_sniffing() {
        let parser = AutoParser::default();
        let line = "ERROR: something terrible happened";
        let result = parser.parse(line).unwrap();
        assert_eq!(result.severity, Some(Severity::Error));
    }
}
