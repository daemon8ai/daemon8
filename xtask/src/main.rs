// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::{
    collections::BTreeSet,
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::{SystemTime, UNIX_EPOCH},
};

use daemon8_providers::{Provider, ServiceIdentity};

const SERVICE: ServiceIdentity = ServiceIdentity {
    name: "daemon8",
    channel_name: Some("daemon8-channel"),
    display_name: "Daemon8",
    hook_marker: "daemon8",
    status_message: Some("daemon8 telemetry"),
};

const MCP_PROVIDERS: &[Provider] = &[Provider::ClaudeCode, Provider::Gemini, Provider::Codex];

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let task = args.first().map(|s| s.as_str()).unwrap_or("");

    match task {
        "uninstall" => uninstall(&args[1..]),
        "deploy-local" => deploy_local(),
        "" | "--help" | "-h" | "help" => print_help(ExitCode::SUCCESS),
        _ => print_help(ExitCode::FAILURE),
    }
}

fn print_help(code: ExitCode) -> ExitCode {
    eprintln!("Usage: cargo xtask <task>");
    eprintln!();
    eprintln!("Tasks:");
    eprintln!("  uninstall     Remove local daemon8 install/state and provider MCP settings");
    eprintln!("                Options: --dry-run, --yes, --backup, --no-backup");
    eprintln!("  deploy-local    Build release binary and install to ~/.cargo/bin");
    code
}

fn uninstall(args: &[String]) -> ExitCode {
    let args = match UninstallArgs::parse(args) {
        UninstallParse::Run(args) => args,
        UninstallParse::Help => {
            print_uninstall_help(ExitCode::SUCCESS);
            return ExitCode::SUCCESS;
        }
        UninstallParse::Error(err) => {
            eprintln!("{err}");
            eprintln!();
            print_uninstall_help(ExitCode::FAILURE);
            return ExitCode::FAILURE;
        }
    };
    let workspace_dir = workspace_root();
    let backup_dir = workspace_dir
        .join("backups")
        .join("installs")
        .join(format!("install-{}", unix_timestamp()));
    let plan = UninstallPlan::discover(&workspace_dir, backup_dir);

    if args.dry_run {
        print_uninstall_plan(&plan, true, args.backup_choice);
        return ExitCode::SUCCESS;
    }

    if !args.yes {
        print_uninstall_plan(&plan, false, args.backup_choice);
        match confirm_uninstall() {
            Ok(true) => {}
            Ok(false) => {
                eprintln!("Aborted. No files were removed.");
                return ExitCode::FAILURE;
            }
            Err(err) => {
                eprintln!("Failed to read confirmation: {err}");
                return ExitCode::FAILURE;
            }
        }
    }

    let backup = match args.backup_choice {
        BackupChoice::Backup => true,
        BackupChoice::NoBackup => false,
        BackupChoice::Prompt if args.yes => true,
        BackupChoice::Prompt => match confirm_backup() {
            Ok(value) => value,
            Err(err) => {
                eprintln!("Failed to read backup choice: {err}");
                return ExitCode::FAILURE;
            }
        },
    };

    if let Err(err) = plan.execute(backup) {
        eprintln!("uninstall failed: {err}");
        return ExitCode::FAILURE;
    }

    if backup {
        eprintln!(
            "Uninstalled daemon8 and backed up files to {}",
            plan.backup_dir.display()
        );
    } else {
        eprintln!("Uninstalled daemon8 without backup.");
    }
    ExitCode::SUCCESS
}

struct UninstallArgs {
    dry_run: bool,
    yes: bool,
    backup_choice: BackupChoice,
}

