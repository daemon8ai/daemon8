// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::{HashMap, HashSet};
use std::net::SocketAddrV4;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use daemon8_types::{DevicePlatform, Observation, ObservationKind, Origin, Severity};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

use crate::Result;
use crate::parser::logcat::LogcatParser;
use crate::parser::loggingctl::LoggingctlParser;
use crate::parser::{LogParser, LogSeverity, ParsedLine};
use crate::transport::{AdbTransport, DeviceInfo, DeviceTransport};

const DEVICE_BATCH_WINDOW: Duration = Duration::from_secs(2);
const STREAM_STALL_AFTER: Duration = Duration::from_secs(15);
const STREAM_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const PLATFORM_DETECT_TIMEOUT: Duration = Duration::from_secs(3);
const STREAM_STOP_WAIT: Duration = Duration::from_millis(500);

static PROBE_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceMonitorFeatures {
    pub android: bool,
    pub vega: bool,
}

impl DeviceMonitorFeatures {
    pub fn any_enabled(self) -> bool {
        self.android || self.vega
    }

    fn allows(self, platform: &DevicePlatform) -> bool {
        match platform {
            DevicePlatform::Android => self.android,
            DevicePlatform::Vega => self.vega,
        }
    }
}

impl Default for DeviceMonitorFeatures {
    fn default() -> Self {
        Self {
            android: true,
            vega: true,
        }
    }
}

struct DeviceState {
    platform: DevicePlatform,
    log_handle: Option<std::thread::JoinHandle<Result<()>>>,
    log_stop: Option<Arc<std::sync::atomic::AtomicBool>>,
    log_done: Option<Arc<std::sync::atomic::AtomicBool>>,
    stream_generation: u64,
    restart_count: u64,
    last_line_at: Instant,
    pending_probe: Option<PendingProbe>,
    batcher: DeviceLogBatcher,
}

struct PendingProbe {
    token: String,
    sent_at: Instant,
}

enum DeviceEvent {
    LogLine {
        serial: String,
        generation: u64,
        line: String,
    },
    StreamEnded {
        serial: String,
        generation: u64,
    },
}

pub struct DeviceManager<T = AdbTransport>
where
    T: DeviceTransport,
{
    transport: T,
    devices: HashMap<String, DeviceState>,
    obs_tx: UnboundedSender<Observation>,
    event_tx: UnboundedSender<DeviceEvent>,
    event_rx: UnboundedReceiver<DeviceEvent>,
    cancel: CancellationToken,
    scan_interval: std::time::Duration,
    features: DeviceMonitorFeatures,
}

impl DeviceManager<AdbTransport> {
    pub fn new(
        addr: SocketAddrV4,
        obs_tx: UnboundedSender<Observation>,
        cancel: CancellationToken,
        scan_interval_secs: u64,
    ) -> Self {
        Self::with_transport(AdbTransport::new(addr), obs_tx, cancel, scan_interval_secs)
    }

    pub fn new_with_features(
        addr: SocketAddrV4,
        obs_tx: UnboundedSender<Observation>,
        cancel: CancellationToken,
        scan_interval_secs: u64,
        features: DeviceMonitorFeatures,
    ) -> Self {
        Self::with_transport_and_features(
            AdbTransport::new(addr),
            obs_tx,
            cancel,
            scan_interval_secs,
            features,
        )
    }
}

