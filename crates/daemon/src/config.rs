// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub browser: ChromeConfig,
    #[serde(default)]
    pub mcp: McpConfig,
    #[serde(default)]
    pub adb: AdbConfig,
    #[serde(default)]
    pub ingestion: IngestionConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub sources: BTreeMap<String, SourceConfig>,
    #[serde(default)]
    pub embeddings: daemon8_embed::EmbedConfig,
    #[serde(skip)]
    pub config_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_host")]
    pub host: IpAddr,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StorageConfig {
    #[serde(default, deserialize_with = "deser_optional_path")]
    pub path: Option<PathBuf>,
    #[serde(default, deserialize_with = "deser_optional_path")]
    pub screenshot_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChromeConfig {
    #[serde(default)]
    pub auto_connect: bool,
    #[serde(default = "default_chrome_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_reconnect_interval")]
    pub reconnect_interval_secs: u64,
    #[serde(default = "default_max_reconnect")]
    pub max_reconnect_interval_secs: u64,
    #[serde(default, deserialize_with = "deser_optional_path")]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default = "default_true")]
    pub stdio: bool,
    #[serde(default)]
    pub http: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IngestionConfig {
    #[serde(default)]
    pub udp: UdpConfig,
    #[serde(default)]
    pub unix: UnixConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UdpConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_udp_bind")]
    pub bind: SocketAddr,
    #[serde(default = "default_max_packet")]
    pub max_packet: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UnixConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deser_optional_path")]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdbConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_adb_server")]
    pub server_addr: SocketAddrV4,
    #[serde(default = "default_adb_scan_interval")]
    pub scan_interval_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default)]
    pub level: LogLevel,
    #[serde(default, deserialize_with = "deser_optional_path")]
    pub file: Option<PathBuf>,
    #[serde(default = "default_true")]
    pub stderr: bool,
    #[serde(default = "default_max_log_files")]
    pub max_log_files: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

impl FromStr for LogLevel {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "trace" => Ok(Self::Trace),
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            other => Err(format!(
                "unknown log level '{other}' (want trace|debug|info|warn|error)"
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SourceConfig {
    File(FileSourceConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSourceConfig {
    pub path: String,
    #[serde(default = "default_line_parser")]
    pub parser: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_line_parser() -> String {
    "line".into()
}

fn deser_optional_path<'de, D: Deserializer<'de>>(d: D) -> Result<Option<PathBuf>, D::Error> {
    let s: Option<String> = Option::deserialize(d)?;
    Ok(s.filter(|s| !s.is_empty()).map(PathBuf::from))
}

fn default_version() -> u32 {
    1
}
fn default_port() -> u16 {
    8888
}
fn default_host() -> IpAddr {
    IpAddr::from([127, 0, 0, 1])
}
fn default_chrome_endpoint() -> String {
    "http://localhost:9222".into()
}
fn default_reconnect_interval() -> u64 {
    5
}
fn default_max_reconnect() -> u64 {
    30
}
fn default_true() -> bool {
    true
}
fn default_udp_bind() -> SocketAddr {
    SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 8889)
}
fn default_max_packet() -> usize {
    65536
}
fn default_adb_server() -> SocketAddrV4 {
    SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 5037)
}
fn default_adb_scan_interval() -> u64 {
    10
}
fn default_max_log_files() -> usize {
    5
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: default_version(),
            server: ServerConfig::default(),
            storage: StorageConfig::default(),
            browser: ChromeConfig::default(),
            mcp: McpConfig::default(),
            adb: AdbConfig::default(),
            ingestion: IngestionConfig::default(),
            logging: LoggingConfig::default(),
            sources: BTreeMap::new(),
            embeddings: daemon8_embed::EmbedConfig::default(),
            config_dir: project_dirs()
                .map(|d| d.config_dir().to_path_buf())
                .unwrap_or_else(|| PathBuf::from(".")),
        }
    }
}

impl Default for AdbConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            server_addr: default_adb_server(),
            scan_interval_secs: default_adb_scan_interval(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::default(),
            file: None,
            stderr: default_true(),
            max_log_files: default_max_log_files(),
        }
    }
}

impl Default for UdpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: default_udp_bind(),
            max_packet: default_max_packet(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            host: default_host(),
        }
    }
}

impl Default for ChromeConfig {
    fn default() -> Self {
        Self {
            auto_connect: false,
            endpoint: default_chrome_endpoint(),
            reconnect_interval_secs: default_reconnect_interval(),
            max_reconnect_interval_secs: default_max_reconnect(),
            path: None,
        }
    }
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            stdio: default_true(),
            http: false,
        }
    }
}

