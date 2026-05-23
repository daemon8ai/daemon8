# Sources

Sources are the runtime signals daemon8 watches -- log files, build output, and error streams. They are declared in `.daemon8/config.md` under the `sources:` frontmatter key. Without file sources, daemon8 has no live project visibility.

## File sources

File sources watch a path on disk and parse new lines as observations.

Fields:
- `id` -- unique identifier (e.g. "app.logs")
- `service` -- logical service name (e.g. "app")
- `kind` -- must be "file"
- `path` -- log file path; use `$PRJ_ROOT` for project-relative paths
- `parser` -- parsing strategy (see parser list below)
- `parser_pattern` -- required only when parser is "grok"
- `tags` -- optional string array for filtering

## Conversation sources

Conversation sources declare a curated project-owned transcript path. They are config metadata today, not live-feed read-through sources. For current cross-provider catch-up, use `daemon8_connect` / `link_conversation` and then `build_context_snapshot`.

Fields:
- `id` -- unique identifier (e.g. "project.codex.sessions")
- `service` -- provider name (e.g. "codex")
- `kind` -- must be "conversation"
- `path` -- project-owned file or directory containing provider transcripts
- `provider` -- one of: claude, codex, gemini
- `tags` -- optional string array for filtering

Do not use conversation sources for broad provider roots like `~/.claude/projects`. Use `link_conversation` at runtime for those.

## Supported parsers

- `line` -- plain text, one observation per line (catch-all)
- `json` -- structured JSON logs (one JSON object per line)
- `syslog` -- RFC 3164/5424 syslog format
- `logfmt` -- key=value structured logs (Heroku, Go stdlib)
- `clf` -- Common/Combined Log Format (Apache, nginx access logs)
- `monolog` -- PHP Monolog format (Laravel, Symfony)
- `auto` -- tries all parsers, picks the best match
- `grok` -- custom pattern (set `parser_pattern` field, e.g. `"%{TIMESTAMP_ISO8601:ts} %{LOGLEVEL:level} %{GREEDYDATA:msg}"`)

## Example

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
```

## Discovering log paths

Use runtime tools to find paths. Do not guess.

- **PHP**: `php-fpm -tt 2>&1 | grep error_log` or `php -i | grep error_log`
- **Python**: `grep -rn "FileHandler\|RotatingFileHandler\|basicConfig.*filename" . --include="*.py"`. Django: check `LOGGING` in settings. gunicorn/uvicorn: check `--access-logfile` / `--error-logfile` in process config.
- **Ruby**: `grep -rn "Logger\.new" . --include="*.rb"`. Rails default: `log/{environment}.log`.
- **Node.js**: no default log file (stdout only). Check for pm2 (`~/.pm2/logs/`) or logging library config (winston, pino).

If these return nothing, ask the user for the path or command.

## Related projects

Declare sibling projects under `related_projects` in config frontmatter so agents can see the relationship during `daemon8_connect`. Automatic cross-project query expansion is not implemented today.

```yaml
related_projects:
  frontend:
    path: "$PRJ_ROOT/../frontend"
```
