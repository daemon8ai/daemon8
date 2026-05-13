// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Project type detector (D1 of project-aware onboarding).
//!
//! Reads manifests at the project root and emits a multi-tag
//! [`ProjectClassification`]. The detector knows nothing about where
//! a framework's logs or runtime data live — the librarian
//! (`source_template` entries) answers that question. D1 only answers
//! "what type of project is this, and what versions does it declare?"
//!
//! Versions are captured verbatim from the manifest. For ecosystems
//! where the manifest is intrinsically a range (Gemfile -> Gemfile.lock,
//! npm `^x.y.z`), the manifest still wins; the lockfile is consulted
//! only when the manifest is unhelpful (currently: Gemfile.lock for
//! `rails`). Per spec, Cargo.toml versions are not captured.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use daemon8_types::{Platform, ProjectClassification};
use serde_json::Value;

pub fn classify(root: &Path) -> Result<ProjectClassification> {
    if !root.is_dir() {
        anyhow::bail!(
            "classify: project root {} is not an existing directory",
            root.display()
        );
    }

    let mut tags: Vec<String> = Vec::new();
    let mut framework_versions: BTreeMap<String, String> = BTreeMap::new();
    let mut manifests: BTreeMap<String, PathBuf> = BTreeMap::new();

    if root.join(".git").exists() {
        push_unique(&mut tags, "git-repo");
    }

    // Malformed manifests are warned-and-skipped: a single broken file
    // must not abort classification because other manifests can still
    // produce useful tags. The warn surface is uniform across every
    // parser path so users see one diagnostic style.
    if let Some(pkg_path) = manifest_path(root, "package.json") {
        manifests.insert("package.json".into(), pkg_path.clone());
        match read_json(&pkg_path) {
            Ok(pkg) => classify_package_json(&pkg, &mut tags, &mut framework_versions),
            Err(e) => tracing::warn!("daemon8: malformed {}: {e}", pkg_path.display()),
        }
    }

    if let Some(composer_path) = manifest_path(root, "composer.json") {
        manifests.insert("composer.json".into(), composer_path.clone());
        match read_json(&composer_path) {
            Ok(composer) => classify_composer_json(&composer, &mut tags, &mut framework_versions),
            Err(e) => tracing::warn!("daemon8: malformed {}: {e}", composer_path.display()),
        }
    }

    if let Some(cargo_path) = manifest_path(root, "Cargo.toml") {
        manifests.insert("Cargo.toml".into(), cargo_path.clone());
        match std::fs::read_to_string(&cargo_path) {
            Ok(text) => classify_cargo_toml(&text, &mut tags),
            Err(e) => tracing::warn!("daemon8: malformed {}: {e}", cargo_path.display()),
        }
    }

    if let Some(gemfile) = manifest_path(root, "Gemfile") {
        manifests.insert("Gemfile".into(), gemfile.clone());
        match std::fs::read_to_string(&gemfile) {
            Ok(text) => classify_gemfile(&text, &mut tags),
            Err(e) => tracing::warn!("daemon8: malformed {}: {e}", gemfile.display()),
        }
        if let Some(lock) = manifest_path(root, "Gemfile.lock") {
            manifests.insert("Gemfile.lock".into(), lock.clone());
            match std::fs::read_to_string(&lock) {
                Ok(text) => extract_gemfile_lock_versions(&text, &mut framework_versions),
                Err(e) => tracing::warn!("daemon8: malformed {}: {e}", lock.display()),
            }
        }
    }

    if let Some(pyproject) = manifest_path(root, "pyproject.toml") {
        manifests.insert("pyproject.toml".into(), pyproject.clone());
        match std::fs::read_to_string(&pyproject) {
            Ok(text) => classify_pyproject(&text, &mut tags, &mut framework_versions),
            Err(e) => tracing::warn!("daemon8: malformed {}: {e}", pyproject.display()),
        }
    }

    if let Some(requirements) = manifest_path(root, "requirements.txt") {
        manifests.insert("requirements.txt".into(), requirements.clone());
        match std::fs::read_to_string(&requirements) {
            Ok(text) => classify_requirements_txt(&text, &mut tags, &mut framework_versions),
            Err(e) => tracing::warn!("daemon8: malformed {}: {e}", requirements.display()),
        }
    }

    if let Some(gomod) = manifest_path(root, "go.mod") {
        manifests.insert("go.mod".into(), gomod);
        push_unique(&mut tags, "go");
    }

    // Config-only signals (no version extraction).
    for candidate in &["next.config.js", "next.config.ts", "next.config.mjs"] {
        if root.join(candidate).exists() {
            push_unique(&mut tags, "nextjs");
            manifests.insert((*candidate).into(), root.join(candidate));
            break;
        }
    }
    for candidate in &["vite.config.js", "vite.config.ts"] {
        if root.join(candidate).exists() {
            push_unique(&mut tags, "vite");
            manifests.insert((*candidate).into(), root.join(candidate));
            break;
        }
    }

    if let Some(app_json) = manifest_path(root, "app.json") {
        manifests.insert("app.json".into(), app_json.clone());
        if let Ok(val) = read_json(&app_json)
            && has_vega_or_kepler_block(&val)
        {
            push_unique(&mut tags, "vega");
            push_unique(&mut tags, "kepler");
        }
    }
    if root.join(".keplerproject").exists() {
        push_unique(&mut tags, "kepler");
        manifests.insert(".keplerproject".into(), root.join(".keplerproject"));
    }

    Ok(ProjectClassification {
        tags,
        framework_versions,
        root: root.to_path_buf(),
        manifests,
        platform: Platform::current(),
    })
}

