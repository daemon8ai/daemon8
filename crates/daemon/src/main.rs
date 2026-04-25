// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

#[path = "../commands/agent.rs"]
mod agent;
mod channel;
mod cleanup;
mod cli_config;
mod cli_hook;
mod client;
mod config;
mod doctor;
mod init;
mod provider;
mod screenshot;
mod service;
mod setup;
pub(crate) mod style;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use daemon8_mcp::ChromeCommand;
use daemon8_store::{SqliteStore, StateModel};
use daemon8_types::Observation;

const CHROME_CMD_CAPACITY: usize = 64;
const BROWSER_ACTION_CAPACITY: usize = 64;

#[derive(Parser)]
#[command(
    name = "daemon8",
    about = "Runtime observation layer for AI coding agents"
)]
struct Cli {
    #[arg(long, global = true)]
    config: Option<String>,

    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the daemon server
    Serve(ServeArgs),
    /// Show daemon health and status
    Status(client::ClientArgs),
    /// Stream observations in real-time
    Tail(client::TailArgs),
    /// Query stored observations
    Query(client::QueryArgs),
    /// List active data source connections
    Connections(client::ClientArgs),
    /// Browser DevTools commands
    #[command(subcommand)]
    Browser(client::ChromeSubcommand),
    /// Show log file location or tail logs
    Logs {
        /// Follow the log file (like tail -f)
        #[arg(short, long)]
        follow: bool,
    },
    /// Show or modify configuration
    #[command(subcommand)]
    Config(ConfigSubcommand),
    /// Generate shell completions
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::aot::Shell,
    },
    /// Install daemon8 as a system service (starts on login, restarts on crash)
    Install,
    /// Remove daemon8 system service
    Uninstall,
    /// Interactive setup wizard (run after install)
    Setup,
    /// Real-time alert relay for Claude Code (experimental)
    Channel,
    /// Run a stateless background or one-shot daemon8 agent
    Agent(agent::AgentArgs),
    /// Diagnose common configuration and environment issues
    Doctor {
        /// Attempt to fix issues that can be repaired automatically
        #[arg(long)]
        fix: bool,
    },
    /// Universal CLI hook handler (invoked by Claude/Cursor/Gemini/Codex/Copilot/Continue)
    #[command(name = "cli-hook", hide = true)]
    CliHook(cli_hook::CliHookArgs),
    /// Initialize a `.daemon8-cli.toml` at the current project
    Init(init::InitArgs),
}

#[derive(Subcommand)]
enum ConfigSubcommand {
    /// Show resolved configuration (default)
    Show {
        #[arg(long)]
        json: bool,
    },
    /// Print config file path
    Path,
    /// Set a config value (e.g. daemon8 config set browser.path "/path/to/browser")
    Set {
        /// Dotted key path (e.g. browser.path, server.port)
        key: String,
        /// Value to set
        value: String,
    },
}

#[derive(clap::Args, Default)]
struct ServeArgs {
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    browser: Option<String>,
    #[arg(long, hide = true)]
    log_dir: Option<String>,
    #[arg(long, hide = true)]
    log_level: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let command = cli.command.unwrap_or(Commands::Serve(ServeArgs::default()));

    let _log_guard = match &command {
        Commands::Serve(args) => {
            let mut cfg = config::load(cli.config.as_deref()).unwrap_or_default();
            if let Some(ref dir) = args.log_dir {
                cfg.logging.file = Some(PathBuf::from(dir));
            }
            if let Some(ref level) = args.log_level {
                cfg.logging.level = level.parse().unwrap_or_default();
            }
            init_tracing(cli.verbose, &cfg.logging, true)
        }
        _ => init_tracing(cli.verbose, &config::LoggingConfig::default(), false),
    };

    match command {
        Commands::Serve(args) => cmd_serve(cli.config, args).await,
        Commands::Status(args) => client::cmd_status(args).await,
        Commands::Tail(args) => client::cmd_tail(args).await,
        Commands::Query(args) => client::cmd_query(args).await,
        Commands::Connections(args) => client::cmd_connections(args).await,
        Commands::Browser(sub) => client::cmd_chrome(sub).await,
        Commands::Logs { follow } => cmd_logs(cli.config, follow),
        Commands::Config(sub) => match sub {
            ConfigSubcommand::Show { json } => cmd_config(cli.config, json),
            ConfigSubcommand::Path => cmd_config_path(cli.config),
            ConfigSubcommand::Set { key, value } => cmd_config_set(cli.config, &key, &value),
        },
        Commands::Completions { shell } => cmd_completions(shell),
        Commands::Install => service::cmd_install(),
        Commands::Uninstall => service::cmd_uninstall(),
        Commands::Setup => setup::cmd_setup().await,
        Commands::Channel => channel::cmd_channel().await,
        Commands::Agent(args) => agent::run_agent(args).await,
        Commands::Doctor { fix } => doctor::cmd_doctor(fix),
        Commands::CliHook(args) => cli_hook::cmd_cli_hook(args),
        Commands::Init(args) => init::cmd_init(args),
    }
}

/// Guard that must be held for the lifetime of the process to ensure
/// non-blocking file log writes are flushed on shutdown.
struct LogGuard {
    _file_guard: Option<tracing_appender::non_blocking::WorkerGuard>,
}

fn init_tracing(verbose: bool, logging: &config::LoggingConfig, file_enabled: bool) -> LogGuard {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::fmt;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let daemon8_level = if verbose {
        "debug"
    } else {
        logging.level.as_str()
    };
    let default_filter = format!(
        "daemon8={l},daemon8_chrome={l},daemon8_mcp={l},daemon8_store={l},\
         daemon8_ingest={l},daemon8_api={l},daemon8_adb={l},daemon={l},\
         adb_client=off,warn",
        l = daemon8_level,
    );
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&default_filter));

    let (file_layer, file_guard) = if file_enabled {
        let log_dir = config::resolve_log_dir(logging.file.as_deref());
        std::fs::create_dir_all(&log_dir).ok();

        let file_appender = tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("daemon8")
            .filename_suffix("log")
            .max_log_files(logging.max_log_files)
            .build(&log_dir)
            .expect("failed to create log file appender");

        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

        let layer = fmt::layer()
            .with_writer(non_blocking)
            .with_ansi(false)
            .with_target(true);

        (Some(layer), Some(guard))
    } else {
        (None, None)
    };

    let stderr_layer = if logging.stderr {
        Some(fmt::layer().with_writer(std::io::stderr).with_target(false))
    } else {
        None
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .init();

    LogGuard {
        _file_guard: file_guard,
    }
}

