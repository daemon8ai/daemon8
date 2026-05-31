// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::io::{BufRead, BufReader};
use std::net::SocketAddrV4;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use adb_client::{ADBDeviceExt, server::ADBServer};
use async_trait::async_trait;
use daemon8_types::DevicePlatform;

use crate::error::{AdbError, Result};

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub serial: String,
    pub model: String,
    pub state: String,
}

pub struct AdbTransport {
    addr: SocketAddrV4,
}

pub struct DeviceLogStream {
    pub handle: std::thread::JoinHandle<Result<()>>,
    pub rx: tokio::sync::mpsc::UnboundedReceiver<String>,
    pub stop: Arc<AtomicBool>,
    pub done: Arc<AtomicBool>,
}

#[async_trait]
pub trait DeviceTransport: Send + Sync + 'static {
    async fn list_devices(&self) -> Result<Vec<DeviceInfo>>;
    async fn shell_command(&self, serial: &str, cmd: &str) -> Result<String>;
    fn spawn_log_stream(&self, serial: String, cmd: String) -> DeviceLogStream;
}

impl AdbTransport {
    pub fn new(addr: SocketAddrV4) -> Self {
        Self { addr }
    }
}

#[async_trait]
impl DeviceTransport for AdbTransport {
    async fn list_devices(&self) -> Result<Vec<DeviceInfo>> {
        let addr = self.addr;
        tokio::task::spawn_blocking(move || {
            let mut server = ADBServer::new(addr);
            let devices = server
                .devices_long()
                .map_err(|e| AdbError::Adb(format!("list devices: {e}")))?;

            Ok(devices
                .into_iter()
                .map(|d| DeviceInfo {
                    serial: d.identifier,
                    model: d.model,
                    state: format!("{}", d.state),
                })
                .collect())
        })
        .await?
    }

    async fn shell_command(&self, serial: &str, cmd: &str) -> Result<String> {
        let addr = self.addr;
        let serial = serial.to_string();
        let cmd = cmd.to_string();
        tokio::task::spawn_blocking(move || {
            let mut server = ADBServer::new(addr);
            let mut device = server
                .get_device_by_name(&serial)
                .map_err(|e| AdbError::Device {
                    serial: serial.clone(),
                    reason: e.to_string(),
                })?;

            let mut buf = Vec::new();
            device
                .shell_command(&cmd, Some(&mut buf), None)
                .map_err(|e| AdbError::Adb(format!("shell '{cmd}': {e}")))?;

            Ok(String::from_utf8_lossy(&buf).into_owned())
        })
        .await?
    }

    /// Spawn a dedicated OS thread that streams shell output line-by-line.
    /// Set the stop flag to signal the thread to exit.
    fn spawn_log_stream(&self, serial: String, cmd: String) -> DeviceLogStream {
        let addr = self.addr;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let done = Arc::new(AtomicBool::new(false));
        let done_flag = done.clone();

        let handle = std::thread::Builder::new()
            .name(format!("adb-log-{serial}"))
            .spawn(move || {
                let result = stream_adb_shell(addr, &serial, &cmd, tx, stop_flag);
                done_flag.store(true, Ordering::Relaxed);
                result
            })
            .expect("failed to spawn log stream thread");

        DeviceLogStream {
            handle,
            rx,
            stop,
            done,
        }
    }
}

