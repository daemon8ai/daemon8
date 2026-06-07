#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

fail() {
  printf 'installer-artifact-smoke: %s\n' "$1" >&2
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
command -v python3 >/dev/null 2>&1 || fail "python3 is required to serve local artifacts"

os_name="$(uname -s)"
arch_name="$(uname -m)"

case "$os_name" in
  Darwin) os_name="apple-darwin" ;;
  Linux) os_name="unknown-linux-gnu" ;;
  *) fail "unsupported OS: $os_name" ;;
esac

case "$arch_name" in
  x86_64|amd64) arch_name="x86_64" ;;
  arm64|aarch64) arch_name="aarch64" ;;
  *) fail "unsupported architecture: $arch_name" ;;
esac

target="$arch_name-$os_name"

tmp_dir="$(mktemp -d)"
server_pid=""
trap 'if [ -n "$server_pid" ]; then kill "$server_pid" 2>/dev/null || true; wait "$server_pid" 2>/dev/null || true; fi; rm -rf "$tmp_dir"' EXIT

artifacts="$tmp_dir/artifacts"
package_dir="$tmp_dir/package"
install_dir="$tmp_dir/install"
mkdir -p "$artifacts" "$package_dir" "$install_dir"

cargo build --release --target "$target" -p daemon8

cp "target/$target/release/daemon8" "$package_dir/"
cp LICENSE "$package_dir/"
tar -C "$package_dir" -czf "$artifacts/daemon8-$target.tar.gz" daemon8 LICENSE

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$artifacts" && sha256sum "daemon8-$target.tar.gz" > checksums.sha256)
else
  (cd "$artifacts" && shasum -a 256 "daemon8-$target.tar.gz" | awk '{print $1 "  " $2}' > checksums.sha256)
fi

port="$(python3 - <<'PY'
import socket
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"

python3 -m http.server "$port" --bind 127.0.0.1 --directory "$artifacts" >/dev/null 2>&1 &
server_pid="$!"

for _ in $(seq 1 50); do
  if curl -fsS "http://127.0.0.1:$port/checksums.sha256" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

curl -fsS "http://127.0.0.1:$port/checksums.sha256" >/dev/null || fail "local artifact server did not start"

DAEMON8_RELEASE_BASE_URL="http://127.0.0.1:$port" \
DAEMON8_INSTALLER_SKIP_SERVICE=1 \
DAEMON8_INSTALL_DIR="$install_dir" \
  bash scripts/install.sh >/dev/null

installed_version="$("$install_dir/daemon8" --version)"
[ "$installed_version" = "daemon8 $workspace_version" ] || fail "unexpected installed version: $installed_version"

printf 'installer-artifact-smoke: ok (%s %s)\n' "$workspace_version" "$target"
