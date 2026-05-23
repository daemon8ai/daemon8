// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Shared daemon8 CLI constants.

pub const SERVICE: daemon8_providers::ServiceIdentity = daemon8_providers::ServiceIdentity {
    name: "daemon8",
    channel_name: Some("daemon8-channel"),
    display_name: "Daemon8",
    status_message: Some("daemon8 telemetry"),
};
