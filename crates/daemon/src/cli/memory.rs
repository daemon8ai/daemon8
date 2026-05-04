// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::HashSet;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use daemon8_store::SurrealStore;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::config;

#[derive(Subcommand)]
pub enum MemorySubcommand {
    /// Export memory query results to Markdown or ZIP
    Export(MemoryExportArgs),
}

#[derive(clap::Args, Debug, Clone)]
pub struct MemoryExportArgs {
    /// Table name (repeat for multiple sections)
    #[arg(long = "table", required = true)]
    pub table: Vec<String>,
    /// Read-only SurrealQL SELECT query (repeat to pair with each table)
    #[arg(long = "query", required = true)]
    pub query: Vec<String>,
    /// Output path (.md for plain export, .zip for one markdown file per section)
    #[arg(long)]
    pub out: PathBuf,
}

pub async fn cmd_memory(
    config_override: Option<String>,
    subcommand: MemorySubcommand,
) -> Result<()> {
    match subcommand {
        MemorySubcommand::Export(args) => cmd_memory_export(config_override, args).await,
    }
}

async fn cmd_memory_export(config_override: Option<String>, args: MemoryExportArgs) -> Result<()> {
    validate_export_args(&args)?;

    let cfg = config::load(config_override.as_deref()).unwrap_or_default();
    let db_path = config::resolve_db_path(cfg.storage.path.as_deref());
    let store = SurrealStore::open(&db_path)
        .await
        .with_context(|| format!("opening daemon8 store at {}", db_path.display()))?;

    let mut sections = Vec::with_capacity(args.table.len());
    for (table, query) in args.table.iter().zip(args.query.iter()) {
        let rows = store
            .raw_select_rows(query)
            .await
            .with_context(|| format!("query failed for table '{table}'"))?;
        sections.push(ExportSection {
            table: table.clone(),
            query: query.clone(),
            rows,
        });
    }

    if let Some(parent) = args.out.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating output directory {}", parent.display()))?;
    }

    let generated_at_unix_s = now_unix_s();
    if is_zip_path(&args.out) {
        write_zip_export(&args.out, &sections, generated_at_unix_s, &db_path)?;
        println!(
            "exported {} markdown files in zip archive {}",
            sections.len(),
            args.out.display()
        );
    } else {
        let markdown = render_combined_markdown(&sections, generated_at_unix_s, &db_path)?;
        std::fs::write(&args.out, markdown)
            .with_context(|| format!("writing export file {}", args.out.display()))?;
        println!(
            "exported {} query sections to {}",
            sections.len(),
            args.out.display()
        );
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct ExportSection {
    table: String,
    query: String,
    rows: Vec<serde_json::Value>,
}

fn validate_export_args(args: &MemoryExportArgs) -> Result<()> {
    if args.table.len() != args.query.len() {
        bail!(
            "--table count ({}) must match --query count ({})",
            args.table.len(),
            args.query.len()
        );
    }

    for table in &args.table {
        validate_table_name(table)?;
    }

    for (table, query) in args.table.iter().zip(args.query.iter()) {
        validate_select_query(table, query)?;
    }

    Ok(())
}

fn now_unix_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn is_zip_path(path: &Path) -> bool {
    path.extension()
        .and_then(|v| v.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
}

fn validate_table_name(table: &str) -> Result<()> {
    let mut chars = table.chars();
    let Some(first) = chars.next() else {
        bail!("table name cannot be empty");
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        bail!("table name '{table}' must start with a letter or underscore");
    }
    if !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        bail!("table name '{table}' may only contain letters, digits, and underscores");
    }
    Ok(())
}

fn validate_select_query(table: &str, query: &str) -> Result<()> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        bail!("query for table '{table}' cannot be empty");
    }
    if trimmed.contains(';') {
        bail!(
            "query for table '{table}' contains ';' - only one single SELECT statement is allowed"
        );
    }

    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("select ") && lower != "select" {
        bail!("query for table '{table}' must start with SELECT");
    }

    let table_lower = table.to_ascii_lowercase();
    let tokens: Vec<&str> = lower
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|w| !w.is_empty())
        .collect();

    let has_exact_from = tokens
        .windows(2)
        .any(|window| window[0] == "from" && window[1] == table_lower.as_str());
    if !has_exact_from {
        bail!(
            "query for table '{table}' must include a FROM {table} clause for explicit table scoping"
        );
    }

    let forbidden = [
        "create", "update", "delete", "insert", "upsert", "relate", "remove", "define", "begin",
        "commit", "cancel", "use", "let",
    ];
    let words: HashSet<&str> = tokens.iter().copied().collect();
    if let Some(keyword) = forbidden.iter().find(|k| words.contains(*k)) {
        bail!(
            "query for table '{table}' contains forbidden keyword '{keyword}' (only read-only SELECT is allowed)"
        );
    }

    Ok(())
}

