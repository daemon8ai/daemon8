// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use super::{LogParser, LogSeverity, ParsedLine};

/// Parses `loggingctl log -o short_precise` output.
///
/// Two known formats from physical devices and emulators:
///
/// Physical (with facility.severity):
///   Mar 24 18:14:47.927259 firestick-xxx local1.err acr_core_dump[14271]:  message
///   Mar 24 18:14:48.429744 firestick-xxx local0.err lcm_service[900]: 900 E lcm-server: message
///
/// Emulator (no facility.severity):
///   Mar 27 23:56:07.183210 amazon-xxx systemd-journald[321]: message
///   Mar 27 23:56:07.184185 amazon-xxx servicergrd[818]: INFO com.amazon.tag: message
pub struct LoggingctlParser;

impl LogParser for LoggingctlParser {
    fn parse_line(&self, line: &str) -> Option<ParsedLine> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }

        // Timestamp: "Mar DD HH:MM:SS.microseconds" -- 3 space-separated tokens
        let mut parts = line.splitn(4, ' ');
        let month = parts.next()?;
        let day = parts.next()?;
        let time = parts.next()?;
        let rest = parts.next()?;

        // Validate timestamp shape
        if month.len() != 3 || !time.contains('.') {
            return None;
        }
        let timestamp = format!("{month} {day} {time}");

        // Next token: hostname
        let (hostname, rest) = split_first_token(rest)?;

        // Try to detect facility.severity (e.g., "local0.err", "kern.info")
        let (facility, severity_from_facility, rest) = try_parse_facility_severity(rest);

        // Process[PID]: -- find the "word[digits]:" pattern
        let (tag, pid, rest) = parse_process_pid(rest)?;

        // Determine severity: check for inline severity markers in the message
        let (severity, message) = if let Some(sev) = severity_from_facility {
            (sev, rest.to_string())
        } else {
            extract_inline_severity(rest)
        };

        Some(ParsedLine {
            timestamp,
            severity,
            tag,
            pid: Some(pid),
            message,
            hostname: Some(hostname.to_string()),
            facility,
        })
    }
}

fn split_first_token(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    let end = s.find(' ')?;
    Some((&s[..end], s[end..].trim_start()))
}

/// Try to parse "facility.severity" token (e.g., "local0.err", "kern.warning").
/// Returns (facility, severity, remaining_str).
/// If the next token doesn't match, returns None for facility/severity and the original string.
fn try_parse_facility_severity(s: &str) -> (Option<String>, Option<LogSeverity>, &str) {
    let s = s.trim_start();
    let first_space = s.find(' ').unwrap_or(s.len());
    let token = &s[..first_space];

    if let Some(dot) = token.find('.') {
        let facility = &token[..dot];
        let severity_str = &token[dot + 1..];

        // Validate it looks like a syslog facility
        let is_facility = facility.starts_with("local")
            || matches!(
                facility,
                "kern"
                    | "user"
                    | "mail"
                    | "daemon"
                    | "auth"
                    | "syslog"
                    | "lpr"
                    | "news"
                    | "uucp"
                    | "cron"
                    | "authpriv"
                    | "ftp"
            );

        if is_facility {
            let severity = parse_syslog_severity(severity_str);
            let rest = if first_space < s.len() {
                s[first_space..].trim_start()
            } else {
                ""
            };
            return (Some(facility.to_string()), Some(severity), rest);
        }
    }

    (None, None, s)
}

fn parse_syslog_severity(s: &str) -> LogSeverity {
    match s.to_lowercase().as_str() {
        "emerg" | "alert" | "crit" | "err" | "error" => LogSeverity::Error,
        "warning" | "warn" => LogSeverity::Warn,
        "notice" | "info" => LogSeverity::Info,
        "debug" => LogSeverity::Debug,
        _ => LogSeverity::Info,
    }
}

/// Parse "process[pid]:" from the current position.
/// Returns (process_name, pid, rest_of_line).
fn parse_process_pid(s: &str) -> Option<(String, u32, &str)> {
    let s = s.trim_start();
    let bracket_open = s.find('[')?;
    let bracket_close = s[bracket_open..].find(']')?;
    let bracket_close = bracket_open + bracket_close;

    let tag = s[..bracket_open].to_string();
    let pid_str = &s[bracket_open + 1..bracket_close];
    let pid: u32 = pid_str.parse().ok()?;

    // Expect "]: " or "]:" after
    let after = &s[bracket_close + 1..];
    let rest = after.strip_prefix(':').unwrap_or(after).trim_start();

    Some((tag, pid, rest))
}

