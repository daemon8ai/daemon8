// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use daemon8_parse::ConversationEvent;
use daemon8_providers::{ALL_PROVIDERS, Provider};
use daemon8_types::{AppName, Observation, ObservationKind, Origin, Severity};

use crate::config::ConversationSourceConfig;

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const MAX_INITIAL_FILES: usize = 50;

struct TailContext {
    origin_name: String,
    provider_id: String,
    tags: Vec<String>,
    obs_tx: mpsc::UnboundedSender<Observation>,
}

pub(crate) fn spawn_conversation_source(
    tasks: &mut JoinSet<()>,
    name: String,
    cfg: ConversationSourceConfig,
    obs_tx: mpsc::UnboundedSender<Observation>,
    cancel: CancellationToken,
) {
    tasks.spawn(async move {
        if let Err(e) = run_conversation_source(name.clone(), cfg, obs_tx, cancel).await {
            tracing::error!(source = %name, "conversation source exited with error: {e}");
        }
    });
}

fn resolve_provider(provider_id: &str) -> Option<Provider> {
    ALL_PROVIDERS
        .iter()
        .find(|p| p.as_provider().id() == provider_id)
        .copied()
}

async fn run_conversation_source(
    name: String,
    cfg: ConversationSourceConfig,
    obs_tx: mpsc::UnboundedSender<Observation>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let provider = resolve_provider(&cfg.provider)
        .ok_or_else(|| anyhow::anyhow!("unknown conversation provider '{}'", cfg.provider))?;

    let home = daemon8_providers::dirs_home();
    let p = provider.as_provider();

    let convo_dir = p
        .conversation_dir(&home)
        .ok_or_else(|| anyhow::anyhow!("{} does not have a conversation directory", p.label()))?;

    let glob_pattern = p
        .conversation_file_glob()
        .ok_or_else(|| anyhow::anyhow!("{} does not have a conversation file glob", p.label()))?;

    if !convo_dir.exists() {
        tracing::debug!(
            source = %name,
            dir = %convo_dir.display(),
            "conversation directory does not exist yet"
        );
    }

    let full_glob = convo_dir.join(glob_pattern).to_string_lossy().to_string();

    let mut initial_files = expand_glob(&full_glob)?;
    if initial_files.len() > MAX_INITIAL_FILES {
        tracing::warn!(
            source = %name,
            total = initial_files.len(),
            limit = MAX_INITIAL_FILES,
            "too many conversation files; tailing only the most recent"
        );
        initial_files.sort_by_key(|p| {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
        initial_files.reverse();
        initial_files.truncate(MAX_INITIAL_FILES);
    }

    tracing::info!(
        source = %name,
        provider = %cfg.provider,
        files = initial_files.len(),
        "conversation source started"
    );

    let ctx = Arc::new(TailContext {
        origin_name: name.clone(),
        provider_id: cfg.provider.clone(),
        tags: cfg.tags.clone(),
        obs_tx,
    });

    let mut tails: JoinSet<()> = JoinSet::new();
    let mut watched: HashSet<PathBuf> = HashSet::new();

    for path in &initial_files {
        watched.insert(path.clone());
        spawn_tail(&mut tails, path.clone(), &ctx, cancel.clone(), true);
    }

    let (notify_tx, mut notify_rx) = mpsc::unbounded_channel::<notify::Event>();
    let mut watcher = {
        let tx = notify_tx.clone();
        notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        })?
    };

    use notify::Watcher;
    let watch_dirs = collect_watch_dirs(&full_glob);
    for dir in &watch_dirs {
        if dir.exists()
            && let Err(e) = watcher.watch(dir, notify::RecursiveMode::Recursive)
        {
            tracing::warn!(source = %name, dir = %dir.display(), "failed to watch: {e}");
        }
    }

    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            event = notify_rx.recv() => {
                let Some(event) = event else { break };
                if matches!(event.kind, notify::EventKind::Create(_)) {
                    for path in &event.paths {
                        if !watched.contains(path) && path_matches_glob(path, &full_glob) {
                            tracing::debug!(source = %ctx.origin_name, file = %path.display(), "new conversation file");
                            watched.insert(path.clone());
                            spawn_tail(&mut tails, path.clone(), &ctx, cancel.clone(), false);
                        }
                    }
                }
            }
            _ = tails.join_next(), if !tails.is_empty() => {}
        }
    }

    tails.abort_all();
    while tails.join_next().await.is_some() {}
    drop(watcher);
    drop(notify_tx);

    Ok(())
}

