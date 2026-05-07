// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Role tag on a chat-completion message. Mirrors the OpenAI shape.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    pub fn as_wire_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

/// One turn in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }
}

/// Per-call generation options.
#[derive(Debug, Clone, Copy)]
pub struct CallOpts {
    pub temperature: f32,
    pub max_tokens: u32,
}

impl Default for CallOpts {
    fn default() -> Self {
        Self {
            temperature: 0.2,
            max_tokens: 1024,
        }
    }
}

/// Token accounting returned by the provider, when available.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

/// Successful completion result.
#[derive(Debug, Clone)]
pub struct Completion {
    pub content: String,
    pub usage: Option<Usage>,
}

/// Failure modes a `LlmClient::complete` call can produce.
///
/// `MissingApiKey` is non-retriable; the caller should transition the agent
/// to `Failed` with a clear human-readable reason. `Http`/`Network`/`Timeout`
/// are per-envelope failures: the runtime marks the offending envelope failed
/// and continues processing the next one.
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("missing API key for env var {var}")]
    MissingApiKey { var: String },
    #[error("provider returned HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("network error talking to provider: {source}")]
    Network {
        #[source]
        source: reqwest::Error,
    },
    #[error("failed to decode provider response: {source}")]
    Decode {
        #[source]
        source: serde_json::Error,
    },
    #[error("provider response was empty (no choices returned)")]
    EmptyResponse,
    #[error("request timed out")]
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_wire_strings_round_trip() {
        assert_eq!(Role::System.as_wire_str(), "system");
        assert_eq!(Role::User.as_wire_str(), "user");
        assert_eq!(Role::Assistant.as_wire_str(), "assistant");

        let json = serde_json::to_string(&Role::User).unwrap();
        assert_eq!(json, "\"user\"");
        let back: Role = serde_json::from_str("\"assistant\"").unwrap();
        assert_eq!(back, Role::Assistant);
    }

    #[test]
    fn call_opts_default_matches_card_defaults() {
        let opts = CallOpts::default();
        assert!((opts.temperature - 0.2).abs() < f32::EPSILON);
        assert_eq!(opts.max_tokens, 1024);
    }

    #[test]
    fn missing_api_key_error_contains_var_name() {
        let e = LlmError::MissingApiKey {
            var: "OPENROUTER_API_KEY".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("OPENROUTER_API_KEY"));
    }
}
