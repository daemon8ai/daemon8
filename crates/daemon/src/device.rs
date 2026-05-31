// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Bridges the mcp `DeviceInputFn` closure seam to the adb `DeviceController`.
//! The daemon crate is the only place mcp and adb meet, keeping mcp adb-free.

use std::net::SocketAddrV4;
use std::sync::Arc;

use daemon8_adb::controller::controller_for;
use daemon8_adb::transport::AdbTransport;
use daemon8_mcp::DeviceInputFn;
use daemon8_types::{DeviceInput, DevicePlatform};

/// Build the device input callback the MCP server invokes for key/text/tap.
pub fn build_device_input_fn(
    addr: SocketAddrV4,
    android_enabled: bool,
    vvd_enabled: bool,
) -> DeviceInputFn {
    let transport = Arc::new(AdbTransport::new(addr));

    Arc::new(move |serial, platform, input| {
        let transport = transport.clone();
        Box::pin(async move {
            ensure_platform_enabled(&platform, android_enabled, vvd_enabled)?;
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

fn ensure_platform_enabled(
    platform: &DevicePlatform,
    android_enabled: bool,
    vvd_enabled: bool,
) -> anyhow::Result<()> {
    match platform {
        DevicePlatform::Android if android_enabled => Ok(()),
        DevicePlatform::Vega if vvd_enabled => Ok(()),
        DevicePlatform::Android => {
            anyhow::bail!("Android device input is disabled; run `daemon8 feature adb enable`")
        }
        DevicePlatform::Vega => {
            anyhow::bail!("VVD device input is disabled; run `daemon8 feature vvd enable`")
        }
    }
}
