## Purpose

Persist a stable reference into the daemon8 librarian. The librarian is daemon8's primary memory of project topology, source locations, and learned conventions — every entry teaches daemon8 about *where* something lives so future sessions can use it without re-discovery.

## When to use (mandatory)

After receiving a daemon8 discovery hint (`query_observations` returns an observation with `kind=custom` and `channel="discovery_hint"`), ALWAYS:
1. Investigate where the project's runtime data lives (logs, configs, conversation files, framework data dirs).
2. Call `librarian_index` once per location with a `source_template` entry, following the schema below.
3. Optionally call `librarian_index` again with `source_instance` entries for concrete discovered paths that resolved on this machine.

After discovering ANY stable reference location via shell exploration (independent of discovery hints), ALWAYS:
1. Call `librarian_lookup` to check for an existing entry on this locator.
2. If absent, call `librarian_index` to persist.

After a `query_observations` response includes a path-pattern hint in `daemon8.hints`, treat it as a directive to file a `source_template` for the path. The hint already names the path, the active project's tags, and the missing template.

## Triggers (non-exhaustive)

- Log file paths discovered via `find`, `ls`, `tail`, or surfaced in tool output
- Config file locations under OS-specific dirs (`~/Library/`, `~/.config/`, `~/Library/Application Support/`)
- Framework data/cache dirs (Metro, Kepler, Expo, Next.js `.next`, Laravel `storage/logs`)
- Documentation URLs the user references repeatedly
- Project-specific protocols, runbooks, or vault notes
- Any source-of-truth file path for a reusable asset

## Node kinds

| `kind` | What it represents | When to use |
|---|---|---|
| `doc` | Documentation: README, wiki page, API reference, vault note | URL or vault path the agent or user will revisit |
| `source_template` | Learned location pattern for a project type | Where logs/data for THIS framework live on THIS OS |
| `source_instance` | A concrete resolved path for one project | A specific file your shell run found |
| `project` | A per-project record | Capturing root path + classification |
| `fix` | A known fix recipe or workaround | Steps that resolved a recurring error |

## Args
- `kind`: required string. One of the kinds above.
- `label`: required string. Human-readable name for this reference.
- `locator_kind`: required string. `file` | `url` | `vault`.
- `locator`: required string. The actual pointer — a file path, URL, or vault note path. Must respect the portability rules below for `source_template`.
- `tags`: optional list of strings. Free-form retrieval tags.
- `project_slug`: optional string. Scope for retrieval.
- `parent_id`: optional string. Place this node under an existing catalog node for hierarchy.
- `edge`: optional object `{ kind: string, target_node_id: string }`. Create a relationship edge at index time.
- `canonicalize`: optional bool. Mark as authoritative reference — canonicalized nodes are never flagged as stale.
- `data`: kind-specific payload (snake_case JSON). Required for `source_template` and `project`. See data shape table below.

## Data payloads

### `source_template` data (`SourceTemplateData`)

```jsonc
{
  "project_types": ["react-native", "vega"],      // OR semantics; subset of KNOWN_PROJECT_TYPE_TAGS
  "kind": "log",                                   // log | config | conversation | cache | crash | build | db | metric
  "locator_pattern": "~/Library/Application Support/Code/logs/*/window*/exthost/amazon-devices.kepler-studio/CoreModule-*.log",
  "platforms": ["macos"],                          // OR semantics; macos | linux | windows
  "parser_hint": "react-native-bridge",            // optional
  "default_tags": ["kepler", "vscode-extension"],
  "description": "Kepler Studio core module log emitted by the VS Code extension",
  "version_constraint": ">=0.74",                  // optional SemVer-style range; omit for version-agnostic
  "discovered_by_session": null,                   // daemon8 fills this
  "discovered_by_provider": null,                  // claude | codex | gemini | user
  "discovered_at_ns": 0,
  "verified_count": 0,
  "last_verified_at_ns": 0,
  "confidence": "agent_discovered"                 // agent_discovered | user_provided | drifted
}
```

