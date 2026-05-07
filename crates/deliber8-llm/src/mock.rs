// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! `MockLlmClient` — deterministic, no-network test double for the
//! specialist runtime and for any downstream code that wants to exercise
//! the trait without standing up a wiremock server.
//!
//! Default behaviour: echo the *last* user message back as the assistant
//! response, prefixed with the configured tag. This is enough for thread
//! correlation tests and produces stable, asserted strings.

use crate::client::LlmClient;
use crate::types::{CallOpts, Completion, LlmError, Message, Role, Usage};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default)]
pub struct MockCallRecord {
    pub system: String,
    pub messages: Vec<Message>,
    pub opts: CallOpts,
}

/// Test-only `LlmClient` that records every call and replies deterministically.
#[derive(Clone)]
pub struct MockLlmClient {
    model: String,
    provider: String,
    prefix: String,
    error: Option<Arc<dyn Fn() -> LlmError + Send + Sync>>,
    calls: Arc<Mutex<Vec<MockCallRecord>>>,
}

impl Default for MockLlmClient {
    fn default() -> Self {
        Self::new("mock", "mock-1")
    }
}

impl MockLlmClient {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            prefix: "[mock] ".into(),
            error: None,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Force every `complete` call to return the supplied error.
    pub fn always_fail<F>(mut self, factory: F) -> Self
    where
        F: Fn() -> LlmError + Send + Sync + 'static,
    {
        self.error = Some(Arc::new(factory));
        self
    }

    pub fn calls(&self) -> Vec<MockCallRecord> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(
        &self,
        system: &str,
        messages: &[Message],
        opts: &CallOpts,
    ) -> Result<Completion, LlmError> {
        self.calls.lock().unwrap().push(MockCallRecord {
            system: system.to_string(),
            messages: messages.to_vec(),
            opts: *opts,
        });

        if let Some(factory) = &self.error {
            return Err(factory());
        }

        let last_user = messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| m.content.as_str())
            .unwrap_or("");
        let content = format!("{}{}", self.prefix, last_user);
        let completion_tokens = content.len() as u32;

        Ok(Completion {
            content,
            usage: Some(Usage {
                prompt_tokens: messages.iter().map(|m| m.content.len() as u32).sum(),
                completion_tokens,
            }),
        })
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

    #[tokio::test]
    async fn echoes_last_user_message() {
        let mock = MockLlmClient::new("mock", "m1");
        let out = mock
            .complete(
                "you are helpful",
                &[Message::user("hello"), Message::user("how are you")],
                &CallOpts::default(),
            )
            .await
            .unwrap();
        assert_eq!(out.content, "[mock] how are you");
        assert_eq!(mock.calls().len(), 1);
        assert_eq!(mock.calls()[0].system, "you are helpful");
    }

    #[tokio::test]
    async fn always_fail_propagates() {
        let mock = MockLlmClient::new("mock", "m1").always_fail(|| LlmError::Timeout);
        let err = mock
            .complete("", &[Message::user("x")], &CallOpts::default())
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Timeout));
    }
}
