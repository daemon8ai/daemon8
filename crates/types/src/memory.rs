// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::fmt;
use std::str::FromStr;

use rmcp::schemars;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTier {
    Short,
    Reference,
    Long,
}

impl MemoryTier {
    /// Long and Reference are durable; bookkeeper jobs may move memory between
    /// them but never auto-delete. Short tier is per-agent working memory and
    /// is reaped by the bookkeeper sweep when its TTL elapses.
    pub fn is_durable(self) -> bool {
        !matches!(self, Self::Short)
    }
}

impl fmt::Display for MemoryTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Short => "short",
            Self::Reference => "reference",
            Self::Long => "long",
        };
        f.write_str(s)
    }
}

impl FromStr for MemoryTier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "short" => Ok(Self::Short),
            "reference" => Ok(Self::Reference),
            "long" => Ok(Self::Long),
            other => Err(format!("unknown memory tier: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    Agent,
    Team,
    Project,
    Global,
}

impl fmt::Display for MemoryScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Agent => "agent",
            Self::Team => "team",
            Self::Project => "project",
            Self::Global => "global",
        };
        f.write_str(s)
    }
}

impl FromStr for MemoryScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "agent" => Ok(Self::Agent),
            "team" => Ok(Self::Team),
            "project" => Ok(Self::Project),
            "global" => Ok(Self::Global),
            other => Err(format!("unknown memory scope: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShortContentKind {
    Scratch,
    Summary,
    Fact,
    Observation,
}

impl fmt::Display for ShortContentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Scratch => "scratch",
            Self::Summary => "summary",
            Self::Fact => "fact",
            Self::Observation => "observation",
        };
        f.write_str(s)
    }
}

impl FromStr for ShortContentKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "scratch" => Ok(Self::Scratch),
            "summary" => Ok(Self::Summary),
            "fact" => Ok(Self::Fact),
            "observation" => Ok(Self::Observation),
            other => Err(format!("unknown short content kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceSourceKind {
    Doc,
    Forum,
    Rfc,
    Code,
    Note,
    Other,
}

impl fmt::Display for ReferenceSourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Doc => "doc",
            Self::Forum => "forum",
            Self::Rfc => "rfc",
            Self::Code => "code",
            Self::Note => "note",
            Self::Other => "other",
        };
        f.write_str(s)
    }
}

impl FromStr for ReferenceSourceKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "doc" => Ok(Self::Doc),
            "forum" => Ok(Self::Forum),
            "rfc" => Ok(Self::Rfc),
            "code" => Ok(Self::Code),
            "note" => Ok(Self::Note),
            "other" => Ok(Self::Other),
            other => Err(format!("unknown reference source kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LongContentKind {
    Fact,
    Pattern,
    Decision,
    Heuristic,
    Example,
}

impl fmt::Display for LongContentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Fact => "fact",
            Self::Pattern => "pattern",
            Self::Decision => "decision",
            Self::Heuristic => "heuristic",
            Self::Example => "example",
        };
        f.write_str(s)
    }
}

impl FromStr for LongContentKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "fact" => Ok(Self::Fact),
            "pattern" => Ok(Self::Pattern),
            "decision" => Ok(Self::Decision),
            "heuristic" => Ok(Self::Heuristic),
            "example" => Ok(Self::Example),
            other => Err(format!("unknown long content kind: {other}")),
        }
    }
}

