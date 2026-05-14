// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Project-aware auto-discovery (Phase D of the onboarding workstream).
//!
//! This module lives in `daemon8` rather than its own crate because it is
//! orchestration glue: it ties together the D1 detector (in
//! `daemon8-providers`), the D6 librarian schema (in `daemon8-store`),
//! and the daemon's observation pipeline.
//!
//! Layout:
//!
//! - [`hint`] (D2) — emits `discovery_hint` observations when the
//!   librarian has no template coverage for a project's tags.
//! - [`scanner`] (D3) — orchestrator. Pulls classification, checks the
//!   librarian, probes the filesystem, emits a hint when needed, and
//!   returns a [`scanner::DiscoveryPlan`] for D4 to render.
//! - [`presentation`] (D4) — renders the plan and prompts the user.
//! - [`registrar`] (D4) — registers confirmed sources with the
//!   librarian and the SourceManager.
//! - [`conversation`] (D5) — active session resolution + per-provider
//!   first-run detection for conversation templates.
//! - [`locator`] — shared pattern-expansion logic used by the scanner
//!   to resolve templates and by `doctor` to diagnose source drift.
//! - [`doctor_checks`] (D8) — pure check logic for project node,
//!   source template inventory, and version-aware source drift.

pub mod conversation;
pub mod doctor_checks;
pub mod hint;
pub mod locator;
#[allow(dead_code)]
pub mod presentation;
#[allow(dead_code)]
pub mod registrar;
pub mod scanner;
