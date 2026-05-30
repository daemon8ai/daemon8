// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Device input control. Android and Vega diverge in how key/text/tap reach the
//! device, so each platform owns a `DeviceController` impl rather than scattering
//! `match platform` arms across the transport. The factory selects by `DevicePlatform`.

use std::sync::Arc;

use async_trait::async_trait;
use daemon8_types::{DeviceKey, DevicePlatform};

use crate::error::Result;
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

/// Vega (Fire TV) input. Vega is a Wayland-based Linux OS with no Android `input`
/// binary; injection goes through `inputd-cli` (button_press / send_text / touch),
/// which speaks evdev `KEY_*` names. Touch is supported (the VVD reports 1920x1080).
pub struct VegaController {
    transport: Arc<AdbTransport>,
    serial: String,
}

#[async_trait]
impl DeviceController for VegaController {
    async fn key(&self, key: DeviceKey) -> Result<()> {
        let cmd = format!("inputd-cli button_press {}", vega_button(key));
        self.transport.shell_command(&self.serial, &cmd).await?;
        Ok(())
    }

    async fn text(&self, text: &str) -> Result<()> {
        let cmd = format!("inputd-cli send_text {}", shell_quote(text));
        self.transport.shell_command(&self.serial, &cmd).await?;
        Ok(())
    }

    async fn tap(&self, x: f64, y: f64) -> Result<()> {
        let cmd = format!("inputd-cli touch {} {}", x as i64, y as i64);
        self.transport.shell_command(&self.serial, &cmd).await?;
        Ok(())
    }
}

/// Map a symbolic key to the Vega evdev key name accepted by `inputd-cli button_press`.
fn vega_button(key: DeviceKey) -> &'static str {
    match key {
        DeviceKey::Up => "KEY_UP",
        DeviceKey::Down => "KEY_DOWN",
        DeviceKey::Left => "KEY_LEFT",
        DeviceKey::Right => "KEY_RIGHT",
        DeviceKey::Select => "KEY_SELECT",
        DeviceKey::Back => "KEY_BACK",
        DeviceKey::Home => "KEY_HOMEPAGE",
        DeviceKey::Menu => "KEY_MENU",
        DeviceKey::PlayPause => "KEY_PLAYPAUSE",
        DeviceKey::VolumeUp => "KEY_VOLUMEUP",
        DeviceKey::VolumeDown => "KEY_VOLUMEDOWN",
    }
}

/// Single-quote a string for the device shell so spaces and metacharacters stay
/// literal as one argument.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
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
    fn vega_buttons_use_evdev_key_names() {
        assert_eq!(vega_button(DeviceKey::Up), "KEY_UP");
        assert_eq!(vega_button(DeviceKey::Select), "KEY_SELECT");
        assert_eq!(vega_button(DeviceKey::Back), "KEY_BACK");
        assert_eq!(vega_button(DeviceKey::Home), "KEY_HOMEPAGE");
        assert_eq!(vega_button(DeviceKey::PlayPause), "KEY_PLAYPAUSE");
    }

    #[test]
    fn input_text_encodes_spaces_and_quotes() {
        assert_eq!(escape_input_text("hello"), "'hello'");
        assert_eq!(escape_input_text("hello world"), "'hello%sworld'");
        assert_eq!(escape_input_text("it's"), r"'it'\''s'");
    }

    #[test]
    fn shell_quote_wraps_and_escapes() {
        assert_eq!(shell_quote("hello world"), "'hello world'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
