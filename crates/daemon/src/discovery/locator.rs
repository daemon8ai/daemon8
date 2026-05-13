// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! Locator pattern expansion (shared between the scanner and doctor).
//!
//! `expand_locator_pattern` is the canonical interpreter for the
//! `source_template.locator_pattern` shape: `~`, `<root>`, and
//! `$VAR`/`${VAR}` references resolve, and glob metacharacters trigger
//! filesystem expansion. The function is pure — it never registers a
//! source or writes to the librarian.
//!
//! Errors are returned as `String` rather than a dedicated enum so each
//! caller can fold the diagnostic into whatever local error type it
//! already exposes (scanner -> `TemplateMissReason::InvalidPattern`,
//! doctor -> `CheckResult::Err`). That keeps the two consumers from
//! becoming coupled through a shared error enum that one of them
//! doesn't need.

use std::path::{Path, PathBuf};

/// Expand `~`, `<root>`, and `$VAR`/`${VAR}` references in a locator
/// pattern, then run glob expansion if the pattern contains glob
/// metacharacters. Returns `Err(message)` for malformed patterns —
/// callers wrap the message into their own error type.
///
/// Existence is **not** verified here. A non-existent literal path
/// round-trips successfully; the caller decides what to do about it.
pub fn expand_locator_pattern(pattern: &str, root: &Path) -> Result<Vec<PathBuf>, String> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Err("pattern is empty".into());
    }

    let after_home = expand_home(trimmed)?;
    let after_root = after_home.replace("<root>", &root.to_string_lossy());
    let after_env = expand_env_vars(&after_root)?;

    if has_glob_chars(&after_env) {
        let mut paths = Vec::new();
        let entries = glob::glob(&after_env).map_err(|e| format!("glob parse error: {e}"))?;
        for entry in entries {
            match entry {
                Ok(p) => paths.push(p),
                Err(e) => {
                    tracing::warn!(
                        pattern = %after_env,
                        "glob entry error: {e}"
                    );
                }
            }
        }
        Ok(paths)
    } else {
        Ok(vec![PathBuf::from(after_env)])
    }
}

fn expand_home(pattern: &str) -> Result<String, String> {
    if let Some(rest) = pattern.strip_prefix("~/") {
        let home = dirs::home_dir().ok_or_else(|| "home directory unavailable".to_string())?;
        Ok(format!("{}/{rest}", home.display()))
    } else if pattern == "~" {
        let home = dirs::home_dir().ok_or_else(|| "home directory unavailable".to_string())?;
        Ok(home.display().to_string())
    } else {
        Ok(pattern.to_string())
    }
}

fn expand_env_vars(input: &str) -> Result<String, String> {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        // ${VAR} form
        if chars.peek() == Some(&'{') {
            chars.next();
            let mut name = String::new();
            let mut closed = false;
            for nc in chars.by_ref() {
                if nc == '}' {
                    closed = true;
                    break;
                }
                name.push(nc);
            }
            if !closed {
                return Err(format!("unterminated ${{}} reference near {name}"));
            }
            let value = std::env::var(&name)
                .map_err(|_| format!("environment variable ${name} is not set"))?;
            out.push_str(&value);
            continue;
        }
        // $VAR form (alphanumeric + underscore)
        let mut name = String::new();
        while let Some(&nc) = chars.peek() {
            if nc.is_ascii_alphanumeric() || nc == '_' {
                name.push(nc);
                chars.next();
            } else {
                break;
            }
        }
        if name.is_empty() {
            // Bare `$` with no name — keep literal.
            out.push('$');
            continue;
        }
        let value =
            std::env::var(&name).map_err(|_| format!("environment variable ${name} is not set"))?;
        out.push_str(&value);
    }
    Ok(out)
}

fn has_glob_chars(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '*' | '?' | '['))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_home_tilde_slash() {
        let expanded = expand_locator_pattern("~/example.log", Path::new("/tmp/root")).unwrap();
        assert_eq!(expanded.len(), 1);
        let s = expanded[0].to_string_lossy();
        assert!(s.ends_with("/example.log"));
        assert!(!s.starts_with('~'));
    }

    #[test]
    fn expand_root_placeholder() {
        let expanded =
            expand_locator_pattern("<root>/logs/runtime.log", Path::new("/tmp/proj")).unwrap();
        assert_eq!(expanded, vec![PathBuf::from("/tmp/proj/logs/runtime.log")]);
    }

    #[test]
    fn expand_env_var_braced() {
        // Safe to set in tests — we set and read in the same process.
        unsafe { std::env::set_var("D8_LOCATOR_TEST_VAR", "/var/tmp/d8") };
        let expanded =
            expand_locator_pattern("${D8_LOCATOR_TEST_VAR}/x.log", Path::new("/")).unwrap();
        assert_eq!(expanded, vec![PathBuf::from("/var/tmp/d8/x.log")]);
        unsafe { std::env::remove_var("D8_LOCATOR_TEST_VAR") };
    }

    #[test]
    fn expand_env_var_bare() {
        unsafe { std::env::set_var("D8_LOCATOR_TEST_VAR2", "/srv/d8") };
        let expanded =
            expand_locator_pattern("$D8_LOCATOR_TEST_VAR2/y.log", Path::new("/")).unwrap();
        assert_eq!(expanded, vec![PathBuf::from("/srv/d8/y.log")]);
        unsafe { std::env::remove_var("D8_LOCATOR_TEST_VAR2") };
    }

    #[test]
    fn expand_missing_env_var_is_error() {
        unsafe { std::env::remove_var("D8_LOCATOR_MISSING_VAR_XYZ") };
        let err =
            expand_locator_pattern("$D8_LOCATOR_MISSING_VAR_XYZ/x", Path::new("/")).unwrap_err();
        assert!(err.contains("D8_LOCATOR_MISSING_VAR_XYZ"));
    }

    #[test]
    fn expand_empty_pattern_rejected() {
        let err = expand_locator_pattern("   ", Path::new("/tmp")).unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn expand_glob_expands_against_filesystem() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.log"), "x").unwrap();
        std::fs::write(tmp.path().join("b.log"), "y").unwrap();
        std::fs::write(tmp.path().join("c.txt"), "z").unwrap();

        let pattern = format!("{}/*.log", tmp.path().display());
        let mut expanded = expand_locator_pattern(&pattern, Path::new("/")).unwrap();
        expanded.sort();
        assert_eq!(expanded.len(), 2);
        assert!(
            expanded
                .iter()
                .all(|p| p.extension().is_some_and(|e| e == "log"))
        );
    }

    #[test]
    fn expand_literal_no_glob_no_filesystem_check() {
        // expand_locator_pattern does NOT verify existence — that's the
        // caller's job. A non-existent literal path round-trips.
        let expanded =
            expand_locator_pattern("/this/path/does/not/exist.log", Path::new("/")).unwrap();
        assert_eq!(
            expanded,
            vec![PathBuf::from("/this/path/does/not/exist.log")]
        );
    }
}
