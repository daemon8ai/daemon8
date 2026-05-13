// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Integration test for D5 first-run conversation bootstrap (Commit 5).
//!
//! The scanner emits a `discovery_hint` whose payload carries a
//! `first_run_providers` array the first time daemon8 runs on a machine
//! that has no conversation `source_template` for one or more AI
//! providers. Once an agent registers a conversation template tagged
//! with a provider id, subsequent scans must NOT include that provider
//! in `first_run_providers`.
//!
//! Two flows verified here:
//!
//! 1. First serve on an empty librarian -> hint includes
//!    `first_run: Some(true)` and `first_run_providers` covering
//!    every provider that exposes a conversation directory and glob.
//! 2. After the agent writes a conversation template tagged with one
//!    provider, the next scan's hint either omits that provider from
//!    `first_run_providers` or, when no other reason to hint remains,
//!    suppresses the hint entirely.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use daemon8_providers::{ALL_PROVIDERS, AiProvider};
use daemon8_store::{LibrarianNode, LibrarianStore, SurrealStore};
use daemon8_types::{
    DiscoveryHintPayload, LibrarianNodeKind, LocatorKind, Observation, ObservationKind, Platform,
    SourceKind, SourceTemplateData, TemplateConfidence,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

// Empty `.git` directory + package.json gives the classifier enough to
// emit at least the universal `git-repo` tag. The first-run branch is
// per-provider and not tied to project classification — but the scanner
// only fires hints once classification produced tags.
fn write_fixture_project(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join("package.json"),
        r#"{"name": "fr-fixture", "dependencies": {}}"#,
    )
    .unwrap();
}

// A source_template that resolves to a real file inside the fixture
// project, used to cover every classification tag so the only reason
// the hint would fire is the first-run providers branch.
fn coverage_template_node(pattern: &str, project_types: &[&str]) -> LibrarianNode {
    let data = SourceTemplateData {
        project_types: project_types.iter().map(|s| (*s).to_string()).collect(),
        kind: SourceKind::Log,
        locator_pattern: pattern.to_string(),
        platforms: vec![Platform::current()],
        parser_hint: None,
        default_tags: vec!["fixture".into()],
        description: "coverage template".into(),
        version_constraint: None,
        discovered_by_session: None,
        discovered_by_provider: None,
        discovered_at_ns: now_ns(),
        verified_count: 0,
        last_verified_at_ns: now_ns(),
        confidence: TemplateConfidence::AgentDiscovered,
    };
    LibrarianNode {
        id: None,
        kind: LibrarianNodeKind::SourceTemplate,
        label: "fixture log".into(),
        locator_kind: LocatorKind::File,
        locator: pattern.to_string(),
        tags: data.project_types.clone(),
        project_slug: String::new(),
        version: String::new(),
        parent_id: None,
        created_at: now_ns(),
        updated_at: now_ns(),
        last_read_at: None,
        deprecated_at: None,
        canonicalized_at: None,
        data: Some(serde_json::to_value(&data).unwrap()),
    }
}

fn conversation_template_node(provider_id: &str) -> LibrarianNode {
    let data = SourceTemplateData {
        project_types: vec!["any".into()],
        kind: SourceKind::Conversation,
        locator_pattern: format!("~/.{provider_id}/projects/**/*.jsonl"),
        platforms: vec![Platform::Macos, Platform::Linux, Platform::Windows],
        parser_hint: Some(format!("ai_conversation_{provider_id}")),
        default_tags: vec![provider_id.into(), "agent".into(), "conversation".into()],
        description: format!("{provider_id} conversation transcript"),
        version_constraint: None,
        discovered_by_session: None,
        discovered_by_provider: Some(provider_id.into()),
        discovered_at_ns: now_ns(),
        verified_count: 0,
        last_verified_at_ns: now_ns(),
        confidence: TemplateConfidence::AgentDiscovered,
    };
    LibrarianNode {
        id: None,
        kind: LibrarianNodeKind::SourceTemplate,
        label: format!("{provider_id} convo"),
        locator_kind: LocatorKind::File,
        locator: data.locator_pattern.clone(),
        tags: data.default_tags.clone(),
        project_slug: String::new(),
        version: String::new(),
        parent_id: None,
        created_at: now_ns(),
        updated_at: now_ns(),
        last_read_at: None,
        deprecated_at: None,
        canonicalized_at: None,
        data: Some(serde_json::to_value(&data).unwrap()),
    }
}

// Providers whose conversation_dir + conversation_file_glob are both
// declared. The scanner only emits first-run payloads for these.
fn eligible_providers() -> Vec<&'static dyn AiProvider> {
    use daemon8_providers::dirs_home;
    let home = dirs_home();
    ALL_PROVIDERS
        .iter()
        .map(|&p| p.as_provider())
        .filter(|p| p.conversation_dir(&home).is_some() && p.conversation_file_glob().is_some())
        .collect()
}

