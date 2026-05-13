## Purpose

Search the librarian catalog for reference pointers. Returns locators (where things are), not content. The librarian is daemon8's primary memory of project topology and learned conventions — `librarian_lookup` is how you read it.

## When to use (mandatory)

Before calling `librarian_index` for any new path or URL, ALWAYS call `librarian_lookup` first to check for an existing entry. Re-indexing an identical locator is fine (auto-versioned), but blind double-writes pollute the graph and obscure what was learned by which session.

After receiving a daemon8 discovery hint, lookup before you investigate:
1. Lookup `kind=source_template` filtered by the project's `classification_tags` — see what daemon8 already knows.
2. Only run shell discovery for the tags that come back empty.

When the user references a topic that might already be cataloged (a runbook, a fix recipe, a project root), lookup first; cite the existing entry instead of re-creating it.

## Triggers (non-exhaustive)

- A discovery hint observation arrives — lookup before investigating.
- The user mentions a project, framework, or error you suspect has been seen before.
- You are about to call `librarian_index` — lookup the locator first.
- You need to find which `source_template` entries cover the active project's tags.
- Catalog hygiene: find stale entries with `stale_before_days`.

## Args
- `id`: optional string. Look up a single node by ID. Returns the node plus all its edges.
- `kinds`: optional list. Filter by kind: `doc` | `source_template` | `fix` | `project` | `source_instance`.
- `tags`: optional list. Filter by tags. Use this with `classification_tags` to find templates that apply to the active project.
- `project_slug`: optional string. Scope to a project.
- `text`: optional string. Case-insensitive search across label and locator.
- `limit`: optional int. Max results (default 20, max 500).
- `include_deprecated`: optional bool. Include superseded/deprecated entries. Default false.
- `stale_before_days`: optional int. Find nodes not accessed in N days.
- `parent_id`: optional string. Browse children of a specific catalog node.

## Returns
- `result: {nodes: [...]}` for filter queries, or `{node, edges: [...]}` for single-ID lookup.

## Errors
- `librarian_store_unavailable`: catalog not configured.

## Examples

Find every source template that applies to a React Native + Vega project on macOS:

```json
{ "kinds": ["source_template"], "tags": ["react-native"] }
```

(Filter the results client-side by `data.platforms` — the librarian filter is tag-based; platform filtering happens on the returned `data` payload.)

Look up a project by slug:

```json
{ "kinds": ["project"], "project_slug": "rtntv-vega" }
```

Find every Doc node about Vega:

```json
{ "kinds": ["doc"], "tags": ["vega"] }
```

Find stale entries that haven't been read in 30 days:

```json
{ "stale_before_days": 30 }
```

## Next

`librarian_index` to add missing references; `librarian_forget(deprecate=true)` to retire stale entries.
