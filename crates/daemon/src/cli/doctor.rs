// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::net::TcpListener;
use std::path::{Path, PathBuf};

use anyhow::Result;
use daemon8_embed::EmbedProvider;
use daemon8_store::StateModel;

use super::observe::{base_url, check_response};
use crate::config::{self, SourceConfig};

struct Check {
    name: &'static str,
    result: CheckResult,
}

enum CheckResult {
    Ok,
    OkHint(String),
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
            CheckResult::OkHint(msg) => write!(f, "[ok]     {} ({})", self.name, msg),
            CheckResult::Fixed(msg) => write!(f, "[fixed]  {} ({})", self.name, msg),
            CheckResult::Warn(msg) => write!(f, "[WARN]   {} ({})", self.name, msg),
            CheckResult::Err(msg) => write!(f, "[ERR]    {} ({})", self.name, msg),
        }
    }
}

pub async fn cmd_doctor(config_path: Option<String>, fix: bool) -> Result<()> {
    let cfg = match crate::config::load(config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[ERR]    config load ({e})");
            std::process::exit(1);
        }
    };
    let config_path_abs = cfg.config_dir.join("config.toml");
    let port = cfg.server.port;

    let mut checks = vec![
        check_config_file(&config_path_abs, fix),
        check_screenshot_dir(&cfg, fix),
        check_data_dir(&cfg, fix),
        check_port(port),
        check_network(),
        check_setup_state(&cfg),
        check_sources(&cfg),
        check_embeddings(&cfg),
        check_store(&cfg, port).await,
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

fn check_setup_state(cfg: &config::Config) -> Check {
    if cfg.setup.projects.is_empty() {
        return Check {
            name: "setup state",
            result: CheckResult::OkHint("no projects applied".into()),
        };
    }

    let mut warnings: Vec<String> = Vec::new();

    for (slug, state) in &cfg.setup.projects {
        let root = Path::new(&state.root_path);
        let cfg_file = Path::new(&state.config_path);

        if !root.exists() {
            warnings.push(format!("{slug}: root_path missing ({})", state.root_path));
        }
        if !cfg_file.exists() {
            warnings.push(format!(
                "{slug}: project config missing ({})",
                state.config_path
            ));
        }
    }

    if warnings.is_empty() {
        Check {
            name: "setup state",
            result: CheckResult::OkHint(format!("{} project(s) applied", cfg.setup.projects.len())),
        }
    } else {
        Check {
            name: "setup state",
            result: CheckResult::Warn(warnings.join("; ")),
        }
    }
}

fn check_sources(cfg: &config::Config) -> Check {
    if cfg.sources.is_empty() {
        return Check {
            name: "sources",
            result: CheckResult::OkHint("none configured".into()),
        };
    }

    let mut warnings: Vec<String> = Vec::new();

    for (name, source) in &cfg.sources {
        match source {
            SourceConfig::File(f) => {
                let path = Path::new(&f.path);
                let is_glob = f.path.contains('*') || f.path.contains('?');

                if is_glob {
                    if let Some(parent) = path.parent()
                        && !parent.as_os_str().is_empty()
                        && !parent.exists()
                    {
                        warnings.push(format!("{name}: parent dir missing ({})", parent.display()));
                    }
                } else if !path.exists() {
                    warnings.push(format!("{name}: path not found ({})", f.path));
                }

                if let Err(e) = daemon8_parse::resolve_parser(&f.parser) {
                    warnings.push(format!("{name}: parser '{}' — {e}", f.parser));
                }
            }
        }
    }

    if warnings.is_empty() {
        Check {
            name: "sources",
            result: CheckResult::OkHint(format!("{} configured", cfg.sources.len())),
        }
    } else {
        Check {
            name: "sources",
            result: CheckResult::Warn(warnings.join("; ")),
        }
    }
}

fn check_embeddings(cfg: &config::Config) -> Check {
    match cfg.embeddings.provider {
        EmbedProvider::None => Check {
            name: "embeddings",
            result: CheckResult::OkHint("disabled".into()),
        },
        EmbedProvider::Fastembed => Check {
            name: "embeddings",
            result: CheckResult::OkHint(format!("fastembed, model={}", cfg.embeddings.model)),
        },
        EmbedProvider::Ollama => {
            let endpoint = cfg
                .embeddings
                .endpoint
                .as_deref()
                .unwrap_or("http://localhost:11434");

            let reachable = parse_host_port(endpoint).is_some_and(|addr| {
                std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(2))
                    .is_ok()
            });

            if reachable {
                Check {
                    name: "embeddings",
                    result: CheckResult::OkHint(format!(
                        "ollama reachable, model={}",
                        cfg.embeddings.model
                    )),
                }
            } else {
                Check {
                    name: "embeddings",
                    result: CheckResult::Warn(format!(
                        "ollama unreachable at {endpoint} — is it running?"
                    )),
                }
            }
        }
        EmbedProvider::Openai => {
            let has_key = cfg
                .embeddings
                .api_key
                .as_ref()
                .is_some_and(|k| !k.is_empty());

            if has_key {
                Check {
                    name: "embeddings",
                    result: CheckResult::OkHint(format!("openai, model={}", cfg.embeddings.model)),
                }
            } else {
                Check {
                    name: "embeddings",
                    result: CheckResult::Warn(
                        "openai provider configured but api_key is missing".into(),
                    ),
                }
            }
        }
    }
}