#[allow(clippy::result_large_err)]
pub fn load(config_path: Option<&str>) -> Result<Config, figment::Error> {
    use figment::Figment;
    use figment::providers::{Env, Format, Serialized, Toml};

    let mut figment = Figment::from(Serialized::defaults(Config::default()));

    let config_file = config_path.map(PathBuf::from).unwrap_or_else(|| {
        project_dirs()
            .map(|d| d.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("config.toml"))
    });

    if config_file.exists() {
        tracing::debug!(?config_file, "loading config file");
        figment = figment.merge(Toml::file(&config_file));
    }

    // Environment variables use double-underscore for nesting:
    // DAEMON8_SERVER__PORT=9090, DAEMON8_CHROME__AUTO_CONNECT=true
    figment = figment.merge(Env::prefixed("DAEMON8_").split("__"));

    let mut cfg: Config = figment.extract()?;

    cfg.config_dir = if config_file.exists() {
        config_file
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(config_dir)
    } else {
        config_dir()
    };

    Ok(cfg)
}

/// Returns the platform `ProjectDirs` instance for daemon8, keyed on build profile.
/// Debug builds use the `daemon8-dev` app slug to isolate test/dev data.
fn project_dirs() -> Option<directories::ProjectDirs> {
    let app = if cfg!(debug_assertions) {
        "daemon8-dev"
    } else {
        "daemon8"
    };
    directories::ProjectDirs::from("dev", "daemon8", app)
}

pub fn resolve_db_path(config_path: Option<&std::path::Path>) -> PathBuf {
    if let Some(p) = config_path {
        return p.to_path_buf();
    }
    project_dirs()
        .map(|d| d.data_dir().join("store"))
        .unwrap_or_else(|| PathBuf::from("store"))
}

pub fn resolve_log_dir(config_path: Option<&std::path::Path>) -> PathBuf {
    if let Some(p) = config_path {
        return p.to_path_buf();
    }
    project_dirs()
        .map(|d| d.data_dir().join("logs"))
        .unwrap_or_else(|| PathBuf::from("logs"))
}

pub fn config_dir() -> PathBuf {
    project_dirs()
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn resolve_screenshot_path(config: &Config) -> PathBuf {
    let dir = if let Some(explicit) = config.storage.screenshot_path.as_ref() {
        expand_tilde(explicit)
    } else if let Some(db) = config.storage.path.as_ref() {
        db.parent()
            .map(|p| p.join("screenshots"))
            .unwrap_or_else(|| PathBuf::from("screenshots"))
    } else {
        project_dirs()
            .map(|d| d.data_dir().join("screenshots"))
            .unwrap_or_else(|| PathBuf::from("screenshots"))
    };

    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(path = %dir.display(), "failed to create screenshot directory: {e}");
    }

    dir
}

fn expand_tilde(path: &std::path::Path) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(rest) = raw.strip_prefix('~')
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest.strip_prefix('/').unwrap_or(rest));
    }
    path.to_path_buf()
}

