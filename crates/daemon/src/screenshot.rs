// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::net::SocketAddrV4;
use std::sync::Arc;

use anyhow::{Context, Result};
use daemon8_adb::transport::AdbTransport;
use daemon8_mcp::{DeviceScreenshotFn, DeviceScreenshotResult};
use daemon8_types::DevicePlatform;

/// Capture a screenshot from a device, trying the best method available.
///
/// For Vega emulators: tries host window capture (xcap) first, falls back to
/// the Vega ADB screenshot tool. Android devices use ADB framebuffer capture.
pub async fn capture_device_screenshot(
    transport: &AdbTransport,
    serial: &str,
    platform: &DevicePlatform,
    android_enabled: bool,
    vvd_enabled: bool,
) -> Result<DeviceScreenshotResult> {
    ensure_platform_enabled(platform, android_enabled, vvd_enabled)?;

    let is_emulator = serial.starts_with("emulator-") || serial.contains("localhost:");

    if is_emulator && matches!(platform, DevicePlatform::Vega) {
        match try_host_window_capture(serial).await {
            Ok(bytes) => {
                return Ok(DeviceScreenshotResult {
                    png_bytes: bytes,
                    source: "host_window_capture".into(),
                });
            }
            Err(e) => {
                if !should_fallback_from_host_capture_error(&e) {
                    return Err(e);
                }
                tracing::debug!(serial, error = %e, "host window capture failed, trying ADB");
            }
        }
    }

    let bytes = transport
        .capture_screenshot(serial, platform)
        .await
        .with_context(|| format!("ADB screenshot failed for {serial}"))?;

    Ok(DeviceScreenshotResult {
        png_bytes: bytes,
        source: match platform {
            DevicePlatform::Android => "adb_framebuffer",
            DevicePlatform::Vega => "adb_screenshooter",
        }
        .into(),
    })
}

#[cfg(feature = "xcap")]
async fn try_host_window_capture(serial: &str) -> Result<Vec<u8>> {
    let serial = serial.to_string();

    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::task::spawn_blocking(move || capture_window_blocking(&serial)),
    )
    .await
    .context("host window capture timed out")?
    .context("host window capture task panicked")?
}

#[cfg(feature = "xcap")]
fn capture_window_blocking(serial: &str) -> Result<Vec<u8>> {
    #[cfg(target_os = "macos")]
    if !objc2_core_graphics::CGPreflightScreenCaptureAccess()
        && !objc2_core_graphics::CGRequestScreenCaptureAccess()
    {
        return Err(ScreenRecordingPermissionDenied.into());
    }

    let windows = xcap::Window::all().context("failed to enumerate windows")?;

    let window_patterns = ["Vega Virtual Device", "Android Emulator", serial];

    let window = windows
        .into_iter()
        .filter(|w| !w.is_minimized().unwrap_or(true))
        .find(|w| {
            let title = w.title().unwrap_or_default();
            window_patterns.iter().any(|p| title.contains(p))
        })
        .context("no matching emulator window found")?;

    let img = window.capture_image().context("window capture failed")?;

    let mut cursor = std::io::Cursor::new(Vec::new());
    image::DynamicImage::from(img)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .context("PNG encoding failed")?;

    Ok(cursor.into_inner())
}

#[derive(Debug)]
struct ScreenRecordingPermissionDenied;

impl std::fmt::Display for ScreenRecordingPermissionDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "macOS Screen Recording permission is not granted for daemon8; enable it in System Settings > Privacy & Security > Screen Recording",
        )
    }
}

impl std::error::Error for ScreenRecordingPermissionDenied {}

fn is_screen_recording_permission_denied(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<ScreenRecordingPermissionDenied>()
            .is_some()
    })
}

fn should_fallback_from_host_capture_error(error: &anyhow::Error) -> bool {
    !is_screen_recording_permission_denied(error)
}

#[cfg(not(feature = "xcap"))]
async fn try_host_window_capture(_serial: &str) -> Result<Vec<u8>> {
    anyhow::bail!("host window capture not available (compile with --features xcap)")
}

/// Build a screenshot callback for use in MCP tools.
pub fn build_screenshot_fn(
    addr: SocketAddrV4,
    android_enabled: bool,
    vvd_enabled: bool,
) -> DeviceScreenshotFn {
    let transport = Arc::new(AdbTransport::new(addr));

    Arc::new(move |serial: String, platform: DevicePlatform| {
        let transport = transport.clone();
        Box::pin(async move {
            capture_device_screenshot(&transport, &serial, &platform, android_enabled, vvd_enabled)
                .await
        })
    })
}

fn ensure_platform_enabled(
    platform: &DevicePlatform,
    android_enabled: bool,
    vvd_enabled: bool,
) -> Result<()> {
    match platform {
        DevicePlatform::Android if android_enabled => Ok(()),
        DevicePlatform::Vega if vvd_enabled => Ok(()),
        DevicePlatform::Android => {
            anyhow::bail!(
                "Android device screenshots are disabled; run `daemon8 feature adb enable`"
            )
        }
        DevicePlatform::Vega => {
            anyhow::bail!("VVD screenshots are disabled; run `daemon8 feature vvd enable`")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn detects_screen_recording_permission_error_through_context() {
        let error = Err::<(), _>(ScreenRecordingPermissionDenied)
            .context("host window capture failed")
            .unwrap_err();

        assert!(is_screen_recording_permission_denied(&error));
        assert!(!should_fallback_from_host_capture_error(&error));
    }

    #[test]
    fn host_capture_general_errors_can_fallback_to_adb() {
        let error = anyhow::anyhow!("no matching emulator window found");

        assert!(should_fallback_from_host_capture_error(&error));
    }
}