enum UninstallParse {
    Run(UninstallArgs),
    Help,
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BackupChoice {
    Prompt,
    Backup,
    NoBackup,
}

impl UninstallArgs {
    fn parse(args: &[String]) -> UninstallParse {
        let mut dry_run = false;
        let mut yes = false;
        let mut backup_choice = None;

        for arg in args {
            match arg.as_str() {
                "--dry-run" => dry_run = true,
                "--yes" | "-y" => yes = true,
                "--backup" => {
                    if backup_choice.is_some() {
                        return UninstallParse::Error(
                            "--backup and --no-backup cannot be combined".into(),
                        );
                    }
                    backup_choice = Some(BackupChoice::Backup);
                }
                "--no-backup" => {
                    if backup_choice.is_some() {
                        return UninstallParse::Error(
                            "--backup and --no-backup cannot be combined".into(),
                        );
                    }
                    backup_choice = Some(BackupChoice::NoBackup);
                }
                "--help" | "-h" => return UninstallParse::Help,
                _ => {
                    return UninstallParse::Error(format!("unknown uninstall argument: {arg}"));
                }
            }
        }

        UninstallParse::Run(Self {
            dry_run,
            yes,
            backup_choice: backup_choice.unwrap_or(BackupChoice::Prompt),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uninstall_args_default_to_prompted_backup() {
        let UninstallParse::Run(args) = UninstallArgs::parse(&[]) else {
            panic!("expected uninstall args to parse");
        };

        assert!(!args.dry_run);
        assert!(!args.yes);
        assert_eq!(args.backup_choice, BackupChoice::Prompt);
    }

    #[test]
    fn uninstall_args_accept_explicit_backup_modes() {
        let UninstallParse::Run(args) = UninstallArgs::parse(&strings(&["--backup"])) else {
            panic!("expected --backup to parse");
        };
        assert_eq!(args.backup_choice, BackupChoice::Backup);

        let UninstallParse::Run(args) = UninstallArgs::parse(&strings(&["--no-backup"])) else {
            panic!("expected --no-backup to parse");
        };
        assert_eq!(args.backup_choice, BackupChoice::NoBackup);
    }

    #[test]
    fn uninstall_args_reject_conflicting_backup_modes() {
        let UninstallParse::Error(err) =
            UninstallArgs::parse(&strings(&["--backup", "--no-backup"]))
        else {
            panic!("expected conflicting backup flags to fail");
        };

        assert!(err.contains("cannot be combined"));
    }

    #[test]
    fn uninstall_args_accept_dry_run_and_yes() {
        let UninstallParse::Run(args) = UninstallArgs::parse(&strings(&["--dry-run", "--yes"]))
        else {
            panic!("expected --dry-run --yes to parse");
        };

        assert!(args.dry_run);
        assert!(args.yes);
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }
}

fn print_uninstall_help(code: ExitCode) -> ExitCode {
    eprintln!("Usage: cargo xtask uninstall [--dry-run] [--yes] [--backup|--no-backup]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --dry-run   Print the uninstall plan without mutating anything");
    eprintln!("  --yes, -y   Skip the destructive confirmation prompt");
    eprintln!("  --backup    Back up install/state and provider configs before removal");
    eprintln!("  --no-backup Remove without creating ./backups/installs output");
    code
}

struct UninstallPlan {
    backup_dir: PathBuf,
    moves: Vec<MoveTarget>,
    provider_configs: Vec<ProviderConfigTarget>,
    snapshots: Vec<ServiceSnapshot>,
    unloads: Vec<ServiceUnload>,
    reloads: Vec<ServiceReload>,
}

impl UninstallPlan {
    fn discover(workspace_dir: &Path, backup_dir: PathBuf) -> Self {
        let mut targets = Vec::new();

        collect_binary_targets(&mut targets);
        collect_service_targets(&mut targets);
        collect_state_targets(&mut targets);

        let snapshots = service_snapshots();
        let unloads = service_unloads();
        let reloads = service_reloads();
        let provider_configs = provider_config_targets();

        Self {
            backup_dir,
            moves: dedupe_targets(workspace_dir, targets),
            provider_configs,
            snapshots,
            unloads,
            reloads,
        }
    }

    fn execute(&self, backup: bool) -> Result<(), String> {
        if backup {
            fs::create_dir_all(&self.backup_dir).map_err(|err| err.to_string())?;
            self.write_manifest().map_err(|err| err.to_string())?;

            for snapshot in &self.snapshots {
                snapshot
                    .capture(&self.backup_dir)
                    .map_err(|err| err.to_string())?;
            }

            self.backup_provider_configs()
                .map_err(|err| err.to_string())?;
        }

        for unload in &self.unloads {
            unload
                .run(backup.then_some(self.backup_dir.as_path()))
                .map_err(|err| err.to_string())?;
        }

        for provider in &self.provider_configs {
            provider.remove_mcp_config(backup.then_some(self.backup_dir.as_path()))?;
        }

        for target in &self.moves {
            if target.source.exists() {
                if backup {
                    move_path(&target.source, &self.backup_dir.join(&target.destination))
                        .map_err(|err| err.to_string())?;
                } else {
                    remove_install_path(&target.source).map_err(|err| err.to_string())?;
                }
            }
        }

        for reload in &self.reloads {
            reload
                .run(backup.then_some(self.backup_dir.as_path()))
                .map_err(|err| err.to_string())?;
        }

        Ok(())
    }

    fn write_manifest(&self) -> io::Result<()> {
        let mut out = String::new();
        out.push_str("daemon8 install backup\n");
        out.push_str(&format!("created_epoch = {}\n", unix_timestamp()));
        out.push_str(&format!("backup_dir = {}\n", self.backup_dir.display()));
        out.push_str(&format!("os = {}\n", env::consts::OS));
        out.push_str(&format!("arch = {}\n", env::consts::ARCH));
        out.push('\n');

        out.push_str("[commands]\n");
        out.push_str(&command_output("daemon8", &["--version"]));
        out.push_str(&path_lookup_output());
        out.push('\n');

        out.push_str("[moves]\n");
        for target in &self.moves {
            out.push_str(&format!(
                "{} -> {}\n",
                target.source.display(),
                target.destination.display()
            ));
        }

        out.push('\n');
        out.push_str("[provider_mcp_configs]\n");
        for provider in &self.provider_configs {
            out.push_str(&format!(
                "{} = {} ({})\n",
                provider.provider.label(),
                provider.config_path.display(),
                provider.status()
            ));
        }

        fs::write(self.backup_dir.join("manifest.txt"), out)
    }

    fn backup_provider_configs(&self) -> io::Result<()> {
        for provider in &self.provider_configs {
            provider.backup_config(&self.backup_dir)?;
        }
        Ok(())
    }
}

#[derive(Clone)]
struct MoveTarget {
    source: PathBuf,
    destination: PathBuf,
}

impl MoveTarget {
    fn new(source: PathBuf, destination: PathBuf) -> Self {
        Self {
            source,
            destination,
        }
    }
}

struct ServiceSnapshot {
    filename: &'static str,
    command: &'static str,
    args: Vec<String>,
}

impl ServiceSnapshot {
    fn capture(&self, backup_dir: &Path) -> io::Result<()> {
        let output = Command::new(self.command).args(&self.args).output();
        let text = render_command_output(self.command, &self.args, output);
        fs::create_dir_all(backup_dir.join("metadata"))?;
        fs::write(backup_dir.join("metadata").join(self.filename), text)
    }
}

struct ServiceUnload {
    command: &'static str,
    args: Vec<String>,
}

impl ServiceUnload {
    fn run(&self, backup_dir: Option<&Path>) -> io::Result<()> {
        let output = Command::new(self.command).args(&self.args).output();
        if let Some(backup_dir) = backup_dir {
            let text = render_command_output(self.command, &self.args, output);
            fs::create_dir_all(backup_dir.join("metadata"))?;
            fs::write(
                backup_dir
                    .join("metadata")
                    .join(format!("unload-{}.txt", self.command)),
                text,
            )?;
        }
        Ok(())
    }
}

struct ServiceReload {
    command: &'static str,
    args: Vec<String>,
}

impl ServiceReload {
    fn run(&self, backup_dir: Option<&Path>) -> io::Result<()> {
        let output = Command::new(self.command).args(&self.args).output();
        if let Some(backup_dir) = backup_dir {
            let text = render_command_output(self.command, &self.args, output);
            fs::create_dir_all(backup_dir.join("metadata"))?;
            fs::write(
                backup_dir
                    .join("metadata")
                    .join(format!("reload-{}.txt", self.command)),
                text,
            )?;
        }
        Ok(())
    }
}

struct ProviderConfigTarget {
    provider: Provider,
    config_path: PathBuf,
    configured: bool,
}

impl ProviderConfigTarget {
    fn backup_config(&self, backup_dir: &Path) -> io::Result<()> {
        if !self.config_path.exists() {
            return Ok(());
        }

        let destination = backup_dir
            .join("provider-configs")
            .join(self.provider.as_provider().id())
            .join(safe_component(&self.config_path));

        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&self.config_path, destination)?;
        Ok(())
    }

    fn remove_mcp_config(&self, backup_dir: Option<&Path>) -> Result<(), String> {
        if !self.config_path.exists() {
            return Ok(());
        }

        let result = self
            .provider
            .as_provider()
            .remove_mcp_config(&self.config_path, &SERVICE);

        match result {
            Ok(true) => {
                if let Some(backup_dir) = backup_dir {
                    self.write_provider_report(backup_dir, "removed")
                        .map_err(|err| err.to_string())?;
                }
                Ok(())
            }
            Ok(false) => {
                if let Some(backup_dir) = backup_dir {
                    self.write_provider_report(backup_dir, "not configured")
                        .map_err(|err| err.to_string())?;
                }
                Ok(())
            }
            Err(err) => Err(format!(
                "{} MCP cleanup failed for {}: {err}",
                self.provider.label(),
                self.config_path.display()
            )),
        }
    }

    fn status(&self) -> &'static str {
        if !self.config_path.exists() {
            "missing"
        } else if self.configured {
            "configured"
        } else {
            "not configured"
        }
    }

    fn write_provider_report(&self, backup_dir: &Path, status: &str) -> io::Result<()> {
        fs::create_dir_all(backup_dir.join("metadata"))?;
        fs::write(
            backup_dir.join("metadata").join(format!(
                "provider-{}-mcp.txt",
                self.provider.as_provider().id()
            )),
            format!(
                "provider = {}\nconfig = {}\nstatus = {status}\n",
                self.provider.label(),
                self.config_path.display()
            ),
        )
    }
}

fn collect_binary_targets(targets: &mut Vec<MoveTarget>) {
    let names = binary_names();
    for dir in binary_dirs() {
        for name in &names {
            targets.push(MoveTarget::new(
                dir.join(name),
                PathBuf::from("bin").join(safe_component(&dir)).join(name),
            ));
        }

        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let file_name = entry.file_name();
                let name = file_name.to_string_lossy();
                if is_daemon8_backup_binary(&name) {
                    targets.push(MoveTarget::new(
                        entry.path(),
                        PathBuf::from("bin")
                            .join(safe_component(&dir))
                            .join(name.as_ref()),
                    ));
                }
            }
        }
    }
}

