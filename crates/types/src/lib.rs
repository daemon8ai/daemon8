// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rmcp::schemars;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const SYSTEM_TAG: &str = "_system";
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

pub trait SourceActivator: Send + Sync {
    fn touch_matching(&self, filter: &Filter);
}

/// Out-of-band signals into the active discovery scanner (D3).
///
/// The scanner's wait loop watches these flags between poll ticks. The
/// implementation lives in the daemon binary; this trait gives the HTTP
/// API layer a way to flip the bits without importing daemon-internal
/// types. `--complete` shortcuts the wait loop with whatever templates
/// the librarian currently has; `--skip` writes the per-project skip
/// marker and returns the unchanged plan.
pub trait DiscoveryControl: Send + Sync {
    fn signal_complete(&self);
    fn signal_skip(&self);
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

// ── Awareness Tree enums ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AwarenessNodeKind {
    Objective,
    Question,
    Hypothesis,
    Fact,
    Decision,
    Constraint,
    Risk,
    Blocker,
}

impl fmt::Display for AwarenessNodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Objective => "objective",
            Self::Question => "question",
            Self::Hypothesis => "hypothesis",
            Self::Fact => "fact",
            Self::Decision => "decision",
            Self::Constraint => "constraint",
            Self::Risk => "risk",
            Self::Blocker => "blocker",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for AwarenessNodeKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "objective" => Ok(Self::Objective),
            "question" => Ok(Self::Question),
            "hypothesis" => Ok(Self::Hypothesis),
            "fact" => Ok(Self::Fact),
            "decision" => Ok(Self::Decision),
            "constraint" => Ok(Self::Constraint),
            "risk" => Ok(Self::Risk),
            "blocker" => Ok(Self::Blocker),
            other => Err(format!("unknown awareness node kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AwarenessAuthority {
    Verified,
    Accepted,
    Inferred,
    Hypothesis,
    Question,
}

impl fmt::Display for AwarenessAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Verified => "verified",
            Self::Accepted => "accepted",
            Self::Inferred => "inferred",
            Self::Hypothesis => "hypothesis",
            Self::Question => "question",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for AwarenessAuthority {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "verified" => Ok(Self::Verified),
            "accepted" => Ok(Self::Accepted),
            "inferred" => Ok(Self::Inferred),
            "hypothesis" => Ok(Self::Hypothesis),
            "question" => Ok(Self::Question),
            other => Err(format!("unknown awareness authority: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AwarenessNodeState {
    Active,
    Resolved,
    Retired,
    Stale,
    Conflicted,
}

impl fmt::Display for AwarenessNodeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Active => "active",
            Self::Resolved => "resolved",
            Self::Retired => "retired",
            Self::Stale => "stale",
            Self::Conflicted => "conflicted",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for AwarenessNodeState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "active" => Ok(Self::Active),
            "resolved" => Ok(Self::Resolved),
            "retired" => Ok(Self::Retired),
            "stale" => Ok(Self::Stale),
            "conflicted" => Ok(Self::Conflicted),
            other => Err(format!("unknown awareness node state: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AwarenessEdgeKind {
    Supports,
    Contradicts,
    Answers,
    DerivedFrom,
    Supersedes,
    AppliesTo,
}

impl fmt::Display for AwarenessEdgeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Supports => "supports",
            Self::Contradicts => "contradicts",
            Self::Answers => "answers",
            Self::DerivedFrom => "derived_from",
            Self::Supersedes => "supersedes",
            Self::AppliesTo => "applies_to",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for AwarenessEdgeKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "supports" => Ok(Self::Supports),
            "contradicts" => Ok(Self::Contradicts),
            "answers" => Ok(Self::Answers),
            "derived_from" => Ok(Self::DerivedFrom),
            "supersedes" => Ok(Self::Supersedes),
            "applies_to" => Ok(Self::AppliesTo),
            other => Err(format!("unknown awareness edge kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AwarenessOperation {
    Capture,
    Update,
    Question,
    Resolve,
    Verify,
    Retire,
}

impl fmt::Display for AwarenessOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Capture => "capture",
            Self::Update => "update",
            Self::Question => "question",
            Self::Resolve => "resolve",
            Self::Verify => "verify",
            Self::Retire => "retire",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for AwarenessOperation {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "capture" => Ok(Self::Capture),
            "update" => Ok(Self::Update),
            "question" => Ok(Self::Question),
            "resolve" => Ok(Self::Resolve),
            "verify" => Ok(Self::Verify),
            "retire" => Ok(Self::Retire),
            other => Err(format!("unknown awareness operation: {other}")),
        }
    }
}

// ── Librarian catalog enums ──────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LibrarianNodeKind {
    Doc,
    SourceTemplate,
    Fix,
    Project,
    // Concrete resolved source for a single project: the result of
    // expanding a source_template's locator_pattern against this
    // machine's filesystem (D4). One source_instance per resolved path;
    // linked to its template via `derived_from` and to its project via
    // `has_source`.
    SourceInstance,
}

impl fmt::Display for LibrarianNodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Doc => "doc",
            Self::SourceTemplate => "source_template",
            Self::Fix => "fix",
            Self::Project => "project",
            Self::SourceInstance => "source_instance",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for LibrarianNodeKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "doc" => Ok(Self::Doc),
            "source_template" => Ok(Self::SourceTemplate),
            "fix" => Ok(Self::Fix),
            "project" => Ok(Self::Project),
            "source_instance" => Ok(Self::SourceInstance),
            other => Err(format!("unknown librarian node kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LibrarianEdgeKind {
    HasSource,
    DocumentedBy,
    Fixes,
    Supersedes,
    ChildOf,
    // Links a source_instance back to the source_template it was
    // resolved from. None when a source_instance was created from an
    // explicit user override rather than a learned template.
    DerivedFrom,
}

impl fmt::Display for LibrarianEdgeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::HasSource => "has_source",
            Self::DocumentedBy => "documented_by",
            Self::Fixes => "fixes",
            Self::Supersedes => "supersedes",
            Self::ChildOf => "child_of",
            Self::DerivedFrom => "derived_from",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for LibrarianEdgeKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "has_source" => Ok(Self::HasSource),
            "documented_by" => Ok(Self::DocumentedBy),
            "fixes" => Ok(Self::Fixes),
            "supersedes" => Ok(Self::Supersedes),
            "child_of" => Ok(Self::ChildOf),
            "derived_from" => Ok(Self::DerivedFrom),
            other => Err(format!("unknown librarian edge kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LocatorKind {
    File,
    Url,
    Vault,
}

impl fmt::Display for LocatorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::File => "file",
            Self::Url => "url",
            Self::Vault => "vault",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for LocatorKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "file" => Ok(Self::File),
            "url" => Ok(Self::Url),
            "vault" => Ok(Self::Vault),
            other => Err(format!("unknown locator kind: {other}")),
        }
    }
}

// ── Project-aware onboarding types (D1 + D6 + D11) ───────────────────
//
// Host platform tag. Carried by ProjectClassification (D1) and the
// platforms array on source_template entries (D6). Kept narrow on
// purpose; OS variants beyond these three get added when we have a
// concrete user need.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Macos,
    Linux,
    Windows,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Macos => "macos",
            Self::Linux => "linux",
            Self::Windows => "windows",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for Platform {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "macos" | "darwin" | "osx" => Ok(Self::Macos),
            "linux" => Ok(Self::Linux),
            "windows" | "win" => Ok(Self::Windows),
            other => Err(format!("unknown platform: {other}")),
        }
    }
}

