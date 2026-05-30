// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Device input control. Android and Vega diverge in how key/text/tap reach the
//! device, so each platform owns a `DeviceController` impl rather than scattering
//! `match platform` arms across the transport. The factory selects by `DevicePlatform`.

use std::sync::Arc;

use async_trait::async_trait;
use daemon8_types::{DeviceKey, DevicePlatform};

use crate::error::{AdbError, Result};
use crate::transport::AdbTransport;

/// Drives input on one connected device. One impl per platform.
#[async_trait]
pub trait DeviceController: Send + Sync {
    async fn key(&self, key: DeviceKey) -> Result<()>;
    async fn text(&self, text: &str) -> Result<()>;
    async fn tap(&self, x: f64, y: f64) -> Result<()>;
}

/// Select the controller for a device's platform.
pub fn controller_for(
    transport: Arc<AdbTransport>,
    serial: String,
    platform: DevicePlatform,
) -> Box<dyn DeviceController> {
    match platform {
        DevicePlatform::Android => Box::new(AndroidController { transport, serial }),
        DevicePlatform::Vega => Box::new(VegaController { transport, serial }),
    }
}

/// Android input via the universal `input` shell utility.
pub struct AndroidController {
    transport: Arc<AdbTransport>,
    serial: String,
}

#[async_trait]
impl DeviceController for AndroidController {
    async fn key(&self, key: DeviceKey) -> Result<()> {
        let cmd = format!("input keyevent {}", android_keycode(key));
        self.transport.shell_command(&self.serial, &cmd).await?;
        Ok(())
    }

    async fn text(&self, text: &str) -> Result<()> {
        let cmd = format!("input text {}", escape_input_text(text));
        self.transport.shell_command(&self.serial, &cmd).await?;
        Ok(())
    }

    async fn tap(&self, x: f64, y: f64) -> Result<()> {
        let cmd = format!("input tap {x} {y}");
        self.transport.shell_command(&self.serial, &cmd).await?;
        Ok(())
    }
}

/// Vega (Fire TV) input. The injection mechanism is unverified against a live VVD;
/// until Phase 0 discovery confirms it, key/text error explicitly rather than
/// silently doing nothing. `tap` is permanently unsupported on a 10-foot TV surface.
pub struct VegaController {
    #[allow(dead_code)]
    transport: Arc<AdbTransport>,
    #[allow(dead_code)]
    serial: String,
}

#[async_trait]
impl DeviceController for VegaController {
    async fn key(&self, _key: DeviceKey) -> Result<()> {
        Err(vega_unverified("key"))
    }

    async fn text(&self, _text: &str) -> Result<()> {
        Err(vega_unverified("text"))
    }

    async fn tap(&self, _x: f64, _y: f64) -> Result<()> {
        Err(AdbError::Adb(
            "tap is not supported on Vega (TV platform has no touch surface)".into(),
        ))
    }
}

fn vega_unverified(op: &str) -> AdbError {
    AdbError::Adb(format!(
        "vega device {op} not yet implemented: injection mechanism unverified against a live VVD"
    ))
}

/// Map a symbolic key to the Android keyevent code.
/// Codes per the Android `KeyEvent` constants.
fn android_keycode(key: DeviceKey) -> u32 {
    match key {
        DeviceKey::Up => 19,
        DeviceKey::Down => 20,
        DeviceKey::Left => 21,
        DeviceKey::Right => 22,
        DeviceKey::Select => 23,
        DeviceKey::Back => 4,
        DeviceKey::Home => 3,
        DeviceKey::Menu => 82,
        DeviceKey::PlayPause => 85,
        DeviceKey::VolumeUp => 24,
        DeviceKey::VolumeDown => 25,
    }
}

/// Prepare text for `input text`. Single-quote for the device shell so metacharacters
/// stay literal, and encode spaces as `%s` because `input text` word-splits otherwise.
fn escape_input_text(text: &str) -> String {
    let inner = text.replace('\'', r"'\''").replace(' ', "%s");
    format!("'{inner}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_keycodes_match_android_constants() {
        assert_eq!(android_keycode(DeviceKey::Up), 19);
        assert_eq!(android_keycode(DeviceKey::Down), 20);
        assert_eq!(android_keycode(DeviceKey::Select), 23);
        assert_eq!(android_keycode(DeviceKey::Back), 4);
        assert_eq!(android_keycode(DeviceKey::PlayPause), 85);
    }

    #[test]
    fn input_text_encodes_spaces_and_quotes() {
        assert_eq!(escape_input_text("hello"), "'hello'");
        assert_eq!(escape_input_text("hello world"), "'hello%sworld'");
        assert_eq!(escape_input_text("it's"), r"'it'\''s'");
    }

    #[tokio::test]
    async fn vega_key_and_text_error_until_verified() {
        let transport = Arc::new(AdbTransport::new("127.0.0.1:5037".parse().unwrap()));
        let vega = VegaController {
            transport,
            serial: "vega-test".into(),
        };
        assert!(matches!(
            vega.key(DeviceKey::Up).await,
            Err(AdbError::Adb(_))
        ));
        assert!(matches!(vega.text("hi").await, Err(AdbError::Adb(_))));
    }

    #[tokio::test]
    async fn vega_tap_unsupported() {
        let transport = Arc::new(AdbTransport::new("127.0.0.1:5037".parse().unwrap()));
        let vega = VegaController {
            transport,
            serial: "vega-test".into(),
        };
        let err = vega.tap(1.0, 2.0).await.unwrap_err();
        assert!(matches!(err, AdbError::Adb(msg) if msg.contains("not supported")));
    }
}
