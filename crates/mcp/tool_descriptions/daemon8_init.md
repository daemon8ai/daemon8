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

The generated config includes `project.id` (slug from project name). Structured tags (`project:`, `lang:`, `framework:`, `tool:`) derive from stack metadata and apply to all observations automatically.

## Errors
  - invalid_scope: the supplied path cannot be resolved or used as a daemon8 project.
  - general_scope: no project marker found at the path. daemon8_init writes project config only -- run it from a directory with .git, Cargo.toml, package.json, composer.json, pyproject.toml, go.mod, artisan, or bin/console.

## Requirements

After init, the config may have auto-detected sources or an empty `sources: []` depending on ecosystem recognition. Sources are the runtime signals daemon8 watches -- logs, build output, error streams. **Do not** add broad provider directories (`~/.claude/projects`) -- daemon8 links transcripts at runtime via `daemon8_connect` / `link_conversation`. The config is **not usable** until all steps below are complete.

### STEP 1: Verify and complete the stack

daemon8 auto-detects languages, frameworks, and tools from ecosystem markers. Auto-detection may be incomplete. **Verify and complete:**

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

Present the updated config and ask: "Does this config look thorough? Are there other log files, build outputs, or services I should add?" Also ask about related projects (frontend, backend, API, mobile app). Add confirmed ones under `related_projects`. **Do not proceed without user confirmation.**

### STEP 4: Replace generated config body

The generated config body contains daemon8's setup instructions. After completing Steps 1-3, **replace** the markdown body (everything after the frontmatter `---`) with concise project-specific notes: dev/test commands, service startup, build outputs, environment assumptions, project-specific gotchas. Do not repeat sources or stack already in frontmatter. `daemon8_connect` will continue flagging this as a requirement until the generated instructions are replaced.

## Next

Retry `daemon8_connect` after a successful init. If the connect response includes a `requirements` field, complete every listed action before doing any other work -- daemon8 has no visibility into the project without sources.

After connect succeeds post-init, state source count and names, mention discovered conversations by provider, summarize capabilities: `read_live_feed`, `create_checkpoint`, `build_context_snapshot`, `start_debug_session`.
