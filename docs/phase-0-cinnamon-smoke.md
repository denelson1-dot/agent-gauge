# Phase 0 Cinnamon/X11 smoke gate

> Historical spike record. The gate was accepted and the product subsequently advanced through the personal-release build. For current packaged-app QA, use [qa.md](qa.md).

Date started: 2026-08-01

Target environment confirmed before the build:

- Linux Mint 22.3
- `XDG_CURRENT_DESKTOP=X-Cinnamon`
- `XDG_SESSION_TYPE=x11`
- `DISPLAY=:0`

## Current implementation decision

The first spike uses Tauri/TAO's narrow native APIs. On Linux these map to GTK keep-above/keep-below hints, taskbar and pager skip hints, workspace stick/unstick, and an empty input shape for click-through. No custom X11/EWMH fallback has been added before Cinnamon demonstrates that one is required.

The narrow Tauri/GTK implementation works on the target Cinnamon/X11 session; no custom X11/EWMH fallback was needed. The gate remains open only for the manual checks listed below and a normal build after installing the system development packages. The live run used development metadata extracted under `/tmp` because this session could not provide an interactive `sudo` password.

WebKitGTK 2.52.3 also hit a GBM/EGL startup abort on this multi-GPU NVIDIA machine. The live run used `WEBKIT_DMABUF_RENDERER_DISABLE_GBM=1`, as documented in the README. Keep that as an explicit target-machine workaround until it is rechecked after a WebKitGTK or driver update.

## Automated checks

| Check | Status | Notes |
| --- | --- | --- |
| React/TypeScript production build | Pass | `npm run check` |
| Geometry recovery unit tests | Pass | Four pure Rust cases: default placement, off-screen recovery, partial visibility, scale change |
| Rust formatting | Pass | `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check` |
| Rust lint | Pass | `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings` |
| Full Rust/Tauri tests | Pass | Four geometry tests plus library, binary, and doc-test targets |

The Rust checks above used temporary `/tmp` `pkg-config` metadata and linker symlinks over already-installed runtime libraries. That proves the source but does not replace installing the prerequisite `-dev` packages before normal development.

## Native smoke checklist

Start the app with `npm run tauri dev`, then record the result of each item. A visual impression is not enough for window-manager state; use `scripts/inspect-x11-window.sh` alongside the manual behavior checks.

- [x] The surface is transparent-capable and frameless (`xwininfo`: depth 32, border width 0).
- [x] The tray icon is present and its DBus menu remains usable while the widget is locked.
- [ ] Manually confirm the widget is absent from Alt-Tab. X11 already confirms skip-taskbar and skip-pager.
- [x] Desktop mode stays below ordinary application windows; root stacking and `_NET_WM_STATE_BELOW` agree.
- [x] Pinned mode stays above ordinary application windows; root stacking and `_NET_WM_STATE_ABOVE` agree.
- [x] Locked desktop and pinned modes pass pointer input through. Pinned mode delivered click events to an `xev` client; inside the desktop-mode widget bounds, X11 pointer lookup resolved directly to Cinnamon's Nemo desktop surface.
- [x] Locked mode does not accept keyboard focus (`WM_HINTS`: client accepts input false).
- [x] Tray unlock restores input (`WM_HINTS`: client accepts input true).
- [ ] Manually inspect editing chrome and exercise drag/resize from every edge/corner. Drag and southeast resize passed through direct X11 input.
- [x] Tray relock restores click-through.
- [ ] Switch workspaces and visually confirm local visibility. `_NET_WM_DESKTOP=0` already confirms the window is not sticky across the four-workspace session.
- [x] Mode, lock state, visibility, position, and size survive a normal tray quit/restart.
- [ ] Invoke Reset Geometry manually. The same recovery path passed both unit coverage and the forced off-screen restart below.
- [x] An intentionally off-screen saved position recovers fully onto the primary monitor.
- [ ] Perform a live scale/resolution change. Automated geometry tests cover scale-change recovery.
- [ ] Manually confirm readable opaque diagnostic content with compositor transparency unavailable.

## X11 evidence

With the app running:

```bash
scripts/inspect-x11-window.sh
```

Observed locked desktop state included `_NET_WM_STATE_BELOW`, `_NET_WM_STATE_SKIP_TASKBAR`, and `_NET_WM_STATE_SKIP_PAGER`. Pinned mode replaced `BELOW` with `_NET_WM_STATE_ABOVE`. `_NET_WM_DESKTOP` was `0`, not the all-workspaces sentinel `0xFFFFFFFF`.

The desktop stacking probe placed Agent Gauge before a normal `xev` client in the root window's bottom-to-top list. After the tray switched to pinned mode, Agent Gauge moved after that client. A locked pinned click delivered both `ButtonPress` and `ButtonRelease` to the underlying `xev` client. In locked desktop mode, X11 pointer lookup at `3400,100`—inside the widget's `2876,24 540x300` bounds—returned Nemo's desktop window (`0x02e00003`) rather than Agent Gauge.

Geometry persistence was exercised at `2796,124` with a `540x300` size on `DP-0`. A forced `99999,99999` position with a nonexistent saved monitor recovered to `2876,24` on `DP-0` while retaining a usable `540x300` size.

## Result and fallback decision

Cinnamon honors Tauri/TAO's native keep-below, keep-above, input-region, workspace, and taskbar/pager hints on this X11 session. No custom X11/EWMH fallback is justified.

The standard Tauri/GTK window policies were retained. During the later product build, the transparent widget surface moved to native GTK/Cairo rendering to avoid the target NVIDIA/WebKitGTK transparency failure; the opaque React settings surface remains in WebKitGTK. No custom X11/EWMH fallback was required.