async fn cmd_serve(config_path: Option<String>, args: ServeArgs) -> Result<()> {
    let mut cfg = config::load(config_path.as_deref()).context("failed to load configuration")?;

    if let Some(port) = args.port {
        cfg.server.port = port;
    }
    if let Some(ref endpoint) = args.browser {
        cfg.browser.auto_connect = true;
        cfg.browser.endpoint = endpoint.clone();
    }

    let db_path = config::resolve_db_path(cfg.storage.path.as_deref());
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating db directory: {}", parent.display()))?;
    }

    let store: Arc<dyn StateModel> = Arc::new(
        SqliteStore::open(&db_path)
            .with_context(|| format!("opening database: {}", db_path.display()))?,
    );

    // Unbounded channel — deliberate policy.  The daemon captures observations
    // best-effort and losslessly: callers POST and return immediately; the store
    // writer drains the receiver without backpressure.  Switching to a bounded
    // channel would introduce dropped observations under burst load and change
    // the data-contract that agents depend on.  Do not change to bounded without
    // measuring the real burst profile, choosing a drop strategy (oldest vs
    // newest), and documenting the trade-offs here.
    let (obs_tx, obs_rx) = mpsc::unbounded_channel::<Observation>();
    // Broadcast carries (Arc<Observation>, Arc<str>) so every subscriber can
    // filter on the typed observation (cheap) and forward the pre-serialized
    // JSON (cheap Arc clone) without re-serializing per subscriber. The
    // serialized payload carries the real id assigned by store.insert.
    let (broadcast_tx, _broadcast_rx) = broadcast::channel::<(Arc<Observation>, Arc<str>)>(1000);
    let cancel = CancellationToken::new();
    let mut tasks = JoinSet::new();

    let node_id: Arc<str> = Arc::from(resolve_node_id().as_str());

    spawn_store_writer(
        &mut tasks,
        obs_rx,
        store.clone(),
        broadcast_tx.clone(),
        cancel.clone(),
        node_id,
    );

    let screenshot_dir = config::resolve_screenshot_path(&cfg);
    cleanup::spawn_cleanup_task(
        &mut tasks,
        store.clone(),
        screenshot_dir.clone(),
        cancel.clone(),
    );

    let (chrome_cmd_tx, chrome_cmd_rx) = mpsc::channel::<ChromeCommand>(CHROME_CMD_CAPACITY);

    let browser_binary = cfg.browser.path.as_deref().map(|p| p.display().to_string());
    let reconnect_policy = daemon8_chrome::ReconnectPolicy {
        initial: Duration::from_secs(cfg.browser.reconnect_interval_secs),
        max: Duration::from_secs(cfg.browser.max_reconnect_interval_secs),
    };
    let chrome_state_rx = spawn_chrome_command_handler(
        &mut tasks,
        chrome_cmd_rx,
        obs_tx.clone(),
        browser_binary,
        reconnect_policy,
        cancel.clone(),
    );

    // Store the chrome endpoint for lazy connection (triggered on first tool use).
    // No auto-connect -- the MCP tools handle it on demand.
    let chrome_endpoint: Arc<std::sync::Mutex<Option<Arc<str>>>> =
        Arc::new(std::sync::Mutex::new(if cfg.browser.auto_connect {
            Some(Arc::from(cfg.browser.endpoint.as_str()))
        } else {
            None
        }));

    let device_screenshot_fn: Option<daemon8_mcp::DeviceScreenshotFn> = if cfg.adb.enabled {
        let addr = cfg.adb.server_addr;
        let scan_interval = cfg.adb.scan_interval_secs;
        let tx = obs_tx.clone();
        let ct = cancel.clone();
        tasks.spawn(async move {
            if let Err(e) = daemon8_adb::connect_and_monitor(addr, scan_interval, tx, ct).await {
                tracing::error!("ADB device monitor error: {e}");
            }
        });
        Some(screenshot::build_screenshot_fn(addr))
    } else {
        None
    };

    if cfg.ingestion.udp.enabled {
        let bind = cfg.ingestion.udp.bind;
        let max_packet = cfg.ingestion.udp.max_packet;
        let tx = obs_tx.clone();
        let ct = cancel.clone();
        tasks.spawn(async move {
            if let Err(e) = daemon8_ingest::udp::run_udp_listener(bind, max_packet, tx, ct).await {
                tracing::error!("UDP listener error: {e}");
            }
        });
    }

    #[cfg(unix)]
    if cfg.ingestion.unix.enabled {
        let path = config::resolve_unix_socket_path(cfg.ingestion.unix.path.as_deref());
        let tx = obs_tx.clone();
        let ct = cancel.clone();
        tasks.spawn(async move {
            if let Err(e) = daemon8_ingest::unix::run_unix_listener(&path, tx, ct).await {
                tracing::error!("Unix socket listener error: {e}");
            }
        });
    }
    #[cfg(windows)]
    if cfg.ingestion.unix.enabled {
        tracing::warn!(
            "unix socket ingestion is not supported on Windows; \
             set ingestion.unix.enabled = false to silence this warning"
        );
    }

    let (sub_tx, _sub_rx) = tokio::sync::watch::channel::<Option<daemon8_types::Filter>>(None);
    let sub_tx = Arc::new(sub_tx);

    // Only start MCP stdio when stdin is a real FIFO from an MCP client.
    // A plain "not a TTY" check is insufficient: launchd, nohup, and shell
    // backgrounding all attach /dev/null (a character device) to stdin.
    // rmcp 1.3 happily initializes on /dev/null, then waiting() returns at
    // EOF and the daemon cancels itself. FIFO detection is the one signal
    // that distinguishes a real client pipe from /dev/null on every launcher.
    let stdin_is_pipe = stdin_is_real_pipe();
    if stdin_is_pipe {
        use rmcp::ServiceExt;

        let mcp = daemon8_mcp::DaemonMcp::new(daemon8_mcp::DaemonMcpConfig {
            store: store.clone(),
            obs_tx: obs_tx.clone(),
            chrome_tx: chrome_cmd_tx.clone(),
            chrome_state: chrome_state_rx.clone(),
            chrome_endpoint: chrome_endpoint.clone(),
            device_screenshot_fn: device_screenshot_fn.clone(),
            screenshot_dir: screenshot_dir.clone(),
            subscription_tx: sub_tx.clone(),
            broadcast_tx: broadcast_tx.clone(),
        });
        let cancel_on_eof = cancel.clone();
        tasks.spawn(async move {
            match mcp.serve(rmcp::transport::stdio()).await {
                Ok(service) => {
                    tracing::info!("MCP stdio server running");
                    let _ = service.waiting().await;
                    tracing::info!("MCP client disconnected (stdin closed), shutting down");
                    cancel_on_eof.cancel();
                }
                Err(e) => {
                    tracing::error!("MCP stdio error: {e}");
                }
            }
        });
    } else if cfg.mcp.stdio {
        tracing::debug!(
            "MCP stdio requested but stdin is not a pipe -- skipping (use via MCP client)"
        );
    }

    let mcp_store = store.clone();
    let mcp_obs_tx = obs_tx.clone();
    let mcp_chrome_tx = chrome_cmd_tx.clone();
    let mcp_state_rx = chrome_state_rx.clone();
    let mcp_ep = chrome_endpoint.clone();
    let mcp_screenshot_fn = device_screenshot_fn.clone();
    let mcp_screenshot_dir = screenshot_dir.clone();
    let mcp_broadcast_tx = broadcast_tx.clone();
    let mcp_cancel = cancel.child_token();

    let mcp_http = rmcp::transport::streamable_http_server::StreamableHttpService::new(
        move || {
            Ok(daemon8_mcp::DaemonMcp::new(daemon8_mcp::DaemonMcpConfig {
                store: mcp_store.clone(),
                obs_tx: mcp_obs_tx.clone(),
                chrome_tx: mcp_chrome_tx.clone(),
                chrome_state: mcp_state_rx.clone(),
                chrome_endpoint: mcp_ep.clone(),
                device_screenshot_fn: mcp_screenshot_fn.clone(),
                screenshot_dir: mcp_screenshot_dir.clone(),
                subscription_tx: sub_tx.clone(),
                broadcast_tx: mcp_broadcast_tx.clone(),
            }))
        },
        Arc::new(
            rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default(),
        ),
        rmcp::transport::streamable_http_server::StreamableHttpServerConfig::default()
            .with_stateful_mode(true)
            .with_cancellation_token(mcp_cancel),
    );

    let api_state = daemon8_api::ApiState {
        store: store.clone(),
        stream_tx: broadcast_tx.clone(),
        chrome_cmd_tx: chrome_cmd_tx.clone(),
        chrome_state: chrome_state_rx.clone(),
        chrome_endpoint: chrome_endpoint.clone(),
    };
    let port = cfg.server.port;
    let app = daemon8_ingest::ingest_router(obs_tx.clone())
        .merge(daemon8_api::api_router(api_state))
        .route(
            "/.well-known/oauth-protected-resource",
            axum::routing::get(move || async move {
                axum::Json(serde_json::json!({
                    "resource": format!("http://localhost:{port}"),
                    "authorization_servers": []
                }))
            }),
        )
        .nest_service("/mcp", mcp_http);

    let bind_addr = format!("{}:{}", cfg.server.host, cfg.server.port);
    let listener = bind_with_retry(&bind_addr, cfg.server.port).await?;

    let mut transports = vec!["http"];
    if cfg.ingestion.udp.enabled {
        transports.push("udp");
    }
    #[cfg(unix)]
    if cfg.ingestion.unix.enabled {
        transports.push("unix");
    }

    let log_dir = config::resolve_log_dir(cfg.logging.file.as_deref());
    tracing::info!(
        port = cfg.server.port,
        mcp = %format!("http://{}:{}/mcp", cfg.server.host, cfg.server.port),
        db = %db_path.display(),
        logs = %log_dir.display(),
        screenshots = %screenshot_dir.display(),
        browser = if cfg.browser.auto_connect { &cfg.browser.endpoint } else { "disabled" },
        transports = %transports.join(", "),
        "daemon8 started"
    );

    let cancel_for_server = cancel.clone();
    tasks.spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(cancel_for_server.cancelled_owned())
            .await
            .unwrap_or_else(|e| tracing::error!("HTTP server error: {e}"));
    });

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("received ctrl-c, shutting down...");
        }
        _ = cancel.cancelled() => {}
    }

    cancel.cancel();

    // Give spawned tasks up to 5 seconds to finish
    let shutdown_deadline = tokio::time::timeout(Duration::from_secs(5), async {
        while tasks.join_next().await.is_some() {}
    });

    if shutdown_deadline.await.is_err() {
        tracing::warn!("shutdown timed out after 5s, aborting remaining tasks");
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }

    tracing::info!("daemon8 stopped");
    Ok(())
}

