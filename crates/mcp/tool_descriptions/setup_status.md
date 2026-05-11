## Purpose

Read-only setup state: which providers are detected and configured, whether the daemon is running, and any issues.

## When

Checking whether daemon8 is wired into your AI tools. Confirming a setup_apply landed.

## Prereq

None.

## Args
  - cwd: optional string. Project working directory. Defaults to the daemon's cwd.

## Returns
  result: {providers: [{name, config_path, was_configured, action}], daemon_running, issues}.

## Errors

none expected.

## Next

setup_apply(yes=true) to register MCP server with detected providers.
