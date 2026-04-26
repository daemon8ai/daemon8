# Testing Gauntlet

Daemon8's CI suite runs 167+ automated tests on every PR — unit coverage
per crate, integration tests against a real HTTP server + SSE pipeline, and
property tests on the Chrome command-handler state machine. That's the
automated floor.

**Above that floor is the Testing Gauntlet: a user-perspective E2E
smoke test that exercises behavior CI cannot reach.** The gauntlet runs
against a compiled `daemon8` binary, real filesystem paths, real service
daemons (launchd / systemd / schtasks), real Chrome, real ADB, and real
MCP clients. It's how we verify a release candidate before cutting a tag.

The gauntlet is structured as **bounded, self-contained tracks** so
contributors can pick one up, build it, and submit without needing deep
knowledge of daemon internals. Everything lives at the daemon's API
boundary.

---

## Why this exists

Automated tests cover the interior: crates, functions, mocks. The
gauntlet covers the exterior: what a human operator actually touches.
These are categories the current test suite does not reach:

- **CLI subcommands.** Every flag, every happy path, every error path.
- **Real MCP protocol handshakes.** `initialize` → `notifications/initialized`
  → `tools/list` → `tools/call` over both stdio and HTTP/SSE transports.
- **Real Chrome DevTools Protocol.** `Runtime.evaluate`, `Page.captureScreenshot`,
  `Emulation.setDeviceMetricsOverride` against a live Chromium.
- **Real ADB devices.** Hot-plug discovery, logcat streaming, platform detection.
- **Service lifecycle.** launchd plist writes, systemd user units, schtasks jobs,
  upgrade and rollback paths, KeepAlive respawn behavior.
- **Persistence + resilience.** SQLite WAL across daemon restarts, SSE
  `Last-Event-ID` replay, retention sweeps, concurrent subscribers.
- **Alternative ingestion transports.** UDP packets and Unix socket newline
  streams (HTTP is covered by CI; these are not).
- **Multi-client reality.** Two or more MCP clients connected simultaneously,
  each with different filters.

Running the gauntlet is not required to contribute code. It is its own
contribution path.

---

## Tracks

The gauntlet has five independent tracks. Each is a discrete contribution:
pick one, build it, submit a PR. No coordination needed with other tracks.

| Track    | Focus                              | Skill level                                       | Hands-on gear needed |
|----------|------------------------------------|---------------------------------------------------|----------------------|
| Phase 1  | CLI surface sweep                  | Entry — shell scripting                           | None |
| Phase 2  | MCP protocol roundtrip             | Intermediate — Rust + async                       | None |
| Phase 3  | Real Chrome CDP                    | Entry — ops / manual verification                 | Chrome + visual observation |
| Phase 4a | Persistence + transport            | Intermediate — shell, `nc`, `socat`               | None |
| Phase 4b | Service lifecycle (per-platform)   | Entry — ops / manual verification                 | macOS + Linux + Windows (one per PR is fine) |

