// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use anyhow::{Context, Result};
use clap::Subcommand;
use comfy_table::{Cell, ContentArrangement, Table, presets::UTF8_FULL_CONDENSED};
use owo_colors::OwoColorize;

use super::observe::{base_url, check_response, handle_reqwest_error, truncate, ClientArgs};

#[derive(Subcommand)]
pub enum ChromeSubcommand {
    /// Connect to a browser's DevTools endpoint
    Connect {
        /// DevTools endpoint URL (default: http://localhost:9222)
        #[arg(default_value = "http://localhost:9222")]
        endpoint: String,
        #[command(flatten)]
        client: ClientArgs,
    },
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
    /// Navigate to a URL and wait for page load
    Navigate {
        /// URL to navigate to
        url: String,
        #[arg(long)]
        tab: Option<String>,
        #[command(flatten)]
        client: ClientArgs,
    },
    /// Simulate network conditions (offline, slow-3g, fast-3g, restore)
    NetworkConditions {
        /// Preset: offline, slow-3g, fast-3g, restore
        #[arg(default_value = "restore")]
        preset: String,
        #[arg(long)]
        tab: Option<String>,
        #[command(flatten)]
        client: ClientArgs,
    },
    /// Clear storage for the current page origin
    StorageClear {
        /// Storage types: all, or comma-separated: cookies,local_storage,session_storage,indexeddb,cache_storage,service_workers
        #[arg(default_value = "all")]
        types: String,
        #[arg(long)]
        tab: Option<String>,
        #[command(flatten)]
        client: ClientArgs,
    },
    /// Inspect localStorage, sessionStorage, and cookies
    StorageInspect {
        #[arg(long)]
        tab: Option<String>,
        #[command(flatten)]
        client: ClientArgs,
    },
    /// Write a storage value
    StorageSet {
        /// Store type: localstorage, sessionstorage, cookie
        #[arg(long, default_value = "localstorage")]
        store_type: String,
        /// Key to set
        key: String,
        /// Value to set
        value: String,
        #[arg(long)]
        tab: Option<String>,
        #[command(flatten)]
        client: ClientArgs,
    },
    /// Identify the element at a screen coordinate
    ElementAtPoint {
        /// X coordinate
        x: f64,
        /// Y coordinate
        y: f64,
        #[arg(long)]
        tab: Option<String>,
        #[command(flatten)]
        client: ClientArgs,
    },
    /// Open a new browser tab
    NewTab {
        /// URL to open (default: about:blank)
        #[arg(default_value = "about:blank")]
        url: String,
        #[command(flatten)]
        client: ClientArgs,
    },
    /// Close a browser tab
    CloseTab {
        /// Tab ID to close (from 'daemon8 browser tabs')
        tab_id: String,
        #[command(flatten)]
        client: ClientArgs,
    },
}

pub async fn cmd_chrome(sub: ChromeSubcommand) -> Result<()> {
    match sub {
        ChromeSubcommand::Connect { endpoint, client } => {
            cmd_chrome_connect(endpoint, client).await
        }
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
        ChromeSubcommand::Navigate { url, tab, client } => {
            cmd_chrome_navigate(url, tab, client).await
        }
        ChromeSubcommand::NetworkConditions {
            preset,
            tab,
            client,
        } => cmd_chrome_network_conditions(preset, tab, client).await,
        ChromeSubcommand::StorageClear { types, tab, client } => {
            cmd_chrome_storage_clear(types, tab, client).await
        }
        ChromeSubcommand::StorageInspect { tab, client } => {
            cmd_chrome_storage_inspect(tab, client).await
        }
        ChromeSubcommand::StorageSet {
            store_type,
            key,
            value,
            tab,
            client,
        } => cmd_chrome_storage_set(store_type, key, value, tab, client).await,
        ChromeSubcommand::ElementAtPoint { x, y, tab, client } => {
            cmd_chrome_element_at_point(x, y, tab, client).await
        }
        ChromeSubcommand::NewTab { url, client } => cmd_chrome_new_tab(url, client).await,
        ChromeSubcommand::CloseTab { tab_id, client } => cmd_chrome_close_tab(tab_id, client).await,
    }
}

