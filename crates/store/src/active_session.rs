// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Single-process "what's currently happening" state.
//!
//! The active debug session and active checkpoint are runtime state that
//! every observation insert needs to stamp onto the row. Both the
//! ingestion writer (in the daemon binary) and the MCP tools (in the
//! mcp crate) need to mutate this state, so it lives in a shared crate
//! they both depend on.
//!
//! Per-MCP-session background flush tasks (B1.6) periodically write
//! `last_activity_ns` to the DB so `find_stale_active` sees current
//! data without an awaited DB write per observation. The global cleanup
//! task no longer touches in-memory session state.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone)]
pub struct ActiveDebugSession {
    pub id: Arc<str>,
    pub project_slug: Arc<str>,
    pub started_at_ns: u64,
    pub last_activity_ns: Arc<AtomicU64>,
    pub agent_id: Arc<str>,
    pub feature: Option<Arc<str>>,
}

impl ActiveDebugSession {
    pub fn touch(&self, now_ns: u64) {
        self.last_activity_ns.store(now_ns, Ordering::Relaxed);
    }

    pub fn last_activity(&self) -> u64 {
        self.last_activity_ns.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Default)]
pub struct ActiveSessionState {
    inner: Arc<ActiveSessionInner>,
}

#[derive(Default)]
struct ActiveSessionInner {
    debug_session: Mutex<Option<ActiveDebugSession>>,
    checkpoint: Mutex<Option<Arc<str>>>,
}

impl ActiveSessionState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current_session(&self) -> Option<ActiveDebugSession> {
        self.inner
            .debug_session
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    pub fn current_checkpoint(&self) -> Option<Arc<str>> {
        self.inner
            .checkpoint
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    pub fn set_session(&self, session: Option<ActiveDebugSession>) {
        if let Ok(mut guard) = self.inner.debug_session.lock() {
            *guard = session;
        }
    }

    pub fn set_checkpoint(&self, checkpoint: Option<Arc<str>>) {
        if let Ok(mut guard) = self.inner.checkpoint.lock() {
            *guard = checkpoint;
        }
    }

    /// Clear both fields. Used on `end_debug_session` / `resolve_debug_session`.
    pub fn clear(&self) {
        self.set_session(None);
        self.set_checkpoint(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session(id: &str, project: &str, ts: u64) -> ActiveDebugSession {
        ActiveDebugSession {
            id: Arc::from(id),
            project_slug: Arc::from(project),
            started_at_ns: ts,
            last_activity_ns: Arc::new(AtomicU64::new(ts)),
            agent_id: Arc::from(":test/claude+plan-agent>"),
            feature: None,
        }
    }

    #[test]
    fn empty_by_default() {
        let s = ActiveSessionState::new();
        assert!(s.current_session().is_none());
        assert!(s.current_checkpoint().is_none());
    }

    #[test]
    fn set_and_clear() {
        let s = ActiveSessionState::new();
        s.set_session(Some(make_session("ds_1", "daemon8", 1_000)));
        s.set_checkpoint(Some(Arc::from("cp_42")));
        let cur = s.current_session().unwrap();
        assert_eq!(&*cur.id, "ds_1");
        assert_eq!(s.current_checkpoint().unwrap().as_ref(), "cp_42");

        s.clear();
        assert!(s.current_session().is_none());
        assert!(s.current_checkpoint().is_none());
    }

    #[test]
    fn touch_updates_last_activity() {
        let s = make_session("x", "p", 1_000);
        assert_eq!(s.last_activity(), 1_000);
        s.touch(9_999);
        assert_eq!(s.last_activity(), 9_999);
    }

    #[test]
    fn clones_share_atomic_state() {
        let s = make_session("x", "p", 1_000);
        let clone = s.clone();
        s.touch(5_000);
        assert_eq!(clone.last_activity(), 5_000);
    }
}
