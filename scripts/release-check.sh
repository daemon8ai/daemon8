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
command -v curl >/dev/null 2>&1 || fail "curl is required for release URL checks"

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

curl -fsSIL https://daemon8.ai/install.sh >/dev/null || fail "public shell installer URL must be reachable"
curl -fsSIL https://daemon8.ai/install.ps1 >/dev/null || fail "public PowerShell installer URL must be reachable"

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

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

DAEMON8_INSTALLER_SELF_TEST=1 bash scripts/install.sh >/dev/null
DAEMON8_INSTALLER_SELF_TEST=1 bash install.sh >/dev/null
mkdir -p "$tmp_dir/delegate-shell/scripts"
cp install.sh "$tmp_dir/delegate-shell/install.sh"
cat > "$tmp_dir/delegate-shell/scripts/install.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf delegated > delegated.out
EOF
chmod +x "$tmp_dir/delegate-shell/scripts/install.sh"
(cd "$tmp_dir/delegate-shell" && DAEMON8_INSTALLER_SELF_TEST=1 bash install.sh >/dev/null)
test "$(cat "$tmp_dir/delegate-shell/delegated.out")" = "delegated" || fail "root shell installer must delegate to local scripts/install.sh"

if grep -R "cargo install daemon8" install.sh install.ps1 scripts/install.sh scripts/install.ps1; then
  fail "installer fallback must not imply crates.io publish"
fi

cp install.sh "$tmp_dir/install.sh"
if ! (cd "$tmp_dir" && DAEMON8_INSTALLER_SELF_TEST=1 bash install.sh >/dev/null); then
  fail "root shell installer must work outside a checkout"
fi
if command -v pwsh >/dev/null 2>&1; then
  pwsh -NoLogo -NoProfile -Command "\$ErrorActionPreference='Stop'; \$env:DAEMON8_INSTALLER_SELF_TEST='1'; ./scripts/install.ps1 | Out-Null; ./install.ps1 | Out-Null"
  mkdir -p "$tmp_dir/delegate-pwsh/scripts"
  cp install.ps1 "$tmp_dir/delegate-pwsh/install.ps1"
  cat > "$tmp_dir/delegate-pwsh/scripts/install.ps1" <<'EOF'
Set-Content -Path delegated.out -Value delegated -NoNewline
EOF
  (cd "$tmp_dir/delegate-pwsh" && pwsh -NoLogo -NoProfile -Command "\$ErrorActionPreference='Continue'; /bin/sh -c 'exit 7'; \$ErrorActionPreference='Stop'; \$env:DAEMON8_INSTALLER_SELF_TEST='1'; ./install.ps1 | Out-Null")
  test "$(cat "$tmp_dir/delegate-pwsh/delegated.out")" = "delegated" || fail "root PowerShell installer must delegate to local scripts/install.ps1"
  cp install.ps1 "$tmp_dir/install.ps1"
  if ! (cd "$tmp_dir" && pwsh -NoLogo -NoProfile -Command "\$ErrorActionPreference='Stop'; \$env:DAEMON8_INSTALLER_SELF_TEST='1'; ./install.ps1 | Out-Null"); then
    fail "root PowerShell installer must work outside a checkout"
  fi
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
  fail "release workflow must not create a GitHub release when server upload is skipped"
fi
upload_line="$(grep -n 'name: Upload to server' .github/workflows/release.yml | cut -d: -f1)"
release_lines="$(grep -n 'softprops/action-gh-release' .github/workflows/release.yml | cut -d: -f1)"
release_line_count="$(printf '%s\n' "$release_lines" | sed '/^$/d' | wc -l | tr -d ' ')"
release_line="$(printf '%s\n' "$release_lines" | sed '/^$/d' | head -n 1)"
if [ -z "$upload_line" ] || [ "$release_line_count" != "1" ] || [ "$upload_line" -ge "$release_line" ]; then
  fail "release workflow must upload to server before exactly one GitHub release step"
fi
next_step_line="$(awk -v start="$upload_line" 'NR > start && /^[[:space:]]+- name:/ { print NR; exit }' .github/workflows/release.yml)"
if [ -n "$next_step_line" ]; then
  upload_end_line=$((next_step_line - 1))
else
  upload_end_line="$(wc -l < .github/workflows/release.yml | tr -d ' ')"
fi
if sed -n "${upload_line},${upload_end_line}p" .github/workflows/release.yml | grep -n '^[[:space:]]*if:'; then
  fail "server upload step must not be conditional"
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

stale_pattern='setup_(apply|status|plan)|\.daemon8\.toml|librarian|discovery_daemon|debug_summary|debug_observe|provider hooks|old setup|deprecated alias|migration shim|:host/codex\+worker>'
if xargs grep -n -E "$stale_pattern" < "$stale_surface_list"; then
  fail "stale alpha release-surface wording found"
fi

current_changelog_hits="$(awk '/^## v0\.3\.0/ { exit } { print }' CHANGELOG.md | grep -n -E "$stale_pattern" || true)"
if [ -n "$current_changelog_hits" ]; then
  printf '%s\n' "$current_changelog_hits" >&2
  fail "stale current changelog wording found"
fi

printf 'release-check: ok (version %s)\n' "$workspace_version"
