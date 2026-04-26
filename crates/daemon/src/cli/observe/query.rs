// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use anyhow::{Context, Result};
use comfy_table::{Cell, ContentArrangement, Table, presets::UTF8_FULL_CONDENSED};
use daemon8_types::StateSlice;

use super::{
    base_url, check_response, format_origin, format_timestamp, handle_reqwest_error, truncate,
    urlenc,
};

#[derive(clap::Args)]
pub struct QueryArgs {
    #[command(flatten)]
    pub client: super::ClientArgs,
    #[arg(long)]
    pub kind: Option<String>,
    #[arg(long)]
    pub severity: Option<String>,
    #[arg(long)]
    pub origin: Option<String>,
    #[arg(long)]
    pub text: Option<String>,
    #[arg(long)]
    pub correlation_id: Option<String>,
    #[arg(long, value_delimiter = ',')]
    pub tags: Option<Vec<String>>,
    #[arg(long)]
    pub since: Option<u64>,
    #[arg(long, default_value = "50")]
    pub limit: usize,
    #[arg(long)]
    pub include_system: bool,
}

pub async fn cmd_query(args: QueryArgs) -> Result<()> {
    let mut params = Vec::new();

    if let Some(ref kind) = args.kind {
        params.push(format!("kinds={}", urlenc(kind)));
    }
    if let Some(ref severity) = args.severity {
        params.push(format!("severity_min={}", urlenc(severity)));
    }
    if let Some(ref origin) = args.origin {
        params.push(format!("origins={}", urlenc(origin)));
    }
    if let Some(ref text) = args.text {
        params.push(format!("text_match={}", urlenc(text)));
    }
    if let Some(ref cid) = args.correlation_id {
        params.push(format!("correlation_id={}", urlenc(cid)));
    }
    if let Some(ref tags) = args.tags {
        params.push(format!("tags={}", urlenc(&tags.join(","))));
    }
    if let Some(since) = args.since {
        params.push(format!("since={since}"));
    }
    if args.include_system {
        params.push("include_system=true".to_string());
    }
    params.push(format!("limit={}", args.limit));

    let query_string = params.join("&");
    let url = format!(
        "{}/api/observe?{query_string}",
        base_url(args.client.resolved_port())
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| handle_reqwest_error(e, args.client.resolved_port()))?;
    let resp = check_response(resp).await?;

    let slice: StateSlice = resp
        .json()
        .await
        .context("failed to parse query response")?;

    if args.client.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&slice).unwrap_or_default()
        );
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("ID"),
        Cell::new("Time"),
        Cell::new("Severity"),
        Cell::new("Kind"),
        Cell::new("Origin"),
        Cell::new("Data"),
    ]);

    for obs in &slice.observations {
        table.add_row(vec![
            Cell::new(obs.id),
            Cell::new(format_timestamp(obs.timestamp_ns)),
            Cell::new(obs.severity.to_string()),
            Cell::new(obs.kind.tag().to_string()),
            Cell::new(format_origin(&obs.origin)),
            Cell::new(truncate(&obs.data.to_string(), 60)),
        ]);
    }

    println!("{table}");
    println!(
        "{} observations (checkpoint: {})",
        slice.observations.len(),
        slice.checkpoint.0
    );

    Ok(())
}
