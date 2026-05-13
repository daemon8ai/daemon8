// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Standard daemon8 MCP response envelope.
//!
//! Every tool wraps its result in `{result, daemon8: {...}, error}` so:
//! - the LLM gets a consistent shape it can pattern-match on (`error` first,
//!   then `result`, then `daemon8.next_actions` for steering);
//! - daemon8 has a place to surface meta-information (active debug session,
//!   next-action hints) on every call without each tool reinventing the
//!   wheel.
//!
//! The envelope is the load-bearing UX win of v0.3. Static tool descriptions
//! describe the world; envelope hints reflect *current* state, which is what
//! LLMs respond to most reliably.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Default, Clone, Serialize)]
pub struct DaemonMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_debug_session: Option<ActiveSessionEcho>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_actions: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Path-pattern hints (D7) — librarian-aware nudges injected when an
    /// observation references a reusable runtime-data path with no
    /// covering source_template. Empty vector is omitted from the
    /// serialized envelope.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub hints: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActiveSessionEcho {
    pub id: String,
    pub project_slug: String,
    pub started_at_ns: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvelopeError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<EnvelopeFix>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvelopeFix {
    pub tool: String,
}

#[derive(Debug, Serialize)]
pub struct Envelope {
    pub result: Value,
    pub daemon8: DaemonMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<EnvelopeError>,
}

impl Envelope {
    pub fn ok(result: Value, meta: DaemonMeta) -> Self {
        Self {
            result,
            daemon8: meta,
            error: None,
        }
    }

    pub fn err(
        code: impl Into<String>,
        message: impl Into<String>,
        hint: Option<&str>,
        fix_tool: Option<&str>,
        meta: DaemonMeta,
    ) -> Self {
        Self {
            result: Value::Null,
            daemon8: meta,
            error: Some(EnvelopeError {
                code: code.into(),
                message: message.into(),
                hint: hint.map(String::from),
                fix: fix_tool.map(|t| EnvelopeFix { tool: t.into() }),
            }),
        }
    }

    pub fn render(self) -> String {
        serde_json::to_string_pretty(&self)
            .unwrap_or_else(|e| format!("{{\"error\":\"envelope serialization failed: {e}\"}}"))
    }
}

/// Convenience: wrap a JSON value in an envelope with no special meta beyond
/// the active session echo (filled in by the caller).
pub fn ok_value(result: Value, meta: DaemonMeta) -> String {
    Envelope::ok(result, meta).render()
}

pub fn err(
    code: &str,
    message: &str,
    hint: Option<&str>,
    fix_tool: Option<&str>,
    meta: DaemonMeta,
) -> String {
    Envelope::err(code, message, hint, fix_tool, meta).render()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ok_includes_meta_fields_when_set() {
        let meta = DaemonMeta {
            active_debug_session: Some(ActiveSessionEcho {
                id: "ds_1".into(),
                project_slug: "daemon8".into(),
                started_at_ns: 1_000,
            }),
            next_actions: Some(vec!["create_checkpoint".into()]),
            hint: Some("an active debug session is running".into()),
            hints: Vec::new(),
        };
        let s = ok_value(json!({"checkpoint": 42}), meta);
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["result"]["checkpoint"], 42);
        assert_eq!(parsed["daemon8"]["active_debug_session"]["id"], "ds_1");
        assert_eq!(parsed["daemon8"]["next_actions"][0], "create_checkpoint");
        assert!(parsed["error"].is_null() || parsed.get("error").is_none());
    }

    #[test]
    fn ok_omits_unset_meta_fields() {
        let s = ok_value(json!({"x": 1}), DaemonMeta::default());
        let parsed: Value = serde_json::from_str(&s).unwrap();
        // active_debug_session, next_actions, hint, error: all skipped when None.
        assert!(parsed["daemon8"].as_object().unwrap().is_empty());
    }

    #[test]
    fn err_carries_code_message_hint_and_fix() {
        let s = err(
            "no_active_debug_session",
            "create_checkpoint requires an active debug session",
            Some("call start_debug_session first"),
            Some("start_debug_session"),
            DaemonMeta::default(),
        );
        let parsed: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["error"]["code"], "no_active_debug_session");
        assert_eq!(parsed["error"]["fix"]["tool"], "start_debug_session");
        assert!(parsed["result"].is_null());
    }
}