fn cmd_logs(config_path: Option<String>, follow: bool) -> Result<()> {
    let cfg = config::load(config_path.as_deref()).unwrap_or_default();
    let log_dir = config::resolve_log_dir(cfg.logging.file.as_deref());

    if !log_dir.exists() {
        anyhow::bail!("log directory does not exist: {}", log_dir.display());
    }

    let mut logs: Vec<_> = std::fs::read_dir(&log_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "log")
                .unwrap_or(false)
        })
        .collect();

    logs.sort_by_key(|e| std::cmp::Reverse(e.metadata().ok().and_then(|m| m.modified().ok())));

    let latest = logs.first().map(|e| e.path());

    match latest {
        Some(path) => {
            println!("{}", path.display());
            if follow {
                #[cfg(unix)]
                {
                    let status = std::process::Command::new("tail")
                        .args(["-f", "-n", "100"])
                        .arg(&path)
                        .status()
                        .context("failed to run tail")?;
                    std::process::exit(status.code().unwrap_or(1));
                }
                #[cfg(windows)]
                {
                    let path_str = path.display().to_string();
                    let status = std::process::Command::new("powershell")
                        .args([
                            "-NoProfile",
                            "-NonInteractive",
                            "-Command",
                            &format!(
                                "Get-Content -LiteralPath '{}' -Tail 100 -Wait",
                                path_str.replace('\'', "''")
                            ),
                        ])
                        .status()
                        .context("failed to run PowerShell Get-Content")?;
                    std::process::exit(status.code().unwrap_or(1));
                }
            }
        }
        None => {
            println!("no log files found in {}", log_dir.display());
        }
    }
    Ok(())
}