/// Extract inline severity from the start of a message.
/// Handles patterns like:
///   "INFO com.amazon.tag: message"
///   "WARNING com.amazon.tag: message"
///   "900 E lcm-server: message"  (pid + single-char severity)
///   "E pkgmgr: message"
///   "I pkgmgr: message"
fn extract_inline_severity(s: &str) -> (LogSeverity, String) {
    let s = s.trim_start();

    // Try "SEVERITY rest" (e.g., "INFO ...", "WARNING ...", "ERROR ...")
    if let Some((first, rest)) = s.split_once(' ') {
        match first.to_uppercase().as_str() {
            "TRACE" => return (LogSeverity::Trace, rest.to_string()),
            "DEBUG" | "D" => return (LogSeverity::Debug, rest.to_string()),
            "INFO" | "I" => return (LogSeverity::Info, rest.to_string()),
            "WARNING" | "WARN" | "W" => return (LogSeverity::Warn, rest.to_string()),
            "ERROR" | "ERR" | "E" => return (LogSeverity::Error, rest.to_string()),
            "FATAL" | "F" => return (LogSeverity::Error, rest.to_string()),
            _ => {}
        }

        // Try "PID SEVERITY rest" (e.g., "900 E lcm-server: message")
        if first.chars().all(|c| c.is_ascii_digit())
            && let Some((sev_char, rest2)) = rest.split_once(' ')
        {
            let severity = match sev_char {
                "V" => Some(LogSeverity::Trace),
                "D" => Some(LogSeverity::Debug),
                "I" => Some(LogSeverity::Info),
                "W" => Some(LogSeverity::Warn),
                "E" => Some(LogSeverity::Error),
                "F" => Some(LogSeverity::Error),
                _ => None,
            };
            if let Some(sev) = severity {
                return (sev, rest2.to_string());
            }
        }
    }

    (LogSeverity::Info, s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_device_with_facility() {
        let parser = LoggingctlParser;
        let line = "Mar 24 18:14:47.927259 firestick-d5c035c0b6c510f5 local1.err acr_core_dump[14271]:  Cannot find library name for process com.rcn.rtntv_vega";
        let parsed = parser.parse_line(line).unwrap();

        assert_eq!(parsed.timestamp, "Mar 24 18:14:47.927259");
        assert_eq!(
            parsed.hostname.as_deref(),
            Some("firestick-d5c035c0b6c510f5")
        );
        assert_eq!(parsed.facility.as_deref(), Some("local1"));
        assert_eq!(parsed.severity, LogSeverity::Error);
        assert_eq!(parsed.tag, "acr_core_dump");
        assert_eq!(parsed.pid, Some(14271));
        assert!(parsed.message.contains("Cannot find library name"));
    }

    #[test]
    fn physical_device_with_facility_and_inline_severity() {
        let parser = LoggingctlParser;
        let line = "Mar 24 18:14:48.429744 firestick-d5c035c0b6c510f5 local0.err lcm_service[900]: 900 E lcm-server:[Lcm.cpp:4872] Deregistering application with clientPid 14225 and package id com.rcn.rtntv_vega after crash";
        let parsed = parser.parse_line(line).unwrap();

        assert_eq!(parsed.severity, LogSeverity::Error);
        assert_eq!(parsed.tag, "lcm_service");
        assert_eq!(parsed.pid, Some(900));
        assert!(parsed.message.contains("Deregistering application"));
    }

    #[test]
    fn emulator_no_facility() {
        let parser = LoggingctlParser;
        let line = "Mar 27 23:56:07.183210 amazon-569cff2b519833c2 systemd-journald[321]: Journal header limits reached or header out-of-date, rotating.";
        let parsed = parser.parse_line(line).unwrap();

        assert_eq!(parsed.timestamp, "Mar 27 23:56:07.183210");
        assert_eq!(parsed.hostname.as_deref(), Some("amazon-569cff2b519833c2"));
        assert!(parsed.facility.is_none());
        assert_eq!(parsed.tag, "systemd-journald");
        assert_eq!(parsed.pid, Some(321));
        assert!(parsed.message.contains("Journal header limits"));
    }

    #[test]
    fn emulator_with_inline_info() {
        let parser = LoggingctlParser;
        let line = "Mar 27 23:56:07.184185 amazon-569cff2b519833c2 servicergrd[818]: INFO com.amazon.appfwk.servicergrd: binder_request{cid=\"28738a207ff914b4\"}: processed request";
        let parsed = parser.parse_line(line).unwrap();

        assert_eq!(parsed.severity, LogSeverity::Info);
        assert_eq!(parsed.tag, "servicergrd");
        assert_eq!(parsed.pid, Some(818));
        assert!(parsed.message.contains("com.amazon.appfwk.servicergrd"));
    }

    #[test]
    fn emulator_with_inline_warning() {
        let parser = LoggingctlParser;
        let line = "Mar 27 23:56:07.193560 amazon-569cff2b519833c2 lcm_service[980]: WARNING com.amazon.lcm.app_control_ops.service.ipcf4.skeleton: .endp_hdl.dth_ctx: [ssn: 735] Received endpoint's death callback.";
        let parsed = parser.parse_line(line).unwrap();

        assert_eq!(parsed.severity, LogSeverity::Warn);
        assert_eq!(parsed.tag, "lcm_service");
    }

    #[test]
    fn emulator_with_inline_error_short() {
        let parser = LoggingctlParser;
        let line = "Mar 27 23:56:07.193846 amazon-569cff2b519833c2 pkgmgrd[982]: E pkgmgr-installer-util:[SecurityManagerWrapper.cpp:130] Security-Manager failed";
        let parsed = parser.parse_line(line).unwrap();

        assert_eq!(parsed.severity, LogSeverity::Error);
        assert_eq!(parsed.tag, "pkgmgrd");
    }

    #[test]
    fn emulator_with_inline_info_short() {
        let parser = LoggingctlParser;
        let line = "Mar 27 23:56:07.193833 amazon-569cff2b519833c2 pkgmgrd[982]: I pkgmgr-service-impl:[PackageManagerServiceImpl.cpp:299] Received request: 'client disconnected remove vendor tracking id listener' pid 14127";
        let parsed = parser.parse_line(line).unwrap();

        assert_eq!(parsed.severity, LogSeverity::Info);
        assert_eq!(parsed.tag, "pkgmgrd");
    }

    #[test]
    fn empty_line_returns_none() {
        let parser = LoggingctlParser;
        assert!(parser.parse_line("").is_none());
        assert!(parser.parse_line("   ").is_none());
    }

    #[test]
    fn garbage_returns_none() {
        let parser = LoggingctlParser;
        assert!(parser.parse_line("not a log line at all").is_none());
    }
}
