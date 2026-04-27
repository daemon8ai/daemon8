# Changelog

All notable, user-facing changes to Daemon8 are recorded here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
for the daemon binary.

## [Unreleased]

### Added
- Root OSS scaffolding: `/README.md`, `/CONTRIBUTING.md`, `/CODE_OF_CONDUCT.md`,
  `/SECURITY.md`, `/NOTICE`, polyglot `/LICENSE` explainer, canonical
  `/LICENSES/` texts, and `/.github/` issue + PR templates.

## [0.1.0-alpha] — TBD-DATE

First public pre-release. Daemon is fully OSS under FCL-1.0-ALv2. No paid
tier, no license verification, no capability gate.

### Added
- **Daemon core** — runtime observation bus on `localhost:9077`. MCP server
  at `/mcp`. Pull API at `/api/observe`. Push stream at `/api/stream` (SSE).
  HTTP ingest at `/ingest`; optional UDP and Unix-socket ingestion.
- **Browser observation** — Chrome DevTools Protocol bridge via raw
  WebSocket. Captures console output, network exchanges, and JS exceptions.
  Internal CDP targets (omnibox popup, extensions, devtools, untrusted,
  error pages) are filtered out of `list_tabs` and not attached as sessions.
- **Browser actions** — `issue_command` supports `eval_js`, `screenshot`,
  `inject_css` / `revert_css`, `list_tabs`, `get_perf_metrics`, `get_dom`,
  `set_viewport` / `clear_viewport`, `network_conditions`, `navigate`,
  `storage_{clear,inspect,set}`, and `element_at_point`.
- **Agent enrollment** — `daemon8 cli-hook` universal hook handler for
  Claude Code / Cursor / Gemini / Codex / Copilot / Continue. `daemon8 init`
  scaffolds a project-local `.daemon8-cli.toml` with role presets
  (solo / queen / worker / watchdog).
- **macOS system service** — `daemon8 install` writes a launchd plist and
  boots it. Preflight surfaces the two macOS 14+ permission dialogs
  (Background Items and App Management) in advance. `daemon8 doctor`
  includes an App Management state probe.

### Changed
- Chrome liveness probe switched from `kill -0 PID` to `libc::proc_pidinfo`
  on macOS to avoid tripping the `kTCCServiceAppManagement` privacy
  permission on Sonoma+.

### Removed
- Legacy capability-gating code: `license.rs`, `license_key.rs`, `gate.rs`,
  the embedded Ed25519 public key, `CapabilityTier`, `LicenseInfo`,
  `LicensingConfig`, the `license` subcommand, and the `--license-key`
  flag. The daemon ships OSS with no tier gate, license check, or paid-tier
  infrastructure in the tree. (See `L9` in the launch plan.)

### Security
- No CVEs to date.

---

[Unreleased]: https://github.com/daemon8ai/daemon8/compare/v0.1.0-alpha...HEAD
[0.1.0-alpha]: https://github.com/daemon8ai/daemon8/releases/tag/v0.1.0-alpha
