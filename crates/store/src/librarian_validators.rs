// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Per-kind validators for `LibrarianNode.data` payloads.
//!
//! The librarian schema (SurrealDB `catalog_node`) carries only the
//! cross-cutting columns (kind, label, locator, tags, timestamps).
//! Kind-specific shapes live in the `data: option<object>` field. These
//! validators enforce the per-kind contracts in code, called from
//! `SurrealLibrarianStore::index_node` before the write.
//!
//! Two contracts matter most:
//!
//! 1. **Portability rules on `locator_pattern`** (source_template).
//!    Templates must use `~`, env-var references (`$VAR`), or `<root>`.
//!    Absolute home paths like `/Users/jhavens/...` leak machine
//!    identity into the librarian and break future export/sync.
//!
//! 2. **Tag and platform sanity.** Empty `platforms` arrays mean the
//!    template would never match anything; unknown `project_types`
//!    tags would never match the D1 detector's output. Reject both at
//!    write time rather than letting them rot in the database.

use daemon8_types::{ProjectNodeData, SourceInstanceData, SourceTemplateData};

use crate::StoreError;

/// Project type tags the D1 detector currently emits. Validators reject
/// `project_types` entries outside this set on a write. Conservative on
/// purpose — adding a new tag is a code change in `project_type.rs` plus
/// one line here, and that pairing is exactly what we want so templates
/// can never reference a tag the detector won't produce.
///
/// The universal `any` tag matches every project regardless of
/// classification (used by conversation source_templates per D5).
pub const KNOWN_PROJECT_TYPE_TAGS: &[&str] = &[
    "any",
    "git-repo",
    "react-native",
    "expo",
    "vega",
    "kepler",
    "nextjs",
    "vite",
    "tanstack-start",
    "rust",
    "rust-workspace",
    "laravel",
    "symfony",
    "python",
    "django",
    "flask",
    "fastapi",
    "go",
    "rails",
];

pub fn validate_source_template_data(data: &SourceTemplateData) -> Result<(), StoreError> {
    if data.project_types.is_empty() {
        return Err(StoreError::Other(
            "source_template.project_types must not be empty".into(),
        ));
    }

    for tag in &data.project_types {
        if !KNOWN_PROJECT_TYPE_TAGS.contains(&tag.as_str()) {
            return Err(StoreError::Other(format!(
                "source_template.project_types contains unknown tag '{tag}'; \
                 known tags: {}",
                KNOWN_PROJECT_TYPE_TAGS.join(", ")
            )));
        }
    }

    if data.platforms.is_empty() {
        return Err(StoreError::Other(
            "source_template.platforms must not be empty (explicit platforms required for portability)".into(),
        ));
    }

    validate_locator_pattern(&data.locator_pattern)?;

    if data.description.trim().is_empty() {
        return Err(StoreError::Other(
            "source_template.description must not be empty".into(),
        ));
    }

    Ok(())
}

// source_instance is a per-machine concrete path; portability rules
// that bind source_template do not apply (the instance was resolved
// against this filesystem on purpose). The validator only guards
// against obviously-broken data that would render the node useless:
// an empty path, or a path that cannot be expressed as a string.
pub fn validate_source_instance_data(data: &SourceInstanceData) -> Result<(), StoreError> {
    if data.resolved_path.as_os_str().is_empty() {
        return Err(StoreError::Other(
            "source_instance.resolved_path must not be empty".into(),
        ));
    }
    Ok(())
}

pub fn validate_project_node_data(data: &ProjectNodeData) -> Result<(), StoreError> {
    if data.slug.trim().is_empty() {
        return Err(StoreError::Other("project.slug must not be empty".into()));
    }

    if data.classification_tags.is_empty() {
        return Err(StoreError::Other(
            "project.classification_tags must not be empty".into(),
        ));
    }

    for tag in &data.classification_tags {
        if !KNOWN_PROJECT_TYPE_TAGS.contains(&tag.as_str()) {
            return Err(StoreError::Other(format!(
                "project.classification_tags contains unknown tag '{tag}'"
            )));
        }
    }

    if data.root_path.as_os_str().is_empty() {
        return Err(StoreError::Other(
            "project.root_path must not be empty".into(),
        ));
    }

    Ok(())
}

