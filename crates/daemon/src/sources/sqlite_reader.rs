// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use daemon8_parse::ConversationEvent;
use daemon8_types::{AppName, Observation, Origin};

use crate::config::SqliteSourceConfig;

struct ThreadRow {
    id: String,
    created_at_ms: i64,
    updated_at_ms: i64,
    cwd: String,
    model: Option<String>,
    git_sha: Option<String>,
    git_branch: Option<String>,
    agent_nickname: Option<String>,
    agent_role: Option<String>,
    cli_version: String,
    tokens_used: i64,
    approval_mode: String,
    first_user_message: String,
}

struct SpawnEdgeRow {
    parent_thread_id: String,
    child_thread_id: String,
    status: String,
}

struct PollState {
    high_water_ms: i64,
    seen: HashMap<String, i64>,
}

pub(crate) fn spawn_sqlite_source(
    tasks: &mut JoinSet<()>,
    name: String,
    cfg: SqliteSourceConfig,
    obs_tx: mpsc::UnboundedSender<Observation>,
    cancel: CancellationToken,
) {
    tasks.spawn(async move {
        if let Err(e) = run_sqlite_source(name.clone(), cfg, obs_tx, cancel).await {
            tracing::error!(source = %name, "sqlite source exited with error: {e}");
        }
    });
}

async fn run_sqlite_source(
    name: String,
    cfg: SqliteSourceConfig,
    obs_tx: mpsc::UnboundedSender<Observation>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let db_path = crate::config::expand_tilde(Path::new(&cfg.path));
    let poll_interval = Duration::from_secs(cfg.poll_interval_secs);

    tracing::info!(
        source = %name,
        path = %db_path.display(),
        poll_secs = cfg.poll_interval_secs,
        "sqlite source started"
    );

    let mut state = PollState {
        high_water_ms: 0,
        seen: HashMap::new(),
    };

    loop {
        if db_path.exists() {
            let db_path_c = db_path.clone();
            let high_water = state.high_water_ms;

            let result =
                tokio::task::spawn_blocking(move || poll_codex_db(&db_path_c, high_water)).await?;

            match result {
                Ok((threads, edges, new_high_water)) => {
                    let events = threads_to_events(&threads, &edges, &mut state);
                    let origin = Origin::Application {
                        name: AppName::from(name.as_str()),
                    };
                    let mut channel_closed = false;
                    for event in &events {
                        let observations = super::conversation_watcher::convert_event(
                            event,
                            origin.clone(),
                            &cfg.tags,
                        );
                        for obs in observations {
                            if obs_tx.send(obs).is_err() {
                                channel_closed = true;
                                break;
                            }
                        }
                        if channel_closed {
                            break;
                        }
                    }
                    state.high_water_ms = new_high_water;
                    if !events.is_empty() {
                        tracing::debug!(
                            source = %name,
                            events = events.len(),
                            high_water_ms = new_high_water,
                            "sqlite poll emitted events"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(source = %name, "sqlite poll error: {e}");
                }
            }
        } else {
            tracing::debug!(
                source = %name,
                path = %db_path.display(),
                "sqlite database not found, will retry"
            );
        }

        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(poll_interval) => {}
        }
    }

    Ok(())
}

fn poll_codex_db(
    db_path: &Path,
    high_water_ms: i64,
) -> anyhow::Result<(Vec<ThreadRow>, Vec<SpawnEdgeRow>, i64)> {
    let conn = Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    let mut threads = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, COALESCE(created_at_ms, created_at * 1000),
                    COALESCE(updated_at_ms, updated_at * 1000),
                    cwd, model, git_sha, git_branch,
                    agent_nickname, agent_role, cli_version,
                    tokens_used, approval_mode, first_user_message
             FROM threads
             WHERE COALESCE(updated_at_ms, updated_at * 1000) > ?1
             ORDER BY COALESCE(updated_at_ms, updated_at * 1000) ASC",
        )?;
        let rows = stmt.query_map([high_water_ms], |row| {
            Ok(ThreadRow {
                id: row.get(0)?,
                created_at_ms: row.get(1)?,
                updated_at_ms: row.get(2)?,
                cwd: row.get(3)?,
                model: row.get(4)?,
                git_sha: row.get(5)?,
                git_branch: row.get(6)?,
                agent_nickname: row.get(7)?,
                agent_role: row.get(8)?,
                cli_version: row.get(9)?,
                tokens_used: row.get(10)?,
                approval_mode: row.get(11)?,
                first_user_message: row.get(12)?,
            })
        })?;

        for row in rows {
            threads.push(row?);
        }
    }

    let new_high_water = threads
        .iter()
        .map(|t| t.updated_at_ms)
        .max()
        .unwrap_or(high_water_ms);

    let mut edges = Vec::new();
    if !threads.is_empty() {
        let thread_ids: Vec<&str> = threads.iter().map(|t| t.id.as_str()).collect();
        let mut stmt = conn
            .prepare("SELECT parent_thread_id, child_thread_id, status FROM thread_spawn_edges")?;
        let rows = stmt.query_map([], |row| {
            Ok(SpawnEdgeRow {
                parent_thread_id: row.get(0)?,
                child_thread_id: row.get(1)?,
                status: row.get(2)?,
            })
        })?;
        for row in rows {
            let edge = row?;
            if thread_ids.contains(&edge.parent_thread_id.as_str())
                || thread_ids.contains(&edge.child_thread_id.as_str())
            {
                edges.push(edge);
            }
        }
    }

    Ok((threads, edges, new_high_water))
}

