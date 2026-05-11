## Purpose

Detect drift across every installed daemon8 hook (all providers, all scopes) and reinstall only those whose hook command no longer points at the running daemon binary. No-op for hook sets that are already current.

## When

After a daemon8 upgrade, after moving the daemon binary, or as a routine check.

## Prereq

None.

## Args

none.

## Returns
  result: array of {provider, scope, action: "ok"|"repaired", settings_path}.

## Errors

filesystem write failures bubble up as {"error": "..."}.

## Next

hooks_list confirms the post-repair state.