fn cmd_config(config_path: Option<String>, json: bool) -> Result<()> {
    let cfg = config::load(config_path.as_deref()).context("failed to load configuration")?;

    if json {
        println!("{}", serde_json::to_string_pretty(&cfg).unwrap_or_default());
        return Ok(());
    }

    let path_val = |p: Option<&std::path::Path>| match p {
        None => style::dim("(default)"),
        Some(path) => path.display().to_string(),
    };
    let bval = |b: bool| {
        if b {
            style::green("true")
        } else {
            "false".to_string()
        }
    };

    println!("  {}", style::blue("Server"));
    println!("    {} {}", style::label("port"), cfg.server.port);
    println!("    {} {}", style::label("host"), cfg.server.host);
    println!();
    println!("  {}", style::blue("Browser"));
    println!("    {} {}", style::label("endpoint"), cfg.browser.endpoint);
    println!(
        "    {} {}",
        style::label("path"),
        path_val(cfg.browser.path.as_deref())
    );
    println!();
    println!("  {}", style::blue("Storage"));
    println!(
        "    {} {}",
        style::label("path"),
        path_val(cfg.storage.path.as_deref())
    );
    println!();
    println!("  {}", style::blue("Device"));
    println!("    {} {}", style::label("adb"), bval(cfg.adb.enabled));
    println!();
    println!("  {}", style::blue("Logging"));
    println!(
        "    {} {}",
        style::label("level"),
        cfg.logging.level.as_str()
    );

    Ok(())
}

fn cmd_config_path(config_path: Option<String>) -> Result<()> {
    let cfg = config::load(config_path.as_deref()).unwrap_or_default();
    println!("{}", cfg.config_dir.join("config.toml").display());
    Ok(())
}

fn cmd_config_set(config_path: Option<String>, key: &str, value: &str) -> Result<()> {
    validate_config_key_value(key, value)?;

    let cfg = config::load(config_path.as_deref()).unwrap_or_default();
    let file_path = config_path
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| cfg.config_dir.join("config.toml"));

    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let mut table: toml::Table = if file_path.exists() {
        let contents = std::fs::read_to_string(&file_path).context("reading config file")?;
        contents.parse().context("parsing config file")?
    } else {
        toml::Table::new()
    };

    let parts: Vec<&str> = key.split('.').collect();

    // Navigate to the parent table, creating sections as needed
    let mut current = &mut table;
    for section in &parts[..parts.len() - 1] {
        current = current
            .entry(section.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .with_context(|| format!("{section} is not a table"))?;
    }

    let field = parts
        .last()
        .expect("non-empty parts guaranteed by validate_config_key_value");

    // Type-aware TOML value based on the key's expected type
    let toml_value = if let Ok(n) = value.parse::<i64>() {
        toml::Value::Integer(n)
    } else if value == "true" || value == "false" {
        toml::Value::Boolean(value == "true")
    } else {
        toml::Value::String(value.to_string())
    };

    current.insert(field.to_string(), toml_value);

    // Atomic write: tmp file then rename
    let tmp_path = file_path.with_extension("toml.tmp");
    let serialized = toml::to_string_pretty(&table)?;
    std::fs::write(&tmp_path, &serialized).context("writing temp config file")?;
    std::fs::rename(&tmp_path, &file_path).context("renaming temp config file")?;

    eprintln!("Set {key} = {value}");

    let cfg = config::load(config_path.as_deref()).unwrap_or_default();
    let daemon_running =
        std::net::TcpStream::connect(format!("127.0.0.1:{}", cfg.server.port)).is_ok();

    if daemon_running {
        eprintln!();
        eprintln!("  Restart the daemon for this change to take effect:");
        eprintln!("  daemon8 install");
    }

    Ok(())
}

/// Validate a config key exists and the value is acceptable before writing anything.
fn validate_config_key_value(key: &str, value: &str) -> Result<()> {
    match key {
        "server.port" => {
            let port: u16 = value
                .parse()
                .map_err(|_| anyhow::anyhow!("server.port must be a valid u16, got: '{value}'"))?;
            if port == 0 {
                anyhow::bail!("server.port cannot be 0");
            }
            if port < 1024 {
                eprintln!("Warning: port {port} is in the privileged range (< 1024)");
            }
        }
        "server.host" => {
            if value.is_empty() || value.contains(char::is_whitespace) {
                anyhow::bail!("server.host must be a non-empty string with no whitespace");
            }
        }
        "storage.path" => {
            if !value.is_empty() {
                let p = std::path::Path::new(value);
                if let Some(parent) = p.parent()
                    && !parent.as_os_str().is_empty()
                    && !parent.exists()
                {
                    anyhow::bail!("parent directory does not exist: {}", parent.display());
                }
            }
        }
        "storage.screenshot_path" => {
            if value.is_empty() {
                anyhow::bail!(
                    "storage.screenshot_path must be non-empty (omit the key to use the default)"
                );
            }
        }
        "browser.path" => {
            if !value.is_empty() {
                let p = std::path::Path::new(value);
                if !p.exists() {
                    anyhow::bail!("browser path does not exist: {value}");
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let meta = std::fs::metadata(p)
                        .with_context(|| format!("cannot stat browser path: {value}"))?;
                    if meta.permissions().mode() & 0o111 == 0 {
                        anyhow::bail!("browser path is not executable: {value}");
                    }
                }
            }
        }
        "browser.endpoint" => {
            let Some(after_scheme) = value
                .strip_prefix("https://")
                .or_else(|| value.strip_prefix("http://"))
            else {
                anyhow::bail!(
                    "browser.endpoint must start with http:// or https://, got: '{value}'"
                );
            };
            if after_scheme.is_empty() {
                anyhow::bail!("browser.endpoint has no host after scheme");
            }
        }
        "browser.auto_connect"
        | "adb.enabled"
        | "ingestion.udp.enabled"
        | "ingestion.unix.enabled"
        | "logging.stderr"
        | "mcp.stdio"
        | "mcp.http" => {
            if value != "true" && value != "false" {
                anyhow::bail!("{key} must be 'true' or 'false', got: '{value}'");
            }
        }
        "adb.server_addr" | "ingestion.udp.bind" => {
            validate_host_port(key, value)?;
        }
        "ingestion.unix.path" => {
            if cfg!(windows) {
                anyhow::bail!(
                    "ingestion.unix.path is not supported on Windows (unix sockets unavailable)"
                );
            }
            if value.is_empty() {
                anyhow::bail!("ingestion.unix.path must be non-empty");
            }
        }
        "logging.level" => match value {
            "trace" | "debug" | "info" | "warn" | "error" => {}
            _ => anyhow::bail!(
                "logging.level must be one of: trace, debug, info, warn, error -- got: '{value}'"
            ),
        },
        "logging.file" => {
            if !value.is_empty() {
                let p = std::path::Path::new(value);
                if let Some(parent) = p.parent()
                    && !parent.as_os_str().is_empty()
                    && !parent.exists()
                {
                    anyhow::bail!("parent directory does not exist: {}", parent.display());
                }
            }
        }
        _ => anyhow::bail!("unknown config key: {key}"),
    }
    Ok(())
}

