## Purpose

Index a reference pointer into the librarian catalog — the librarian stores *where* information lives (URLs, file paths, vault notes), not the information itself.

## When

Cataloging a documentation URL, a known log source configuration file, a fix recipe, or a project entry point. Re-indexing the same locator auto-bumps the version and deprecates the previous entry.

## Prereq

None.

## Args
  - kind: required string. doc | source_template | fix | project.
  - label: required string. Human-readable name for this reference.
  - locator_kind: required string. file | url | vault.
  - locator: required string. The actual pointer — a file path, URL, or vault note path.
  - tags: optional list of strings. Free-form retrieval tags.
  - project_slug: optional string. Scope for retrieval.
  - parent_id: optional string. Place this node under an existing catalog node for hierarchy.
  - edge: optional object { kind: string, target_node_id: string }. Create a relationship edge at index time.
  - canonicalize: optional bool. Mark as authoritative reference — canonicalized nodes are never flagged as stale.

## Returns
  result: {id, version}.

## Errors
  - librarian_store_unavailable: catalog not configured.
  - invalid_kind: kind string not recognized.
  - invalid_locator_kind: locator_kind string not recognized.

## Next

librarian_lookup to verify; link related nodes via edge param on a second librarian_index call.