fn render_combined_markdown(
    sections: &[ExportSection],
    generated_at_unix_s: u64,
    db_path: &Path,
) -> Result<String> {
    let mut out = String::new();
    out.push_str("# daemon8 memory export\n\n");
    out.push_str(&format!("generated_at_unix_s: {generated_at_unix_s}\n\n"));
    out.push_str(&format!("db_path: {}\n\n", db_path.display()));

    for (idx, section) in sections.iter().enumerate() {
        out.push_str(&render_section_markdown(
            idx + 1,
            section,
            generated_at_unix_s,
            db_path,
        )?);
    }

    Ok(out)
}

fn render_section_markdown(
    index: usize,
    section: &ExportSection,
    generated_at_unix_s: u64,
    db_path: &Path,
) -> Result<String> {
    let mut out = String::new();
    out.push_str(&format!("## {}. {}\n\n", index, section.table));
    out.push_str(&format!("generated_at_unix_s: {generated_at_unix_s}\n\n"));
    out.push_str(&format!("db_path: {}\n\n", db_path.display()));
    out.push_str("query:\n\n");
    out.push_str("```sql\n");
    out.push_str(section.query.trim());
    out.push_str("\n```\n\n");
    out.push_str(&format!("rows: {}\n\n", section.rows.len()));
    out.push_str("```json\n");
    out.push_str(
        &serde_json::to_string_pretty(&section.rows)
            .context("serializing export rows to JSON markdown block")?,
    );
    out.push_str("\n```\n\n");
    Ok(out)
}

fn write_zip_export(
    out_path: &Path,
    sections: &[ExportSection],
    generated_at_unix_s: u64,
    db_path: &Path,
) -> Result<()> {
    let file = std::fs::File::create(out_path)
        .with_context(|| format!("creating zip archive {}", out_path.display()))?;
    write_zip_to_writer(file, sections, generated_at_unix_s, db_path)
        .with_context(|| format!("writing zip archive {}", out_path.display()))
}

