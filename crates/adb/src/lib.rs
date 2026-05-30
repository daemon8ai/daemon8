// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

pub mod controller;
pub mod device_manager;
mod error;
pub mod parser;
pub mod transport;

pub use controller::{DeviceController, controller_for};
pub use error::{AdbError, Result};

use std::net::SocketAddrV4;

use daemon8_types::Observation;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use device_manager::DeviceManager;

/// Start the ADB device manager. Discovers devices, detects platform
/// (Android vs Vega), and streams parsed log observations.
///
/// Blocks until cancelled.
pub async fn connect_and_monitor(
    server_addr: SocketAddrV4,
    scan_interval_secs: u64,
    obs_tx: UnboundedSender<Observation>,
    cancel: CancellationToken,
) -> Result<()> {
    let mut mgr = DeviceManager::new(server_addr, obs_tx, cancel, scan_interval_secs);
    mgr.run().await
}
