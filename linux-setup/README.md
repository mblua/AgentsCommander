# Linux Setup

This folder contains the Ubuntu dependencies needed to build and test
AgentsCommander locally on Linux.

## Ubuntu

Run:

```bash
./linux-setup/install-ubuntu-deps.sh
```

Then verify:

```bash
./linux-setup/verify-ubuntu-deps.sh
```

## What This Installs

- C/C++ build toolchain for Rust native crates.
- `pkg-config`, used by Rust `*-sys` crates to discover system libraries.
- OpenSSL development headers.
- Wayland, GLib, GTK, WebKitGTK, AppIndicator, and SVG development packages
  used by Tauri/Wry on Linux.

## Notes

Ubuntu package names can vary across releases. This script targets modern
Ubuntu releases where Tauri uses WebKitGTK 4.1. If `libwebkit2gtk-4.1-dev` is
not available on a given machine, try the distro-provided WebKitGTK development
package for that Ubuntu version.
