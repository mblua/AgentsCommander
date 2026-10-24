#!/usr/bin/env bash
# Installs the Ubuntu packages the Linux Rust jobs build against, keeping every
# downloaded .deb in APT_DEB_CACHE (default ~/apt-debs) so CI can cache it: apt
# then installs from that directory and downloads only what changed.
# Shared by .github/workflows/pr-regression-gates.yml (clippy-linux,
# rust-regression-linux, rust-release-parity-linux) and cache-warm.yml
# (warm-apt-linux), so the package list lives here once.
#
# Why: the runners' Ubuntu mirror is sometimes ~400x slower than usual, and a
# 1-minute install then takes 13-46 minutes (#3190).
set -euo pipefail

cache="${APT_DEB_CACHE:-$HOME/apt-debs}"
mkdir -p "$cache/partial"

sudo apt-get update
# The four original packages are Tauri's own. The rest are the #1842
# xcap native chain: bindgen needs libclang; system-deps needs the
# pkg-config metadata for libpipewire-0.3 AND libspa-0.2 (E1 proved
# libspa-0.2-dev is a SEPARATE package that only arrives
# transitively, so listing it is what stops a future dependency
# change breaking the build silently); libwayshot-xcap needs
# gbm/drm/EGL; xcb needs libxcb + randr.
sudo apt-get install -y \
  -o Dir::Cache::Archives="$cache" \
  -o APT::Keep-Downloaded-Packages=true \
  libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf \
  build-essential pkg-config clang libclang-dev \
  libpipewire-0.3-dev libspa-0.2-dev \
  libxcb1-dev libxcb-randr0-dev libxcb-shm0-dev \
  libgbm-dev libdrm-dev libegl1-mesa-dev libdbus-1-dev

# Drop .deb files the mirror no longer offers, so the cache does not grow forever.
sudo apt-get -o Dir::Cache::Archives="$cache" autoclean
# apt leaves a root-owned lock file behind; give the directory back to the
# runner user so actions/cache can read and save it.
sudo rm -f "$cache/lock"
sudo chown -R "$(id -u):$(id -g)" "$cache"
echo "apt cache: $(find "$cache" -maxdepth 1 -name '*.deb' | wc -l) .deb files in $cache"
