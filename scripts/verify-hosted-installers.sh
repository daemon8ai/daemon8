#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

fail() {
  printf 'verify-hosted-installers: %s\n' "$1" >&2
  exit 1
}

command -v curl >/dev/null 2>&1 || fail "curl is required"

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

hosted_install_sh="$tmp_dir/install.sh"
hosted_install_ps1="$tmp_dir/install.ps1"

curl -fsSL https://daemon8.ai/install.sh -o "$hosted_install_sh" || fail "public shell installer must be readable"
curl -fsSL https://daemon8.ai/install.ps1 -o "$hosted_install_ps1" || fail "public PowerShell installer must be readable"

cmp -s scripts/install.sh "$hosted_install_sh" || fail "hosted install.sh does not match scripts/install.sh"
cmp -s scripts/install.ps1 "$hosted_install_ps1" || fail "hosted install.ps1 does not match scripts/install.ps1"

if grep -R -n 'releases/latest/download' "$hosted_install_sh" "$hosted_install_ps1"; then
  fail "hosted installers must use explicit resolved release tags, not releases/latest/download"
fi

if ! grep -q 'releases?per_page=1' "$hosted_install_sh" || ! grep -q 'releases?per_page=1' "$hosted_install_ps1"; then
  fail "hosted installers must include prerelease discovery during alpha"
fi

hardcoded_alpha_tag='v[0-9]+\.[0-9]+\.[0-9]+-alpha\.[0-9]+'
if grep -R -n -E "$hardcoded_alpha_tag" "$hosted_install_sh" "$hosted_install_ps1"; then
  fail "hosted installer fallback examples must not hardcode old alpha tags"
fi

printf 'verify-hosted-installers: ok\n'
