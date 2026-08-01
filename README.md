# Agent Gauge

Agent Gauge is a small, local Linux desktop widget for seeing AI-agent subscription usage at a glance. It is built for Linux Mint 22.3, Cinnamon, and X11.

It reads the local Codex CLI and a reversible Claude Code status-line feed. It does not scrape provider websites, make model calls, copy provider credentials, send telemetry, or run a hosted service.

## Install

Open this package with Software Manager and choose **Install**:

`src-tauri/target/release/bundle/deb/Agent Gauge_0.1.4_amd64.deb`

That is the complete application package. You do not need Rust, Node.js, a compiler, or any `-dev` packages to use it. Mint's package installer will resolve the normal GTK/WebKit runtime libraries if they are not already present.

After installation, launch **Agent Gauge** from the Cinnamon application menu. The first run opens Settings; the widget itself is controlled from the Agent Gauge tray icon.

## What ships

- A transparent, frameless widget with Desktop and Pinned layers.
- A locked, fully click-through mode plus tray-controlled unlock, drag, and resize.
- Glass, Cutout, and Signal themes.
- Read-only Codex usage via `codex app-server`.
- Automatic, reversible Claude Code status-line capture on first launch.
- Provider ordering, enable/disable controls, manual refresh, saved geometry, and start-at-login.
- Versioned custom-adapter manifests and snapshots with hash-bound trust, timeouts, and output limits.
- A clearly disabled example adapter documenting the supported contract, plus creation of up to five named local adapter starters.

Unknown values stay unknown. In particular, Agent Gauge never substitutes a zero balance or reset time when a provider did not report one.

## Quick QA

The end-user test pass is in [docs/qa.md](docs/qa.md). It only assumes that the `.deb` has been installed; it does not ask you to set up a development environment.

## Claude capture and removal

On first launch, Agent Gauge installs a small dispatcher as Claude Code's status-line command. If another command is already configured, Agent Gauge preserves and chains it when its shape is safe to do so. **Disconnect** restores the prior value and prevents automatic reconnection; **Connect Claude** can enable it again. If that setting changes after connection, Agent Gauge reports a conflict and leaves the live setting untouched.

Disconnect Claude before uninstalling if capture is enabled. Then remove Agent Gauge normally through Software Manager. Per-user settings are under `~/.config/agent-gauge`, cache under `~/.cache/agent-gauge`, and window state under `~/.local/state/agent-gauge`; those folders can be removed afterward if you also want to reset all preferences and cached readings.

## Custom adapters

Adapter manifests live under `~/.config/agent-gauge/adapters`. Settings can create up to five disabled starter folders for additional providers. These are local executable integrations that must be implemented against a provider's read-only interface; adding one does not create a provider account or contact a service. See the bundled JSON Schemas in `schemas/adapter-manifest-v1.schema.json` and `schemas/adapter-snapshot-v1.schema.json`. Agent Gauge never executes a new or changed adapter until you explicitly trust its current manifest and executable hashes.

## Development

The commands below are for contributors building from source, not for installing the application:

```bash
npm ci
npm run check
cargo test --manifest-path src-tauri/Cargo.toml --all-targets --locked
npm run tauri dev
npm run tauri build -- --bundles deb
```

Source builds require the Rust toolchain, Node.js, and Tauri's Debian/Ubuntu development libraries. The packaged application does not.

The historical Cinnamon/X11 feasibility evidence is retained in [docs/phase-0-cinnamon-smoke.md](docs/phase-0-cinnamon-smoke.md).

## License

MIT
