// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use anyhow::Result;
use daemon8_types::{Observation, Severity};
use owo_colors::OwoColorize;
use reqwest_eventsource::{Event, EventSource};
use tokio_stream::StreamExt;

use super::{base_url, format_origin, format_severity, format_timestamp, truncate, urlenc};

#[derive(clap::Args)]
pub struct TailArgs {
    #[command(flatten)]
    pub client: super::ClientArgs,
    #[arg(long)]
    pub kind: Option<String>,
    #[arg(long)]
    pub severity: Option<String>,
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
    pub no_color: bool,
    #[arg(long)]
    pub include_system: bool,
}

pub async fn cmd_tail(args: TailArgs) -> Result<()> {
    let mut params: Vec<String> = Vec::new();
    if let Some(ref kind) = args.kind {
        params.push(format!("kinds={}", urlenc(kind)));
    }
    if let Some(ref severity) = args.severity
        && severity.parse::<Severity>().is_ok()
    {
        params.push(format!("severity_min={}", urlenc(severity)));
    }
    if let Some(ref origin) = args.origin {
        params.push(format!("origins={}", urlenc(origin)));
    }
    if let Some(ref service) = args.service {
        params.push(format!("service={}", urlenc(service)));
    }
    if let Some(ref source) = args.source {
        params.push(format!("source={}", urlenc(source)));
    }
    if let Some(ref source_instance) = args.source_instance {
        params.push(format!("source_instance={}", urlenc(source_instance)));
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
    if args.include_system {
        params.push("include_system=true".to_string());
    }
    let query = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };
    let url = format!(
        "{}/api/stream{query}",
        base_url(args.client.resolved_port())
    );
    let mut es = EventSource::get(&url);

    let use_color = !args.no_color;
    let json_mode = args.client.json;

    while let Some(event) = es.next().await {
        match event {
            Ok(Event::Open) => {
                if !json_mode {
                    eprintln!("{}", "Connected to daemon stream...".dimmed());
                }
            }
            Ok(Event::Message(msg)) => {
                if json_mode {
                    println!("{}", msg.data);
                } else {
                    let obs: Observation = match serde_json::from_str(&msg.data) {
                        Ok(o) => o,
                        Err(_) => continue,
                    };
                    let ts = format_timestamp(obs.timestamp_ns);
                    let sev = format_severity(&obs.severity, use_color);
                    let origin = format_origin(&obs.origin);
                    let kind = obs.kind.tag();
                    let data = truncate(&obs.data.to_string(), 120);
                    println!("[{ts}] {sev} {origin} {kind}: {data}");
                }
            }
            Err(reqwest_eventsource::Error::Transport(e)) => {
                if e.is_connect() {
                    eprintln!(
                        "Cannot connect to daemon at localhost:{}. Is it running? Start with: daemon8 serve",
                        args.client.resolved_port()
                    );
                    return Ok(());
                }
                eprintln!("Stream error: {e}");
                es.close();
                return Ok(());
            }
            Err(e) => {
                eprintln!("Stream error: {e}");
                es.close();
                return Ok(());
            }
        }
    }

    Ok(())
}
