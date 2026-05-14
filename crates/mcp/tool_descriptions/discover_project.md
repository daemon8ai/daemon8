## Purpose

Classify an explicit project root and ask daemon8/librarian what source coverage is known, missing, or ready to register.

## When

Use at session start, after compaction, or before debugging a project whose source coverage is unknown. This is the on-demand replacement for daemon-start discovery; daemon8 does not infer a project from its own current directory.

## Prereq

`project_root` must be the repository or application root you want daemon8 to inspect.

## Args
  - project_root: required string. Explicit project root path.

## Returns
  result.plan: structured discovery plan.
  result.report: compact rendered report for the agent/user.
  result.next_actions: suggested follow-up tools, usually `librarian_index`, `awareness_status`, or `query_observations`.

## Errors
  - missing_project_root: project_root was empty.
  - project_discovery_unavailable: daemon was started without the discovery callback.

## Next

If source templates are missing, investigate the project/framework source locations and register durable reusable refs with `librarian_index`. Then call `awareness_status(project_root=...)` to verify source awareness before relying on checkpoint deltas.
