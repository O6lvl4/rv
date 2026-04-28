#!/usr/bin/env bash
# rv installer.
set -euo pipefail
REPO="O6lvl4/rv"
INSTALL_DIR="${RV_INSTALL_DIR:-$HOME/.local/bin}"
PIN="${RV_VERSION:-}"

err() { printf 'rv-install: %s\n' "$*" >&2; exit 1; }
say() { printf 'rv-install: %s\n' "$*"; }

detect_target() {
  local os arch
  os="$(uname -s)"; arch="$(uname -m)"
  case "$os" in
    Darwin) case "$arch" in arm64|aarch64) echo "aarch64-apple-darwin";; x86_64) echo "x86_64-apple-darwin";; *) err "unsupported macOS arch: $arch";; esac ;;
    Linux)  case "$arch" in aarch64|arm64) echo "aarch64-unknown-linux-musl";; x86_64|amd64) echo "x86_64-unknown-linux-musl";; *) err "unsupported Linux arch: $arch";; esac ;;
    *) err "unsupported OS: $os" ;;
  esac
}

resolve_tag() {
  if [ -n "$PIN" ]; then echo "$PIN"; return; fi
  curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1
}

main() {
  command -v curl >/dev/null 2>&1 || err "curl is required"
  command -v tar  >/dev/null 2>&1 || err "tar is required"
  local target tag asset url tmpdir
  target="$(detect_target)"
  tag="$(resolve_tag)"
  [ -n "$tag" ] || err "could not resolve a release tag (set RV_VERSION=vX.Y.Z to pin)"

  asset="rv-${tag}-${target}.tar.gz"
  url="https://github.com/${REPO}/releases/download/${tag}/${asset}"

  tmpdir="$(mktemp -d)"
  trap 'rm -rf "${tmpdir:-}"' EXIT

  say "downloading $asset"
  curl -fsSL "$url" -o "${tmpdir}/${asset}"

  say "verifying sha256"
  if curl -fsSL "${url}.sha256" -o "${tmpdir}/${asset}.sha256" 2>/dev/null; then
    if command -v shasum >/dev/null 2>&1; then
      ( cd "$tmpdir" && shasum -a 256 -c "${asset}.sha256" )
    elif command -v sha256sum >/dev/null 2>&1; then
      ( cd "$tmpdir" && sha256sum -c "${asset}.sha256" )
    fi
  fi

  say "extracting"
  tar -xzf "${tmpdir}/${asset}" -C "${tmpdir}"
  local stage="${tmpdir}/rv-${tag}-${target}"

  mkdir -p "$INSTALL_DIR"
  install -m 0755 "${stage}/rv" "${INSTALL_DIR}/rv"
  ln -sfn rv "${INSTALL_DIR}/rvx"

  say "installed to ${INSTALL_DIR}"
  say "  rv = $(${INSTALL_DIR}/rv --version 2>/dev/null || echo 'not on PATH yet')"

  case ":$PATH:" in
    *":${INSTALL_DIR}:"*) ;;
    *) say "${INSTALL_DIR} is not on PATH. Add: export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
  esac

  say "done. Try: rv install 3.3.5"
  say "    (rv shells out to ruby-build; install with 'brew install ruby-build' first)"
}

main "$@"
