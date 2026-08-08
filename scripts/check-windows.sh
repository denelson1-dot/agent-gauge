#!/usr/bin/env bash
# Typecheck the Windows build from a Linux checkout.
#
# Agent Gauge ships on Linux and Windows, but day-to-day development happens on
# Linux. This gives Windows-side changes a normal edit-compile loop instead of
# waiting on CI.
#
# It works because `gtk` is declared only under
# `[target.'cfg(target_os = "linux")'.dependencies]`, so a Windows-target check
# never touches the GTK packages, and `cargo check` does not link, so no MSVC
# toolchain is required.
#
# Prerequisites:
#   rustup target add x86_64-pc-windows-gnu
#   sudo apt install binutils-mingw-w64-x86-64
#
# Optionally `gcc-mingw-w64-x86-64`, which `windres` uses to preprocess the
# resource script. It is a large install for one preprocessing step, so if it is
# absent this script substitutes the host compiler — that is sound here because
# a .rc file only needs #include/#define expansion, and nothing this script
# produces is ever shipped.
#
# This is a development aid, not a release path. The authority on the Windows
# build is the CI matrix, which compiles for MSVC on a Windows runner.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v x86_64-w64-mingw32-windres >/dev/null 2>&1; then
    echo "error: x86_64-w64-mingw32-windres not found." >&2
    echo "       sudo apt install binutils-mingw-w64-x86-64" >&2
    exit 1
fi

if ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
    shim_dir="$(mktemp -d)"
    trap 'rm -rf "$shim_dir"' EXIT
    cat >"$shim_dir/x86_64-w64-mingw32-gcc" <<'SHIM'
#!/bin/sh
exec gcc "$@"
SHIM
    chmod +x "$shim_dir/x86_64-w64-mingw32-gcc"
    PATH="$shim_dir:$PATH"
    export PATH
fi

exec cargo check \
    --manifest-path "$repo_root/src-tauri/Cargo.toml" \
    --target x86_64-pc-windows-gnu \
    --all-targets \
    "$@"
