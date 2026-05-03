// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::fmt;
use std::str::FromStr;

use rmcp::schemars;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvelopeStatus {
    Queued,
    Delivered,
    Read,
    Failed,
    Expired,
    Cancelled,
}

impl EnvelopeStatus {
    /// Terminal states are excluded from `list_pending` and most queue work.
    /// `Read` counts as terminal because once read, the envelope is consumed
    /// from the inbox-delivery perspective even though the record persists.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Read | Self::Failed | Self::Expired | Self::Cancelled
        )
    }
}

impl fmt::Display for EnvelopeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Queued => "queued",
            Self::Delivered => "delivered",
            Self::Read => "read",
            Self::Failed => "failed",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
        };
        f.write_str(s)
    }
}

impl FromStr for EnvelopeStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "queued" => Ok(Self::Queued),
            "delivered" => Ok(Self::Delivered),
            "read" => Ok(Self::Read),
            "failed" => Ok(Self::Failed),
            "expired" => Ok(Self::Expired),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!("unknown envelope status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvelopePriority {
    Low,
    Normal,
    High,
    Urgent,
}

impl fmt::Display for EnvelopePriority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
            Self::Urgent => "urgent",
        };
        f.write_str(s)
    }
}

impl FromStr for EnvelopePriority {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "normal" => Ok(Self::Normal),
            "high" => Ok(Self::High),
            "urgent" => Ok(Self::Urgent),
            other => Err(format!("unknown envelope priority: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvelopeKind {
    Message,
    Request,
    Response,
    Notice,
    Control,
}

impl fmt::Display for EnvelopeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Message => "message",
            Self::Request => "request",
            Self::Response => "response",
            Self::Notice => "notice",
            Self::Control => "control",
        };
        f.write_str(s)
    }
}

