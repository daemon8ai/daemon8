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

Common envelope. Success returns `code=initialized` with a `requirements` field listing mandatory actions. The `requirements` field is NOT optional -- every listed action MUST be completed before proceeding. An existing config without overwrite returns `status=blocked`, `code=config_exists`. With `ignore=true`, returns `code=project_ignored`. With `ignore=false`, returns `code=project_unignored` with a `next_action` to retry `daemon8_connect`.

The generated config includes `project.id` (a slug derived from the project name) used for structured tag scoping. Structured tags (`project:`, `lang:`, `framework:`, `tool:`) are derived automatically from the config's stack metadata and applied to all observations from this project.

To declare sibling projects (e.g. a frontend/backend pair), add a `related_projects` map to the config keyed by project id:

```yaml
related_projects:
  frontend:
    path: "$PRJ_ROOT/../frontend"
```

## Requirements

After a successful init, the generated config has `sources: []`. The config is NOT usable until sources are populated. You MUST complete ALL of the following steps. Do NOT skip or defer any step.

### STEP 1: Full project audit

You MUST build a complete picture of this project before adding sources. Do ALL of the following IN ORDER:

1. Read ALL package manager files to identify every dependency across the ENTIRE project: package.json, Cargo.toml, composer.json, pyproject.toml, go.mod, Gemfile, and their lock files.

2. Scan the full project structure. Run `tree -L 3 -I 'node_modules|vendor|target|.git|dist|build|__pycache__' .` to get a full view. On Windows use `tree /F` and filter manually.

3. Review containerization and infrastructure: Dockerfile, docker-compose.yml, .dockerignore, Kubernetes manifests, terraform files, serverless configs.

4. Review deployment and CI/CD: .github/workflows/, .gitlab-ci.yml, Jenkinsfile, deploy scripts, Procfile, Caddyfile, nginx configs.

5. Review runtime configuration: .env.example, config files, environment-specific settings, database configs, queue configs, cache configs.

After this audit, update the `project.stack` section in the frontmatter with ALL languages, frameworks, and tools found across the ENTIRE project. The auto-detected values are a starting point only -- they are NOT complete.

### STEP 2: Add sources

Using what you learned in Step 1, add file and conversation source entries to the `sources` array in the frontmatter. You MUST investigate every log path, build output, and error stream the project produces. Do NOT stop at the obvious ones -- dig through config files, docker entrypoints, and supervisor configs to find ALL log outputs.

daemon8 supports these log parsers. ANY log file that matches one of these formats MUST be added as a source:

- `line` -- plain text, one observation per line (catch-all)
- `json` -- structured JSON logs (one JSON object per line)
- `syslog` -- RFC 3164/5424 syslog format
- `logfmt` -- key=value structured logs (Heroku, Go stdlib)
- `clf` -- Common/Combined Log Format (Apache, nginx access logs)
- `monolog` -- PHP Monolog format (Laravel, Symfony)
- `auto` -- tries all parsers, picks the best match
- `grok` -- custom pattern (set `parser_pattern` field)

Search for ALL of these across the entire project:
- Application logs (storage/logs/, logs/, log/, *.log, stdout/stderr)
- Build output and compilation logs
- Web server access and error logs (nginx, Apache, Caddy)
- Database query logs (slow query logs, general logs)
- Queue and worker logs (Redis, RabbitMQ, SQS worker output)
- Error tracking output (crash logs, exception dumps)
- Container logs (docker-compose log paths, k8s pod logs)
- Provider transcripts for session continuity

Source field schemas:

file source fields: `id` (unique identifier, e.g. "app.logs"), `service` (logical service name, e.g. "app"), `kind` (must be "file"), `path` (log file path, use $PRJ_ROOT for project-relative), `parser` (line | json | syslog | logfmt | clf | monolog | auto | grok), `parser_pattern` (required only when parser is grok), `tags` (optional string array).

conversation source fields: `id` (unique identifier, e.g. "claude.sessions"), `service` (provider name, e.g. "claude"), `kind` (must be "conversation"), `path` (directory containing provider transcripts), `provider` (claude | codex | gemini), `tags` (optional string array).

Example populated sources array:

```yaml
sources:
  - id: app.logs
    service: app
    kind: file
    path: "$PRJ_ROOT/storage/logs/laravel.log"
    parser: monolog

  - id: nginx.access
    service: nginx
    kind: file
    path: "/var/log/nginx/access.log"
    parser: clf

  - id: worker.output
    service: queue
    kind: file
    path: "$PRJ_ROOT/logs/worker.log"
    parser: json

  - id: claude.sessions
    service: claude
    kind: conversation
    provider: claude
    path: "~/.claude/projects"
```

### STEP 3: Confirm with the user

After completing Steps 1 and 2, present the updated config to the user and ask: "Does this config look thorough? Are there other log files, build outputs, or services I should add?" Do NOT proceed without user confirmation.

## Next

Retry `daemon8_connect` after a successful init. If the connect response includes a `requirements` field, you MUST complete every listed action before doing any other work. This is not optional -- daemon8 has no visibility into the project without sources.

After `ignore=true`, no further action is needed -- `daemon8_connect` will return `blocked/project_ignored`. After `ignore=false`, follow the `next_action` to reconnect.
