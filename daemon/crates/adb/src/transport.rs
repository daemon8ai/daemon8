// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::net::SocketAddrV4;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use adb_client::{ADBDeviceExt, server::ADBServer};
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

impl AdbTransport {
    pub fn new(addr: SocketAddrV4) -> Self {
        Self { addr }
    }

    pub async fn list_devices(&self) -> Result<Vec<DeviceInfo>> {
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

    pub async fn shell_command(&self, serial: &str, cmd: &str) -> Result<String> {
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

    /// Spawn a dedicated OS thread that streams shell output line-by-line.
    /// Returns a JoinHandle and a channel receiver for raw lines.
    /// Set the stop flag to signal the thread to exit.
    pub fn spawn_log_stream(
        &self,
        serial: String,
        cmd: String,
    ) -> (
        std::thread::JoinHandle<Result<()>>,
        tokio::sync::mpsc::UnboundedReceiver<String>,
        Arc<AtomicBool>,
    ) {
        let addr = self.addr;
        // Unbounded: logcat streams at device rate; backpressure would block the
        // ADB read loop and cause the device to drop lines upstream. The consumer
        // batches into the observation store, which has its own retention ceiling.
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();

        let handle = std::thread::Builder::new()
            .name(format!("adb-log-{serial}"))
            .spawn(move || {
                let mut server = ADBServer::new(addr);
                let mut device =
                    server
                        .get_device_by_name(&serial)
                        .map_err(|e| AdbError::Device {
                            serial: serial.clone(),
                            reason: e.to_string(),
                        })?;

                // Capture output to a buffer that we drain periodically.
                // adb_client's shell_command blocks until EOF, so for streaming
                // commands like `logcat -f` we need a different approach:
                // pipe through a writer that sends lines as they arrive.
                let mut line_writer = LineSender {
                    tx: tx.clone(),
                    buf: String::new(),
                    stop: stop_flag,
                };

                let result = device.shell_command(&cmd, Some(&mut line_writer), None);

                match result {
                    Ok(_) => {
                        // Flush any remaining partial line
                        if !line_writer.buf.is_empty() {
                            let _ = line_writer.tx.send(std::mem::take(&mut line_writer.buf));
                        }
                        Ok(())
                    }
                    Err(e) => {
                        tracing::error!(serial, cmd, error = %e, "log stream ended with error");
                        Err(AdbError::Adb(format!("log stream: {e}")))
                    }
                }
            })
            .expect("failed to spawn log stream thread");

        (handle, rx, stop)
    }
}

struct LineSender {
    tx: tokio::sync::mpsc::UnboundedSender<String>,
    buf: String,
    stop: Arc<AtomicBool>,
}

impl std::io::Write for LineSender {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.stop.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "stop requested",
            ));
        }

        let text = String::from_utf8_lossy(data);
        self.buf.push_str(&text);

        while let Some(newline_pos) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=newline_pos).collect();
            let line = line
                .trim_end_matches('\n')
                .trim_end_matches('\r')
                .to_string();
            if !line.is_empty() && self.tx.send(line).is_err() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "receiver dropped",
                ));
            }
        }

        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn line_sender_splits_lines() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let mut sender = LineSender {
            tx,
            buf: String::new(),
            stop,
        };

        use std::io::Write;
        sender.write_all(b"line one\nline two\npartial").unwrap();

        assert_eq!(rx.try_recv().unwrap(), "line one");
        assert_eq!(rx.try_recv().unwrap(), "line two");
        assert!(rx.try_recv().is_err()); // partial not sent yet

        sender.write_all(b" end\n").unwrap();
        assert_eq!(rx.try_recv().unwrap(), "partial end");
    }

    #[test]
    fn line_sender_respects_stop_flag() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(true));
        let mut sender = LineSender {
            tx,
            buf: String::new(),
            stop,
        };

        use std::io::Write;
        let result = sender.write(b"should fail");
        assert!(result.is_err());
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
