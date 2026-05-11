# Librarian — Reference Catalog

## Philosophy

The librarian is a **complement** to whatever knowledge system the user already has (Obsidian, cloud storage, wikis, READMEs). It stores only high-value pointers — never content. Think of it as a relational index into material that lives elsewhere: documentation URLs, fix recipes, source configs, project protocols.

**Pointers, not content.** The librarian catalogs *where* information lives, not the information itself. The locator field points to the source of truth; the librarian makes it findable and traversable.

**Quality over quantity.** Fewer well-connected nodes beat many shallow ones. Before entering data into the graph, confirm the structural and hierarchical setup with the user. Establish top-level categories first, then populate. Resist spreading knowledge too thin — if a node won't be looked up, it adds noise.

## Node kinds

- `doc` — Documentation: READMEs, wiki pages, API docs, vault notes
- `source_template` — Log/telemetry source configurations, parser templates
- `fix` — Known fix recipes, workarounds, error resolution steps
- `project` — Project entry points, repo roots, workspace roots

## Edge kinds

- `has_source` — project -> its log source config
- `documented_by` — any node -> its documentation
- `fixes` — fix -> the error or node it resolves
- `supersedes` — new version -> old version (auto-created on re-index)
- `child_of` — child node -> parent node (mirrors parent_id for graph traversal)

## Locator types

- `file` — Local file path (relative to project root preferred)
- `url` — HTTP/HTTPS URL to external documentation
- `vault` — Obsidian vault note path

## Versioning (datever)

Versions use date-based identifiers in `YYYY.MM.DD` format. Same-day re-indexes append a sequence suffix: `2026.05.11`, `2026.05.11.2`, `2026.05.11.3`.

Re-indexing the same `locator_kind + locator` combination automatically:
1. Creates the new node with today's datever (or next sequence if same day)
2. Sets `deprecated_at` on the old node
3. Creates a `supersedes` edge from new -> old

Pass `include_deprecated=true` to `librarian_lookup` to see version history.

## Lifecycle

Every node tracks timestamps:
- `created_at` — when first indexed
- `updated_at` — last modification
- `last_read_at` — updated on every lookup hit (use `stale_before_days` to find unused references)
- `deprecated_at` — set on soft-delete or when superseded
- `canonicalized_at` — marks as authoritative reference (see below)

## Canonicalized material

Pass `canonicalize=true` when indexing to mark a node as **the authority** on its topic. Canonicalized nodes are never flagged as stale, regardless of how frequently they're accessed. Use this for foundational references that define how something works — not for every entry.

When material on a canonicalized topic needs updating, look up the existing canonical node first. Re-index it (which auto-creates a new datever version and deprecates the old) rather than creating a separate, unlinked node.

## Hierarchy

Use `parent_id` to nest references under category nodes. When new material doesn't fit existing categories, ask the user what classification makes sense. Reorganize when a parent has many children (>15) or nesting is deep (>4 levels).

## Hygiene

Periodically run `librarian_lookup(stale_before_days=30)` to surface unused entries. Suggest deprecation to the user — don't auto-delete. Canonicalized nodes are excluded from stale results. When a parent has many children, suggest subcategories.

## Tools

- `librarian_index(kind, label, locator_kind, locator, ..., canonicalize?)` — add or update a reference
- `librarian_lookup(id?, kinds?, tags?, text?, ...)` — search the catalog
- `librarian_forget(id, deprecate?, confirm?)` — retire or remove a reference
