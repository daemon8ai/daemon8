// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::sync::LazyLock;

use daemon8_types::Severity;
use regex::Regex;

static SEVERITY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(emergency|emerg|fatal|critical|crit|error|err|warning|warn|notice|info|debug|trace)\b")
        .expect("severity regex is valid")
});

pub fn sniff_severity(text: &str) -> Option<Severity> {
    let m = SEVERITY_RE.find(text)?;
    text_to_severity(m.as_str())
}

pub fn text_to_severity(s: &str) -> Option<Severity> {
    match s.to_ascii_lowercase().as_str() {
        "trace" => Some(Severity::Trace),
        "debug" | "dbg" => Some(Severity::Debug),
        "info" | "information" | "informational" | "notice" => Some(Severity::Info),
        "warn" | "warning" => Some(Severity::Warn),
        "error" | "err" | "fatal" | "critical" | "crit" | "alert" | "emergency" | "emerg" => {
            Some(Severity::Error)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_common_levels() {
        assert_eq!(
            sniff_severity("ERROR: something broke"),
            Some(Severity::Error)
        );
        assert_eq!(sniff_severity("WARNING: disk full"), Some(Severity::Warn));
        assert_eq!(sniff_severity("[INFO] started"), Some(Severity::Info));
        assert_eq!(
            sniff_severity("DEBUG checking cache"),
            Some(Severity::Debug)
        );
        assert_eq!(sniff_severity("TRACE enter fn"), Some(Severity::Trace));
        assert_eq!(sniff_severity("FATAL out of memory"), Some(Severity::Error));
        assert_eq!(
            sniff_severity("CRITICAL threshold breached"),
            Some(Severity::Error)
        );
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(sniff_severity("error: lower"), Some(Severity::Error));
        assert_eq!(sniff_severity("Warning: mixed"), Some(Severity::Warn));
        assert_eq!(sniff_severity("Info: cap"), Some(Severity::Info));
    }

    #[test]
    fn first_match_wins() {
        assert_eq!(sniff_severity("ERROR then WARNING"), Some(Severity::Error));
        assert_eq!(sniff_severity("INFO before ERROR"), Some(Severity::Info));
    }

    #[test]
    fn no_match() {
        assert_eq!(sniff_severity("just a plain line"), None);
        assert_eq!(sniff_severity(""), None);
        assert_eq!(sniff_severity("processing 42 items"), None);
    }

    #[test]
    fn word_boundary_prevents_partial() {
        assert_eq!(sniff_severity("informational data"), None);
        assert_eq!(sniff_severity("warnings are important"), None);
        assert_eq!(sniff_severity("errors happen"), None);
        assert_eq!(sniff_severity("the debugger crashed"), None);
    }
}
