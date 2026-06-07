// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::{VecDeque, hash_map::DefaultHasher};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use daemon8_core::project_config::{
    FileSource, ProjectConfig, ProjectSource, resolve_project_source_path,
};
use daemon8_parse::{ParsedLine, timestamp::normalize_timestamp_ns};
use daemon8_store::{CursorState, CursorStore};
use daemon8_types::{
    AppName, Filter, Observation, ObservationKind, ObservationKindTag, Origin, OriginPattern,
    SYSTEM_TAG, Severity, SourceLocation,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const MAX_LINES_PER_TRIGGER: usize = 500;
const MAX_BYTES_PER_TRIGGER: u64 = 256 * 1024;
const CURSOR_MARKER_BYTES: u64 = 256;

#[derive(Debug)]
pub struct ReadThroughResult {
    pub observations: Vec<Observation>,
    pub errors: Vec<ReadThroughError>,
}

#[derive(Debug)]
pub struct ReadThroughError {
    pub source_id: String,
    pub code: String,
    pub message: String,
}

pub async fn read_through_file_sources(
    scope_root: &Path,
    config: &ProjectConfig,
    filter: &Filter,
    cursors: &dyn CursorStore,
) -> ReadThroughResult {
    let derived_tags = config.derived_tags();
    let scope_root_str = scope_root.display().to_string();
    let mut observations = Vec::new();
    let mut errors = Vec::new();

    for source in &config.sources {
        let ProjectSource::File(file_source) = source else {
            continue;
        };

        let resolved_path = match resolve_project_source_path(config, source) {
            Ok(path) => path,
            Err(err) => {
                errors.push(ReadThroughError {
                    source_id: file_source.id.clone(),
                    code: "invalid_source_path".into(),
                    message: err.to_string(),
                });
                continue;
            }
        };

        let canonical_path = match canonical_source_path(&resolved_path) {
            Ok(path) => path,
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => {
                errors.push(ReadThroughError {
                    source_id: file_source.id.clone(),
                    code: "read_failed".into(),
                    message: err.to_string(),
                });
                continue;
            }
        };
        let canonical_path_str = canonical_path.display().to_string();

        if !should_read_source(file_source, &canonical_path_str, filter) {
            continue;
        }

        let fingerprint = match source_file_fingerprint(&canonical_path) {
            Ok(fp) => fp,
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => {
                errors.push(ReadThroughError {
                    source_id: file_source.id.clone(),
                    code: "read_failed".into(),
                    message: err.to_string(),
                });
                continue;
            }
        };

        let cursor_position = match cursors
            .get_cursor(&scope_root_str, &file_source.id, &canonical_path_str)
            .await
        {
            Ok(cursor) => cursor
                .as_ref()
                .and_then(|c| valid_cursor_position(c, &canonical_path, &fingerprint)),
            Err(err) => {
                errors.push(ReadThroughError {
                    source_id: file_source.id.clone(),
                    code: "cursor_lookup_failed".into(),
                    message: err.to_string(),
                });
                continue;
            }
        };

        let window = match read_complete_window(&canonical_path, cursor_position) {
            Ok(window) => window,
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => {
                errors.push(ReadThroughError {
                    source_id: file_source.id.clone(),
                    code: "read_failed".into(),
                    message: err.to_string(),
                });
                continue;
            }
        };

        let parser = match daemon8_parse::resolve_parser_with_pattern(
            &file_source.parser,
            file_source.parser_pattern.as_deref(),
        ) {
            Ok(parser) => parser,
            Err(err) => {
                errors.push(ReadThroughError {
                    source_id: file_source.id.clone(),
                    code: "invalid_parser".into(),
                    message: err.to_string(),
                });
                continue;
            }
        };

        for line in &window.lines {
            let Some(parsed) = parser.parse(&line.text) else {
                continue;
            };
            let obs = file_observation(file_source, &canonical_path, parsed, &derived_tags);
            if filter.matches(&obs) {
                observations.push(obs);
            }
        }

        let marker = source_cursor_marker(&canonical_path, window.next_position)
            .ok()
            .flatten();
        if let Err(err) = cursors
            .upsert_cursor(CursorState {
                id: None,
                scope_root: scope_root_str.clone(),
                source: file_source.id.clone(),
                source_instance: canonical_path_str.clone(),
                position: window.next_position,
                updated_at: current_ns(),
                metadata: Some(json!({
                    "reader": "read_through",
                    "file": fingerprint,
                    "marker": marker,
                    "max_lines": MAX_LINES_PER_TRIGGER,
                    "max_bytes": MAX_BYTES_PER_TRIGGER
                })),
            })
            .await
        {
            tracing::warn!(
                source = %file_source.id,
                "read-through cursor upsert failed: {err}"
            );
        }
    }

    ReadThroughResult {
        observations,
        errors,
    }
}

fn should_read_source(source: &FileSource, canonical_path: &str, filter: &Filter) -> bool {
    if let Some(ref sources) = filter.source
        && !sources.is_empty()
        && !sources.iter().any(|s| s == &source.id)
    {
        return false;
    }

    if let Some(ref instances) = filter.source_instance
        && !instances.is_empty()
        && !instances.iter().any(|i| i == canonical_path)
    {
        return false;
    }

    if let Some(ref services) = filter.service
        && !services.is_empty()
        && !services.iter().any(|s| s == &source.service)
    {
        return false;
    }

    if let Some(ref origins) = filter.origins
        && !origins.is_empty()
        && origins.iter().all(|o| {
            matches!(
                o,
                OriginPattern::AnyBrowser
                    | OriginPattern::BrowserTab(_)
                    | OriginPattern::AnyDevice
                    | OriginPattern::DeviceSerial(_)
            )
        })
    {
        return false;
    }

    if let Some(ref kinds) = filter.kinds
        && !kinds.is_empty()
        && !kinds
            .iter()
            .any(|k| matches!(k, ObservationKindTag::Log | ObservationKindTag::Custom))
    {
        return false;
    }

    true
}

struct ObservationStamp<'a> {
    service: &'a str,
    source: &'a str,
    path: &'a Path,
    tags: Vec<String>,
    project_tags: &'a [String],
    kind: ObservationKind,
    data: Value,
    severity: Severity,
    source_timestamp: Option<&'a str>,
}

