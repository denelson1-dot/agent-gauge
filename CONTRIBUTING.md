# Contributing to Agent Gauge

Thanks for taking a look. Agent Gauge is a small, deliberately narrow
application, so the most useful contributions are usually bug reports with
enough detail to reproduce, and focused fixes.

## Before you start

Agent Gauge reads usage that providers already expose to software you have
signed into locally. It does not scrape provider websites, make model calls, or
transmit credentials anywhere but back to the provider that issued them. A
change that would break any of those properties will not be accepted, however
convenient it is.

## Reporting a bug

Open an issue with:

- your platform and version (Linux distribution and desktop environment, or
  Windows version), and the Agent Gauge version from **Settings → Diagnostics**;
- what you expected and what happened instead;
- whether the gauge showed a status such as `Stale`, `Expired`, or an error.

For anything about *where* the widget sits or *how* it is layered, the
application can tell you what the window system actually did rather than what it
was asked for:

```bash
agent-gauge --diagnose-window-layer
```

Include that output. It is far more useful than a description.

## Building from source

Source builds need the Rust toolchain, Node.js, and Tauri's platform
development libraries. On Debian/Ubuntu:

```bash
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
                 librsvg2-dev libxdo-dev libssl-dev patchelf
```

Then:

```bash
npm ci
npm run tauri dev
```

## The checks that must pass

CI runs these on both `ubuntu-22.04` and `windows-latest`, and a red leg is a
build break rather than a warning. Run them locally before opening a pull
request:

```bash
npm run check
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --all-targets --locked
```

If you are working on Linux and touching anything platform-specific, typecheck
the Windows build too — it needs no MSVC toolchain, because `cargo check` does
not link:

```bash
rustup target add x86_64-pc-windows-gnu
sudo apt install binutils-mingw-w64-x86-64
./scripts/check-windows.sh
```

## How the code is organised

- `src-tauri/src/render.rs` decides everything the widget displays —
  percentages, countdowns, statuses, ordering — once, for both platforms. If a
  reading looks wrong on screen, the decision was almost certainly made here.
- `src-tauri/src/platform/` holds anything platform-specific, organised by topic
  so both implementations of a contract sit in one file. That adjacency is
  deliberate: keep it.
- `src-tauri/src/providers/` holds the built-in readers; `src-tauri/src/adapters/`
  holds the custom-adapter trust, execution and limits.
- `src/` is the React settings window. The widget itself is not React on Linux —
  it is drawn natively with GTK/Cairo.

## Conventions

- Unknown values stay unknown. Never substitute a zero balance or reset time
  for a value a provider did not report.
- Prefer a test that runs on both platforms over two platform-specific ones.
  The shell-quoting rules, the `PATH`/`PATHEXT` search and the generated adapter
  scaffolds are all exercised on both runners for this reason.
- Commit messages are lowercase, imperative, and say why: `fix: hold the usage
  poll floor on failed attempts too`.

## Releasing

Pushing a `v*` tag runs `.github/workflows/release.yml`, which builds the `.deb`
and AppImage on Linux and the MSI and NSIS installers on Windows, and attaches
them to a draft release.
