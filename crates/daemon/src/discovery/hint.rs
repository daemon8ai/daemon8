// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Discovery hint emission (D2).
//!
//! When daemon8 classifies a project and finds that the librarian has no
//! `source_template` entries covering the project's tags, it emits a
//! structured observation on the `discovery_hint` channel. Agents
//! reading observations via the MCP `query_observations` tool see the
//! hint, investigate where the project's runtime data lives, and call
//! `librarian_index` with one or more `source_template` nodes.
//!
//! This module owns three pieces:
//!
//! 1. [`should_emit_hint`] — the decision predicate. Pure; trivial to
//!    test independently of the emission path.
//! 2. [`build_payload`] — assembles the [`DiscoveryHintPayload`] with
//!    the instruction text the agent reads. Pure.
//! 3. [`emit_discovery_hint`] — wraps the payload in an
//!    [`Observation`] and pushes it onto the daemon's observation
//!    channel. Mirrors how source watchers and the conversation watcher
//!    write to the same channel.

use std::time::{SystemTime, UNIX_EPOCH};

use daemon8_store::librarian_validators::KNOWN_PROJECT_TYPE_TAGS;
use daemon8_types::{
    AppName, DiscoveryHintPayload, Observation, ObservationKind, Origin, ProjectClassification,
    Severity, SourceTemplateData,
};
use tokio::sync::mpsc::UnboundedSender;

/// Origin name carried on discovery-hint observations. Picked so the
/// observation lands under `app:daemon8.discovery` in filters and the
/// agent can target it precisely.
const DISCOVERY_ORIGIN: &str = "daemon8.discovery";

/// Channel string on `ObservationKind::Custom`. This is the agent-facing
/// contract — referenced in tool descriptions and integration tests.
pub const DISCOVERY_HINT_CHANNEL: &str = "discovery_hint";

/// Decide whether a hint should be emitted for the given classification.
///
/// Emits when the project has classification tags AND the librarian
/// returned zero matching source_templates. A fully covered project
/// (`librarian_templates_matched > 0`) does not get a hint — the cache
/// path applies, no agent involvement needed.
///
/// A classification with no tags is degenerate (the universal `git-repo`
/// tag should always be present once D1 has run) but we still return
/// false rather than spamming hints.
pub fn should_emit_hint(
    classification: &ProjectClassification,
    librarian_templates_matched: usize,
) -> bool {
    !classification.tags.is_empty() && librarian_templates_matched == 0
}

/// Build a [`DiscoveryHintPayload`] from a classification plus the
/// librarian's current coverage of the classification tags.
///
/// `matched_templates` is the set of source_template entries the librarian
/// returned for this classification; `missing_for_tags` is the subset of
/// classification tags with no template coverage (typically the full
/// classification tag list when the librarian is empty for these tags).
pub fn build_payload(
    classification: &ProjectClassification,
    matched_templates: &[SourceTemplateData],
    missing_for_tags: &[String],
) -> DiscoveryHintPayload {
    let known_project_type_tags_ref: Vec<String> = KNOWN_PROJECT_TYPE_TAGS
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    let instruction_text = render_instruction_text(
        &classification.tags,
        &known_project_type_tags_ref,
        missing_for_tags,
    );

    DiscoveryHintPayload {
        project_root: classification.root.clone(),
        classification_tags: classification.tags.clone(),
        framework_versions: classification.framework_versions.clone(),
        platform: classification.platform,
        known_templates_matched: matched_templates.len() as u32,
        missing_for_tags: missing_for_tags.to_vec(),
        known_project_type_tags_ref,
        instruction_text,
        first_run: None,
        emitted_at_ns: now_ns(),
    }
}

