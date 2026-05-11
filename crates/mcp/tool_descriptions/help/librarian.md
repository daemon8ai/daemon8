# Librarian — Reference Catalog

## When to use

The librarian catalogs *pointers* to where information lives — not the information itself. Use it when you discover a documentation URL, a log source config file, a known fix recipe, or a project entry point that should be retrievable later.

## Node kinds

- `doc` — Documentation: READMEs, wiki pages, API docs, vault notes
- `source_template` — Log/telemetry source configurations, parser templates
- `fix` — Known fix recipes, workarounds, error resolution steps
- `project` — Project entry points, repo roots, workspace roots

## Edge kinds

- `has_source` — project → its log source config
- `documented_by` — any node → its documentation
- `fixes` — fix → the error or node it resolves
- `supersedes` — new version → old version (auto-created on re-index)
- `child_of` — child node → parent node (mirrors parent_id for graph traversal)

## Locator types

- `file` — Local file path (relative to project root preferred)
- `url` — HTTP/HTTPS URL to external documentation
- `vault` — Obsidian vault note path

## Versioning

Re-indexing the same `locator_kind + locator` combination automatically:
1. Creates the new node with `version = old_version + 1`
2. Sets `deprecated_at` on the old node
3. Creates a `supersedes` edge from new → old

Pass `include_deprecated=true` to `librarian_lookup` to see version history.

## Lifecycle

Every node tracks timestamps:
- `created_at` — when first indexed
- `updated_at` — last modification
- `last_read_at` — updated on every lookup hit (use `stale_before_days` to find unused references)
- `deprecated_at` — set on soft-delete or when superseded

## Hierarchy

Use `parent_id` to nest references under category nodes. When new material doesn't fit existing categories, ask the user what classification makes sense. Reorganize when a parent has many children (>15) or nesting is deep (>4 levels).

## Hygiene

Periodically run `librarian_lookup(stale_before_days=30)` to surface unused entries. Suggest deprecation to the user — don't auto-delete. When a parent has many children, suggest subcategories.

## Tools

- `librarian_index(kind, label, locator_kind, locator, ...)` — add or update a reference
- `librarian_lookup(id?, kinds?, tags?, text?, ...)` — search the catalog
- `librarian_forget(id, deprecate?, confirm?)` — retire or remove a reference
