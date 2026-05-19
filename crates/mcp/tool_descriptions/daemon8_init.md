## Purpose

Write `.daemon8/config.md` for an explicit project directory, or mark the project as ignored so `daemon8_connect` stops prompting for setup.

## When

Use after `daemon8_connect` returns `setup_required` because project config is missing or invalid. Also callable before connect when the user explicitly asks to initialize a project path. When the user declines setup, call with `ignore=true` to suppress future `setup_required` responses for this project.

## Prereq

An explicit `project_path`.

## Args

- `project_path`: required project directory.
- `name`: optional project name.
- `overwrite`: replace an existing config when true.
- `ignore`: when true, mark this project as ignored instead of initializing. `daemon8_connect` will return `project_ignored` instead of `setup_required` for ignored projects. Call with `ignore=false` to re-enable.

## Returns

Common envelope. Success returns `code=initialized` with a hint to populate sources; an existing config without overwrite returns `status=blocked`, `code=config_exists`. With `ignore=true`, returns `code=project_ignored`. With `ignore=false`, returns `code=project_unignored` with a `next_action` to retry `daemon8_connect`.

The generated config includes `project.id` (a slug derived from the project name) used for structured tag scoping. Structured tags (`project:`, `lang:`, `framework:`, `tool:`) are derived automatically from the config's stack metadata and applied to all observations from this project.

To declare sibling projects (e.g. a frontend/backend pair), add a `related_projects` map to the config keyed by project id:

```yaml
related_projects:
  frontend:
    path: "$PRJ_ROOT/../frontend"
```

## Next

Retry `daemon8_connect` after a successful init. After connecting, populate `.daemon8/config.md` sources with file and conversation entries for this project's logs, build output, and provider transcripts -- this is a one-time investment that enables daemon8's runtime observation for the project. After `ignore=true`, no further action is needed -- `daemon8_connect` will return `blocked/project_ignored`. After `ignore=false`, follow the `next_action` to reconnect.