fn file_observation(
    source: &FileSource,
    path: &Path,
    parsed: ParsedLine,
    project_tags: &[String],
) -> Observation {
    let source_timestamp = parsed.timestamp.as_deref();
    let mut data = Value::Object(parsed.fields);
    data["message"] = Value::String(parsed.message);
    if let Some(ref ts) = parsed.timestamp {
        data["timestamp"] = Value::String(ts.clone());
    }
    data["parser"] = Value::String(source.parser.clone());
    let kind = parsed
        .channel
        .map(|channel| ObservationKind::Custom { channel })
        .unwrap_or(ObservationKind::Log);
    stamped_observation(ObservationStamp {
        service: &source.service,
        source: &source.id,
        path,
        tags: source.tags.clone(),
        project_tags,
        kind,
        data,
        severity: parsed.severity.unwrap_or(Severity::Info),
        source_timestamp,
    })
}

fn stamped_observation(input: ObservationStamp<'_>) -> Observation {
    let ObservationStamp {
        service,
        source,
        path,
        mut tags,
        project_tags,
        kind,
        data,
        severity,
        source_timestamp,
    } = input;
    tags.retain(|tag| tag != SYSTEM_TAG);
    let mut all_tags = project_tags.to_vec();
    all_tags.push(format!("source:{source}"));
    all_tags.append(&mut tags);
    all_tags.sort();
    all_tags.dedup();
    let mut obs = Observation::new(
        Origin::Application {
            name: AppName::from(service),
        },
        kind,
        data,
        severity,
        Some(SourceLocation {
            file: path.display().to_string(),
            line: 0,
            function: None,
        }),
    );
    if let Some(timestamp_ns) = source_timestamp.and_then(parsed_timestamp_ns) {
        obs.timestamp_ns = timestamp_ns;
    }
    obs.service = Some(Arc::from(service));
    obs.source = Some(Arc::from(source));
    obs.source_instance = Some(Arc::from(path.display().to_string()));
    if !all_tags.is_empty() {
        obs.tags = Some(all_tags);
    }
    obs
}

fn parsed_timestamp_ns(raw: &str) -> Option<u64> {
    normalize_timestamp_ns(raw).and_then(|ns| u64::try_from(ns).ok())
}

#[derive(Debug)]
struct ReadWindow {
    lines: Vec<CompleteLine>,
    next_position: u64,
}

