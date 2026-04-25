// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::net::TcpListener;
use std::path::PathBuf;

use anyhow::Result;

use crate::config;

struct Check {
    name: &'static str,
    result: CheckResult,
}

enum CheckResult {
    Ok,
    Fixed(String),
    Warn(String),
    Err(String),
}

impl Check {
    fn is_failure(&self) -> bool {
        matches!(self.result, CheckResult::Err(_))
    }
}

impl std::fmt::Display for Check {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.result {
            CheckResult::Ok => write!(f, "[ok]     {}", self.name),
            CheckResult::Fixed(msg) => write!(f, "[fixed]  {} ({})", self.name, msg),
            CheckResult::Warn(msg) => write!(f, "[WARN]   {} ({})", self.name, msg),
            CheckResult::Err(msg) => write!(f, "[ERR]    {} ({})", self.name, msg),
        }
    }
}

pub fn cmd_doctor(fix: bool) -> Result<()> {
    let cfg = crate::config::load(None).unwrap_or_default();
    let config_path = cfg.config_dir.join("config.toml");

    let mut checks = vec![
        check_config_file(&config_path, fix),
        check_screenshot_dir(&cfg, fix),
        check_data_dir(&cfg, fix),
        check_port(cfg.server.port),
        check_network(),
    ];

    #[cfg(target_os = "macos")]
    checks.push(check_macos_launchd_state());

    let has_failure = checks.iter().any(|c| c.is_failure());
    let has_warning = checks
        .iter()
        .any(|c| matches!(c.result, CheckResult::Warn(_)));

    for check in &checks {
        println!("{check}");
    }

    if has_failure {
        eprintln!("\ndoctor: errors found — run 'daemon8 doctor --fix' to repair");
        std::process::exit(1);
    } else if has_warning {
        eprintln!("\ndoctor: warnings found — run 'daemon8 doctor --fix' to resolve");
    } else {
        eprintln!("\ndoctor: all checks passed");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Individual checks
// ---------------------------------------------------------------------------

fn check_config_file(config_path: &std::path::Path, fix: bool) -> Check {
    if config_path.exists() {
        return Check {
            name: "config file",
            result: CheckResult::Ok,
        };
    }

    if !fix {
        return Check {
            name: "config file",
            result: CheckResult::Warn("missing (run doctor --fix to create)".into()),
        };
    }

    // Create parent dir + empty default config
    if let Some(parent) = config_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return Check {
            name: "config file",
            result: CheckResult::Err(format!("could not create config dir: {e}")),
        };
    }

    let default_cfg = config::Config::default();
    match toml::to_string_pretty(&default_cfg) {
        Ok(content) => match std::fs::write(config_path, content) {
            Ok(()) => Check {
                name: "config file",
                result: CheckResult::Fixed("created with defaults".into()),
            },
            Err(e) => Check {
                name: "config file",
                result: CheckResult::Err(format!("write failed: {e}")),
            },
        },
        Err(e) => Check {
            name: "config file",
            result: CheckResult::Err(format!("serialize failed: {e}")),
        },
    }
}

fn check_screenshot_dir(cfg: &config::Config, fix: bool) -> Check {
    let dir = resolve_screenshot_path_no_create(cfg);

    if dir.exists() {
        return match is_writable(&dir) {
            true => Check {
                name: "screenshot dir",
                result: CheckResult::Ok,
            },
            false => Check {
                name: "screenshot dir",
                result: CheckResult::Err(format!("not writable: {}", dir.display())),
            },
        };
    }

    if !fix {
        return Check {
            name: "screenshot dir",
            result: CheckResult::Warn(format!(
                "missing: {} (run doctor --fix to create)",
                dir.display()
            )),
        };
    }

    match std::fs::create_dir_all(&dir) {
        Ok(()) => Check {
            name: "screenshot dir",
            result: CheckResult::Fixed(format!("created {}", dir.display())),
        },
        Err(e) => Check {
            name: "screenshot dir",
            result: CheckResult::Err(format!("could not create: {e}")),
        },
    }
}

fn check_data_dir(cfg: &config::Config, fix: bool) -> Check {
    let db_path = config::resolve_db_path(cfg.storage.path.as_deref());
    let dir = db_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    if dir.exists() && is_writable(&dir) {
        return Check {
            name: "data dir",
            result: CheckResult::Ok,
        };
    }

    if !dir.exists() {
        if fix {
            return match std::fs::create_dir_all(&dir) {
                Ok(()) => Check {
                    name: "data dir",
                    result: CheckResult::Fixed(format!("created {}", dir.display())),
                },
                Err(e) => Check {
                    name: "data dir",
                    result: CheckResult::Err(format!("could not create: {e}")),
                },
            };
        }
        return Check {
            name: "data dir",
            result: CheckResult::Warn(format!("missing: {}", dir.display())),
        };
    }

    Check {
        name: "data dir",
        result: CheckResult::Err(format!("not writable: {}", dir.display())),
    }
}

fn check_port(port: u16) -> Check {
    // Leak is intentional: doctor is a one-shot CLI command, runs once, exits.
    let name: &'static str = Box::leak(format!("port {port}").into_boxed_str());
    match TcpListener::bind(format!("127.0.0.1:{port}")) {
        Ok(_) => Check {
            name,
            result: CheckResult::Ok,
        },
        Err(_) => {
            if probe_daemon_health(port) {
                Check {
                    name,
                    result: CheckResult::Ok,
                }
            } else {
                Check {
                    name,
                    result: CheckResult::Warn(
                        "in use by another process (not a healthy daemon)".into(),
                    ),
                }
            }
        }
    }
}

fn probe_daemon_health(port: u16) -> bool {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let Ok(mut stream) = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        std::time::Duration::from_secs(2),
    ) else {
        return false;
    };
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .ok();

    let request = format!("GET /health HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }

    let mut buf = [0u8; 256];
    let n = stream.read(&mut buf).unwrap_or(0);
    let response = String::from_utf8_lossy(&buf[..n]);
    response.contains("200") && response.contains("ok")
}

fn check_network() -> Check {
    use std::net::{TcpStream, ToSocketAddrs};

    // TCP-connect test against a well-known host to prove DNS + outbound
    // connectivity without pulling in an HTTP client just for doctor.
    let addr = match "daemon8.ai:443".to_socket_addrs() {
        Ok(mut addrs) => addrs.next(),
        Err(_) => None,
    };

    let reachable = addr
        .is_some_and(|a| TcpStream::connect_timeout(&a, std::time::Duration::from_secs(3)).is_ok());

    Check {
        name: "network reachable",
        result: if reachable {
            CheckResult::Ok
        } else {
            CheckResult::Warn("outbound network unreachable".into())
        },
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Same resolution as `config::resolve_screenshot_path` but without auto-creating
/// the directory -- we need to detect whether it exists for the check.
fn resolve_screenshot_path_no_create(cfg: &config::Config) -> PathBuf {
    if let Some(raw) = cfg.storage.screenshot_path.as_deref() {
        let raw_str = raw.to_string_lossy();
        if let Some(rest) = raw_str.strip_prefix('~')
            && let Some(home) = dirs::home_dir()
        {
            return home.join(rest.strip_prefix('/').unwrap_or(rest));
        }
        return raw.to_path_buf();
    }

    if let Some(db) = cfg.storage.path.as_deref() {
        return db
            .parent()
            .map(|p| p.join("screenshots"))
            .unwrap_or_else(|| PathBuf::from("screenshots"));
    }

    directories::ProjectDirs::from(
        "dev",
        "daemon8",
        if cfg!(debug_assertions) {
            "daemon8-dev"
        } else {
            "daemon8"
        },
    )
    .map(|d| d.data_dir().join("screenshots"))
    .unwrap_or_else(|| PathBuf::from("screenshots"))
}

// macOS launchd service state probe. On Sonoma+ a failed launchctl load
// usually means either the ad-hoc codesign identity churned OR the user
// hasn't granted App Management in System Settings. Both are recoverable
// without code changes, so the remediation text is the load-bearing output.
#[cfg(target_os = "macos")]
fn check_macos_launchd_state() -> Check {
    let uid = unsafe { libc::geteuid() };
    let target = format!("gui/{uid}/dev.daemon8.daemon");

    let output = match std::process::Command::new("launchctl")
        .args(["print", &target])
        .output()
    {
        Ok(o) => o,
        Err(_) => {
            return Check {
                name: "launchd service",
                result: CheckResult::Warn(
                    "launchctl unavailable — run `daemon8 install` to register".into(),
                ),
            };
        }
    };

    if !output.status.success() {
        return Check {
            name: "launchd service",
            result: CheckResult::Warn(
                "not registered — run `daemon8 install` to set up the launchd agent".into(),
            ),
        };
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let state = text
        .lines()
        .find(|l| l.trim_start().starts_with("state ="))
        .and_then(|l| l.split('=').nth(1))
        .map(|s| s.trim())
        .unwrap_or("unknown");

    match state {
        "running" => Check {
            name: "launchd service",
            result: CheckResult::Ok,
        },
        "not running" | "waiting" => Check {
            name: "launchd service",
            result: CheckResult::Err(format!(
                "state={state}: launchd registered but cannot start. Check (1) codesign identity — re-run `codesign --force --sign - ~/.cargo/bin/daemon8`, (2) App Management — open System Settings > Privacy & Security > App Management and toggle daemon8 on."
            )),
        },
        other => Check {
            name: "launchd service",
            result: CheckResult::Warn(format!(
                "state={other} — inspect `launchctl print {target}`"
            )),
        },
    }
}

fn is_writable(dir: &std::path::Path) -> bool {
    let test_file = dir.join(".daemon8-doctor-probe");
    match std::fs::write(&test_file, b"probe") {
        Ok(()) => {
            let _ = std::fs::remove_file(&test_file);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_config_file_missing_no_fix() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");

        let result = check_config_file(&config_path, false);
        assert!(
            matches!(result.result, CheckResult::Warn(_)),
            "missing config file with fix=false should return Warn"
        );
    }

    #[test]
    fn check_port_in_use() {
        let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => return,
            Err(err) => panic!("failed to bind test listener: {err}"),
        };
        let port = listener.local_addr().unwrap().port();

        let result = check_port(port);
        assert!(
            matches!(result.result, CheckResult::Warn(_)),
            "occupied port should return Warn"
        );
    }
}