fn collect_service_targets(targets: &mut Vec<MoveTarget>) {
    #[cfg(target_os = "macos")]
    if let Some(home) = home_dir() {
        targets.push(MoveTarget::new(
            home.join("Library/LaunchAgents/dev.daemon8.daemon.plist"),
            PathBuf::from("service/macos/dev.daemon8.daemon.plist"),
        ));
    }

    #[cfg(target_os = "linux")]
    if let Some(home) = home_dir() {
        targets.push(MoveTarget::new(
            home.join(".config/systemd/user/daemon8.service"),
            PathBuf::from("service/linux/daemon8.service"),
        ));
    }
}

fn collect_state_targets(targets: &mut Vec<MoveTarget>) {
    for app in ["daemon8", "daemon8-dev"] {
        if let Some(project_dirs) = directories::ProjectDirs::from("dev", "daemon8", app) {
            push_dir_target(
                targets,
                project_dirs.data_dir(),
                &format!("state/{app}/data"),
            );
            push_dir_target(
                targets,
                project_dirs.config_dir(),
                &format!("state/{app}/config"),
            );
            push_dir_target(
                targets,
                project_dirs.cache_dir(),
                &format!("state/{app}/cache"),
            );
        }
    }

    #[cfg(target_os = "macos")]
    if let Some(home) = home_dir() {
        targets.push(MoveTarget::new(
            home.join("Library/Application Support/dev.daemon8.backups"),
            PathBuf::from("state/legacy/dev.daemon8.backups"),
        ));
    }
}

