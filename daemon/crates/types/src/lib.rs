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
        }
    }
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Filter {
    pub kinds: Option<Vec<ObservationKindTag>>,
    pub severity_min: Option<Severity>,
    pub origins: Option<Vec<OriginPattern>>,
    pub text_match: Option<String>,
    pub since: Option<Checkpoint>,
    pub limit: Option<usize>,
}

impl Filter {
    pub fn matches(&self, obs: &Observation) -> bool {
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
            let haystack = obs.data.to_string().to_ascii_lowercase();
            if !haystack.contains(&text.to_ascii_lowercase()) {
                return false;
            }
        }

        true
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
    fn tab_id_equality_regardless_of_construction_path() {
        let from_str = TabId::from("abc");
        let from_string = TabId::from(String::from("abc"));
        let from_arc: TabId = std::sync::Arc::<str>::from("abc").into();
        assert_eq!(from_str, from_string);
        assert_eq!(from_string, from_arc);
        assert_eq!(from_str, "abc");
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
}
