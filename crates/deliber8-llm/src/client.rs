// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! [`LlmClient`] trait + an OpenAI-compatible HTTP implementation that drives
//! OpenRouter, Ollama, vLLM, LM Studio, and OpenAI itself.
//!
//! The trait is deliberately small: a system prompt, a flat list of messages,
//! and per-call options. Streaming is intentionally out of scope for v1 — the
//! specialist loop publishes its response as one envelope, not as a stream.

use crate::config::ProviderConfig;
use crate::types::{CallOpts, Completion, LlmError, Message, Usage};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Boundary between the specialist runtime and any LLM provider.
///
/// Implementations must be `Send + Sync` so a single client can be shared
/// across the per-agent task and any future helpers.
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(
        &self,
        system: &str,
        messages: &[Message],
        opts: &CallOpts,
    ) -> Result<Completion, LlmError>;

    /// Human-readable model identifier ("openai/gpt-4o-mini").
    fn model_name(&self) -> &str;

    /// Provider label ("openrouter", "openai", ...).
    fn provider_label(&self) -> &str;
}

/// HTTP client speaking the OpenAI `/chat/completions` shape. Configurable
/// via [`ProviderConfig`]; the API key is resolved from the environment at
/// construction time and stored only inside this struct.
pub struct OpenAiCompatClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    provider: String,
    model: String,
    extra_headers: Vec<(String, String)>,
}

impl std::fmt::Debug for OpenAiCompatClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatClient")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("extra_headers", &self.extra_headers)
            .finish()
    }
}

impl OpenAiCompatClient {
    /// Build a client from a parsed `ProviderConfig`. Reads the API key from
    /// the env var named in `cfg.api_key_env` (if any). A missing env var on
    /// a provider that requires one returns `LlmError::MissingApiKey`.
    pub fn from_config(cfg: &ProviderConfig) -> Result<Self, LlmError> {
        let api_key = match cfg.api_key_env.as_deref() {
            Some(var_name) => match std::env::var(var_name) {
                Ok(v) if !v.trim().is_empty() => Some(v),
                Ok(_) | Err(_) => {
                    return Err(LlmError::MissingApiKey {
                        var: var_name.to_string(),
                    });
                }
            },
            None => None,
        };

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("reqwest client should build");

        Ok(Self {
            http,
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            api_key,
            provider: cfg.provider.clone(),
            model: cfg.model.clone(),
            extra_headers: cfg
                .extra_headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        })
    }
}

#[derive(Serialize)]
struct WireMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct WireRequest<'a> {
    model: &'a str,
    messages: Vec<WireMessage<'a>>,
    temperature: f32,
    max_tokens: u32,
}

#[derive(Deserialize)]
struct WireResponse {
    choices: Vec<WireChoice>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireChoice {
    message: WireChoiceMessage,
}

#[derive(Deserialize)]
struct WireChoiceMessage {
    #[serde(default)]
    content: String,
}

#[derive(Deserialize)]
struct WireUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

#[async_trait]
impl LlmClient for OpenAiCompatClient {
    async fn complete(
        &self,
        system: &str,
        messages: &[Message],
        opts: &CallOpts,
    ) -> Result<Completion, LlmError> {
        let mut wire_msgs: Vec<WireMessage<'_>> = Vec::with_capacity(messages.len() + 1);
        if !system.is_empty() {
            wire_msgs.push(WireMessage {
                role: "system",
                content: system,
            });
        }
        for m in messages {
            wire_msgs.push(WireMessage {
                role: m.role.as_wire_str(),
                content: &m.content,
            });
        }

        let body = WireRequest {
            model: &self.model,
            messages: wire_msgs,
            temperature: opts.temperature,
            max_tokens: opts.max_tokens,
        };

        let url = format!("{}/chat/completions", self.base_url);
        let mut req = self.http.post(&url).json(&body);
        if let Some(k) = &self.api_key {
            req = req.bearer_auth(k);
        }
        for (k, v) in &self.extra_headers {
            req = req.header(k, v);
        }

        let resp = req
            .send()
            .await
            .map_err(|source| match source.is_timeout() {
                true => LlmError::Timeout,
                false => LlmError::Network { source },
            })?;

        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|source| LlmError::Network { source })?;

        if !status.is_success() {
            let body = String::from_utf8_lossy(&bytes).to_string();
            return Err(LlmError::Http {
                status: status.as_u16(),
                body,
            });
        }

        let parsed: WireResponse =
            serde_json::from_slice(&bytes).map_err(|source| LlmError::Decode { source })?;

        let content = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or(LlmError::EmptyResponse)?;

        let usage = parsed.usage.map(|u| Usage {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
        });

        Ok(Completion { content, usage })
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn provider_label(&self) -> &str {
        &self.provider
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_api_key_env_surfaces_error() {
        // Use a uniquely-named env var that's never set anywhere. We do NOT
        // touch the env table — set_var/remove_var are unsafe in 2024
        // edition because they race with other threads reading env. By
        // picking a name that no test or production code references, we
        // get the "missing key" path without any global mutation.
        let unique = format!(
            "DAEMON8_LLM_NEVER_SET_KEY_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let cfg = ProviderConfig {
            provider: "openrouter".into(),
            model: "x".into(),
            base_url: "https://openrouter.ai/api/v1".into(),
            api_key_env: Some(unique),
            temperature: 0.2,
            max_tokens: 128,
            extra_headers: Default::default(),
        };
        let err = OpenAiCompatClient::from_config(&cfg).unwrap_err();
        assert!(matches!(err, LlmError::MissingApiKey { .. }));
    }

    #[test]
    fn ollama_no_api_key_required() {
        let cfg = ProviderConfig {
            provider: "ollama".into(),
            model: "llama3.2".into(),
            base_url: "http://127.0.0.1:11434/v1".into(),
            api_key_env: None,
            temperature: 0.2,
            max_tokens: 128,
            extra_headers: Default::default(),
        };
        let c = OpenAiCompatClient::from_config(&cfg).unwrap();
        assert_eq!(c.model_name(), "llama3.2");
        assert_eq!(c.provider_label(), "ollama");
    }
}
