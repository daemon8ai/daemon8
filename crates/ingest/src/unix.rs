// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::path::Path;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use daemon8_types::Observation;

use crate::Result;
use crate::normalize;

pub async fn run_unix_listener(
    path: &Path,
    tx: UnboundedSender<Observation>,
    cancel: CancellationToken,
) -> Result<()> {
    // Remove stale socket file from a previous run
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }

    let listener = UnixListener::bind(path)?;
    tracing::info!(path = %path.display(), "Unix socket listener bound");

    let cleanup_path = path.to_owned();
    let _cleanup = CleanupGuard(cleanup_path);

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let tx = tx.clone();
                        let cancel = cancel.clone();
                        tokio::spawn(async move {
                            handle_connection(stream, tx, cancel).await;
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Unix socket accept error");
                    }
                }
            }
            () = cancel.cancelled() => {
                tracing::debug!("Unix socket listener stopping");
                return Ok(());
            }
        }
    }
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    tx: UnboundedSender<Observation>,
    cancel: CancellationToken,
) {
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();

    loop {
        tokio::select! {
            result = lines.next_line() => {
                match result {
                    Ok(Some(line)) => {
                        match serde_json::from_str::<serde_json::Value>(&line) {
                            Ok(value) => {
                                let obs = normalize::normalize(value);
                                let _ = tx.send(obs);
                            }
                            Err(e) => {
                                tracing::debug!(error = %e, "Unix socket: invalid JSON line, skipping");
                            }
                        }
                    }
                    Ok(None) => break, // EOF
                    Err(e) => {
                        tracing::debug!(error = %e, "Unix socket read error");
                        break;
                    }
                }
            }
            () = cancel.cancelled() => break,
        }
    }
}

/// Removes the socket file when the listener shuts down.
struct CleanupGuard(std::path::PathBuf);

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