fn spawn_tail(
    tasks: &mut JoinSet<()>,
    path: PathBuf,
    ctx: &Arc<TailContext>,
    cancel: CancellationToken,
    seek_to_end: bool,
) {
    let ctx = ctx.clone();
    tasks.spawn(async move {
        if let Err(e) = tail_conversation(path.clone(), &ctx, cancel, seek_to_end).await {
            tracing::warn!(file = %path.display(), source = %ctx.origin_name, "conversation tail stopped: {e}");
        }
    });
}

async fn tail_conversation(
    path: PathBuf,
    ctx: &TailContext,
    cancel: CancellationToken,
    seek_to_end: bool,
) -> anyhow::Result<()> {
    let mut file = std::fs::File::open(&path)?;
    let mut offset = if seek_to_end {
        file.seek(SeekFrom::End(0))?
    } else {
        0
    };

    let mut line_buf = String::new();

    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(POLL_INTERVAL) => {
                offset = read_new_lines(&path, &mut file, offset, &mut line_buf, ctx)?;
            }
        }
    }

    Ok(())
}

fn read_new_lines(
    path: &Path,
    file: &mut std::fs::File,
    mut offset: u64,
    line_buf: &mut String,
    ctx: &TailContext,
) -> anyhow::Result<u64> {
    let file_len = std::fs::metadata(path)?.len();

    if file_len < offset {
        *file = std::fs::File::open(path)?;
        offset = 0;
    }

    if file_len == offset {
        return Ok(offset);
    }

    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::new(&*file);

    loop {
        line_buf.clear();
        let bytes_read = reader.read_line(line_buf)?;
        if bytes_read == 0 {
            break;
        }
        offset += bytes_read as u64;

        let line = line_buf.trim_end_matches('\n').trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }

        let events = daemon8_parse::parse_conversation_line(&ctx.provider_id, line);
        let mut channel_closed = false;

        for event in events {
            let observations = event_to_observations(&event, ctx);
            for obs in observations {
                if ctx.obs_tx.send(obs).is_err() {
                    channel_closed = true;
                    break;
                }
            }
            if channel_closed {
                break;
            }
        }

        if channel_closed {
            break;
        }
    }

    Ok(offset)
}

