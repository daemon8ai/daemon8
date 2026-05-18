#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

fail() {
  printf 'release-check: %s\n' "$1" >&2
  exit 1
}

workspace_version="$(
  awk '
    /^\[workspace\.package\]/ { in_section = 1; next }
    /^\[/ && in_section { exit }
    in_section && $1 == "version" {
      gsub(/"/, "", $3)
      print $3
      exit
    }
  ' Cargo.toml
)"

[ -n "$workspace_version" ] || fail "workspace package version not found"

awk -v want="$workspace_version" '
  /^daemon8-/ && $0 ~ /version =/ {
    line = $0
    sub(/^.*version = "/, "", line)
    sub(/".*$/, "", line)
    if (line != want) {
      printf "internal dependency version mismatch: %s (want %s)\n", $0, want > "/dev/stderr"
      exit 1
    }
  }
' Cargo.toml || fail "internal dependency versions must match workspace version"

ref="${RELEASE_REF:-${GITHUB_REF_NAME:-}}"
if [ -z "$ref" ] && [ "${GITHUB_REF:-}" != "" ]; then
  ref="${GITHUB_REF#refs/tags/}"
fi
ref="${ref#refs/tags/}"

if [[ "$ref" == v* ]]; then
  tag_version="${ref#v}"
  [ "$tag_version" = "$workspace_version" ] || fail "tag $ref does not match workspace version $workspace_version"
fi

bash -n scripts/install.sh
bash -n install.sh

if command -v pwsh >/dev/null 2>&1; then
  pwsh -NoLogo -NoProfile -Command "\$ErrorActionPreference='Stop'; [scriptblock]::Create((Get-Content -Raw 'scripts/install.ps1')) | Out-Null; [scriptblock]::Create((Get-Content -Raw 'install.ps1')) | Out-Null"
fi

if grep -R "cargo install daemon8" install.sh install.ps1 scripts/install.sh scripts/install.ps1; then
  fail "installer fallback must not imply crates.io publish"
fi

cargo metadata --no-deps --format-version 1 >/dev/null
cargo fmt --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check

if grep -R -n -E 'setup_(apply|status|plan)|\.daemon8\.toml|awareness|librarian|discovery|debug_summary|debug_observe|provider hooks|old setup|deprecated alias|migration shim' README.md crates/mcp/tool_descriptions; then
  fail "stale alpha release-surface wording found"
fi

printf 'release-check: ok (version %s)\n' "$workspace_version"