// What kind of runtime data a source_template points to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Log,
    Config,
    Conversation,
    Cache,
    Crash,
    Build,
    Db,
    Metric,
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Log => "log",
            Self::Config => "config",
            Self::Conversation => "conversation",
            Self::Cache => "cache",
            Self::Crash => "crash",
            Self::Build => "build",
            Self::Db => "db",
            Self::Metric => "metric",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for SourceKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "log" => Ok(Self::Log),
            "config" => Ok(Self::Config),
            "conversation" => Ok(Self::Conversation),
            "cache" => Ok(Self::Cache),
            "crash" => Ok(Self::Crash),
            "build" => Ok(Self::Build),
            "db" => Ok(Self::Db),
            "metric" => Ok(Self::Metric),
            other => Err(format!("unknown source kind: {other}")),
        }
    }
}

// Provenance of a source_template entry. Lets the doctor flow (D8)
// distinguish "agent found this" from "user wrote this in TOML" from
// "we've seen this template fail and marked it suspect."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TemplateConfidence {
    AgentDiscovered,
    UserProvided,
    Drifted,
}

impl fmt::Display for TemplateConfidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::AgentDiscovered => "agent_discovered",
            Self::UserProvided => "user_provided",
            Self::Drifted => "drifted",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for TemplateConfidence {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "agent_discovered" => Ok(Self::AgentDiscovered),
            "user_provided" => Ok(Self::UserProvided),
            "drifted" => Ok(Self::Drifted),
            other => Err(format!("unknown template confidence: {other}")),
        }
    }
}