async fn check_store(cfg: &config::Config, port: u16) -> Check {
    const NAME: &str = "store";

    // Try API first
    let url = format!("{}/api/summary", base_url(port));
    if let Ok(resp) = reqwest::get(&url).await
        && let Ok(_) = check_response(resp).await
    {
        return Check {
            name: NAME,
            result: CheckResult::Ok,
        };
    }

    let db_path = config::resolve_db_path(cfg.storage.path.as_deref());

    if !db_path.exists() {
        return Check {
            name: NAME,
            result: CheckResult::OkHint("not yet created (first run will initialize)".into()),
        };
    }

    match daemon8_store::SurrealStore::open(&db_path).await {
        Ok(store) => match store.health_check().await {
            Ok(()) => Check {
                name: NAME,
                result: CheckResult::Ok,
            },
            Err(e) => Check {
                name: NAME,
                result: CheckResult::Err(format!("health check failed: {e}")),
            },
        },
        Err(e) => Check {
            name: NAME,
            result: CheckResult::Err(format!("could not open database: {e}")),
        },
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

fn parse_host_port(url: &str) -> Option<std::net::SocketAddr> {
    use std::net::ToSocketAddrs;

    let stripped = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);

    let authority = stripped.split('/').next().unwrap_or(stripped);
    authority.to_socket_addrs().ok()?.next()
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

    fn applied_state(slug: &str, root: &Path, cfg_path: &Path) -> config::ProjectSetupState {
        config::ProjectSetupState {
            slug: slug.into(),
            root_path: root.display().to_string(),
            config_path: cfg_path.display().to_string(),
            applied_at_ns: 0,
            desired_scope: Vec::new(),
            hook_policy: "manual".into(),
            sources: Vec::new(),
            source_audit: Vec::new(),
        }
    }

    #[test]
    fn check_setup_state_empty_is_ok_hint() {
        let cfg = config::Config::default();
        let result = check_setup_state(&cfg);
        assert!(matches!(result.result, CheckResult::OkHint(_)));
    }

    #[test]
    fn check_setup_state_present_paths_is_ok_hint_with_count() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let cfg_path = root.join(".daemon8.toml");
        std::fs::write(&cfg_path, "version = 1\n").unwrap();

        let mut cfg = config::Config::default();
        cfg.setup
            .projects
            .insert("demo".into(), applied_state("demo", &root, &cfg_path));

        let result = check_setup_state(&cfg);
        match result.result {
            CheckResult::OkHint(msg) => assert!(msg.contains("1 project")),
            other => panic!("expected OkHint, got {other:?}"),
        }
    }

    #[test]
    fn check_setup_state_missing_root_warns() {
        let mut cfg = config::Config::default();
        let bogus_root = PathBuf::from("/nonexistent/daemon8-doctor-root");
        let bogus_cfg = bogus_root.join(".daemon8.toml");
        cfg.setup.projects.insert(
            "ghost".into(),
            applied_state("ghost", &bogus_root, &bogus_cfg),
        );

        let result = check_setup_state(&cfg);
        match result.result {
            CheckResult::Warn(msg) => {
                assert!(msg.contains("ghost: root_path missing"), "got: {msg}");
                assert!(msg.contains("ghost: project config missing"), "got: {msg}");
            }
            other => panic!("expected Warn, got {other:?}"),
        }
    }

    #[test]
    fn check_setup_state_missing_config_only_warns_about_config() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let absent_cfg = root.join("missing.toml");

        let mut cfg = config::Config::default();
        cfg.setup
            .projects
            .insert("demo".into(), applied_state("demo", &root, &absent_cfg));

        let result = check_setup_state(&cfg);
        match result.result {
            CheckResult::Warn(msg) => {
                assert!(
                    !msg.contains("root_path missing"),
                    "root exists, should not warn: {msg}"
                );
                assert!(msg.contains("project config missing"), "got: {msg}");
            }
            other => panic!("expected Warn, got {other:?}"),
        }
    }

    impl std::fmt::Debug for CheckResult {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                CheckResult::Ok => write!(f, "Ok"),
                CheckResult::OkHint(s) => write!(f, "OkHint({s})"),
                CheckResult::Fixed(s) => write!(f, "Fixed({s})"),
                CheckResult::Warn(s) => write!(f, "Warn({s})"),
                CheckResult::Err(s) => write!(f, "Err({s})"),
            }
        }
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

    #[test]
    fn config_load_honors_explicit_path() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.toml");
        std::fs::write(&cfg_path, "version = 1\n").unwrap();

        let cfg = crate::config::load(Some(cfg_path.to_str().unwrap())).unwrap();
        assert_eq!(cfg.config_dir, tmp.path());
    }

    #[test]
    fn config_load_surfaces_parse_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.toml");
        std::fs::write(&cfg_path, "this is = not [valid] toml ===\n").unwrap();

        let result = crate::config::load(Some(cfg_path.to_str().unwrap()));
        assert!(
            result.is_err(),
            "malformed config must surface as Err, got: {result:?}"
        );
    }
}