async fn run_scan(
    lib: Arc<dyn LibrarianStore>,
    root: std::path::PathBuf,
) -> (
    Result<daemon8::discovery::scanner::DiscoveryPlan, daemon8::discovery::scanner::ScannerError>,
    Vec<Observation>,
) {
    let (obs_tx, mut obs_rx) = mpsc::unbounded_channel::<Observation>();
    let cancel = CancellationToken::new();
    // Short timeout: the first-run branch only emits the hint and then
    // waits for the agent. We don't need to wait the full window — we
    // capture the hint as soon as it shows up and let the scanner
    // time out cleanly.
    let cfg = daemon8::discovery::scanner::ScannerConfig {
        wait_timeout: Duration::from_millis(800),
        poll_interval: Duration::from_millis(200),
        cache_max_age: Duration::from_secs(7 * 24 * 60 * 60),
    };
    let lib_for_scan = lib.clone();
    let cancel_for_scan = cancel.clone();
    let scan = tokio::spawn(async move {
        daemon8::discovery::scanner::scan(
            &root,
            &*lib_for_scan,
            &obs_tx,
            Vec::new(),
            cfg,
            cancel_for_scan,
            None,
        )
        .await
    });
    let plan = tokio::time::timeout(Duration::from_secs(5), scan)
        .await
        .expect("scan completes in test budget")
        .expect("scan task join");
    let mut hints = Vec::new();
    while let Ok(Some(obs)) = tokio::time::timeout(Duration::from_millis(50), obs_rx.recv()).await {
        hints.push(obs);
    }
    (plan, hints)
}

fn parse_hint_payload(obs: &Observation) -> DiscoveryHintPayload {
    match &obs.kind {
        ObservationKind::Custom { channel } => {
            assert_eq!(channel, "discovery_hint", "expected discovery_hint channel");
        }
        other => panic!("expected Custom kind, got {other:?}"),
    }
    serde_json::from_value(obs.data.clone()).expect("hint payload deserializes")
}

#[tokio::test]
async fn first_serve_with_empty_librarian_emits_first_run_for_every_eligible_provider() {
    let surreal = Arc::new(SurrealStore::memory().await.unwrap());
    let lib: Arc<dyn LibrarianStore> = Arc::new(surreal.librarian_store());

    let tmp = tempfile::tempdir().unwrap();
    write_fixture_project(tmp.path());

    let (plan, hints) = run_scan(lib.clone(), tmp.path().to_path_buf()).await;
    let _plan = plan.expect("scan ok");
    assert!(!hints.is_empty(), "expected at least one hint observation");

    let payload = parse_hint_payload(&hints[0]);
    assert_eq!(
        payload.first_run,
        Some(true),
        "empty librarian must produce first_run hint: {payload:?}"
    );
    let fr = payload
        .first_run_providers
        .as_ref()
        .expect("first_run_providers populated");
    let eligible = eligible_providers();
    assert_eq!(
        fr.len(),
        eligible.len(),
        "first_run_providers should cover every eligible provider; got {fr:?} expected {} entries",
        eligible.len()
    );
    for provider in &eligible {
        assert!(
            fr.iter().any(|p| p.provider_id == provider.id()),
            "first_run_providers missing {}",
            provider.id()
        );
    }
}

#[tokio::test]
async fn provider_with_conversation_template_is_excluded_from_first_run() {
    let surreal = Arc::new(SurrealStore::memory().await.unwrap());
    let lib: Arc<dyn LibrarianStore> = Arc::new(surreal.librarian_store());

    // Pre-seed: agent has already registered Claude's conversation template.
    lib.index_node(conversation_template_node("claude"))
        .await
        .unwrap();

    let tmp = tempfile::tempdir().unwrap();
    write_fixture_project(tmp.path());

    let (plan, hints) = run_scan(lib.clone(), tmp.path().to_path_buf()).await;
    let _plan = plan.expect("scan ok");
    assert!(!hints.is_empty(), "expected at least one hint observation");

    let payload = parse_hint_payload(&hints[0]);
    let fr = payload
        .first_run_providers
        .as_ref()
        .expect("non-claude providers still first-run");

    assert!(
        !fr.iter().any(|p| p.provider_id == "claude"),
        "claude should be excluded once its template exists: {fr:?}"
    );
    // At least one other eligible provider remains in the first-run
    // set on a fresh librarian.
    let eligible = eligible_providers();
    if eligible.len() > 1 {
        assert!(
            !fr.is_empty(),
            "expected remaining first-run entries for non-claude providers"
        );
    }
}

#[tokio::test]
async fn hint_is_suppressed_once_coverage_and_all_providers_satisfied() {
    let surreal = Arc::new(SurrealStore::memory().await.unwrap());
    let lib: Arc<dyn LibrarianStore> = Arc::new(surreal.librarian_store());

    let tmp = tempfile::tempdir().unwrap();
    write_fixture_project(tmp.path());

    // Coverage template that resolves both classification tags
    // (git-repo + dependency-free package.json doesn't add any other
    // detectable tag, so git-repo alone is the tag set here).
    let log_path = tmp.path().join("runtime.log");
    std::fs::write(&log_path, "x").unwrap();
    let pattern = format!("{}/runtime.log", tmp.path().display());
    lib.index_node(coverage_template_node(&pattern, &["git-repo"]))
        .await
        .unwrap();

    // Conversation template per eligible provider so the first-run
    // branch is fully satisfied.
    for provider in eligible_providers() {
        lib.index_node(conversation_template_node(provider.id()))
            .await
            .unwrap();
    }

    let (plan, hints) = run_scan(lib.clone(), tmp.path().to_path_buf()).await;
    let plan = plan.expect("scan ok");
    assert!(
        !plan.awaiting_agent,
        "plan should not be awaiting agent when coverage + first-run both satisfied"
    );
    // No hint should fire on the silent path.
    assert!(
        hints.is_empty(),
        "expected zero hint observations on the silent path; got {} hints with kinds {:?}",
        hints.len(),
        hints.iter().map(|h| &h.kind).collect::<Vec<_>>()
    );
    let _ = BTreeMap::<String, String>::new();
}
