## Purpose

Alias for setup_status. Returns the same read-only state report. Kept for backward compatibility.

## When

Previewing what providers are detected before calling setup_apply.

## Prereq

None.

## Args
  - cwd: optional string. Project working directory for provider config context. Omit only when provider setup is global.

## Returns
  result: same shape as setup_status.

## Errors

none expected.

## Next

setup_apply(yes=true) to register MCP server with detected providers.
