// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use grok::Grok;

use crate::severity::text_to_severity;
use crate::{ParsedLine, Parser};

pub struct GrokParser {
    pattern: grok::Pattern,
}

impl GrokParser {
    pub fn new(pattern_str: &str) -> Result<Self, String> {
        let grok = Grok::default();
        let pattern = grok
            .compile(pattern_str, true)
            .map_err(|e| format!("invalid grok pattern: {e}"))?;
        Ok(Self { pattern })
    }
}

impl Parser for GrokParser {
    fn name(&self) -> &str {
        "grok"
    }

    fn parse(&self, line: &str) -> Option<ParsedLine> {
        let matches = self.pattern.match_against(line)?;

        let mut fields = serde_json::Map::new();
        let mut severity = None;
        let mut message = None;
        let mut timestamp = None;
        let mut channel = None;

        for (key, value) in matches.iter() {
            match key {
                "level" | "loglevel" | "LOGLEVEL" | "severity" => {
                    severity = text_to_severity(value);
                    if severity.is_none() {
                        fields.insert(
                            key.to_string(),
                            serde_json::Value::String(value.to_string()),
                        );
                    }
                }
                "msg" | "message" | "MESSAGE" => message = Some(value.to_string()),
                "timestamp" | "ts" | "time" | "TIMESTAMP_ISO8601" => {
                    timestamp = Some(value.to_string())
                }
                "channel" | "logger" => channel = Some(value.to_string()),
                _ => {
                    fields.insert(
                        key.to_string(),
                        serde_json::Value::String(value.to_string()),
                    );
                }
            }
        }

        Some(ParsedLine {
            timestamp,
            severity,
            channel,
            message: message.unwrap_or_else(|| line.to_string()),
            fields,
        })
    }
}

#[cfg(test)]
mod tests {
    use daemon8_types::Severity;

    use super::*;

    #[test]
    fn parse_with_loglevel() {
        let parser = GrokParser::new("%{LOGLEVEL:level} %{GREEDYDATA:message}").unwrap();
        let result = parser.parse("ERROR something broke").unwrap();
        assert_eq!(result.severity, Some(Severity::Error));
        assert_eq!(result.message, "something broke");
    }

    #[test]
    fn parse_with_timestamp() {
        let parser = GrokParser::new(
            "%{TIMESTAMP_ISO8601:timestamp} %{LOGLEVEL:level} %{GREEDYDATA:message}",
        )
        .unwrap();
        let result = parser
            .parse("2024-01-15T14:32:01.000Z ERROR connection refused")
            .unwrap();
        assert_eq!(result.severity, Some(Severity::Error));
        assert_eq!(result.message, "connection refused");
        assert!(result.timestamp.is_some());
    }

    #[test]
    fn parse_custom_fields() {
        let parser =
            GrokParser::new("%{IP:client_ip} %{WORD:method} %{URIPATHPARAM:path}").unwrap();
        let result = parser.parse("192.168.1.1 GET /api/users").unwrap();
        assert_eq!(
            result.fields.get("client_ip"),
            Some(&serde_json::Value::String("192.168.1.1".to_string()))
        );
        assert_eq!(
            result.fields.get("method"),
            Some(&serde_json::Value::String("GET".to_string()))
        );
    }

    #[test]
    fn no_match_returns_none() {
        let parser = GrokParser::new("%{IP:ip}").unwrap();
        assert!(parser.parse("no ip here").is_none());
    }

    #[test]
    fn invalid_pattern_errors() {
        let result = GrokParser::new("%{NONEXISTENT_PATTERN:field}");
        assert!(result.is_err());
    }
}