impl<T> DeviceManager<T>
where
    T: DeviceTransport,
{
    pub fn with_transport(
        transport: T,
        obs_tx: UnboundedSender<Observation>,
        cancel: CancellationToken,
        scan_interval_secs: u64,
    ) -> Self {
        Self::with_transport_and_features(
            transport,
            obs_tx,
            cancel,
            scan_interval_secs,
            DeviceMonitorFeatures::default(),
        )
    }

    pub fn with_transport_and_features(
        transport: T,
        obs_tx: UnboundedSender<Observation>,
        cancel: CancellationToken,
        scan_interval_secs: u64,
        features: DeviceMonitorFeatures,
    ) -> Self {
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            transport,
            devices: HashMap::new(),
            obs_tx,
            event_tx,
            event_rx,
            cancel,
            scan_interval: std::time::Duration::from_secs(scan_interval_secs),
            features,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        tracing::info!("device manager started");

        if let Err(e) = self.scan().await {
            tracing::warn!(error = %e, "initial device scan failed");
        }

        let mut batch_interval = tokio::time::interval(DEVICE_BATCH_WINDOW);
        batch_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut scan_interval = tokio::time::interval(self.scan_interval);
        scan_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        batch_interval.tick().await;
        scan_interval.tick().await;

        loop {
            tokio::select! {
                () = self.cancel.cancelled() => {
                    tracing::info!("device manager shutting down");
                    self.stop_all_streams();
                    return Ok(());
                }
                event = self.event_rx.recv() => {
                    if let Some(event) = event {
                        self.handle_device_event(event).await;
                    }
                }
                _ = batch_interval.tick() => {
                    self.flush_batches();
                    self.check_stream_health().await;
                }
                _ = scan_interval.tick() => {
                    if let Err(e) = self.scan().await {
                        tracing::warn!(error = %e, "device scan failed");
                    }
                }
            }
        }
    }

    async fn scan(&mut self) -> Result<()> {
        let devices = self.transport.list_devices().await?;
        let current_serials: HashSet<&str> = devices.iter().map(|d| d.serial.as_str()).collect();

        // Detect disconnected devices
        let disconnected: Vec<String> = self
            .devices
            .keys()
            .filter(|s| !current_serials.contains(s.as_str()))
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
        let platform = match tokio::time::timeout(
            PLATFORM_DETECT_TIMEOUT,
            self.transport.shell_command(&serial, "which loggingctl"),
        )
        .await
        {
            Ok(Ok(output)) if !output.trim().is_empty() && !output.contains("not found") => {
                tracing::info!(serial = %serial, "detected Vega OS");
                DevicePlatform::Vega
            }
            Ok(Err(err)) => {
                tracing::warn!(
                    serial = %serial,
                    error = %err,
                    "device platform detection failed; defaulting to Android"
                );
                DevicePlatform::Android
            }
            Err(_) => {
                tracing::warn!(
                    serial = %serial,
                    timeout_ms = PLATFORM_DETECT_TIMEOUT.as_millis(),
                    "device platform detection timed out; defaulting to Android"
                );
                DevicePlatform::Android
            }
            _ => {
                tracing::info!(serial = %serial, "detected Android");
                DevicePlatform::Android
            }
        };

        if !self.features.allows(&platform) {
            tracing::info!(
                serial = %serial,
                platform = ?platform,
                "device platform feature is disabled; skipping log stream"
            );
            return;
        }

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

        let (cmd, parser): (String, Box<dyn LogParser>) = match platform {
            DevicePlatform::Vega => (
                "loggingctl log -f -o short_precise".into(),
                Box::new(LoggingctlParser),
            ),
            DevicePlatform::Android => ("logcat -v threadtime".into(), Box::new(LogcatParser)),
        };

        let origin = Origin::Device {
            serial: serial.as_str().into(),
            platform: platform.clone(),
        };
        let batcher = DeviceLogBatcher::new(origin, parser);

        self.devices.insert(
            serial.clone(),
            DeviceState {
                platform,
                log_handle: None,
                log_stop: None,
                log_done: None,
                stream_generation: 0,
                restart_count: 0,
                last_line_at: Instant::now(),
                pending_probe: None,
                batcher,
            },
        );
        self.start_log_stream(&serial, cmd);
    }

    fn handle_disconnect(&mut self, serial: &str) {
        if let Some(mut state) = self.devices.remove(serial) {
            tracing::info!(serial = %serial, "device disconnected");

            stop_log_stream(
                state.log_handle.take(),
                state.log_stop.take(),
                state.log_done.take(),
                serial,
            );
            state.batcher.flush(&self.obs_tx);

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

    fn flush_batches(&mut self) {
        for state in self.devices.values_mut() {
            state.batcher.flush(&self.obs_tx);
        }
    }

    async fn check_stream_health(&mut self) {
        enum HealthAction {
            Probe {
                serial: String,
                token: String,
                cmd: String,
            },
            Restart {
                serial: String,
            },
        }

        let now = Instant::now();
        let mut actions = Vec::new();

        for (serial, state) in &mut self.devices {
            if let Some(probe) = &state.pending_probe {
                if now.duration_since(probe.sent_at) >= STREAM_PROBE_TIMEOUT {
                    actions.push(HealthAction::Restart {
                        serial: serial.clone(),
                    });
                }
                continue;
            }

            if now.duration_since(state.last_line_at) >= STREAM_STALL_AFTER {
                let token = next_probe_token();
                let cmd = probe_command_for_platform(&state.platform, &token);
                state.pending_probe = Some(PendingProbe {
                    token: token.clone(),
                    sent_at: now,
                });
                actions.push(HealthAction::Probe {
                    serial: serial.clone(),
                    token,
                    cmd,
                });
            }
        }

        for action in actions {
            match action {
                HealthAction::Probe { serial, token, cmd } => {
                    match tokio::time::timeout(
                        STREAM_PROBE_TIMEOUT,
                        self.transport.shell_command(&serial, &cmd),
                    )
                    .await
                    {
                        Ok(Ok(_)) => {}
                        Ok(Err(err)) => {
                            tracing::warn!(
                                serial = %serial,
                                token = %token,
                                error = %err,
                                "device log stream health probe failed; restarting"
                            );
                            self.restart_log_stream(&serial, "health probe failed");
                        }
                        Err(_) => {
                            tracing::warn!(
                                serial = %serial,
                                token = %token,
                                timeout_ms = STREAM_PROBE_TIMEOUT.as_millis(),
                                "device log stream health probe command timed out; restarting"
                            );
                            self.restart_log_stream(&serial, "health probe command timed out");
                        }
                    }
                }
                HealthAction::Restart { serial } => {
                    self.restart_log_stream(&serial, "health probe timed out");
                }
            }
        }
    }

    fn restart_log_stream(&mut self, serial: &str, reason: &str) {
        let Some(state) = self.devices.get_mut(serial) else {
            return;
        };

        state.pending_probe = None;
        state.restart_count = state.restart_count.saturating_add(1);
        tracing::warn!(
            serial = %serial,
            restart_count = state.restart_count,
            reason,
            "restarting device log stream"
        );
        let cmd = log_command_for_platform(&state.platform);
        self.start_log_stream(serial, cmd);
    }

    async fn handle_device_event(&mut self, event: DeviceEvent) {
        match event {
            DeviceEvent::LogLine {
                serial,
                generation,
                line,
            } => {
                if let Some(state) = self.devices.get_mut(&serial)
                    && state.stream_generation == generation
                {
                    state.last_line_at = Instant::now();
                    if let Some(probe) = &state.pending_probe
                        && line.contains(&probe.token)
                    {
                        state.pending_probe = None;
                        return;
                    }
                    if state.pending_probe.is_some() {
                        state.pending_probe = None;
                    }
                    state.batcher.push(line, &self.obs_tx);
                }
            }
            DeviceEvent::StreamEnded { serial, generation } => {
                if let Some(state) = self.devices.get_mut(&serial)
                    && state.stream_generation == generation
                {
                    state.batcher.flush(&self.obs_tx);
                    state.restart_count += 1;
                    state.pending_probe = None;
                    tracing::warn!(
                        serial = %serial,
                        generation,
                        restart_count = state.restart_count,
                        "device log stream ended; restarting"
                    );
                    let cmd = log_command_for_platform(&state.platform);
                    self.start_log_stream(&serial, cmd);
                }
            }
        }
    }

    fn start_log_stream(&mut self, serial: &str, cmd: String) {
        let Some(state) = self.devices.get_mut(serial) else {
            return;
        };

        stop_log_stream(
            state.log_handle.take(),
            state.log_stop.take(),
            state.log_done.take(),
            serial,
        );

        state.stream_generation = state.stream_generation.saturating_add(1);
        let generation = state.stream_generation;
        let stream = self.transport.spawn_log_stream(serial.to_string(), cmd);

        let mut rx = stream.rx;
        let event_tx = self.event_tx.clone();
        let cancel = self.cancel.clone();
        let serial = serial.to_string();
        let event_serial = serial.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    line = rx.recv() => {
                        match line {
                            Some(line) => {
                                let _ = event_tx.send(DeviceEvent::LogLine {
                                    serial: event_serial.clone(),
                                    generation,
                                    line,
                                });
                            }
                            None => {
                                let _ = event_tx.send(DeviceEvent::StreamEnded {
                                    serial: event_serial.clone(),
                                    generation,
                                });
                                break;
                            }
                        }
                    }
                    () = cancel.cancelled() => break,
                }
            }
        });

        if let Some(state) = self.devices.get_mut(&serial) {
            state.log_handle = Some(stream.handle);
            state.log_stop = Some(stream.stop);
            state.log_done = Some(stream.done);
        }
    }
}

