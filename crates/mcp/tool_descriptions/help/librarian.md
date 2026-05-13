# Librarian — Reference Catalog

## Role

The librarian is **daemon8's primary memory of project topology and learned conventions**. It catalogs *where* runtime data lives, *where* documentation can be found, and *which* fix recipes resolve which errors. Every entry is a pointer — a URL, a file path, or a vault note — never the content itself.

It is also a **complement** to whatever knowledge system the user already has (Obsidian, cloud storage, wikis, READMEs). The librarian doesn't replace that store; it indexes it.

## How agents are expected to use it

Two rules that govern almost every interaction:

1. **Before writing, look it up.** Call `librarian_lookup` before `librarian_index`. Re-indexing an identical locator is safe (auto-versioned), but blind writes pollute the graph.
2. **When you discover a stable reference, persist it.** Logs you found via `find`, config dirs you stumbled across, doc URLs the user pointed you to — file them. Future sessions read what you write.

After receiving a daemon8 discovery hint (`query_observations` returns `kind=custom`, `channel="discovery_hint"`), follow the hint's `instruction_text` literally: investigate the framework's runtime data locations and write one `source_template` per location.

After a `query_observations` response carries an entry in `daemon8.hints` that names a reusable path, file the corresponding `source_template`. The hint already identifies the path and the active project's tags.

## Node kinds

- `doc` — Documentation: READMEs, wiki pages, API docs, vault notes.
- `source_template` — Learned location pattern for a project type. The unit daemon8 reuses across projects.
- `source_instance` — A concrete resolved path for one project; usually derived from a template.
- `fix` — Known fix recipes, workarounds, error resolution steps.
- `project` — Project entry points, repo roots, classification metadata.

## Edge kinds

- `has_source` — project -> source_instance
- `derived_from` — source_instance -> source_template (when registration came from a template match)
- `documented_by` — any node -> its documentation
- `fixes` — fix -> the error or node it resolves
- `supersedes` — new version -> old version (auto-created on re-index)
- `deprecates` — replacement source_template -> drifted source_template
- `child_of` — child node -> parent node

## Locator types

- `file` — Local file path. For `source_template` patterns, use the portable forms (`~`, `$VAR`, `<root>`); see the `librarian_index` tool description for the validator rules.
- `url` — HTTP/HTTPS URL.
- `vault` — Obsidian vault note path.

## Portability rules (source_template only)

`source_template.locator_pattern` must round-trip across machines. Use `~` for home, env-vars for OS roots, `<root>` for project-relative paths. Absolute home paths (`/Users/<name>/...`, `/home/<name>/...`), Windows user paths (`C:\Users\...`), and UNC paths are rejected by the validator. `platforms` and `project_types` MUST be set explicitly — both use OR semantics on lookup. Allowed `project_types` come from a hardcoded allowlist that the discovery hint echoes back to you in `known_project_type_tags_ref`.

## Versioning (datever)

Versions use date-based identifiers in `YYYY.MM.DD` format. Same-day re-indexes append a sequence suffix: `2026.05.13`, `2026.05.13.2`, `2026.05.13.3`.

Re-indexing the same `locator_kind + locator` combination:
1. Creates the new node with today's datever (or next sequence if same day).
2. Sets `deprecated_at` on the old node.
3. Creates a `supersedes` edge new -> old.

Pass `include_deprecated=true` to `librarian_lookup` to see version history.

## Lifecycle

Every node tracks timestamps:
- `created_at` — when first indexed
- `updated_at` — last modification
- `last_read_at` — updated on every lookup hit (use `stale_before_days` to find unused references)
- `deprecated_at` — set on soft-delete or when superseded
- `canonicalized_at` — marks as authoritative reference

`source_template` carries additional state: `verified_count` (incremented when daemon8 successfully resolves the template in a project) and `confidence` (`agent_discovered` | `user_provided` | `drifted`).

## Canonicalized material

Pass `canonicalize=true` when indexing to mark a node as **the authority** on its topic. Canonicalized nodes are never flagged as stale. Use this for foundational references — not every entry.

When canonicalized material needs updating, look up the existing canonical node first. Re-index it (auto-creates a new datever and deprecates the old) rather than creating a separate, unlinked node.

## Hierarchy

Use `parent_id` to nest references under category nodes. When new material doesn't fit existing categories, ask the user what classification makes sense. Reorganize when a parent has many children (>15) or nesting is deep (>4 levels).

## Hygiene

Periodically run `librarian_lookup(stale_before_days=30)` to surface unused entries. Suggest deprecation to the user — don't auto-delete. Canonicalized nodes are excluded from stale results.

## Tools

- `librarian_index(kind, label, locator_kind, locator, data?, ..., canonicalize?)` — add or update a reference. `data` carries the kind-specific payload (`SourceTemplateData`, `ProjectNodeData`, `SourceInstanceData`).
- `librarian_lookup(id?, kinds?, tags?, text?, ...)` — search the catalog.
- `librarian_forget(id, deprecate?, confirm?)` — retire or remove a reference.