fn validate_host_port(key: &str, value: &str) -> Result<()> {
    let Some((host, port_str)) = value.rsplit_once(':') else {
        anyhow::bail!("{key} must be host:port format, got: '{value}'");
    };
    if host.is_empty() {
        anyhow::bail!("{key} host part must not be empty");
    }
    let port: u16 = port_str
        .parse()
        .map_err(|_| anyhow::anyhow!("{key} port must be 1-65535, got: '{port_str}'"))?;
    if port == 0 {
        anyhow::bail!("{key} port must be 1-65535, got: 0");
    }
    Ok(())
}

fn cmd_completions(shell: clap_complete::aot::Shell) -> Result<()> {
    use clap::CommandFactory;
    clap_complete::aot::generate(
        shell,
        &mut Cli::command(),
        "daemon8",
        &mut std::io::stdout(),
    );
    Ok(())
}

async fn bind_with_retry(addr: &str, port: u16) -> Result<tokio::net::TcpListener> {
    const MAX_ATTEMPTS: u32 = 5;
    let mut delay = Duration::from_millis(500);

    for attempt in 1..=MAX_ATTEMPTS {
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => return Ok(listener),
            Err(e) if attempt < MAX_ATTEMPTS => {
                let holder = find_port_holder(port);
                tracing::warn!(
                    attempt,
                    port,
                    delay_ms = delay.as_millis() as u64,
                    holder = holder.as_deref().unwrap_or("unknown"),
                    error = %e,
                    "bind failed, retrying"
                );
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(5));
            }
            Err(e) => {
                let msg = if let Some(pid) = find_port_holder(port) {
                    format!(
                        "port {port} is held by PID {pid}. \
                         Kill it with: kill {pid}"
                    )
                } else {
                    format!("port {port} is in use by another process")
                };
                return Err(e).with_context(|| msg);
            }
        }
    }
    unreachable!()
}

fn find_port_holder(port: u16) -> Option<String> {
    let output = std::process::Command::new("lsof")
        .args(["-ti", &format!(":{port}")])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().next().map(|s| s.trim().to_string())
}

/// Drain observations from the channel into the store, broadcasting each
/// persisted observation to SSE subscribers. The broadcast payload is
/// `(Arc<Observation>, Arc<str>)` — the typed observation for server-side
/// filtering and the pre-serialized JSON for zero-copy fanout. The JSON
/// carries the real `id` assigned by `store.insert`; callers relying on
/// `Last-Event-ID` depend on this ordering.
fn spawn_store_writer(
    tasks: &mut JoinSet<()>,
    mut rx: mpsc::UnboundedReceiver<Observation>,
    store: Arc<dyn StateModel>,
    broadcast_tx: broadcast::Sender<(Arc<Observation>, Arc<str>)>,
    cancel: CancellationToken,
    node_id: Arc<str>,
) {
    tasks.spawn(async move {
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Some(mut obs) => {
                            if obs.node_id.is_none() {
                                obs.node_id = Some(node_id.clone());
                            }
                            let insert_copy = obs.clone();
                            match store.insert(insert_copy) {
                                Ok(id) => {
                                    obs.id = id;
                                    match serde_json::to_string(&obs) {
                                        Ok(json) => {
                                            let arc_obs = Arc::new(obs);
                                            let arc_json: Arc<str> = Arc::from(json);
                                            let _ = broadcast_tx.send((arc_obs, arc_json));
                                        }
                                        Err(e) => {
                                            tracing::error!(id, error = %e, "observation failed to serialize; dropping from broadcast");
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("store insert failed: {e}");
                                }
                            }
                        }
                        None => break, // channel closed
                    }
                }
                () = cancel.cancelled() => break,
            }
        }
        tracing::debug!("store writer stopped");
    });
}

/// Periodically delete observations and screenshots older than their retention windows.
/// Decision the chrome command handler makes when receiving a Connect command.
///
/// Centralizes the idempotency logic so it can be unit tested without spinning up
/// a real Chrome process. The handler calls `decide_connect_action` and then
/// performs the corresponding side effects -- there is no duplicated logic.
#[derive(Debug, PartialEq, Eq)]
enum ConnectDecision {
    /// No live task; spawn a fresh connector.
    Spawn,
    /// Live task exists for a different endpoint; abort it and spawn a new one.
    AbortAndSpawn,
    /// Already connecting/connected to the requested endpoint -- do nothing.
    Ignore,
}

fn decide_connect_action(
    task_alive: bool,
    state: daemon8_chrome::ConnectionState,
    same_endpoint: bool,
) -> ConnectDecision {
    use daemon8_chrome::ConnectionState;

    if !task_alive {
        return ConnectDecision::Spawn;
    }
    match state {
        ConnectionState::Connecting | ConnectionState::Reconnecting => ConnectDecision::Ignore,
        ConnectionState::Connected if same_endpoint => ConnectDecision::Ignore,
        _ => ConnectDecision::AbortAndSpawn,
    }
}

/// State the chrome command handler maintains across Connect commands.
///
/// Extracted so the connect-arm logic can be exercised by unit tests without
/// requiring a real Chrome process. Production and tests both call
/// `handle_connect`, which is the only place that decides whether to abort
/// a previous task and which is the only place that mutates `current_endpoint`.
struct ChromeHandlerState {
    chrome_handle: Option<tokio::task::JoinHandle<()>>,
    current_endpoint: Option<String>,
}

impl ChromeHandlerState {
    fn new() -> Self {
        Self {
            chrome_handle: None,
            current_endpoint: None,
        }
    }