fn threads_to_events(
    threads: &[ThreadRow],
    edges: &[SpawnEdgeRow],
    state: &mut PollState,
) -> Vec<ConversationEvent> {
    let mut events = Vec::new();

    for thread in threads {
        if let Some(&prev_ms) = state.seen.get(&thread.id)
            && prev_ms == thread.updated_at_ms
        {
            continue;
        }
        state.seen.insert(thread.id.clone(), thread.updated_at_ms);

        events.push(ConversationEvent::SessionMeta {
            session_id: thread.id.clone(),
            cwd: Some(thread.cwd.clone()),
            provider: "codex".into(),
            model: thread.model.clone(),
        });

        events.push(ConversationEvent::TurnMeta {
            model: thread.model.clone(),
            git_branch: thread.git_branch.clone(),
            git_sha: thread.git_sha.clone(),
            tokens: if thread.tokens_used > 0 {
                Some(thread.tokens_used as u64)
            } else {
                None
            },
            duration_ms: None,
            permission_mode: if thread.approval_mode.is_empty() {
                None
            } else {
                Some(thread.approval_mode.clone())
            },
            cli_version: if thread.cli_version.is_empty() {
                None
            } else {
                Some(thread.cli_version.clone())
            },
        });

        if !thread.first_user_message.is_empty() {
            events.push(ConversationEvent::UserPrompt {
                text: thread.first_user_message.clone(),
                timestamp: Some(ms_to_timestamp_string(thread.created_at_ms)),
            });
        }
    }

    for edge in edges {
        let child = threads.iter().find(|t| t.id == edge.child_thread_id);
        if child.is_some() {
            events.push(ConversationEvent::AgentSpawn {
                parent_session: edge.parent_thread_id.clone(),
                child_session: edge.child_thread_id.clone(),
                role: child.and_then(|c| c.agent_role.clone()),
                nickname: child.and_then(|c| c.agent_nickname.clone()),
                status: Some(edge.status.clone()),
            });
        }
    }

    events
}

