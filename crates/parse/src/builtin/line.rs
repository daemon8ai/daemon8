// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use daemon8_types::Severity;

use crate::severity::sniff_severity;
use crate::{ParsedLine, Parser};

pub struct LineParser;

impl Parser for LineParser {
    fn name(&self) -> &str {
        "line"
    }

    fn parse(&self, line: &str) -> Option<ParsedLine> {
        let severity = sniff_severity(line).unwrap_or(Severity::Info);
        Some(ParsedLine {
            timestamp: None,
            severity: Some(severity),
            channel: None,
            message: line.to_string(),
            fields: serde_json::Map::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("line")
            .join(name)
    }

    #[test]
    fn always_returns_some() {
        let parser = LineParser;
        assert!(parser.parse("").is_some());
        assert!(parser.parse("anything goes").is_some());
        assert!(parser.parse("{\"json\": true}").is_some());
    }

    #[test]
    fn preserves_full_line() {
        let parser = LineParser;
        let input = "  leading whitespace and trailing  ";
        let result = parser.parse(input).unwrap();
        assert_eq!(result.message, input);
    }

    #[test]
    fn severity_defaults_to_info() {
        let parser = LineParser;
        let result = parser.parse("test").unwrap();
        assert_eq!(result.severity, Some(Severity::Info));
    }

    #[test]
    fn severity_sniffed_from_text() {
        let parser = LineParser;
        assert_eq!(
            parser.parse("ERROR: something broke").unwrap().severity,
            Some(Severity::Error)
        );
        assert_eq!(
            parser
                .parse("WARNING: slow API endpoint called")
                .unwrap()
                .severity,
            Some(Severity::Warn)
        );
        assert_eq!(
            parser.parse("[DEBUG] checking cache hit").unwrap().severity,
            Some(Severity::Debug)
        );
        assert_eq!(
            parser.parse("FATAL out of memory").unwrap().severity,
            Some(Severity::Error)
        );
    }

    #[test]
    fn no_extra_fields() {
        let parser = LineParser;
        let result = parser.parse("test").unwrap();
        assert!(result.fields.is_empty());
        assert!(result.timestamp.is_none());
        assert!(result.channel.is_none());
    }

    #[test]
    fn parse_fixtures() {
        let parser = LineParser;
        let fixture = fixture_path("mixed.log");
        let content = std::fs::read_to_string(&fixture)
            .unwrap_or_else(|e| panic!("read fixture {}: {e}", fixture.display()));
        let mut parsed_count = 0;
        for line in content.lines() {
            let result = parser.parse(line);
            assert!(result.is_some(), "line parser should parse every line");
            parsed_count += 1;
        }
        assert!(parsed_count > 0, "fixture should have lines");
    }
}
