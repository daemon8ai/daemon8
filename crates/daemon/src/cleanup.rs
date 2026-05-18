// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use daemon8_store::{DebugSessionStore, MemoryStore, StateModel};
use daemon8_types::{DebugSessionOutcome, DebugSessionStatus, MemoryKind};

pub const RETENTION_SECS: u64 = 24 * 60 * 60;

pub const SCREENSHOT_RETENTION_SECS: u64 = 24 * 60 * 60;

pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(8 * 60 * 60);

/// Default inactivity threshold before an active debug session is auto-ended.
/// Belt-and-suspenders against an LLM that forgets to call end_debug_session:
/// at 4h the session is marked abandoned, a thin SessionSummary is written so
/// the row never silently disappears, and observations from that session
/// become eligible for the 24h reaper.
pub const DEFAULT_INACTIVITY_AUTO_END_SECS: u64 = 4 * 60 * 60;

pub struct CleanupCtx {
    pub store: Arc<dyn StateModel>,
    pub debug_session_store: Option<Arc<dyn DebugSessionStore>>,
    pub memory_store: Option<Arc<dyn MemoryStore>>,
    pub inactivity_auto_end_secs: u64,
}

/// Spawn a background task that periodically:
/// 1) auto-ends debug sessions whose `last_activity` is older than the
///    configured inactivity threshold;
/// 2) deletes stale observations (skipping rows linked to active sessions);
/// 3) deletes stale screenshot files.
///
/// Per-session last_activity flushing is handled by each DaemonMcp instance's
/// background flush task (B1.6), so the cleanup task only queries the DB.
///
/// The task waits one full `CLEANUP_INTERVAL` before the first sweep so a
/// freshly started daemon doesn't immediately churn the WAL. Cancels cleanly
/// via the `CancellationToken`.
pub fn spawn_cleanup_task(
    tasks: &mut JoinSet<()>,
    ctx: CleanupCtx,
    screenshot_dir: PathBuf,
    cancel: CancellationToken,
) {
    tasks.spawn(async move {
        loop {
            tokio::select! {
                () = tokio::time::sleep(CLEANUP_INTERVAL) => {}
                () = cancel.cancelled() => break,
            }
            run_cleanup_pass(&ctx, &screenshot_dir).await;
        }
        tracing::debug!("cleanup task stopped");
    });
}

pub(crate) async fn run_cleanup_pass(ctx: &CleanupCtx, screenshot_dir: &std::path::Path) {
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;

    auto_end_stale_debug_sessions(ctx, now_ns).await;

    let cutoff = now_ns.saturating_sub(Duration::from_secs(RETENTION_SECS).as_nanos() as u64);
    match ctx.store.cleanup_before(cutoff).await {
        Ok(0) => {}
        Ok(n) => tracing::debug!(deleted = n, "observation cleanup sweep"),
        Err(e) => tracing::error!("observation cleanup failed: {e}"),
    }

    cleanup_screenshots(screenshot_dir);
}

