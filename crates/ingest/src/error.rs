// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use thiserror::Error;

#[derive(Debug, Error)]
pub enum IngestError {
    #[error("bind failed: {0}")]
    Bind(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, IngestError>;
