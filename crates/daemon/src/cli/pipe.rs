// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::net::SocketAddr;

use anyhow::Result;
use clap::Args;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UdpSocket;

#[derive(Args)]
pub struct PipeArgs {
    /// Channel name for filtering (e.g. "laravel-dev", "vite")
    #[arg(long)]
    channel: Option<String>,

    /// Application name
    #[arg(long)]
    app: Option<String>,

    /// Comma-separated tags
    #[arg(long, value_delimiter = ',')]
    tags: Option<Vec<String>>,

    /// Default severity for all lines (default: info)
    #[arg(long, default_value = "info")]
    severity: String,
}

pub async fn cmd_pipe(args: PipeArgs) -> Result<()> {
    let cfg = crate::config::load(None).unwrap_or_default();
    let target: SocketAddr = cfg.ingestion.udp.bind;

    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.connect(target).await?;

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();

    while let Some(line) = reader.next_line().await? {
        println!("{line}");

        let payload = json!({
            "message": line,
            "kind": "log",
            "severity": args.severity,
            "channel": args.channel,
            "app": args.app,
            "tags": args.tags,
            "source": "pipe",
        });

        let bytes = serde_json::to_vec(&payload)?;
        let _ = socket.send(&bytes).await;
    }

    // stdin closed — emit exit observation
    let exit_payload = json!({
        "message": "pipe: stdin closed (upstream process exited)",
        "kind": "lifecycle",
        "severity": "warn",
        "channel": args.channel,
        "app": args.app,
        "source": "pipe",
    });
    let bytes = serde_json::to_vec(&exit_payload)?;
    let _ = socket.send(&bytes).await;

    Ok(())
}
