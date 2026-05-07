// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! LLM client trait + OpenAI-compatible HTTP client for daemon8 specialists.
//!
//! This crate is the seam between the deliber8 specialist runtime and the
//! outside world. It exposes a small `LlmClient` trait, a concrete
//! `OpenAiCompatClient` that speaks the OpenAI `/v1/chat/completions` shape
//! (and therefore drives OpenRouter, Ollama, vLLM, LM Studio, and OpenAI
//! itself), plus a `MockLlmClient` for tests.
//!
//! API keys are read from environment variables at client construction time;
//! the agent card carries only the *name* of the env var, never the secret.
//! See [`config::ProviderConfig`] for the shape stored on `AgentCard.model`.

mod client;
mod config;
mod mock;
mod openrouter;
mod types;

pub use client::{LlmClient, OpenAiCompatClient};
pub use config::{ConfigError, ProviderConfig, parse_from_card};
pub use mock::MockLlmClient;
pub use types::{CallOpts, Completion, LlmError, Message, Role, Usage};