fn push_dir_target(targets: &mut Vec<MoveTarget>, source: &Path, destination: &str) {
    targets.push(MoveTarget::new(
        source.to_path_buf(),
        PathBuf::from(destination),
    ));
}

fn dedupe_targets(workspace_dir: &Path, targets: Vec<MoveTarget>) -> Vec<MoveTarget> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();

    for target in targets {
        if target.source.starts_with(workspace_dir.join("backups")) {
            continue;
        }

        let key = target.source.clone();
        if seen.insert(key) && target.source.exists() {
            deduped.push(target);
        }
    }

    deduped
}

fn service_snapshots() -> Vec<ServiceSnapshot> {
    #[cfg(target_os = "macos")]
    {
        let mut snapshots = Vec::new();
        if let Some(domain) = launchd_domain() {
            snapshots.push(ServiceSnapshot {
                filename: "launchctl-before.txt",
                command: "launchctl",
                args: vec!["print".into(), format!("{domain}/dev.daemon8.daemon")],
            });
        }
        snapshots
    }

    #[cfg(target_os = "linux")]
    {
        vec![ServiceSnapshot {
            filename: "systemctl-before.txt",
            command: "systemctl",
            args: vec!["--user".into(), "status".into(), "daemon8".into()],
        }]
    }

    #[cfg(windows)]
    {
        vec![ServiceSnapshot {
            filename: "schtasks-before.xml",
            command: "schtasks",
            args: vec![
                "/Query".into(),
                "/TN".into(),
                "Daemon8".into(),
                "/XML".into(),
            ],
        }]
    }
}