// ── manifest parsing helpers ─────────────────────────────────────────

fn manifest_path(root: &Path, name: &str) -> Option<PathBuf> {
    let path = root.join(name);
    path.exists().then_some(path)
}

fn read_json(path: &Path) -> Result<Value> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing JSON from {}", path.display()))
}

fn push_unique(tags: &mut Vec<String>, tag: &str) {
    if !tags.iter().any(|t| t == tag) {
        tags.push(tag.to_string());
    }
}

// package.json signals: react-native, expo, next, vite, @tanstack/start.
// Versions captured from `dependencies` (preferred) or `devDependencies`.
fn classify_package_json(
    pkg: &Value,
    tags: &mut Vec<String>,
    versions: &mut BTreeMap<String, String>,
) {
    let dep_pools = ["dependencies", "devDependencies", "peerDependencies"];

    let lookup = |key: &str| -> Option<String> {
        for pool in dep_pools {
            if let Some(v) = pkg
                .get(pool)
                .and_then(|d| d.get(key))
                .and_then(|v| v.as_str())
            {
                return Some(v.to_string());
            }
        }
        None
    };

    if let Some(v) = lookup("react-native") {
        push_unique(tags, "react-native");
        versions.insert("react-native".into(), v);
    }
    if let Some(v) = lookup("expo") {
        push_unique(tags, "expo");
        versions.insert("expo".into(), v);
    }
    if let Some(v) = lookup("next") {
        push_unique(tags, "nextjs");
        versions.insert("next".into(), v);
    }
    if let Some(v) = lookup("vite") {
        push_unique(tags, "vite");
        versions.insert("vite".into(), v);
    }
    if let Some(v) = lookup("@tanstack/start").or_else(|| lookup("@tanstack/react-start")) {
        push_unique(tags, "tanstack-start");
        versions.insert("@tanstack/start".into(), v);
    }
}

// composer.json signals: laravel/framework, symfony/*. Versions read
// from the `require` map.
fn classify_composer_json(
    composer: &Value,
    tags: &mut Vec<String>,
    versions: &mut BTreeMap<String, String>,
) {
    let Some(require) = composer.get("require").and_then(|v| v.as_object()) else {
        return;
    };

    if let Some(v) = require.get("laravel/framework").and_then(|v| v.as_str()) {
        push_unique(tags, "laravel");
        versions.insert("laravel/framework".into(), v.to_string());
    }

    let has_symfony = require.keys().any(|k| k.starts_with("symfony/"));
    if has_symfony {
        push_unique(tags, "symfony");
        // Record the highest-priority symfony package version if present.
        for key in [
            "symfony/framework-bundle",
            "symfony/symfony",
            "symfony/runtime",
        ] {
            if let Some(v) = require.get(key).and_then(|v| v.as_str()) {
                versions.insert(key.into(), v.to_string());
                break;
            }
        }
    }
}

// Cargo.toml: tag `rust`, plus `rust-workspace` if a [workspace] section
// is present. Per spec, no version capture.
fn classify_cargo_toml(text: &str, tags: &mut Vec<String>) {
    push_unique(tags, "rust");
    if text.lines().any(|line| line.trim() == "[workspace]") {
        push_unique(tags, "rust-workspace");
    }
}

// Gemfile: presence of a `rails` gem line. Version comes from
// Gemfile.lock if available (handled separately).
fn classify_gemfile(text: &str, tags: &mut Vec<String>) {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("gem \"rails\"") || trimmed.starts_with("gem 'rails'") {
            push_unique(tags, "rails");
            break;
        }
    }
}