// locator_pattern must be portable across machines: use `~` for the
// user home, env-var references for OS-specific roots, or `<root>` for
// project-relative paths. Absolute paths under `/Users/<name>/`,
// `/home/<name>/`, any Windows drive `X:\Users\` or `X:/Users/`, or
// UNC paths (`\\server\share`) would silently break future export/sync
// and pin the template to one machine. Reject them with a clear
// message naming the offending shape.
fn validate_locator_pattern(pattern: &str) -> Result<(), StoreError> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Err(StoreError::Other(
            "source_template.locator_pattern must not be empty".into(),
        ));
    }

    if trimmed.starts_with("/Users/") || trimmed.starts_with("/home/") {
        return Err(StoreError::Other(format!(
            "source_template.locator_pattern '{trimmed}' is an absolute home path; \
             use ~ for the user home (portability requirement)"
        )));
    }

    if is_windows_user_path(trimmed) {
        return Err(StoreError::Other(format!(
            "source_template.locator_pattern '{trimmed}' is an absolute Windows user path; \
             use $LOCALAPPDATA or $USERPROFILE (portability requirement)"
        )));
    }

    if is_unc_path(trimmed) {
        return Err(StoreError::Other(format!(
            "source_template.locator_pattern '{trimmed}' is a UNC network path; \
             templates must not embed remote shares (portability requirement)"
        )));
    }

    Ok(())
}

// Matches `<drive>:\Users\…` or `<drive>:/Users/…` for any single
// ASCII letter drive. Case-insensitive on the `Users` segment because
// Windows is case-insensitive on paths.
fn is_windows_user_path(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    if bytes.len() < 9 {
        return false;
    }
    if !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
        return false;
    }
    let sep = bytes[2];
    if sep != b'\\' && sep != b'/' {
        return false;
    }
    let tail = &pattern[3..];
    let needle = if sep == b'\\' { "Users\\" } else { "Users/" };
    tail.len() >= needle.len() && tail[..needle.len()].eq_ignore_ascii_case(needle)
}

