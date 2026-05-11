// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

pub mod builtin;
pub mod custom;
pub mod severity;
pub mod timestamp;

use std::path::PathBuf;

use daemon8_types::Severity;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("unknown parser: {0}")]
    UnknownParser(String),
    #[error("failed to read custom parser at {path}: {source}")]
    ReadFailed {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid custom parser TOML at {path}: {source}")]
    InvalidToml {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("invalid regex in custom parser {name}: {source}")]
    InvalidRegex { name: String, source: regex::Error },
    #[error("custom parser {name} missing required field mapping: {field}")]
    MissingFieldMapping { name: String, field: String },
    #[error("grok parser requires a pattern (set parser_pattern in source config)")]
    GrokPatternRequired,
    #[error("invalid grok pattern: {0}")]
    InvalidGrokPattern(String),
}

#[derive(Debug, Clone)]
pub struct ParsedLine {
    pub timestamp: Option<String>,
    pub severity: Option<Severity>,
    pub channel: Option<String>,
    pub message: String,
    pub fields: serde_json::Map<String, serde_json::Value>,
}

pub trait Parser: Send + Sync {
    fn name(&self) -> &str;
    fn parse(&self, line: &str) -> Option<ParsedLine>;
}

pub fn resolve_parser(name: &str) -> Result<Box<dyn Parser>, ParseError> {
    resolve_parser_with_pattern(name, None)
}

pub fn resolve_parser_with_pattern(
    name: &str,
    pattern: Option<&str>,
) -> Result<Box<dyn Parser>, ParseError> {
    if name == "grok" {
        let pat = pattern.ok_or(ParseError::GrokPatternRequired)?;
        let parser =
            builtin::grok_parser::GrokParser::new(pat).map_err(ParseError::InvalidGrokPattern)?;
        return Ok(Box::new(parser));
    }

    if let Some(parser) = builtin::get(name) {
        return Ok(parser);
    }

    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("daemon8")
        .join("parsers");
    let path = config_dir.join(format!("{name}.toml"));

    if path.exists() {
        let parser = custom::CustomParser::from_file(&path)?;
        return Ok(Box::new(parser));
    }

    Err(ParseError::UnknownParser(name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_builtin_parsers() {
        for name in ["monolog", "json", "line", "syslog", "logfmt", "clf", "auto"] {
            let parser = resolve_parser(name);
            assert!(parser.is_ok(), "builtin parser '{name}' should resolve");
            assert_eq!(parser.unwrap().name(), name);
        }
    }

    #[test]
    fn resolve_unknown_parser_errors() {
        match resolve_parser("nonexistent-parser-xyz") {
            Err(ParseError::UnknownParser(name)) => assert_eq!(name, "nonexistent-parser-xyz"),
            Err(other) => panic!("expected UnknownParser, got: {other}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }
}
