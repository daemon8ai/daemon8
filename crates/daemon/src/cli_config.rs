// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Shared daemon8 CLI constants.

pub const PROJECT_CONFIG_DIR: &str = ".daemon8";
pub const PROJECT_CONFIG_FILENAME: &str = "config.md";

pub const SERVICE: daemon8_providers::ServiceIdentity = daemon8_providers::ServiceIdentity {
    name: "daemon8",
    channel_name: Some("daemon8-channel"),
    display_name: "Daemon8",
    hook_marker: "daemon8",
    status_message: Some("daemon8 telemetry"),
};
