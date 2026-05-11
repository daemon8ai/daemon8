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

use daemon8_parse::Parser;
use daemon8_parse::timestamp::normalize_timestamp_ns;
use daemon8_types::{AppName, Observation, ObservationKind, Origin, Severity};

use crate::config::FileSourceConfig;

const POLL_INTERVAL: Duration = Duration::from_millis(250);

struct TailContext {
    origin_name: String,
    tags: Vec<String>,
    parser: Arc<dyn Parser>,
    obs_tx: mpsc::UnboundedSender<Observation>,
}

pub(crate) fn spawn_file_source(
    tasks: &mut JoinSet<()>,
    name: String,
    cfg: FileSourceConfig,
    obs_tx: mpsc::UnboundedSender<Observation>,
    cancel: CancellationToken,
) {
    let parser = match daemon8_parse::resolve_parser_with_pattern(
        &cfg.parser,
        cfg.parser_pattern.as_deref(),
    ) {
        Ok(p) => Arc::from(p),
        Err(e) => {
            tracing::error!(source = %name, parser = %cfg.parser, "failed to resolve parser: {e}");
            return;
        }
    };

    tasks.spawn(async move {
        if let Err(e) = run_file_source(name.clone(), cfg, obs_tx, parser, cancel).await {
            tracing::error!(source = %name, "file source exited with error: {e}");
        }
    });
}

