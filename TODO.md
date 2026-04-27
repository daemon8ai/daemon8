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
  checklist exercising `connect_browser`, `issue_command`, screenshot, eval JS,
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

Docs live in the [`daemon8ai/daemon8-site`](https://github.com/daemon8ai/daemon8-site) repo and
render at [daemon8.ai/docs](https://daemon8.ai/docs). Issues are filed at
[daemon8ai/daemon8/issues](https://github.com/daemon8ai/daemon8/issues)
under the `docs` label.

The OSS-reality rewrite landed the pages below against the current code —
every MCP tool, every `issue_command` action, the full CLI surface, all
observation kinds, the `.daemon8-cli.toml` schema, and local-only
telemetry honesty. What's left is narrower.

### 1. Dedicated installation page

`quickstart.mdx` covers the happy path (`cargo install`, macOS self-sign,
`daemon8 install`, `daemon8 setup`). A dedicated `installation.mdx`
reference page would consolidate the full install surface in one place:

- The three install paths (`cargo binstall` once signed binaries ship,
  `cargo install --path crates/daemon` from a checkout, `cargo install
  --git https://github.com/daemon8ai/daemon8` without one) and when to
  reach for each.
- Local-build self-signing on macOS (`codesign --force --sign -`) and
  why it is needed after every rebuild when using `cargo install`.
- macOS first-launch prompts (Gatekeeper "unidentified developer",
  Background Items Added, App Management) and what approving each does.
- Verifying a signed release binary:
  `codesign -dvv $(which daemon8) | grep Authority`.
- Windows install via Task Scheduler and the "signed binaries pending"
  status on Windows.
- Per-platform uninstall, including local data cleanup (observations DB,
  screenshots directory, logs directory, config).

### 2. Origin pattern reference

`observation-schema.mdx` describes the `Origin` union. What it could call
out more directly is the filter syntax that `query_observations` and
`GET /api/observe` accept:

- `app:<name>` — matches application-origin observations with a given
  `app` name.
- `browser:<tab_id>` — matches observations from a specific Chrome tab's
  CDP session.
- `device:<serial>` — matches observations from a specific ADB-connected
  device or emulator.

A short table inside `observation-schema.mdx` (or a dedicated subsection
in `mcp-tools.mdx` near `query_observations`) would make this discoverable.

### 3. Minor hygiene

- Page `order:` frontmatter is assigned across groups; spot-check when
  adding new pages so the sidebar stays in the intended sequence.
- New pages should follow the existing component conventions:
  `<SimpleTable>`, `<Link>`, `<Callout>`, `<CommandTabs>`,
  `<McpConfigTabs>`.
- Inside JSX expressions, literal JSON like `{"ok":true}` must be written
  as `{'{"ok":true}'}` — the curly brace otherwise opens a JSX expression
  and MDX fails to parse.

---

## How to propose something new

If you have an idea that doesn't match any item here, open a discussion at
[GitHub Discussions](https://github.com/daemon8ai/daemon8/discussions). We
triage on roughly a weekly cadence, and a small proposal conversation
before the code is written almost always saves rework.