Tracking issues for each phase live on GitHub under the
[`gauntlet`](https://github.com/daemon8ai/daemon8/labels/gauntlet) label.

---

## Prerequisites

All tracks:

- Rust stable (the daemon crate's `rust-toolchain.toml` pins it)
- A working `daemon8` binary (either `cargo install --path crates/daemon --force`
  or `cargo binstall daemon8` once releases are published)
- POSIX shell (zsh or bash)

Track-specific:

- **Phase 2** — no extra prereqs; the track includes a Rust binary using
  rmcp's client transport.
- **Phase 3** — a Chromium-based browser (Chrome, Chromium, Brave, Edge,
  or Arc). No Developer ID signing needed for local gauntlet runs.
- **Phase 4a** — `nc` (netcat) or `socat` for UDP / Unix socket tests.
  Both are standard on macOS and Linux.
- **Phase 4b** — sudo access on the target OS. No remote machine needed;
  the gauntlet runs against the operator's own install.

---

## Harness layout

The gauntlet lives at the repo root in a `gauntlet/` directory:

```
gauntlet/
├── README.md                  # entry point + how to run the full gauntlet
├── run.sh                     # top-level orchestrator
├── phase1-cli.sh              # Phase 1 implementation
├── phase2-mcp/                # Phase 2 — Rust binary using rmcp client
│   ├── Cargo.toml
│   └── src/main.rs
├── phase3-chrome.md           # Phase 3 — manual checklist
├── phase4a-persistence.sh     # Phase 4a
├── phase4b-service.md         # Phase 4b — manual per-platform checklist
└── fixtures/
    ├── config-stale-licensing.toml
    ├── sample-observations.jsonl
    └── test-page.html
```

The top-level `run.sh` orchestrates everything and produces a summary
report. Each phase runs independently; `run.sh --phase=1` runs only
Phase 1.

`gauntlet/` is outside the daemon crate and outside CI. It is not
blocking on every PR. Operators run it when preparing a release
candidate (`v0.1.0-rc.1`, etc.) and before announcing releases.

---

## How to claim a phase

1. Read the tracking issue for the phase you're interested in.
2. Comment on the issue with what you plan to cover and a rough timeline
   (no hard SLAs — a rough ETA helps other contributors avoid duplicating
   effort).
3. A maintainer assigns the issue to you. Claim is good for two weeks;
   if no progress shows up in that window, the issue is reopened for
   other contributors.
4. Open a draft PR early (even empty). Helps surface blockers before
   they become rework.

If the phase is large, **split it**. Phase 1 (CLI sweep) is fine as one
PR because the scope is one script. Phase 2 (MCP) may warrant splitting
stdio and HTTP transports into two PRs.

---

## Phase details

### Phase 1 — CLI surface sweep

**Goal:** every subcommand of `daemon8` is exercised at least once with
valid args (happy path) and once with obviously-invalid args (error
path). Nothing regresses silently.

**Scope:**

- `daemon8 serve` — boot, accept one request, shut down cleanly via SIGTERM
- `daemon8 status` — against a running daemon + against a stopped daemon
- `daemon8 tail` — happy path + invalid filter
- `daemon8 query` — with `--kind`, `--severity`, `--since`, `--text`, `--limit`
- `daemon8 connections` — basic check
- `daemon8 browser {tabs, eval, screenshot, inject-css, revert-css, perf, dom, set-viewport, clear-viewport}`
  — each with stubbed chrome command receiver; Phase 3 covers real Chrome
- `daemon8 logs` + `daemon8 logs -f`
- `daemon8 config {show, path, set, show --json}`
- `daemon8 completions {bash, zsh, fish, powershell}`
- `daemon8 doctor` (read-only) — not `--fix`, which mutates system state
- `daemon8 init` — scaffolding a `.daemon8-cli.toml`
- `daemon8 --version`, `daemon8 --help`, and each subcommand's `--help`

**Acceptance:**

1. `gauntlet/phase1-cli.sh` exists and is idempotent — safe to run repeatedly.
2. Uses a sandboxed config/data directory (`$TMPDIR/daemon8-gauntlet-*/`).
3. Starts the daemon on a random unused port ≥ `19000` to avoid collision
   with the operator's real daemon.
4. Cleans up after itself: kills the daemon, removes temp dir, no orphans.
5. Emits a pass/fail line per subcommand in the format:
   ```
   PASS  phase1.cli.serve_boots
   PASS  phase1.cli.status_reports_running
   FAIL  phase1.cli.browser_eval_empty_expression (expected exit != 0, got 0)
   ```
6. Final line is a summary: `SUMMARY: N/M passed, K failed`.
7. Exits `0` iff all tests pass.

**Hints:**

- Use `bash -euo pipefail` at the top.
- Background the daemon with `daemon8 serve --port 19077 &`, capture
  `$!`, trap on EXIT to kill it.
- Give the daemon ~500ms to bind before hitting the HTTP API.
- `daemon8 doctor --fix` is explicitly excluded — it mutates system
  state, belongs to Phase 4b.

### Phase 2 — MCP protocol roundtrip

**Goal:** verify the MCP handshake + each of the 8 tools over both stdio
and HTTP/SSE transports using a real MCP client, not mocks.

**Scope:**

- `initialize` handshake with `protocolVersion: "2025-03-26"` and a fake
  `clientInfo`
- `notifications/initialized`
- `tools/list` — assert exactly 8 tools and exact names match
- `tools/call` for each tool:
  - `status` — response shape (keys, no tier fields)
  - `query_observations` — seed via `/ingest`, call, verify results
  - `create_checkpoint` — call twice, checkpoint advances
  - `list_connections` — shape validation
  - `ingest_observation` — call, verify via `/api/observe`
  - `subscribe_observations` — set filter, verify applied
  - `connect_browser` — verify `ChromeCommand::Connect` reaches the channel
    (stubbed receiver in the gauntlet, no real Chrome)
  - `issue_command` — dispatch each variant (`EvalJs`, `ListTabs`, `Screenshot`,
    `InjectCss`, `RevertCss`, `GetPerfMetrics`, `GetDom`, `SetViewport`,
    `ClearViewport`, `NetworkConditions`, `Navigate`, `StorageClear`,
    `StorageInspect`, `StorageSet`, `ElementAtPoint`)

Two transports:

- **stdio** — spawn daemon with stdio MCP transport, communicate via
  child-process stdin/stdout. rmcp provides the client-side wrapper.
- **HTTP/SSE streamable** — daemon running with `mcp.http = true`, client
  uses rmcp's `StreamableHttpClientTransport` or equivalent against
  `http://localhost:19077/mcp`.

**Acceptance:**

1. Binary at `gauntlet/phase2-mcp/` — `cargo run` runs the full battery.
2. Each transport × tool combination emits one PASS/FAIL line.
3. Final summary reports `8 tools × 2 transports = 16` checks.
4. `--transport=stdio` and `--transport=http` flags allow running one
   transport at a time.
5. Exits `0` iff all checks pass.

**Hints:**

- rmcp client example: [rmcp/examples/rust-sdk](https://github.com/modelcontextprotocol/rust-sdk/tree/main/examples)
- The stdio transport requires launching the daemon as a child process
  and connecting via its stdin/stdout pipes. rmcp handles the JSON-RPC
  framing.
- HTTP/SSE transport requires `Mcp-Session-Id` header management — rmcp
  handles that automatically.
- For `connect_browser` and `issue_command`, the daemon binary you spawn
  should have a stubbed chrome command receiver. Easiest: set the chrome
  endpoint to an unreachable address; the command enters the channel but
  Chrome never actually gets contacted. Phase 3 covers real Chrome.

### Phase 3 — Real Chrome CDP

**Goal:** verify daemon8 can actually drive a real Chromium browser
through every command action variant.

**Scope:**

- `daemon8 browser tabs` — lists real tabs
- `daemon8 browser eval "document.title"` — real JS eval
- `daemon8 browser screenshot --output /tmp/shot.png` — file exists, valid PNG header
- `daemon8 browser screenshot --selector "h1"` — element screenshot
- `daemon8 browser inject-css "body { border: 5px solid red }"` — visually verified
- `daemon8 browser revert-css` — visually verified
- `daemon8 browser perf` — non-empty metrics returned
- `daemon8 browser dom "h1"` — real DOM extraction
- `daemon8 browser set-viewport --width 390 --height 844 --mobile true`
  — visually verified layout change
- `daemon8 browser clear-viewport` — restore
- **Kill Chrome mid-session** — observe reconnection behavior in daemon logs

**Acceptance:**

1. `gauntlet/phase3-chrome.md` exists with a step-by-step checklist.
2. Each step has: the command, the expected result, and a pass/fail
   checkbox.
3. Includes a "Setup" section (how to launch Chrome with
   `--remote-debugging-port=9222`) and a "Teardown" section.
4. Includes a free-form "Observer notes" section for anomalies.
5. At the end, the operator signs + dates with a one-line summary:
   `Approved by <handle> on <date>: N/11 steps pass, M failures noted`.

**Hints:**

- Use `https://example.com` as the test page — stable, minimal.
- For visual verification of CSS injection, include a screenshot-before
  and screenshot-after step.
- For viewport changes, take a CDP screenshot before and after to compare
  pixel dimensions.

### Phase 4a — Persistence + alternative transports

**Goal:** verify SQLite persistence survives restarts, SSE replay works
after a daemon bounce, retention sweep fires, UDP + Unix ingestion work.

**Scope:**

1. **Persistence across restart** — seed 100 observations, stop daemon,
   restart, `GET /api/observe?limit=100` returns all 100 with same IDs.
2. **SSE Last-Event-ID replay** — subscriber connects, seeds 10 obs,
   kill daemon mid-stream, restart, subscriber reconnects with
   `Last-Event-ID`, verify clean replay with no gaps.
3. **Env-var override** — `DAEMON8_SERVER__PORT=19090 daemon8 serve`
   binds port 19090, not 9077.
4. **Stale config tolerance** — inject `[licensing] key = "..."` into
   `config.toml`, verify daemon boots clean with no warnings and no
   observable difference.
5. **UDP ingestion** — enable `ingestion.udp`, send JSON via `nc -u`,
   verify observation surfaces in `/api/observe`.
6. **Unix socket ingestion** — enable `ingestion.unix`, send JSON via
   `socat`, verify observation surfaces.
7. **Retention sweep** — `cleanup_before(now)` via test seam or sleep
   past the interval, verify old observations are purged.

**Acceptance:**

1. `gauntlet/phase4a-persistence.sh` — idempotent, sandboxed, emits
   PASS/FAIL per test case.
2. No step depends on a previous step's side effects (each is isolated).
3. Clean teardown.
4. Exit code 0 iff all pass.

**Hints:**

- For the retention test, expose a hidden CLI flag (`--retention-secs`
  or similar) that shortens the sweep interval for test purposes. If
  one doesn't exist, either add one (separate PR first) or use the
  store trait directly in a Rust helper.
- `jq` is useful for asserting on JSON shapes; prefer it over grep.

### Phase 4b — Service lifecycle (per-platform)

**Goal:** verify `daemon8 install` / `daemon8 uninstall` create and
remove platform services cleanly, across macOS, Linux, and Windows.

Each platform is a separate submission. One contributor covers one
platform per PR.

**Scope (per platform):**

- `daemon8 install` — service registered, daemon running, `daemon8 status`
  reports running
- `daemon8 status` across a reboot — service auto-starts
- `kill -9 <pid>` — launchd/systemd respawns (KeepAlive)
- `daemon8 uninstall` — service file removed, process stopped
- **Upgrade path** — backup binary, uninstall, rebuild, reinstall, verify
- **Rollback path** — uninstall new binary, restore `.bak`, reinstall

Platform-specific:

- **macOS** — includes App Management permission flow. First install
  triggers TCC prompt; document the user-facing observation (should not
  break, should surface the prompt clearly).
- **Linux** — systemd user units specifically (not system-wide). Verify
  `systemctl --user` commands work without sudo.
- **Windows** — Task Scheduler task named `Daemon8`. Verify task shows
  in Task Scheduler GUI and runs at login.

**Acceptance:**

1. `gauntlet/phase4b-service.md` — platform-indexed checklist (one
   section per platform).
2. Each step has command + expected + checkbox.
3. Observer signs + dates.
4. Platforms can be contributed independently; the doc accepts partial
   coverage.

**Hints:**

- macOS install triggers a TCC prompt on first install. Document the
  user experience, don't try to suppress it.
- On Linux, `journalctl --user -u daemon8` shows service logs.
- On Windows, `schtasks /Query /TN Daemon8 /V /FO LIST` verifies task
  config without opening the GUI.

---

## Output format

Every phase's script should emit lines in this format:

```
PASS  phase<N>.<module>.<test_name>
FAIL  phase<N>.<module>.<test_name> (one-line reason)
SKIP  phase<N>.<module>.<test_name> (why skipped)
```

End of run emits a summary line:

```
SUMMARY: N passed, M failed, K skipped in T seconds
```

The top-level `gauntlet/run.sh` aggregates across phases and produces a
markdown report in `gauntlet/runs/YYYY-MM-DD-HHMM.md`:

```markdown
# Gauntlet Run — 2026-04-21 14:30 — daemon8 v0.1.0-rc.1

Host: macOS 14.5, M2 Pro, rustc 1.92
Daemon: <git sha> @ <branch>

## Summary
- Phase 1 CLI:         48/48 PASS
- Phase 2 MCP:         16/16 PASS
- Phase 3 Chrome:       9/9 manual-approved by @handle
- Phase 4a Persistence: 7/7 PASS
- Phase 4b Service:     macOS 6/6 approved by @handle (Linux, Windows pending)

## Notable findings
- [free-form observations]

## Failures
- [list if any]

## Signed off
- @handle on 2026-04-21
```

Completed runs are committed to the repo — they serve as a historical
record of what shipped when.

---

## Review criteria

A gauntlet-phase PR is merged when:

1. The script/doc is at the conventional path (`gauntlet/phase<N>-*.sh`
   or `.md`).
2. The acceptance criteria for that phase are met.
3. A maintainer runs the phase locally on at least one machine and it
   passes.
4. The PR includes a sample run output in the PR body.
5. The relevant tracking issue is linked.
6. CLA is signed (automatic via CLA Assistant).

Maintainers may request targeted additions (covering an edge case the
contributor missed), but scope creep is explicitly resisted — a gauntlet
phase PR is one phase, not "phase 1 + some other improvements."

---

## Asking questions

- Use the phase's tracking issue for phase-specific design discussion.
- Use [GitHub Discussions](https://github.com/daemon8ai/daemon8/discussions)
  for broader gauntlet-strategy questions.
- Security-sensitive questions about what the gauntlet should cover go
  to `mail@daemon8.ai`, not public channels.

---

## Why not run this in CI?

Three reasons:

1. **Side effects.** `daemon8 install` writes to
   `~/Library/LaunchAgents/` or equivalent. CI runners are ephemeral
   but the semantics are wrong — we want the gauntlet to exercise the
   real filesystem on a real user's machine.
2. **Permissions.** macOS TCC prompts require human interaction on
   first install. CI can't approve the "App Management" dialog.
3. **Speed.** The gauntlet is comprehensive, not fast. Running it
   per-PR would slow CI dramatically. Per-release is the right cadence.

What CI does cover today: every test in `crates/**/tests/` and
`crates/daemon/tests/integration.rs`. That's the automated floor.
The gauntlet is the ceiling.
