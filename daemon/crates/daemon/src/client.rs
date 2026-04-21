// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use anyhow::{Context, Result};
use clap::Subcommand;
use comfy_table::{Cell, ContentArrangement, Table, presets::UTF8_FULL_CONDENSED};
use daemon8_types::{
    ConnectionInfo, HealthStatus, Observation, Origin, RuntimeSummary, Severity, StateSlice,
};
use owo_colors::OwoColorize;
use reqwest_eventsource::{Event, EventSource};
use tokio_stream::StreamExt;

#[derive(clap::Args, Clone)]
pub struct ClientArgs {
    #[arg(long)]
    pub port: Option<u16>,
    #[arg(long)]
    pub json: bool,
}

impl ClientArgs {
    pub fn resolved_port(&self) -> u16 {
        self.port.unwrap_or_else(|| {
            crate::config::load(None)
                .map(|c| c.server.port)
                .unwrap_or(9077)
        })
    }
}

#[derive(clap::Args)]
pub struct TailArgs {
    #[command(flatten)]
    pub client: ClientArgs,
    #[arg(long)]
    pub kind: Option<String>,
    #[arg(long)]
    pub severity: Option<String>,
    #[arg(long)]
    pub origin: Option<String>,
    #[arg(long)]
    pub no_color: bool,
}

#[derive(clap::Args)]
pub struct QueryArgs {
    #[command(flatten)]
    pub client: ClientArgs,
    #[arg(long)]
    pub kind: Option<String>,
    #[arg(long)]
    pub severity: Option<String>,
    #[arg(long)]
    pub origin: Option<String>,
    #[arg(long)]
    pub text: Option<String>,
    #[arg(long)]
    pub since: Option<u64>,
    #[arg(long, default_value = "50")]
    pub limit: usize,
}

#[derive(Subcommand)]
pub enum ChromeSubcommand {
    /// List monitored browser tabs
    Tabs(ClientArgs),
    /// Evaluate JavaScript in a browser tab
    Eval {
        /// JavaScript expression
        expression: String,
        #[arg(long)]
        tab: Option<String>,
        #[command(flatten)]
        client: ClientArgs,
    },
    /// Take a screenshot of a browser tab
    Screenshot {
        #[arg(long)]
        selector: Option<String>,
        #[arg(long)]
        tab: Option<String>,
        #[arg(long, default_value = "screenshot.png")]
        output: String,
        #[command(flatten)]
        client: ClientArgs,
    },
    /// Inject CSS into a browser tab
    InjectCss {
        /// CSS text to inject
        css: String,
        #[arg(long)]
        tab: Option<String>,
        #[command(flatten)]
        client: ClientArgs,
    },
    /// Revert all injected CSS
    RevertCss {
        #[arg(long)]
        tab: Option<String>,
        #[command(flatten)]
        client: ClientArgs,
    },
    /// Get performance metrics from a browser tab
    Perf {
        #[arg(long)]
        tab: Option<String>,
        #[command(flatten)]
        client: ClientArgs,
    },
    /// Get DOM HTML from a browser tab (full page or CSS selector)
    Dom {
        /// CSS selector (omit for full page HTML)
        selector: Option<String>,
        #[arg(long)]
        tab: Option<String>,
        #[command(flatten)]
        client: ClientArgs,
    },
    /// Mobile emulation
    SetViewport {
        #[arg(long, default_value = "390")]
        width: u32,
        #[arg(long, default_value = "844")]
        height: u32,
        #[arg(long, default_value = "3.0")]
        scale: f64,
        #[arg(long, default_value = "true")]
        mobile: bool,
        #[arg(long)]
        ua: Option<String>,
        #[arg(long)]
        tab: Option<String>,
        #[command(flatten)]
        client: ClientArgs,
    },
    /// Restore desktop viewport
    ClearViewport {
        #[arg(long)]
        tab: Option<String>,
        #[command(flatten)]
        client: ClientArgs,
    },
}

fn base_url(port: u16) -> String {
    format!("http://localhost:{port}")
}

fn handle_reqwest_error(e: reqwest::Error, port: u16) -> anyhow::Error {
    if e.is_connect() {
        anyhow::anyhow!(
            "Cannot connect to daemon at localhost:{port}. Is it running? Start with: daemon8 serve"
        )
    } else {
        anyhow::anyhow!("HTTP request failed: {e}")
    }
}

async fn check_response(resp: reqwest::Response) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        Ok(resp)
    } else {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Server returned {status}: {body}");
    }
}

