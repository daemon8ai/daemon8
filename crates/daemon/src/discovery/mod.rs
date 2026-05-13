// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Project-aware auto-discovery (Phase D of the onboarding workstream).
//!
//! This module lives in `daemon8` rather than its own crate because it is
//! orchestration glue: it ties together the D1 detector (in
//! `daemon8-providers`), the D6 librarian schema (in `daemon8-store`),
//! and the daemon's observation pipeline.
//!
//! Commit 2 lands the discovery-hint mechanism only — see [`hint`]. The
//! scanner (D3), presentation (D4), conversation auto-detect (D5), and
//! cache logic land in later commits.
//!
//! The hint API is `pub` but unused inside the bin until D3 wires it
//! into the serve path. Unit and integration tests exercise it directly,
//! so dead-code lints are suppressed module-wide.

#![allow(dead_code)]

pub mod hint;
