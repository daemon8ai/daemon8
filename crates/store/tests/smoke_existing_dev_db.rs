// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Opt-in smoke test: open an existing on-disk SurrealKV store and run
//! the full schema init path. Confirms an existing dev install with
//! pre-migration `catalog_node` rows survives the addition of the new
//! `data` field.
//!
//! Run with:
//!
//! ```text
//! DAEMON8_SMOKE_DB=/tmp/d8_smoke_store cargo test --test smoke_existing_dev_db -- --nocapture
//! ```
//!
//! Skipped silently when the env var is unset so CI does not flake.

use daemon8_store::StateModel;

#[tokio::test]
async fn opens_existing_dev_db_without_schema_errors() {
    let Some(path) = std::env::var_os("DAEMON8_SMOKE_DB") else {
        eprintln!("DAEMON8_SMOKE_DB not set; skipping");
        return;
    };
    let path = std::path::PathBuf::from(path);
    assert!(
        path.exists(),
        "smoke DB path does not exist: {}",
        path.display()
    );

    let store = daemon8_store::SurrealStore::open(&path)
        .await
        .expect("open existing dev store");

    let summary = store.summary().await.expect("summary");
    eprintln!(
        "opened {} cleanly, observation_count={}",
        path.display(),
        summary.observation_count
    );
}