async fn cmd_chrome_connect(endpoint: String, args: ClientArgs) -> Result<()> {
    let url = format!("{}/api/connect", base_url(args.resolved_port()));
    let body = serde_json::json!({ "endpoint": endpoint });

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

    let status = result
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let ep = result
        .get("endpoint")
        .and_then(|v| v.as_str())
        .unwrap_or(&endpoint);
    println!("{} {ep}", status.green());

    Ok(())
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

    if let Some(temp_path) = result.get("screenshot").and_then(|v| v.as_str()) {
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
        let count = result.get("reverted_count").and_then(|v| v.as_u64()).unwrap_or(0);
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

async fn cmd_chrome_navigate(
    url: String,
    tab: Option<String>,
    client_args: ClientArgs,
) -> Result<()> {
    let api_url = format!("{}/api/browser/act", base_url(client_args.resolved_port()));
    let body = serde_json::json!({
        "action": "navigate",
        "tab_id": tab,
        "url": url,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&api_url)
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
        let title = result["title"].as_str().unwrap_or("(unknown)");
        println!("Navigated to {url} — title: {title}");
    }

    Ok(())
}

async fn cmd_chrome_network_conditions(
    preset: String,
    tab: Option<String>,
    client_args: ClientArgs,
) -> Result<()> {
    let url = format!("{}/api/browser/act", base_url(client_args.resolved_port()));
    let body = serde_json::json!({
        "action": "network_conditions",
        "tab_id": tab,
        "network_preset": preset,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| handle_reqwest_error(e, client_args.resolved_port()))?;
    check_response(resp).await?;

    println!("Network conditions set to: {preset}");
    Ok(())
}

async fn cmd_chrome_storage_clear(
    types: String,
    tab: Option<String>,
    client_args: ClientArgs,
) -> Result<()> {
    let url = format!("{}/api/browser/act", base_url(client_args.resolved_port()));
    let body = serde_json::json!({
        "action": "storage_clear",
        "tab_id": tab,
        "storage_types": types,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| handle_reqwest_error(e, client_args.resolved_port()))?;
    check_response(resp).await?;

    println!("Storage cleared: {types}");
    Ok(())
}

async fn cmd_chrome_storage_inspect(tab: Option<String>, client_args: ClientArgs) -> Result<()> {
    let url = format!("{}/api/browser/act", base_url(client_args.resolved_port()));
    let body = serde_json::json!({
        "action": "storage_inspect",
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

    println!(
        "{}",
        serde_json::to_string_pretty(&result).unwrap_or_default()
    );
    Ok(())
}

async fn cmd_chrome_storage_set(
    store_type: String,
    key: String,
    value: String,
    tab: Option<String>,
    client_args: ClientArgs,
) -> Result<()> {
    let url = format!("{}/api/browser/act", base_url(client_args.resolved_port()));
    let body = serde_json::json!({
        "action": "storage_set",
        "tab_id": tab,
        "store_type": store_type,
        "storage_key": key,
        "storage_value": value,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| handle_reqwest_error(e, client_args.resolved_port()))?;
    check_response(resp).await?;

    println!("Set {store_type}[{key}] = {value}");
    Ok(())
}

async fn cmd_chrome_element_at_point(
    x: f64,
    y: f64,
    tab: Option<String>,
    client_args: ClientArgs,
) -> Result<()> {
    let url = format!("{}/api/browser/act", base_url(client_args.resolved_port()));
    let body = serde_json::json!({
        "action": "element_at_point",
        "tab_id": tab,
        "x": x,
        "y": y,
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
        let tag = result["tag"].as_str().unwrap_or("?");
        let id = result["id"].as_str().unwrap_or("");
        let classes = result["classes"].as_str().unwrap_or("");
        let text = result["text"].as_str().unwrap_or("");
        let id_part = if id.is_empty() {
            String::new()
        } else {
            format!("#{id}")
        };
        let class_part = if classes.is_empty() {
            String::new()
        } else {
            format!(".{}", classes.replace(' ', "."))
        };
        println!("{tag}{id_part}{class_part}");
        if !text.is_empty() {
            println!("  text: {}", truncate(text, 80));
        }
    }

    Ok(())
}

async fn cmd_chrome_new_tab(url: String, client_args: ClientArgs) -> Result<()> {
    let api_url = format!("{}/api/browser/act", base_url(client_args.resolved_port()));
    let body = serde_json::json!({ "action": "new_tab", "url": url });

    let client = reqwest::Client::new();
    let resp = client
        .post(&api_url)
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
        let tab_id = result["tab_id"].as_str().unwrap_or("unknown");
        println!("{} {tab_id}", "opened".green());
    }
    Ok(())
}

async fn cmd_chrome_close_tab(tab_id: String, client_args: ClientArgs) -> Result<()> {
    let api_url = format!("{}/api/browser/act", base_url(client_args.resolved_port()));
    let body = serde_json::json!({ "action": "close_tab", "tab_id": tab_id });

    let client = reqwest::Client::new();
    let resp = client
        .post(&api_url)
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
        let closed = result["closed"].as_bool().unwrap_or(false);
        if closed {
            println!("{} {tab_id}", "closed".green());
        } else {
            println!("{} {tab_id}", "failed".red());
        }
    }
    Ok(())
}