pub fn resolve_unix_socket_path(config_path: Option<&std::path::Path>) -> PathBuf {
    config_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/tmp/daemon8.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_db_path_explicit() {
        let p = resolve_db_path(Some(std::path::Path::new("/tmp/test.db")));
        assert_eq!(p, PathBuf::from("/tmp/test.db"));
    }

    #[test]
    fn resolve_db_path_default() {
        let p = resolve_db_path(None);
        assert!(p.to_string_lossy().contains("store"));
    }

    #[test]
    fn resolve_db_path_default_uses_profile_tagged_slug() {
        let p = resolve_db_path(None);
        let s = p.to_string_lossy();
        #[cfg(debug_assertions)]
        assert!(
            s.contains("daemon8-dev"),
            "debug build should use daemon8-dev slug, got {s}"
        );
        #[cfg(not(debug_assertions))]
        assert!(
            s.contains("daemon8") && !s.contains("daemon8-dev"),
            "release build should use daemon8 slug (no -dev suffix), got {s}"
        );
        assert!(s.contains("store"));
    }

    #[test]
    fn default_config_is_sane() {
        let cfg = Config::default();
        assert_eq!(cfg.server.port, 8888);
        assert_eq!(cfg.server.host, IpAddr::from([127, 0, 0, 1]));
        assert!(cfg.mcp.stdio);
        assert!(!cfg.mcp.http);
        assert_eq!(cfg.logging.level, LogLevel::Info);
        assert!(cfg.storage.path.is_none());
        assert!(cfg.browser.path.is_none());
    }

    #[test]
    fn empty_string_path_deserializes_to_none() {
        let toml_str = r#"
            [storage]
            path = ""
            screenshot_path = ""
            [browser]
            path = ""
            [logging]
            file = ""
            [ingestion.unix]
            path = ""
        "#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert!(cfg.storage.path.is_none());
        assert!(cfg.storage.screenshot_path.is_none());
        assert!(cfg.browser.path.is_none());
        assert!(cfg.logging.file.is_none());
        assert!(cfg.ingestion.unix.path.is_none());
    }

    #[test]
    fn non_empty_path_deserializes_to_some() {
        let cfg: Config = toml::from_str(
            r#"[storage]
path = "/var/lib/daemon8/obs.db"
"#,
        )
        .unwrap();
        assert_eq!(
            cfg.storage.path,
            Some(PathBuf::from("/var/lib/daemon8/obs.db"))
        );
    }

    #[test]
    fn ipaddr_host_parses_from_string() {
        let cfg: Config = toml::from_str(
            r#"[server]
host = "0.0.0.0"
port = 8888
"#,
        )
        .unwrap();
        assert_eq!(cfg.server.host, IpAddr::from([0, 0, 0, 0]));
    }

    #[test]
    fn ipaddr_host_rejects_garbage() {
        let result: Result<Config, _> = toml::from_str(
            r#"[server]
host = "not-an-ip"
port = 8888
"#,
        );
        assert!(
            result.is_err(),
            "expected deserialize to reject garbage host"
        );
    }

    #[test]
    fn log_level_deserializes_case_insensitive_lowercase() {
        let cfg: Config = toml::from_str(
            r#"[logging]
level = "warn"
"#,
        )
        .unwrap();
        assert_eq!(cfg.logging.level, LogLevel::Warn);
    }

    #[test]
    fn log_level_rejects_unknown() {
        let result: Result<Config, _> = toml::from_str(
            r#"[logging]
level = "garbage"
"#,
        );
        assert!(
            result.is_err(),
            "expected deserialize to reject unknown log level"
        );
    }

    #[test]
    fn adb_server_addr_parses_valid_socket() {
        let cfg: Config = toml::from_str(
            r#"[adb]
server_addr = "192.168.1.10:5555"
"#,
        )
        .unwrap();
        assert_eq!(
            cfg.adb.server_addr,
            SocketAddrV4::new(std::net::Ipv4Addr::new(192, 168, 1, 10), 5555)
        );
    }

    #[test]
    fn adb_server_addr_rejects_hostname() {
        let result: Result<Config, _> = toml::from_str(
            r#"[adb]
server_addr = "localhost:5037"
"#,
        );
        assert!(
            result.is_err(),
            "SocketAddrV4 should reject hostnames, got {result:?}"
        );
    }

    #[test]
    fn udp_bind_parses_valid_socket() {
        let cfg: Config = toml::from_str(
            r#"[ingestion.udp]
bind = "0.0.0.0:8889"
"#,
        )
        .unwrap();
        assert_eq!(cfg.ingestion.udp.bind.port(), 8889);
        assert!(cfg.ingestion.udp.bind.ip().is_unspecified());
    }

    #[test]
    fn sources_default_is_empty() {
        let cfg = Config::default();
        assert!(cfg.sources.is_empty());
    }

    #[test]
    fn sources_file_type_parses() {
        let cfg: Config = toml::from_str(
            r#"
[sources.laravel]
type = "file"
path = "/var/log/laravel/*.log"
parser = "monolog"
tags = ["php", "laravel"]

[sources.nginx]
type = "file"
path = "/var/log/nginx/access.log"
parser = "clf"
"#,
        )
        .unwrap();
        assert_eq!(cfg.sources.len(), 2);
        match &cfg.sources["laravel"] {
            SourceConfig::File(f) => {
                assert_eq!(f.path, "/var/log/laravel/*.log");
                assert_eq!(f.parser, "monolog");
                assert_eq!(f.tags, vec!["php", "laravel"]);
            }
        }
        match &cfg.sources["nginx"] {
            SourceConfig::File(f) => {
                assert_eq!(f.parser, "clf");
                assert!(f.tags.is_empty());
            }
        }
    }

    #[test]
    fn sources_file_defaults_parser_to_line() {
        let cfg: Config = toml::from_str(
            r#"
[sources.raw]
type = "file"
path = "/tmp/test.log"
"#,
        )
        .unwrap();
        match &cfg.sources["raw"] {
            SourceConfig::File(f) => {
                assert_eq!(f.parser, "line");
            }
        }
    }
}
