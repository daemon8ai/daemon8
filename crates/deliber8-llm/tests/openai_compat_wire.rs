// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Wire-level tests for `OpenAiCompatClient` against a wiremock server. No
//! network, no real keys.

use daemon8_deliber8_llm::{
    CallOpts, LlmClient, LlmError, Message, OpenAiCompatClient, ProviderConfig, parse_from_card,
};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_KEY_VAR: &str = "DAEMON8_LLM_TEST_KEY";

fn set_test_key(value: &str) {
    // SAFETY: tests in this file run sequentially via the default per-file
    // wiremock fixture and do not race other code that reads this env var.
    unsafe { std::env::set_var(TEST_KEY_VAR, value) };
}

fn cfg_for(server: &MockServer, provider: &str) -> ProviderConfig {
    ProviderConfig {
        provider: provider.to_string(),
        model: "test-model".into(),
        base_url: server.uri(),
        api_key_env: Some(TEST_KEY_VAR.into()),
        temperature: 0.2,
        max_tokens: 64,
        extra_headers: Default::default(),
    }
}

#[tokio::test]
async fn complete_returns_assistant_content_and_usage() {
    set_test_key("sk-test");
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("Authorization", "Bearer sk-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "cmpl-1",
            "choices": [{
                "message": { "role": "assistant", "content": "the answer is 4" }
            }],
            "usage": { "prompt_tokens": 12, "completion_tokens": 5 }
        })))
        .mount(&server)
        .await;

    let client = OpenAiCompatClient::from_config(&cfg_for(&server, "openai")).unwrap();
    let out = client
        .complete(
            "you are terse",
            &[Message::user("what is 2+2?")],
            &CallOpts::default(),
        )
        .await
        .unwrap();

    assert_eq!(out.content, "the answer is 4");
    let usage = out.usage.unwrap();
    assert_eq!(usage.prompt_tokens, 12);
    assert_eq!(usage.completion_tokens, 5);
}

#[tokio::test]
async fn non_2xx_propagates_status_and_body() {
    set_test_key("sk-test");
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(401).set_body_string("{\"error\":{\"message\":\"unauth\"}}"),
        )
        .mount(&server)
        .await;

    let client = OpenAiCompatClient::from_config(&cfg_for(&server, "openai")).unwrap();
    let err = client
        .complete("", &[Message::user("hi")], &CallOpts::default())
        .await
        .unwrap_err();

    match err {
        LlmError::Http { status, body } => {
            assert_eq!(status, 401);
            assert!(body.contains("unauth"));
        }
        other => panic!("expected Http error, got {other:?}"),
    }
}

#[tokio::test]
async fn empty_choices_response_is_empty_response_error() {
    set_test_key("sk-test");
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "choices": [] })))
        .mount(&server)
        .await;

    let client = OpenAiCompatClient::from_config(&cfg_for(&server, "openai")).unwrap();
    let err = client
        .complete("", &[Message::user("x")], &CallOpts::default())
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::EmptyResponse));
}

#[tokio::test]
async fn openrouter_headers_are_forwarded() {
    set_test_key("sk-test");
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("HTTP-Referer", "https://daemon8.ai"))
        .and(header("X-Title", "daemon8"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "role": "assistant", "content": "ok" }}]
        })))
        .mount(&server)
        .await;

    // Build via parse_from_card so the openrouter defaults kick in, then
    // override base_url to point at wiremock.
    let mut cfg = parse_from_card(&json!({
        "provider": "openrouter",
        "model": "openai/gpt-4o-mini",
        "api_key_env": TEST_KEY_VAR,
    }))
    .unwrap();
    cfg.base_url = server.uri();

    let client = OpenAiCompatClient::from_config(&cfg).unwrap();
    let out = client
        .complete("", &[Message::user("x")], &CallOpts::default())
        .await
        .unwrap();
    assert_eq!(out.content, "ok");
}
