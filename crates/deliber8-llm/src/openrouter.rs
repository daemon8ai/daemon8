// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! OpenRouter-specific small helpers. Most of the wire is identical to OpenAI;
//! OpenRouter just appreciates `HTTP-Referer` and `X-Title` headers so it can
//! categorize traffic in its dashboard.

pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const DEFAULT_API_KEY_ENV: &str = "OPENROUTER_API_KEY";
pub const REFERER: &str = "https://daemon8.ai";
pub const TITLE: &str = "daemon8";
