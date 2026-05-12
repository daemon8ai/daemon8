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

    let initial_files = expand_glob(&full_glob)?;
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
            match event {
                ConversationEvent::ToolUse {
                    tool,
                    input,
                    timestamp,
                    ..
                } => {
                    let kind = ObservationKind::ToolCall {
                        tool,
                        input,
                        output: None,
                        exit_code: None,
                        duration_ms: None,
                    };
                    let mut obs = Observation::new(
                        Origin::Application {
                            name: AppName::from(ctx.origin_name.as_str()),
                        },
                        kind,
                        serde_json::Value::Null,
                        Severity::Info,
                        None,
                    );
                    if !ctx.tags.is_empty() {
                        obs.tags = Some(ctx.tags.clone());
                    }
                    if let Some(ts) = timestamp
                        && let Some(ns) = daemon8_parse::timestamp::normalize_timestamp_ns(&ts)
                        && let Ok(ns_u64) = u64::try_from(ns)
                    {
                        obs.timestamp_ns = ns_u64;
                    }
                    if ctx.obs_tx.send(obs).is_err() {
                        channel_closed = true;
                        break;
                    }
                }
                ConversationEvent::ToolResult { .. } | ConversationEvent::SessionMeta { .. } => {}
            }
        }

        if channel_closed {
            break;
        }
    }

    Ok(offset)
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
            writeln!(f, r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_1","name":"Bash","input":{{"command":"ls"}}}}]}}}}"#).unwrap();
        }

        tokio::time::sleep(Duration::from_millis(500)).await;

        let mut received = Vec::new();
        while let Ok(obs) = rx.try_recv() {
            received.push(obs);
        }

        assert!(!received.is_empty(), "expected at least 1 observation");
        match &received[0].kind {
            ObservationKind::ToolCall { tool, input, .. } => {
                assert_eq!(tool, "Bash");
                assert_eq!(input["command"], "ls");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
        assert!(
            received[0]
                .tags
                .as_ref()
                .is_some_and(|t| t.contains(&"ai-session".to_string()))
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

        let mut received = Vec::new();
        while let Ok(obs) = rx.try_recv() {
            received.push(obs);
        }

        assert_eq!(
            received.len(),
            2,
            "expected 2 observations for parallel tool calls"
        );
        match &received[0].kind {
            ObservationKind::ToolCall { tool, .. } => assert_eq!(tool, "Read"),
            other => panic!("expected ToolCall, got {other:?}"),
        }
        match &received[1].kind {
            ObservationKind::ToolCall { tool, .. } => assert_eq!(tool, "Bash"),
            other => panic!("expected ToolCall, got {other:?}"),
        }

        cancel.cancel();
        while tasks.join_next().await.is_some() {}
    }

    #[tokio::test]
    async fn non_tool_lines_are_skipped() {
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
            writeln!(
                f,
                r#"{{"type":"permission-mode","permissionMode":"auto","sessionId":"abc"}}"#
            )
            .unwrap();
            writeln!(f, r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"hello"}}]}}}}"#).unwrap();
        }

        tokio::time::sleep(Duration::from_millis(500)).await;

        assert!(
            rx.try_recv().is_err(),
            "non-tool lines should not produce observations"
        );

        cancel.cancel();
        while tasks.join_next().await.is_some() {}
    }
}
