// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

mod cleanup;
mod cli;
mod cli_config;
mod config;
mod screenshot;
mod sources;
pub(crate) mod style;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

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
    Serve(cli::serve::ServeArgs),
    /// Show daemon health and status
    Status(cli::observe::ClientArgs),
    /// Stream observations in real-time
    Tail(cli::observe::TailArgs),
    /// Query stored observations
    Query(cli::observe::QueryArgs),
    /// List active data source connections
    Connections(cli::observe::ClientArgs),
    /// Browser DevTools commands
    #[command(subcommand)]
    Browser(cli::browser::ChromeSubcommand),
    /// Manage per-session observation lens (filter + ring buffer)
    #[command(subcommand)]
    Lens(cli::lens::LensSubcommand),
    /// Show log file location or tail logs
    Logs {
        /// Follow the log file (like tail -f)
        #[arg(short, long)]
        follow: bool,
    },
    /// Show or modify configuration
    #[command(subcommand)]
    Config(cli::config_cmd::ConfigSubcommand),
    /// Generate shell completions
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::aot::Shell,
    },
    /// Report what daemon8 found in this project and register with detected agents
    #[command(subcommand)]
    Setup(SetupSubcommand),
    /// Manage daemon8 system service (install/uninstall)
    #[command(subcommand)]
    Service(ServiceSubcommand),
    /// Real-time alert relay for MCP clients (experimental)
    Channel,
    /// Diagnose common configuration and environment issues
    Doctor {
        /// Attempt to fix issues that can be repaired automatically
        #[arg(long)]
        fix: bool,
    },
    // Hidden aliases for backward compatibility
    #[command(hide = true)]
    Install,
    #[command(hide = true)]
    Uninstall,
    #[command(hide = true)]
    Init(cli::init::InitArgs),
    #[command(hide = true)]
    Features(cli::features::FeaturesArgs),
}

#[derive(Subcommand)]
enum SetupSubcommand {
    /// Register MCP with detected agents, then run the project-aware discovery scan
    Apply(cli::setup::SetupArgs),
    /// Enable optional features interactively
    Features(cli::features::FeaturesArgs),
    /// Write a `.daemon8.toml` override file for explicit source configuration
    Init(cli::init::InitArgs),
}

#[derive(Subcommand)]
enum ServiceSubcommand {
    /// Install daemon8 as a system service (starts on login, restarts on crash)
    Install,
    /// Remove daemon8 system service
    Uninstall,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let command = cli
        .command
        .unwrap_or(Commands::Serve(cli::serve::ServeArgs::default()));

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
    }?;

    match command {
        Commands::Serve(args) => cli::serve::cmd_serve(cli.config, args).await,
        Commands::Status(args) => cli::observe::status::cmd_status(args).await,
        Commands::Tail(args) => cli::observe::tail::cmd_tail(args).await,
        Commands::Query(args) => cli::observe::query::cmd_query(args).await,
        Commands::Connections(args) => cli::observe::connections::cmd_connections(args).await,
        Commands::Browser(sub) => cli::browser::cmd_chrome(sub).await,
        Commands::Lens(sub) => cli::lens::cmd_lens(sub).await,
        Commands::Logs { follow } => cli::logs::cmd_logs(cli.config, follow),
        Commands::Config(sub) => cli::config_cmd::cmd_config(cli.config, sub),
        Commands::Completions { shell } => {
            use clap::CommandFactory;
            cli::completions::cmd_completions(shell, &mut Cli::command())
        }
        Commands::Setup(sub) => match sub {
            SetupSubcommand::Apply(args) => cli::setup::cmd_setup(cli.config, args).await,
            SetupSubcommand::Features(args) => cli::features::cmd_features(args),
            SetupSubcommand::Init(args) => cli::init::cmd_init(args),
        },
        Commands::Service(sub) => match sub {
            ServiceSubcommand::Install => cli::service::cmd_install(),
            ServiceSubcommand::Uninstall => cli::service::cmd_uninstall(),
        },
        Commands::Channel => cli::channel::cmd_channel().await,
        Commands::Doctor { fix } => cli::doctor::cmd_doctor(cli.config, fix).await,
        // Hidden backward-compat aliases
        Commands::Install => cli::service::cmd_install(),
        Commands::Uninstall => cli::service::cmd_uninstall(),
        Commands::Init(args) => cli::init::cmd_init(args),
        Commands::Features(args) => cli::features::cmd_features(args),
    }
}

