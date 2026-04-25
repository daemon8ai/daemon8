// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use super::{LogParser, LogSeverity, ParsedLine};

/// Parses Android `logcat -v threadtime` output.
///
/// Format:
///   MM-DD HH:MM:SS.mmm  PID  TID PRIORITY TAG: message
///
/// Example:
///   03-27 15:30:45.123  1234  5678 I ActivityManager: Start proc com.example
///   03-27 15:30:45.456  1234  5678 W System.err: java.lang.NullPointerException
pub struct LogcatParser;

impl LogParser for LogcatParser {
    fn parse_line(&self, line: &str) -> Option<ParsedLine> {
        let line = line.trim();
        if line.is_empty() || line.starts_with("-----") {
            return None;
        }

        // Split into whitespace-delimited tokens
        let mut tokens = line.splitn(6, char::is_whitespace);

        let date = tokens.next()?; // MM-DD
        let time = tokens.next()?; // HH:MM:SS.mmm

        // Validate date shape (MM-DD)
        if date.len() < 5 || date.as_bytes().get(2) != Some(&b'-') {
            return None;
        }

        // Skip whitespace -- PID and TID may have variable spacing
        let rest = skip_token(line, 2)?;
        let (pid_str, rest) = next_nonws_token(rest)?;
        let (tid_str, rest) = next_nonws_token(rest)?;
        let (priority, rest) = next_nonws_token(rest)?;

        let pid: u32 = pid_str.parse().ok()?;
        let _tid: u32 = tid_str.parse().ok()?;

        let severity = match priority {
            "V" => LogSeverity::Trace,
            "D" => LogSeverity::Debug,
            "I" => LogSeverity::Info,
            "W" => LogSeverity::Warn,
            "E" | "F" | "A" => LogSeverity::Error,
            _ => LogSeverity::Info,
        };

        // TAG: message -- tag ends at first ':'
        let rest = rest.trim_start();
        let (tag, message) = if let Some(colon) = rest.find(':') {
            let tag = rest[..colon].trim();
            let msg = rest[colon + 1..].trim_start();
            (tag.to_string(), msg.to_string())
        } else {
            (String::new(), rest.to_string())
        };

        Some(ParsedLine {
            timestamp: format!("{date} {time}"),
            severity,
            tag,
            pid: Some(pid),
            message,
            hostname: None,
            facility: None,
        })
    }
}

fn skip_token(s: &str, n: usize) -> Option<&str> {
    let mut rest = s;
    for _ in 0..n {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace)?;
        rest = &rest[end..];
    }
    Some(rest.trim_start())
}

fn next_nonws_token(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    Some((&s[..end], &s[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_threadtime() {
        let parser = LogcatParser;
        let line = "03-27 15:30:45.123  1234  5678 I ActivityManager: Start proc com.example";
        let parsed = parser.parse_line(line).unwrap();

        assert_eq!(parsed.timestamp, "03-27 15:30:45.123");
        assert_eq!(parsed.severity, LogSeverity::Info);
        assert_eq!(parsed.tag, "ActivityManager");
        assert_eq!(parsed.pid, Some(1234));
        assert_eq!(parsed.message, "Start proc com.example");
    }

    #[test]
    fn error_priority() {
        let parser = LogcatParser;
        let line = "03-27 15:30:45.456  1234  5678 E System.err: java.lang.NullPointerException";
        let parsed = parser.parse_line(line).unwrap();

        assert_eq!(parsed.severity, LogSeverity::Error);
        assert_eq!(parsed.tag, "System.err");
        assert!(parsed.message.contains("NullPointerException"));
    }

    #[test]
    fn warning_priority() {
        let parser = LogcatParser;
        let line =
            "03-27 15:30:45.789   999  1000 W ResourceType: No known package when getting value";
        let parsed = parser.parse_line(line).unwrap();

        assert_eq!(parsed.severity, LogSeverity::Warn);
        assert_eq!(parsed.tag, "ResourceType");
    }

    #[test]
    fn debug_priority() {
        let parser = LogcatParser;
        let line = "03-27 15:30:46.000 12345 12345 D dalvikvm: GC_CONCURRENT freed 1024K";
        let parsed = parser.parse_line(line).unwrap();

        assert_eq!(parsed.severity, LogSeverity::Debug);
        assert_eq!(parsed.tag, "dalvikvm");
        assert_eq!(parsed.pid, Some(12345));
    }

    #[test]
    fn separator_line_returns_none() {
        let parser = LogcatParser;
        assert!(parser.parse_line("--------- beginning of main").is_none());
    }

    #[test]
    fn empty_returns_none() {
        let parser = LogcatParser;
        assert!(parser.parse_line("").is_none());
        assert!(parser.parse_line("   ").is_none());
    }

    #[test]
    fn fatal_maps_to_error() {
        let parser = LogcatParser;
        let line = "03-27 15:30:45.123  1234  5678 F FATAL: process crashed";
        let parsed = parser.parse_line(line).unwrap();

        assert_eq!(parsed.severity, LogSeverity::Error);
    }
}
