# Changelog

All notable changes to Agent Gauge are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.0] — 2026-09-02

### Added

- Claude usage now falls back to the desktop app's own record of plan usage
  (`plan-usage-history.json`), which needs no terminal session, no request and
  no credentials of Agent Gauge's own. This covers the case the endpoint could
  not: Claude Code is what refreshes the sign-in token, so for anyone working
  mainly in the desktop app it expires and stays expired.

### Fixed

- A reading the usage endpoint replayed from its cache is no longer shown in
  preference to a fresher one from the desktop app. When the endpoint could not
  be reached its last reading was replayed indefinitely, so an expired sign-in
  pinned the widget to whatever it saw last — which is how a five-hour window
  with no activity in it came to read as a confident `0%` hours after the fact.
  Sources are now ranked by how current the reading is.
- A window whose reset time is missing from the reading being shown now borrows
  the one the endpoint last reported, provided it has not yet passed. A reset
  timestamp does not stop being true because it was learned earlier, so the
  weekly countdown survives a fallback instead of reading *Reset unavailable*.
- The Linux widget now draws the reason a reading has stopped moving. The
  message was produced for both renderers but only the Windows one drew it; on
  Linux it merely tinted the status label red, so the card said *Stale* without
  ever saying why, and the sentence explaining what to do was reachable only
  through Settings → Trackers.

## [0.4.0] — 2026-09-01

### Added

- The Claude usage-endpoint poll interval is now a setting: *Backup usage
  check* under Settings → Trackers, with options from one to twenty minutes.
  It only governs the endpoint read that stands in while no terminal session
  is feeding the status-line capture.

### Changed

- The default poll interval is ten minutes rather than fifteen — six requests
  an hour at most. Reading usage still does not consume it.
- A poll interval shorter than the widget's refresh interval is now honoured.
  Previously the endpoint was only consulted on a widget refresh, so a shorter
  floor could never be reached.

## [0.3.1] — 2026-08-30

### Fixed

- The Claude usage-endpoint poll now holds its 15-minute floor after a failed
  attempt, not only after a successful one. A rate-limited or rejected request
  is no longer retried faster than a working one, so an error cannot turn into
  a burst of requests.

## [0.3.0] — 2026-08-30

### Added

- Claude usage is read from Claude's own account usage endpoint when no
  terminal session has fed the status-line capture for more than two minutes,
  authenticating with the sign-in Claude Code already holds. This fills the gap
  left by the Claude Code desktop app, which never renders a status line and so
  never runs the capture command.

  The endpoint is asked at most once every 15 minutes. Credentials are read and
  never written; refreshing an expired sign-in remains Claude Code's job, so
  Agent Gauge cannot invalidate the token your editor is holding. Reading usage
  does not consume it — this is a metering endpoint, not an inference call.

### Fixed

- The widget's saved position now survives a restart.

## [0.2.0] — 2026-08-08

### Added

- Windows support, at parity with Linux: WebView2 drawing, a desktop layer via
  Explorer's `WorkerW` with a bottom-of-z-order fallback, a per-user `Run`
  registry entry for start-at-login, and MSI and NSIS installers.
- Claude status-line capture without Python or `/bin/sh`. Installations created
  by earlier versions are migrated automatically on first launch and the old
  dispatcher script is removed.
- Adapter manifests may name an `interpreter`, so a script can declare what runs
  it on Windows, where there is no `#!` line or execute bit. Trust still covers
  the manifest and the executable together.
- CI builds, lints, and tests on both `ubuntu-22.04` and `windows-latest`, plus
  `scripts/check-windows.sh` for typechecking the Windows build from a Linux
  checkout.

### Changed

- Everything the widget displays is now derived once in `src-tauri/src/render.rs`
  and handed to whichever painter the platform uses, so the two platforms cannot
  drift apart.
- Platform-specific code is gathered under `src-tauri/src/platform/`, organised
  by topic so both implementations of a contract sit in one file.

## [0.1.4]

### Added

- First release: a transparent, frameless desktop widget with Desktop and Pinned
  layers, a locked click-through mode with tray-controlled unlock, drag and
  resize, the Glass, Cutout and Signal themes, read-only Codex usage via
  `codex app-server`, reversible Claude Code status-line capture, provider
  ordering and enable/disable controls, saved geometry, start-at-login, and
  versioned custom-adapter manifests with hash-bound trust.

[0.5.0]: https://github.com/denelson1-dot/agent-gauge/releases/tag/v0.5.0
[0.4.0]: https://github.com/denelson1-dot/agent-gauge/releases/tag/v0.4.0
[0.3.1]: https://github.com/denelson1-dot/agent-gauge/releases/tag/v0.3.1
[0.3.0]: https://github.com/denelson1-dot/agent-gauge/commit/70b4c31
[0.2.0]: https://github.com/denelson1-dot/agent-gauge/commit/9c6e2f3
[0.1.4]: https://github.com/denelson1-dot/agent-gauge/commit/3e3f719