// Gemfile.lock format includes a top-level GEM/specs section with
// "    rails (x.y.z)" lines. Cheap to scrape without a full parser.
fn extract_gemfile_lock_versions(text: &str, versions: &mut BTreeMap<String, String>) {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("rails (")
            && let Some(end) = rest.find(')')
        {
            versions.insert("rails".into(), rest[..end].to_string());
            return;
        }
    }
}

// pyproject.toml dependency-extraction is best-effort: we only look at
// the common `[project] dependencies` and `[tool.poetry.dependencies]`
// surfaces, and we only care about a handful of frameworks. A full TOML
// parse would be overkill given we just need the version-suffix.
fn classify_pyproject(text: &str, tags: &mut Vec<String>, versions: &mut BTreeMap<String, String>) {
    push_unique(tags, "python");

    let frameworks = [
        ("django", "django"),
        ("flask", "flask"),
        ("fastapi", "fastapi"),
    ];

    for (needle, tag) in frameworks {
        if let Some(version) = extract_pyproject_version(text, needle) {
            push_unique(tags, tag);
            versions.insert(needle.into(), version);
        }
    }
}

// TODO(C3): replace this substring-with-word-boundary scan with a real
// TOML/requirements parser. The current approach is a near-term
// workaround for the bug where `django-rest-framework==4.0` would
// match the `django` package and emit a wrong tag+version.
fn extract_pyproject_version(text: &str, package: &str) -> Option<String> {
    // PEP 621: dependencies = ["django>=4.2", ...]
    // Poetry:  django = "^4.2"
    // Either way, we look for a line that mentions the package name and
    // capture whatever version specifier follows. Best-effort.
    for raw in text.lines() {
        let line = raw.trim();
        let lc = line.to_ascii_lowercase();
        let Some(pos) = find_package_with_boundary(&lc, package) else {
            continue;
        };
        let after = &line[pos + package.len()..];
        let trimmed = after.trim_start_matches(['"', '\'', ' ', '\t']);
        // Match PEP 508 specifiers (>=, ==, ~=, !=) or poetry-style (=).
        let specifier = trimmed.trim_start_matches(['=', '>', '<', '~', '!', '^', ' ', '"', '\'']);
        // The version itself ends at the first quote/comma/space after digits.
        let end = specifier
            .find(['"', '\'', ',', ']'])
            .unwrap_or(specifier.len());
        let version_part = specifier[..end].trim();
        if !version_part.is_empty() && version_part.chars().any(|c| c.is_ascii_digit()) {
            return Some(version_part.to_string());
        }
    }
    None
}

// Find `package` in `haystack` (already lowercased) at a position where
// the character immediately after the package name is one of the
// recognized separators for PEP 508 / Poetry / requirements.txt syntax,
// or end-of-line. Prevents `django-rest-framework` from matching
// `django`.
fn find_package_with_boundary(haystack: &str, package: &str) -> Option<usize> {
    let mut start = 0;
    while let Some(rel) = haystack[start..].find(package) {
        let pos = start + rel;
        let after_idx = pos + package.len();
        let next_char = haystack[after_idx..].chars().next();
        if is_package_boundary(next_char) {
            return Some(pos);
        }
        start = after_idx;
    }
    None
}

fn is_package_boundary(next: Option<char>) -> bool {
    match next {
        None => true,
        Some(c) => matches!(
            c,
            '[' | '\'' | '"' | ' ' | '=' | '>' | '<' | '~' | '!' | '^' | '\t' | ',' | ']' | ';'
        ),
    }
}

// requirements.txt: one `package==version` per line, plus `>=`, `~=`, etc.
fn classify_requirements_txt(
    text: &str,
    tags: &mut Vec<String>,
    versions: &mut BTreeMap<String, String>,
) {
    push_unique(tags, "python");

    let frameworks = [
        ("django", "django"),
        ("flask", "flask"),
        ("fastapi", "fastapi"),
    ];

    for line in text.lines() {
        let stripped = line.split('#').next().unwrap_or("").trim();
        if stripped.is_empty() {
            continue;
        }
        let lc = stripped.to_ascii_lowercase();
        for (needle, tag) in frameworks {
            // Word-boundary aware: require the char after the needle to
            // be a recognized package-name terminator so that lines like
            // `django-cors-headers==4.0` don't trigger the `django` tag.
            // TODO(C3): replace with proper requirements parser.
            if !lc.starts_with(needle) {
                continue;
            }
            let next = lc[needle.len()..].chars().next();
            if !is_package_boundary(next) {
                continue;
            }
            push_unique(tags, tag);
            let after = &stripped[needle.len()..];
            let specifier = after.trim_start_matches(['=', '>', '<', '~', '!', ' ']);
            let end = specifier.find([' ', ',', ';']).unwrap_or(specifier.len());
            let version_part = specifier[..end].trim();
            if !version_part.is_empty() {
                versions.insert(needle.into(), version_part.to_string());
            }
        }
    }
}

