#!/usr/bin/env bash
# Emits sample log lines across all 6 parser formats.
# Usage: ./emit.sh [--burst N] [--loop SECS]
#   --burst N    write N batches (default: 1)
#   --loop SECS  repeat every SECS seconds (ctrl-c to stop)

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIR="$SCRIPT_DIR/logs"
mkdir -p "$DIR"

burst=1
loop_secs=0

while [[ $# -gt 0 ]]; do
    case $1 in
        --burst) burst=$2; shift 2 ;;
        --loop)  loop_secs=$2; shift 2 ;;
        *) echo "unknown arg: $1"; exit 1 ;;
    esac
done

emit_batch() {
    local ts
    ts=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
    local ts_mono
    ts_mono=$(date +"%Y-%m-%d %H:%M:%S")
    local ts_syslog
    ts_syslog=$(date +"%b %e %H:%M:%S")
    local ts_clf
    ts_clf=$(date +"%d/%b/%Y:%H:%M:%S %z")

    # JSON - mixed severities, extra fields
    echo "{\"timestamp\":\"$ts\",\"level\":\"info\",\"msg\":\"request handled\",\"method\":\"GET\",\"path\":\"/api/users\",\"duration_ms\":42}" >> "$DIR/json.log"
    echo "{\"timestamp\":\"$ts\",\"level\":\"warn\",\"msg\":\"slow query detected\",\"query\":\"SELECT * FROM orders\",\"duration_ms\":1523}" >> "$DIR/json.log"
    echo "{\"timestamp\":\"$ts\",\"level\":\"error\",\"msg\":\"connection pool exhausted\",\"pool\":\"primary\",\"active\":50,\"max\":50}" >> "$DIR/json.log"
    echo "{\"timestamp\":\"$ts\",\"level\":\"debug\",\"msg\":\"cache miss\",\"key\":\"user:1234\",\"ttl\":300}" >> "$DIR/json.log"

    # logfmt - key=value pairs
    echo "ts=$ts level=info msg=\"server started\" port=8080 workers=4" >> "$DIR/logfmt.log"
    echo "ts=$ts level=warn msg=\"memory pressure\" used_mb=3800 limit_mb=4096 gc_runs=12" >> "$DIR/logfmt.log"
    echo "ts=$ts level=error msg=\"failed to connect to redis\" host=redis-01 port=6379 retries=3" >> "$DIR/logfmt.log"

    # monolog - PHP-style [datetime] channel.LEVEL: message {context} [extra]
    echo "[$ts_mono] app.INFO: User logged in {\"user_id\":42,\"ip\":\"10.0.0.1\"} []" >> "$DIR/monolog.log"
    echo "[$ts_mono] database.WARNING: Slow query detected {\"query\":\"SELECT * FROM logs\",\"time_ms\":890} []" >> "$DIR/monolog.log"
    echo "[$ts_mono] security.ERROR: Failed login attempt {\"email\":\"admin@example.com\",\"attempts\":5} []" >> "$DIR/monolog.log"
    echo "[$ts_mono] queue.DEBUG: Job dispatched {\"job\":\"SendEmail\",\"queue\":\"default\"} []" >> "$DIR/monolog.log"

    # syslog - RFC 3164
    echo "<14>${ts_syslog} sandbox-host sshd[12345]: Accepted publickey for admin from 10.0.0.1 port 22" >> "$DIR/syslog.log"
    echo "<11>${ts_syslog} sandbox-host kernel: Out of memory: Kill process 9876 (oom-victim)" >> "$DIR/syslog.log"
    echo "<12>${ts_syslog} sandbox-host cron[456]: Job completed successfully" >> "$DIR/syslog.log"
    echo "<36>${ts_syslog} sandbox-host nginx[789]: upstream timed out (110: Connection timed out)" >> "$DIR/syslog.log"

    # CLF - Combined Log Format (web access logs)
    echo "192.168.1.10 - alice [$ts_clf] \"GET /dashboard HTTP/1.1\" 200 15234 \"https://app.example.com/\" \"Mozilla/5.0 (Macintosh)\"" >> "$DIR/access.log"
    echo "10.0.0.5 - - [$ts_clf] \"POST /api/login HTTP/1.1\" 401 89 \"-\" \"curl/7.88.1\"" >> "$DIR/access.log"
    echo "172.16.0.1 - bob [$ts_clf] \"GET /missing-page HTTP/1.1\" 404 1234 \"https://app.example.com/links\" \"Mozilla/5.0 (X11; Linux)\"" >> "$DIR/access.log"
    echo "10.0.0.20 - - [$ts_clf] \"DELETE /api/data HTTP/1.1\" 500 567 \"-\" \"python-requests/2.31\"" >> "$DIR/access.log"

    # plain line - anything goes
    echo "[$ts] Application starting up..." >> "$DIR/plain.log"
    echo "[$ts] Loading configuration from /etc/app/config.yaml" >> "$DIR/plain.log"
    echo "[$ts] WARNING: deprecated API endpoint called" >> "$DIR/plain.log"

    # mixed - one of each format for auto-detect parser
    echo "{\"timestamp\":\"$ts\",\"level\":\"info\",\"msg\":\"auto-detect json line\"}" >> "$DIR/mixed.log"
    echo "ts=$ts level=error msg=\"auto-detect logfmt line\" code=500" >> "$DIR/mixed.log"
    echo "[$ts_mono] auto.WARNING: auto-detect monolog line {} []" >> "$DIR/mixed.log"
    echo "<14>${ts_syslog} sandbox-host auto[999]: auto-detect syslog line" >> "$DIR/mixed.log"
    echo "just a plain line with ERROR in it for auto-detect" >> "$DIR/mixed.log"

    echo "emitted batch at $ts (json:4 logfmt:3 monolog:4 syslog:4 clf:4 plain:3 mixed:5 = 27 lines)"
}

for ((i=1; i<=burst; i++)); do
    emit_batch
done

if [[ $loop_secs -gt 0 ]]; then
    echo "looping every ${loop_secs}s (ctrl-c to stop)"
    while true; do
        sleep "$loop_secs"
        emit_batch
    done
fi
