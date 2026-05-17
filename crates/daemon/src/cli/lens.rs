// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use anyhow::{Context, Result};
use clap::Subcommand;
use owo_colors::OwoColorize;

use super::observe::{ClientArgs, base_url, check_response, handle_reqwest_error};

#[derive(clap::Args)]
pub struct LensSetArgs {
    #[arg(long)]
    pub kind: Option<String>,
    #[arg(long)]
    pub severity_min: Option<String>,
    #[arg(long)]
    pub origin: Option<String>,
    #[arg(long)]
    pub service: Option<String>,
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long)]
    pub source_instance: Option<String>,
    #[arg(long, help = "Search across materialized observation text")]
    pub text: Option<String>,
    #[arg(long)]
    pub correlation_id: Option<String>,
    #[arg(long, value_delimiter = ',')]
    pub tags: Option<Vec<String>>,
    #[arg(long)]
    pub include_system: bool,
    #[arg(long)]
    pub capacity: Option<usize>,
    #[command(flatten)]
    pub client: ClientArgs,
}

#[derive(Subcommand)]
pub enum LensSubcommand {
    /// Show current lens status (filter, buffer depth, cursor)
    Status(ClientArgs),
    /// Set or replace the active lens filter
    Set(Box<LensSetArgs>),
    /// Remove the active lens
    Clear(ClientArgs),
}

pub async fn cmd_lens(sub: LensSubcommand) -> Result<()> {
    match sub {
        LensSubcommand::Status(args) => cmd_lens_status(args).await,
        LensSubcommand::Set(args) => cmd_lens_set(*args).await,
        LensSubcommand::Clear(args) => cmd_lens_clear(args).await,
    }
}

async fn cmd_lens_status(args: ClientArgs) -> Result<()> {
    let url = format!("{}/api/lens", base_url(args.resolved_port()));

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| handle_reqwest_error(e, args.resolved_port()))?;
    let resp = check_response(resp).await?;
    let status: serde_json::Value = resp.json().await.context("failed to parse response")?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&status).unwrap_or_default()
        );
        return Ok(());
    }

    let active = status["active"].as_bool().unwrap_or(false);
    if !active {
        println!("{}", "No active lens".dimmed());
        return Ok(());
    }

    let buffered = status["buffered"].as_u64().unwrap_or(0);
    let capacity = status["capacity"].as_u64().unwrap_or(0);
    let cursor = status["cursor"].as_u64().unwrap_or(0);

    println!("{}", "Lens active".green());
    println!("  buffered: {buffered}/{capacity}");
    println!("  cursor:   {cursor}");

    if let Some(filter) = status.get("filter") {
        let parts: Vec<String> = filter
            .as_object()
            .into_iter()
            .flat_map(|m| m.iter())
            .filter(|(_, v)| !v.is_null())
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        if !parts.is_empty() {
            println!("  filter:   {}", parts.join(", "));
        }
    }

    Ok(())
}

async fn cmd_lens_set(args: LensSetArgs) -> Result<()> {
    let url = format!("{}/api/lens", base_url(args.client.resolved_port()));

    let mut body = serde_json::Map::new();
    if let Some(v) = args.kind {
        body.insert("kinds".into(), serde_json::json!(v));
    }
    if let Some(v) = args.severity_min {
        body.insert("severity_min".into(), serde_json::json!(v));
    }
    if let Some(v) = args.origin {
        body.insert("origins".into(), serde_json::json!(v));
    }
    if let Some(v) = args.service {
        body.insert("service".into(), serde_json::json!(v));
    }
    if let Some(v) = args.source {
        body.insert("source".into(), serde_json::json!(v));
    }
    if let Some(v) = args.source_instance {
        body.insert("source_instance".into(), serde_json::json!(v));
    }
    if let Some(v) = args.text {
        body.insert("text_match".into(), serde_json::json!(v));
    }
    if let Some(v) = args.correlation_id {
        body.insert("correlation_id".into(), serde_json::json!(v));
    }
    if let Some(v) = args.tags {
        body.insert("tags".into(), serde_json::json!(v.join(",")));
    }
    if args.include_system {
        body.insert("include_system".into(), serde_json::json!(true));
    }
    if let Some(v) = args.capacity {
        body.insert("capacity".into(), serde_json::json!(v));
    }

    let client = reqwest::Client::new();
    let resp = client
        .put(&url)
        .json(&serde_json::Value::Object(body))
        .send()
        .await
        .map_err(|e| handle_reqwest_error(e, args.client.resolved_port()))?;
    let resp = check_response(resp).await?;
    let status: serde_json::Value = resp.json().await.context("failed to parse response")?;

    if args.client.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&status).unwrap_or_default()
        );
        return Ok(());
    }

    let buffered = status["buffered"].as_u64().unwrap_or(0);
    let capacity = status["capacity"].as_u64().unwrap_or(0);
    println!("{} (buffer: {buffered}/{capacity})", "Lens set".green());

    Ok(())
}

async fn cmd_lens_clear(args: ClientArgs) -> Result<()> {
    let url = format!("{}/api/lens", base_url(args.resolved_port()));

    let client = reqwest::Client::new();
    let resp = client
        .delete(&url)
        .send()
        .await
        .map_err(|e| handle_reqwest_error(e, args.resolved_port()))?;
    check_response(resp).await?;

    println!("{}", "Lens cleared".green());
    Ok(())
}
