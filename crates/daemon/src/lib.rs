// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Library face of the `daemon8` bin crate.
//!
//! Exists so integration tests under `crates/daemon/tests/` can reach
//! internal modules (currently the discovery scanner). The bin entry
//! at `src/main.rs` continues to own runtime wiring; this lib does not
//! re-export the CLI, the serve loop, or any HTTP/MCP machinery.
//!
//! Only modules whose public API is exercised by integration tests are
//! re-declared here. Adding a module to the lib is a deliberate
//! decision — internal-only orchestration code should not appear in
//! this list.

pub mod config;
pub mod discovery;