// UNC: `\\server\share\…`. Two leading backslashes followed by a
// non-separator character is enough to identify it; we don't try to
// validate the full UNC grammar.
fn is_unc_path(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    bytes.len() >= 3
        && bytes[0] == b'\\'
        && bytes[1] == b'\\'
        && bytes[2] != b'\\'
        && bytes[2] != b'/'
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use daemon8_types::{
        Platform, ProjectNodeData, SourceInstanceData, SourceKind, SourceTemplateData,
        TemplateConfidence,
    };

    use super::*;

    fn good_template() -> SourceTemplateData {
        SourceTemplateData {
            project_types: vec!["react-native".into()],
            kind: SourceKind::Log,
            locator_pattern: "~/Library/Logs/example.log".into(),
            platforms: vec![Platform::Macos],
            parser_hint: None,
            default_tags: vec!["example".into()],
            description: "example log".into(),
            version_constraint: None,
            discovered_by_session: None,
            discovered_by_provider: None,
            discovered_at_ns: 0,
            verified_count: 0,
            last_verified_at_ns: 0,
            confidence: TemplateConfidence::AgentDiscovered,
        }
    }

    fn good_project() -> ProjectNodeData {
        ProjectNodeData {
            root_path: PathBuf::from("/tmp/sample"),
            slug: "sample".into(),
            classification_tags: vec!["rust".into()],
            framework_versions: BTreeMap::new(),
            platform: Platform::Macos,
            created_at_ns: 0,
            last_serve_at_ns: 0,
            skip_discovery: false,
        }
    }

    #[test]
    fn accepts_well_formed_template() {
        validate_source_template_data(&good_template()).unwrap();
    }

    #[test]
    fn rejects_absolute_home_path() {
        let mut t = good_template();
        t.locator_pattern = "/Users/jhavens/Library/Logs/example.log".into();
        let err = validate_source_template_data(&t).unwrap_err();
        assert!(err.to_string().contains("absolute home path"));
    }

    #[test]
    fn rejects_absolute_linux_home_path() {
        let mut t = good_template();
        t.locator_pattern = "/home/jhavens/.cache/example".into();
        let err = validate_source_template_data(&t).unwrap_err();
        assert!(err.to_string().contains("absolute home path"));
    }

    #[test]
    fn rejects_windows_user_path() {
        let mut t = good_template();
        t.locator_pattern = "C:\\Users\\jhavens\\AppData\\example.log".into();
        let err = validate_source_template_data(&t).unwrap_err();
        assert!(err.to_string().contains("Windows user path"));
    }

    #[test]
    fn rejects_d_drive_users_path() {
        let mut t = good_template();
        t.locator_pattern = "D:\\Users\\jhavens\\AppData\\example.log".into();
        let err = validate_source_template_data(&t).unwrap_err();
        assert!(err.to_string().contains("Windows user path"));
    }

    #[test]
    fn rejects_capital_c_forward_slash_users_path() {
        let mut t = good_template();
        t.locator_pattern = "C:/Users/jhavens/AppData/example.log".into();
        let err = validate_source_template_data(&t).unwrap_err();
        assert!(err.to_string().contains("Windows user path"));
    }

    #[test]
    fn rejects_lowercase_drive_letter_windows_user_path() {
        let mut t = good_template();
        t.locator_pattern = "e:\\users\\jhavens\\log.txt".into();
        let err = validate_source_template_data(&t).unwrap_err();
        assert!(err.to_string().contains("Windows user path"));
    }

    #[test]
    fn rejects_unc_path() {
        let mut t = good_template();
        t.locator_pattern = "\\\\fileserver\\share\\app\\log.txt".into();
        let err = validate_source_template_data(&t).unwrap_err();
        assert!(err.to_string().contains("UNC network path"));
    }

    #[test]
    fn accepts_non_user_windows_drive_path() {
        // We only reject home-shaped Windows paths; arbitrary absolute
        // drive paths (e.g. C:\ProgramData\…) still come through because
        // they are sometimes the only portable answer on Windows.
        let mut t = good_template();
        t.locator_pattern = "C:\\ProgramData\\daemon8\\runtime.log".into();
        t.platforms = vec![Platform::Windows];
        validate_source_template_data(&t).unwrap();
    }

    #[test]
    fn rejects_empty_platforms() {
        let mut t = good_template();
        t.platforms = vec![];
        let err = validate_source_template_data(&t).unwrap_err();
        assert!(err.to_string().contains("platforms must not be empty"));
    }

    #[test]
    fn rejects_unknown_project_type_tag() {
        let mut t = good_template();
        t.project_types = vec!["nonexistent-framework".into()];
        let err = validate_source_template_data(&t).unwrap_err();
        assert!(
            err.to_string()
                .contains("unknown tag 'nonexistent-framework'")
        );
    }

    #[test]
    fn rejects_empty_project_types() {
        let mut t = good_template();
        t.project_types = vec![];
        let err = validate_source_template_data(&t).unwrap_err();
        assert!(err.to_string().contains("project_types must not be empty"));
    }

    #[test]
    fn rejects_empty_locator_pattern() {
        let mut t = good_template();
        t.locator_pattern = "   ".into();
        let err = validate_source_template_data(&t).unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn rejects_empty_description() {
        let mut t = good_template();
        t.description = " ".into();
        let err = validate_source_template_data(&t).unwrap_err();
        assert!(err.to_string().contains("description must not be empty"));
    }

    #[test]
    fn accepts_root_placeholder_locator() {
        let mut t = good_template();
        t.locator_pattern = "<root>/logs/runtime.log".into();
        validate_source_template_data(&t).unwrap();
    }

    #[test]
    fn accepts_env_var_locator() {
        let mut t = good_template();
        t.locator_pattern = "$LOCALAPPDATA/example/log.txt".into();
        t.platforms = vec![Platform::Windows];
        validate_source_template_data(&t).unwrap();
    }

    #[test]
    fn accepts_any_universal_tag() {
        let mut t = good_template();
        t.project_types = vec!["any".into()];
        validate_source_template_data(&t).unwrap();
    }

    #[test]
    fn accepts_well_formed_project() {
        validate_project_node_data(&good_project()).unwrap();
    }

    #[test]
    fn rejects_empty_project_slug() {
        let mut p = good_project();
        p.slug = "  ".into();
        let err = validate_project_node_data(&p).unwrap_err();
        assert!(err.to_string().contains("slug must not be empty"));
    }

    #[test]
    fn rejects_empty_classification_tags() {
        let mut p = good_project();
        p.classification_tags = vec![];
        let err = validate_project_node_data(&p).unwrap_err();
        assert!(
            err.to_string()
                .contains("classification_tags must not be empty")
        );
    }

    #[test]
    fn rejects_unknown_classification_tag() {
        let mut p = good_project();
        p.classification_tags = vec!["mystery-framework".into()];
        let err = validate_project_node_data(&p).unwrap_err();
        assert!(err.to_string().contains("unknown tag 'mystery-framework'"));
    }

    fn good_instance() -> SourceInstanceData {
        SourceInstanceData {
            kind: SourceKind::Log,
            resolved_path: PathBuf::from("/tmp/sample/runtime.log"),
            parser: Some("line".into()),
            tags: vec!["fixture".into()],
            version_constraint: None,
            registered_at_ns: 0,
            last_verified_at_ns: 0,
        }
    }

    #[test]
    fn accepts_well_formed_source_instance() {
        validate_source_instance_data(&good_instance()).unwrap();
    }

    #[test]
    fn rejects_empty_resolved_path() {
        let mut i = good_instance();
        i.resolved_path = PathBuf::new();
        let err = validate_source_instance_data(&i).unwrap_err();
        assert!(err.to_string().contains("resolved_path must not be empty"));
    }
}
