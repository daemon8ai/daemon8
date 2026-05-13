// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! `daemon8 discover` — CLI surface for the discovery scanner (D3/D4).
//!
//! Three escape hatches:
//!
//! - `--complete` POSTs to the running daemon's `/api/discover/complete`
//!   endpoint. The daemon's scanner short-circuits its wait loop on the
//!   next poll tick and returns with whatever templates are present.
//! - `--skip` writes `.daemon8/skip-discovery` under the project root.
//!   Future `daemon8 serve` invocations honor the marker and bypass
//!   discovery entirely. Also POSTs to the running daemon (if any) so
//!   an in-flight scan returns immediately.
//! - `--rescan` removes the per-project skip marker so the next
//!   `daemon8 serve` re-runs the scanner and re-presents the plan.
//!   Pure local operation — the running daemon caches the project
//!   node, so restart-on-rescan is the simplest contract.

use anyhow::{Context, Result};

use crate::cli::observe::{ClientArgs, base_url, check_response, handle_reqwest_error};
use crate::discovery::scanner::SKIP_MARKER_REL_PATH;

#[derive(clap::Args, Debug)]
pub struct DiscoverArgs {
    /// Tell the running daemon's scanner to stop waiting and return
    /// with whatever templates the librarian currently has.
    #[arg(long, conflicts_with_all = ["skip", "rescan"])]
    pub complete: bool,

    /// Write the per-project skip marker so future `daemon8 serve`
    /// runs do not invoke the scanner. Also signals any in-flight
    /// scanner to abort its wait loop.
    #[arg(long, conflicts_with_all = ["complete", "rescan"])]
    pub skip: bool,

    /// Remove the per-project skip marker so the next `daemon8 serve`
    /// re-runs the discovery scanner.
    #[arg(long, conflicts_with_all = ["complete", "skip"])]
    pub rescan: bool,

    #[command(flatten)]
    pub client: ClientArgs,
}

pub async fn cmd_discover(args: DiscoverArgs) -> Result<()> {
    if args.skip {
        let root = std::env::current_dir().context("reading current directory")?;
        let marker = root.join(SKIP_MARKER_REL_PATH);
        if let Some(parent) = marker.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("creating skip-marker parent dir at {}", parent.display())
            })?;
        }
        std::fs::write(&marker, b"discovery skipped\n")
            .with_context(|| format!("writing skip marker at {}", marker.display()))?;
        println!("skip marker written: {}", marker.display());
        // Best-effort signal to a running daemon. Failure here is
        // expected when the daemon is not running.
        let _ = post_signal(&args.client, "skip").await;
        return Ok(());
    }

    if args.rescan {
        let root = std::env::current_dir().context("reading current directory")?;
        let marker = root.join(SKIP_MARKER_REL_PATH);
        if marker.exists() {
            std::fs::remove_file(&marker)
                .with_context(|| format!("removing skip marker at {}", marker.display()))?;
            println!("skip marker removed: {}", marker.display());
        } else {
            println!("no skip marker to remove at {}", marker.display());
        }
        println!("restart `daemon8 serve` to re-run discovery for this project");
        return Ok(());
    }

    if args.complete {
        post_signal(&args.client, "complete").await?;
        println!("discovery complete signal sent");
        return Ok(());
    }

    println!("daemon8 discover: nothing to do. Pass --complete, --skip, or --rescan.");
    Ok(())
}

async fn post_signal(client_args: &ClientArgs, kind: &str) -> Result<()> {
    let port = client_args.resolved_port();
    let url = format!("{}/api/discover/{kind}", base_url(port));
    let resp = reqwest::Client::new()
        .post(&url)
        .send()
        .await
        .map_err(|e| handle_reqwest_error(e, port))?;
    check_response(resp).await?;
    Ok(())
}