fn stream_adb_shell(
    addr: SocketAddrV4,
    serial: &str,
    cmd: &str,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let mut child = Command::new("adb")
        .args(adb_shell_args(addr, serial, cmd))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AdbError::Adb(format!("spawn adb shell stream: {e}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AdbError::Adb("adb shell stream had no stdout".into()))?;
    let child = Arc::new(Mutex::new(child));
    let process_done = Arc::new(AtomicBool::new(false));
    let monitor_child = child.clone();
    let monitor_stop = stop.clone();
    let monitor_done = process_done.clone();

    let monitor = std::thread::spawn(move || {
        while !monitor_done.load(Ordering::Relaxed) {
            if monitor_stop.load(Ordering::Relaxed) {
                if let Ok(mut child) = monitor_child.lock() {
                    let _ = child.kill();
                }
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    });

    let mut reader = BufReader::new(stdout);
    let mut line = String::new();

    loop {
        line.clear();
        let bytes = match reader.read_line(&mut line) {
            Ok(bytes) => bytes,
            Err(e) => {
                stop.store(true, Ordering::Relaxed);
                if let Ok(mut child) = child.lock() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                process_done.store(true, Ordering::Relaxed);
                let _ = monitor.join();
                return Err(AdbError::Adb(format!("read adb shell stream: {e}")));
            }
        };

        if bytes == 0 {
            break;
        }

        let trimmed = line.trim_end_matches(['\r', '\n']);
        if !trimmed.is_empty() && tx.send(trimmed.to_string()).is_err() {
            stop.store(true, Ordering::Relaxed);
            break;
        }
    }

    let status = child
        .lock()
        .expect("adb child mutex poisoned")
        .wait()
        .map_err(|e| AdbError::Adb(format!("wait adb shell stream: {e}")))?;
    process_done.store(true, Ordering::Relaxed);
    let _ = monitor.join();

    if stop.load(Ordering::Relaxed) || status.success() {
        Ok(())
    } else {
        Err(AdbError::Adb(format!(
            "adb shell stream exited with status {status}"
        )))
    }
}

fn adb_shell_args(addr: SocketAddrV4, serial: &str, cmd: &str) -> Vec<String> {
    vec![
        "-H".into(),
        addr.ip().to_string(),
        "-P".into(),
        addr.port().to_string(),
        "-s".into(),
        serial.into(),
        "shell".into(),
        cmd.into(),
    ]
}

impl AdbTransport {
    pub async fn shell_command_raw(&self, serial: &str, cmd: &str) -> Result<Vec<u8>> {
        let addr = self.addr;
        let serial = serial.to_string();
        let cmd = cmd.to_string();
        tokio::task::spawn_blocking(move || {
            let mut server = ADBServer::new(addr);
            let mut device = server
                .get_device_by_name(&serial)
                .map_err(|e| AdbError::Device {
                    serial: serial.clone(),
                    reason: e.to_string(),
                })?;

            let mut buf = Vec::new();
            device
                .shell_command(&cmd, Some(&mut buf), None)
                .map_err(|e| AdbError::Adb(format!("shell '{cmd}': {e}")))?;

            Ok(buf)
        })
        .await?
    }

    pub async fn pull_file(&self, serial: &str, remote_path: &str) -> Result<Vec<u8>> {
        let addr = self.addr;
        let serial = serial.to_string();
        let remote_path = remote_path.to_string();
        tokio::task::spawn_blocking(move || {
            let mut server = ADBServer::new(addr);
            let mut device = server
                .get_device_by_name(&serial)
                .map_err(|e| AdbError::Device {
                    serial: serial.clone(),
                    reason: e.to_string(),
                })?;

            let mut buf = Vec::new();
            device
                .pull(&remote_path, &mut buf)
                .map_err(|e| AdbError::Adb(format!("pull {remote_path}: {e}")))?;

            Ok(buf)
        })
        .await?
    }

    /// Capture a screenshot from the device. Uses the ADB framebuffer protocol
    /// for Android, and on-device tools for Vega.
    pub async fn capture_screenshot(
        &self,
        serial: &str,
        platform: &DevicePlatform,
    ) -> Result<Vec<u8>> {
        match platform {
            DevicePlatform::Android => self.capture_screenshot_framebuffer(serial).await,
            DevicePlatform::Vega => self.capture_screenshot_vega(serial).await,
        }
    }

    async fn capture_screenshot_framebuffer(&self, serial: &str) -> Result<Vec<u8>> {
        let addr = self.addr;
        let serial = serial.to_string();
        tokio::task::spawn_blocking(move || {
            let mut server = ADBServer::new(addr);
            let mut device = server
                .get_device_by_name(&serial)
                .map_err(|e| AdbError::Device {
                    serial: serial.clone(),
                    reason: e.to_string(),
                })?;

            device
                .framebuffer_bytes()
                .map_err(|e| AdbError::Adb(format!("framebuffer: {e}")))
        })
        .await?
    }

    async fn capture_screenshot_vega(&self, serial: &str) -> Result<Vec<u8>> {
        const REMOTE_PATH: &str = "/tmp/daemon8-shot.png";

        self.shell_command(serial, &format!("screenshooter {REMOTE_PATH}"))
            .await?;

        let bytes = self.pull_file(serial, REMOTE_PATH).await?;

        let _ = self
            .shell_command(serial, &format!("rm {REMOTE_PATH}"))
            .await;

        if bytes.is_empty() {
            return Err(AdbError::ScreenshotEmpty);
        }

        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn adb_shell_args_include_server_device_and_command() {
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 5037);
        let args = adb_shell_args(addr, "emulator-5554", "logcat -v threadtime");

        assert_eq!(
            args,
            vec![
                "-H",
                "127.0.0.1",
                "-P",
                "5037",
                "-s",
                "emulator-5554",
                "shell",
                "logcat -v threadtime"
            ]
        );
    }

    #[tokio::test]
    async fn list_devices_connects_to_server() {
        // This test requires a running ADB server. Skip gracefully if unavailable.
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 5037);
        let transport = AdbTransport::new(addr);

        match transport.list_devices().await {
            Ok(devices) => {
                // Server is running -- devices may be empty, that's fine
                tracing::info!("found {} devices", devices.len());
            }
            Err(_) => {
                // No ADB server -- skip
                eprintln!("ADB server not available, skipping");
            }
        }
    }
}
