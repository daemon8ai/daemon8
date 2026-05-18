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
  if ! grep -Eq "^## v${workspace_version} - [0-9]{4}-[0-9]{2}-[0-9]{2}$" CHANGELOG.md; then
    fail "CHANGELOG.md must promote v${workspace_version} from Unreleased before tag release"
  fi
fi

bash -n scripts/install.sh
bash -n install.sh

if command -v pwsh >/dev/null 2>&1; then
  pwsh -NoLogo -NoProfile -Command "\$ErrorActionPreference='Stop'; [scriptblock]::Create((Get-Content -Raw 'scripts/install.ps1')) | Out-Null; [scriptblock]::Create((Get-Content -Raw 'install.ps1')) | Out-Null"
fi

DAEMON8_INSTALLER_SELF_TEST=1 bash scripts/install.sh >/dev/null
DAEMON8_INSTALLER_SELF_TEST=1 bash install.sh >/dev/null

if grep -R "cargo install daemon8" install.sh install.ps1 scripts/install.sh scripts/install.ps1; then
  fail "installer fallback must not imply crates.io publish"
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
cp install.sh "$tmp_dir/install.sh"
if ! (cd "$tmp_dir" && DAEMON8_INSTALLER_SELF_TEST=1 bash install.sh >/dev/null); then
  fail "root shell installer must work outside a checkout"
fi
if command -v pwsh >/dev/null 2>&1; then
  pwsh -NoLogo -NoProfile -Command "\$ErrorActionPreference='Stop'; \$env:DAEMON8_INSTALLER_SELF_TEST='1'; ./scripts/install.ps1 | Out-Null; ./install.ps1 | Out-Null"
  cp install.ps1 "$tmp_dir/install.ps1"
  if ! (cd "$tmp_dir" && pwsh -NoLogo -NoProfile -Command "\$ErrorActionPreference='Stop'; \$env:DAEMON8_INSTALLER_SELF_TEST='1'; ./install.ps1 | Out-Null"); then
    fail "root PowerShell installer must work outside a checkout"
  fi
fi

metadata="$(cargo metadata --no-deps --format-version 1)"
package_count="$(printf '%s' "$metadata" | { grep -o '"id":"path+file://' || true; } | wc -l | tr -d ' ')"
publish_false_count="$(printf '%s' "$metadata" | { grep -o '"publish":\[\]' || true; } | wc -l | tr -d ' ')"
if [ "$package_count" = "0" ] || [ "$publish_false_count" != "$package_count" ]; then
  fail "workspace crates must remain publish=false until crates.io release is intentionally enabled"
fi

if grep -R -n -E 'cargo (install|binstall) daemon8|crates\.io|cargo-binstall|Homebrew|homebrew' README.md DEPLOY.md; then
  fail "public install docs must not imply crates.io publish"
fi

if grep -n "if: env.DEPLOY_HOST != ''" .github/workflows/release.yml; then
  fail "release workflow must not create a GitHub release when server upload is skipped"
fi

cargo metadata --no-deps --format-version 1 >/dev/null
cargo fmt --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check

stale_surface_list="$tmp_dir/stale-surface-files"
git ls-files \
  README.md \
  DEPLOY.md \
  SECURITY.md \
  CHANGELOG.md \
  CONTRIBUTING.md \
  CODE_OF_CONDUCT.md \
  example-config.toml \
  install.sh \
  install.ps1 \
  .github \
  scripts \
  crates \
  | grep -v '^scripts/release-check.sh$' > "$stale_surface_list"

stale_pattern='setup_(apply|status|plan)|\.daemon8\.toml|awareness|librarian|discovery|debug_summary|debug_observe|provider hooks|old setup|deprecated alias|migration shim|:host/codex\+worker>'
if grep -n -E "$stale_pattern" $(cat "$stale_surface_list"); then
  fail "stale alpha release-surface wording found"
fi

current_changelog_hits="$(awk '/^## v0\.3\.0/ { exit } { print }' CHANGELOG.md | grep -n -E "$stale_pattern" || true)"
if [ -n "$current_changelog_hits" ]; then
  printf '%s\n' "$current_changelog_hits" >&2
  fail "stale current changelog wording found"
fi

printf 'release-check: ok (version %s)\n' "$workspace_version"