fn write_zip_to_writer<W: Write + Seek>(
    writer: W,
    sections: &[ExportSection],
    generated_at_unix_s: u64,
    db_path: &Path,
) -> Result<()> {
    let mut zip = ZipWriter::new(writer);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    for (idx, section) in sections.iter().enumerate() {
        let entry_name = format!("{:02}-{}.md", idx + 1, section.table);
        zip.start_file(&entry_name, options)
            .with_context(|| format!("starting zip entry {entry_name}"))?;
        let section_md = render_section_markdown(idx + 1, section, generated_at_unix_s, db_path)
            .with_context(|| format!("rendering markdown for zip entry {entry_name}"))?;
        zip.write_all(section_md.as_bytes())
            .with_context(|| format!("writing zip entry {entry_name}"))?;
    }

    zip.finish().context("finalizing zip archive")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    fn args(table: &[&str], query: &[&str]) -> MemoryExportArgs {
        MemoryExportArgs {
            table: table.iter().map(|s| s.to_string()).collect(),
            query: query.iter().map(|s| s.to_string()).collect(),
            out: PathBuf::from("tmp.zip"),
        }
    }

    #[test]
    fn validate_export_args_rejects_mismatched_counts() {
        let err = validate_export_args(&args(&["memory"], &["SELECT * FROM memory", "SELECT 1"]))
            .expect_err("mismatched counts should fail");
        assert!(err.to_string().contains("--table count"));
    }

    #[test]
    fn validate_export_args_rejects_non_select() {
        let err = validate_export_args(&args(&["memory"], &["DELETE FROM memory"]))
            .expect_err("non-select should fail");
        assert!(err.to_string().contains("must start with SELECT"));
    }

    #[test]
    fn validate_export_args_rejects_wrong_table_scope() {
        let err = validate_export_args(&args(&["memory"], &["SELECT * FROM memory_long"]))
            .expect_err("wrong table scope should fail");
        assert!(err.to_string().contains("FROM memory"));
    }

    #[test]
    fn is_zip_path_accepts_uppercase_extension() {
        assert!(is_zip_path(Path::new("export.ZIP")));
        assert!(!is_zip_path(Path::new("export.md")));
    }

    #[test]
    fn write_zip_to_writer_creates_expected_markdown_entries() {
        let mut cursor = std::io::Cursor::new(Vec::<u8>::new());
        let sections = vec![
            ExportSection {
                table: "memory".into(),
                query: "SELECT * FROM memory LIMIT 1".into(),
                rows: vec![serde_json::json!({"id":"abc"})],
            },
            ExportSection {
                table: "memory_long".into(),
                query: "SELECT * FROM memory_long LIMIT 1".into(),
                rows: vec![serde_json::json!({"id":"def"})],
            },
        ];

        write_zip_to_writer(&mut cursor, &sections, 1_700_000_000, Path::new("/tmp/db"))
            .expect("write zip");
        cursor.set_position(0);

        let mut archive = zip::ZipArchive::new(cursor).expect("open zip archive");
        assert_eq!(archive.len(), 2);
        assert!(archive.by_name("01-memory.md").is_ok());
        assert!(archive.by_name("02-memory_long.md").is_ok());

        let mut first = archive.by_name("01-memory.md").expect("first entry exists");
        let mut first_text = String::new();
        first
            .read_to_string(&mut first_text)
            .expect("read first entry");
        assert!(first_text.contains("## 1. memory"));
        assert!(first_text.contains("\"id\": \"abc\""));
    }

    #[test]
    fn validate_export_args_accepts_valid_select_pairs() {
        validate_export_args(&args(
            &["memory", "memory_long"],
            &[
                "SELECT * FROM memory ORDER BY created_at DESC LIMIT 5",
                "SELECT * FROM memory_long LIMIT 10",
            ],
        ))
        .expect("valid export args");
    }

    #[test]
    fn validate_export_args_rejects_semicolon() {
        let err = validate_export_args(&args(&["memory"], &["SELECT * FROM memory;"]))
            .expect_err("semicolon should fail");
        assert!(err.to_string().contains("only one single SELECT statement"));
    }

    #[test]
    fn validate_export_args_rejects_invalid_table_name() {
        let err = validate_export_args(&args(&["memory-long"], &["SELECT * FROM memory-long"]))
            .expect_err("invalid table should fail");
        assert!(
            err.to_string()
                .contains("may only contain letters, digits, and underscores")
        );
    }

    #[test]
    fn render_section_markdown_includes_query_and_rows() {
        let md = render_section_markdown(
            2,
            &ExportSection {
                table: "memory".into(),
                query: "SELECT * FROM memory LIMIT 1".into(),
                rows: vec![serde_json::json!({"id":"abc","content":"hello"})],
            },
            1_700_000_000,
            Path::new("/tmp/db"),
        )
        .expect("render section");
        assert!(md.contains("## 2. memory"));
        assert!(md.contains("SELECT * FROM memory LIMIT 1"));
        assert!(md.contains("rows: 1"));
    }

    #[test]
    fn render_combined_markdown_includes_sections_and_counts() {
        let md = render_combined_markdown(
            &[ExportSection {
                table: "memory".into(),
                query: "SELECT * FROM memory LIMIT 1".into(),
                rows: vec![serde_json::json!({"id":"abc","content":"hello"})],
            }],
            1_700_000_000,
            Path::new("/tmp/daemon8.db"),
        )
        .expect("render markdown");

        assert!(md.contains("# daemon8 memory export"));
        assert!(md.contains("## 1. memory"));
        assert!(md.contains("rows: 1"));
        assert!(md.contains("\"id\": \"abc\""));
        assert!(md.contains("db_path: /tmp/daemon8.db"));
    }

    #[test]
    fn validate_export_args_rejects_mutating_keyword() {
        let err = validate_export_args(&args(&["memory"], &["SELECT * FROM memory UPDATE foo"]))
            .expect_err("mutating keyword should fail");
        assert!(err.to_string().contains("forbidden keyword"));
    }
}