pub async fn cmd_status(args: ClientArgs) -> Result<()> {
    use crate::config;
    use crate::style;

    let cfg = config::load(None).unwrap_or_default();
    let config_path = cfg.config_dir.join("config.toml");

    let config_exists = config_path.exists();
    let config_label = if config_exists {
        style::green("exists")
    } else {
        style::dim("not found")
    };

    let data_dir = config::resolve_db_path(cfg.storage.path.as_deref());
    let data_dir_display = data_dir.parent().unwrap_or(&data_dir).display().to_string();

    let screenshot_dir = config::resolve_screenshot_path(&cfg);

    let port = args.resolved_port();
    let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), port);
    let running =
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(1)).is_ok();
    let process_label = if running {
        style::green("running")
    } else {
        style::dim("stopped")
    };

    if args.json {
        let json = serde_json::json!({
            "config_path": config_path.display().to_string(),
            "config_exists": config_exists,
            "data_dir": data_dir_display,
            "screenshot_dir": screenshot_dir.display().to_string(),
            "daemon": if running { "running" } else { "stopped" },
            "port": port,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
        return Ok(());
    }

    println!();
    println!("  {}", style::blue("Daemon8 Status"));
    println!("    {} {}", style::label("Config"), config_path.display());
    println!("    {}   {config_label}", style::label("      "));
    println!("    {} {data_dir_display}", style::label("Data   "));
    println!(
        "    {} {}",
        style::label("Screens"),
        screenshot_dir.display()
    );
    println!("    {} {process_label}", style::label("Daemon "));

    // If daemon is running, also fetch live summary
    if running {
        let url = format!("{}/api/summary", base_url(port));
        if let Ok(resp) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .unwrap_or_default()
            .get(&url)
            .send()
            .await
            && resp.status().is_success()
            && let Ok(summary) = resp.json::<RuntimeSummary>().await
        {
            let health_str = match summary.health {
                HealthStatus::Ok => style::green("ok"),
                HealthStatus::ErrorsDetected => "errors_detected".yellow().to_string(),
                HealthStatus::NoSources => style::dim("no_sources"),
            };
            println!();
            println!("    {} {health_str}", style::label("Health "));
            println!(
                "    {} {}",
                style::label("Obs    "),
                format_number(summary.observation_count)
            );
            println!(
                "    {} {}",
                style::label("Errors "),
                summary.error_count_last_60s
            );
            if !summary.active_channels.is_empty() {
                println!(
                    "    {} {}",
                    style::label("Sources"),
                    summary.active_channels.join(", ")
                );
            }
        }
    }

    println!();
    Ok(())
}

pub async fn cmd_tail(args: TailArgs) -> Result<()> {
    // Kind and severity ride on the server-side filter — exact-match semantics
    // match the existing client-side behavior. Origin stays client-side because
    // `--origin foo` is a substring match in this CLI, while the server's
    // `origins=` param is structured (app / app:name / browser / ...).
    let mut params: Vec<String> = Vec::new();
    if let Some(ref kind) = args.kind {
        params.push(format!("kinds={}", urlenc(kind)));
    }
    if let Some(ref severity) = args.severity
        && severity.parse::<Severity>().is_ok()
    {
        params.push(format!("severity_min={}", urlenc(severity)));
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

    let origin_filter: Option<String> = args.origin.clone();
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
                let obs: Observation = match serde_json::from_str(&msg.data) {
                    Ok(o) => o,
                    Err(_) => continue,
                };

                if let Some(ref pattern) = origin_filter {
                    let origin_str = format_origin(&obs.origin);
                    if !origin_str.contains(pattern.as_str()) {
                        continue;
                    }
                }

                if json_mode {
                    println!("{}", msg.data);
                } else {
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
    if let Some(since) = args.since {
        params.push(format!("since={since}"));
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

pub async fn cmd_connections(args: ClientArgs) -> Result<()> {
    let url = format!("{}/api/connections", base_url(args.resolved_port()));
    let resp = reqwest::get(&url)
        .await
        .map_err(|e| handle_reqwest_error(e, args.resolved_port()))?;
    let resp = check_response(resp).await?;

    if args.json {
        let raw: serde_json::Value = resp.json().await.unwrap_or_default();
        println!("{}", serde_json::to_string_pretty(&raw).unwrap_or_default());
        return Ok(());
    }

    #[derive(serde::Deserialize)]
    struct ConnectionsResponse {
        connections: Vec<ConnectionInfo>,
    }

    let data: ConnectionsResponse = resp
        .json()
        .await
        .context("failed to parse connections response")?;

    if data.connections.is_empty() {
        println!("No active connections.");
        return Ok(());
    }

    print_connections_table(&data.connections);
    Ok(())
}

pub async fn cmd_chrome(sub: ChromeSubcommand) -> Result<()> {
    match sub {
        ChromeSubcommand::Tabs(args) => cmd_chrome_tabs(args).await,
        ChromeSubcommand::Eval {
            expression,
            tab,
            client,
        } => cmd_chrome_eval(expression, tab, client).await,
        ChromeSubcommand::Screenshot {
            selector,
            tab,
            output,
            client,
        } => cmd_chrome_screenshot(selector, tab, output, client).await,
        ChromeSubcommand::InjectCss { css, tab, client } => {
            cmd_chrome_inject_css(css, tab, client).await
        }
        ChromeSubcommand::RevertCss { tab, client } => cmd_chrome_revert_css(tab, client).await,
        ChromeSubcommand::Perf { tab, client } => cmd_chrome_perf(tab, client).await,
        ChromeSubcommand::Dom {
            selector,
            tab,
            client,
        } => cmd_chrome_dom(selector, tab, client).await,
        ChromeSubcommand::SetViewport {
            width,
            height,
            scale,
            mobile,
            ua,
            tab,
            client,
        } => cmd_chrome_set_viewport(width, height, scale, mobile, ua, tab, client).await,
        ChromeSubcommand::ClearViewport { tab, client } => {
            cmd_chrome_clear_viewport(tab, client).await
        }
    }
}

async fn cmd_chrome_tabs(args: ClientArgs) -> Result<()> {
    let url = format!("{}/api/browser/act", base_url(args.resolved_port()));
    let body = serde_json::json!({ "action": "list_tabs" });

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| handle_reqwest_error(e, args.resolved_port()))?;
    let resp = check_response(resp).await?;

    let result: serde_json::Value = resp.json().await.context("failed to parse response")?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
        return Ok(());
    }

    if let Some(tabs) = result.get("tabs").and_then(|v| v.as_array()) {
        let mut table = Table::new();
        table.load_preset(UTF8_FULL_CONDENSED);
        table.set_content_arrangement(ContentArrangement::Dynamic);
        table.set_header(vec![
            Cell::new("Tab ID"),
            Cell::new("Title"),
            Cell::new("URL"),
        ]);

        for tab in tabs {
            table.add_row(vec![
                Cell::new(tab.get("id").and_then(|v| v.as_str()).unwrap_or("-")),
                Cell::new(tab.get("title").and_then(|v| v.as_str()).unwrap_or("-")),
                Cell::new(tab.get("url").and_then(|v| v.as_str()).unwrap_or("-")),
            ]);
        }

        println!("{table}");
    } else {
        println!("No tabs found.");
    }

    Ok(())
}

async fn cmd_chrome_eval(
    expression: String,
    tab: Option<String>,
    client_args: ClientArgs,
) -> Result<()> {
    let url = format!("{}/api/browser/act", base_url(client_args.resolved_port()));
    let body = serde_json::json!({
        "action": "eval_js",
        "expression": expression,
        "tab_id": tab,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| handle_reqwest_error(e, client_args.resolved_port()))?;
    let resp = check_response(resp).await?;

    let result: serde_json::Value = resp.json().await.context("failed to parse response")?;

    if client_args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
    } else if let Some(val) = result.get("result") {
        println!("{val}");
    }

    Ok(())
}

async fn cmd_chrome_screenshot(
    selector: Option<String>,
    tab: Option<String>,
    output: String,
    client_args: ClientArgs,
) -> Result<()> {
    let url = format!("{}/api/browser/act", base_url(client_args.resolved_port()));
    let body = serde_json::json!({
        "action": "screenshot",
        "tab_id": tab,
        "selector": selector,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| handle_reqwest_error(e, client_args.resolved_port()))?;
    let resp = check_response(resp).await?;

    let result: serde_json::Value = resp.json().await.context("failed to parse response")?;

    if let Some(temp_path) = result.get("path").and_then(|v| v.as_str()) {
        std::fs::copy(temp_path, &output)
            .with_context(|| format!("failed to copy screenshot to {output}"))?;
        let _ = std::fs::remove_file(temp_path);

        let size = result
            .get("size_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        println!("Screenshot saved to {output} ({size} bytes)");
    } else {
        anyhow::bail!(
            "unexpected response: {}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
    }

    Ok(())
}

async fn cmd_chrome_inject_css(
    css: String,
    tab: Option<String>,
    client_args: ClientArgs,
) -> Result<()> {
    let url = format!("{}/api/browser/act", base_url(client_args.resolved_port()));
    let body = serde_json::json!({
        "action": "inject_css",
        "css": css,
        "tab_id": tab,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| handle_reqwest_error(e, client_args.resolved_port()))?;
    let resp = check_response(resp).await?;

    let result: serde_json::Value = resp.json().await.context("failed to parse response")?;

    if client_args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
    } else if let Some(id) = result.get("element_id").and_then(|v| v.as_str()) {
        println!("Injected CSS as element: {id}");
    }

    Ok(())
}

async fn cmd_chrome_revert_css(tab: Option<String>, client_args: ClientArgs) -> Result<()> {
    let url = format!("{}/api/browser/act", base_url(client_args.resolved_port()));
    let body = serde_json::json!({
        "action": "revert_css",
        "tab_id": tab,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| handle_reqwest_error(e, client_args.resolved_port()))?;
    let resp = check_response(resp).await?;

    let result: serde_json::Value = resp.json().await.context("failed to parse response")?;

    if client_args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
    } else {
        let count = result.get("reverted").and_then(|v| v.as_u64()).unwrap_or(0);
        println!("Reverted {count} injected style(s)");
    }

    Ok(())
}

async fn cmd_chrome_perf(tab: Option<String>, client_args: ClientArgs) -> Result<()> {
    let url = format!("{}/api/browser/act", base_url(client_args.resolved_port()));
    let mut body = serde_json::json!({ "action": "get_perf_metrics" });
    if let Some(ref t) = tab {
        body["tab_id"] = serde_json::json!(t);
    }

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| handle_reqwest_error(e, client_args.resolved_port()))?;
    let resp = check_response(resp).await?;

    let result: serde_json::Value = resp.json().await.context("failed to parse response")?;

    if client_args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
        return Ok(());
    }

    if let Some(metrics) = result.get("metrics").and_then(|v| v.as_array()) {
        let mut table = Table::new();
        table.load_preset(UTF8_FULL_CONDENSED);
        table.set_content_arrangement(ContentArrangement::Dynamic);
        table.set_header(vec![Cell::new("Metric"), Cell::new("Value")]);

        for m in metrics {
            let name = m.get("name").and_then(|v| v.as_str()).unwrap_or("-");
            let value = m.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0);
            table.add_row(vec![Cell::new(name), Cell::new(format!("{value:.2}"))]);
        }

        println!("{table}");
    } else {
        println!("No metrics returned.");
    }

    Ok(())
}

async fn cmd_chrome_dom(
    selector: Option<String>,
    tab: Option<String>,
    client_args: ClientArgs,
) -> Result<()> {
    let url = format!("{}/api/browser/act", base_url(client_args.resolved_port()));
    let mut body = serde_json::json!({ "action": "get_dom" });
    if let Some(ref s) = selector {
        body["selector"] = serde_json::json!(s);
    }
    if let Some(ref t) = tab {
        body["tab_id"] = serde_json::json!(t);
    }

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| handle_reqwest_error(e, client_args.resolved_port()))?;
    let resp = check_response(resp).await?;

    let result: serde_json::Value = resp.json().await.context("failed to parse response")?;

    if client_args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
        return Ok(());
    }

    if let Some(html) = result.get("html").and_then(|v| v.as_str()) {
        let display = truncate(html, 2000);
        println!("{display}");
    } else {
        println!("No HTML returned.");
    }

    Ok(())
}

async fn cmd_chrome_set_viewport(
    width: u32,
    height: u32,
    scale: f64,
    mobile: bool,
    ua: Option<String>,
    tab: Option<String>,
    client_args: ClientArgs,
) -> Result<()> {
    let url = format!("{}/api/browser/act", base_url(client_args.resolved_port()));
    let body = serde_json::json!({
        "action": "set_viewport",
        "tab_id": tab,
        "viewport_width": width,
        "viewport_height": height,
        "viewport_scale": scale,
        "viewport_mobile": mobile,
        "viewport_ua": ua,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| handle_reqwest_error(e, client_args.resolved_port()))?;
    let resp = check_response(resp).await?;

    let result: serde_json::Value = resp.json().await.context("failed to parse response")?;

    if client_args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
    } else {
        println!(
            "Viewport set to {}x{} (scale={})",
            result.get("width").and_then(|v| v.as_u64()).unwrap_or(0),
            result.get("height").and_then(|v| v.as_u64()).unwrap_or(0),
            result.get("scale").and_then(|v| v.as_f64()).unwrap_or(0.0)
        );
    }

    Ok(())
}

async fn cmd_chrome_clear_viewport(tab: Option<String>, client_args: ClientArgs) -> Result<()> {
    let url = format!("{}/api/browser/act", base_url(client_args.resolved_port()));
    let body = serde_json::json!({
        "action": "clear_viewport",
        "tab_id": tab,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| handle_reqwest_error(e, client_args.resolved_port()))?;
    let resp = check_response(resp).await?;

    let result: serde_json::Value = resp.json().await.context("failed to parse response")?;

    if client_args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
    } else {
        println!("Viewport cleared");
    }

    Ok(())
}

fn print_connections_table(connections: &[ConnectionInfo]) {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("ID"),
        Cell::new("Type"),
        Cell::new("Name"),
        Cell::new("Observations"),
    ]);

    for conn in connections {
        let kind_str = match conn.kind {
            daemon8_types::ConnectionKind::Application => "application",
            daemon8_types::ConnectionKind::Browser => "browser",
            daemon8_types::ConnectionKind::Device => "device",
        };
        table.add_row(vec![
            Cell::new(&conn.id),
            Cell::new(kind_str),
            Cell::new(&conn.name),
            Cell::new(conn.observation_count),
        ]);
    }

    println!("{table}");
}

