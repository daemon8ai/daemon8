// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

pub(crate) fn cmd_completions(
    shell: clap_complete::aot::Shell,
    cmd: &mut clap::Command,
) -> anyhow::Result<()> {
    clap_complete::aot::generate(shell, cmd, "daemon8", &mut std::io::stdout());
    Ok(())
}
