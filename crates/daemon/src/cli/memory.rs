// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use futures::StreamExt;
use serde_json::Value;

use super::observe::{ClientArgs, base_url, check_response, handle_reqwest_error};

const MAX_NDJSON_LINE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Subcommand)]
pub enum MemorySubcommand {
    /// Export memory query rows to one Markdown file per row
    Export(MemoryExportArgs),
}

#[derive(clap::Args, Debug, Clone)]
pub struct MemoryExportArgs {
    /// Read-only SurrealQL SELECT query. Must include ORDER BY for deterministic paging.
    #[arg(long)]
    pub query: String,

    /// Output directory. One Markdown file is written per returned row.
    #[arg(long)]
    pub out: PathBuf,

    /// Rows fetched per bounded daemon API page.
    #[arg(long, default_value_t = daemon8_api::MEMORY_EXPORT_DEFAULT_PAGE_SIZE)]
    pub page_size: u64,

    #[command(flatten)]
    pub client: ClientArgs,
}

pub async fn cmd_memory(
    _config_override: Option<String>,
    subcommand: MemorySubcommand,
) -> Result<()> {
    match subcommand {
        MemorySubcommand::Export(args) => cmd_memory_export(args).await,
    }
}

async fn cmd_memory_export(args: MemoryExportArgs) -> Result<()> {
    validate_export_args(&args)?;
    prepare_output_dir(&args.out)?;

    let port = args.client.resolved_port();
    let url = format!("{}/api/memory/export", base_url(port));
    let client = reqwest::Client::new();
    let resp = match client
        .post(&url)
        .json(&serde_json::json!({
            "query": args.query.trim(),
            "page_size": args.page_size,
        }))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) if e.is_connect() => bail!(
            "daemon8 memory export requires a running daemon API at localhost:{port}. Start with: daemon8 serve"
        ),
        Err(e) => return Err(handle_reqwest_error(e, port)),
    };

    let resp = check_response(resp).await?;
    let exported_at = now_exported_at();
    let query_hash = short_stable_hash(args.query.trim().as_bytes());
    let rows = write_ndjson_response(resp, &args.out, &exported_at, &query_hash).await?;

    println!("exported {rows} markdown files to {}", args.out.display());
    Ok(())
}

fn validate_export_args(args: &MemoryExportArgs) -> Result<()> {
    if let Err(message) = daemon8_api::validate_memory_export_query(&args.query) {
        bail!(message);
    }
    if !(1..=daemon8_api::MEMORY_EXPORT_MAX_PAGE_SIZE).contains(&args.page_size) {
        bail!(
            "page_size must be between 1 and {}",
            daemon8_api::MEMORY_EXPORT_MAX_PAGE_SIZE
        );
    }
    validate_output_target(&args.out)
}

fn validate_output_target(out: &Path) -> Result<()> {
    if out.as_os_str().is_empty() {
        bail!("--out cannot be empty");
    }
    if out.exists() && !out.is_dir() {
        bail!("--out must be a directory, got file {}", out.display());
    }
    if out.exists() && out.read_dir()?.next().is_some() {
        bail!(
            "--out must be a new or empty directory so stale export files cannot be mixed with current results"
        );
    }
    Ok(())
}

fn prepare_output_dir(out: &Path) -> Result<()> {
    validate_output_target(out)?;
    std::fs::create_dir_all(out)
        .with_context(|| format!("creating output directory {}", out.display()))
}

async fn write_ndjson_response(
    resp: reqwest::Response,
    out_dir: &Path,
    exported_at: &str,
    query_hash: &str,
) -> Result<usize> {
    let mut stream = resp.bytes_stream();
    let mut buffer = Vec::<u8>::new();
    let mut row_index = 0usize;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading memory export response chunk")?;
        buffer.extend_from_slice(&chunk);
        drain_complete_lines(
            &mut buffer,
            out_dir,
            exported_at,
            query_hash,
            &mut row_index,
        )?;
    }

    if !buffer.is_empty() {
        write_ndjson_line(&buffer, out_dir, exported_at, query_hash, &mut row_index)?;
    }

    Ok(row_index)
}

fn drain_complete_lines(
    buffer: &mut Vec<u8>,
    out_dir: &Path,
    exported_at: &str,
    query_hash: &str,
    row_index: &mut usize,
) -> Result<()> {
    while let Some(pos) = buffer.iter().position(|byte| *byte == b'\n') {
        let line: Vec<u8> = buffer.drain(..=pos).collect();
        if line.len() > MAX_NDJSON_LINE_BYTES + 1 {
            bail!("memory export row exceeded {} bytes", MAX_NDJSON_LINE_BYTES);
        }
        write_ndjson_line(
            &line[..line.len() - 1],
            out_dir,
            exported_at,
            query_hash,
            row_index,
        )?;
    }
    if buffer.len() > MAX_NDJSON_LINE_BYTES {
        bail!("memory export row exceeded {} bytes", MAX_NDJSON_LINE_BYTES);
    }
    Ok(())
}

