// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

//! `daemon8 init` -- scaffold `.daemon8-cli.toml` at cwd.
//!
//! Schema reference: https://daemon8.ai/docs/cli-hook-config

use std::env;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::cli_config::PROJECT_CONFIG_FILENAME;

#[derive(clap::Args, Default)]
pub struct InitArgs {
    /// Preset role for this project: queen | worker | solo | watchdog.
    #[arg(long, default_value = "solo")]
    pub role: String,

    /// Overwrite an existing `.daemon8-cli.toml` at this location.
    #[arg(long)]
    pub force: bool,

    /// Explicit project slug. Defaults to the cwd basename.
    #[arg(long)]
    pub slug: Option<String>,
}

pub fn cmd_init(args: InitArgs) -> Result<()> {
    let cwd = env::current_dir().context("cannot read current working directory")?;
    let target = cwd.join(PROJECT_CONFIG_FILENAME);

    if target.exists() && !args.force {
        bail!(
            "{} already exists; pass --force to overwrite",
            target.display()
        );
    }

    let role = match args.role.as_str() {
        "queen" | "worker" | "solo" | "watchdog" => args.role.clone(),
        other => bail!("invalid --role '{other}'; expected one of: queen, worker, solo, watchdog"),
    };

    let slug = args.slug.unwrap_or_else(|| derive_slug(&cwd));
    let contents = render_template(&slug, &role);

    std::fs::write(&target, contents)
        .with_context(|| format!("failed to write {}", target.display()))?;

    println!("wrote {}", target.display());
    println!("role: {role}");
    println!("slug: {slug}");
    println!();
    println!("register the daemon8 cli-hook in your CLI's settings to begin enrollment.");
    println!("docs: https://daemon8.ai/docs/cli-hook");
    Ok(())
}

fn derive_slug(cwd: &Path) -> String {
    cwd.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string()
}

fn render_template(slug: &str, role: &str) -> String {
    format!(
        r##"# Daemon8 CLI hook configuration.
# Schema reference: https://daemon8.ai/docs/cli-hook-config

[project]
slug = "{slug}"
role_default = "{role}"

[enrollment]
enabled = true
scope = []

[features]
intents = true
inbox = true
state_tracking = true
compaction_recovery = true
heartbeat_interval = "30s"

[intents.auto_declare]
expertise = []

[distillation]
track_todowrite = true
track_git_commits = true
track_file_writes = true
coarsen_file_writes_below_threshold = 5
"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_includes_slug_and_role() {
        let out = render_template("my-proj", "worker");
        assert!(out.contains(r#"slug = "my-proj""#));
        assert!(out.contains(r#"role_default = "worker""#));
    }

    #[test]
    fn derive_slug_uses_basename() {
        let p = std::path::PathBuf::from("/tmp/foo/bar");
        assert_eq!(derive_slug(&p), "bar");
    }
}
