// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::HashMap;
use std::path::Path;

use daemon8_types::Severity;
use regex::Regex;
use serde::Deserialize;

use crate::{ParseError, ParsedLine, Parser};

#[derive(Deserialize)]
struct CustomParserToml {
    parser: ParserMeta,
    pattern: PatternConfig,
    fields: FieldMapping,
    #[serde(default)]
    severity_map: HashMap<String, String>,
}

#[derive(Deserialize)]
struct ParserMeta {
    name: String,
    #[allow(dead_code)]
    version: Option<String>,
}

#[derive(Deserialize)]
struct PatternConfig {
    regex: String,
}

#[derive(Deserialize)]
struct FieldMapping {
    message: String,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    channel: Option<String>,
}

pub struct CustomParser {
    name: String,
    regex: Regex,
    fields: FieldMapping,
    severity_map: HashMap<String, Severity>,
}

impl CustomParser {
    pub fn from_file(path: &Path) -> Result<Self, ParseError> {
        let content = std::fs::read_to_string(path).map_err(|e| ParseError::ReadFailed {
            path: path.to_path_buf(),
            source: e,
        })?;

        let config: CustomParserToml =
            toml::from_str(&content).map_err(|e| ParseError::InvalidToml {
                path: path.to_path_buf(),
                source: e,
            })?;

        let regex =
            Regex::new(&config.pattern.regex).map_err(|e| ParseError::InvalidRegex {
                name: config.parser.name.clone(),
                source: e,
            })?;

        if !regex
            .capture_names()
            .flatten()
            .any(|n| n == config.fields.message)
        {
            return Err(ParseError::MissingFieldMapping {
                name: config.parser.name,
                field: config.fields.message,
            });
        }

        let severity_map = config
            .severity_map
            .into_iter()
            .filter_map(|(k, v)| {
                let sev = match v.to_ascii_lowercase().as_str() {
                    "trace" => Severity::Trace,
                    "debug" => Severity::Debug,
                    "info" => Severity::Info,
                    "warn" | "warning" => Severity::Warn,
                    "error" => Severity::Error,
                    _ => return None,
                };
                Some((k, sev))
            })
            .collect();

        Ok(Self {
            name: config.parser.name,
            regex,
            fields: config.fields,
            severity_map,
        })
    }
}

impl Parser for CustomParser {
    fn name(&self) -> &str {
        &self.name
    }

    fn parse(&self, line: &str) -> Option<ParsedLine> {
        let caps = self.regex.captures(line)?;

        let message = caps
            .name(&self.fields.message)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();

        let timestamp = self
            .fields
            .timestamp
            .as_ref()
            .and_then(|name| caps.name(name))
            .map(|m| m.as_str().to_string());

        let severity = self
            .fields
            .severity
            .as_ref()
            .and_then(|name| caps.name(name))
            .and_then(|m| {
                let raw = m.as_str();
                self.severity_map
                    .get(raw)
                    .copied()
                    .or_else(|| raw.parse::<Severity>().ok())
            });

        let channel = self
            .fields
            .channel
            .as_ref()
            .and_then(|name| caps.name(name))
            .map(|m| m.as_str().to_string());

        let mut fields = serde_json::Map::new();
        let reserved: &[Option<&str>] = &[
            Some(self.fields.message.as_str()),
            self.fields.timestamp.as_deref(),
            self.fields.severity.as_deref(),
            self.fields.channel.as_deref(),
        ];
        for name in self.regex.capture_names().flatten() {
            if reserved.iter().any(|r| r == &Some(name)) {
                continue;
            }
            if let Some(m) = caps.name(name) {
                fields.insert(
                    name.to_string(),
                    serde_json::Value::String(m.as_str().to_string()),
                );
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_toml(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(format!("{name}.toml"));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn load_and_parse_custom() {
        let dir = tempfile::tempdir().unwrap();
        let toml = r#"
[parser]
name = "my-format"
version = "1.0"

[pattern]
regex = '^\[(?P<timestamp>[^\]]+)\] (?P<severity>\w+) (?P<message>.*)'

[fields]
timestamp = "timestamp"
severity = "severity"
message = "message"

[severity_map]
FATAL = "error"
WARNING = "warn"
"#;
        let path = write_toml(dir.path(), "my-format", toml);
        let parser = CustomParser::from_file(&path).expect("should load");

        assert_eq!(parser.name(), "my-format");

        let result = parser
            .parse("[2024-01-15 10:00:00] WARNING disk almost full")
            .expect("should parse");
        assert_eq!(result.timestamp.as_deref(), Some("2024-01-15 10:00:00"));
        assert_eq!(result.severity, Some(Severity::Warn));
        assert_eq!(result.message, "disk almost full");
    }

    #[test]
    fn custom_parser_fatal_maps_to_error() {
        let dir = tempfile::tempdir().unwrap();
        let toml = r#"
[parser]
name = "test"

[pattern]
regex = '(?P<severity>\w+): (?P<message>.*)'

[fields]
message = "message"
severity = "severity"

[severity_map]
FATAL = "error"
"#;
        let path = write_toml(dir.path(), "test", toml);
        let parser = CustomParser::from_file(&path).unwrap();
        let result = parser.parse("FATAL: everything is on fire").unwrap();
        assert_eq!(result.severity, Some(Severity::Error));
    }

    #[test]
    fn invalid_regex_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let toml = r#"
[parser]
name = "bad"

[pattern]
regex = '(?P<message>[unclosed'

[fields]
message = "message"
"#;
        let path = write_toml(dir.path(), "bad", toml);
        let result = CustomParser::from_file(&path);
        assert!(matches!(result, Err(ParseError::InvalidRegex { .. })));
    }

    #[test]
    fn missing_message_field_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let toml = r#"
[parser]
name = "bad"

[pattern]
regex = '(?P<text>.*)'

[fields]
message = "msg"
"#;
        let path = write_toml(dir.path(), "bad", toml);
        let result = CustomParser::from_file(&path);
        assert!(matches!(result, Err(ParseError::MissingFieldMapping { .. })));
    }

    #[test]
    fn extra_named_groups_become_fields() {
        let dir = tempfile::tempdir().unwrap();
        let toml = r#"
[parser]
name = "extra"

[pattern]
regex = '(?P<host>\S+) (?P<message>.*)'

[fields]
message = "message"
"#;
        let path = write_toml(dir.path(), "extra", toml);
        let parser = CustomParser::from_file(&path).unwrap();
        let result = parser.parse("db-01 connection timeout").unwrap();
        assert_eq!(result.message, "connection timeout");
        assert_eq!(
            result.fields.get("host"),
            Some(&serde_json::Value::String("db-01".to_string()))
        );
    }
}
