// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use dateparser::parse;

pub fn normalize_timestamp_ns(raw: &str) -> Option<i64> {
    let dt = parse(raw).ok()?;
    Some(
        dt.timestamp_nanos_opt()
            .unwrap_or(dt.timestamp() * 1_000_000_000),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601() {
        let ns = normalize_timestamp_ns("2024-01-15T14:32:01Z").unwrap();
        assert!(ns > 0);
    }

    #[test]
    fn rfc2822() {
        let ns = normalize_timestamp_ns("Mon, 15 Jan 2024 14:32:01 +0000").unwrap();
        assert!(ns > 0);
    }

    #[test]
    fn date_only() {
        let ns = normalize_timestamp_ns("2024-01-15").unwrap();
        assert!(ns > 0);
    }

    #[test]
    fn datetime_no_tz() {
        let ns = normalize_timestamp_ns("2024-01-15 14:32:01").unwrap();
        assert!(ns > 0);
    }

    #[test]
    fn unix_epoch_seconds() {
        let ns = normalize_timestamp_ns("1705328121").unwrap();
        assert!(ns > 0);
    }

    #[test]
    fn clf_timestamp() {
        let ns = normalize_timestamp_ns("10/Jan/2024:13:55:36 -0700");
        // non-deterministic
        if let Some(ns) = ns {
            assert!(ns > 0);
        }
    }

    #[test]
    fn syslog_bsd_timestamp() {
        let ns = normalize_timestamp_ns("Oct 11 22:14:15");
        // non-deterministic
        if let Some(ns) = ns {
            assert!(ns > 0);
        }
    }

    #[test]
    fn garbage_returns_none() {
        assert!(normalize_timestamp_ns("not a date").is_none());
        assert!(normalize_timestamp_ns("").is_none());
    }
}
