// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

// End-to-end file source pipeline: config file → source registration →
// lazy activation via query → file watcher tails new lines → parser
// extracts fields → observation stored in DB → queryable via HTTP API.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

struct ChildGuard(std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn toml_path(p: &Path) -> String {
    p.display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn daemon8_command() -> std::process::Command {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_daemon8"));
    for (key, _) in std::env::vars() {
        if key.starts_with("DAEMON8_") {
            cmd.env_remove(key);
        }
    }
    cmd
}

struct Sandbox {
    _tmp: tempfile::TempDir,
    config_path: std::path::PathBuf,
    log_dir: std::path::PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let tmp = tempfile::TempDir::new().unwrap();
        let log_dir = tmp.path().join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();

        let json_log = log_dir.join("json.log");
        let logfmt_log = log_dir.join("logfmt.log");
        let monolog_log = log_dir.join("monolog.log");
        let syslog_log = log_dir.join("syslog.log");
        let access_log = log_dir.join("access.log");
        let plain_log = log_dir.join("plain.log");
        let mixed_log = log_dir.join("mixed.log");

        for path in [
            &json_log,
            &logfmt_log,
            &monolog_log,
            &syslog_log,
            &access_log,
            &plain_log,
            &mixed_log,
        ] {
            std::fs::write(path, "").unwrap();
        }

        let store_path = tmp.path().join("store");
        let screenshot_dir = tmp.path().join("screenshots");
        let config_path = tmp.path().join("config.toml");

        std::fs::write(
            &config_path,
            format!(
                r#"[storage]
path = "{store}"
screenshot_path = "{screenshots}"

[adb]
enabled = false

[logging]
level = "debug"
stderr = false

[sources.structured-json]
kind = "file"
path = "{json}"
parser = "json"
tags = ["json", "structured"]

[sources.app-logfmt]
kind = "file"
path = "{logfmt}"
parser = "logfmt"
tags = ["logfmt", "structured"]

[sources.php-monolog]
kind = "file"
path = "{monolog}"
parser = "monolog"
tags = ["monolog", "php"]

[sources.system-syslog]
kind = "file"
path = "{syslog}"
parser = "syslog"
tags = ["syslog", "system"]

[sources.web-access]
kind = "file"
path = "{access}"
parser = "clf"
tags = ["clf", "http"]

[sources.plaintext]
kind = "file"
path = "{plain}"
parser = "line"
tags = ["plain", "fallback"]

[sources.auto-mixed]
kind = "file"
path = "{mixed}"
parser = "auto"
tags = ["auto", "mixed"]
"#,
                store = toml_path(&store_path),
                screenshots = toml_path(&screenshot_dir),
                json = toml_path(&json_log),
                logfmt = toml_path(&logfmt_log),
                monolog = toml_path(&monolog_log),
                syslog = toml_path(&syslog_log),
                access = toml_path(&access_log),
                plain = toml_path(&plain_log),
                mixed = toml_path(&mixed_log),
            ),
        )
        .unwrap();

        Self {
            _tmp: tmp,
            config_path,
            log_dir,
        }
    }

    fn log_path(&self, name: &str) -> std::path::PathBuf {
        self.log_dir.join(name)
    }

    fn append(&self, name: &str, lines: &[&str]) {
        let path = self.log_path(name);
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
    }
}

async fn wait_for_health(base: &str) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);

    loop {
        if let Ok(resp) = client.get(format!("{base}/health")).send().await
            && resp.status().is_success()
            && resp.text().await.unwrap_or_default() == "ok"
        {
            return;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "daemon8 serve did not become healthy at {base}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn query_obs(base: &str, params: &str) -> (Vec<Value>, u64) {
    let resp: Value = reqwest::get(format!("{base}/api/observe?{params}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let obs = resp["observations"].as_array().cloned().unwrap_or_default();
    let checkpoint = resp["checkpoint"].as_u64().unwrap_or(0);
    (obs, checkpoint)
}

async fn poll_until(base: &str, params: &str, min_count: usize) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    loop {
        let (obs, _) = query_obs(base, params).await;
        if obs.len() >= min_count {
            return obs;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {min_count} observations at {params}, got {}",
            obs.len()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn start_daemon(sandbox: &Sandbox) -> (String, ChildGuard) {
    let port = free_port();
    let child = daemon8_command()
        .args([
            "--config",
            sandbox.config_path.to_str().unwrap(),
            "serve",
            "--port",
            &port.to_string(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    (format!("http://127.0.0.1:{port}"), ChildGuard(child))
}

// -----------------------------------------------------------------------
// Full pipeline: config → watcher → parser → DB → query
// -----------------------------------------------------------------------

#[tokio::test]
async fn json_source_pipeline() {
    let sandbox = Sandbox::new();
    let (base, _child) = start_daemon(&sandbox);
    wait_for_health(&base).await;

    // First query activates the source (lazy activation)
    let _ = query_obs(&base, "tags=json&limit=1").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    sandbox.append(
        "json.log",
        &[
            r#"{"timestamp":"2026-01-15T10:00:00Z","level":"info","msg":"request handled","method":"GET","path":"/api/users","duration_ms":42}"#,
            r#"{"timestamp":"2026-01-15T10:00:01Z","level":"error","msg":"connection pool exhausted","pool":"primary","active":50}"#,
            r#"{"timestamp":"2026-01-15T10:00:02Z","level":"debug","msg":"cache miss","key":"user:1234"}"#,
        ],
    );

    let obs = poll_until(&base, "tags=json&limit=10", 3).await;

    assert_eq!(obs.len(), 3);

    let severities: Vec<&str> = obs
        .iter()
        .map(|o| o["severity"].as_str().unwrap())
        .collect();
    assert!(severities.contains(&"info"));
    assert!(severities.contains(&"error"));
    assert!(severities.contains(&"debug"));

    let info_obs = obs.iter().find(|o| o["severity"] == "info").unwrap();
    assert_eq!(info_obs["data"]["message"], "request handled");
    assert_eq!(info_obs["data"]["method"], "GET");
    assert_eq!(info_obs["data"]["duration_ms"], 42);
    assert_eq!(info_obs["origin"]["type"], "application");
}

#[tokio::test]
async fn logfmt_source_pipeline() {
    let sandbox = Sandbox::new();
    let (base, _child) = start_daemon(&sandbox);
    wait_for_health(&base).await;

    let _ = query_obs(&base, "tags=logfmt&limit=1").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    sandbox.append(
        "logfmt.log",
        &[
            r#"ts=2026-01-15T10:00:00Z level=info msg="server started" port=8080 workers=4"#,
            r#"ts=2026-01-15T10:00:01Z level=error msg="redis down" host=redis-01 retries=3"#,
        ],
    );

    let obs = poll_until(&base, "tags=logfmt&limit=10", 2).await;

    assert_eq!(obs.len(), 2);
    assert!(obs.iter().any(|o| o["severity"] == "info"));
    assert!(obs.iter().any(|o| o["severity"] == "error"));

    let info_obs = obs.iter().find(|o| o["severity"] == "info").unwrap();
    assert_eq!(info_obs["data"]["message"], "server started");
    assert_eq!(info_obs["data"]["port"], "8080");
}

#[tokio::test]
async fn monolog_source_pipeline() {
    let sandbox = Sandbox::new();
    let (base, _child) = start_daemon(&sandbox);
    wait_for_health(&base).await;

    let _ = query_obs(&base, "tags=monolog&limit=1").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    sandbox.append(
        "monolog.log",
        &[
            r#"[2026-01-15 10:00:00] app.INFO: User logged in {"user_id":42,"ip":"10.0.0.1"} []"#,
            r#"[2026-01-15 10:00:01] security.ERROR: Failed login {"email":"admin@example.com","attempts":5} []"#,
        ],
    );

    let obs = poll_until(&base, "tags=monolog&limit=10", 2).await;

    assert_eq!(obs.len(), 2);

    let info_obs = obs.iter().find(|o| o["severity"] == "info").unwrap();
    assert_eq!(info_obs["data"]["message"], "User logged in");
    assert_eq!(info_obs["data"]["channel"], "app");
    assert_eq!(info_obs["data"]["user_id"], 42);

    let error_obs = obs.iter().find(|o| o["severity"] == "error").unwrap();
    assert_eq!(error_obs["data"]["channel"], "security");
}

#[tokio::test]
async fn syslog_source_pipeline() {
    let sandbox = Sandbox::new();
    let (base, _child) = start_daemon(&sandbox);
    wait_for_health(&base).await;

    let _ = query_obs(&base, "tags=syslog&limit=1").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    sandbox.append(
        "syslog.log",
        &[
            "<14>Jan 15 10:00:00 myhost sshd[12345]: Accepted publickey for admin from 10.0.0.1 port 22",
            "<11>Jan 15 10:00:01 myhost kernel: Out of memory: Kill process 9876 (oom-victim)",
        ],
    );

    let obs = poll_until(&base, "tags=syslog&limit=10", 2).await;

    assert_eq!(obs.len(), 2);
    assert!(obs.iter().any(|o| o["severity"] == "info"));
    assert!(obs.iter().any(|o| o["severity"] == "error"));

    let syslog_obs = obs.iter().find(|o| o["severity"] == "info").unwrap();
    assert_eq!(syslog_obs["data"]["hostname"], "myhost");
    assert_eq!(syslog_obs["data"]["app"], "sshd");
}

#[tokio::test]
async fn clf_source_pipeline() {
    let sandbox = Sandbox::new();
    let (base, _child) = start_daemon(&sandbox);
    wait_for_health(&base).await;

    let _ = query_obs(&base, "tags=clf&limit=1").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    sandbox.append(
        "access.log",
        &[
            r#"192.168.1.10 - alice [15/Jan/2026:10:00:00 -0700] "GET /dashboard HTTP/1.1" 200 15234 "https://app.example.com/" "Mozilla/5.0""#,
            r#"10.0.0.20 - - [15/Jan/2026:10:00:01 -0700] "DELETE /api/data HTTP/1.1" 500 567 "-" "python-requests/2.31""#,
        ],
    );

    let obs = poll_until(&base, "tags=clf&limit=10", 2).await;

    assert_eq!(obs.len(), 2);

    let ok_obs = obs.iter().find(|o| o["data"]["status"] == 200).unwrap();
    assert_eq!(ok_obs["data"]["client_ip"], "192.168.1.10");
    assert_eq!(ok_obs["data"]["method"], "GET");
    assert_eq!(ok_obs["severity"], "info");

    let err_obs = obs.iter().find(|o| o["data"]["status"] == 500).unwrap();
    assert_eq!(err_obs["severity"], "error");
}

#[tokio::test]
async fn plaintext_severity_sniffing_pipeline() {
    let sandbox = Sandbox::new();
    let (base, _child) = start_daemon(&sandbox);
    wait_for_health(&base).await;

    let _ = query_obs(&base, "tags=plain&limit=1").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    sandbox.append(
        "plain.log",
        &[
            "[2026-01-15T10:00:00Z] Application starting up...",
            "[2026-01-15T10:00:01Z] WARNING: slow API endpoint called",
            "[2026-01-15T10:00:02Z] ERROR: disk space critically low",
        ],
    );

    let obs = poll_until(&base, "tags=plain&limit=10", 3).await;

    assert_eq!(obs.len(), 3);

    let severities: Vec<&str> = obs
        .iter()
        .map(|o| o["severity"].as_str().unwrap())
        .collect();
    assert!(
        severities.contains(&"info"),
        "plain line without keyword defaults to info"
    );
    assert!(
        severities.contains(&"warn"),
        "WARNING should be sniffed as warn"
    );
    assert!(
        severities.contains(&"error"),
        "ERROR should be sniffed as error"
    );
}

#[tokio::test]
async fn auto_detect_mixed_formats_pipeline() {
    let sandbox = Sandbox::new();
    let (base, _child) = start_daemon(&sandbox);
    wait_for_health(&base).await;

    let _ = query_obs(&base, "tags=auto&limit=1").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    sandbox.append(
        "mixed.log",
        &[
            r#"{"timestamp":"2026-01-15T10:00:00Z","level":"info","msg":"json line"}"#,
            r#"ts=2026-01-15T10:00:01Z level=error msg="logfmt line" code=500"#,
            r#"[2026-01-15 10:00:02] app.WARNING: monolog line {} []"#,
            r#"<14>Jan 15 10:00:03 myhost auto[999]: syslog line"#,
            "just a plain ERROR line",
        ],
    );

    let obs = poll_until(&base, "tags=auto&limit=10", 5).await;

    assert_eq!(obs.len(), 5);

    let json_obs = obs
        .iter()
        .find(|o| o["data"]["message"] == "json line")
        .unwrap();
    assert_eq!(json_obs["severity"], "info");

    let logfmt_obs = obs
        .iter()
        .find(|o| o["data"]["message"] == "logfmt line")
        .unwrap();
    assert_eq!(logfmt_obs["severity"], "error");

    let monolog_obs = obs
        .iter()
        .find(|o| o["data"]["message"] == "monolog line")
        .unwrap();
    assert_eq!(monolog_obs["severity"], "warn");
    assert_eq!(monolog_obs["data"]["channel"], "app");

    let syslog_obs = obs
        .iter()
        .find(|o| o["data"]["message"] == "syslog line")
        .unwrap();
    assert_eq!(syslog_obs["data"]["hostname"], "myhost");

    let plain_obs = obs
        .iter()
        .find(|o| {
            o["data"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("plain ERROR"))
        })
        .unwrap();
    assert_eq!(plain_obs["severity"], "error");
}

// -----------------------------------------------------------------------
// Cross-source: all 7 sources activated and producing in one daemon
// -----------------------------------------------------------------------

#[tokio::test]
async fn all_sources_produce_observations() {
    let sandbox = Sandbox::new();
    let (base, _child) = start_daemon(&sandbox);
    wait_for_health(&base).await;

    // Activate all sources by querying each tag
    for tag in [
        "json", "logfmt", "monolog", "syslog", "clf", "plain", "auto",
    ] {
        let _ = query_obs(&base, &format!("tags={tag}&limit=1")).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    sandbox.append("json.log", &[r#"{"level":"info","msg":"json-check"}"#]);
    sandbox.append(
        "logfmt.log",
        &[r#"ts=2026-01-15T10:00:00Z level=info msg="logfmt-check""#],
    );
    sandbox.append(
        "monolog.log",
        &[r#"[2026-01-15 10:00:00] app.INFO: monolog-check {} []"#],
    );
    sandbox.append(
        "syslog.log",
        &["<14>Jan 15 10:00:00 myhost test[1]: syslog-check"],
    );
    sandbox.append(
        "access.log",
        &[r#"127.0.0.1 - - [15/Jan/2026:10:00:00 -0700] "GET /clf-check HTTP/1.1" 200 100"#],
    );
    sandbox.append("plain.log", &["plain-check"]);
    sandbox.append("mixed.log", &[r#"{"level":"info","msg":"auto-check"}"#]);

    let obs = poll_until(
        &base,
        "tags=json,logfmt,monolog,syslog,clf,plain,auto&limit=50",
        7,
    )
    .await;

    let messages: Vec<String> = obs
        .iter()
        .filter_map(|o| o["data"]["message"].as_str().map(String::from))
        .collect();

    assert!(
        messages.iter().any(|m| m == "json-check"),
        "missing json source: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m == "logfmt-check"),
        "missing logfmt source: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m == "monolog-check"),
        "missing monolog source: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m == "syslog-check"),
        "missing syslog source: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("clf-check")),
        "missing clf source: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m == "plain-check"),
        "missing plain source: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m == "auto-check"),
        "missing auto source: {messages:?}"
    );
}