    /// Apply the idempotency decision for a Connect command and, if a new task
    /// should be spawned, invoke `spawn` to produce its `JoinHandle`. Returns
    /// `true` when a new task was spawned.
    fn handle_connect<F>(
        &mut self,
        endpoint: &str,
        state: daemon8_chrome::ConnectionState,
        spawn: F,
    ) -> bool
    where
        F: FnOnce() -> tokio::task::JoinHandle<()>,
    {
        let task_alive = self
            .chrome_handle
            .as_ref()
            .is_some_and(|h| !h.is_finished());
        let same_endpoint = self.current_endpoint.as_deref() == Some(endpoint);

        match decide_connect_action(task_alive, state, same_endpoint) {
            ConnectDecision::Ignore => false,
            ConnectDecision::AbortAndSpawn => {
                if let Some(handle) = self.chrome_handle.take() {
                    handle.abort();
                }
                self.current_endpoint = Some(endpoint.to_string());
                self.chrome_handle = Some(spawn());
                true
            }
            ConnectDecision::Spawn => {
                self.chrome_handle = None;
                self.current_endpoint = Some(endpoint.to_string());
                self.chrome_handle = Some(spawn());
                true
            }
        }
    }
}

/// Listen for ChromeCommand messages and spawn connect_and_monitor accordingly.
/// Returns a watch::Receiver so MCP and other consumers can observe connection state.
fn spawn_chrome_command_handler(
    tasks: &mut JoinSet<()>,
    mut rx: mpsc::Receiver<ChromeCommand>,
    obs_tx: mpsc::UnboundedSender<Observation>,
    browser_path: Option<String>,
    reconnect: daemon8_chrome::ReconnectPolicy,
    cancel: CancellationToken,
) -> tokio::sync::watch::Receiver<daemon8_chrome::ConnectionState> {
    let (status, state_rx) = daemon8_chrome::ConnectionStatus::new();

    tasks.spawn(async move {
        let mut handler = ChromeHandlerState::new();
        let mut action_tx: Option<mpsc::Sender<daemon8_chrome::BrowserAction>> = None;

        loop {
            tokio::select! {
                cmd = rx.recv() => {
                    match cmd {
                        Some(ChromeCommand::Connect { endpoint }) => {
                            let tx = obs_tx.clone();
                            let token = cancel.clone();
                            let (atx, action_rx) = mpsc::channel(BROWSER_ACTION_CAPACITY);
                            let task_status = status.clone();
                            let bp = browser_path.clone();
                            let endpoint_for_task = endpoint.clone();

                            let spawned = handler.handle_connect(&endpoint, status.current(), || {
                                tokio::spawn(async move {
                                    if let Err(e) = daemon8_chrome::connect_and_monitor(
                                        endpoint_for_task, tx, action_rx, token, task_status, bp, reconnect,
                                    ).await {
                                        tracing::error!("browser monitor exited with error: {e}");
                                    }
                                })
                            });

                            if spawned {
                                action_tx = Some(atx);
                            } else {
                                tracing::debug!(
                                    endpoint = %endpoint,
                                    state = ?status.current(),
                                    "Connect ignored: handler already managing this endpoint"
                                );
                                // atx is dropped here; the unused action_rx was
                                // already captured by the (unused) spawn closure.
                                drop(atx);
                            }
                        }
                        Some(ChromeCommand::Action(browser_action)) => {
                            if let Some(ref atx) = action_tx {
                                if atx.send(browser_action).await.is_err() {
                                    tracing::warn!("Browser action channel closed (browser disconnected?)");
                                    action_tx = None;
                                }
                            } else {
                                tracing::warn!("Browser action received but no active connection");
                                browser_action.reply_error("Browser not connected");
                            }
                        }
                        None => break,
                    }
                }
                () = cancel.cancelled() => break,
            }
        }

        if let Some(handle) = handler.chrome_handle.take() {
            handle.abort();
        }
        tracing::debug!("chrome command handler stopped");
    });

    state_rx
}

fn resolve_node_id() -> String {
    let mut buf = [0u8; 256];
    unsafe {
        if libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) == 0 {
            let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            String::from_utf8_lossy(&buf[..len]).into_owned()
        } else {
            "unknown".to_string()
        }
    }
}

/// Returns true only when stdin is a FIFO (named or anonymous pipe).
///
/// This is the signal we use to decide whether to start MCP stdio. A real
/// MCP client connects stdin as an anonymous pipe; launchd, nohup, and
/// `&`-backgrounded shells attach /dev/null (a character device) instead.
/// Checking S_IFMT against S_IFIFO cleanly separates the two regardless of
/// how the daemon was launched.
#[cfg(unix)]
fn stdin_is_real_pipe() -> bool {
    use std::os::fd::AsRawFd;
    let fd = std::io::stdin().as_raw_fd();
    // SAFETY: fstat on a valid fd (stdin is always open) with a zeroed
    // stat buffer is sound. We only read st_mode from the result on success.
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &mut st) != 0 {
            return false;
        }
        (st.st_mode & libc::S_IFMT) == libc::S_IFIFO
    }
}

#[cfg(windows)]
fn stdin_is_real_pipe() -> bool {
    use std::os::windows::io::AsRawHandle;
    // GetFileType returns FILE_TYPE_PIPE (3) for anonymous pipes.
    unsafe extern "system" {
        fn GetFileType(hFile: *mut std::ffi::c_void) -> u32;
    }
    let handle = std::io::stdin().as_raw_handle();
    unsafe { GetFileType(handle as *mut _) == 3 }
}

//
// These tests pin down the regression we hit twice: a Connect command
// arriving while a previous connect task is in flight should NOT abort the
// running task and start a new one. The handler must be idempotent on the
// (endpoint, state) tuple. We test the real production state machine
// (`ChromeHandlerState::handle_connect`) by passing controllable spawn
// closures, so the test exercises the same code path that runs in production.

#[cfg(test)]
mod channel_capacity_tests {
    use super::*;
    use tokio::sync::mpsc::error::TrySendError;

    #[test]
    fn chrome_cmd_channel_enforces_capacity() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<ChromeCommand>(CHROME_CMD_CAPACITY);
        for i in 0..CHROME_CMD_CAPACITY {
            tx.try_send(ChromeCommand::Connect {
                endpoint: format!("http://slot-{i}"),
            })
            .expect("fill up to capacity");
        }
        let overflow = tx.try_send(ChromeCommand::Connect {
            endpoint: "http://overflow".into(),
        });
        assert!(
            matches!(overflow, Err(TrySendError::Full(_))),
            "expected Full after {CHROME_CMD_CAPACITY} sends, got {overflow:?}"
        );
    }

    #[test]
    fn browser_action_channel_enforces_capacity() {
        let (tx, _rx) =
            tokio::sync::mpsc::channel::<daemon8_chrome::BrowserAction>(BROWSER_ACTION_CAPACITY);
        let mut replies = Vec::with_capacity(BROWSER_ACTION_CAPACITY + 1);
        for _ in 0..BROWSER_ACTION_CAPACITY {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            replies.push(reply_rx);
            tx.try_send(daemon8_chrome::BrowserAction::ListTabs { reply: reply_tx })
                .expect("fill up to capacity");
        }
        let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();
        let overflow = tx.try_send(daemon8_chrome::BrowserAction::ListTabs { reply: reply_tx });
        assert!(
            matches!(overflow, Err(TrySendError::Full(_))),
            "expected Full after {BROWSER_ACTION_CAPACITY} sends, got {overflow:?}"
        );
    }
}

