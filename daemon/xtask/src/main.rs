// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::{
    env, fs,
    path::PathBuf,
    process::{Command, ExitCode},
};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let task = args.first().map(|s| s.as_str()).unwrap_or("");

    match task {
        "deploy-local" => deploy_local(),
        _ => {
            eprintln!("Usage: cargo xtask <task>");
            eprintln!();
            eprintln!("Tasks:");
            eprintln!("  deploy-local    Build release binary and install to ~/.cargo/bin");
            ExitCode::FAILURE
        }
    }
}

fn deploy_local() -> ExitCode {
    let workspace_dir = workspace_root();

    eprintln!("Building daemon8 (release)...");
    let status = Command::new("cargo")
        .args(["build", "--release", "-p", "daemon8"])
        .current_dir(&workspace_dir)
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!("Build failed with exit code: {}", s);
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("Failed to run cargo build: {e}");
            return ExitCode::FAILURE;
        }
    }

    let src = workspace_dir.join("target/release/daemon8");
    let dest = cargo_bin_dir().join("daemon8");

    eprintln!("Installing to {}...", dest.display());
    if let Err(e) = fs::copy(&src, &dest) {
        eprintln!("Failed to copy binary: {e}");
        return ExitCode::FAILURE;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o755));
    }

    if cfg!(target_os = "macos") {
        macos_service_restart();
    }

    eprintln!("Done. Binary: {}", dest.display());
    ExitCode::SUCCESS
}

fn workspace_root() -> PathBuf {
    let output = Command::new("cargo")
        .args(["locate-project", "--workspace", "--message-format=plain"])
        .output()
        .expect("failed to run cargo locate-project");
    let path = String::from_utf8(output.stdout).unwrap();
    PathBuf::from(path.trim()).parent().unwrap().to_path_buf()
}

fn cargo_bin_dir() -> PathBuf {
    let home = env::var("CARGO_HOME")
        .or_else(|_| env::var("HOME").map(|h| format!("{h}/.cargo")))
        .expect("neither CARGO_HOME nor HOME is set");
    PathBuf::from(home).join("bin")
}

fn launchd_domain() -> Option<String> {
    let output = Command::new("id").args(["-u"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let uid = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if uid.is_empty() {
        None
    } else {
        Some(format!("gui/{uid}"))
    }
}

fn macos_service_restart() {
    let home = env::var("HOME").unwrap_or_default();
    let label = "dev.daemon8.daemon";
    let plist = format!("{home}/Library/LaunchAgents/{label}.plist");

    if std::path::Path::new(&plist).exists() {
        if let Some(domain) = launchd_domain() {
            let _ = Command::new("launchctl")
                .args(["bootout", &domain, &plist])
                .status();
            let status = Command::new("launchctl")
                .args(["bootstrap", &domain, &plist])
                .status();
            match status {
                Ok(s) if s.success() => eprintln!("daemon8 service started."),
                _ => eprintln!(
                    "Warning: failed to start service. Check: launchctl print {domain}/{label}"
                ),
            }
            return;
        }

        let _ = Command::new("launchctl").args(["unload", &plist]).status();
        let status = Command::new("launchctl").args(["load", &plist]).status();
        match status {
            Ok(s) if s.success() => eprintln!("daemon8 service started."),
            _ => {
                eprintln!("Warning: failed to start service. Check: launchctl list | grep daemon8")
            }
        }
    } else {
        eprintln!("No launchd plist found at {plist}. Service not started.");
        eprintln!("Run 'daemon8 install' to create one, or copy the plist manually.");
    }
}