fn ms_to_timestamp_string(ms: i64) -> String {
    let secs = ms / 1000;
    let frac = ms % 1000;
    format!("{secs}.{frac:03}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_thread(id: &str, updated_at_ms: i64) -> ThreadRow {
        ThreadRow {
            id: id.into(),
            created_at_ms: updated_at_ms - 1000,
            updated_at_ms,
            cwd: "/project".into(),
            model: Some("o3".into()),
            git_sha: Some("abc123".into()),
            git_branch: Some("main".into()),
            agent_nickname: None,
            agent_role: None,
            cli_version: "0.130.0".into(),
            tokens_used: 5000,
            approval_mode: "suggest".into(),
            first_user_message: "fix the bug".into(),
        }
    }

    fn fresh_state() -> PollState {
        PollState {
            high_water_ms: 0,
            seen: HashMap::new(),
        }
    }

    #[test]
    fn threads_to_events_emits_session_and_turn_meta() {
        let threads = vec![make_thread("t1", 1000)];
        let mut state = fresh_state();
        let events = threads_to_events(&threads, &[], &mut state);

        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::SessionMeta { session_id, cwd: Some(c), provider, model: Some(m) }
            if session_id == "t1" && c == "/project" && provider == "codex" && m == "o3"
        )));
        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::TurnMeta { model: Some(m), git_branch: Some(b), tokens: Some(5000), .. }
            if m == "o3" && b == "main"
        )));
    }

    #[test]
    fn threads_to_events_emits_user_prompt() {
        let threads = vec![make_thread("t1", 1000)];
        let mut state = fresh_state();
        let events = threads_to_events(&threads, &[], &mut state);

        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::UserPrompt { text, .. } if text == "fix the bug"
        )));
    }

    #[test]
    fn threads_to_events_skips_empty_message() {
        let mut thread = make_thread("t1", 1000);
        thread.first_user_message = String::new();
        let mut state = fresh_state();
        let events = threads_to_events(&[thread], &[], &mut state);

        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ConversationEvent::UserPrompt { .. }))
        );
    }

    #[test]
    fn threads_to_events_deduplicates() {
        let threads = vec![make_thread("t1", 1000)];
        let mut state = fresh_state();

        let first = threads_to_events(&threads, &[], &mut state);
        assert!(!first.is_empty());

        let second = threads_to_events(&threads, &[], &mut state);
        assert!(second.is_empty());
    }

    #[test]
    fn threads_to_events_re_emits_on_update() {
        let mut state = fresh_state();

        let threads_v1 = vec![make_thread("t1", 1000)];
        let first = threads_to_events(&threads_v1, &[], &mut state);
        assert!(!first.is_empty());

        let threads_v2 = vec![make_thread("t1", 2000)];
        let second = threads_to_events(&threads_v2, &[], &mut state);
        assert!(!second.is_empty());
    }

    #[test]
    fn spawn_edges_produce_agent_spawn() {
        let parent = make_thread("parent", 1000);
        let mut child = make_thread("child", 1000);
        child.agent_role = Some("reviewer".into());
        child.agent_nickname = Some("rev-1".into());
        let threads = vec![parent, child];

        let edges = vec![SpawnEdgeRow {
            parent_thread_id: "parent".into(),
            child_thread_id: "child".into(),
            status: "completed".into(),
        }];
        let mut state = fresh_state();
        let events = threads_to_events(&threads, &edges, &mut state);

        assert!(events.iter().any(|e| matches!(e,
            ConversationEvent::AgentSpawn {
                parent_session, child_session, role: Some(r), nickname: Some(n), status: Some(s)
            }
            if parent_session == "parent" && child_session == "child"
                && r == "reviewer" && n == "rev-1" && s == "completed"
        )));
    }

    #[test]
    fn spawn_edges_skipped_when_child_not_in_set() {
        let threads = vec![make_thread("parent", 1000)];
        let edges = vec![SpawnEdgeRow {
            parent_thread_id: "parent".into(),
            child_thread_id: "unknown_child".into(),
            status: "completed".into(),
        }];
        let mut state = fresh_state();
        let events = threads_to_events(&threads, &edges, &mut state);

        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ConversationEvent::AgentSpawn { .. }))
        );
    }

    #[test]
    fn ms_to_timestamp_string_format() {
        assert_eq!(ms_to_timestamp_string(1778605137640), "1778605137.640");
        assert_eq!(ms_to_timestamp_string(1000), "1.000");
        assert_eq!(ms_to_timestamp_string(0), "0.000");
    }

    #[test]
    fn poll_codex_db_reads_threads() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        create_test_schema(&conn);

        conn.execute(
            "INSERT INTO threads (id, rollout_path, created_at, updated_at, source,
                model_provider, cwd, title, sandbox_policy, approval_mode,
                cli_version, first_user_message, model, created_at_ms, updated_at_ms)
             VALUES (?1, '', ?2, ?3, 'cli', 'openai', '/project', 'test',
                'none', 'suggest', '0.130.0', 'hello', 'o3', ?4, ?5)",
            rusqlite::params!["t1", 1000, 1001, 1000000, 1001000],
        )
        .unwrap();
        drop(conn);

        let (threads, _edges, high_water) = poll_codex_db(&db_path, 0).unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id, "t1");
        assert_eq!(threads[0].cwd, "/project");
        assert_eq!(threads[0].model.as_deref(), Some("o3"));
        assert_eq!(high_water, 1001000);
    }

    #[test]
    fn poll_codex_db_respects_high_water() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        create_test_schema(&conn);

        for (id, updated_ms) in [("t1", 1000), ("t2", 2000), ("t3", 3000)] {
            conn.execute(
                "INSERT INTO threads (id, rollout_path, created_at, updated_at, source,
                    model_provider, cwd, title, sandbox_policy, approval_mode,
                    cli_version, first_user_message, created_at_ms, updated_at_ms)
                 VALUES (?1, '', 1, 1, 'cli', 'openai', '/p', 'test',
                    'none', 'suggest', '', '', ?2, ?3)",
                rusqlite::params![id, updated_ms / 1000, updated_ms],
            )
            .unwrap();
        }
        drop(conn);

        let (threads, _, _) = poll_codex_db(&db_path, 1500).unwrap();
        assert_eq!(threads.len(), 2);
        assert!(threads.iter().all(|t| t.updated_at_ms > 1500));
    }

    #[test]
    fn poll_codex_db_reads_spawn_edges() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        create_test_schema(&conn);

        for id in ["parent", "child"] {
            conn.execute(
                "INSERT INTO threads (id, rollout_path, created_at, updated_at, source,
                    model_provider, cwd, title, sandbox_policy, approval_mode,
                    cli_version, first_user_message, created_at_ms, updated_at_ms)
                 VALUES (?1, '', 1, 1, 'cli', 'openai', '/p', 'test',
                    'none', 'suggest', '', '', 1000, 1000)",
                rusqlite::params![id],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO thread_spawn_edges (parent_thread_id, child_thread_id, status)
             VALUES ('parent', 'child', 'completed')",
            [],
        )
        .unwrap();
        drop(conn);

        let (_, edges, _) = poll_codex_db(&db_path, 0).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].parent_thread_id, "parent");
        assert_eq!(edges[0].child_thread_id, "child");
    }

    fn create_test_schema(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                rollout_path TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                source TEXT NOT NULL,
                model_provider TEXT NOT NULL,
                cwd TEXT NOT NULL,
                title TEXT NOT NULL,
                sandbox_policy TEXT NOT NULL,
                approval_mode TEXT NOT NULL,
                tokens_used INTEGER NOT NULL DEFAULT 0,
                cli_version TEXT NOT NULL DEFAULT '',
                first_user_message TEXT NOT NULL DEFAULT '',
                agent_nickname TEXT,
                agent_role TEXT,
                model TEXT,
                git_sha TEXT,
                git_branch TEXT,
                created_at_ms INTEGER,
                updated_at_ms INTEGER
            );
            CREATE TABLE thread_spawn_edges (
                parent_thread_id TEXT NOT NULL,
                child_thread_id TEXT NOT NULL PRIMARY KEY,
                status TEXT NOT NULL
            );",
        )
        .unwrap();
    }
}
