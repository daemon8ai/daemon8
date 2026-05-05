// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

pub mod connections;
pub mod query;
pub mod status;
pub mod tail;

pub use query::QueryArgs;
pub use tail::TailArgs;

use anyhow::Result;
use comfy_table::{Cell, ContentArrangement, Table, presets::UTF8_FULL_CONDENSED};
use daemon8_types::{ConnectionInfo, ConnectionKind, Origin, Severity};

#[derive(clap::Args, Clone, Debug)]
pub struct ClientArgs {
    #[arg(long)]
    pub port: Option<u16>,
    #[arg(long)]
    pub json: bool,
}

impl ClientArgs {
    pub fn resolved_port(&self) -> u16 {
        self.port.unwrap_or_else(|| {
            crate::config::load(None)
                .map(|c| c.server.port)
                .unwrap_or(8888)
        })
    }
}

pub fn base_url(port: u16) -> String {
    format!("http://localhost:{port}")
}

pub fn handle_reqwest_error(e: reqwest::Error, port: u16) -> anyhow::Error {
    if e.is_connect() {
        anyhow::anyhow!(
            "Cannot connect to daemon at localhost:{port}. Is it running? Start with: daemon8 serve"
        )
    } else {
        anyhow::anyhow!("HTTP request failed: {e}")
    }
}

pub async fn check_response(resp: reqwest::Response) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        Ok(resp)
    } else {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Server returned {status}: {body}");
    }
}

pub fn print_connections_table(connections: &[ConnectionInfo]) {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("ID"),
        Cell::new("Type"),
        Cell::new("Name"),
        Cell::new("Observations"),
    ]);

    for conn in connections {
        let kind_str = match conn.kind {
            ConnectionKind::Application => "application",
            ConnectionKind::Browser => "browser",
            ConnectionKind::Device => "device",
        };
        table.add_row(vec![
            Cell::new(&conn.id),
            Cell::new(kind_str),
            Cell::new(&conn.name),
            Cell::new(conn.observation_count),
        ]);
    }

    println!("{table}");
}

/// Convert epoch nanoseconds to "HH:MM:SS" (UTC).
pub fn format_timestamp(timestamp_ns: u64) -> String {
    let secs = (timestamp_ns / 1_000_000_000) as i64;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// Format severity with optional color.
pub fn format_severity(s: &Severity, use_color: bool) -> String {
    use owo_colors::OwoColorize;

    let label = s.to_string().to_uppercase();
    if !use_color {
        return label;
    }
    match s {
        Severity::Error => label.red().to_string(),
        Severity::Warn => label.yellow().to_string(),
        Severity::Info => label.green().to_string(),
        Severity::Debug => label.dimmed().to_string(),
        Severity::Trace => label.dimmed().to_string(),
    }
}

/// Format an Origin into a compact "type:name" string.
pub fn format_origin(o: &Origin) -> String {
    match o {
        Origin::Application { name } => format!("app:{name}"),
        Origin::Browser { url, .. } => format!("browser:{url}"),
        Origin::Device { serial, .. } => format!("device:{serial}"),
    }
}

/// Truncate a string, appending "..." if it exceeds `max` characters.
/// Safe for multi-byte UTF-8.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{truncated}...")
    }
}

/// Percent-encode a query parameter value.
pub fn urlenc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b',' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// Format a number with thousand separators (simple implementation).
pub fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use daemon8_types::DevicePlatform;

    #[test]
    fn format_timestamp_midnight() {
        assert_eq!(format_timestamp(0), "00:00:00");
    }

    #[test]
    fn format_timestamp_midday() {
        // 12:34:56 UTC = (12*3600 + 34*60 + 56) seconds = 45296 seconds
        let ns = 45_296_000_000_000u64;
        assert_eq!(format_timestamp(ns), "12:34:56");
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let long = "a".repeat(200);
        let result = truncate(&long, 50);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 50);
    }

    #[test]
    fn urlenc_preserves_unreserved() {
        assert_eq!(urlenc("hello-world_42"), "hello-world_42");
    }

    #[test]
    fn urlenc_encodes_special_chars() {
        assert_eq!(urlenc("a b"), "a%20b");
        assert_eq!(urlenc("foo=bar"), "foo%3Dbar");
    }

    #[test]
    fn format_number_thousands() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1_000), "1,000");
        assert_eq!(format_number(1_234_567), "1,234,567");
    }

    #[test]
    fn format_origin_variants() {
        let app = Origin::Application {
            name: "myapp".into(),
        };
        assert_eq!(format_origin(&app), "app:myapp");

        let browser = Origin::Browser {
            tab_id: "tab1".into(),
            url: "https://example.com".into(),
        };
        assert_eq!(format_origin(&browser), "browser:https://example.com");

        let device = Origin::Device {
            serial: "ABC123".into(),
            platform: DevicePlatform::default(),
        };
        assert_eq!(format_origin(&device), "device:ABC123");
    }
}