/// Serialize the payload into a discovery-hint observation and push it
/// onto the daemon's observation channel. The store writer task picks
/// it up, assigns an id, and broadcasts it to MCP/SSE subscribers.
///
/// Returns an error if the channel is closed (the receiver has been
/// dropped). Callers should treat that as a daemon-shutdown signal.
pub fn emit_discovery_hint(
    obs_tx: &UnboundedSender<Observation>,
    payload: DiscoveryHintPayload,
) -> Result<(), DiscoveryHintError> {
    let project_root = payload.project_root.display().to_string();
    let data = serde_json::to_value(&payload).map_err(DiscoveryHintError::Serialize)?;

    let mut obs = Observation::new(
        Origin::Application {
            name: AppName::from(DISCOVERY_ORIGIN),
        },
        ObservationKind::Custom {
            channel: DISCOVERY_HINT_CHANNEL.to_string(),
        },
        data,
        Severity::Info,
        None,
    );

    // Tag with project_root so agents filtering by tags can scope hints
    // to the specific project they care about. The classification tags
    // are part of the payload, not the observation tags — the agent
    // reads payload fields rather than relying on tag fan-out.
    obs.tags = Some(vec![format!("project_root:{project_root}")]);

    obs_tx
        .send(obs)
        .map_err(|_| DiscoveryHintError::ChannelClosed)
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryHintError {
    #[error("failed to serialize DiscoveryHintPayload: {0}")]
    Serialize(#[source] serde_json::Error),

    #[error("observation channel closed")]
    ChannelClosed,
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos() as u64
}

// The instruction text is the agent's runbook. It must list the
// classification tags so the agent knows what it's onboarding, surface
// the full validator allowlist so the agent only writes tags that will
// pass validation, and spell out the portability rules so the agent
// never embeds a literal `/Users/<name>` path.
fn render_instruction_text(
    classification_tags: &[String],
    known_project_type_tags_ref: &[String],
    missing_for_tags: &[String],
) -> String {
    let tags_list = classification_tags.join(", ");
    let known_tags_list = known_project_type_tags_ref.join(", ");
    let missing_list = if missing_for_tags.is_empty() {
        "(none — all classification tags lack coverage)".to_string()
    } else {
        missing_for_tags.join(", ")
    };

    format!(
        "daemon8 discovery hint: this project is classified as [{tags_list}]. The librarian has 0 source_template entries matching these tags on this machine. Investigate where logs, conversation data, build artifacts, and other runtime sources live for these frameworks on this OS, then call librarian_index with source_template entries for each location you find.\n\
\n\
Tags with no librarian coverage: [{missing_list}].\n\
\n\
Required for each source_template node:\n\
  kind: \"source_template\"\n\
  data: {{\n\
    project_types: <subset of: [{known_tags_list}]>\n\
    kind: \"log\" | \"config\" | \"conversation\" | \"cache\" | \"crash\" | \"build\" | \"db\" | \"metric\"\n\
    locator_pattern: \"<path with ~ for home; env vars allowed; glob OK>\"\n\
    platforms: [\"<os tags>\"]\n\
    parser_hint: <optional parser name>\n\
    default_tags: [\"<labels for resulting observations>\"]\n\
    description: \"<one-line human-readable name>\"\n\
    version_constraint: <SemVer range like \">=0.74\" or null for version-agnostic>\n\
    confidence: \"agent_discovered\"\n\
    discovered_at_ns: <integer ns timestamp>\n\
    verified_count: 0\n\
    last_verified_at_ns: <same as discovered_at_ns>\n\
  }}\n\
\n\
Portability rules (enforced by validator, rejection returns actionable error):\n\
  - locator_pattern MUST use ~ for home directory (never /Users/<name>)\n\
  - locator_pattern MUST NOT contain absolute paths starting with /Users/, /home/, C:\\Users\\, D:\\Users\\, or similar\n\
  - locator_pattern MUST NOT contain UNC paths (\\\\server\\share)\n\
  - Use <root> placeholder for project-relative paths\n\
  - Use $VAR or ${{VAR}} for environment variables\n\
  - project_types MUST be a subset of: [{known_tags_list}]\n\
  - platforms MUST NOT be empty\n\
\n\
When you have written all the templates you can confidently identify, call this hint complete. If unsure about a location, omit it rather than guess. If multiple version-specific paths exist for the same source kind, write multiple templates with appropriate version_constraint values."
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use daemon8_types::{Filter, ObservationKindTag, Platform};
    use tokio::sync::mpsc;

    use super::*;

    fn classification() -> ProjectClassification {
        let mut versions = BTreeMap::new();
        versions.insert("react-native".to_string(), "0.74.5".to_string());
        ProjectClassification {
            tags: vec!["react-native".into(), "vega".into()],
            framework_versions: versions,
            root: PathBuf::from("/tmp/rtntv_vega"),
            manifests: BTreeMap::new(),
            platform: Platform::Macos,
        }
    }

    #[test]
    fn build_payload_includes_known_tags_reference() {
        let c = classification();
        let payload = build_payload(&c, &[], &c.tags);
        assert_eq!(
            payload.known_project_type_tags_ref.len(),
            KNOWN_PROJECT_TYPE_TAGS.len(),
            "payload must mirror the validator allowlist 1:1",
        );
        for tag in KNOWN_PROJECT_TYPE_TAGS {
            assert!(
                payload.known_project_type_tags_ref.contains(&(*tag).into()),
                "payload missing known tag {tag}",
            );
        }
    }

    #[test]
    fn build_payload_handles_empty_missing_tags() {
        let c = classification();
        let payload = build_payload(&c, &[], &[]);
        assert!(payload.missing_for_tags.is_empty());
        assert!(
            payload
                .instruction_text
                .contains("all classification tags lack coverage"),
            "instruction text should explain the empty-missing case",
        );
    }

    #[test]
    fn should_emit_hint_returns_true_for_unrecognized_project() {
        let c = classification();
        assert!(should_emit_hint(&c, 0));
    }

    #[test]
    fn should_emit_hint_returns_false_for_fully_covered_project() {
        let c = classification();
        assert!(!should_emit_hint(&c, 4));
    }

    #[test]
    fn should_emit_hint_returns_false_for_classification_with_no_tags() {
        let mut c = classification();
        c.tags.clear();
        assert!(!should_emit_hint(&c, 0));
    }

    #[test]
    fn instruction_text_includes_classification_tags() {
        let c = classification();
        let payload = build_payload(&c, &[], &c.tags);
        assert!(payload.instruction_text.contains("react-native"));
        assert!(payload.instruction_text.contains("vega"));
    }

    #[test]
    fn instruction_text_includes_portability_rules() {
        let c = classification();
        let payload = build_payload(&c, &[], &c.tags);
        assert!(payload.instruction_text.contains("Portability rules"));
        assert!(payload.instruction_text.contains("~ for home"));
        assert!(payload.instruction_text.contains("UNC"));
        assert!(payload.instruction_text.contains("<root>"));
    }

    #[test]
    fn instruction_text_lists_known_tags() {
        let c = classification();
        let payload = build_payload(&c, &[], &c.tags);
        for tag in KNOWN_PROJECT_TYPE_TAGS {
            assert!(
                payload.instruction_text.contains(tag),
                "instruction text missing known tag {tag}",
            );
        }
    }

    #[test]
    fn first_run_field_defaults_to_none() {
        let c = classification();
        let payload = build_payload(&c, &[], &c.tags);
        assert!(payload.first_run.is_none());
    }

    #[tokio::test]
    async fn emit_discovery_hint_pushes_custom_observation_on_channel() {
        let (tx, mut rx) = mpsc::unbounded_channel::<Observation>();
        let c = classification();
        let payload = build_payload(&c, &[], &c.tags);

        emit_discovery_hint(&tx, payload.clone()).unwrap();

        let obs = rx.recv().await.expect("hint observation");
        assert_eq!(obs.kind.tag(), ObservationKindTag::Custom);
        match &obs.kind {
            ObservationKind::Custom { channel } => {
                assert_eq!(channel, DISCOVERY_HINT_CHANNEL);
            }
            other => panic!("expected Custom kind, got {other:?}"),
        }
        assert_eq!(obs.severity, Severity::Info);

        // Payload round-trips out of the observation `data` field.
        let parsed: DiscoveryHintPayload = serde_json::from_value(obs.data.clone()).unwrap();
        assert_eq!(parsed, payload);

        // Filter discipline: queries scoped to custom kind catch the hint.
        let filter = Filter {
            kinds: Some(vec![ObservationKindTag::Custom]),
            ..Default::default()
        };
        assert!(filter.matches(&obs));
    }

    #[test]
    fn emit_discovery_hint_returns_channel_closed_when_receiver_dropped() {
        let (tx, rx) = mpsc::unbounded_channel::<Observation>();
        drop(rx);

        let c = classification();
        let payload = build_payload(&c, &[], &c.tags);
        let err = emit_discovery_hint(&tx, payload).unwrap_err();
        assert!(matches!(err, DiscoveryHintError::ChannelClosed));
    }
}
