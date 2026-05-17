#!/usr/bin/env bash
set -euo pipefail

REPO="daemon8ai/daemon8"
BINARY="daemon8"
VERSION="${DAEMON8_VERSION:-latest}"
INSTALL_DIR="${DAEMON8_INSTALL_DIR:-}"

GREEN="\033[0;32m"
BLUE="\033[0;34m"
RED="\033[0;31m"
DIM="\033[2m"
BOLD="\033[1m"
RESET="\033[0m"

step() { printf "\n${BLUE}[%s/%s]${RESET} %s\n" "$1" "$2" "$3"; }
ok()   { printf "  ${GREEN}+${RESET} %s\n" "$1"; }
dim()  { printf "  ${DIM}%s${RESET}\n" "$1"; }
err()  { printf "  ${RED}!${RESET} %s\n" "$1" >&2; }

TOTAL_STEPS=4

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os" in
    Darwin) os="apple-darwin" ;;
    Linux)  os="unknown-linux-gnu" ;;
    *)      err "Unsupported OS: $os"; exit 1 ;;
  esac
  case "$arch" in
    x86_64|amd64)  arch="x86_64" ;;
    arm64|aarch64) arch="aarch64" ;;
    *)             err "Unsupported architecture: $arch"; exit 1 ;;
  esac
  echo "${arch}-${os}"
}

resolve_install_dir() {
  if [ -n "$INSTALL_DIR" ]; then
    return
  fi
  if [ -d "$HOME/.cargo/bin" ]; then
    INSTALL_DIR="$HOME/.cargo/bin"
  elif [ -d "$HOME/.local/bin" ]; then
    INSTALL_DIR="$HOME/.local/bin"
  else
    INSTALL_DIR="$HOME/.local/bin"
  fi
}

compute_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

download_url() {
  local target="$1"
  if [ "$VERSION" = "latest" ]; then
    echo "https://github.com/${REPO}/releases/latest/download/${BINARY}-${target}.tar.gz"
  else
    echo "https://github.com/${REPO}/releases/download/${VERSION}/${BINARY}-${target}.tar.gz"
  fi
}

checksums_url() {
  if [ "$VERSION" = "latest" ]; then
    echo "https://github.com/${REPO}/releases/latest/download/checksums.sha256"
  else
    echo "https://github.com/${REPO}/releases/download/${VERSION}/checksums.sha256"
  fi
}

printf "\n${BOLD}Daemon8 Installer${RESET}\n"

TARGET="$(detect_target)"
resolve_install_dir

step 1 $TOTAL_STEPS "Download"

URL="$(download_url "$TARGET")"
ARCHIVE_NAME="${BINARY}-${TARGET}.tar.gz"
dim "Platform: $TARGET"
dim "Source:   $URL"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

ARCHIVE="$TMPDIR/$ARCHIVE_NAME"
if ! curl -fsSL "$URL" -o "$ARCHIVE" 2>/dev/null; then
  err "Download failed for $TARGET."
  err "No prebuilt binary may exist for this platform."
  err "Install from source instead: cargo install daemon8"
  if [ "$VERSION" != "latest" ]; then
    err "Version requested: $VERSION"
  fi
  exit 1
fi

ok "Downloaded $ARCHIVE_NAME"

step 2 $TOTAL_STEPS "Verify"

CHECKSUMS_URL="$(checksums_url)"
CHECKSUMS_FILE="$TMPDIR/checksums.sha256"
if curl -fsSL "$CHECKSUMS_URL" -o "$CHECKSUMS_FILE" 2>/dev/null; then
  EXPECTED="$(grep "$ARCHIVE_NAME" "$CHECKSUMS_FILE" | awk '{print $1}')"
  if [ -z "$EXPECTED" ]; then
    dim "No checksum entry for $ARCHIVE_NAME; skipping verification"
  else
    ACTUAL="$(compute_sha256 "$ARCHIVE")"
    if [ "$EXPECTED" = "$ACTUAL" ]; then
      ok "SHA-256 verified"
    else
      err "Checksum verification failed!"
      err "Expected: $EXPECTED"
      err "Got:      $ACTUAL"
      err "The downloaded file may be corrupted. Aborting."
      exit 1
    fi
  fi
else
  dim "Checksum file not available; skipping verification"
fi

tar xz -C "$TMPDIR" -f "$ARCHIVE"

if [ ! -f "$TMPDIR/$BINARY" ]; then
  err "Binary not found in archive"
  exit 1
fi

step 3 $TOTAL_STEPS "Install"

if [ -f "$INSTALL_DIR/$BINARY" ]; then
  dim "Updating existing installation"
fi

mkdir -p "$INSTALL_DIR"
mv "$TMPDIR/$BINARY" "$INSTALL_DIR/$BINARY"
chmod +x "$INSTALL_DIR/$BINARY"

if [ -f "$TMPDIR/LICENSE" ]; then
  cp "$TMPDIR/LICENSE" "$INSTALL_DIR/LICENSE-daemon8"
fi

if [ "$(uname -s)" = "Darwin" ]; then
  codesign --force --sign - "$INSTALL_DIR/$BINARY" 2>/dev/null || true
  dim "Ad-hoc codesigned (macOS)"
fi

ok "Installed to $INSTALL_DIR/$BINARY"

case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    SHELL_NAME="$(basename "${SHELL:-/bin/bash}")"
    case "$SHELL_NAME" in
      zsh)  RC_FILE="$HOME/.zshrc" ;;
      bash) RC_FILE="$HOME/.bashrc" ;;
      fish) RC_FILE="$HOME/.config/fish/config.fish" ;;
      *)    RC_FILE="" ;;
    esac
    if [ -n "$RC_FILE" ]; then
      if [ "$SHELL_NAME" = "fish" ]; then
        echo "set -gx PATH $INSTALL_DIR \$PATH" >> "$RC_FILE"
      else
        echo "export PATH=\"$INSTALL_DIR:\$PATH\"" >> "$RC_FILE"
      fi
      ok "Added $INSTALL_DIR to PATH in $RC_FILE"
      dim "Restart your shell or run: export PATH=\"$INSTALL_DIR:\$PATH\""
      export PATH="$INSTALL_DIR:$PATH"
    else
      dim "Add $INSTALL_DIR to your PATH manually"
      export PATH="$INSTALL_DIR:$PATH"
    fi
    ;;
esac

step 4 $TOTAL_STEPS "Next steps"
echo ""
dim "Start daemon8: daemon8 serve"
dim "Install the login service when wanted: daemon8 service install"
dim "Inside a project, initialize alpha config: daemon8 init"
dim ".daemon8/ should stay gitignored; daemon8 does not edit project .gitignore"
