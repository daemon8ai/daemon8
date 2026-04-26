// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdbError {
    #[error("adb: {0}")]
    Adb(String),

    #[error("device {serial}: {reason}")]
    Device { serial: String, reason: String },

    #[error("screenshot produced empty file")]
    ScreenshotEmpty,

    #[error("thread panicked: {0}")]
    Join(#[from] tokio::task::JoinError),
}

pub type Result<T> = std::result::Result<T, AdbError>;