fn stop_log_stream(
    handle: Option<std::thread::JoinHandle<Result<()>>>,
    stop: Option<Arc<std::sync::atomic::AtomicBool>>,
    done: Option<Arc<std::sync::atomic::AtomicBool>>,
    serial: &str,
) {
    if let Some(stop) = stop {
        stop.store(true, Ordering::Relaxed);
    }

    let Some(handle) = handle else {
        return;
    };

    if let Some(done) = done.as_ref() {
        let started = Instant::now();
        while !done.load(Ordering::Relaxed) && started.elapsed() < STREAM_STOP_WAIT {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    if handle.is_finished() {
        let _ = handle.join();
    } else {
        tracing::warn!(
            serial = %serial,
            wait_ms = STREAM_STOP_WAIT.as_millis(),
            "device log stream thread did not stop before restart; detaching"
        );
    }
}

fn next_probe_token() -> String {
    let n = PROBE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("daemon8_stream_probe_{n}")
}

fn probe_command_for_platform(platform: &DevicePlatform, token: &str) -> String {
    match platform {
        DevicePlatform::Vega => format!("logger -t daemon8_probe -p user.info {token}"),
        DevicePlatform::Android => format!("log -p i -t daemon8_probe {token}"),
    }
}

fn log_command_for_platform(platform: &DevicePlatform) -> String {
    match platform {
        DevicePlatform::Vega => "loggingctl log -f -o short_precise".into(),
        DevicePlatform::Android => "logcat -v threadtime".into(),
    }
}

struct DeviceLogBatcher {
    origin: Origin,
    parser: Box<dyn LogParser>,
    pending: HashMap<u64, BatchedParsedLine>,
    window_started: Instant,
    window: Duration,
}

struct BatchedParsedLine {
    parsed: ParsedLine,
    repeat_count: u64,
    first_seen_at: String,
    last_seen_at: String,
}

impl DeviceLogBatcher {
    fn new(origin: Origin, parser: Box<dyn LogParser>) -> Self {
        Self {
            origin,
            parser,
            pending: HashMap::new(),
            window_started: Instant::now(),
            window: DEVICE_BATCH_WINDOW,
        }
    }

    fn push(&mut self, line: String, obs_tx: &UnboundedSender<Observation>) {
        if self.window_started.elapsed() >= self.window {
            self.flush(obs_tx);
        }

        let Some(parsed) = self.parser.parse_line(&line) else {
            return;
        };

        let hash = parsed_line_hash(&self.origin, &parsed);
        let timestamp = parsed.timestamp.clone();

        if let Some(existing) = self.pending.get_mut(&hash) {
            existing.repeat_count = existing.repeat_count.saturating_add(1);
            existing.last_seen_at = timestamp;
            return;
        }

        self.pending.insert(
            hash,
            BatchedParsedLine {
                first_seen_at: timestamp.clone(),
                last_seen_at: timestamp,
                parsed,
                repeat_count: 1,
            },
        );
    }

    fn flush(&mut self, obs_tx: &UnboundedSender<Observation>) {
        for entry in self.pending.drain().map(|(_, entry)| entry) {
            let obs = parsed_to_observation(entry, &self.origin);
            let _ = obs_tx.send(obs);
        }
        self.window_started = Instant::now();
    }
}

fn parsed_line_hash(origin: &Origin, parsed: &ParsedLine) -> u64 {
    use std::hash::{Hash, Hasher};

    let (_, origin_key) = daemon8_types::observation_origin_fields(origin);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    origin_key.hash(&mut hasher);
    parsed.tag.hash(&mut hasher);
    parsed.severity.hash(&mut hasher);
    parsed.message.hash(&mut hasher);
    parsed.hostname.hash(&mut hasher);
    parsed.facility.hash(&mut hasher);
    hasher.finish()
}

fn parsed_to_observation(entry: BatchedParsedLine, origin: &Origin) -> Observation {
    let parsed = entry.parsed;
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
        "first_seen_at": entry.first_seen_at,
        "last_seen_at": entry.last_seen_at,
        "repeat_count": entry.repeat_count,
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

    let mut obs = Observation::new(origin.clone(), ObservationKind::Log, data, severity, None);
    obs.service = Some("adb".into());
    obs.source = Some("device.logs".into());
    if let Origin::Device { serial, platform } = origin {
        let stream = match platform {
            DevicePlatform::Vega => "loggingctl",
            DevicePlatform::Android => "logcat",
        };
        obs.source_instance = Some(format!("{serial}/{stream}").into());
    }
    obs
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
    use tokio::time::{Duration, timeout};

    use super::*;
    use crate::transport::DeviceLogStream;

    #[derive(Clone, Default)]
    struct FakeTransport {
        inner: Arc<Mutex<FakeInner>>,
    }

    #[derive(Default)]
    struct FakeInner {
        devices: Vec<DeviceInfo>,
        loggingctl_available: bool,
        list_count: usize,
        shell_commands: Vec<String>,
        commands: Vec<String>,
        streams: Vec<Option<UnboundedSender<String>>>,
    }

    impl FakeTransport {
        fn vega(serial: &str) -> Self {
            let fake = Self::default();
            {
                let mut inner = fake.inner.lock().unwrap();
                inner.devices.push(DeviceInfo {
                    serial: serial.into(),
                    model: "VVD".into(),
                    state: "device".into(),
                });
                inner.loggingctl_available = true;
            }
            fake
        }

        fn android(serial: &str) -> Self {
            let fake = Self::default();
            {
                let mut inner = fake.inner.lock().unwrap();
                inner.devices.push(DeviceInfo {
                    serial: serial.into(),
                    model: "Android".into(),
                    state: "device".into(),
                });
                inner.loggingctl_available = false;
            }
            fake
        }

        fn stream_count(&self) -> usize {
            self.inner.lock().unwrap().streams.len()
        }

        fn list_count(&self) -> usize {
            self.inner.lock().unwrap().list_count
        }

        fn send_line(&self, stream_idx: usize, line: &str) {
            let tx = self.inner.lock().unwrap().streams[stream_idx]
                .as_ref()
                .unwrap()
                .clone();
            tx.send(line.into()).unwrap();
        }

        fn close_stream(&self, stream_idx: usize) {
            self.inner.lock().unwrap().streams[stream_idx].take();
        }

        fn commands(&self) -> Vec<String> {
            self.inner.lock().unwrap().commands.clone()
        }

        fn shell_commands(&self) -> Vec<String> {
            self.inner.lock().unwrap().shell_commands.clone()
        }
    }

    #[async_trait]
    impl DeviceTransport for FakeTransport {
        async fn list_devices(&self) -> Result<Vec<DeviceInfo>> {
            let mut inner = self.inner.lock().unwrap();
            inner.list_count += 1;
            Ok(inner.devices.clone())
        }

        async fn shell_command(&self, _serial: &str, cmd: &str) -> Result<String> {
            let mut inner = self.inner.lock().unwrap();
            inner.shell_commands.push(cmd.into());
            if cmd == "which loggingctl" && inner.loggingctl_available {
                Ok("/usr/bin/loggingctl\n".into())
            } else {
                Ok(String::new())
            }
        }

        fn spawn_log_stream(&self, _serial: String, cmd: String) -> DeviceLogStream {
            let (tx, rx) = unbounded_channel();
            self.inner.lock().unwrap().commands.push(cmd);
            self.inner.lock().unwrap().streams.push(Some(tx));

            DeviceLogStream {
                handle: std::thread::spawn(|| Ok(())),
                rx,
                stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                done: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            }
        }
    }

    async fn handle_next_event<T: DeviceTransport>(manager: &mut DeviceManager<T>) {
        let event = timeout(Duration::from_secs(1), manager.event_rx.recv())
            .await
            .expect("device event timed out")
            .expect("device event channel closed");
        manager.handle_device_event(event).await;
    }

    fn vega_line(timestamp: &str, message: &str) -> String {
        format!(
            "{timestamp} amazon-vvd com.daemon8.vegasandbox[123]: INFO com.amazon.keplerscript: [KeplerScript-JavaScript] {message}"
        )
    }

    fn drain_observations(rx: &mut UnboundedReceiver<Observation>) -> Vec<Observation> {
        let mut observations = Vec::new();
        while let Ok(obs) = rx.try_recv() {
            observations.push(obs);
        }
        observations
    }

    #[tokio::test]
    async fn stream_end_restarts_same_connected_device() {
        let fake = FakeTransport::vega("emulator-5554");
        let (obs_tx, mut obs_rx) = unbounded_channel();
        let cancel = CancellationToken::new();
        let mut manager = DeviceManager::with_transport(fake.clone(), obs_tx, cancel, 60);

        manager.scan().await.unwrap();
        assert_eq!(fake.stream_count(), 1);

        fake.send_line(
            0,
            &vega_line("May 31 04:02:20.000000", "first after connect"),
        );
        handle_next_event(&mut manager).await;
        fake.close_stream(0);
        handle_next_event(&mut manager).await;

        assert_eq!(fake.stream_count(), 2, "stream end must restart log stream");
        assert_eq!(
            fake.commands(),
            vec![
                "loggingctl log -f -o short_precise",
                "loggingctl log -f -o short_precise"
            ]
        );

        fake.send_line(
            1,
            &vega_line("May 31 04:02:21.000000", "second after restart"),
        );
        handle_next_event(&mut manager).await;
        manager
            .devices
            .get_mut("emulator-5554")
            .unwrap()
            .batcher
            .flush(&manager.obs_tx);

        let observations = drain_observations(&mut obs_rx);
        let messages: Vec<String> = observations
            .iter()
            .filter_map(|obs| obs.data.get("message").and_then(|v| v.as_str()))
            .map(ToOwned::to_owned)
            .collect();

        assert!(
            messages.iter().any(|m| m.contains("first after connect")),
            "first stream line should be flushed before restart: {messages:?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("second after restart")),
            "restarted stream line should be observed: {messages:?}"
        );
    }

    #[tokio::test]
    async fn silent_stall_probe_timeout_restarts_same_connected_device() {
        let fake = FakeTransport::vega("emulator-5554");
        let (obs_tx, _obs_rx) = unbounded_channel();
        let cancel = CancellationToken::new();
        let mut manager = DeviceManager::with_transport(fake.clone(), obs_tx, cancel, 60);

        manager.scan().await.unwrap();
        assert_eq!(fake.stream_count(), 1);

        let state = manager.devices.get_mut("emulator-5554").unwrap();
        state.last_line_at = Instant::now() - STREAM_STALL_AFTER - Duration::from_secs(1);

        manager.check_stream_health().await;
        let shell_commands = fake.shell_commands();
        assert!(
            shell_commands
                .iter()
                .any(|cmd| cmd
                    .starts_with("logger -t daemon8_probe -p user.info daemon8_stream_probe_")),
            "stalled Vega stream should emit a logger probe: {shell_commands:?}"
        );
        assert_eq!(
            fake.stream_count(),
            1,
            "first stale check probes before restarting"
        );

        let state = manager.devices.get_mut("emulator-5554").unwrap();
        state.pending_probe.as_mut().unwrap().sent_at =
            Instant::now() - STREAM_PROBE_TIMEOUT - Duration::from_secs(1);

        manager.check_stream_health().await;
        assert_eq!(
            fake.stream_count(),
            2,
            "probe timeout should restart only the stale stream"
        );
    }

    #[tokio::test]
    async fn probe_ack_line_clears_pending_probe_without_observation_noise() {
        let fake = FakeTransport::vega("emulator-5554");
        let (obs_tx, mut obs_rx) = unbounded_channel();
        let cancel = CancellationToken::new();
        let mut manager = DeviceManager::with_transport(fake.clone(), obs_tx, cancel, 60);

        manager.scan().await.unwrap();
        let _ = drain_observations(&mut obs_rx);
        let state = manager.devices.get_mut("emulator-5554").unwrap();
        state.last_line_at = Instant::now() - STREAM_STALL_AFTER - Duration::from_secs(1);

        manager.check_stream_health().await;
        let token = manager.devices["emulator-5554"]
            .pending_probe
            .as_ref()
            .unwrap()
            .token
            .clone();

        fake.send_line(0, &vega_line("May 31 04:02:22.000000", &token));
        handle_next_event(&mut manager).await;

        assert!(
            manager.devices["emulator-5554"].pending_probe.is_none(),
            "probe acknowledgement should mark the stream healthy"
        );
        manager
            .devices
            .get_mut("emulator-5554")
            .unwrap()
            .batcher
            .flush(&manager.obs_tx);
        assert!(
            drain_observations(&mut obs_rx).is_empty(),
            "internal health probe lines should not become device observations"
        );
    }

    #[tokio::test]
    async fn disabled_vega_feature_skips_vvd_log_stream() {
        let fake = FakeTransport::vega("emulator-5554");
        let (obs_tx, mut obs_rx) = unbounded_channel();
        let cancel = CancellationToken::new();
        let mut manager = DeviceManager::with_transport_and_features(
            fake.clone(),
            obs_tx,
            cancel,
            60,
            DeviceMonitorFeatures {
                android: true,
                vega: false,
            },
        );

        manager.scan().await.unwrap();

        assert_eq!(fake.stream_count(), 0);
        assert!(manager.devices.is_empty());
        assert!(
            drain_observations(&mut obs_rx).is_empty(),
            "disabled platform should not emit lifecycle or log observations"
        );
    }

    #[tokio::test]
    async fn disabled_android_feature_skips_android_log_stream() {
        let fake = FakeTransport::android("emulator-5554");
        let (obs_tx, mut obs_rx) = unbounded_channel();
        let cancel = CancellationToken::new();
        let mut manager = DeviceManager::with_transport_and_features(
            fake.clone(),
            obs_tx,
            cancel,
            60,
            DeviceMonitorFeatures {
                android: false,
                vega: true,
            },
        );

        manager.scan().await.unwrap();

        assert_eq!(fake.stream_count(), 0);
        assert!(manager.devices.is_empty());
        assert!(
            drain_observations(&mut obs_rx).is_empty(),
            "disabled Android platform should not emit lifecycle or log observations"
        );
    }

    #[tokio::test]
    async fn run_keeps_scanning_after_batch_ticks() {
        let fake = FakeTransport::default();
        let (obs_tx, _obs_rx) = unbounded_channel();
        let cancel = CancellationToken::new();
        let mut manager = DeviceManager::with_transport(fake.clone(), obs_tx, cancel.clone(), 1);

        let task = tokio::spawn(async move { manager.run().await });
        tokio::time::sleep(Duration::from_millis(2300)).await;
        cancel.cancel();

        task.await.unwrap().unwrap();
        assert!(
            fake.list_count() >= 3,
            "initial scan plus interval scans should continue despite batch ticks"
        );
    }

    #[test]
    fn batcher_collapses_repeated_device_lines_with_different_timestamps() {
        let (obs_tx, mut obs_rx) = unbounded_channel();
        let origin = Origin::Device {
            serial: "emulator-5554".into(),
            platform: DevicePlatform::Vega,
        };
        let mut batcher = DeviceLogBatcher::new(origin, Box::new(LoggingctlParser));

        for i in 0..100 {
            batcher.push(
                vega_line(
                    &format!("May 31 04:02:{:02}.000000", i % 60),
                    "[MountingManager::SurfaceTelemetryLogger] mutations=527 transactions=5",
                ),
                &obs_tx,
            );
        }
        batcher.flush(&obs_tx);

        let observations = drain_observations(&mut obs_rx);
        assert_eq!(
            observations.len(),
            1,
            "timestamp-only duplicates should collapse into one observation"
        );
        assert_eq!(observations[0].data["repeat_count"], serde_json::json!(100));
        assert_eq!(
            observations[0].data["message"],
            serde_json::json!(
                "com.amazon.keplerscript: [KeplerScript-JavaScript] [MountingManager::SurfaceTelemetryLogger] mutations=527 transactions=5"
            )
        );
    }
}
