## Purpose

Write `.daemon8/config.md` for an explicit project directory. Required before daemon8 can observe the project.

## When

Use after `daemon8_connect` returns `setup_required` because project config is missing or invalid. Also callable before connect when the user explicitly asks to initialize a project path.

## Prereq

An explicit `project_path`.

## Args

- `project_path`: required project directory.
- `name`: optional project name.
- `overwrite`: replace an existing config when true.
- `ignore`: administrative override. When true, marks this project as permanently ignored by the daemon. This is NOT part of the normal setup flow -- do not call with `ignore=true` during standard setup. Call with `ignore=false` to re-enable.

## Returns

Common envelope. Success returns `code=initialized` with a `requirements` field listing mandatory actions. The `requirements` field is NOT optional -- every listed action MUST be completed before proceeding. daemon8 returns `status=blocked`, `code=config_exists` when a config already exists and `overwrite` is false. With `ignore=true`, returns `code=project_ignored`. With `ignore=false`, returns `code=project_unignored` with a `next_action` to retry `daemon8_connect`.

The generated config includes `project.id` (a slug derived from the project name) used for structured tag scoping. Structured tags (`project:`, `lang:`, `framework:`, `tool:`) are derived automatically from the config's stack metadata and applied to all observations from this project.

## Errors
  - invalid_scope: the supplied path cannot be resolved or used as a daemon8 project.
  - general_scope: no project marker found at the path. daemon8_init writes project config only -- run it from a directory with .git, Cargo.toml, package.json, composer.json, pyproject.toml, go.mod, artisan, or bin/console.

## Requirements

After a successful init, the config may have auto-detected sources or an empty `sources: []` array depending on whether daemon8 recognized the project's ecosystem. Sources are the runtime signals daemon8 watches -- logs, build output, error streams, and similar files -- so it can surface what is happening in real time. daemon8 links provider catch-up transcripts at runtime through `daemon8_connect` / `link_conversation`; do not add broad global provider transcript directories such as `~/.claude/projects` to `.daemon8/config.md` during setup. daemon8 supports explicit, intentionally curated conversation sources when the project owns that transcript path. The config is NOT usable until you verify and complete it. You MUST complete ALL of the following steps. Do NOT skip or defer any step.

### STEP 1: Verify and complete the stack

daemon8 auto-detects languages, frameworks, and tools from ecosystem markers (package manager files, framework config files, build files). The auto-detected values may be substantially correct but are NOT guaranteed complete. You MUST:

1. Read ALL package manager files to identify every dependency across the ENTIRE project: package.json, Cargo.toml, composer.json, pyproject.toml, go.mod, Gemfile, and their lock files.

2. Scan the full project structure. Run `tree -L 3 -I 'node_modules|vendor|target|.git|dist|build|__pycache__' .` to get a full view. On Windows use `tree /F` and filter manually.

3. Review containerization, CI/CD, and runtime configuration files.

4. Update the `project.stack` section with ALL languages, frameworks, and tools found across the ENTIRE project.

For workspace or monorepo roots containing multiple sub-projects, scan ALL sub-project directories. daemon8 detects workspace roots by finding ecosystem markers in child directories even when the root itself has no markers.

### STEP 2: Verify and supplement sources

Verify auto-detected source paths. Search for sources daemon8 missed — **daemon8 observes only what is declared.**

Search the entire project for:
- Application logs (storage/logs/, logs/, *.log)
- Web server logs (nginx, Apache, Caddy — access + error)
- Database query logs (slow query, general)
- Queue/worker output (Redis, RabbitMQ, SQS)
- Build output, error tracking, container logs

Use runtime tools to discover log paths programmatically. Call `daemon8_help(topic="sources")` for per-language discovery commands and the full field reference. If discovery fails, ask the user.

### STEP 3: Confirm with the user

After completing Steps 1 and 2, explain the sources in user terms, present the updated config, and ask: "Does this config look thorough? Are there other log files, build outputs, or services I should add?" Also ask: "Are there related projects -- a frontend, backend, API, mobile app, or docs site -- that share context with this one?" If yes, add them under `related_projects`. Do NOT proceed without user confirmation.

## Next

Retry `daemon8_connect` after a successful init. If the connect response includes a `requirements` field, complete every listed action before doing any other work -- daemon8 has no visibility into the project without sources.

After connect succeeds post-init, orient the user with a cohesive handoff. State how many sources daemon8 is watching and name them. Mention any discovered conversations by provider. Summarize what daemon8 can do from here: real-time log monitoring via `read_live_feed`, before/after comparisons via `create_checkpoint`, cross-session awareness via `build_context_snapshot`, and structured debugging via `start_debug_session`.
