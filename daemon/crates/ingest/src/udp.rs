// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use tokio::net::UdpSocket;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use daemon8_types::Observation;

use crate::Result;
use crate::normalize;

pub async fn run_udp_listener(
    bind_addr: std::net::SocketAddr,
    max_packet: usize,
    tx: UnboundedSender<Observation>,
    cancel: CancellationToken,
) -> Result<()> {
    let socket = UdpSocket::bind(bind_addr).await?;
    tracing::info!(bind = %bind_addr, "UDP listener bound");

    let mut buf = vec![0u8; max_packet];

    loop {
        tokio::select! {
            result = socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, _addr)) => {
                        match serde_json::from_slice::<serde_json::Value>(&buf[..len]) {
                            Ok(value) => {
                                let obs = normalize::normalize(value);
                                let _ = tx.send(obs);
                            }
                            Err(e) => {
                                tracing::debug!(len, error = %e, "UDP: invalid JSON, dropping");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "UDP recv error");
                    }
                }
            }
            () = cancel.cancelled() => {
                tracing::debug!("UDP listener stopping");
                return Ok(());
            }
        }
    }
}