// Output of crates/providers/src/project_type.rs::classify.
//
// Multi-tag by design: a single repo can be react-native + vega +
// git-repo. The librarian's source_template lookups use these tags
// with OR semantics (template applies if ANY tag matches).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ProjectClassification {
    pub tags: Vec<String>,
    pub framework_versions: BTreeMap<String, String>,
    pub root: PathBuf,
    pub manifests: BTreeMap<String, PathBuf>,
    pub platform: Platform,
}

// Payload stored on a LibrarianNode with kind = source_template.
// Validated at write time by crates/store/src/librarian_validators.rs.
//
// Cross-cutting fields (kind, locator, tags) live on LibrarianNode
// itself for SurrealDB indexability; the per-kind fields live here
// in `data` so the schema stays flexible.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct SourceTemplateData {
    pub project_types: Vec<String>,
    pub kind: SourceKind,
    pub locator_pattern: String,
    pub platforms: Vec<Platform>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parser_hint: Option<String>,
    pub default_tags: Vec<String>,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_constraint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovered_by_session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovered_by_provider: Option<String>,
    pub discovered_at_ns: u64,
    pub verified_count: u32,
    pub last_verified_at_ns: u64,
    pub confidence: TemplateConfidence,
}

// Payload stored on a LibrarianNode with kind = project.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ProjectNodeData {
    pub root_path: PathBuf,
    pub slug: String,
    pub classification_tags: Vec<String>,
    pub framework_versions: BTreeMap<String, String>,
    pub platform: Platform,
    pub created_at_ns: u64,
    pub last_serve_at_ns: u64,
    pub skip_discovery: bool,
}

// Payload stored on a LibrarianNode with kind = source_instance.
// Written by the D4 confirmation path: one per resolved_path that the
// scanner produced, linked to the originating project via `has_source`
// and (when applicable) back to its source_template via `derived_from`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct SourceInstanceData {
    pub kind: SourceKind,
    pub resolved_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parser: Option<String>,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_constraint: Option<String>,
    pub registered_at_ns: u64,
    pub last_verified_at_ns: u64,
}

