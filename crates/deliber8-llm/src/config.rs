// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Parse `AgentCard.model` (a free-form `serde_json::Value`) into a typed
//! `ProviderConfig`. Defaults are filled in by provider name so an operator
//! can spawn an agent with `{"provider": "openrouter", "model": "openai/gpt-4o-mini"}`
//! and the rest is sane.

use crate::openrouter;
use serde_json::Value;
use std::collections::BTreeMap;
use thiserror::Error;

/// Typed view of an agent's `model` field after parsing/defaulting.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// `openrouter`, `openai`, `ollama`, or another OpenAI-compat label.
    pub provider: String,
    /// Model identifier the provider expects (e.g. `openai/gpt-4o-mini`).
    pub model: String,
    /// Base URL to POST `/chat/completions` against.
    pub base_url: String,
    /// Name of the environment variable holding the API key. The secret is
    /// resolved at task-spawn time, never stored in this struct.
    pub api_key_env: Option<String>,
    pub temperature: f32,
    pub max_tokens: u32,
    /// Extra headers to include on every request (e.g. OpenRouter telemetry).
    pub extra_headers: BTreeMap<String, String>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("agent card has no model configuration")]
    MissingModel,
    #[error("model field must be a JSON object")]
    NotAnObject,
    #[error("model.provider is required and must be a string")]
    MissingProvider,
    #[error("model.model is required and must be a string")]
    MissingModelName,
    #[error("model.base_url is required for provider '{provider}' with no built-in default")]
    MissingBaseUrl { provider: String },
    #[error("model.api_key_env is required for provider '{provider}' with no built-in default")]
    MissingApiKeyEnv { provider: String },
    #[error("model.temperature must be a number in [0, 2]")]
    BadTemperature,
    #[error("model.max_tokens must be a positive integer <= 100000")]
    BadMaxTokens,
}

/// Parse an `AgentCard.model` JSON value into a `ProviderConfig`.
///
/// Required fields: `provider`, `model`. Everything else is filled in by
/// provider when omitted. An empty/null `model` field returns `MissingModel`.
pub fn parse_from_card(card_model: &Value) -> Result<ProviderConfig, ConfigError> {
    let obj = match card_model {
        Value::Null => return Err(ConfigError::MissingModel),
        Value::Object(map) if map.is_empty() => return Err(ConfigError::MissingModel),
        Value::Object(map) => map,
        _ => return Err(ConfigError::NotAnObject),
    };

    let provider = obj
        .get("provider")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(ConfigError::MissingProvider)?;

    let model = obj
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(ConfigError::MissingModelName)?;

    let base_url = obj
        .get("base_url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| default_base_url(&provider))
        .ok_or_else(|| ConfigError::MissingBaseUrl {
            provider: provider.clone(),
        })?;

    // api_key_env is optional for some providers (e.g. local Ollama), but if
    // we have a built-in default for the provider we use it.
    let api_key_env = obj
        .get("api_key_env")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| default_api_key_env(&provider));

    // Hard requirement: cloud providers must have an api_key_env.
    if api_key_env.is_none() && requires_api_key(&provider) {
        return Err(ConfigError::MissingApiKeyEnv {
            provider: provider.clone(),
        });
    }

    let temperature = match obj.get("temperature") {
        None | Some(Value::Null) => 0.2,
        Some(v) => v
            .as_f64()
            .filter(|t| (0.0..=2.0).contains(t))
            .ok_or(ConfigError::BadTemperature)? as f32,
    };

    let max_tokens = match obj.get("max_tokens") {
        None | Some(Value::Null) => 1024,
        Some(v) => v
            .as_u64()
            .filter(|n| *n > 0 && *n <= 100_000)
            .ok_or(ConfigError::BadMaxTokens)? as u32,
    };

    let mut extra_headers = BTreeMap::new();
    if provider == "openrouter" {
        extra_headers.insert("HTTP-Referer".to_string(), openrouter::REFERER.to_string());
        extra_headers.insert("X-Title".to_string(), openrouter::TITLE.to_string());
    }
    if let Some(extra) = obj.get("extra_headers").and_then(Value::as_object) {
        for (k, v) in extra {
            if let Some(s) = v.as_str() {
                extra_headers.insert(k.clone(), s.to_string());
            }
        }
    }

    Ok(ProviderConfig {
        provider,
        model,
        base_url,
        api_key_env,
        temperature,
        max_tokens,
        extra_headers,
    })
}