#[derive(Debug)]
struct CompleteLine {
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SourceFileFingerprint {
    len: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    modified_ns: Option<u64>,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SourceCursorMarker {
    position: u64,
    window_start: u64,
    hash: String,
}

fn canonical_source_path(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

fn source_file_fingerprint(path: &Path) -> std::io::Result<SourceFileFingerprint> {
    let metadata = std::fs::metadata(path)?;
    Ok(SourceFileFingerprint {
        len: metadata.len(),
        modified_ns: metadata.modified().ok().and_then(system_time_ns),
        #[cfg(unix)]
        dev: {
            use std::os::unix::fs::MetadataExt;
            metadata.dev()
        },
        #[cfg(unix)]
        ino: {
            use std::os::unix::fs::MetadataExt;
            metadata.ino()
        },
    })
}

fn system_time_ns(time: std::time::SystemTime) -> Option<u64> {
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos() as u64)
}

fn source_cursor_marker(path: &Path, position: u64) -> std::io::Result<Option<SourceCursorMarker>> {
    if position == 0 {
        return Ok(None);
    }

    let window_start = position.saturating_sub(CURSOR_MARKER_BYTES);
    let window_len = position - window_start;
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(window_start))?;
    let mut bytes = Vec::with_capacity(window_len as usize);
    file.take(window_len).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != window_len {
        return Ok(None);
    }

    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(Some(SourceCursorMarker {
        position,
        window_start,
        hash: format!("{:016x}", hasher.finish()),
    }))
}

fn valid_cursor_position(
    cursor: &CursorState,
    path: &Path,
    fingerprint: &SourceFileFingerprint,
) -> Option<u64> {
    if cursor.position > fingerprint.len {
        return None;
    }
    let metadata = cursor.metadata.as_ref()?;
    let stored = metadata
        .get("file")
        .and_then(|value| serde_json::from_value::<SourceFileFingerprint>(value.clone()).ok())?;
    if !same_source_file(&stored, fingerprint) {
        return None;
    }
    if cursor.position == 0 {
        return Some(0);
    }

    let stored_marker = metadata
        .get("marker")
        .and_then(|value| serde_json::from_value::<SourceCursorMarker>(value.clone()).ok())?;
    if stored_marker.position != cursor.position {
        return None;
    }
    let current_marker = source_cursor_marker(path, cursor.position).ok().flatten()?;
    if stored_marker == current_marker {
        Some(cursor.position)
    } else {
        None
    }
}

#[cfg(unix)]
fn same_source_file(stored: &SourceFileFingerprint, current: &SourceFileFingerprint) -> bool {
    stored.dev == current.dev && stored.ino == current.ino
}

#[cfg(not(unix))]
fn same_source_file(stored: &SourceFileFingerprint, current: &SourceFileFingerprint) -> bool {
    stored.modified_ns == current.modified_ns
}

fn read_complete_window(path: &Path, cursor_position: Option<u64>) -> std::io::Result<ReadWindow> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let cursored = cursor_position.is_some_and(|pos| pos <= file_len);
    let start = match cursor_position {
        Some(pos) if pos <= file_len => pos,
        _ => file_len.saturating_sub(MAX_BYTES_PER_TRIGGER),
    };
    let end = (start + MAX_BYTES_PER_TRIGGER).min(file_len);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((end - start) as usize);
    file.take(end - start).read_to_end(&mut bytes)?;
    Ok(collect_complete_lines(
        &bytes,
        start,
        !cursored && start > 0,
        cursored,
    ))
}

fn collect_complete_lines(
    bytes: &[u8],
    base_offset: u64,
    drop_leading_partial: bool,
    stop_at_limit: bool,
) -> ReadWindow {
    let mut cursor = 0usize;
    if drop_leading_partial && !bytes.is_empty() {
        if let Some(pos) = bytes.iter().position(|byte| *byte == b'\n') {
            cursor = pos + 1;
        } else {
            return ReadWindow {
                lines: Vec::new(),
                next_position: base_offset,
            };
        }
    }

    let mut lines = VecDeque::new();
    let mut line_start = cursor;
    let mut next_position = base_offset + cursor as u64;
    while cursor < bytes.len() {
        if bytes[cursor] == b'\n' {
            let mut line = &bytes[line_start..cursor];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            if lines.len() == MAX_LINES_PER_TRIGGER {
                if stop_at_limit {
                    break;
                }
                lines.pop_front();
            }
            lines.push_back(CompleteLine {
                text: String::from_utf8_lossy(line).into_owned(),
            });
            cursor += 1;
            next_position = base_offset + cursor as u64;
            line_start = cursor;
            continue;
        }
        cursor += 1;
    }

    ReadWindow {
        lines: Vec::from(lines),
        next_position,
    }
}