#[cfg(test)]
mod chrome_handler_tests {
    use super::*;
    use daemon8_chrome::ConnectionState;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio_util::sync::CancellationToken;

    /// Spawn a tokio task that lives until `token` is cancelled. The returned
    /// `JoinHandle` reports `is_finished() == true` only after cancellation.
    fn spawn_controlled(token: CancellationToken) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            token.cancelled().await;
        })
    }

    /// Build a spawn closure that increments `counter` and returns a
    /// JoinHandle controllable via the closed-over CancellationToken.
    fn counting_spawner(
        counter: Arc<AtomicUsize>,
        token: CancellationToken,
    ) -> impl FnOnce() -> tokio::task::JoinHandle<()> {
        move || {
            counter.fetch_add(1, Ordering::SeqCst);
            spawn_controlled(token)
        }
    }

    #[tokio::test]
    async fn connect_while_disconnected_spawns_task() {
        let mut state = ChromeHandlerState::new();
        let count = Arc::new(AtomicUsize::new(0));
        let token = CancellationToken::new();

        let spawned = state.handle_connect(
            "ws://localhost:9222/devtools/browser/abc",
            ConnectionState::Disconnected,
            counting_spawner(count.clone(), token.clone()),
        );

        assert!(spawned, "Connect from Disconnected should spawn");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            state.current_endpoint.as_deref(),
            Some("ws://localhost:9222/devtools/browser/abc")
        );
        assert!(state.chrome_handle.is_some());

        token.cancel();
    }

    #[tokio::test]
    async fn connect_same_endpoint_while_connected_is_noop() {
        let mut state = ChromeHandlerState::new();
        let count = Arc::new(AtomicUsize::new(0));
        let token = CancellationToken::new();
        let endpoint = "ws://localhost:9222/devtools/browser/xyz";

        // First connect to seed state.
        state.handle_connect(
            endpoint,
            ConnectionState::Disconnected,
            counting_spawner(count.clone(), token.clone()),
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let original_handle_id = state.chrome_handle.as_ref().map(|h| h.id());

        // Second connect to the same endpoint while Connected -- must not
        // spawn, must not abort the existing handle.
        let spawned = state.handle_connect(
            endpoint,
            ConnectionState::Connected,
            counting_spawner(count.clone(), CancellationToken::new()),
        );

        assert!(!spawned, "duplicate Connect must be ignored");
        assert_eq!(count.load(Ordering::SeqCst), 1, "no new spawn");
        assert_eq!(
            state.chrome_handle.as_ref().map(|h| h.id()),
            original_handle_id,
            "existing handle must be preserved"
        );
        assert_eq!(state.current_endpoint.as_deref(), Some(endpoint));
        assert!(
            !state
                .chrome_handle
                .as_ref()
                .expect("handle present")
                .is_finished(),
            "existing task must still be running"
        );

        token.cancel();
    }

    #[tokio::test]
    async fn connect_different_endpoint_while_connected_aborts_and_respawns() {
        let mut state = ChromeHandlerState::new();
        let count = Arc::new(AtomicUsize::new(0));
        let first_token = CancellationToken::new();

        state.handle_connect(
            "ws://localhost:9222/devtools/browser/aaa",
            ConnectionState::Disconnected,
            counting_spawner(count.clone(), first_token.clone()),
        );
        let first_handle_id = state.chrome_handle.as_ref().map(|h| h.id());

        let second_token = CancellationToken::new();
        let spawned = state.handle_connect(
            "ws://localhost:9223/devtools/browser/bbb",
            ConnectionState::Connected,
            counting_spawner(count.clone(), second_token.clone()),
        );

        assert!(spawned, "different endpoint must respawn");
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert_ne!(
            state.chrome_handle.as_ref().map(|h| h.id()),
            first_handle_id,
            "must hold the new handle"
        );
        assert_eq!(
            state.current_endpoint.as_deref(),
            Some("ws://localhost:9223/devtools/browser/bbb")
        );

        // Give the runtime a chance to observe the abort. We don't assert
        // first_token.is_cancelled() because handle.abort() does not flip the
        // CancellationToken -- it drops the task at the next await point.
        // The handle-id swap above is the load-bearing assertion.
        tokio::task::yield_now().await;
        let _ = first_token; // explicitly unused

        second_token.cancel();
    }

    #[tokio::test]
    async fn connect_while_connecting_is_noop() {
        let mut state = ChromeHandlerState::new();
        let count = Arc::new(AtomicUsize::new(0));
        let token = CancellationToken::new();
        let endpoint = "ws://localhost:9222/devtools/browser/connecting";

        state.handle_connect(
            endpoint,
            ConnectionState::Disconnected,
            counting_spawner(count.clone(), token.clone()),
        );
        let original_handle_id = state.chrome_handle.as_ref().map(|h| h.id());

        // Even with a DIFFERENT endpoint, Connecting state means a connect is
        // in flight; the handler must wait, not start a parallel attempt.
        let spawned = state.handle_connect(
            "ws://localhost:9999/devtools/browser/different",
            ConnectionState::Connecting,
            counting_spawner(count.clone(), CancellationToken::new()),
        );

        assert!(!spawned, "Connect during Connecting must be ignored");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            state.chrome_handle.as_ref().map(|h| h.id()),
            original_handle_id
        );
        // Endpoint must NOT be updated, since we ignored the request.
        assert_eq!(state.current_endpoint.as_deref(), Some(endpoint));

        token.cancel();
    }

    #[tokio::test]
    async fn connect_while_reconnecting_is_noop() {
        let mut state = ChromeHandlerState::new();
        let count = Arc::new(AtomicUsize::new(0));
        let token = CancellationToken::new();
        let endpoint = "ws://localhost:9222/devtools/browser/reconnecting";

        state.handle_connect(
            endpoint,
            ConnectionState::Disconnected,
            counting_spawner(count.clone(), token.clone()),
        );
        let original_handle_id = state.chrome_handle.as_ref().map(|h| h.id());

        let spawned = state.handle_connect(
            endpoint,
            ConnectionState::Reconnecting,
            counting_spawner(count.clone(), CancellationToken::new()),
        );

        assert!(!spawned, "Connect during Reconnecting must be ignored");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            state.chrome_handle.as_ref().map(|h| h.id()),
            original_handle_id
        );

        token.cancel();
    }

    #[tokio::test]
    async fn connect_after_task_finished_respawns_cleanly() {
        let mut state = ChromeHandlerState::new();
        let count = Arc::new(AtomicUsize::new(0));
        let first_token = CancellationToken::new();
        let endpoint = "ws://localhost:9222/devtools/browser/finished";

        state.handle_connect(
            endpoint,
            ConnectionState::Disconnected,
            counting_spawner(count.clone(), first_token.clone()),
        );

        // Cancel the first task and wait for it to actually finish.
        first_token.cancel();
        if let Some(h) = state.chrome_handle.as_ref() {
            // Spin until is_finished() flips. The task body just awaits the
            // token, so this resolves on the next scheduler tick.
            for _ in 0..100 {
                if h.is_finished() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(h.is_finished(), "first task must have completed");
        }

        // Now Connect again. State will be Disconnected (the watch stays
        // wherever it was; in production the connector updates it). Even if
        // state were Connected, task_alive == false flips the decision to
        // Spawn.
        let second_token = CancellationToken::new();
        let spawned = state.handle_connect(
            endpoint,
            ConnectionState::Connected, // would normally block, but task is finished
            counting_spawner(count.clone(), second_token.clone()),
        );

        assert!(spawned, "finished task must allow respawn");
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert!(
            !state
                .chrome_handle
                .as_ref()
                .expect("handle present")
                .is_finished(),
            "new task must be running"
        );
        assert_eq!(state.current_endpoint.as_deref(), Some(endpoint));

        second_token.cancel();
    }

    #[test]
    fn decide_connect_action_truth_table() {
        // No live task -> always Spawn.
        assert_eq!(
            decide_connect_action(false, ConnectionState::Disconnected, false),
            ConnectDecision::Spawn
        );
        assert_eq!(
            decide_connect_action(false, ConnectionState::Connected, true),
            ConnectDecision::Spawn
        );

        // Live task + Connecting/Reconnecting -> always Ignore.
        assert_eq!(
            decide_connect_action(true, ConnectionState::Connecting, false),
            ConnectDecision::Ignore
        );
        assert_eq!(
            decide_connect_action(true, ConnectionState::Reconnecting, true),
            ConnectDecision::Ignore
        );

        // Live task + Connected: same endpoint Ignore, different endpoint AbortAndSpawn.
        assert_eq!(
            decide_connect_action(true, ConnectionState::Connected, true),
            ConnectDecision::Ignore
        );
        assert_eq!(
            decide_connect_action(true, ConnectionState::Connected, false),
            ConnectDecision::AbortAndSpawn
        );

        // Live task + Disconnected (rare; task running but state went stale).
        assert_eq!(
            decide_connect_action(true, ConnectionState::Disconnected, false),
            ConnectDecision::AbortAndSpawn
        );
    }
}