/// Provenance trail for a `memory_long` row. Each entry records one source
/// that contributed to the long memory: a short memory that was promoted, an
/// envelope that was extracted from, or a reference doc that corroborated.
/// Stored as a flexible object array on the `memory_long` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProvenanceEntry {
    pub source_kind: String,
    pub source_id: String,
    pub validated_at: u64,
    pub validated_by: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_serde_roundtrip<T>(value: &T, wire: &str)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + fmt::Debug,
    {
        let text = serde_json::to_string(value).expect("serialize");
        assert_eq!(text, wire);
        let back: T = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(&back, value);
    }

    #[test]
    fn memory_tier_serde_snake_case() {
        assert_serde_roundtrip(&MemoryTier::Short, "\"short\"");
        assert_serde_roundtrip(&MemoryTier::Reference, "\"reference\"");
        assert_serde_roundtrip(&MemoryTier::Long, "\"long\"");
    }

    #[test]
    fn memory_tier_display_fromstr_roundtrip() {
        for t in [MemoryTier::Short, MemoryTier::Reference, MemoryTier::Long] {
            let text = t.to_string();
            let back: MemoryTier = text.parse().expect("parse");
            assert_eq!(back, t);
        }
        assert!("nonsense".parse::<MemoryTier>().is_err());
    }

    #[test]
    fn memory_tier_is_durable_classification() {
        assert!(!MemoryTier::Short.is_durable());
        assert!(MemoryTier::Reference.is_durable());
        assert!(MemoryTier::Long.is_durable());
    }

    #[test]
    fn memory_scope_serde_snake_case() {
        assert_serde_roundtrip(&MemoryScope::Agent, "\"agent\"");
        assert_serde_roundtrip(&MemoryScope::Team, "\"team\"");
        assert_serde_roundtrip(&MemoryScope::Project, "\"project\"");
        assert_serde_roundtrip(&MemoryScope::Global, "\"global\"");
    }

    #[test]
    fn memory_scope_display_fromstr_roundtrip() {
        for s in [
            MemoryScope::Agent,
            MemoryScope::Team,
            MemoryScope::Project,
            MemoryScope::Global,
        ] {
            let text = s.to_string();
            let back: MemoryScope = text.parse().expect("parse");
            assert_eq!(back, s);
        }
        assert!("nonsense".parse::<MemoryScope>().is_err());
    }

    #[test]
    fn short_content_kind_serde_snake_case() {
        assert_serde_roundtrip(&ShortContentKind::Scratch, "\"scratch\"");
        assert_serde_roundtrip(&ShortContentKind::Summary, "\"summary\"");
        assert_serde_roundtrip(&ShortContentKind::Fact, "\"fact\"");
        assert_serde_roundtrip(&ShortContentKind::Observation, "\"observation\"");
    }

    #[test]
    fn short_content_kind_display_fromstr_roundtrip() {
        for k in [
            ShortContentKind::Scratch,
            ShortContentKind::Summary,
            ShortContentKind::Fact,
            ShortContentKind::Observation,
        ] {
            let text = k.to_string();
            let back: ShortContentKind = text.parse().expect("parse");
            assert_eq!(back, k);
        }
        assert!("nonsense".parse::<ShortContentKind>().is_err());
    }

    #[test]
    fn reference_source_kind_serde_snake_case() {
        assert_serde_roundtrip(&ReferenceSourceKind::Doc, "\"doc\"");
        assert_serde_roundtrip(&ReferenceSourceKind::Forum, "\"forum\"");
        assert_serde_roundtrip(&ReferenceSourceKind::Rfc, "\"rfc\"");
        assert_serde_roundtrip(&ReferenceSourceKind::Code, "\"code\"");
        assert_serde_roundtrip(&ReferenceSourceKind::Note, "\"note\"");
        assert_serde_roundtrip(&ReferenceSourceKind::Other, "\"other\"");
    }

    #[test]
    fn reference_source_kind_display_fromstr_roundtrip() {
        for k in [
            ReferenceSourceKind::Doc,
            ReferenceSourceKind::Forum,
            ReferenceSourceKind::Rfc,
            ReferenceSourceKind::Code,
            ReferenceSourceKind::Note,
            ReferenceSourceKind::Other,
        ] {
            let text = k.to_string();
            let back: ReferenceSourceKind = text.parse().expect("parse");
            assert_eq!(back, k);
        }
        assert!("nonsense".parse::<ReferenceSourceKind>().is_err());
    }

    #[test]
    fn long_content_kind_serde_snake_case() {
        assert_serde_roundtrip(&LongContentKind::Fact, "\"fact\"");
        assert_serde_roundtrip(&LongContentKind::Pattern, "\"pattern\"");
        assert_serde_roundtrip(&LongContentKind::Decision, "\"decision\"");
        assert_serde_roundtrip(&LongContentKind::Heuristic, "\"heuristic\"");
        assert_serde_roundtrip(&LongContentKind::Example, "\"example\"");
    }

    #[test]
    fn long_content_kind_display_fromstr_roundtrip() {
        for k in [
            LongContentKind::Fact,
            LongContentKind::Pattern,
            LongContentKind::Decision,
            LongContentKind::Heuristic,
            LongContentKind::Example,
        ] {
            let text = k.to_string();
            let back: LongContentKind = text.parse().expect("parse");
            assert_eq!(back, k);
        }
        assert!("nonsense".parse::<LongContentKind>().is_err());
    }

    #[test]
    fn provenance_entry_serde_roundtrip() {
        let p = ProvenanceEntry {
            source_kind: "memory_short".into(),
            source_id: "ms_abc".into(),
            validated_at: 1_700_000_000_000,
            validated_by: "agent:bookkeeper".into(),
        };
        let text = serde_json::to_string(&p).unwrap();
        let back: ProvenanceEntry = serde_json::from_str(&text).unwrap();
        assert_eq!(back, p);
    }
}
