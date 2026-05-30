// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Bridges the mcp `DeviceInputFn` closure seam to the adb `DeviceController`.
//! The daemon crate is the only place mcp and adb meet, keeping mcp adb-free.

use std::net::SocketAddrV4;
use std::sync::Arc;

use daemon8_adb::controller::controller_for;
use daemon8_adb::transport::AdbTransport;
use daemon8_mcp::DeviceInputFn;
use daemon8_types::DeviceInput;

/// Build the device input callback the MCP server invokes for key/text/tap.
pub fn build_device_input_fn(addr: SocketAddrV4) -> DeviceInputFn {
    let transport = Arc::new(AdbTransport::new(addr));

    Arc::new(move |serial, platform, input| {
        let transport = transport.clone();
        Box::pin(async move {
            let controller = controller_for(transport, serial, platform);
            match input {
                DeviceInput::Key { key } => controller.key(key).await,
                DeviceInput::Text { text } => controller.text(&text).await,
                DeviceInput::Tap { x, y } => controller.tap(x, y).await,
            }
            .map_err(anyhow::Error::from)
        })
    })
}