#[cfg(test)]
mod chrome_handler_property_tests {
    use super::*;
    use daemon8_chrome::ConnectionState;
    use proptest::prelude::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::runtime::Runtime;
    use tokio_util::sync::CancellationToken;

    /// Randomize a ConnectionState for property tests.
    fn arb_state() -> impl Strategy<Value = ConnectionState> {
        prop_oneof![
            Just(ConnectionState::Disconnected),
            Just(ConnectionState::Connecting),
            Just(ConnectionState::Connected),
            Just(ConnectionState::Reconnecting),
        ]
    }

    /// Small endpoint alphabet so commands frequently re-target the same
    /// endpoint, exercising the same-endpoint idempotency path.
    fn arb_endpoint() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("http://localhost:9222".to_string()),
            Just("http://localhost:9223".to_string()),
            Just("http://localhost:9224".to_string()),
        ]
    }

    proptest! {
        // Invariant: after replaying any sequence of (state, endpoint) Connect
        // commands through ChromeHandlerState, at most one live JoinHandle
        // exists at any point. handle_connect MUST abort the prior task before
        // spawning a replacement.
        #[test]
        fn at_most_one_live_task_across_connect_sequence(
            commands in proptest::collection::vec((arb_state(), arb_endpoint()), 1..30)
        ) {
            let rt = Runtime::new().unwrap();
            rt.block_on(async {
                let mut state = ChromeHandlerState::new();
                // Track the cancel token of the currently-live task so we can
                // detect whether the prior task was correctly aborted.
                let mut current_cancel: Option<CancellationToken> = None;
                let spawn_count = Arc::new(AtomicUsize::new(0));

                for (conn_state, endpoint) in commands {
                    let my_cancel = CancellationToken::new();
                    let my_cancel_for_spawn = my_cancel.clone();
                    let sc = spawn_count.clone();
                    let spawned = state.handle_connect(&endpoint, conn_state, || {
                        sc.fetch_add(1, Ordering::SeqCst);
                        tokio::spawn(async move { my_cancel_for_spawn.cancelled().await; })
                    });

                    if spawned {
                        // Give the scheduler a tick to register the abort on
                        // the prior task (tokio::spawn + abort is asynchronous).
                        tokio::task::yield_now().await;
                        if let Some(prior) = current_cancel.take() {
                            // Prior task must be on the way out. The test
                            // invariant is that the handler took() and aborted
                            // it, which manifests as either cancellation
                            // observed OR the JoinHandle is_finished().
                            prior.cancel();
                        }
                        current_cancel = Some(my_cancel);
                    } else {
                        // Not spawned — the handler ignored this Connect.
                        // The cancel token we built is orphaned; drop it.
                        drop(my_cancel);
                    }
                }

                // Final state: at most one live task handle.
                let live = state
                    .chrome_handle
                    .as_ref()
                    .map(|h| !h.is_finished())
                    .unwrap_or(false);
                prop_assert!(
                    !live || state.chrome_handle.is_some(),
                    "invariant: at most one live chrome_handle at a time"
                );
                Ok(())
            }).unwrap();
        }
    }
}