fn event_to_observations(event: &ConversationEvent, ctx: &TailContext) -> Vec<Observation> {
    let origin = Origin::Application {
        name: AppName::from(ctx.origin_name.as_str()),
    };

    match event {
        ConversationEvent::ToolUse {
            tool,
            input,
            timestamp,
            ..
        } => {
            let ts_ns = resolve_timestamp(timestamp.as_deref());

            let mut tool_obs = Observation::new(
                origin.clone(),
                ObservationKind::ToolCall {
                    tool: tool.clone(),
                    input: input.clone(),
                    output: None,
                    exit_code: None,
                    duration_ms: None,
                },
                serde_json::Value::Null,
                Severity::Info,
                None,
            );
            apply_tags(&mut tool_obs, ctx);
            if let Some(ns) = ts_ns {
                tool_obs.timestamp_ns = ns;
            }

            let mut custom_obs = Observation::new(
                origin,
                ObservationKind::Custom {
                    channel: "conversation.tool_use".into(),
                },
                serde_json::json!({
                    "tool": tool,
                    "input": input,
                }),
                Severity::Info,
                None,
            );
            apply_tags(&mut custom_obs, ctx);
            if let Some(ns) = ts_ns {
                custom_obs.timestamp_ns = ns;
            }

            vec![tool_obs, custom_obs]
        }
        ConversationEvent::ToolResult {
            call_id,
            output,
            exit_code,
            timestamp,
        } => {
            let mut obs = Observation::new(
                origin,
                ObservationKind::Custom {
                    channel: "conversation.tool_result".into(),
                },
                serde_json::json!({
                    "call_id": call_id,
                    "output": output,
                    "exit_code": exit_code,
                }),
                Severity::Info,
                None,
            );
            apply_tags(&mut obs, ctx);
            apply_timestamp(&mut obs, timestamp.as_deref());
            vec![obs]
        }
        ConversationEvent::SessionMeta {
            session_id,
            cwd,
            provider,
            model,
        } => {
            let mut obs = Observation::new(
                origin,
                ObservationKind::Custom {
                    channel: "conversation.session_meta".into(),
                },
                serde_json::json!({
                    "session_id": session_id,
                    "cwd": cwd,
                    "provider": provider,
                    "model": model,
                }),
                Severity::Info,
                None,
            );
            apply_tags(&mut obs, ctx);
            obs.session_id = Some(session_id.as_str().into());
            vec![obs]
        }
        ConversationEvent::UserPrompt { text, timestamp } => {
            let mut obs = Observation::new(
                origin,
                ObservationKind::Custom {
                    channel: "conversation.user_prompt".into(),
                },
                serde_json::json!({ "text": text }),
                Severity::Info,
                None,
            );
            apply_tags(&mut obs, ctx);
            apply_timestamp(&mut obs, timestamp.as_deref());
            vec![obs]
        }
        ConversationEvent::TurnMeta {
            model,
            git_branch,
            git_sha,
            tokens,
            duration_ms,
            permission_mode,
            cli_version,
        } => {
            let mut obs = Observation::new(
                origin,
                ObservationKind::Custom {
                    channel: "conversation.turn_meta".into(),
                },
                serde_json::json!({
                    "model": model,
                    "git_branch": git_branch,
                    "git_sha": git_sha,
                    "tokens": tokens,
                    "duration_ms": duration_ms,
                    "permission_mode": permission_mode,
                    "cli_version": cli_version,
                }),
                Severity::Debug,
                None,
            );
            apply_tags(&mut obs, ctx);
            vec![obs]
        }
        ConversationEvent::AgentSpawn {
            parent_session,
            child_session,
            role,
            nickname,
            status,
        } => {
            let mut obs = Observation::new(
                origin,
                ObservationKind::Custom {
                    channel: "conversation.agent_spawn".into(),
                },
                serde_json::json!({
                    "parent_session": parent_session,
                    "child_session": child_session,
                    "role": role,
                    "nickname": nickname,
                    "status": status,
                }),
                Severity::Info,
                None,
            );
            apply_tags(&mut obs, ctx);
            vec![obs]
        }
        ConversationEvent::FileChange { path, timestamp } => {
            let mut obs = Observation::new(
                origin,
                ObservationKind::Custom {
                    channel: "conversation.file_change".into(),
                },
                serde_json::json!({ "path": path }),
                Severity::Debug,
                None,
            );
            apply_tags(&mut obs, ctx);
            apply_timestamp(&mut obs, timestamp.as_deref());
            vec![obs]
        }
        ConversationEvent::RawEvent {
            line_type,
            timestamp,
        } => {
            let mut obs = Observation::new(
                origin,
                ObservationKind::Custom {
                    channel: format!("conversation.{line_type}"),
                },
                serde_json::Value::Null,
                Severity::Trace,
                None,
            );
            apply_tags(&mut obs, ctx);
            apply_timestamp(&mut obs, timestamp.as_deref());
            vec![obs]
        }
    }
}

fn apply_tags(obs: &mut Observation, ctx: &TailContext) {
    if !ctx.tags.is_empty() {
        obs.tags = Some(ctx.tags.clone());
    }
}

fn apply_timestamp(obs: &mut Observation, ts: Option<&str>) {
    if let Some(ns) = resolve_timestamp(ts) {
        obs.timestamp_ns = ns;
    }
}

fn resolve_timestamp(ts: Option<&str>) -> Option<u64> {
    ts.and_then(daemon8_parse::timestamp::normalize_timestamp_ns)
        .and_then(|ns| u64::try_from(ns).ok())
}

