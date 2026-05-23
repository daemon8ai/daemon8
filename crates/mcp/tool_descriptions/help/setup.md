# Setup protocol

Setup is mandatory. daemon8 cannot observe a project without `.daemon8/config.md`.

## When setup fires

`daemon8_connect` returns `setup_required` when no valid config exists at the project directory. This happens on first use and when config is corrupted or missing.

## Setup steps

1. Call `daemon8_init(project_path)` to generate `.daemon8/config.md`.
2. Open the generated config. Complete all steps in the markdown body: verify the stack, audit log paths, populate sources.
3. Complete ALL steps. Do not skip or defer -- daemon8 has no visibility into the project without sources. Search for every log file, build output, error stream, and runtime signal the project produces.
4. Explain the sources to the user in plain terms. Present the updated config and confirm.
5. Replace the generated config body with project-specific notes. `daemon8_connect` flags configs that still contain the generated setup instructions.
6. Retry `daemon8_connect`. If the response includes a `requirements` field, complete every listed action before proceeding.

## What to look for during source discovery

- Application logs (storage/logs/, logs/, log/, *.log, stdout/stderr)
- Build output and compilation logs
- Web server access and error logs (nginx, Apache, Caddy)
- Database query logs (slow query logs, general logs)
- Queue and worker logs (Redis, RabbitMQ, SQS worker output)
- Error tracking output (crash logs, exception dumps)
- Container logs (docker-compose log paths, k8s pod logs)

Call `daemon8_help(topic="sources")` for the full field schema and parser reference.

## Workspace and monorepo detection

daemon8 detects workspace roots by scanning immediate child directories for ecosystem markers (Cargo.toml, package.json, etc.). A directory without root-level markers can still be classified as a project if its children contain recognizable ecosystems. The init flow handles workspaces the same as single projects.