fn service_unloads() -> Vec<ServiceUnload> {
    #[cfg(target_os = "macos")]
    {
        let mut unloads = Vec::new();
        if let (Some(home), Some(domain)) = (home_dir(), launchd_domain()) {
            let plist = home
                .join("Library/LaunchAgents/dev.daemon8.daemon.plist")
                .display()
                .to_string();
            unloads.push(ServiceUnload {
                command: "launchctl",
                args: vec!["bootout".into(), domain, plist],
            });
        }
        unloads
    }

    #[cfg(target_os = "linux")]
    {
        vec![ServiceUnload {
            command: "systemctl",
            args: vec![
                "--user".into(),
                "disable".into(),
                "--now".into(),
                "daemon8".into(),
            ],
        }]
    }

    #[cfg(windows)]
    {
        vec![ServiceUnload {
            command: "schtasks",
            args: vec![
                "/Delete".into(),
                "/TN".into(),
                "Daemon8".into(),
                "/F".into(),
            ],
        }]
    }
}

fn service_reloads() -> Vec<ServiceReload> {
    #[cfg(target_os = "linux")]
    {
        vec![ServiceReload {
            command: "systemctl",
            args: vec!["--user".into(), "daemon-reload".into()],
        }]
    }

    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

fn provider_config_targets() -> Vec<ProviderConfigTarget> {
    let home = daemon8_providers::dirs_home();
    MCP_PROVIDERS
        .iter()
        .map(|&provider| {
            let config_path = provider.config_path(&home);
            let configured = provider.as_provider().is_configured(&config_path, &SERVICE);

            ProviderConfigTarget {
                provider,
                config_path,
                configured,
            }
        })
        .collect()
}

fn binary_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(dir) = env::var("DAEMON8_INSTALL_DIR") {
        dirs.push(PathBuf::from(dir));
    }

    if let Ok(cargo_home) = env::var("CARGO_HOME") {
        dirs.push(PathBuf::from(cargo_home).join("bin"));
    } else if let Some(home) = home_dir() {
        dirs.push(home.join(".cargo/bin"));
    }

    #[cfg(unix)]
    {
        if let Some(home) = home_dir() {
            dirs.push(home.join(".local/bin"));
        }
        dirs.push(PathBuf::from("/usr/local/bin"));
    }

    #[cfg(windows)]
    {
        if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local_app_data).join("Programs/daemon8"));
        }
    }

    dirs
}

fn binary_names() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["daemon8.exe", "daemon8-mcp-server.exe", "LICENSE-daemon8"]
    } else {
        vec!["daemon8", "daemon8-mcp-server", "LICENSE-daemon8"]
    }
}

fn is_daemon8_backup_binary(name: &str) -> bool {
    name.starts_with("daemon8.bak")
        || name.starts_with("daemon8.prev")
        || name.starts_with("daemon8.v1.bak")
}

