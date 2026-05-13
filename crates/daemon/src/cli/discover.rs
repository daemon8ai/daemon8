// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! `daemon8 discover` — CLI surface for the discovery scanner (D3).
//!
//! Two escape hatches into a running scanner wait loop:
//!
//! - `--complete` POSTs to the running daemon's `/api/discover/complete`
//!   endpoint. The daemon's scanner short-circuits its wait loop on the
//!   next poll tick and returns with whatever templates are present.
//! - `--skip` writes `.daemon8/skip-discovery` under the project root.
//!   Future `daemon8 serve` invocations honor the marker and bypass
//!   discovery entirely. Also POSTs to the running daemon (if any) so
//!   an in-flight scan returns immediately.
//!
//! Without either flag the subcommand prints status text only — the
//! interactive `discover --rescan` flow lands in D4.

use anyhow::{Context, Result};

use crate::cli::observe::{ClientArgs, base_url, check_response, handle_reqwest_error};
use crate::discovery::scanner::SKIP_MARKER_REL_PATH;

#[derive(clap::Args, Debug)]
pub struct DiscoverArgs {
    /// Tell the running daemon's scanner to stop waiting and return
    /// with whatever templates the librarian currently has.
    #[arg(long, conflicts_with = "skip")]
    pub complete: bool,

    /// Write the per-project skip marker so future `daemon8 serve`
    /// runs do not invoke the scanner. Also signals any in-flight
    /// scanner to abort its wait loop.
    #[arg(long, conflicts_with = "complete")]
    pub skip: bool,

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

    if args.complete {
        post_signal(&args.client, "complete").await?;
        println!("discovery complete signal sent");
        return Ok(());
    }

    println!(
        "daemon8 discover: nothing to do. Pass --complete or --skip; \
         the interactive rescan flow lands with D4."
    );
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
