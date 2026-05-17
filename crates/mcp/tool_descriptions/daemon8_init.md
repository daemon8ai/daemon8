## Purpose

Write `.daemon8/config.md` for an explicit project directory.

## When

Use after `daemon8_connect` returns `setup_required` because project config is missing or invalid. This tool is also callable before connect when the user explicitly asks to initialize a project path.

## Prereq

An explicit `project_path`.

## Args

- `project_path`: required project directory.
- `name`: optional project name.
- `overwrite`: replace an existing config when true.

## Returns

Common envelope. Success returns `code=initialized`; an existing config without overwrite returns `status=blocked`, `code=config_exists`.

## Next

Retry `daemon8_connect` after a successful init.
