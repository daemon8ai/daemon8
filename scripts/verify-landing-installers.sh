#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
LANDING_DIR="${DAEMON8_LANDING_DIR:-$(cd "$ROOT_DIR/.." && pwd)/daemon8-landing}"

fail() {
  printf 'verify-landing-installers: %s\n' "$1" >&2
  exit 1
}

[ -d "$LANDING_DIR" ] || fail "landing repo not found at $LANDING_DIR"

cmp -s "$ROOT_DIR/scripts/install.sh" "$LANDING_DIR/public/install.sh" || fail "public/install.sh does not match scripts/install.sh"
cmp -s "$ROOT_DIR/scripts/install.sh" "$LANDING_DIR/server/scripts/install.sh" || fail "server/scripts/install.sh does not match scripts/install.sh"
cmp -s "$ROOT_DIR/scripts/install.ps1" "$LANDING_DIR/public/install.ps1" || fail "public/install.ps1 does not match scripts/install.ps1"
cmp -s "$ROOT_DIR/scripts/install.ps1" "$LANDING_DIR/server/scripts/install.ps1" || fail "server/scripts/install.ps1 does not match scripts/install.ps1"

printf 'verify-landing-installers: ok (%s)\n' "$LANDING_DIR"