fn write_ndjson_line(
    line: &[u8],
    out_dir: &Path,
    exported_at: &str,
    query_hash: &str,
    row_index: &mut usize,
) -> Result<()> {
    if line.is_empty() {
        return Ok(());
    }

    let row: Value = serde_json::from_slice(line).context("parsing memory export row")?;
    *row_index += 1;
    write_row_markdown(out_dir, &row, *row_index, exported_at, query_hash)?;
    Ok(())
}

fn write_row_markdown(
    out_dir: &Path,
    row: &Value,
    row_index: usize,
    exported_at: &str,
    query_hash: &str,
) -> Result<PathBuf> {
    let filename = row_filename(row, row_index)?;
    let path = out_dir.join(filename);
    let markdown = render_row_markdown(row, row_index, exported_at, query_hash)?;
    std::fs::write(&path, markdown).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

fn row_filename(row: &Value, row_index: usize) -> Result<String> {
    let mut parts = Vec::new();
    if let Some(project) = row_string(row, "project_slug") {
        parts.push(project);
    }
    if let Some(kind) = row_string(row, "kind") {
        parts.push(kind);
    }
    if let Some(id) = row.get("id").and_then(scalar_filename_value) {
        parts.push(id);
    }

    let stem = if parts.is_empty() {
        let row_json = serde_json::to_vec(row).unwrap_or_default();
        format!("row-{:06}-{}", row_index, short_stable_hash(&row_json))
    } else {
        let stem = sanitize_filename_stem(parts.join("-"));
        if stem == "row" {
            let row_json = serde_json::to_vec(row).unwrap_or_default();
            format!("row-{:06}-{}", row_index, short_stable_hash(&row_json))
        } else {
            stem
        }
    };

    Ok(format!("{row_index:06}-{stem}.md"))
}

fn row_string(row: &Value, key: &str) -> Option<String> {
    row.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn scalar_filename_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn sanitize_filename_stem(input: String) -> String {
    let mut out = String::new();
    let mut previous_separator = false;

    for ch in input.trim().chars() {
        let next = if ch.is_ascii_alphanumeric() {
            Some(ch.to_ascii_lowercase())
        } else if matches!(ch, '-' | '_' | '.' | ' ' | ':' | '/' | '\\') {
            Some('-')
        } else {
            None
        };

        let Some(next) = next else {
            continue;
        };
        if next == '-' {
            if previous_separator {
                continue;
            }
            previous_separator = true;
        } else {
            previous_separator = false;
        }
        out.push(next);
        if out.len() >= 120 {
            break;
        }
    }

    let out = out.trim_matches(['-', '.', '_']).to_string();
    if out.is_empty() || out == "." || out == ".." {
        "row".into()
    } else {
        out
    }
}

fn render_row_markdown(
    row: &Value,
    row_index: usize,
    exported_at: &str,
    query_hash: &str,
) -> Result<String> {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("exported_at: \"{exported_at}\"\n"));
    out.push_str(&format!("row_index: {row_index}\n"));
    out.push_str(&format!("query_hash: \"{query_hash}\"\n"));
    append_scalar_frontmatter(row, &mut out)?;
    out.push_str("---\n\n");

    if let Some(content) = row.get("content").and_then(Value::as_str) {
        out.push_str(content.trim_end());
        out.push('\n');
    }
    Ok(out)
}

fn append_scalar_frontmatter(row: &Value, out: &mut String) -> Result<()> {
    let Some(object) = row.as_object() else {
        return Ok(());
    };

    let reserved: HashSet<&str> = [
        "content",
        "created_at",
        "exported_at",
        "row_index",
        "query_hash",
        "updated_at",
    ]
    .into_iter()
    .collect();
    for (key, value) in object {
        if reserved.contains(key.as_str()) || !is_frontmatter_key_safe(key) {
            continue;
        }
        if !is_frontmatter_value_safe(value) {
            continue;
        }
        out.push_str(key);
        out.push_str(": ");
        out.push_str(&serde_json::to_string(value).context("rendering frontmatter scalar")?);
        out.push('\n');
    }
    append_memory_timestamp_frontmatter(row, out, "created_at")?;
    append_memory_timestamp_frontmatter(row, out, "updated_at")?;
    Ok(())
}

fn append_memory_timestamp_frontmatter(row: &Value, out: &mut String, key: &str) -> Result<()> {
    let Some(value) = row.get(key).and_then(Value::as_u64) else {
        return Ok(());
    };
    out.push_str(key);
    out.push_str(": ");
    out.push_str(&serde_json::to_string(&memory_timestamp(value)).context("rendering timestamp")?);
    out.push('\n');
    Ok(())
}

fn is_frontmatter_key_safe(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn is_frontmatter_value_safe(value: &Value) -> bool {
    match value {
        Value::Bool(_) | Value::Number(_) => true,
        Value::String(value) => value.len() <= 200 && !value.contains(['\n', '\r']),
        Value::Array(values) => {
            values.len() <= 100
                && values.iter().all(|value| match value {
                    Value::Bool(_) | Value::Number(_) => true,
                    Value::String(value) => value.len() <= 200 && !value.contains(['\n', '\r']),
                    _ => false,
                })
        }
        _ => false,
    }
}

fn now_exported_at() -> String {
    humantime::format_rfc3339_seconds(SystemTime::now()).to_string()
}

fn memory_timestamp(value: u64) -> String {
    let since_epoch = if value > 10_000_000_000 {
        Duration::from_nanos(value)
    } else {
        Duration::from_secs(value)
    };
    humantime::format_rfc3339_seconds(UNIX_EPOCH + since_epoch).to_string()
}

fn short_stable_hash(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")[..12].to_string()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::{Json, Router};

    use super::*;

    fn args(query: &str) -> MemoryExportArgs {
        MemoryExportArgs {
            query: query.into(),
            out: PathBuf::from("tmp-export"),
            page_size: daemon8_api::MEMORY_EXPORT_DEFAULT_PAGE_SIZE,
            client: ClientArgs {
                port: None,
                json: false,
            },
        }
    }

    #[test]
    fn validate_export_args_rejects_non_select() {
        let err = validate_export_args(&args("DELETE FROM memory ORDER BY created_at DESC"))
            .expect_err("non-select should fail");
        assert!(err.to_string().contains("must start with SELECT"));
    }

    #[test]
    fn validate_export_args_rejects_query_without_order_by() {
        let err = validate_export_args(&args("SELECT * FROM memory"))
            .expect_err("missing order by should fail");
        assert!(err.to_string().contains("ORDER BY"));
    }

    #[test]
    fn validate_export_args_accepts_paged_select_contract() {
        validate_export_args(&args("SELECT * FROM memory ORDER BY created_at DESC"))
            .expect("valid export query");
    }

    #[test]
    fn validate_output_target_rejects_existing_non_empty_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("old.md"), "stale").expect("write stale file");

        let err = validate_output_target(dir.path()).expect_err("non-empty dir should fail");

        assert!(err.to_string().contains("new or empty directory"));
    }

    #[test]
    fn sanitize_filename_stem_blocks_path_traversal_and_hidden_names() {
        assert_eq!(
            sanitize_filename_stem("../.env/secret".into()),
            "env-secret"
        );
        assert_eq!(sanitize_filename_stem("///".into()), "row");
    }

    #[test]
    fn row_filename_uses_scalar_id_slug() {
        let filename = row_filename(
            &serde_json::json!({
                "id":"memory:abc/../def",
                "project_slug": "daemon8",
                "kind": "pattern",
                "content":"hello"
            }),
            7,
        )
        .expect("filename");
        assert_eq!(filename, "000007-daemon8-pattern-memory-abc-def.md");
    }

    #[test]
    fn row_filename_falls_back_to_index_and_hash() {
        let filename = row_filename(&serde_json::json!({"content":"hello"}), 3).expect("filename");
        assert!(filename.starts_with("000003-row-000003-"));
        assert!(filename.ends_with(".md"));
    }

    #[test]
    fn render_row_markdown_uses_content_body_and_metadata_frontmatter() {
        let row = serde_json::json!({
            "id": "memory:abc",
            "content": "hello",
            "project_slug": "daemon8",
            "kind": "pattern",
            "tags": ["project:daemon8"],
            "created_at": 1_778_084_707_000_000_000_u64,
            "updated_at": 1_778_084_708_000_000_000_u64,
            "confidence": 0.95,
            "metadata": {"nested": true}
        });
        let markdown =
            render_row_markdown(&row, 1, "2026-05-06T13:42:10Z", "abcd1234").expect("markdown");

        assert!(markdown.starts_with("---\n"));
        assert!(markdown.contains("exported_at: \"2026-05-06T13:42:10Z\"\n"));
        assert!(markdown.contains("row_index: 1\n"));
        assert!(markdown.contains("id: \"memory:abc\"\n"));
        assert!(markdown.contains("project_slug: \"daemon8\"\n"));
        assert!(markdown.contains("kind: \"pattern\"\n"));
        assert!(markdown.contains("tags: [\"project:daemon8\"]\n"));
        assert!(markdown.contains("created_at: \"2026-05-06T16:25:07Z\"\n"));
        assert!(markdown.contains("updated_at: \"2026-05-06T16:25:08Z\"\n"));
        assert!(markdown.contains("confidence: 0.95\n"));
        assert!(!markdown.contains("created_at: 1778084707000000000"));
        assert!(!markdown.contains("content: \"hello\""));
        assert!(!markdown.contains("metadata:"));
        assert_eq!(markdown.split("\n---\n\n").nth(1).expect("body"), "hello\n");
    }

    #[test]
    fn write_ndjson_line_writes_each_row_immediately() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut row_index = 0usize;
        write_ndjson_line(
            br#"{"id":"memory:first","content":"one"}"#,
            dir.path(),
            "2026-05-06T13:42:10Z",
            "hash",
            &mut row_index,
        )
        .expect("write first row");

        assert_eq!(row_index, 1);
        assert!(dir.path().join("000001-memory-first.md").exists());

        write_ndjson_line(
            br#"{"id":"memory:second","content":"two"}"#,
            dir.path(),
            "2026-05-06T13:42:10Z",
            "hash",
            &mut row_index,
        )
        .expect("write second row");

        assert_eq!(row_index, 2);
        assert!(dir.path().join("000002-memory-second.md").exists());
    }

    #[test]
    fn drain_complete_lines_keeps_partial_next_row_buffered() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut row_index = 0usize;
        let mut buffer = br#"{"id":"memory:first","content":"one"}
{"id":"memory:second","content":"two"}"#
            .to_vec();

        drain_complete_lines(
            &mut buffer,
            dir.path(),
            "2026-05-06T13:42:10Z",
            "hash",
            &mut row_index,
        )
        .expect("drain one complete row");

        assert_eq!(row_index, 1);
        assert!(dir.path().join("000001-memory-first.md").exists());
        assert!(!dir.path().join("000002-memory-second.md").exists());
        assert!(String::from_utf8(buffer).unwrap().contains("memory:second"));
    }

    #[test]
    fn drain_complete_lines_rejects_unbounded_row_buffer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut row_index = 0usize;
        let mut buffer = vec![b'a'; MAX_NDJSON_LINE_BYTES + 1];

        let err = drain_complete_lines(
            &mut buffer,
            dir.path(),
            "2026-05-06T13:42:10Z",
            "hash",
            &mut row_index,
        )
        .expect_err("oversized row should fail");

        assert!(err.to_string().contains("exceeded"));
        assert_eq!(row_index, 0);
    }

    #[tokio::test]
    async fn cmd_memory_export_posts_query_and_writes_response_rows() {
        let captured_body = Arc::new(Mutex::new(None::<Value>));
        let captured = captured_body.clone();
        let app = Router::new().route(
            "/api/memory/export",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let captured = captured.clone();
                async move {
                    assert_eq!(
                        headers
                            .get(header::CONTENT_TYPE)
                            .and_then(|value| value.to_str().ok()),
                        Some("application/json")
                    );
                    *captured.lock().expect("capture mutex") = Some(body);
                    (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "application/x-ndjson")],
                        Body::from(
                            "{\"id\":\"memory:first\",\"content\":\"one\"}\n{\"id\":\"memory:second\",\"content\":\"two\"}\n",
                        ),
                    )
                        .into_response()
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let port = listener.local_addr().expect("local addr").port();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test app");
        });

        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("export");
        let args = MemoryExportArgs {
            query: "  SELECT * FROM memory ORDER BY created_at DESC  ".into(),
            out: out.clone(),
            page_size: 2,
            client: ClientArgs {
                port: Some(port),
                json: false,
            },
        };

        cmd_memory_export(args).await.expect("memory export");
        server.abort();

        assert_eq!(
            captured_body.lock().expect("capture mutex").as_ref(),
            Some(&serde_json::json!({
                "query": "SELECT * FROM memory ORDER BY created_at DESC",
                "page_size": 2
            }))
        );
        assert!(out.join("000001-memory-first.md").exists());
        assert!(out.join("000002-memory-second.md").exists());
        assert_eq!(std::fs::read_dir(&out).expect("export dir").count(), 2);
    }
}
