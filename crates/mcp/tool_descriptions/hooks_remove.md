## Purpose

Uninstall daemon8 hook entries from a provider's settings. Other (non-daemon8) hooks in the same file are left untouched.

## When

Backing daemon8 out of a project, switching to a different telemetry tool, or removing accidentally-installed hooks.

## Prereq

None.

## Args
  - provider: REQUIRED. "claude", "codex", or "gemini".
  - scope: optional, claude only. "local" | "shared" | "global". Omit to remove from all three claude scopes.

## Returns
  result: array of {provider, scope, action: "removed"|"noop", settings_path}.

## Errors
  - missing provider; unknown provider; unknown scope. Returns {"error": "..."}.

## Next

hooks_list to confirm; setup_apply if you want to reinstall.
