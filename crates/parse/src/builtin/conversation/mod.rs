// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

pub mod claude;
pub mod codex;
pub mod gemini;

#[derive(Debug, Clone, PartialEq)]
pub enum ConversationEvent {
    ToolUse {
        tool: String,
        input: serde_json::Value,
        call_id: Option<String>,
        timestamp: Option<String>,
    },
    ToolResult {
        call_id: Option<String>,
        output: serde_json::Value,
        exit_code: Option<i32>,
        timestamp: Option<String>,
    },
    SessionMeta {
        session_id: String,
        cwd: Option<String>,
        provider: String,
        model: Option<String>,
    },
}