fn move_path(source: &Path, destination: &Path) -> io::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }

    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(_) if source.is_dir() => {
            copy_dir_all(source, destination)?;
            fs::remove_dir_all(source)
        }
        Err(_) => {
            fs::copy(source, destination)?;
            fs::remove_file(source)
        }
    }
}

fn remove_install_path(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }

    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn copy_dir_all(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let destination_path = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&entry.path(), &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), destination_path)?;
        }
    }
    Ok(())
}

fn print_uninstall_plan(plan: &UninstallPlan, dry_run: bool, backup_choice: BackupChoice) {
    if dry_run {
        eprintln!(
            "Dry run: no files/provider settings will be removed and no service will be unloaded."
        );
    } else {
        eprintln!(
            "This will unload daemon8's local service, remove provider MCP settings, and remove daemon8-owned install/state files."
        );
    }

    match backup_choice {
        BackupChoice::Prompt => {
            eprintln!("Backup: prompt before uninstall (defaults to yes with --yes)");
            eprintln!("Backup dir if selected: {}", plan.backup_dir.display());
        }
        BackupChoice::Backup => eprintln!("Backup dir: {}", plan.backup_dir.display()),
        BackupChoice::NoBackup => eprintln!("Backup: disabled"),
    }

    eprintln!();
    eprintln!("Service snapshots:");
    if plan.snapshots.is_empty() {
        eprintln!("  (none for this platform)");
    } else {
        for snapshot in &plan.snapshots {
            eprintln!("  {} {}", snapshot.command, snapshot.args.join(" "));
        }
    }

    eprintln!();
    eprintln!("Service unloads:");
    if plan.unloads.is_empty() {
        eprintln!("  (none for this platform)");
    } else {
        for unload in &plan.unloads {
            eprintln!("  {} {}", unload.command, unload.args.join(" "));
        }
    }

    eprintln!();
    eprintln!("Provider MCP settings:");
    for provider in &plan.provider_configs {
        eprintln!(
            "  {}: {} ({})",
            provider.provider.label(),
            provider.config_path.display(),
            provider.status()
        );
    }

    eprintln!();
    eprintln!("Install/state paths:");
    if plan.moves.is_empty() {
        eprintln!("  (no daemon8 install/state paths found)");
    }
    for target in &plan.moves {
        if matches!(backup_choice, BackupChoice::NoBackup) {
            eprintln!("  remove {}", target.source.display());
        } else {
            eprintln!(
                "  {} -> {}",
                target.source.display(),
                plan.backup_dir.join(&target.destination).display()
            );
        }
    }
}

fn confirm_uninstall() -> io::Result<bool> {
    use std::io::Write;

    eprintln!();
    eprintln!("Type 'uninstall daemon8' to continue.");
    eprint!("Confirmation: ");
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim() == "uninstall daemon8")
}

fn confirm_backup() -> io::Result<bool> {
    use std::io::Write;

    eprint!("Back up install/state files and provider configs first? [Y/n]: ");
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim();
    Ok(answer.is_empty() || answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}

fn command_output(command: &str, args: &[&str]) -> String {
    render_command_output(command, args, Command::new(command).args(args).output())
}

fn render_command_output<S>(
    command: &str,
    args: &[S],
    output: io::Result<std::process::Output>,
) -> String
where
    S: AsRef<str>,
{
    let mut out = String::new();
    out.push_str(&format!("$ {} {}\n", command, join_args(args)));
    match output {
        Ok(output) => {
            out.push_str(&format!("status = {}\n", output.status));
            out.push_str(&String::from_utf8_lossy(&output.stdout));
            out.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        Err(err) => out.push_str(&format!("error = {err}\n")),
    }
    out
}

fn join_args<S>(args: &[S]) -> String
where
    S: AsRef<str>,
{
    args.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(" ")
}

fn path_lookup_output() -> String {
    if cfg!(windows) {
        command_output("where", &["daemon8"])
    } else {
        command_output("sh", &["-c", "command -v -a daemon8"])
    }
}

fn safe_component(path: &Path) -> String {
    path.display()
        .to_string()
        .chars()
        .map(|ch| match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' => ch,
            _ => '_',
        })
        .collect()
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
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