/// Convert epoch nanoseconds to "HH:MM:SS" (UTC).
fn format_timestamp(timestamp_ns: u64) -> String {
    let secs = (timestamp_ns / 1_000_000_000) as i64;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// Format severity with optional color.
fn format_severity(s: &Severity, use_color: bool) -> String {
    let label = s.to_string().to_uppercase();
    if !use_color {
        return label;
    }
    match s {
        Severity::Error => label.red().to_string(),
        Severity::Warn => label.yellow().to_string(),
        Severity::Info => label.green().to_string(),
        Severity::Debug => label.dimmed().to_string(),
        Severity::Trace => label.dimmed().to_string(),
    }
}

/// Format an Origin into a compact "type:name" string.
fn format_origin(o: &Origin) -> String {
    match o {
        Origin::Application { name } => format!("app:{name}"),
        Origin::Browser { url, .. } => format!("browser:{url}"),
        Origin::Device { serial, .. } => format!("device:{serial}"),
    }
}

/// Truncate a string, appending "..." if it exceeds `max` characters.
/// Safe for multi-byte UTF-8.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{truncated}...")
    }
}

/// Percent-encode a query parameter value.
fn urlenc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b',' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// Format a number with thousand separators (simple implementation).
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_timestamp_midnight() {
        assert_eq!(format_timestamp(0), "00:00:00");
    }

    #[test]
    fn format_timestamp_midday() {
        // 12:34:56 UTC = (12*3600 + 34*60 + 56) seconds = 45296 seconds
        let ns = 45_296_000_000_000u64;
        assert_eq!(format_timestamp(ns), "12:34:56");
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let long = "a".repeat(200);
        let result = truncate(&long, 50);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 50);
    }

    #[test]
    fn urlenc_preserves_unreserved() {
        assert_eq!(urlenc("hello-world_42"), "hello-world_42");
    }

    #[test]
    fn urlenc_encodes_special_chars() {
        assert_eq!(urlenc("a b"), "a%20b");
        assert_eq!(urlenc("foo=bar"), "foo%3Dbar");
    }

    #[test]
    fn format_number_thousands() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1_000), "1,000");
        assert_eq!(format_number(1_234_567), "1,234,567");
    }

    #[test]
    fn format_origin_variants() {
        let app = Origin::Application {
            name: "myapp".into(),
        };
        assert_eq!(format_origin(&app), "app:myapp");

        let browser = Origin::Browser {
            tab_id: "tab1".into(),
            url: "https://example.com".into(),
        };
        assert_eq!(format_origin(&browser), "browser:https://example.com");

        let device = Origin::Device {
            serial: "ABC123".into(),
            platform: daemon8_types::DevicePlatform::default(),
        };
        assert_eq!(format_origin(&device), "device:ABC123");
    }
}