fn current_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use daemon8_core::project_config::parse_project_config_file;
    use daemon8_store::SurrealStore;

    fn write_test_config(root: &Path, source_yaml: &str) {
        let dir = root.join(".daemon8");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.md"),
            format!(
                r#"---
daemon8_schema: 1
created_at: "2026-05-17T00:00:00Z"
updated_at: "2026-05-17T00:00:00Z"
project:
  name: read-through-test
  stack:
    languages: [rust]
    frameworks: [tokio]
    tools: [cargo]
vars:
  PRJ_ROOT: "{}"
sources:
{source_yaml}
---
# daemon8
"#,
                root.display()
            ),
        )
        .unwrap();
    }

    fn load_test_config(root: &Path) -> ProjectConfig {
        parse_project_config_file(&root.join(".daemon8/config.md")).unwrap()
    }

    #[tokio::test]
    async fn read_through_parses_file_observations() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("app.log"), "first\nsecond\nthird\n").unwrap();
        write_test_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    parser: line
    path: "$PRJ_ROOT/app.log"
    tags: [runtime]
"#,
        );
        let config = load_test_config(tmp.path());
        let store = SurrealStore::memory().await.unwrap();
        let cursors = store.cursor_store();

        let result =
            read_through_file_sources(tmp.path(), &config, &Filter::default(), &cursors).await;

        assert!(
            result.errors.is_empty(),
            "errors: {:?}",
            result.errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
        assert_eq!(result.observations.len(), 3);
        assert_eq!(result.observations[0].service.as_deref(), Some("app"));
        assert_eq!(result.observations[0].source.as_deref(), Some("app.logs"));
        let canonical_log = std::fs::canonicalize(tmp.path().join("app.log")).unwrap();
        assert_eq!(
            result.observations[0].source_instance.as_deref(),
            Some(canonical_log.display().to_string().as_str())
        );
        assert!(
            result.observations[0]
                .tags
                .as_ref()
                .unwrap()
                .contains(&"runtime".into())
        );
    }

    #[tokio::test]
    async fn read_through_advances_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("app.log");
        std::fs::write(&log_path, "line-one\n").unwrap();
        write_test_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    parser: line
    path: "$PRJ_ROOT/app.log"
"#,
        );
        let config = load_test_config(tmp.path());
        let store = SurrealStore::memory().await.unwrap();
        let cursors = store.cursor_store();
        let filter = Filter::default();

        let r1 = read_through_file_sources(tmp.path(), &config, &filter, &cursors).await;
        assert_eq!(r1.observations.len(), 1);

        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .unwrap();
        writeln!(f, "line-two").unwrap();

        let r2 = read_through_file_sources(tmp.path(), &config, &filter, &cursors).await;
        assert_eq!(r2.observations.len(), 1);
        assert!(
            r2.observations[0].data.to_string().contains("line-two"),
            "second read should return only the new line"
        );
    }

    #[tokio::test]
    async fn read_through_skips_excluded_source() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("app.log"), "line\n").unwrap();
        write_test_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    parser: line
    path: "$PRJ_ROOT/app.log"
"#,
        );
        let config = load_test_config(tmp.path());
        let store = SurrealStore::memory().await.unwrap();
        let cursors = store.cursor_store();
        let filter = Filter {
            source: Some(vec!["other.source".into()]),
            ..Default::default()
        };

        let result = read_through_file_sources(tmp.path(), &config, &filter, &cursors).await;
        assert_eq!(result.observations.len(), 0);
    }

    #[tokio::test]
    async fn read_through_skips_browser_only_filter() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("app.log"), "line\n").unwrap();
        write_test_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    parser: line
    path: "$PRJ_ROOT/app.log"
"#,
        );
        let config = load_test_config(tmp.path());
        let store = SurrealStore::memory().await.unwrap();
        let cursors = store.cursor_store();
        let filter = Filter {
            origins: Some(vec![OriginPattern::AnyBrowser]),
            ..Default::default()
        };

        let result = read_through_file_sources(tmp.path(), &config, &filter, &cursors).await;
        assert_eq!(result.observations.len(), 0);
    }

    #[tokio::test]
    async fn read_through_applies_severity_filter() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("app.log"),
            "{\"severity\":\"info\",\"message\":\"hello\"}\n{\"severity\":\"warn\",\"message\":\"danger\"}\n",
        )
        .unwrap();
        write_test_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    parser: json
    path: "$PRJ_ROOT/app.log"
