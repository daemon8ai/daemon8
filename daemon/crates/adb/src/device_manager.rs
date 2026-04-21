// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::HashMap;
use std::net::SocketAddrV4;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use daemon8_types::{DevicePlatform, Observation, ObservationKind, Origin, Severity};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::Result;
use crate::parser::logcat::LogcatParser;
use crate::parser::loggingctl::LoggingctlParser;
use crate::parser::{LogParser, LogSeverity, ParsedLine};
use crate::transport::{AdbTransport, DeviceInfo};

struct DeviceState {
    platform: DevicePlatform,
    log_handle: Option<std::thread::JoinHandle<Result<()>>>,
    log_stop: Option<Arc<std::sync::atomic::AtomicBool>>,
}

pub struct DeviceManager {
    transport: AdbTransport,
    devices: HashMap<String, DeviceState>,
    obs_tx: UnboundedSender<Observation>,
    cancel: CancellationToken,
    scan_interval: std::time::Duration,
}

impl DeviceManager {
    pub fn new(
        addr: SocketAddrV4,
        obs_tx: UnboundedSender<Observation>,
        cancel: CancellationToken,
        scan_interval_secs: u64,
    ) -> Self {
        Self {
            transport: AdbTransport::new(addr),
            devices: HashMap::new(),
            obs_tx,
            cancel,
            scan_interval: std::time::Duration::from_secs(scan_interval_secs),
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        tracing::info!("device manager started");

        loop {
            tokio::select! {
                () = self.cancel.cancelled() => {
                    tracing::info!("device manager shutting down");
                    self.stop_all_streams();
                    return Ok(());
                }
                () = tokio::time::sleep(self.scan_interval) => {
                    if let Err(e) = self.scan().await {
                        tracing::warn!(error = %e, "device scan failed");
                    }
                }
            }
        }
    }

    async fn scan(&mut self) -> Result<()> {
        let devices = self.transport.list_devices().await?;
        let current_serials: Vec<String> = devices.iter().map(|d| d.serial.clone()).collect();

        // Detect disconnected devices
        let disconnected: Vec<String> = self
            .devices
            .keys()
            .filter(|s| !current_serials.contains(s))
            .cloned()
            .collect();

        for serial in disconnected {
            self.handle_disconnect(&serial);
        }

        // Detect new devices
        for device in devices {
            if !self.devices.contains_key(&device.serial) {
                self.handle_connect(device).await;
            }
        }

        Ok(())
    }

    async fn handle_connect(&mut self, info: DeviceInfo) {
        let serial = info.serial.clone();
        tracing::info!(serial = %serial, model = %info.model, "device connected");

        // Detect platform
        let platform = match self
            .transport
            .shell_command(&serial, "which loggingctl")
            .await
        {
            Ok(output) if !output.trim().is_empty() && !output.contains("not found") => {
                tracing::info!(serial = %serial, "detected Vega OS");
                DevicePlatform::Vega
            }
            _ => {
                tracing::info!(serial = %serial, "detected Android");
                DevicePlatform::Android
            }
        };

        // Emit lifecycle observation
        let _ = self.obs_tx.send(Observation::new(
            Origin::Device {
                serial: serial.as_str().into(),
                platform: platform.clone(),
            },
            ObservationKind::Lifecycle {
                event_name: "device_connected".into(),
                frame_id: serial.clone(),
            },
            serde_json::json!({
                "model": info.model,
                "state": info.state,
                "platform": platform,
            }),
            Severity::Info,
            None,
        ));

        // Start log stream
        let (cmd, parser): (String, Box<dyn LogParser>) = match platform {
            DevicePlatform::Vega => (
                "loggingctl log -f -o short_precise".into(),
                Box::new(LoggingctlParser),
            ),
            DevicePlatform::Android => ("logcat -v threadtime".into(), Box::new(LogcatParser)),
        };

        let (handle, mut rx, stop) = self.transport.spawn_log_stream(serial.clone(), cmd);

        // Bridge: read raw lines from the stream thread, parse, and send as Observations
        let obs_tx = self.obs_tx.clone();
        let origin = Origin::Device {
            serial: serial.as_str().into(),
            platform: platform.clone(),
        };
        let cancel = self.cancel.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    line = rx.recv() => {
                        match line {
                            Some(line) => {
                                if let Some(parsed) = parser.parse_line(&line) {
                                    let obs = parsed_to_observation(parsed, &origin);
                                    let _ = obs_tx.send(obs);
                                }
                            }
                            None => break, // stream ended
                        }
                    }
                    () = cancel.cancelled() => break,
                }
            }
        });

        self.devices.insert(
            serial,
            DeviceState {
                platform,
                log_handle: Some(handle),
                log_stop: Some(stop),
            },
        );
    }

    fn handle_disconnect(&mut self, serial: &str) {
        if let Some(mut state) = self.devices.remove(serial) {
            tracing::info!(serial = %serial, "device disconnected");

            if let Some(stop) = state.log_stop.take() {
                stop.store(true, Ordering::Relaxed);
            }
            if let Some(handle) = state.log_handle.take() {
                let _ = handle.join();
            }

            let _ = self.obs_tx.send(Observation::new(
                Origin::Device {
                    serial: serial.into(),
                    platform: state.platform,
                },
                ObservationKind::Lifecycle {
                    event_name: "device_disconnected".into(),
                    frame_id: serial.to_string(),
                },
                serde_json::json!({}),
                Severity::Info,
                None,
            ));
        }
    }

    fn stop_all_streams(&mut self) {
        let serials: Vec<String> = self.devices.keys().cloned().collect();
        for serial in serials {
            self.handle_disconnect(&serial);
        }
    }
}

fn parsed_to_observation(parsed: ParsedLine, origin: &Origin) -> Observation {
    let severity = match parsed.severity {
        LogSeverity::Trace => Severity::Trace,
        LogSeverity::Debug => Severity::Debug,
        LogSeverity::Info => Severity::Info,
        LogSeverity::Warn => Severity::Warn,
        LogSeverity::Error => Severity::Error,
    };

    let mut data = serde_json::json!({
        "tag": parsed.tag,
        "message": parsed.message,
        "timestamp": parsed.timestamp,
    });

    if let Some(pid) = parsed.pid {
        data["pid"] = serde_json::json!(pid);
    }
    if let Some(ref hostname) = parsed.hostname {
        data["hostname"] = serde_json::json!(hostname);
    }
    if let Some(ref facility) = parsed.facility {
        data["facility"] = serde_json::json!(facility);
    }

    Observation::new(origin.clone(), ObservationKind::Log, data, severity, None)
}