async fn run_file_source(
    name: String,
    cfg: FileSourceConfig,
    obs_tx: mpsc::UnboundedSender<Observation>,
    parser: Arc<dyn Parser>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let ctx = Arc::new(TailContext {
        origin_name: name.clone(),
        tags: cfg.tags.clone(),
        parser,
        obs_tx,
    });

    let initial_files = expand_glob(&cfg.path)?;
    if initial_files.is_empty() {
        tracing::debug!(source = %name, pattern = %cfg.path, "no files matched glob on startup");
    }

    let mut tails: JoinSet<()> = JoinSet::new();
    let mut watched: HashSet<PathBuf> = HashSet::new();

    for path in &initial_files {
        watched.insert(path.clone());
        spawn_tail(&mut tails, path.clone(), &ctx, cancel.clone(), true);
    }

    let parent_dirs = collect_watch_dirs(&cfg.path);
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
    for dir in &parent_dirs {
        if dir.exists()
            && let Err(e) = watcher.watch(dir, notify::RecursiveMode::NonRecursive)
        {
            tracing::warn!(source = %name, dir = %dir.display(), "failed to watch directory: {e}");
        }
    }

    let glob_pattern = cfg.path.clone();

    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            event = notify_rx.recv() => {
                let Some(event) = event else { break };
                if let notify::EventKind::Create(_) = event.kind {
                    for path in &event.paths {
                        if !watched.contains(path) && path_matches_glob(path, &glob_pattern) {
                            tracing::debug!(source = %ctx.origin_name, file = %path.display(), "new file detected, starting tail");
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

    // Keep the watcher alive for the duration of the source
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
        if let Err(e) = tail_file(path.clone(), &ctx, cancel, seek_to_end).await {
            tracing::warn!(file = %path.display(), source = %ctx.origin_name, "tail stopped: {e}");
        }
    });
}

async fn tail_file(
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
    let metadata = std::fs::metadata(path)?;
    let file_len = metadata.len();

    if file_len < offset {
        tracing::debug!(file = %path.display(), "file rotation detected, resetting to beginning");
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

        if let Some(parsed) = ctx.parser.parse(line) {
            let severity = parsed.severity.unwrap_or(Severity::Info);

            let mut data = serde_json::Value::Object(parsed.fields);
            data["message"] = serde_json::Value::String(parsed.message.clone());
            if let Some(ref ts) = parsed.timestamp {
                data["timestamp"] = serde_json::Value::String(ts.clone());
            }
            if let Some(ref ch) = parsed.channel {
                data["channel"] = serde_json::Value::String(ch.clone());
            }

            let mut obs = Observation::new(
                Origin::Application {
                    name: AppName::from(ctx.origin_name.as_str()),
                },
                ObservationKind::Log,
                data,
                severity,
                None,
            );

            if let Some(ref ts) = parsed.timestamp
                && let Some(ns) = normalize_timestamp_ns(ts)
            {
                obs.timestamp_ns = ns as u64;
            }

            if !ctx.tags.is_empty() {
                obs.tags = Some(ctx.tags.clone());
            }

            if ctx.obs_tx.send(obs).is_err() {
                break;
            }
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
            Err(e) => {
                tracing::warn!(pattern, "glob entry error: {e}");
            }
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
    use std::collections::BTreeMap;
    use std::io::Write;
    use tempfile::tempdir;

    use crate::config::SourceConfig;

    fn spawn_file_sources(
        tasks: &mut JoinSet<()>,
        sources: &BTreeMap<String, SourceConfig>,
        obs_tx: mpsc::UnboundedSender<Observation>,
        cancel: CancellationToken,
    ) {
        for (name, source) in sources {
            match source {
                SourceConfig::File(cfg) => {
                    spawn_file_source(
                        tasks,
                        name.clone(),
                        cfg.clone(),
                        obs_tx.clone(),
                        cancel.clone(),
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn basic_tail_produces_observations() {
        let dir = tempdir().unwrap();
        let log_file = dir.path().join("test.log");
        std::fs::write(&log_file, "").unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let mut tasks = JoinSet::new();

        let mut sources = BTreeMap::new();
        sources.insert(
            "test-source".to_string(),
            SourceConfig::File(FileSourceConfig {
                path: log_file.to_string_lossy().to_string(),
                parser: "line".to_string(),
                parser_pattern: None,
                tags: vec![],
            }),
        );

        spawn_file_sources(&mut tasks, &sources, tx, cancel.clone());
        tokio::time::sleep(Duration::from_millis(100)).await;

        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&log_file)
                .unwrap();
            writeln!(f, "hello from test").unwrap();
            writeln!(f, "second line").unwrap();
        }

        tokio::time::sleep(Duration::from_millis(500)).await;

        let mut received = Vec::new();
        while let Ok(obs) = rx.try_recv() {
            received.push(obs);
        }

        assert!(
            received.len() >= 2,
            "expected at least 2 observations, got {}",
            received.len()
        );
        assert_eq!(received[0].data["message"], "hello from test");
        assert_eq!(received[1].data["message"], "second line");

        cancel.cancel();
        while tasks.join_next().await.is_some() {}
    }

    #[tokio::test]
    async fn parser_integration_with_severity() {
        let dir = tempdir().unwrap();
        let log_file = dir.path().join("app.log");
        std::fs::write(&log_file, "").unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let mut tasks = JoinSet::new();

        let mut sources = BTreeMap::new();
        sources.insert(
            "json-source".to_string(),
            SourceConfig::File(FileSourceConfig {
                path: log_file.to_string_lossy().to_string(),
                parser: "json".to_string(),
                parser_pattern: None,
                tags: vec!["app".to_string()],
            }),
        );

        spawn_file_sources(&mut tasks, &sources, tx, cancel.clone());
        tokio::time::sleep(Duration::from_millis(100)).await;

        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&log_file)
                .unwrap();
            writeln!(f, r#"{{"message":"db timeout","level":"error"}}"#).unwrap();
        }

        tokio::time::sleep(Duration::from_millis(500)).await;

        let mut received = Vec::new();
        while let Ok(obs) = rx.try_recv() {
            received.push(obs);
        }

        assert!(!received.is_empty(), "expected at least 1 observation");

        let obs = &received[0];
        assert!(
            obs.tags
                .as_ref()
                .is_some_and(|t| t.contains(&"app".to_string())),
            "observation should carry source config tags"
        );

        cancel.cancel();
        while tasks.join_next().await.is_some() {}
    }

    #[tokio::test]
    async fn glob_expansion_only_matches_pattern() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.log"), "line-a\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "line-b\n").unwrap();

        let pattern = dir.path().join("*.log").to_string_lossy().to_string();
        let files = expand_glob(&pattern).unwrap();

        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("a.log"));
    }

    #[tokio::test]
    async fn rotation_detection_resets_to_beginning() {
        let dir = tempdir().unwrap();
        let log_file = dir.path().join("rotating.log");
        std::fs::write(&log_file, "").unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let mut tasks = JoinSet::new();

        let mut sources = BTreeMap::new();
        sources.insert(
            "rotate-test".to_string(),
            SourceConfig::File(FileSourceConfig {
                path: log_file.to_string_lossy().to_string(),
                parser: "line".to_string(),
                parser_pattern: None,
                tags: vec![],
            }),
        );

        spawn_file_sources(&mut tasks, &sources, tx, cancel.clone());
        tokio::time::sleep(Duration::from_millis(100)).await;

        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&log_file)
                .unwrap();
            writeln!(f, "before-rotation").unwrap();
        }
        tokio::time::sleep(Duration::from_millis(500)).await;

        std::fs::write(&log_file, "after-rotation\n").unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;

        let mut received = Vec::new();
        while let Ok(obs) = rx.try_recv() {
            received.push(obs);
        }

        let messages: Vec<&str> = received
            .iter()
            .map(|o| o.data["message"].as_str().unwrap_or(""))
            .collect();

        assert!(
            messages.contains(&"before-rotation"),
            "should have captured pre-rotation line, got: {messages:?}"
        );
        assert!(
            messages.contains(&"after-rotation"),
            "should have captured post-rotation line, got: {messages:?}"
        );

        cancel.cancel();
        while tasks.join_next().await.is_some() {}
    }

    #[tokio::test]
    async fn empty_source_map_spawns_nothing() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let mut tasks = JoinSet::new();

        let sources = BTreeMap::new();
        spawn_file_sources(&mut tasks, &sources, tx, cancel.clone());

        assert!(
            tasks.is_empty(),
            "no tasks should be spawned for empty source map"
        );
        cancel.cancel();
    }

    #[test]
    fn collect_watch_dirs_extracts_parent() {
        let dirs = collect_watch_dirs("/var/log/*.log");
        assert_eq!(dirs, vec![PathBuf::from("/var/log")]);
    }

    #[test]
    fn collect_watch_dirs_handles_double_star() {
        let dirs = collect_watch_dirs("/var/log/**/*.log");
        assert_eq!(dirs, vec![PathBuf::from("/var/log")]);
    }

    #[test]
    fn path_matches_glob_basic() {
        assert!(path_matches_glob(
            Path::new("/var/log/app.log"),
            "/var/log/*.log"
        ));
        assert!(!path_matches_glob(
            Path::new("/var/log/app.txt"),
            "/var/log/*.log"
        ));
    }
}