fn expand_glob(pattern: &str) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in glob::glob(pattern)? {
        match entry {
            Ok(path) if path.is_file() => paths.push(path),
            Ok(_) => {}
            Err(e) => tracing::warn!(pattern, "glob entry error: {e}"),
        }
    }
    Ok(paths)
}

fn collect_watch_dirs(pattern: &str) -> Vec<PathBuf> {
    let path = Path::new(pattern);
    let mut dir = path.parent().map(|p| p.to_path_buf());

    while let Some(ref d) = dir {
        let s = d.to_string_lossy();
        if s.contains('*') || s.contains('?') || s.contains('[') {
            dir = d.parent().map(|p| p.to_path_buf());
        } else {
            break;
        }
    }

    match dir {
        Some(d) if !d.as_os_str().is_empty() => vec![d],
        _ => vec![],
    }
}

fn path_matches_glob(path: &Path, pattern: &str) -> bool {
    let Ok(matcher) = glob::Pattern::new(pattern) else {
        return false;
    };
    matcher.matches_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    fn test_ctx(
        origin: &str,
        provider: &str,
        tags: Vec<String>,
        obs_tx: mpsc::UnboundedSender<Observation>,
    ) -> Arc<TailContext> {
        Arc::new(TailContext {
            origin_name: origin.into(),
            provider_id: provider.into(),
            tags,
            obs_tx,
        })
    }

    fn drain_observations(rx: &mut mpsc::UnboundedReceiver<Observation>) -> Vec<Observation> {
        let mut received = Vec::new();
        while let Ok(obs) = rx.try_recv() {
            received.push(obs);
        }
        received
    }

    #[tokio::test]
    async fn conversation_source_emits_tool_calls() {
        let dir = tempdir().unwrap();
        let convo_file = dir.path().join("test-session.jsonl");
        std::fs::write(&convo_file, "").unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let mut tasks = JoinSet::new();

        let ctx = test_ctx("test-convo", "claude", vec!["ai-session".into()], tx);
        spawn_tail(&mut tasks, convo_file.clone(), &ctx, cancel.clone(), false);

        tokio::time::sleep(Duration::from_millis(100)).await;

        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&convo_file)
                .unwrap();
            writeln!(f, r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{{"role":"assistant","model":"claude-opus-4-6","content":[{{"type":"tool_use","id":"toolu_1","name":"Bash","input":{{"command":"ls"}}}}]}}}}"#).unwrap();
        }

        tokio::time::sleep(Duration::from_millis(500)).await;

        let received = drain_observations(&mut rx);
        let tool_calls: Vec<_> = received
            .iter()
            .filter(|o| matches!(&o.kind, ObservationKind::ToolCall { .. }))
            .collect();
        assert_eq!(tool_calls.len(), 1, "expected 1 ToolCall observation");
        match &tool_calls[0].kind {
            ObservationKind::ToolCall { tool, input, .. } => {
                assert_eq!(tool, "Bash");
                assert_eq!(input["command"], "ls");
            }
            _ => unreachable!(),
        }
        assert!(
            tool_calls[0]
                .tags
                .as_ref()
                .is_some_and(|t| t.contains(&"ai-session".to_string()))
        );

        let custom_tool: Vec<_> = received
            .iter()
            .filter(|o| matches!(&o.kind, ObservationKind::Custom { channel } if channel == "conversation.tool_use"))
            .collect();
        assert_eq!(
            custom_tool.len(),
            1,
            "expected 1 Custom conversation.tool_use observation"
        );

        cancel.cancel();
        while tasks.join_next().await.is_some() {}
    }

    #[tokio::test]
    async fn parallel_tool_calls_emit_multiple_observations() {
        let dir = tempdir().unwrap();
        let convo_file = dir.path().join("test-session.jsonl");
        std::fs::write(&convo_file, "").unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let mut tasks = JoinSet::new();

        let ctx = test_ctx("test", "claude", vec![], tx);
        spawn_tail(&mut tasks, convo_file.clone(), &ctx, cancel.clone(), false);

        tokio::time::sleep(Duration::from_millis(100)).await;

        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&convo_file)
                .unwrap();
            writeln!(f, r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"t1","name":"Read","input":{{"file":"a.rs"}}}},{{"type":"tool_use","id":"t2","name":"Bash","input":{{"command":"ls"}}}}]}}}}"#).unwrap();
        }

        tokio::time::sleep(Duration::from_millis(500)).await;

        let received = drain_observations(&mut rx);
        let tool_calls: Vec<_> = received
            .iter()
            .filter(|o| matches!(&o.kind, ObservationKind::ToolCall { .. }))
            .collect();
        assert_eq!(
            tool_calls.len(),
            2,
            "expected 2 ToolCall observations for parallel tool calls"
        );

        cancel.cancel();
        while tasks.join_next().await.is_some() {}
    }

    #[tokio::test]
    async fn all_event_types_produce_observations() {
        let dir = tempdir().unwrap();
        let convo_file = dir.path().join("test-session.jsonl");
        std::fs::write(&convo_file, "").unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let mut tasks = JoinSet::new();

        let ctx = test_ctx("test", "claude", vec!["conversation".into()], tx);
        spawn_tail(&mut tasks, convo_file.clone(), &ctx, cancel.clone(), false);

        tokio::time::sleep(Duration::from_millis(100)).await;

        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&convo_file)
                .unwrap();
            writeln!(
                f,
                r#"{{"type":"permission-mode","permissionMode":"auto","sessionId":"abc"}}"#
            )
            .unwrap();
            writeln!(f, r#"{{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{{"role":"user","content":[{{"type":"text","text":"fix it"}}]}}}}"#).unwrap();
            writeln!(f, r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","version":"2.1.0","gitBranch":"main","message":{{"role":"assistant","model":"claude-opus-4-6","content":[{{"type":"tool_use","id":"t1","name":"Bash","input":{{"command":"ls"}}}}]}}}}"#).unwrap();
            writeln!(f, r#"{{"type":"ai-title","title":"test session"}}"#).unwrap();
        }

        tokio::time::sleep(Duration::from_millis(500)).await;

        let received = drain_observations(&mut rx);

        let channels: Vec<String> = received
            .iter()
            .filter_map(|o| match &o.kind {
                ObservationKind::Custom { channel } => Some(channel.clone()),
                ObservationKind::ToolCall { .. } => Some("tool_call".into()),
                _ => None,
            })
            .collect();

        assert!(
            channels.contains(&"conversation.session_meta".to_string()),
            "missing session_meta; got: {channels:?}"
        );
        assert!(
            channels.contains(&"conversation.user_prompt".to_string()),
            "missing user_prompt; got: {channels:?}"
        );
        assert!(
            channels.contains(&"tool_call".to_string()),
            "missing tool_call; got: {channels:?}"
        );
        assert!(
            channels.contains(&"conversation.tool_use".to_string()),
            "missing conversation.tool_use; got: {channels:?}"
        );
        assert!(
            channels.contains(&"conversation.turn_meta".to_string()),
            "missing turn_meta; got: {channels:?}"
        );
        assert!(
            channels.contains(&"conversation.ai-title".to_string()),
            "missing ai-title raw event; got: {channels:?}"
        );

        let session_obs = received
            .iter()
            .find(|o| {
                matches!(&o.kind, ObservationKind::Custom { channel } if channel == "conversation.session_meta")
            })
            .unwrap();
        assert_eq!(session_obs.session_id.as_deref(), Some("abc"));

        cancel.cancel();
        while tasks.join_next().await.is_some() {}
    }

    #[tokio::test]
    async fn max_initial_files_guard() {
        let dir = tempdir().unwrap();
        let convo_dir = dir.path().join("sessions");
        std::fs::create_dir_all(&convo_dir).unwrap();

        for i in 0..60 {
            std::fs::write(convo_dir.join(format!("session-{i:03}.jsonl")), "").unwrap();
        }

        let mut files = expand_glob(&convo_dir.join("*.jsonl").to_string_lossy()).unwrap();

        assert_eq!(files.len(), 60);

        if files.len() > MAX_INITIAL_FILES {
            files.sort_by_key(|p| {
                std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            });
            files.reverse();
            files.truncate(MAX_INITIAL_FILES);
        }
        assert_eq!(files.len(), MAX_INITIAL_FILES);
    }
}