async fn auto_end_stale_debug_sessions(ctx: &CleanupCtx, now_ns: u64) {
    let (Some(ds_store), Some(mem_store)) =
        (ctx.debug_session_store.as_ref(), ctx.memory_store.as_ref())
    else {
        return;
    };
    let threshold_ns =
        now_ns.saturating_sub(Duration::from_secs(ctx.inactivity_auto_end_secs).as_nanos() as u64);

    let stale = match ds_store.find_stale_active(threshold_ns).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "find_stale_active failed");
            return;
        }
    };
    if stale.is_empty() {
        return;
    }

    tracing::info!(count = stale.len(), "auto-ending stale debug sessions");

    for session in stale {
        let id = match &session.id {
            Some(id) => id.clone(),
            None => continue,
        };
        let summary = format!(
            "Auto-abandoned: no activity for {} hours. Project: {}, started_at_ns: {}.",
            ctx.inactivity_auto_end_secs / 3600,
            session.project_slug,
            session.started_at,
        );
        let mem = daemon8_store::Memory {
            id: None,
            created_at: now_ns,
            updated_at: now_ns,
            kind: MemoryKind::SessionSummary,
            content: summary,
            source_observations: Vec::new(),
            tags: vec![
                "kind:debug_session_summary".into(),
                format!("project:{}", session.project_slug),
                "outcome:abandoned".into(),
                "auto_ended:true".into(),
            ],
            project_slug: session.project_slug.clone(),
            session_id: Some(id.clone()),
            confidence: 1.0,
            data: None,
        };
        let summary_id = match mem_store.save_memory(mem).await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(session = %id, error = %e, "auto-end summary save failed");
                continue;
            }
        };
        if let Err(e) = ds_store
            .end_debug_session(
                &id,
                DebugSessionStatus::Abandoned,
                Some(DebugSessionOutcome::Abandoned),
                Some(summary_id),
                now_ns,
            )
            .await
        {
            tracing::warn!(session = %id, error = %e, "auto-end DB update failed");
        }
    }
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
    use daemon8_store::{DebugSession, MemoryFilter, SurrealStore};
    use daemon8_types::{Observation, ObservationKind, Origin, Severity};

    fn make_observation(timestamp_ns: u64) -> Observation {
        make_observation_for_session(timestamp_ns, None)
    }

    fn make_observation_for_session(
        timestamp_ns: u64,
        debug_session_id: Option<&str>,
    ) -> Observation {
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
            service: None,
            source: None,
            source_instance: None,
            correlation_id: None,
            parent_id: None,
            tags: None,
            session_id: None,
            node_id: None,
            debug_session_id: debug_session_id.map(Arc::from),
            checkpoint_id: None,
            error_hash: None,
        }
    }

    #[tokio::test]
    async fn cleanup_before_removes_old_observations() {
        let store = Arc::new(SurrealStore::memory().await.unwrap());

        store.insert(make_observation(1_000)).await.unwrap();
        store.insert(make_observation(2_000)).await.unwrap();
        store.insert(make_observation(3_000)).await.unwrap();

        let deleted = store.cleanup_before(2_500).await.unwrap();
        assert_eq!(
            deleted, 2,
            "observations at t=1000 and t=2000 should be deleted"
        );

        let slice = store
            .query(&daemon8_types::Filter::default())
            .await
            .unwrap();
        assert_eq!(slice.observations.len(), 1);
        assert_eq!(slice.observations[0].timestamp_ns, 3_000);
    }

    fn build_ctx(store: Arc<SurrealStore>, threshold_secs: u64) -> CleanupCtx {
        CleanupCtx {
            store: store.clone(),
            debug_session_store: Some(Arc::new(store.debug_session_store())),
            memory_store: Some(Arc::new(store.memory_store())),
            inactivity_auto_end_secs: threshold_secs,
        }
    }

    fn fresh_session(project: &str, started_at: u64) -> DebugSession {
        DebugSession {
            id: None,
            started_at,
            ended_at: None,
            last_activity: started_at,
            project_slug: project.to_string(),
            description: None,
            status: DebugSessionStatus::Active,
            outcome: None,
            summary_memory_id: None,
            agent_id: ":test/claude+plan-agent>".into(),
            feature: None,
        }
    }

    #[tokio::test]
    async fn cleanup_skips_observations_linked_to_active_debug_sessions() {
        let store = Arc::new(SurrealStore::memory().await.unwrap());
        let ds_store = store.debug_session_store();
        let ctx = build_ctx(store.clone(), 4 * 3600);

        let session_id = ds_store
            .start_debug_session(fresh_session("daemon8", 1_000))
            .await
            .unwrap();

        // One observation tied to the active session, one untied — both with
        // very old timestamps so the cutoff would normally take both.
        store
            .insert(make_observation_for_session(1_000, Some(&session_id)))
            .await
            .unwrap();
        store.insert(make_observation(1_000)).await.unwrap();

        let deleted = ctx.store.cleanup_before(u64::MAX).await.unwrap();
        assert_eq!(
            deleted, 1,
            "untied observation should be reaped; tied one must survive"
        );

        let slice = store
            .query(&daemon8_types::Filter::default())
            .await
            .unwrap();
        assert_eq!(slice.observations.len(), 1);
        assert_eq!(
            slice.observations[0].debug_session_id.as_deref(),
            Some(session_id.as_str())
        );
    }

    #[tokio::test]
    async fn cleanup_reaps_observations_after_session_ends() {
        let store = Arc::new(SurrealStore::memory().await.unwrap());
        let ds_store = store.debug_session_store();
        let ctx = build_ctx(store.clone(), 4 * 3600);

        let session_id = ds_store
            .start_debug_session(fresh_session("daemon8", 1_000))
            .await
            .unwrap();
        store
            .insert(make_observation_for_session(1_000, Some(&session_id)))
            .await
            .unwrap();
        ds_store
            .end_debug_session(
                &session_id,
                DebugSessionStatus::Completed,
                Some(DebugSessionOutcome::Resolved),
                None,
                2_000,
            )
            .await
            .unwrap();

        let deleted = ctx.store.cleanup_before(u64::MAX).await.unwrap();
        assert_eq!(deleted, 1, "completed session frees its observations");
    }

    #[tokio::test]
    async fn auto_end_writes_summary_and_marks_abandoned_for_stale_sessions() {
        let store = Arc::new(SurrealStore::memory().await.unwrap());
        let ds_store = store.debug_session_store();

        // Threshold = 1 second; session is 10s old.
        let ctx = build_ctx(store.clone(), 1);

        let now_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let stale = ds_store
            .start_debug_session(fresh_session("daemon8", now_ns - 10_000_000_000))
            .await
            .unwrap();
        ds_store
            .touch_debug_session(&stale, now_ns - 10_000_000_000)
            .await
            .unwrap();

        run_cleanup_pass(&ctx, std::path::Path::new("/tmp/nonexistent")).await;

        let session = ds_store.get_debug_session(&stale).await.unwrap().unwrap();
        assert_eq!(session.status, DebugSessionStatus::Abandoned);
        assert!(session.summary_memory_id.is_some());

        let mem_store = store.memory_store();
        let summaries = mem_store
            .query_memory(&MemoryFilter {
                kinds: Some(vec![MemoryKind::SessionSummary]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(summaries.len(), 1);
        assert!(summaries[0].tags.contains(&"auto_ended:true".to_string()));
    }

    /// Per-MCP-session flush tasks (B1.6) replaced the global flush_active_last_activity.
    /// Verify that touch_debug_session directly updates the DB last_activity field.
    #[tokio::test]
    async fn touch_debug_session_updates_last_activity_in_db() {
        let store = Arc::new(SurrealStore::memory().await.unwrap());
        let ds_store: Arc<dyn DebugSessionStore> = Arc::new(store.debug_session_store());

        let session_id = ds_store
            .start_debug_session(fresh_session("p", 1_000))
            .await
            .unwrap();

        // Direct DB touch — no in-memory state needed
        ds_store
            .touch_debug_session(&session_id, 99_999)
            .await
            .unwrap();

        let session = ds_store
            .get_debug_session(&session_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(session.last_activity, 99_999);
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

        let past = SystemTime::now() - Duration::from_secs(SCREENSHOT_RETENTION_SECS + 3600);
        let times = std::fs::FileTimes::new().set_modified(past);
        std::fs::File::options()
            .write(true)
            .open(&old_file)
            .unwrap()
            .set_times(times)
            .unwrap();

        cleanup_screenshots(dir);

        assert!(!old_file.exists(), "old screenshot should be deleted");
        assert!(recent_file.exists(), "recent screenshot should be kept");
        assert!(
            unrelated_file.exists(),
            "unrelated file should be untouched"
        );
    }
}
