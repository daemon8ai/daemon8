# Help wanted — Daemon8 TODO

Daemon8 is under active development and we're actively looking for
contributors. Everything listed here is ready for work. Most items are scoped
so a single PR can cover one; the testing gauntlet tracks and docs pages are
deliberately good-first-issue sized.

Before starting:

1. Read [`CONTRIBUTING.md`](./CONTRIBUTING.md)
2. Sign the CLA (CLA Assistant will prompt automatically on your first PR)
3. For anything non-trivial, open a discussion or issue first to align on scope

---

## Testing gauntlet

Five self-contained user-perspective testing tracks. Full detail in
[`TESTING.md`](./TESTING.md); short version below so you can pick a track.
Issues are filed at
[daemon8ai/daemon8/issues](https://github.com/daemon8ai/daemon8/issues) under
the `gauntlet` and `help wanted` labels.

- **Phase 1 — CLI surface sweep** — entry level (shell scripting). Script a
  full-surface exercise of the `daemon8` CLI: `install`, `status`, `doctor`,
  `tail`, `query`, `connections`, `logs`, `config`, `serve`, `uninstall`.
  Deliverable: `gauntlet/phase1-cli.sh`.
- **Phase 2 — MCP protocol roundtrip** — intermediate (Rust + async). Build a
  minimal rmcp client that exercises all 8 MCP tools over both stdio and
  HTTP/SSE transports. Deliverable: `gauntlet/phase2-mcp/`.
- **Phase 3 — Real Chrome CDP** — entry level (manual verification). Manual
  checklist exercising `debug_connect`, `debug_act`, screenshot, eval JS,
  navigate, inject CSS, storage operations against a real Chrome.
  Deliverable: `gauntlet/phase3-chrome.md`.
- **Phase 4a — Persistence + transports** — intermediate (shell, `nc`, `socat`).
  Exercise restart persistence, SSE `Last-Event-ID` replay, env-var config
  override, stale-config tolerance, UDP ingestion, Unix socket ingestion,
  retention sweep. Deliverable: `gauntlet/phase4a-persistence.sh`.
- **Phase 4b — Service lifecycle (per-platform)** — entry level (ops). Manual
  install/uninstall/upgrade/rollback checklist on macOS, Linux, and Windows.
  Deliverable: `gauntlet/phase4b-service.md`.

---

## Documentation gaps

[Docs](https://daemon8.ai/docs) — source code at
[Docs — Source Code](https://github.com/daemon8ai/ui/tree/main/content/docs/).
Issues are filed at
[daemon8ai/ui/issues](https://github.com/daemon8ai/ui/issues) under the
`docs` label. The audit below groups work into four priorities: stale
content (urgent), missing CLI coverage, missing HTTP/transport coverage,
missing schema/config detail, and missing install/signing coverage.

### 1. Stale content to correct (priority)

Pages that document behavior that no longer exists or is materially wrong.
Correcting these should land before adding any new pages, because they
actively mislead readers today.

- **`free-vs-pro.mdx`** — **delete.** Daemon8 is fully OSS with no tiers. The
  page asserts `$100/year` pricing and describes a Free/Paid split that no
  longer exists.
- **`mcp-tools.mdx`** — **rewrite.** Splits the 8 MCP tools into "Free Query
  Tools (6)" and "Pro Command Tools (2)". All 8 ship unconditionally.
  Rewrite as a single reference listing every tool at the same tier. The
  page should cover: `debug_observe`, `debug_summary`, `debug_checkpoint`,
  `debug_connections`, `debug_connect`, `debug_act`, `debug_ingest`,
  `debug_subscribe`. Drop the "Tier enforcement happens at daemon startup"
  sentence.
- **`reference.mdx`** — **edit.** Remove:
    1. The "One important note on tier" paragraph near the top.
    2. The `daemon8 license status` row in the Setup table.
    3. The `daemon8 license activate` row in the Setup table.
    4. The "(Free)" suffix from the `## Observation (Free)` section heading.
    5. The "(Pro)" suffix from the `## Browser Control (Pro)` section
       heading, plus the sentence that says those commands require a Pro
       license.
    6. The mention of "license key activation, telemetry opt-in" from the
       `daemon8 setup` row — rewrite to describe what setup actually does
       (browser detection, MCP client config, service install).
  Also add a new row for `daemon8 init` (see §2 below).
- **`configuration.mdx`** — **edit.** Correct:
    1. `Log retention` default from `72 hours` to `24 hours` (the actual
       fixed retention window; retention is not currently user-configurable).
    2. Remove the `Extended telemetry` row — the daemon does not collect
       telemetry of any kind.
  Expand per §5 below.
- **`telemetry-and-privacy.mdx`** — **rewrite.** The current page describes
  a baseline telemetry payload (OS, environment, daemon version, machine
  ID) transmitted on an 8-hour interval. The daemon does NOT phone home —
  no data leaves the machine. The rewrite should:
    1. State plainly that nothing is transmitted to any remote server.
    2. Describe what the daemon stores locally (observations pruned after
       24 hours, screenshots under the platform app-support directory).
    3. Point at the background cleanup sweep and the storage path config
       keys.
- **`from-logs-to-a-shared-stream.mdx`** — **edit.** Remove the sentence
  "Free gives you the query layer. Pro adds the command layer." Query and
  command ship in the same binary.
- **`monitor-browser-activity.mdx`** — **edit.** Remove the line "For browser
  commands and device control, see debug_act (Pro). Commands require a Pro
  license; the Free tier receives from the browser but cannot issue actions
  to it." Replace with a plain cross-link to `debug_act` as the browser
  command surface, with no tier gating.
- **`debug-act.mdx`** — **edit.** Drop `(Pro)` from the title and navLabel.
  Drop the Callout block that claims `debug_act` is absent from the Free
  tool surface. Document as a normal reference page; all 15 actions are
  always available.
- **`quickstart.mdx`** — **rewrite.** The current page instructs users to
  `curl -fsSL https://daemon8.ai/install/YOUR_KEY | sh`. That URL does not
  exist, and there are no license keys. Replace with the two real install
  paths from the root README:
    1. `cargo binstall daemon8` (once signed release binaries ship).
    2. `cargo install --git https://github.com/daemon8ai/daemon8 daemon8`
       as today's fallback, with the macOS self-sign step.
  Then `daemon8 install` and `daemon8 setup`.

### 2. Missing CLI documentation

`reference.mdx` covers most CLI subcommands, but these are not documented:

- **`daemon8 init`** — scaffolds a `.daemon8-cli.toml` at the project root.
  Document its role in agent enrollment and the four role presets (`queen`,
  `worker`, `solo`, `watchdog`). Document the `--force` and `--slug` flags.
  Include a short example of what the generated config looks like.
- **`daemon8 cli-hook`** — hidden hook handler invoked by AI CLIs (Claude
  Code, Cursor, Gemini, Codex, Copilot, Continue, opencode) during agent
  enrollment. Not shown in `--help`, but worth documenting its role: reads
  stdin JSON, resolves project-local `.daemon8-cli.toml`, POSTs an
  `agent.*` observation to `/ingest`. Would be a new `cli-hook-config.mdx`
  reference page covering the hook payload contract, the TOML schema, and
  how providers are registered.

### 3. Missing HTTP and transport documentation

`http-ingest.mdx` documents `POST /ingest` as the only endpoint. The
following surface is not yet documented:

- **`POST /ingest/batch`** — accepts a JSON array of observations in the
  same shape as `/ingest`. Returns `{"ok": true, "count": N}` on success.
  Use when batching reduces network overhead.
- **`GET /api/stream`** — Server-Sent Events stream. Supports the
  `Last-Event-ID` HTTP header for resume. If the requested id has been
  pruned, the server emits a synthetic `event: gap` frame whose data
  payload gives the oldest still-available id, so gap-aware clients can
  refetch from the store and gap-unaware clients ignore the unknown event
  type per the SSE spec.
- **`GET /health`** — returns `ok` when the daemon is up. Use for container
  / launchd / systemd liveness probes.
- **`GET /api/observe`** — the HTTP backing for `debug_observe`. Query
  params: `kinds`, `severity_min`, `origins`, `text_match`, `since`,
  `limit`. Returns `{"observations": [...]}`.
- **`GET /api/summary`** — the HTTP backing for `debug_summary`.
- **`GET /api/connections`** — the HTTP backing for `debug_connections`.
- **`POST /api/connect`** — the HTTP backing for `debug_connect`.
- **`POST /api/browser/act`** — the HTTP backing for `debug_act`. The
  request body is a tagged sum type (`{"action": "eval_js", "tab_id": ...}`)
  matching the MCP tool's parameter shape. Reference `debug-act.mdx` for
  per-action parameters.

UDP and Unix socket ingestion are not documented. Both are opt-in and off
by default:

- **UDP ingestion** — configured under `[ingestion.udp]` with `enabled`,
  `bind` (default `127.0.0.1:9078`), `max_packet` (default `65536`).
  Useful for fire-and-forget ingest from short-lived processes that
  cannot afford an HTTP handshake.
- **Unix socket ingestion** — configured under `[ingestion.unix]` with
  `enabled` and `path` (default `/tmp/daemon8.sock`). Useful for sandboxed
  environments where loopback HTTP is restricted.

### 4. Missing observation schema detail

`http-ingest.mdx` describes the envelope but does not give per-`kind`
field schemas. `monitor-browser-activity.mdx` hints at the browser kinds.
The following need concrete per-field documentation, either as an expanded
`http-ingest.mdx` or as a new `observation-schema.mdx` reference page.

- **`log`** — general application output. `data.message` is the canonical
  text field; other fields are free-form.
- **`query`** — database/SQL query. Required `data.sql`, optional
  `data.duration_ms` (numeric).
- **`http_exchange`** — HTTP request/response. Required `data.method`,
  `data.url`, `data.status`, `data.duration_ms`.
- **`exception`** — caught error. Required `data.message`, optional
  `data.trace`. Top-level `file` and `line` carry source location.
- **`state_snapshot`** — named state dump. Required `data.label`; the rest
  of `data` is free-form.
- **`metric`** — numeric measurement. Required `data.name`, `data.value`
  (numeric).
- **`custom`** — free-form observation. Required `data.channel`; the rest
  of `data` is free-form.
- **`js_exception`** — browser-origin. `data.message`, `data.line`,
  `data.column`.
- **`lifecycle`** — browser-origin. `data.event_name`, `data.frame_id`.

`http-ingest.mdx` also currently omits two valid `kind` values (`js_exception`,
`lifecycle`) and one valid `severity` value (`trace`). Add those too.

Document the origin patterns that feed `debug_observe` / `GET /api/observe`
filters:

- `app:<name>` — matches HTTP-ingested observations where the payload
  carried an `app` field.
- `browser:<tab_id>` — matches observations originating from a specific
  Chrome tab's CDP session.
- `device:<serial>` — matches observations from a specific ADB-connected
  device or emulator.

### 5. Missing configuration detail

`configuration.mdx` currently covers only a handful of user-facing
settings. The full config surface (all sections, all keys) is:

- `[server]` — `port` (default `9077`), `host` (IP to bind, default
  `127.0.0.1`)
- `[storage]` — `path` (SQLite DB path, empty = platform default),
  `screenshot_path` (directory for `debug_act` screenshots)
- `[browser]` — `auto_connect`, `endpoint` (default
  `http://localhost:9222`), `reconnect_interval_secs`,
  `max_reconnect_interval_secs`, `path` (Chrome binary override)
- `[mcp]` — `stdio` (stdio transport, default `true`), `http` (HTTP/SSE
  transport, default `false`)
- `[adb]` — `enabled`, `server_addr` (default `127.0.0.1:5037`),
  `scan_interval_secs`
- `[ingestion.udp]`, `[ingestion.unix]` — see §3 above
- `[logging]` — `level` (`trace`/`debug`/`info`/`warn`/`error`), `file`
  (log directory), `stderr`, `max_log_files`

Also document:

- The figment layered-config model: compiled defaults < TOML config file
  < environment variables (`DAEMON8_` prefix, `__` for nesting, e.g.
  `DAEMON8_SERVER__PORT=9090`) < CLI flags.
- Platform config paths: `~/.config/daemon8/config.toml` on Linux,
  `~/Library/Application Support/dev.daemon8.daemon8/config.toml` on
  macOS.
- `daemon8 config show`, `daemon8 config path`, and
  `daemon8 config set <key> <value>` as the CLI surface for inspecting
  and editing config.

### 6. Missing install, signing, and trust documentation

No docs page covers these today:

- The three supported install paths (`cargo binstall`, `cargo install`,
  `cargo install --git`) and when to use each.
- Local-build self-signing on macOS (`codesign --force --sign -`) and
  why it is needed after every rebuild when using `cargo install`.
- The macOS first-launch prompts (Gatekeeper "unidentified developer",
  Background Items, App Management) and what approving each does.
- How to verify a signed release binary:
  `codesign -dvv $(which daemon8) | grep Authority`.
- Windows install via Task Scheduler and the "signed binaries pending"
  status on Windows.
- Uninstall paths per platform, including local data cleanup.

A new `installation.mdx` reference page would consolidate what is
currently spread across README, quickstart, and implicit knowledge.

### Minor hygiene

- Every page's frontmatter `order:` should be checked against its group.
  After deletion of `free-vs-pro.mdx` and the new `installation.mdx`,
  renumber affected entries.
- Several pages reference `<SimpleTable>`, `<Link>`, and `<Callout>`
  components. New pages should follow the same component conventions.

---

## SDK demo improvements

Each of the three SDK demos lives in its own repo under `daemon8ai/`. They
currently ship a minimal surface; contributions that make them stronger
learning references are welcome. Open issues on the relevant demo repo:

- [`daemon8ai/daemon8-demo-php`](https://github.com/daemon8ai/daemon8-demo-php/issues)
- [`daemon8ai/daemon8-demo-laravel`](https://github.com/daemon8ai/daemon8-demo-laravel/issues)
- [`daemon8ai/daemon8-demo-symfony`](https://github.com/daemon8ai/daemon8-demo-symfony/issues)

Concrete areas where demos could grow:

- More realistic application shapes beyond hello-world.
- Example patterns for common ingestion scenarios — a full web-request
  lifecycle threading database query, outbound HTTP call, and exception
  capture under one correlation id.
- README walkthroughs that narrate the emitted observation stream step by
  step.
- CI workflows that run the demo against a real daemon instance and
  assert on the observation stream.

---

## SDKs on the roadmap

Each of these would be a new repository under `daemon8ai/`. Before
starting, please open a discussion so we can align on API shape — the
existing PHP / Laravel / Symfony SDKs define the observation envelope, and
new SDKs should wire to the same ingestion contract.

- JavaScript / TypeScript (browser + Node)
- Python (sync + async)
- Rust (client-side — distinct from the daemon binary itself)
- .NET
- Go

Discuss before starting:
[GitHub Discussions](https://github.com/daemon8ai/daemon8/discussions).

---

## How to propose something new

If you have an idea that doesn't match any item here, open a discussion at
[GitHub Discussions](https://github.com/daemon8ai/daemon8/discussions). We
triage on roughly a weekly cadence, and a small proposal conversation
before the code is written almost always saves rework.