fn has_vega_or_kepler_block(app_json: &Value) -> bool {
    if app_json.get("vega").is_some() || app_json.get("kepler").is_some() {
        return true;
    }
    if let Some(expo) = app_json.get("expo") {
        return expo.get("vega").is_some() || expo.get("kepler").is_some();
    }
    false
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("sample-projects")
    }

    #[test]
    fn react_native_vega_project_emits_all_expected_tags() {
        let result = classify(&fixtures_root().join("react-native-rtntv")).unwrap();
        assert_eq!(result.tags, vec!["react-native", "vega", "kepler"]);
        assert_eq!(
            result
                .framework_versions
                .get("react-native")
                .map(String::as_str),
            Some("0.74.5")
        );
    }

    #[test]
    fn laravel_project_emits_laravel_tag_and_version() {
        let result = classify(&fixtures_root().join("laravel-rcn")).unwrap();
        assert_eq!(result.tags, vec!["laravel"]);
        assert_eq!(
            result
                .framework_versions
                .get("laravel/framework")
                .map(String::as_str),
            Some("^11.0")
        );
    }

    #[test]
    fn rust_workspace_emits_both_rust_tags_and_no_version() {
        let result = classify(&fixtures_root().join("rust-workspace-daemon8")).unwrap();
        assert_eq!(result.tags, vec!["rust", "rust-workspace"]);
        assert!(
            result.framework_versions.is_empty(),
            "Cargo versions not captured per spec; got: {:?}",
            result.framework_versions
        );
    }

    #[test]
    fn expo_project_emits_both_react_native_and_expo_with_versions() {
        let result = classify(&fixtures_root().join("expo-blank")).unwrap();
        assert_eq!(result.tags, vec!["react-native", "expo"]);
        assert_eq!(
            result.framework_versions.get("expo").map(String::as_str),
            Some("~52.0.0")
        );
        assert_eq!(
            result
                .framework_versions
                .get("react-native")
                .map(String::as_str),
            Some("0.76.0")
        );
    }

    #[test]
    fn symfony_project_emits_symfony_tag() {
        let result = classify(&fixtures_root().join("mixed-symfony-php")).unwrap();
        assert!(
            result.tags.contains(&"symfony".to_string()),
            "tags: {:?}",
            result.tags
        );
    }

    #[test]
    fn classify_records_platform() {
        let result = classify(&fixtures_root().join("rust-workspace-daemon8")).unwrap();
        assert_eq!(result.platform, Platform::current());
    }

    #[test]
    fn classify_returns_err_for_nonexistent_root() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let err = classify(&missing).unwrap_err();
        assert!(
            err.to_string().contains("not an existing directory"),
            "err: {err}"
        );
    }

    #[test]
    fn classify_returns_err_for_file_root() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("not-a-dir");
        std::fs::write(&file, "junk").unwrap();
        let err = classify(&file).unwrap_err();
        assert!(
            err.to_string().contains("not an existing directory"),
            "err: {err}"
        );
    }

    #[test]
    fn pyproject_django_rest_framework_does_not_emit_django_tag() {
        let mut tags = vec![];
        let mut versions = BTreeMap::new();
        let text = r#"[project]
dependencies = ["django-rest-framework==4.0", "requests==2.31"]
"#;
        classify_pyproject(text, &mut tags, &mut versions);
        assert!(tags.contains(&"python".to_string()));
        assert!(
            !tags.contains(&"django".to_string()),
            "django tag must not leak from django-rest-framework match; tags: {tags:?}"
        );
        assert!(
            !versions.contains_key("django"),
            "django version must not be captured; versions: {versions:?}"
        );
    }

    #[test]
    fn pyproject_fastapi_utils_does_not_emit_fastapi_tag() {
        let mut tags = vec![];
        let mut versions = BTreeMap::new();
        let text = r#"[tool.poetry.dependencies]
fastapi-utils = "^0.2"
"#;
        classify_pyproject(text, &mut tags, &mut versions);
        assert!(
            !tags.contains(&"fastapi".to_string()),
            "fastapi tag must not leak from fastapi-utils; tags: {tags:?}"
        );
    }

    #[test]
    fn requirements_django_cors_headers_does_not_emit_django_tag() {
        let mut tags = vec![];
        let mut versions = BTreeMap::new();
        classify_requirements_txt(
            "django-cors-headers==4.0.0\nrequests==2.31.0\n",
            &mut tags,
            &mut versions,
        );
        assert!(
            !tags.contains(&"django".to_string()),
            "django tag must not leak from django-cors-headers; tags: {tags:?}"
        );
        assert!(
            !versions.contains_key("django"),
            "django version must not be captured; versions: {versions:?}"
        );
    }

    #[test]
    fn requirements_flask_restful_does_not_emit_flask_tag() {
        let mut tags = vec![];
        let mut versions = BTreeMap::new();
        classify_requirements_txt("flask-restful==0.3.10\n", &mut tags, &mut versions);
        assert!(
            !tags.contains(&"flask".to_string()),
            "flask tag must not leak from flask-restful; tags: {tags:?}"
        );
    }

    #[test]
    fn mixed_node_and_rust_project_emits_both_ecosystems() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("package.json"),
            r#"{"dependencies":{"react-native":"0.74.0"}}"#,
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let result = classify(tmp.path()).unwrap();
        assert!(result.tags.contains(&"react-native".to_string()));
        assert!(result.tags.contains(&"rust".to_string()));
    }

    #[test]
    fn nextjs_detected_from_js_config() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("next.config.js"), "module.exports = {};").unwrap();
        let result = classify(tmp.path()).unwrap();
        assert!(result.tags.contains(&"nextjs".to_string()));
    }

    #[test]
    fn nextjs_detected_from_ts_config() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("next.config.ts"), "export default {};").unwrap();
        let result = classify(tmp.path()).unwrap();
        assert!(result.tags.contains(&"nextjs".to_string()));
    }

    #[test]
    fn nextjs_detected_from_mjs_config() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("next.config.mjs"), "export default {};").unwrap();
        let result = classify(tmp.path()).unwrap();
        assert!(result.tags.contains(&"nextjs".to_string()));
    }

    #[test]
    fn vite_detected_from_config() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("package.json"),
            r#"{"devDependencies":{"vite":"^5.0.0"}}"#,
        )
        .unwrap();
        let result = classify(tmp.path()).unwrap();
        assert!(result.tags.contains(&"vite".to_string()));
    }

    #[test]
    fn python_django_inline_project() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("requirements.txt"),
            "django==4.2.7\nrequests==2.31.0\n",
        )
        .unwrap();
        let result = classify(tmp.path()).unwrap();
        assert!(result.tags.contains(&"python".to_string()));
        assert!(result.tags.contains(&"django".to_string()));
    }

    #[test]
    fn python_flask_inline_project() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("requirements.txt"), "flask==3.0.0\n").unwrap();
        let result = classify(tmp.path()).unwrap();
        assert!(result.tags.contains(&"flask".to_string()));
    }

    #[test]
    fn rails_inline_project() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Gemfile"), "gem \"rails\", \"~> 7.1\"\n").unwrap();
        let result = classify(tmp.path()).unwrap();
        assert!(result.tags.contains(&"rails".to_string()));
    }

    #[test]
    fn go_inline_project() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("go.mod"), "module example.com/x\ngo 1.22\n").unwrap();
        let result = classify(tmp.path()).unwrap();
        assert!(result.tags.contains(&"go".to_string()));
    }

    #[test]
    fn classify_on_empty_dir_yields_empty_tags() {
        let tmp = tempfile::tempdir().unwrap();
        let result = classify(tmp.path()).unwrap();
        assert!(result.tags.is_empty(), "tags: {:?}", result.tags);
        assert!(result.framework_versions.is_empty());
        assert!(result.manifests.is_empty());
    }

    #[test]
    fn gemfile_lock_version_takes_precedence() {
        let mut versions = BTreeMap::new();
        extract_gemfile_lock_versions(
            "GEM\n  specs:\n    rails (7.1.3)\n    actionpack (7.1.3)\n",
            &mut versions,
        );
        assert_eq!(versions.get("rails").map(String::as_str), Some("7.1.3"));
    }

    #[test]
    fn pyproject_version_extraction_handles_poetry_style() {
        let text = r#"[tool.poetry.dependencies]
python = "^3.11"
django = "^5.0"
"#;
        let v = extract_pyproject_version(text, "django");
        assert_eq!(v.as_deref(), Some("5.0"));
    }

    #[test]
    fn requirements_txt_extracts_pinned_versions() {
        let mut tags = vec![];
        let mut versions = BTreeMap::new();
        classify_requirements_txt(
            "django==4.2.7\nrequests==2.31.0\n",
            &mut tags,
            &mut versions,
        );
        assert!(tags.contains(&"django".to_string()));
        assert_eq!(versions.get("django").map(String::as_str), Some("4.2.7"));
    }
}