impl FromStr for EnvelopeKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "message" => Ok(Self::Message),
            "request" => Ok(Self::Request),
            "response" => Ok(Self::Response),
            "notice" => Ok(Self::Notice),
            "control" => Ok(Self::Control),
            other => Err(format!("unknown envelope kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvelopeRecord {
    pub id: String,
    pub kind: EnvelopeKind,
    pub status: EnvelopeStatus,
    pub priority: EnvelopePriority,
    pub from_address: String,
    pub to_address: String,
    pub inbox_address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliver_after: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub project_refs: Vec<String>,
    #[serde(default)]
    pub team_refs: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assert_serde_roundtrip<T>(value: &T, expected_wire: &str)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + fmt::Debug,
    {
        let text = serde_json::to_string(value).expect("serialize");
        assert_eq!(text, expected_wire);
        let back: T = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(&back, value);
    }

    #[test]
    fn envelope_status_serde_snake_case() {
        assert_serde_roundtrip(&EnvelopeStatus::Queued, "\"queued\"");
        assert_serde_roundtrip(&EnvelopeStatus::Delivered, "\"delivered\"");
        assert_serde_roundtrip(&EnvelopeStatus::Read, "\"read\"");
        assert_serde_roundtrip(&EnvelopeStatus::Failed, "\"failed\"");
        assert_serde_roundtrip(&EnvelopeStatus::Expired, "\"expired\"");
        assert_serde_roundtrip(&EnvelopeStatus::Cancelled, "\"cancelled\"");
    }

    #[test]
    fn envelope_priority_serde_snake_case() {
        assert_serde_roundtrip(&EnvelopePriority::Low, "\"low\"");
        assert_serde_roundtrip(&EnvelopePriority::Normal, "\"normal\"");
        assert_serde_roundtrip(&EnvelopePriority::High, "\"high\"");
        assert_serde_roundtrip(&EnvelopePriority::Urgent, "\"urgent\"");
    }

    #[test]
    fn envelope_kind_serde_snake_case() {
        assert_serde_roundtrip(&EnvelopeKind::Message, "\"message\"");
        assert_serde_roundtrip(&EnvelopeKind::Request, "\"request\"");
        assert_serde_roundtrip(&EnvelopeKind::Response, "\"response\"");
        assert_serde_roundtrip(&EnvelopeKind::Notice, "\"notice\"");
        assert_serde_roundtrip(&EnvelopeKind::Control, "\"control\"");
    }

    #[test]
    fn envelope_status_display_fromstr_roundtrip() {
        for s in [
            EnvelopeStatus::Queued,
            EnvelopeStatus::Delivered,
            EnvelopeStatus::Read,
            EnvelopeStatus::Failed,
            EnvelopeStatus::Expired,
            EnvelopeStatus::Cancelled,
        ] {
            let text = s.to_string();
            let back: EnvelopeStatus = text.parse().expect("parse");
            assert_eq!(back, s);
        }
        assert!("nonsense".parse::<EnvelopeStatus>().is_err());
    }

    #[test]
    fn envelope_priority_display_fromstr_roundtrip() {
        for p in [
            EnvelopePriority::Low,
            EnvelopePriority::Normal,
            EnvelopePriority::High,
            EnvelopePriority::Urgent,
        ] {
            let text = p.to_string();
            let back: EnvelopePriority = text.parse().expect("parse");
            assert_eq!(back, p);
        }
        assert!("nonsense".parse::<EnvelopePriority>().is_err());
    }

    #[test]
    fn envelope_kind_display_fromstr_roundtrip() {
        for k in [
            EnvelopeKind::Message,
            EnvelopeKind::Request,
            EnvelopeKind::Response,
            EnvelopeKind::Notice,
            EnvelopeKind::Control,
        ] {
            let text = k.to_string();
            let back: EnvelopeKind = text.parse().expect("parse");
            assert_eq!(back, k);
        }
        assert!("nonsense".parse::<EnvelopeKind>().is_err());
    }

    #[test]
    fn envelope_status_terminal_classification() {
        assert!(!EnvelopeStatus::Queued.is_terminal());
        assert!(!EnvelopeStatus::Delivered.is_terminal());
        assert!(EnvelopeStatus::Read.is_terminal());
        assert!(EnvelopeStatus::Failed.is_terminal());
        assert!(EnvelopeStatus::Expired.is_terminal());
        assert!(EnvelopeStatus::Cancelled.is_terminal());
    }

    fn full_record() -> EnvelopeRecord {
        EnvelopeRecord {
            id: "env_abc123".into(),
            kind: EnvelopeKind::Request,
            status: EnvelopeStatus::Queued,
            priority: EnvelopePriority::High,
            from_address: "agent:supervisor@daemon8".into(),
            to_address: "agent:specialist@daemon8".into(),
            inbox_address: "agent:specialist@daemon8".into(),
            subject: Some("inspect repo".into()),
            body: Some("walk crates/types".into()),
            payload: Some(json!({"files": ["lib.rs"], "depth": 2})),
            correlation_id: Some("corr-1".into()),
            thread_id: Some("thread-1".into()),
            reply_to: Some("agent:supervisor@daemon8".into()),
            created_at: 1_700_000_000_000_000_000,
            updated_at: 1_700_000_000_000_000_001,
            deliver_after: Some(1_700_000_000_000_001_000),
            delivered_at: Some(1_700_000_000_000_002_000),
            read_at: None,
            expires_at: Some(1_700_000_001_000_000_000),
            failed_at: None,
            failure_reason: None,
            tags: vec!["urgent".into(), "ops".into()],
            project_refs: vec!["proj:daemon8".into()],
            team_refs: vec!["team:core".into()],
        }
    }

    fn minimal_record() -> EnvelopeRecord {
        EnvelopeRecord {
            id: "env_min".into(),
            kind: EnvelopeKind::Notice,
            status: EnvelopeStatus::Queued,
            priority: EnvelopePriority::Normal,
            from_address: "system".into(),
            to_address: "agent:any".into(),
            inbox_address: "agent:any".into(),
            subject: None,
            body: None,
            payload: None,
            correlation_id: None,
            thread_id: None,
            reply_to: None,
            created_at: 0,
            updated_at: 0,
            deliver_after: None,
            delivered_at: None,
            read_at: None,
            expires_at: None,
            failed_at: None,
            failure_reason: None,
            tags: vec![],
            project_refs: vec![],
            team_refs: vec![],
        }
    }

    fn roundtrip_record(record: &EnvelopeRecord) {
        let text = serde_json::to_string(record).expect("serialize");
        let back: EnvelopeRecord = serde_json::from_str(&text).expect("deserialize");
        let text_again = serde_json::to_string(&back).expect("reserialize");
        assert_eq!(
            text, text_again,
            "envelope must roundtrip identically; first={text} second={text_again}"
        );
    }

    #[test]
    fn envelope_record_full_roundtrip() {
        roundtrip_record(&full_record());
    }

    #[test]
    fn envelope_record_minimal_roundtrip() {
        roundtrip_record(&minimal_record());
    }
}