// Payload carried in the `data` field of a discovery hint observation
// (`ObservationKind::Custom { channel: "discovery_hint" }`). The hint is
// emitted when daemon8 classifies a project but finds no `source_template`
// entries in the librarian covering the project's tags. The agent reads
// the hint via `query_observations` and responds by calling
// `librarian_index` with one or more source_template nodes.
//
// `known_project_type_tags_ref` is a copy of the validator allowlist at
// emission time so the agent knows exactly which tags will pass write
// validation. Duplicated per-hint on purpose: the hint payload is the
// agent's contract, and an in-flight hint must remain interpretable
// even if the allowlist grows later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct DiscoveryHintPayload {
    pub project_root: PathBuf,
    pub classification_tags: Vec<String>,
    pub framework_versions: BTreeMap<String, String>,
    pub platform: Platform,
    pub known_templates_matched: u32,
    pub missing_for_tags: Vec<String>,
    pub known_project_type_tags_ref: Vec<String>,
    pub instruction_text: String,
    /// True when this hint carries at least one first-run provider entry
    /// — i.e. some AI provider on this machine has no conversation
    /// `source_template` yet and the agent is being asked to register one.
    /// Computed from `first_run_providers.is_some()`; serialized only when
    /// true so older hints round-trip identically.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_run: Option<bool>,
    /// Per-provider first-run payloads. Populated by D5 when the
    /// librarian has no conversation `source_template` for one or more
    /// providers detectable on this machine. Each entry carries the
    /// provider's filesystem layout (from the `AiProvider` trait) so
    /// the agent can write a correctly-shaped template without probing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_run_providers: Option<Vec<FirstRunPayload>>,
    pub emitted_at_ns: u64,
}

