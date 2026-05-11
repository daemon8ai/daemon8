## Purpose

Search the librarian catalog for reference pointers. Returns locators (where things are), not content.

## When

Finding known documentation, log source configs, fixes for an error, or browsing a project's reference hierarchy. Also used for catalog hygiene — finding stale entries not accessed in N days.

## Prereq

None.

## Args
  - id: optional string. Look up a single node by ID. Returns the node plus all its edges.
  - kinds: optional list. Filter by kind: doc | source_template | fix | project.
  - tags: optional list. Filter by tags.
  - project_slug: optional string. Scope to a project.
  - text: optional string. Case-insensitive search across label and locator.
  - limit: optional int. Max results (default 20, max 500).
  - include_deprecated: optional bool. Include superseded/deprecated entries. Default false.
  - stale_before_days: optional int. Find nodes not accessed in N days.
  - parent_id: optional string. Browse children of a specific catalog node.

## Returns
  result: {nodes: [...]} or {node, edges: [...]} for single-ID lookup.

## Errors
  - librarian_store_unavailable: catalog not configured.

## Next

librarian_index to add missing references; librarian_forget(deprecate=true) to retire stale entries.
