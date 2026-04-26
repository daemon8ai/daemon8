// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

mod cli;
mod cleanup;
mod cli_config;
mod config;
mod providers;
mod screenshot;
pub(crate) mod style;

use std::path::PathBuf;

use anyhow::Result;
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
    /// Install daemon8 as a system service (starts on login, restarts on crash)
    Install,
    /// Remove daemon8 system service
    Uninstall,
    /// Interactive setup wizard (run after install)
    Setup,
    /// Real-time alert relay for Claude Code (experimental)
    Channel,
    /// Run a stateless background or one-shot daemon8 agent
    Agent(cli::agent::AgentArgs),
    /// Diagnose common configuration and environment issues
    Doctor {
        /// Attempt to fix issues that can be repaired automatically
        #[arg(long)]
        fix: bool,
    },
    /// Universal CLI hook handler (invoked by Claude/Cursor/Gemini/Codex/Copilot/Continue)
    #[command(name = "cli-hook", hide = true)]
    CliHook(cli::hook_handler::CliHookArgs),
    /// Initialize a `.daemon8.toml` at the current project
    Init(cli::init::InitArgs),
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
    };

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
        Commands::Install => cli::service::cmd_install(),
        Commands::Uninstall => cli::service::cmd_uninstall(),
        Commands::Setup => cli::setup::cmd_setup().await,
        Commands::Channel => cli::channel::cmd_channel().await,
        Commands::Agent(args) => cli::agent::run_agent(args).await,
        Commands::Doctor { fix } => cli::doctor::cmd_doctor(fix),
        Commands::CliHook(args) => cli::hook_handler::cmd_cli_hook(args),
        Commands::Init(args) => cli::init::cmd_init(args),
    }
}

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
