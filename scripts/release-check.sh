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
command -v jq >/dev/null 2>&1 || fail "jq is required for release metadata checks"

if [ "$(sed -n '1p' .gitignore)" != ".*" ] || [ "$(sed -n '2p' .gitignore)" != "*.md" ]; then
  fail ".gitignore must start with default-deny rules: .* then *.md"
fi

tracked_context="$(git ls-files | grep -E '^(\.claude/|\.aimind/|CLAUDE\.md$|AGENTS\.md$)' || true)"
if [ -n "$tracked_context" ]; then
  printf '%s\n' "$tracked_context" >&2
  fail "AI context files must not be tracked"
fi

tracked_hidden_or_markdown="$(git ls-files | grep -E '^\.|\.md$' || true)"
unexpected_tracked_context="$(
  printf '%s\n' "$tracked_hidden_or_markdown" | grep -Ev '^$|^\.github/|^\.gitignore$|^(CHANGELOG|CODE_OF_CONDUCT|CONTRIBUTING|DEPLOY|README|SECURITY)\.md$|^crates/mcp/tool_descriptions/.*\.md$' || true
)"
if [ -n "$unexpected_tracked_context" ]; then
  printf '%s\n' "$unexpected_tracked_context" >&2
  fail "tracked hidden/markdown files must be explicitly allowlisted"
fi

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
bash -n scripts/installer-artifact-smoke.sh
bash -n scripts/verify-landing-installers.sh
bash -n scripts/verify-hosted-installers.sh

if command -v pwsh >/dev/null 2>&1; then
  pwsh -NoLogo -NoProfile -Command "\$ErrorActionPreference='Stop'; [scriptblock]::Create((Get-Content -Raw 'scripts/install.ps1')) | Out-Null"
  pwsh -NoLogo -NoProfile -Command "\$ErrorActionPreference='Stop'; [scriptblock]::Create((Get-Content -Raw 'scripts/installer-artifact-smoke.ps1')) | Out-Null"
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

DAEMON8_INSTALLER_SELF_TEST=1 bash scripts/install.sh >/dev/null

if grep -R "cargo install daemon8" scripts/install.sh scripts/install.ps1; then
  fail "installer fallback must not imply crates.io publish"
fi

if command -v pwsh >/dev/null 2>&1; then
  pwsh -NoLogo -NoProfile -Command "\$ErrorActionPreference='Stop'; \$env:DAEMON8_INSTALLER_SELF_TEST='1'; ./scripts/install.ps1 | Out-Null"
fi

metadata="$(cargo metadata --no-deps --format-version 1)"
if ! printf '%s' "$metadata" | jq -e --arg version "$workspace_version" '.packages | map(select(.name | startswith("daemon8"))) | length > 0 and all(.version == $version)' >/dev/null; then
  fail "daemon8 workspace crate versions must match workspace version"
fi
if ! printf '%s' "$metadata" | jq -e '.packages | length > 0 and all(.publish == [])' >/dev/null; then
  fail "workspace crates must remain publish=false until crates.io release is intentionally enabled"
fi

if grep -R -n -E 'cargo (install|binstall) daemon8|crates\.io|cargo-binstall|Homebrew|homebrew' README.md DEPLOY.md; then
  fail "public install docs must not imply crates.io publish"
fi

if grep -n "if: env.DEPLOY_HOST != ''" .github/workflows/release.yml; then
  fail "release workflow must not gate GitHub release creation on server upload config"
fi
release_lines="$(grep -n 'softprops/action-gh-release' .github/workflows/release.yml | cut -d: -f1)"
release_line_count="$(printf '%s\n' "$release_lines" | sed '/^$/d' | wc -l | tr -d ' ')"
release_line="$(printf '%s\n' "$release_lines" | sed '/^$/d' | head -n 1)"
upload_line="$(grep -n 'name: Upload to server' .github/workflows/release.yml | cut -d: -f1)"
verify_release_line="$(grep -n 'name: Verify GitHub release assets' .github/workflows/release.yml | cut -d: -f1)"
if [ "$release_line_count" != "1" ] || [ -z "$release_line" ]; then
  fail "release workflow must contain exactly one GitHub release step"
fi
if [ -n "$upload_line" ] && [ "$release_line" -ge "$upload_line" ]; then
  fail "release workflow must create the GitHub release before server upload"
fi
if [ -z "$verify_release_line" ] || [ "$verify_release_line" -le "$release_line" ]; then
  fail "release workflow must verify GitHub release assets after release creation"
fi
release_step_line="$(grep -n 'name: Create GitHub release' .github/workflows/release.yml | cut -d: -f1)"
next_step_line="$(awk -v start="$release_step_line" 'NR > start && /^[[:space:]]+- name:/ { print NR; exit }' .github/workflows/release.yml)"
release_step_end_line=$((next_step_line - 1))
if sed -n "${release_step_line},${release_step_end_line}p" .github/workflows/release.yml | grep -n '^[[:space:]]*if:'; then
  fail "GitHub release step must not be conditional"
fi
hardcoded_alpha_tag='v[0-9]+\.[0-9]+\.[0-9]+-alpha\.[0-9]+'
if grep -R -n -E "$hardcoded_alpha_tag" scripts/install.sh scripts/install.ps1; then
  fail "installer fallback examples must not hardcode old alpha tags"
fi
if grep -R -n 'releases/latest/download' scripts/install.sh scripts/install.ps1; then
  fail "installers must use explicit resolved release tags, not releases/latest/download"
fi
if ! grep -q 'releases?per_page=1' scripts/install.sh || ! grep -q 'releases?per_page=1' scripts/install.ps1; then
  fail "installers must fall back to prerelease discovery during alpha"
fi

if [ -n "${DAEMON8_LANDING_DIR:-}" ] || [ -d "$ROOT_DIR/../daemon8-landing" ]; then
  bash scripts/verify-landing-installers.sh
else
  printf 'release-check: landing installer sync skipped (set DAEMON8_LANDING_DIR or check out ../daemon8-landing)\n'
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
  .github \
  scripts \
  crates \
  | grep -v '^scripts/release-check.sh$' > "$stale_surface_list"

stale_pattern='setup_(apply|status|plan)|\.daemon8\.toml|librarian|discovery_daemon|debug_summary|debug_observe|provider hooks|old setup|deprecated alias|migration shim|:host/codex\+worker>|[Dd]eliber8|[Uu]plink8|[Bb]ookkeeper|[Rr]oomin8'
if xargs grep -n -E "$stale_pattern" < "$stale_surface_list"; then
  fail "stale alpha release-surface wording found"
fi

machine_path_patterns="$tmp_dir/machine-path-patterns"
user_home_component="Users"
printf '%s\n' "/${user_home_component}/" "C:\\${user_home_component}\\" "C:/${user_home_component}/" > "$machine_path_patterns"
if xargs grep -n -F -f "$machine_path_patterns" < "$stale_surface_list"; then
  fail "hardcoded user machine path found"
fi

current_changelog_hits="$(awk '/^## v0\.3\.0/ { exit } { print }' CHANGELOG.md | grep -n -E "$stale_pattern" || true)"
if [ -n "$current_changelog_hits" ]; then
  printf '%s\n' "$current_changelog_hits" >&2
  fail "stale current changelog wording found"
fi

printf 'release-check: ok (version %s)\n' "$workspace_version"