// Per-provider conversation template bootstrap payload. Attached to a
// `DiscoveryHintPayload` when daemon8 has never seen a conversation
// `source_template` for this provider on this machine. The agent reads
// the filesystem-layout hints and writes a `source_template` whose
// `locator_pattern` is derived from `conversation_dir_hint` +
// `conversation_file_glob_hint`. Subsequent projects on this machine
// reuse that template instead of re-asking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct FirstRunPayload {
    pub provider_id: String,
    pub provider_label: String,
    pub conversation_dir_hint: String,
    pub conversation_file_glob_hint: String,
    pub session_id_env_vars: Vec<String>,
    pub instruction_text: String,
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
                label: "before-migration".into(),
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
        observation.correlation_id = Some(Arc::from("corr-1"));
        observation.session_id = Some(Arc::from("session-1"));
        observation.node_id = Some(Arc::from("node-1"));

        let search_text = observation_search_text(&observation);

        assert!(search_text.contains("device:ABC123"));
        assert!(search_text.contains("project:daemon8"));
        assert!(search_text.contains("corr-1"));
        assert!(search_text.contains("src/device.rs"));
        assert!(search_text.contains("surface failed"));
        assert!(search_text.len() <= OBSERVATION_SEARCH_TEXT_LIMIT_BYTES);
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
    fn librarian_node_kind_roundtrip() {
        for (variant, wire) in [
            (LibrarianNodeKind::Doc, "doc"),
            (LibrarianNodeKind::SourceTemplate, "source_template"),
            (LibrarianNodeKind::Fix, "fix"),
            (LibrarianNodeKind::Project, "project"),
            (LibrarianNodeKind::SourceInstance, "source_instance"),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, format!("\"{wire}\""));
            let back: LibrarianNodeKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant);
            assert_eq!(variant.to_string(), wire);
            assert_eq!(wire.parse::<LibrarianNodeKind>().unwrap(), variant);
        }
    }

    #[test]
    fn librarian_edge_kind_roundtrip() {
        for (variant, wire) in [
            (LibrarianEdgeKind::HasSource, "has_source"),
            (LibrarianEdgeKind::DocumentedBy, "documented_by"),
            (LibrarianEdgeKind::Fixes, "fixes"),
            (LibrarianEdgeKind::Supersedes, "supersedes"),
            (LibrarianEdgeKind::ChildOf, "child_of"),
            (LibrarianEdgeKind::DerivedFrom, "derived_from"),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, format!("\"{wire}\""));
            let back: LibrarianEdgeKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant);
            assert_eq!(variant.to_string(), wire);
            assert_eq!(wire.parse::<LibrarianEdgeKind>().unwrap(), variant);
        }
    }

    #[test]
    fn locator_kind_roundtrip() {
        for (variant, wire) in [
            (LocatorKind::File, "file"),
            (LocatorKind::Url, "url"),
            (LocatorKind::Vault, "vault"),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, format!("\"{wire}\""));
            let back: LocatorKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant);
            assert_eq!(variant.to_string(), wire);
            assert_eq!(wire.parse::<LocatorKind>().unwrap(), variant);
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
    #[test]
    fn librarian_payload_schema_serializes_snake_case() {
        let classification = ProjectClassification {
            tags: vec!["react-native".into()],
            framework_versions: BTreeMap::new(),
            root: PathBuf::from("/tmp/x"),
            manifests: BTreeMap::new(),
            platform: Platform::Macos,
        };
        let v = serde_json::to_value(&classification).unwrap();
        let obj = v.as_object().unwrap();
        for key in [
            "tags",
            "framework_versions",
            "root",
            "manifests",
            "platform",
        ] {
            assert!(
                obj.contains_key(key),
                "ProjectClassification missing snake_case key {key}: {v}"
            );
        }
        for key in obj.keys() {
            assert!(
                !key.chars().any(|c| c.is_ascii_uppercase()),
                "ProjectClassification has non-snake_case key {key}"
            );
        }

        let template = SourceTemplateData {
            project_types: vec!["react-native".into()],
            kind: SourceKind::Log,
            locator_pattern: "~/x".into(),
            platforms: vec![Platform::Macos],
            parser_hint: Some("grok".into()),
            default_tags: vec![],
            description: "x".into(),
            version_constraint: Some(">=1".into()),
            discovered_by_session: Some("s".into()),
            discovered_by_provider: Some("claude".into()),
            discovered_at_ns: 1,
            verified_count: 0,
            last_verified_at_ns: 2,
            confidence: TemplateConfidence::AgentDiscovered,
        };
        let v = serde_json::to_value(&template).unwrap();
        let obj = v.as_object().unwrap();
        for key in [
            "project_types",
            "kind",
            "locator_pattern",
            "platforms",
            "parser_hint",
            "default_tags",
            "description",
            "version_constraint",
            "discovered_by_session",
            "discovered_by_provider",
            "discovered_at_ns",
            "verified_count",
            "last_verified_at_ns",
            "confidence",
        ] {
            assert!(
                obj.contains_key(key),
                "SourceTemplateData missing snake_case key {key}: {v}"
            );
        }
        // confidence variant must also be snake_case.
        assert_eq!(obj["confidence"], serde_json::json!("agent_discovered"));

        let project = ProjectNodeData {
            root_path: PathBuf::from("/tmp/x"),
            slug: "x".into(),
            classification_tags: vec!["rust".into()],
            framework_versions: BTreeMap::new(),
            platform: Platform::Linux,
            created_at_ns: 1,
            last_serve_at_ns: 2,
            skip_discovery: false,
        };
        let v = serde_json::to_value(&project).unwrap();
        let obj = v.as_object().unwrap();
        for key in [
            "root_path",
            "slug",
            "classification_tags",
            "framework_versions",
            "platform",
            "created_at_ns",
            "last_serve_at_ns",
            "skip_discovery",
        ] {
            assert!(
                obj.contains_key(key),
                "ProjectNodeData missing snake_case key {key}: {v}"
            );
        }
    }

    // D2: the discovery hint payload is the agent's contract for the
    // `discovery_hint` channel. Round-trip + key shape are part of the
    // public surface; lock both with one explicit test.
    #[test]
    fn discovery_hint_payload_round_trip_and_snake_case() {
        let mut versions = BTreeMap::new();
        versions.insert("react-native".into(), "0.74.5".into());

        let payload = DiscoveryHintPayload {
            project_root: PathBuf::from("/tmp/rtntv_vega"),
            classification_tags: vec!["react-native".into(), "vega".into()],
            framework_versions: versions,
            platform: Platform::Macos,
            known_templates_matched: 0,
            missing_for_tags: vec!["react-native".into(), "vega".into()],
            known_project_type_tags_ref: vec!["any".into(), "react-native".into()],
            instruction_text: "find runtime data for these tags".into(),
            first_run: None,
            first_run_providers: None,
            emitted_at_ns: 1_700_000_000_000_000_000,
        };

        let v = serde_json::to_value(&payload).unwrap();
        let obj = v.as_object().unwrap();
        for key in [
            "project_root",
            "classification_tags",
            "framework_versions",
            "platform",
            "known_templates_matched",
            "missing_for_tags",
            "known_project_type_tags_ref",
            "instruction_text",
            "emitted_at_ns",
        ] {
            assert!(
                obj.contains_key(key),
                "DiscoveryHintPayload missing snake_case key {key}: {v}"
            );
        }
        for key in obj.keys() {
            assert!(
                !key.chars().any(|c| c.is_ascii_uppercase()),
                "DiscoveryHintPayload has non-snake_case key {key}"
            );
        }

        assert!(
            !obj.contains_key("first_run"),
            "first_run=None should be skipped: {v}"
        );
        assert!(
            !obj.contains_key("first_run_providers"),
            "first_run_providers=None should be skipped: {v}"
        );

        let round_tripped: DiscoveryHintPayload = serde_json::from_value(v).unwrap();
        assert_eq!(round_tripped, payload);
    }

    // D5: FirstRunPayload is the agent's contract for the per-provider
    // conversation-bootstrap branch. Lock the shape and the round-trip.
    #[test]
    fn first_run_payload_round_trip_and_snake_case() {
        let payload = FirstRunPayload {
            provider_id: "claude".into(),
            provider_label: "Claude Code".into(),
            conversation_dir_hint: "~/.claude/projects".into(),
            conversation_file_glob_hint: "**/*.jsonl".into(),
            session_id_env_vars: vec!["CLAUDE_SESSION_ID".into()],
            instruction_text: "write a conversation source_template...".into(),
        };
        let v = serde_json::to_value(&payload).unwrap();
        let obj = v.as_object().unwrap();
        for key in [
            "provider_id",
            "provider_label",
            "conversation_dir_hint",
            "conversation_file_glob_hint",
            "session_id_env_vars",
            "instruction_text",
        ] {
            assert!(
                obj.contains_key(key),
                "FirstRunPayload missing snake_case key {key}: {v}"
            );
        }
        for key in obj.keys() {
            assert!(
                !key.chars().any(|c| c.is_ascii_uppercase()),
                "FirstRunPayload has non-snake_case key {key}"
            );
        }
        let round_tripped: FirstRunPayload = serde_json::from_value(v).unwrap();
        assert_eq!(round_tripped, payload);
    }

    // When first_run_providers is populated, both `first_run` (bool) and
    // `first_run_providers` (array) must appear in the serialized form so
    // agents can branch on either signal.
    #[test]
    fn discovery_hint_payload_emits_first_run_fields_when_populated() {
        let mut versions = BTreeMap::new();
        versions.insert("react-native".into(), "0.74.5".into());
        let fr = FirstRunPayload {
            provider_id: "claude".into(),
            provider_label: "Claude Code".into(),
            conversation_dir_hint: "~/.claude/projects".into(),
            conversation_file_glob_hint: "**/*.jsonl".into(),
            session_id_env_vars: vec!["CLAUDE_SESSION_ID".into()],
            instruction_text: "...".into(),
        };
        let payload = DiscoveryHintPayload {
            project_root: PathBuf::from("/tmp/proj"),
            classification_tags: vec!["react-native".into()],
            framework_versions: versions,
            platform: Platform::Macos,
            known_templates_matched: 0,
            missing_for_tags: vec!["react-native".into()],
            known_project_type_tags_ref: vec!["react-native".into()],
            instruction_text: "...".into(),
            first_run: Some(true),
            first_run_providers: Some(vec![fr]),
            emitted_at_ns: 1_700_000_000_000_000_000,
        };
        let v = serde_json::to_value(&payload).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.get("first_run").and_then(|x| x.as_bool()), Some(true));
        assert!(
            obj.get("first_run_providers")
                .and_then(|x| x.as_array())
                .map(|a| a.len())
                == Some(1),
            "expected first_run_providers array of length 1: {v}"
        );
        let round_tripped: DiscoveryHintPayload = serde_json::from_value(v).unwrap();
        assert_eq!(round_tripped, payload);
    }
}
