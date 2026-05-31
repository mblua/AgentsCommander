#!/usr/bin/env bash
set -euo pipefail

missing=0

require_cmd() {
  local cmd="$1"
  if command -v "$cmd" >/dev/null 2>&1; then
    echo "ok: command $cmd"
  else
    echo "missing: command $cmd" >&2
    missing=1
  fi
}

require_pkg() {
  local pkg="$1"
  if pkg-config --exists "$pkg"; then
    echo "ok: pkg-config $pkg"
  else
    echo "missing: pkg-config $pkg" >&2
    missing=1
  fi
}

require_cmd cc
require_cmd pkg-config
require_cmd cargo
require_cmd rustc
require_cmd node
require_cmd npm

require_pkg openssl
require_pkg wayland-client
require_pkg glib-2.0
require_pkg gtk+-3.0
require_pkg webkit2gtk-4.1
require_pkg ayatana-appindicator3-0.1
require_pkg librsvg-2.0

if [[ "$missing" -ne 0 ]]; then
  echo "One or more Linux build dependencies are missing." >&2
  exit 1
fi

echo "All checked Linux build dependencies are available."
