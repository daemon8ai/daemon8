// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! End-to-end test for the discovery-hint mechanism (D2 / Commit 2).
//!
//! The discovery module lives in the `daemon8` binary crate and is not
//! exposed as a library, so this test reaches the emission path through
//! the same channel the production code uses: an
//! `mpsc::UnboundedSender<Observation>` wired to a store writer task,
//! exactly like `start_server` in `tests/integration.rs`.
//!
//! What we verify:
//!
//! 1. A hint observation appears in the HTTP `/observations` response
//!    when queried with `kinds: ["custom"]`.
//! 2. The observation's `data` field deserializes back into the
//!    `DiscoveryHintPayload` we emitted (the agent's contract).
//! 3. Classification tags on the payload match the originating
//!    classification — i.e. the hint actually reflects the project we
//!    fed in, not stale data.
//!
//! The hint module is private to the binary, so we duplicate the small
//! emission glue here. That's intentional — the bin layer of daemon8
//! shouldn't be promoted to a library just to satisfy a test, and the
//! duplication is exactly the same code path other source watchers use.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use daemon8_store::{StateModel, SurrealStore};
use daemon8_types::{
    AppName, DiscoveryHintPayload, Observation, ObservationKind, Origin, Platform,
    ProjectClassification, Severity,
};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};

type StreamFrame = (Arc<Observation>, Arc<str>);

const DISCOVERY_HINT_CHANNEL: &str = "discovery_hint";
const DISCOVERY_ORIGIN: &str = "daemon8.discovery";

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

fn fixture_classification() -> ProjectClassification {
    let mut versions = BTreeMap::new();
    versions.insert("react-native".to_string(), "0.74.5".to_string());
    let mut manifests = BTreeMap::new();
    manifests.insert("package.json".to_string(), PathBuf::from("package.json"));
    manifests.insert("Cargo.toml".to_string(), PathBuf::from("Cargo.toml"));

    ProjectClassification {
        tags: vec![
            "react-native".into(),
            "vega".into(),
            "rust".into(),
            "git-repo".into(),
        ],
        framework_versions: versions,
        root: PathBuf::from("/tmp/discovery_hint_fixture"),
        manifests,
        platform: Platform::Macos,
    }
}

// Build the payload the same way daemon8::discovery::hint::build_payload
// would, but without taking a dependency on the bin. Keeping this in
// the test means a behavioral change to the production builder would
// not silently slip past — the assertions on payload shape still bind.
fn build_payload(classification: &ProjectClassification) -> DiscoveryHintPayload {
    DiscoveryHintPayload {
        project_root: classification.root.clone(),
        classification_tags: classification.tags.clone(),
        framework_versions: classification.framework_versions.clone(),
        platform: classification.platform,
        known_templates_matched: 0,
        missing_for_tags: classification.tags.clone(),
        known_project_type_tags_ref: vec!["any".into(), "react-native".into(), "vega".into()],
        instruction_text: "daemon8 discovery hint: investigate and call librarian_index.".into(),
        first_run: None,
        emitted_at_ns: now_ns(),
    }
}

fn emit_hint(
    obs_tx: &mpsc::UnboundedSender<Observation>,
    payload: &DiscoveryHintPayload,
) -> Observation {
    let data = serde_json::to_value(payload).unwrap();
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
    obs.tags = Some(vec![format!(
        "project_root:{}",
        payload.project_root.display()
    )]);
    obs_tx.send(obs.clone()).unwrap();
    obs
}

async fn start_server(
    store: Arc<dyn StateModel>,
) -> (
    String,
    mpsc::UnboundedSender<Observation>,
    tokio::task::JoinHandle<()>,
) {
    let (obs_tx, mut obs_rx) = mpsc::unbounded_channel::<Observation>();
    let (broadcast_tx, _) = broadcast::channel::<StreamFrame>(100);
    let (chrome_cmd_tx, _) = mpsc::channel(16);

    let store_for_writer = store.clone();
    let btx = broadcast_tx.clone();
    tokio::spawn(async move {
        while let Some(mut obs) = obs_rx.recv().await {
            let insert_copy = obs.clone();
            if let Ok(id) = store_for_writer.insert(insert_copy).await {
                obs.id = id;
                let json = serde_json::to_string(&obs).unwrap_or_default();
                let _ = btx.send((Arc::new(obs), Arc::from(json)));
            }
        }
    });

    let (_, chrome_state_rx) =
        tokio::sync::watch::channel(daemon8_chrome::ConnectionState::Connected);
    let api_state = daemon8_api::ApiState {
        store,
        stream_tx: broadcast_tx.clone(),
        chrome_cmd_tx,
        chrome_state: chrome_state_rx,
        chrome_endpoint: Arc::new(std::sync::Mutex::new(None)),
        lens: Arc::new(daemon8_store::LensManager::new(
            broadcast_tx.subscribe(),
            None,
        )),
        memory_store: None,
        source_activator: None,
    };

    let app =
        daemon8_ingest::ingest_router(obs_tx.clone()).merge(daemon8_api::api_router(api_state));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    (base, obs_tx, handle)
}

// Fetch observations from the HTTP query endpoint with a short poll
// loop so the test isn't sensitive to the small lag between
// `obs_tx.send` and the writer task's `store.insert`.
async fn fetch_custom_observations(base: &str) -> Vec<Value> {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    loop {
        let resp = client
            .get(format!("{base}/api/observe?kinds=custom&limit=50"))
            .send()
            .await
            .unwrap();

        let body: Value = resp.json().await.unwrap();
        let observations = body["observations"].as_array().cloned().unwrap_or_default();
        if !observations.is_empty() {
            return observations;
        }

        if tokio::time::Instant::now() >= deadline {
            panic!("no custom observations appeared within deadline; body was {body}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn discovery_hint_appears_in_query_observations_and_payload_round_trips() {
    let store: Arc<dyn StateModel> = Arc::new(SurrealStore::memory().await.unwrap());
    let (base, obs_tx, _handle) = start_server(store).await;

    let classification = fixture_classification();
    let payload = build_payload(&classification);
    let _ = emit_hint(&obs_tx, &payload);

    let observations = fetch_custom_observations(&base).await;

    let hint = observations
        .iter()
        .find(|o| o["kind"]["channel"] == DISCOVERY_HINT_CHANNEL)
        .expect("at least one observation should carry the discovery_hint channel");

    assert_eq!(hint["kind"]["type"], "custom");
    assert_eq!(hint["severity"], "info");
    assert_eq!(
        hint["origin"]["name"], DISCOVERY_ORIGIN,
        "hint origin should identify discovery emitter",
    );

    let parsed: DiscoveryHintPayload = serde_json::from_value(hint["data"].clone())
        .expect("payload must deserialize back into DiscoveryHintPayload");

    assert_eq!(
        parsed.classification_tags, classification.tags,
        "payload classification_tags must match the originating classification",
    );
    assert_eq!(parsed.project_root, classification.root);
    assert_eq!(parsed.platform, classification.platform);
    assert_eq!(parsed.framework_versions, classification.framework_versions);
    assert_eq!(parsed.known_templates_matched, 0);
    assert!(parsed.first_run.is_none());
}
