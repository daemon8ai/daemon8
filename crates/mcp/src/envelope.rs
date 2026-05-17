// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Alpha MCP response envelope.

pub use daemon8_core::control::{
    AlphaEnvelope, AlphaStatus, NextAction, ScopeMode, SessionConnection,
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn ok_uses_common_alpha_shape() {
        let body = AlphaEnvelope::success("ok", "ok", json!({"checkpoint": 42}))
            .with_next_action(NextAction::new("create_checkpoint", "bookmark", json!({})))
            .render();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["status"], "success");
        assert_eq!(parsed["code"], "ok");
        assert_eq!(parsed["data"]["checkpoint"], 42);
        assert_eq!(parsed["next_actions"][0]["tool"], "create_checkpoint");
        assert!(parsed.get("result").is_none());
        assert!(parsed.get("daemon8").is_none());
        assert!(parsed.get("error").is_none());
    }

    #[test]
    fn error_uses_common_alpha_shape() {
        let body = AlphaEnvelope::non_success(
            AlphaStatus::ConnectRequired,
            "connect_required",
            "connect first",
            "daemon8_connect binds this MCP session to a scope",
        )
        .with_next_action(NextAction::new("daemon8_connect", "bind scope", json!({})))
        .render();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["status"], "connect_required");
        assert_eq!(parsed["code"], "connect_required");
        assert_eq!(parsed["next_actions"][0]["tool"], "daemon8_connect");
    }
}
