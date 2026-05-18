#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
LOCAL_INSTALLER="$SCRIPT_DIR/scripts/install.sh"
REMOTE_INSTALLER="${DAEMON8_INSTALLER_SCRIPT_URL:-https://daemon8.ai/install.sh}"

if [ -f "$LOCAL_INSTALLER" ]; then
  exec "$LOCAL_INSTALLER" "$@"
fi

if [ "${DAEMON8_INSTALLER_SELF_TEST:-}" = "1" ]; then
  printf 'installer fallback: %s\n' "$REMOTE_INSTALLER"
  exit 0
fi

curl -fsSL "$REMOTE_INSTALLER" | bash -s -- "$@"
