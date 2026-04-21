// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ChromeError {
    #[error("http: {0}")]
    Http(String),

    #[error("json: {0}")]
    Json(String),

    #[error("websocket: {0}")]
    WebSocket(String),

    #[error("cdp: {0}")]
    Cdp(String),

    #[error("javascript exception: {0}")]
    JsException(String),

    #[error("element not found: {0}")]
    ElementNotFound(String),

    #[error("no page loaded -- navigate to a page first")]
    NoPageLoaded,

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("disconnected")]
    Disconnected,
}

pub type Result<T> = std::result::Result<T, ChromeError>;

impl From<reqwest::Error> for ChromeError {
    fn from(e: reqwest::Error) -> Self {
        ChromeError::Http(e.to_string())
    }
}

impl From<serde_json::Error> for ChromeError {
    fn from(e: serde_json::Error) -> Self {
        ChromeError::Json(e.to_string())
    }
}

impl From<tokio_tungstenite::tungstenite::Error> for ChromeError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        ChromeError::WebSocket(e.to_string())
    }
}
