// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use daemon8_store::StateModel;

pub const RETENTION_SECS: u64 = 24 * 60 * 60;

pub const SCREENSHOT_RETENTION_SECS: u64 = 24 * 60 * 60;

pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(8 * 60 * 60);

/// Spawn a background task that periodically deletes stale observations and screenshots.
///
/// The task waits one full `CLEANUP_INTERVAL` before the first sweep so a freshly
/// started daemon doesn't immediately churn the WAL. After each sweep it sleeps
/// for another interval. The task cancels cleanly via the `CancellationToken`.
pub fn spawn_cleanup_task(
    tasks: &mut JoinSet<()>,
    store: Arc<dyn StateModel>,
    screenshot_dir: PathBuf,
    cancel: CancellationToken,
) {
    tasks.spawn(async move {
        loop {
            tokio::select! {
                () = tokio::time::sleep(CLEANUP_INTERVAL) => {}
                () = cancel.cancelled() => break,
            }

            let cutoff = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .saturating_sub(Duration::from_secs(RETENTION_SECS))
                .as_nanos() as u64;

            match store.cleanup_before(cutoff) {
                Ok(0) => {}
                Ok(n) => {
                    tracing::debug!(deleted = n, "observation cleanup sweep");
                    if let Err(e) = store.vacuum_incremental(200) {
                        tracing::debug!("incremental vacuum: {e}");
                    }
                }
                Err(e) => tracing::error!("observation cleanup failed: {e}"),
            }

            if let Err(e) = store.wal_checkpoint() {
                tracing::debug!("wal checkpoint: {e}");
            }

            cleanup_screenshots(&screenshot_dir);
        }
        tracing::debug!("cleanup task stopped");
    });
}

/// Delete screenshot files older than `SCREENSHOT_RETENTION_SECS` from the configured directory.
///
/// Screenshots land as `daemon8-screenshot-*.png`. This walks the directory (non-recursive),
/// checks mtime, and deletes stale files. Errors on individual files are logged and skipped.
fn cleanup_screenshots(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    let retention = Duration::from_secs(SCREENSHOT_RETENTION_SECS);
    let mut deleted = 0u32;

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("daemon8-screenshot-") || !name.ends_with(".png") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if now.duration_since(modified).unwrap_or_default() > retention {
            if let Err(e) = std::fs::remove_file(entry.path()) {
                tracing::debug!(path = %entry.path().display(), "failed to remove stale screenshot: {e}");
            } else {
                deleted += 1;
            }
        }
    }

    if deleted > 0 {
        tracing::debug!(deleted, "screenshot cleanup");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daemon8_store::MemoryStore;
    use daemon8_types::{Observation, ObservationKind, Origin, Severity};

    fn make_observation(timestamp_ns: u64) -> Observation {
        Observation {
            id: 0,
            timestamp_ns,
            severity: Severity::Info,
            origin: Origin::Application {
                name: "test".into(),
            },
            kind: ObservationKind::Log,
            data: serde_json::json!({"msg": "test"}),
            source_location: None,
        }
    }

    #[test]
    fn cleanup_before_removes_old_observations() {
        let store = Arc::new(MemoryStore::new());

        store.insert(make_observation(1_000)).unwrap();
        store.insert(make_observation(2_000)).unwrap();
        store.insert(make_observation(3_000)).unwrap();

        let deleted = store.cleanup_before(2_500).unwrap();
        assert_eq!(
            deleted, 2,
            "observations at t=1000 and t=2000 should be deleted"
        );

        // The remaining observation should still be queryable
        let slice = store.query(&daemon8_types::Filter::default()).unwrap();
        assert_eq!(slice.observations.len(), 1);
        assert_eq!(slice.observations[0].timestamp_ns, 3_000);
    }

    #[test]
    fn screenshot_cleanup_removes_old_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        // Create an "old" screenshot and a "recent" screenshot
        let old_file = dir.join("daemon8-screenshot-old.png");
        let recent_file = dir.join("daemon8-screenshot-recent.png");
        let unrelated_file = dir.join("other-file.txt");

        std::fs::write(&old_file, b"old").unwrap();
        std::fs::write(&recent_file, b"recent").unwrap();
        std::fs::write(&unrelated_file, b"keep").unwrap();

        // Set old file mtime to 48h ago via touch command
        let past = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - (SCREENSHOT_RETENTION_SECS + 3600);
        let ts = format!("{past}");
        // Use touch -t with a formatted timestamp
        let formatted = std::process::Command::new("date")
            .args(["-r", &ts, "+%Y%m%d%H%M.%S"])
            .output()
            .expect("date command should work");
        let touch_ts = String::from_utf8_lossy(&formatted.stdout);
        let touch_ts = touch_ts.trim();

        std::process::Command::new("touch")
            .args(["-t", touch_ts, old_file.to_str().unwrap()])
            .status()
            .expect("touch should work");

        cleanup_screenshots(dir);

        assert!(!old_file.exists(), "old screenshot should be deleted");
        assert!(recent_file.exists(), "recent screenshot should be kept");
        assert!(
            unrelated_file.exists(),
            "unrelated file should be untouched"
        );
    }
}