fn default_base_url(provider: &str) -> Option<String> {
    match provider {
        "openrouter" => Some(openrouter::DEFAULT_BASE_URL.to_string()),
        "openai" => Some("https://api.openai.com/v1".to_string()),
        "ollama" => Some("http://127.0.0.1:11434/v1".to_string()),
        _ => None,
    }
}

fn default_api_key_env(provider: &str) -> Option<String> {
    match provider {
        "openrouter" => Some(openrouter::DEFAULT_API_KEY_ENV.to_string()),
        "openai" => Some("OPENAI_API_KEY".to_string()),
        // Ollama runs locally and doesn't require auth by default.
        "ollama" => None,
        _ => None,
    }
}

fn requires_api_key(provider: &str) -> bool {
    !matches!(provider, "ollama")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_minimal_openrouter_card_with_defaults() {
        let v = json!({ "provider": "openrouter", "model": "openai/gpt-4o-mini" });
        let cfg = parse_from_card(&v).unwrap();
        assert_eq!(cfg.provider, "openrouter");
        assert_eq!(cfg.model, "openai/gpt-4o-mini");
        assert_eq!(cfg.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(cfg.api_key_env.as_deref(), Some("OPENROUTER_API_KEY"));
        assert_eq!(cfg.max_tokens, 1024);
        assert!(cfg.extra_headers.contains_key("HTTP-Referer"));
        assert!(cfg.extra_headers.contains_key("X-Title"));
    }

    #[test]
    fn parses_explicit_overrides() {
        let v = json!({
            "provider": "openai",
            "model": "gpt-4o-mini",
            "base_url": "https://my-proxy.example.com/v1",
            "api_key_env": "MY_KEY",
            "temperature": 0.7,
            "max_tokens": 256,
            "extra_headers": { "X-Custom": "yes" },
        });
        let cfg = parse_from_card(&v).unwrap();
        assert_eq!(cfg.base_url, "https://my-proxy.example.com/v1");
        assert_eq!(cfg.api_key_env.as_deref(), Some("MY_KEY"));
        assert_eq!(cfg.temperature, 0.7);
        assert_eq!(cfg.max_tokens, 256);
        assert_eq!(cfg.extra_headers.get("X-Custom").unwrap(), "yes");
    }

    #[test]
    fn ollama_does_not_require_api_key() {
        let v = json!({ "provider": "ollama", "model": "llama3.2" });
        let cfg = parse_from_card(&v).unwrap();
        assert!(cfg.api_key_env.is_none());
        assert_eq!(cfg.base_url, "http://127.0.0.1:11434/v1");
    }

    #[test]
    fn unknown_provider_requires_explicit_base_url() {
        let v = json!({ "provider": "weird-cloud", "model": "foo" });
        let err = parse_from_card(&v).unwrap_err();
        assert!(matches!(err, ConfigError::MissingBaseUrl { .. }));
    }

    #[test]
    fn unknown_provider_with_base_url_still_requires_api_key_env() {
        let v = json!({
            "provider": "weird-cloud",
            "model": "foo",
            "base_url": "https://x/v1",
        });
        let err = parse_from_card(&v).unwrap_err();
        assert!(matches!(err, ConfigError::MissingApiKeyEnv { .. }));
    }

    #[test]
    fn null_model_field_is_missing_model() {
        let err = parse_from_card(&json!(null)).unwrap_err();
        assert!(matches!(err, ConfigError::MissingModel));
    }

    #[test]
    fn empty_object_is_missing_model() {
        let err = parse_from_card(&json!({})).unwrap_err();
        assert!(matches!(err, ConfigError::MissingModel));
    }

    #[test]
    fn rejects_out_of_range_temperature() {
        let v = json!({ "provider": "openrouter", "model": "x", "temperature": 5.0 });
        let err = parse_from_card(&v).unwrap_err();
        assert!(matches!(err, ConfigError::BadTemperature));
    }
}
