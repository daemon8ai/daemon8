// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::borrow::Borrow;
use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rmcp::schemars;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const SYSTEM_TAG: &str = "_system";
pub const TAG_PREFIX_PROJECT: &str = "project:";
pub const TAG_PREFIX_LANG: &str = "lang:";
pub const TAG_PREFIX_FRAMEWORK: &str = "framework:";
pub const TAG_PREFIX_TOOL: &str = "tool:";
pub const TAG_PREFIX_SOURCE: &str = "source:";
pub const OBSERVATION_SEARCH_TEXT_LIMIT_BYTES: usize = 16 * 1024;

macro_rules! arc_str_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Arc<str>);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(Arc::from(s))
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(Arc::from(s.as_str()))
            }
        }

        impl From<Arc<str>> for $name {
            fn from(s: Arc<str>) -> Self {
                Self(s)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.as_str() == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.as_str() == *other
            }
        }

        impl FromStr for $name {
            type Err = std::convert::Infallible;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self::from(s))
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                stringify!($name).into()
            }
            fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
                String::json_schema(generator)
            }
        }
    };
}

arc_str_newtype!(TabId);
arc_str_newtype!(SessionId);
arc_str_newtype!(DeviceSerial);
arc_str_newtype!(AppName);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DebugAction {
    EvalJs,
    Screenshot,
    InjectCss,
    RevertCss,
    ListTabs,
    GetPerfMetrics,
    GetDom,
    SetViewport,
    ClearViewport,
    NetworkConditions,
    Navigate,
    StorageClear,
    StorageInspect,
    StorageSet,
    ElementAtPoint,
    NewTab,
    CloseTab,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Severity {
    pub fn level(self) -> u8 {
        match self {
            Self::Trace => 0,
            Self::Debug => 1,
            Self::Info => 2,
            Self::Warn => 3,
            Self::Error => 4,
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for Severity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "trace" => Ok(Self::Trace),
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            other => Err(format!("unknown severity: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Origin {
    Application {
        name: AppName,
    },
    Browser {
        tab_id: TabId,
        url: String,
    },
    Device {
        serial: DeviceSerial,
        platform: DevicePlatform,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DevicePlatform {
    #[default]
    Android,
    Vega,
}

/// Enum discriminant without payload — use for filtering/matching without carrying variant data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationKindTag {
    Log,
    Query,
    HttpExchange,
    Exception,
    StateSnapshot,
    Metric,
    Custom,
    JsException,
    Lifecycle,
    ToolCall,
}

impl ObservationKindTag {
    pub fn is_dedup_exempt(self) -> bool {
        matches!(self, Self::ToolCall | Self::Metric | Self::Lifecycle)
    }
}

impl fmt::Display for ObservationKindTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Log => "log",
            Self::Query => "query",
            Self::HttpExchange => "http_exchange",
            Self::Exception => "exception",
            Self::StateSnapshot => "state_snapshot",
            Self::Metric => "metric",
            Self::Custom => "custom",
            Self::JsException => "js_exception",
            Self::Lifecycle => "lifecycle",
            Self::ToolCall => "tool_call",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for ObservationKindTag {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "log" => Ok(Self::Log),
            "query" => Ok(Self::Query),
            "http_exchange" => Ok(Self::HttpExchange),
            "exception" => Ok(Self::Exception),
            "state_snapshot" => Ok(Self::StateSnapshot),
            "metric" => Ok(Self::Metric),
            "custom" => Ok(Self::Custom),
            "js_exception" => Ok(Self::JsException),
            "lifecycle" => Ok(Self::Lifecycle),
            "tool_call" => Ok(Self::ToolCall),
            other => Err(format!("unknown observation kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ObservationKind {
    Log,
    Query {
        sql: String,
        duration_ms: f64,
    },
    HttpExchange {
        method: String,
        url: String,
        status: Option<u16>,
        duration_ms: Option<f64>,
    },
    Exception {
        message: String,
        trace: Option<String>,
    },
    StateSnapshot {
        label: String,
    },
    Metric {
        name: String,
        value: f64,
    },
    Custom {
        channel: String,
    },
    JsException {
        message: String,
        line: Option<u32>,
        column: Option<u32>,
    },
    Lifecycle {
        event_name: String,
        frame_id: String,
    },
    ToolCall {
        tool: String,
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        input: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<f64>,
    },
}

impl ObservationKind {
    pub fn tag(&self) -> ObservationKindTag {
        match self {
            Self::Log => ObservationKindTag::Log,
            Self::Query { .. } => ObservationKindTag::Query,
            Self::HttpExchange { .. } => ObservationKindTag::HttpExchange,
            Self::Exception { .. } => ObservationKindTag::Exception,
            Self::StateSnapshot { .. } => ObservationKindTag::StateSnapshot,
            Self::Metric { .. } => ObservationKindTag::Metric,
            Self::Custom { .. } => ObservationKindTag::Custom,
            Self::JsException { .. } => ObservationKindTag::JsException,
            Self::Lifecycle { .. } => ObservationKindTag::Lifecycle,
            Self::ToolCall { .. } => ObservationKindTag::ToolCall,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceLocation {
    pub file: String,
    pub line: u32,
    pub function: Option<String>,
}

/// Monotonic sequence number. "I've seen everything up to this ID."
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
pub struct Checkpoint(pub u64);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: u64,
    pub origin: Origin,
    pub kind: ObservationKind,
    pub data: serde_json::Value,
    pub severity: Severity,
    pub source_location: Option<SourceLocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_instance: Option<Arc<str>>,
    pub timestamp_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug_session_id: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_hash: Option<Arc<str>>,
}

impl Observation {
    /// Build a new observation. `id` starts at 0 (the store assigns the real
    /// monotonic sequence on insert) and `timestamp_ns` is captured from the
    /// system clock at call time.
    pub fn new(
        origin: Origin,
        kind: ObservationKind,
        data: serde_json::Value,
        severity: Severity,
        source_location: Option<SourceLocation>,
    ) -> Self {
        let timestamp_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before UNIX epoch")
            .as_nanos() as u64;

        Self {
            id: 0,
            origin,
            kind,
            data,
            severity,
            source_location,
            service: None,
            source: None,
            source_instance: None,
            timestamp_ns,
            correlation_id: None,
            parent_id: None,
            tags: None,
            session_id: None,
            node_id: None,
            debug_session_id: None,
            checkpoint_id: None,
            error_hash: None,
        }
    }
}

pub fn observation_origin_fields(origin: &Origin) -> (&'static str, String) {
    match origin {
        Origin::Application { name } => ("application", format!("app:{name}")),
        Origin::Browser { tab_id, .. } => ("browser", format!("browser:{tab_id}")),
        Origin::Device { serial, .. } => ("device", format!("device:{serial}")),
    }
}

pub fn observation_search_text(obs: &Observation) -> String {
    let (_, origin_key) = observation_origin_fields(&obs.origin);
    let mut text = String::new();

    push_search_part(&mut text, obs.severity.to_string());
    push_search_part(&mut text, obs.kind.tag().to_string());
    push_search_part(&mut text, origin_key);
    push_search_part(&mut text, origin_search_text(&obs.origin));
    if let Ok(kind) = serde_json::to_string(&obs.kind) {
        push_search_part(&mut text, kind);
    }

    if let Some(ref tags) = obs.tags {
        for tag in tags {
            push_search_part(&mut text, tag);
        }
    }

    if let Some(ref location) = obs.source_location {
        push_search_part(&mut text, &location.file);
        push_search_part(&mut text, location.line.to_string());
        if let Some(ref function) = location.function {
            push_search_part(&mut text, function);
        }
    }

    if let Some(ref service) = obs.service {
        push_search_part(&mut text, service);
    }
    if let Some(ref source) = obs.source {
        push_search_part(&mut text, source);
    }
    if let Some(ref source_instance) = obs.source_instance {
        push_search_part(&mut text, source_instance);
    }

    if let Some(ref correlation_id) = obs.correlation_id {
        push_search_part(&mut text, correlation_id);
    }
    if let Some(parent_id) = obs.parent_id {
        push_search_part(&mut text, parent_id.to_string());
    }
    if let Some(ref session_id) = obs.session_id {
        push_search_part(&mut text, session_id);
    }
    if let Some(ref node_id) = obs.node_id {
        push_search_part(&mut text, node_id);
    }
    if let Some(ref debug_session_id) = obs.debug_session_id {
        push_search_part(&mut text, debug_session_id);
    }
    if let Some(ref checkpoint_id) = obs.checkpoint_id {
        push_search_part(&mut text, checkpoint_id);
    }
    if let Some(ref error_hash) = obs.error_hash {
        push_search_part(&mut text, error_hash);
    }

    push_search_part(&mut text, obs.data.to_string());
    truncate_observation_search_text(text)
}

fn origin_search_text(origin: &Origin) -> String {
    match origin {
        Origin::Application { name } => format!("application {name}"),
        Origin::Browser { tab_id, url } => format!("browser {tab_id} {url}"),
        Origin::Device { serial, platform } => format!("device {serial} {platform:?}"),
    }
}

fn push_search_part(text: &mut String, part: impl AsRef<str>) {
    let part = part.as_ref();
    if part.is_empty() {
        return;
    }
    if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(part);
}

fn truncate_observation_search_text(mut text: String) -> String {
    if text.len() <= OBSERVATION_SEARCH_TEXT_LIMIT_BYTES {
        return text;
    }

    let mut end = OBSERVATION_SEARCH_TEXT_LIMIT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OriginPattern {
    AnyApplication,
    ApplicationNamed(AppName),
    AnyBrowser,
    BrowserTab(TabId),
    AnyDevice,
    DeviceSerial(DeviceSerial),
}

impl OriginPattern {
    pub fn parse(s: &str) -> Self {
        match s.split_once(':') {
            Some(("app", "*")) | None if s == "app" => Self::AnyApplication,
            Some(("app", name)) => Self::ApplicationNamed(name.into()),
            Some(("browser", "*")) | None if s == "browser" => Self::AnyBrowser,
            Some(("browser", tab_id)) => Self::BrowserTab(tab_id.into()),
            Some(("device", "*")) | None if s == "device" => Self::AnyDevice,
            Some(("device", serial)) => Self::DeviceSerial(serial.into()),
            _ => Self::ApplicationNamed(s.into()),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Filter {
    pub kinds: Option<Vec<ObservationKindTag>>,
    pub severity_min: Option<Severity>,
    pub origins: Option<Vec<OriginPattern>>,
    pub text_match: Option<String>,
    pub since: Option<Checkpoint>,
    pub limit: Option<usize>,
    pub correlation_id: Option<String>,
    pub tags: Option<Vec<String>>,
    pub service: Option<Vec<String>>,
    pub source: Option<Vec<String>>,
    pub source_instance: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_system: Option<bool>,
}

impl Filter {
    pub fn matches(&self, obs: &Observation) -> bool {
        if !self.include_system.unwrap_or(false)
            && let Some(ref tags) = obs.tags
            && tags.iter().any(|t| t == SYSTEM_TAG)
        {
            return false;
        }

        if let Some(ref cp) = self.since
            && obs.id <= cp.0
        {
            return false;
        }

        if let Some(min) = self.severity_min
            && obs.severity.level() < min.level()
        {
            return false;
        }

        if let Some(ref kinds) = self.kinds
            && !kinds.is_empty()
            && !kinds.contains(&obs.kind.tag())
        {
            return false;
        }

        if let Some(ref origins) = self.origins
            && !origins.is_empty()
            && !origins.iter().any(|p| p.matches(&obs.origin))
        {
            return false;
        }

        if let Some(ref text) = self.text_match {
            let text = text.trim();
            if !text.is_empty()
                && !observation_search_text(obs)
                    .to_ascii_lowercase()
                    .contains(&text.to_ascii_lowercase())
            {
                return false;
            }
        }

        if let Some(ref cid) = self.correlation_id {
            match &obs.correlation_id {
                Some(obs_cid) if obs_cid.as_ref() == cid.as_str() => {}
                _ => return false,
            }
        }

        if let Some(ref services) = self.service
            && !services.is_empty()
            && !matches_arc_str_filter(&obs.service, services)
        {
            return false;
        }

        if let Some(ref sources) = self.source
            && !sources.is_empty()
            && !matches_arc_str_filter(&obs.source, sources)
        {
            return false;
        }

        if let Some(ref source_instances) = self.source_instance
            && !source_instances.is_empty()
            && !matches_arc_str_filter(&obs.source_instance, source_instances)
        {
            return false;
        }

        if let Some(ref required_tags) = self.tags
            && !required_tags.is_empty()
        {
            match &obs.tags {
                Some(obs_tags) => {
                    if !required_tags.iter().all(|t| obs_tags.contains(t)) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        true
    }

    pub fn parse_severity(raw: &str) -> Option<Severity> {
        raw.trim().parse::<Severity>().ok()
    }

    pub fn parse_kinds(raw: &str) -> Vec<ObservationKindTag> {
        raw.split(',')
            .filter_map(|s| s.trim().parse::<ObservationKindTag>().ok())
            .collect()
    }

    pub fn parse_origins(raw: &str) -> Vec<OriginPattern> {
        raw.split(',')
            .map(|s| OriginPattern::parse(s.trim()))
            .collect()
    }

    pub fn parse_tags(raw: &str) -> Vec<String> {
        Self::parse_string_list(raw)
    }

    pub fn parse_string_list(raw: &str) -> Vec<String> {
        raw.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    pub fn kinds_from_vec(v: Vec<String>) -> Vec<ObservationKindTag> {
        v.into_iter()
            .filter_map(|s| s.trim().parse::<ObservationKindTag>().ok())
            .collect()
    }

    pub fn origins_from_vec(v: Vec<String>) -> Vec<OriginPattern> {
        v.into_iter()
            .map(|s| OriginPattern::parse(s.trim()))
            .collect()
    }
}

fn matches_arc_str_filter(value: &Option<Arc<str>>, allowed: &[String]) -> bool {
    match value {
        Some(value) => allowed.iter().any(|candidate| candidate == value.as_ref()),
        None => false,
    }
}

impl OriginPattern {
    pub fn matches(&self, origin: &Origin) -> bool {
        match (self, origin) {
            (Self::AnyApplication, Origin::Application { .. }) => true,
            (Self::ApplicationNamed(name), Origin::Application { name: n }) => n == name,
            (Self::AnyBrowser, Origin::Browser { .. }) => true,
            (Self::BrowserTab(tab), Origin::Browser { tab_id, .. }) => tab_id == tab,
            (Self::AnyDevice, Origin::Device { .. }) => true,
            (Self::DeviceSerial(serial), Origin::Device { serial: s, .. }) => s == serial,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Pattern,
    Decision,
    ErrorSignature,
    SessionSummary,
    UserFlagged,
}

impl fmt::Display for MemoryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Pattern => "pattern",
            Self::Decision => "decision",
            Self::ErrorSignature => "error_signature",
            Self::SessionSummary => "session_summary",
            Self::UserFlagged => "user_flagged",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for MemoryKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "pattern" => Ok(Self::Pattern),
            "decision" => Ok(Self::Decision),
            "error_signature" => Ok(Self::ErrorSignature),
            "session_summary" => Ok(Self::SessionSummary),
            "user_flagged" => Ok(Self::UserFlagged),
            other => Err(format!("unknown memory kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DebugSessionStatus {
    Active,
    Completed,
    Abandoned,
}

impl fmt::Display for DebugSessionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Abandoned => "abandoned",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for DebugSessionStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "active" => Ok(Self::Active),
            "completed" => Ok(Self::Completed),
            "abandoned" => Ok(Self::Abandoned),
            other => Err(format!("unknown debug session status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DebugSessionOutcome {
    Resolved,
    Abandoned,
    InProgress,
}

impl fmt::Display for DebugSessionOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Resolved => "resolved",
            Self::Abandoned => "abandoned",
            Self::InProgress => "in_progress",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for DebugSessionOutcome {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "resolved" => Ok(Self::Resolved),
            "abandoned" => Ok(Self::Abandoned),
            "in_progress" => Ok(Self::InProgress),
            other => Err(format!("unknown debug session outcome: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SliceSummary {
    pub total: usize,
    pub counts_by_kind: HashMap<String, usize>,
    pub counts_by_severity: HashMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSlice {
    pub observations: Vec<Observation>,
    pub checkpoint: Checkpoint,
    pub summary: SliceSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Ok,
    ErrorsDetected,
    NoSources,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    Application,
    Browser,
    Device,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub id: String,
    pub kind: ConnectionKind,
    pub name: String,
    pub observation_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSummary {
    pub observation_count: u64,
    pub error_count_last_60s: u64,
    pub active_channels: Vec<String>,
    pub connections: Vec<ConnectionInfo>,
    pub health: HealthStatus,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn obs(origin: Origin, kind: ObservationKind) -> Observation {
        Observation {
            id: 0,
            origin,
            kind,
            data: json!({"note": "roundtrip fixture"}),
            severity: Severity::Info,
            source_location: None,
            service: None,
            source: None,
            source_instance: None,
            timestamp_ns: 0,
            correlation_id: None,
            parent_id: None,
            tags: None,
            session_id: None,
            node_id: None,
            debug_session_id: None,
            checkpoint_id: None,
            error_hash: None,
        }
    }

    fn roundtrip_observation(o: &Observation) {
        let text = serde_json::to_string(o).expect("serialize");
        let back: Observation = serde_json::from_str(&text).expect("deserialize");
        let text_again = serde_json::to_string(&back).expect("reserialize");
        assert_eq!(
            text, text_again,
            "observation should roundtrip identically; first={text} second={text_again}"
        );
    }

    #[test]
    fn origin_application_roundtrip() {
        roundtrip_observation(&obs(
            Origin::Application {
                name: AppName::from("test-app"),
            },
            ObservationKind::Log,
        ));
    }

    #[test]
    fn origin_browser_roundtrip() {
        roundtrip_observation(&obs(
            Origin::Browser {
                tab_id: TabId::from("tab-abc-123"),
                url: "https://example.com/page".into(),
            },
            ObservationKind::Log,
        ));
    }

    #[test]
    fn origin_device_roundtrip() {
        roundtrip_observation(&obs(
            Origin::Device {
                serial: DeviceSerial::from("emulator-5554"),
                platform: DevicePlatform::Android,
            },
            ObservationKind::Log,
        ));
    }

    #[test]
    fn observation_kind_variants_all_roundtrip() {
        let cases = vec![
            ObservationKind::Log,
            ObservationKind::Query {
                sql: "SELECT 1".into(),
                duration_ms: 3.5,
            },
            ObservationKind::HttpExchange {
                method: "GET".into(),
                url: "/api/users".into(),
                status: Some(200),
                duration_ms: Some(42.0),
            },
            ObservationKind::Exception {
                message: "boom".into(),
                trace: Some("stack".into()),
            },
            ObservationKind::StateSnapshot {
                label: "before-action".into(),
            },
            ObservationKind::Metric {
                name: "cpu".into(),
                value: 0.75,
            },
            ObservationKind::Custom {
                channel: "events".into(),
            },
            ObservationKind::JsException {
                message: "undefined".into(),
                line: Some(12),
                column: Some(5),
            },
            ObservationKind::Lifecycle {
                event_name: "ready".into(),
                frame_id: "frame-1".into(),
            },
            ObservationKind::ToolCall {
                tool: "Bash".into(),
                input: json!({"command": "ls"}),
                output: Some(json!({"stdout": "Cargo.toml\n"})),
                exit_code: Some(0),
                duration_ms: Some(12.5),
            },
        ];

        for kind in cases {
            roundtrip_observation(&obs(Origin::Application { name: "x".into() }, kind));
        }
    }

    #[test]
    fn newtypes_serialize_transparent_as_strings() {
        let origin = Origin::Browser {
            tab_id: TabId::from("abc"),
            url: "https://x".into(),
        };
        let v: Value = serde_json::to_value(&origin).unwrap();
        assert_eq!(
            v["tab_id"],
            json!("abc"),
            "TabId must serialize as a bare string, got {v:#?}"
        );

        let origin = Origin::Device {
            serial: DeviceSerial::from("S123"),
            platform: DevicePlatform::Vega,
        };
        let v: Value = serde_json::to_value(&origin).unwrap();
        assert_eq!(v["serial"], json!("S123"));

        let origin = Origin::Application {
            name: AppName::from("my-app"),
        };
        let v: Value = serde_json::to_value(&origin).unwrap();
        assert_eq!(v["name"], json!("my-app"));
    }

    #[test]
    fn newtypes_deserialize_from_bare_strings() {
        let v = json!({
            "type": "browser",
            "tab_id": "tab-42",
            "url": "https://example.com"
        });
        let origin: Origin = serde_json::from_value(v).unwrap();
        match origin {
            Origin::Browser { tab_id, url } => {
                assert_eq!(tab_id.as_str(), "tab-42");
                assert_eq!(url, "https://example.com");
            }
            _ => panic!("expected Browser origin"),
        }
    }

    #[test]
    fn origin_pattern_roundtrip_all_variants() {
        let patterns = vec![
            OriginPattern::AnyApplication,
            OriginPattern::ApplicationNamed(AppName::from("svc")),
            OriginPattern::AnyBrowser,
            OriginPattern::BrowserTab(TabId::from("tab-9")),
            OriginPattern::AnyDevice,
            OriginPattern::DeviceSerial(DeviceSerial::from("S-42")),
        ];
        for p in patterns {
            let text = serde_json::to_string(&p).unwrap();
            let back: OriginPattern = serde_json::from_str(&text).unwrap();
            let text_again = serde_json::to_string(&back).unwrap();
            assert_eq!(text, text_again);
        }
    }

    #[test]
    fn filter_matches_honors_newtype_identity() {
        let matching = obs(
            Origin::Browser {
                tab_id: TabId::from("tab-1"),
                url: "".into(),
            },
            ObservationKind::Log,
        );
        let other = obs(
            Origin::Browser {
                tab_id: TabId::from("tab-2"),
                url: "".into(),
            },
            ObservationKind::Log,
        );
        let filter = Filter {
            origins: Some(vec![OriginPattern::BrowserTab(TabId::from("tab-1"))]),
            ..Filter::default()
        };
        assert!(filter.matches(&matching));
        assert!(!filter.matches(&other));
    }

    #[test]
    fn observation_search_text_includes_filter_metadata() {
        let mut observation = obs(
            Origin::Device {
                serial: DeviceSerial::from("ABC123"),
                platform: DevicePlatform::Vega,
            },
            ObservationKind::Exception {
                message: "surface failed".into(),
                trace: Some("stack".into()),
            },
        );
        observation.data = json!({"message": "HDMI overlay timeout"});
        observation.source_location = Some(SourceLocation {
            file: "src/device.rs".into(),
            line: 77,
            function: Some("render_overlay".into()),
        });
        observation.tags = Some(vec!["project:daemon8".into(), "domain:device".into()]);
        observation.service = Some(Arc::from("adb"));
        observation.source = Some(Arc::from("device.logs"));
        observation.source_instance = Some(Arc::from("ABC123/loggingctl"));
        observation.correlation_id = Some(Arc::from("corr-1"));
        observation.session_id = Some(Arc::from("session-1"));
        observation.node_id = Some(Arc::from("node-1"));

        let search_text = observation_search_text(&observation);

        assert!(search_text.contains("device:ABC123"));
        assert!(search_text.contains("project:daemon8"));
        assert!(search_text.contains("corr-1"));
        assert!(search_text.contains("adb"));
        assert!(search_text.contains("device.logs"));
        assert!(search_text.contains("ABC123/loggingctl"));
        assert!(search_text.contains("src/device.rs"));
        assert!(search_text.contains("surface failed"));
        assert!(search_text.len() <= OBSERVATION_SEARCH_TEXT_LIMIT_BYTES);
    }

    #[test]
    fn filter_matches_provenance_fields() {
        let mut matching = obs(
            Origin::Application {
                name: AppName::from("cargo"),
            },
            ObservationKind::Log,
        );
        matching.service = Some(Arc::from("cargo"));
        matching.source = Some(Arc::from("cargo.check"));
        matching.source_instance = Some(Arc::from("target/daemon8/cargo-check.log"));

        let mut other = matching.clone();
        other.service = Some(Arc::from("claude"));
        other.source = Some(Arc::from("claude.conversations"));
        other.source_instance = Some(Arc::from("session.jsonl"));

        let filter = Filter {
            service: Some(vec!["cargo".into()]),
            source: Some(vec!["cargo.check".into()]),
            source_instance: Some(vec!["target/daemon8/cargo-check.log".into()]),
            ..Filter::default()
        };
        assert!(filter.matches(&matching));
        assert!(!filter.matches(&other));
    }

    #[test]
    fn filter_text_match_searches_materialized_metadata() {
        let mut matching = obs(
            Origin::Application {
                name: AppName::from("daemon8-test"),
            },
            ObservationKind::Log,
        );
        matching.tags = Some(vec!["domain:device".into()]);
        matching.correlation_id = Some(Arc::from("corr-1"));

        let other = obs(
            Origin::Application {
                name: AppName::from("daemon8-test"),
            },
            ObservationKind::Log,
        );

        let tag_filter = Filter {
            text_match: Some("domain:device".into()),
            ..Filter::default()
        };
        assert!(tag_filter.matches(&matching));
        assert!(!tag_filter.matches(&other));

        let partial_filter = Filter {
            text_match: Some("dev".into()),
            ..Filter::default()
        };
        assert!(partial_filter.matches(&matching));
        assert!(!partial_filter.matches(&other));

        let origin_filter = Filter {
            text_match: Some("app:daemon8-test".into()),
            ..Filter::default()
        };
        assert!(origin_filter.matches(&matching));
        assert!(origin_filter.matches(&other));

        let blank_filter = Filter {
            text_match: Some("   ".into()),
            ..Filter::default()
        };
        assert!(blank_filter.matches(&other));
    }

    #[test]
    fn tab_id_equality_regardless_of_construction_path() {
        let from_str = TabId::from("abc");
        let from_string = TabId::from(String::from("abc"));
        let from_arc: TabId = std::sync::Arc::<str>::from("abc").into();
        assert_eq!(from_str, from_string);
        assert_eq!(from_string, from_arc);
        assert_eq!(from_str, "abc");
    }

    #[test]
    fn system_tag_excluded_by_default() {
        let mut system_obs = obs(
            Origin::Application {
                name: AppName::from("hook"),
            },
            ObservationKind::Log,
        );
        system_obs.tags = Some(vec![SYSTEM_TAG.to_string()]);

        let normal_obs = obs(
            Origin::Application {
                name: AppName::from("app"),
            },
            ObservationKind::Log,
        );

        let default_filter = Filter::default();
        assert!(!default_filter.matches(&system_obs));
        assert!(default_filter.matches(&normal_obs));

        let include_filter = Filter {
            include_system: Some(true),
            ..Filter::default()
        };
        assert!(include_filter.matches(&system_obs));
        assert!(include_filter.matches(&normal_obs));
    }

    #[test]
    fn debug_action_roundtrip_snake_case() {
        let cases = vec![
            (DebugAction::EvalJs, "eval_js"),
            (DebugAction::GetPerfMetrics, "get_perf_metrics"),
            (DebugAction::SetViewport, "set_viewport"),
            (DebugAction::StorageInspect, "storage_inspect"),
            (DebugAction::ElementAtPoint, "element_at_point"),
        ];
        for (variant, wire) in cases {
            let text = serde_json::to_string(&variant).unwrap();
            assert_eq!(text, format!("\"{wire}\""));
            let back: DebugAction = serde_json::from_str(&text).unwrap();
            assert_eq!(back, variant);
        }
    }

    #[test]
    fn dedup_exempt_kinds() {
        assert!(ObservationKindTag::ToolCall.is_dedup_exempt());
        assert!(ObservationKindTag::Metric.is_dedup_exempt());
        assert!(ObservationKindTag::Lifecycle.is_dedup_exempt());

        assert!(!ObservationKindTag::Log.is_dedup_exempt());
        assert!(!ObservationKindTag::Query.is_dedup_exempt());
        assert!(!ObservationKindTag::HttpExchange.is_dedup_exempt());
        assert!(!ObservationKindTag::Exception.is_dedup_exempt());
        assert!(!ObservationKindTag::JsException.is_dedup_exempt());
        assert!(!ObservationKindTag::StateSnapshot.is_dedup_exempt());
        assert!(!ObservationKindTag::Custom.is_dedup_exempt());
    }

    // T8: every public payload that crosses the MCP boundary must
    // serialize as snake_case JSON so the agent-facing contract stays
    // consistent with the rest of daemon8-types. Sniff-test the three
    // D1+D6 payloads by serializing a fully-populated instance and
    // checking the keys explicitly.
}
