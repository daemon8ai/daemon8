// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Library face of the `daemon8` bin crate.
//!
//! Exists so integration tests under `crates/daemon/tests/` can reach
//! internal modules. The bin entry at `src/main.rs` continues to own
//! runtime wiring; this lib does not re-export the CLI, the serve
//! loop, or any HTTP/MCP machinery.

pub mod config;
