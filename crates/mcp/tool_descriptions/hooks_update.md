## Purpose

Reinstall daemon8 hook entries (force=true). Replaces any prior daemon8 entries — useful when the daemon binary path has moved, the hook spec list has changed, or a stale entry needs to be flushed.

## When

After upgrading daemon8 to a new install location, after edits to the canonical hook spec set, or when hooks_list shows entries with a stale command path.

## Prereq

None.

## Args
  - provider: REQUIRED. "claude", "codex", or "gemini".
  - scope: optional, claude only. "local" | "shared" | "global". Omit to update all three claude scopes.

## Returns
  result: array of {provider, scope, action: "updated", settings_path}.

## Errors
  - missing provider; unknown provider; unknown scope; settings file write failures.

## Next

hooks_list to verify; hooks_repair to scan all providers for further drift.
