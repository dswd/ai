#!/bin/sh
set -eu

REPO="dswd/ai"
BASE_URL="https://github.com/${REPO}/releases/latest/download"
LOCAL_DIR="${HOME}/.bin"

bold()  { printf '\033[1m%s\033[0m\n' "$1"; }
err()   { printf '\033[31m%s\033[0m\n' "$1" >&2; exit 1; }
ok()    { printf '\033[32m%s\033[0m\n' "$1"; }

usage() {
  echo "Usage: curl -fsSL https://dswd.github.io/ai/install.sh | sh"
  echo "       curl -fsSL https://dswd.github.io/ai/install.sh | sudo sh -s -- --global"
  exit 1
}

GLOBAL=false
case "${1:-}" in
  --global) GLOBAL=true; shift ;;
  --help|-h) usage ;;
esac

OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}" in
  Linux)  os="linux" ;;
  Darwin) os="macos" ;;
  *)
    err "$(bold 'Unsupported OS:' ${OS})
Windows users: download the binary from
  ${BASE_URL}/ai-windows-amd64.exe"
    ;;
esac

case "${ARCH}" in
  x86_64|amd64)     arch="amd64" ;;
  aarch64|arm64)    arch="arm64" ;;
  *)                err "$(bold 'Unsupported architecture:' ${ARCH})" ;;
esac

BINARY="ai-${os}-${arch}"
URL="${BASE_URL}/${BINARY}"

if ${GLOBAL}; then
  DEST="/usr/local/bin/ai"
  bold "Installing to /usr/local/bin (global)..."
  if [ ! -w /usr/local/bin ]; then
    err "$(bold 'Permission denied.') Run with sudo:
  curl -fsSL https://dswd.github.io/ai/install.sh | sudo sh -s -- --global"
  fi
else
  DEST="${LOCAL_DIR}/ai"
  bold "Installing to ${LOCAL_DIR}..."
  mkdir -p "${LOCAL_DIR}"
fi

if command -v curl >/dev/null 2>&1; then
  curl -fsSL --progress-bar "${URL}" -o "${DEST}"
elif command -v wget >/dev/null 2>&1; then
  wget -q --show-progress "${URL}" -O "${DEST}"
else
  err "$(bold 'curl or wget required to download the binary.')"
fi

chmod +x "${DEST}"

if ! ${GLOBAL}; then
  RC=""
  for f in "${HOME}/.bashrc" "${HOME}/.zshrc" "${HOME}/.profile"; do
    [ -f "${f}" ] && RC="${f}" && break
  done
  if [ -n "${RC}" ]; then
    if ! grep -qF 'export PATH="$HOME/.bin:$PATH"' "${RC}" 2>/dev/null; then
      echo 'export PATH="$HOME/.bin:$PATH"' >> "${RC}"
    fi
  fi
  case "$(basename "${SHELL:-sh}")" in
    bash) export PATH="$HOME/.bin:$PATH" ;;
    zsh)  export PATH="$HOME/.bin:$PATH" ;;
  esac
  bold ""
  bold "Done! Start a new terminal or run:"
  bold "  export PATH=\"\$HOME/.bin:\$PATH\""
fi

ok "ai installed to ${DEST}"
ok "Run: ai --init"