### `project` data (`ProjectNodeData`)

```jsonc
{
  "root_path": "/Users/me/code/example",
  "slug": "example",
  "classification_tags": ["react-native", "git-repo"],
  "framework_versions": { "react-native": "0.74.5" },
  "platform": "macos",
  "created_at_ns": 0,
  "last_serve_at_ns": 0,
  "skip_discovery": false
}
```

### `source_instance` data (`SourceInstanceData`)

```jsonc
{
  "kind": "log",
  "resolved_path": "/Users/me/Library/Application Support/Code/logs/.../CoreModule-2026-05-13.log",
  "parser": "react-native-bridge",                 // optional
  "tags": ["kepler"],
  "version_constraint": null,
  "registered_at_ns": 0,
  "last_verified_at_ns": 0
}
```

## Portability rules (enforced by the validator)

`source_template.locator_pattern` MUST be portable across machines:

- Use `~` for the user home — NEVER a literal `/Users/<name>/...` or `/home/<name>/...` (rejected).
- Use env-var references for OS-specific roots: `$XDG_CONFIG_HOME`, `$LOCALAPPDATA`, `$TMPDIR`. Both `$VAR` and `${VAR}` forms work.
- Use `<root>` for project-relative paths — resolved per-project at registration.
- Glob characters (`*`, `?`, `[...]`) are allowed; expansion happens at registration.
- `platforms` MUST be set explicitly — never imply OS from the path.
- `project_types` entries MUST come from the known allowlist. Current set: `any`, `git-repo`, `react-native`, `expo`, `vega`, `kepler`, `nextjs`, `vite`, `tanstack-start`, `rust`, `rust-workspace`, `laravel`, `symfony`, `python`, `django`, `flask`, `fastapi`, `go`, `rails`. The discovery hint payload always echoes the current allowlist in `known_project_type_tags_ref`.
- Windows absolute user paths (`C:\Users\...`) and UNC paths (`\\server\share`) are rejected.

## Returns
- `result: {id, version}`.

## Errors
- `librarian_store_unavailable`: catalog not configured.
- `invalid_kind`: kind string not recognized.
- `invalid_locator_kind`: locator_kind string not recognized.
- Validator errors include the offending field and the rule it violated.

## Examples

Register a Metro bundler log template for React Native projects on macOS:

```json
{
  "kind": "source_template",
  "label": "Metro bundler log",
  "locator_kind": "file",
  "locator": "/tmp/metro.log",
  "tags": ["react-native", "metro"],
  "data": {
    "project_types": ["react-native"],
    "kind": "log",
    "locator_pattern": "/tmp/metro.log",
    "platforms": ["macos"],
    "default_tags": ["metro"],
    "description": "Metro bundler stdout written to /tmp by the start script",
    "discovered_at_ns": 0,
    "verified_count": 0,
    "last_verified_at_ns": 0,
    "confidence": "agent_discovered"
  }
}
```

Register a project node:

```json
{
  "kind": "project",
  "label": "rtntv-vega",
  "locator_kind": "file",
  "locator": "/Users/me/code/rtntv_vega",
  "project_slug": "rtntv-vega",
  "data": {
    "root_path": "/Users/me/code/rtntv_vega",
    "slug": "rtntv-vega",
    "classification_tags": ["react-native", "vega", "git-repo"],
    "framework_versions": { "react-native": "0.74.5" },
    "platform": "macos",
    "created_at_ns": 0,
    "last_serve_at_ns": 0,
    "skip_discovery": false
  }
}
```

Register a doc:

```json
{
  "kind": "doc",
  "label": "Vega media controls API",
  "locator_kind": "url",
  "locator": "https://developer.amazon.com/.../vega-media-controls",
  "tags": ["vega", "media", "api"]
}
```

## Next

Call `librarian_lookup` to verify the entry persisted; chain related nodes via the `edge` param on a follow-up `librarian_index` call.