struct LogGuard {
    _file_guard: Option<tracing_appender::non_blocking::WorkerGuard>,
}

struct FileLogWriter {
    writer: tracing_appender::non_blocking::NonBlocking,
    guard: tracing_appender::non_blocking::WorkerGuard,
}

fn init_tracing(
    verbose: bool,
    logging: &config::LoggingConfig,
    file_enabled: bool,
) -> Result<LogGuard> {
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

    let log_dir = config::resolve_log_dir(logging.file.as_deref());
    let (file_layer, file_guard) = if file_enabled {
        let file_log = build_file_log_writer(logging)?;

        let layer = fmt::layer()
            .with_writer(file_log.writer)
            .with_ansi(false)
            .with_target(true);

        (Some(layer), Some(file_log.guard))
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

    tracing::debug!(
        level = daemon8_level,
        file_enabled,
        stderr = logging.stderr,
        log_dir = %log_dir.display(),
        max_log_files = logging.max_log_files,
        rotation = "daily",
        "logging initialized"
    );

    Ok(LogGuard {
        _file_guard: file_guard,
    })
}

fn build_file_log_writer(logging: &config::LoggingConfig) -> Result<FileLogWriter> {
    let log_dir = config::resolve_log_dir(logging.file.as_deref());
    std::fs::create_dir_all(&log_dir)
        .with_context(|| format!("creating log directory: {}", log_dir.display()))?;

    let file_appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("daemon8")
        .filename_suffix("log")
        .max_log_files(logging.max_log_files)
        .build(&log_dir)
        .with_context(|| format!("creating log file appender in {}", log_dir.display()))?;

    let (writer, guard) = tracing_appender::non_blocking(file_appender);
    Ok(FileLogWriter { writer, guard })
}

#[cfg(test)]
mod logging_tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    fn daemon_log_paths(log_dir: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Ok(entries) = std::fs::read_dir(log_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_daemon_log = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("daemon8.") && name.ends_with(".log"));
                if is_daemon_log {
                    paths.push(path);
                }
            }
        }
        paths.sort();
        paths
    }

    fn logging_config(log_dir: PathBuf) -> config::LoggingConfig {
        config::LoggingConfig {
            level: config::LogLevel::Debug,
            file: Some(log_dir),
            stderr: false,
            max_log_files: 3,
        }
    }

    #[test]
    fn file_logging_writes_and_prunes_old_logs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let log_dir = tmp.path().join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();

        for day in 1..=5 {
            let path = log_dir.join(format!("daemon8.2000-01-0{day}.log"));
            std::fs::write(path, format!("old log {day}\n")).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }

        let file_log = build_file_log_writer(&logging_config(log_dir.clone())).unwrap();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(file_log.writer)
                .with_ansi(false)
                .with_target(true),
        );
        let default_guard = tracing::subscriber::set_default(subscriber);
        tracing::info!(
            target: "daemon8",
            "narrow file logging test event"
        );
        drop(default_guard);
        drop(file_log.guard);

        let paths = daemon_log_paths(&log_dir);
        assert!(
            paths.len() <= 3,
            "expected max 3 retained daemon logs, found {}: {paths:?}",
            paths.len()
        );
        assert!(
            paths.iter().any(|path| std::fs::read_to_string(path)
                .unwrap_or_default()
                .contains("narrow file logging test event")),
            "retained logs should include the emitted test event: {paths:?}"
        );
    }

    #[test]
    fn file_logging_rejects_invalid_log_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let log_path = tmp.path().join("not-a-directory");
        std::fs::write(&log_path, "this is a file, not a directory").unwrap();

        let err = match build_file_log_writer(&logging_config(log_path)) {
            Ok(_) => panic!("invalid log path should fail"),
            Err(err) => err,
        };
        let message = format!("{err:#}");
        assert!(
            message.contains("creating log directory"),
            "error should explain log directory failure; error was: {message}"
        );
    }
}
