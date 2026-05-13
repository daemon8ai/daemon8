// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Structural lock between the D1 detector and the librarian validator.
//!
//! Any tag `classify()` emits must be present in
//! `daemon8_store::librarian_validators::KNOWN_PROJECT_TYPE_TAGS`.
//! If a future code change in `project_type.rs` adds a new tag without
//! updating the validator's allow-list, source_template writes that
//! reference the new tag would be rejected even though the detector
//! claims to produce it. This test fails fast in that case.

use std::path::PathBuf;

use daemon8_providers::project_type::classify;
use daemon8_store::librarian_validators::KNOWN_PROJECT_TYPE_TAGS;

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sample-projects")
}

fn assert_all_tags_known(label: &str, tags: &[String]) {
    for tag in tags {
        if tag == "git-repo" {
            // git-repo is intentionally emitted by classify but is not
            // a project-type tag the validator gates source_templates on;
            // it appears in KNOWN_PROJECT_TYPE_TAGS so a source_template
            // could in principle scope to it, but we want the assertion
            // to be exhaustive — flag it the same way as everything else.
        }
        assert!(
            KNOWN_PROJECT_TYPE_TAGS.contains(&tag.as_str()),
            "fixture {label}: classify emitted tag '{tag}' that is not in \
             KNOWN_PROJECT_TYPE_TAGS — add it to validators or stop emitting it"
        );
    }
}

#[test]
fn every_fixture_tag_is_known_to_validator() {
    let fixtures = [
        "react-native-rtntv",
        "laravel-rcn",
        "mixed-symfony-php",
        "expo-blank",
        "rust-workspace-daemon8",
    ];
    for f in fixtures {
        let result = classify(&fixtures_root().join(f)).unwrap();
        assert_all_tags_known(f, &result.tags);
    }
}

#[test]
fn every_synthetic_project_tag_is_known_to_validator() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // nextjs from config
    std::fs::write(root.join("next.config.js"), "module.exports = {};").unwrap();
    let result = classify(root).unwrap();
    assert_all_tags_known("synthetic-nextjs", &result.tags);
    std::fs::remove_file(root.join("next.config.js")).unwrap();

    // vite from package.json
    std::fs::write(
        root.join("package.json"),
        r#"{"devDependencies":{"vite":"^5.0.0"}}"#,
    )
    .unwrap();
    let result = classify(root).unwrap();
    assert_all_tags_known("synthetic-vite", &result.tags);

    // python + django via requirements.txt
    std::fs::write(root.join("requirements.txt"), "django==4.2.7\n").unwrap();
    let result = classify(root).unwrap();
    assert_all_tags_known("synthetic-django", &result.tags);

    // python + flask via pyproject.toml
    std::fs::write(
        root.join("pyproject.toml"),
        "[project]\ndependencies = [\"flask>=3.0\"]\n",
    )
    .unwrap();
    let result = classify(root).unwrap();
    assert_all_tags_known("synthetic-flask", &result.tags);

    // rails via Gemfile
    std::fs::write(root.join("Gemfile"), "gem \"rails\", \"~> 7.1\"\n").unwrap();
    let result = classify(root).unwrap();
    assert_all_tags_known("synthetic-rails", &result.tags);

    // go via go.mod
    std::fs::write(root.join("go.mod"), "module example.com/x\ngo 1.22\n").unwrap();
    let result = classify(root).unwrap();
    assert_all_tags_known("synthetic-go", &result.tags);

    // mixed node + rust
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let result = classify(root).unwrap();
    assert_all_tags_known("synthetic-mixed", &result.tags);
}