"#,
        );
        let config = load_test_config(tmp.path());
        let store = SurrealStore::memory().await.unwrap();
        let cursors = store.cursor_store();
        let filter = Filter {
            severity_min: Some(Severity::Warn),
            ..Default::default()
        };

        let result = read_through_file_sources(tmp.path(), &config, &filter, &cursors).await;
        assert_eq!(result.observations.len(), 1);
        assert!(result.observations[0].data.to_string().contains("danger"));
    }

    #[tokio::test]
    async fn read_through_skips_missing_file_sources() {
        let tmp = tempfile::tempdir().unwrap();
        write_test_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    parser: line
    path: "$PRJ_ROOT/missing.log"
"#,
        );
        let config = load_test_config(tmp.path());
        let store = SurrealStore::memory().await.unwrap();
        let cursors = store.cursor_store();

        let result =
            read_through_file_sources(tmp.path(), &config, &Filter::default(), &cursors).await;

        assert_eq!(result.observations.len(), 0);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn read_through_missing_file_does_not_block_present_sources() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("app.log"), "line\n").unwrap();
        write_test_config(
            tmp.path(),
            r#"  - id: missing.logs
    service: missing
    kind: file
    parser: line
    path: "$PRJ_ROOT/missing.log"
  - id: app.logs
    service: app
    kind: file
    parser: line
    path: "$PRJ_ROOT/app.log"
"#,
        );
        let config = load_test_config(tmp.path());
        let store = SurrealStore::memory().await.unwrap();
        let cursors = store.cursor_store();

        let result =
            read_through_file_sources(tmp.path(), &config, &Filter::default(), &cursors).await;

        assert!(result.errors.is_empty());
        assert_eq!(result.observations.len(), 1);
        assert_eq!(result.observations[0].source.as_deref(), Some("app.logs"));
    }

    #[tokio::test]
    async fn read_through_surfaces_present_directory_read_errors() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("app.log")).unwrap();
        write_test_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    parser: line
    path: "$PRJ_ROOT/app.log"
"#,
        );
        let config = load_test_config(tmp.path());
        let store = SurrealStore::memory().await.unwrap();
        let cursors = store.cursor_store();

        let result =
            read_through_file_sources(tmp.path(), &config, &Filter::default(), &cursors).await;

        assert_eq!(result.observations.len(), 0);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].source_id, "app.logs");
        assert_eq!(result.errors[0].code, "read_failed");
    }

    #[tokio::test]
    async fn read_through_handles_bad_parser() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("app.log"), "line\n").unwrap();
        write_test_config(
            tmp.path(),
            r#"  - id: app.logs
    service: app
    kind: file
    parser: nonexistent
    path: "$PRJ_ROOT/app.log"
"#,
        );
        let config = load_test_config(tmp.path());
        let store = SurrealStore::memory().await.unwrap();
        let cursors = store.cursor_store();

        let result =
            read_through_file_sources(tmp.path(), &config, &Filter::default(), &cursors).await;

        assert_eq!(result.observations.len(), 0);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].code, "invalid_parser");
    }

    #[test]
    fn complete_line_window_leaves_unterminated_tail_pending() {
        let window = collect_complete_lines(b"one\ntwo\nthree", 0, false, false);
        assert_eq!(window.lines.len(), 2);
        assert_eq!(window.next_position, 8);
        assert_eq!(window.lines[1].text, "two");
    }

    #[test]
    fn tailing_keeps_last_500_lines() {
        let input = (0..600)
            .map(|n| format!("{n}\n"))
            .collect::<Vec<_>>()
            .join("");
        let window = collect_complete_lines(input.as_bytes(), 0, false, false);
        assert_eq!(window.lines.len(), 500);
        assert_eq!(window.lines[0].text, "100");
        assert_eq!(window.lines[499].text, "599");
    }

    #[test]
    fn cursored_stops_at_500_lines_without_dropping() {
        let input = (0..600)
            .map(|n| format!("{n}\n"))
            .collect::<Vec<_>>()
            .join("");
        let window = collect_complete_lines(input.as_bytes(), 0, false, true);
        assert_eq!(window.lines.len(), 500);
        assert_eq!(window.lines[0].text, "0");
        assert_eq!(window.lines[499].text, "499");
        let expected_pos: u64 = (0..500).map(|n| format!("{n}\n").len() as u64).sum();
        assert_eq!(window.next_position, expected_pos);
    }
}
