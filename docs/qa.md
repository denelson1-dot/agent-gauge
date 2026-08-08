# Agent Gauge end-user QA

This checklist tests the packaged application. It does not require Rust, Node.js, compilers, or development libraries.

Sections 1–6 apply to both platforms. Section 7 is Windows-only and covers the behaviour that has no Linux equivalent.

## 1. Install and first launch

1. Install the package for your platform:
   - **Linux** — double-click `src-tauri/target/release/bundle/deb/Agent Gauge_0.2.0_amd64.deb` and install it with Software Manager.
   - **Windows** — run the `.msi` or the NSIS `-setup.exe`. Expect a SmartScreen warning, because the installers are unsigned; choose **More info → Run anyway**.
2. Launch **Agent Gauge** from the application menu or Start menu.
3. Confirm that Settings opens on first launch, the tray icon appears, and the widget appears without a terminal or console window.
4. Choose whether it should start at login, then select **Continue**.

Pass condition: Settings, tray, and widget all start normally. No SDK or build tool is requested.

## 2. Widget behavior

Use the tray menu or Settings to exercise these controls:

- Switch between **Desktop** and **Pinned**. Desktop should sit behind normal windows; Pinned should sit above them. On Windows, see section 7 for what Desktop mode is expected to do.
- Choose **Unlock layout**, then drag and resize the widget. Lock it again and confirm clicks pass through to the desktop or window beneath it.
- Resize down to the enforced minimum and confirm every provider-reported ring, bar, reset time, and known balance remains visible; missing metrics should collapse without leaving reserved blank slots.
- Hide and show the widget. The tray icon must remain available while it is hidden.
- Before hiding, note the exact widget position and size. Show it again and confirm both are unchanged.
- Select each theme: Glass, Cutout, and Signal.
- Quit from the tray, reopen Agent Gauge, and confirm the chosen theme, layer, position, size, visibility, and lock state return.

If you have both platforms available, compare the widgets side by side. The wording, percentages, and countdowns should be identical for the same readings — they are produced by the same code.

## 3. Trackers

- Codex should connect automatically when the local Codex CLI is installed and signed in. Compare the shown percent and reset time with the CLI's own usage display. If Codex reports an explicit zero credit balance, Agent Gauge should show zero; if it reports no balance, the row should be absent.
- Select **Refresh now** and confirm the tracker briefly enters Refreshing and returns to a stable state.
- Disable and re-enable a tracker, then reorder the trackers. Confirm the widget follows the changes.
- If a CLI is unavailable or signed out, confirm Agent Gauge reports that state instead of inventing usage data.

## 4. Claude check

1. Confirm Settings reports Claude capture as connected after the first launch; no connection button should be necessary.
2. Use Claude Code normally until it emits status-line rate-limit information.
3. Refresh Agent Gauge and confirm the five-hour and/or weekly readings appear.
4. Select **Disconnect** and confirm Settings reports that capture is no longer connected and stays disconnected after restarting Agent Gauge.
5. Select **Connect Claude** to enable it again if desired.
6. Leave Claude Code idle until the five-hour window passes the reset time it reported. Within about half a minute of that time the ring should read `0% used` with `Window reset` and `Awaiting new activity`, rather than holding the last captured percent. Resume normal Claude Code use and confirm a real percent and a fresh reset time return.

Automatic first-run connection deliberately edits Claude Code's `settings.json` (`~/.claude/settings.json`, or `%USERPROFILE%\.claude\settings.json` on Windows). Agent Gauge records the previous status-line value and restores it on disconnect. If you manually change that setting after connection, disconnect should report a conflict and leave your newer value alone.

**Upgrading from a version before 0.2:** the old Python dispatcher is replaced automatically on first launch. Confirm the `statusLine` command in Claude's settings now points at the Agent Gauge executable with `--capture-claude`, and that `claude-status-dispatcher.py` is gone from the configuration directory. If you had your own status line chained behind Agent Gauge, confirm its output still appears.

## 5. Start at login

Toggle **Start at login** on, close and reopen Settings, and confirm it remains on. Toggle it off and confirm it remains off. A full login/reboot check is the final real-desktop validation.

On Windows the setting is a value named `Agent Gauge` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`; it should appear and disappear with the toggle, and should be absent after uninstalling.

## 6. Optional adapter check

Confirm the example adapter is explained separately, does not appear as an enabled tracker, and does not count toward the five additional-tracker slots. Select **Add tracker**, create a named starter, and confirm it remains disabled and untrusted. Its folder appears under the adapters path shown in Diagnostics. Up to five starters may exist. After implementing and reviewing one, **Trust & enable** binds trust to its manifest and executable hashes; modifying either file must disable execution until it is trusted again.

The generated starter is `read-usage.py` on Linux and `read-usage.ps1` on Windows. Trust and enable it and confirm it reports **Waiting** rather than failing to start — that is the check that the generated script actually runs on the platform that generated it.

## 7. Windows-only checks

These cover behaviour Windows implements differently, and are the priority if time on a Windows machine is limited. Work top to bottom.

1. **Files land in the right place.** Diagnostics should show configuration under `%APPDATA%\agent-gauge` and cache under `%LOCALAPPDATA%\agent-gauge\cache`. Nothing belonging to Agent Gauge should appear next to the executable or in the folder it was launched from.

2. **Single instance.** Launch Agent Gauge a second time while it is running. The second launch should exit without opening a window, and the first should keep running. Then end the process from Task Manager and launch again — it should start normally rather than claiming another instance is active.

3. **The widget is visible and transparent.** The rounded surface should show the desktop behind it, with no opaque rectangle or black box around the corners. This is the check that WebView2 transparency behaves; if it fails, note whether it fails only in Desktop mode.

4. **Desktop mode.** Run `agent-gauge.exe --diagnose-window-layer` from a terminal and read the printed report after a couple of seconds. It states whether the widget attached to Explorer's `WorkerW` or fell back to bottom-of-z-order.
   - `attached to the desktop (WorkerW)` — confirm the widget stays put when other windows are activated, and that desktop icons are still visible and clickable.
   - `below other windows` — the documented fallback. Confirm the widget is still visible and still sits under ordinary windows.

   Then restart Explorer (Task Manager → Windows Explorer → Restart) and confirm the widget reappears on the desktop layer within about five seconds. If you have more than one monitor, especially with one placed above or to the left of the primary, confirm the widget returns to the same monitor and position after a restart rather than jumping.

5. **Drag, resize, lock.** Unlock the layout and confirm dragging and edge-resizing work, then lock and confirm clicks pass through. Do this in both Desktop and Pinned mode; a reparented window handles mouse input differently.

6. **Tray.** Confirm a left click opens the tray menu, and that every entry works.

7. **Codex.** Confirm the Codex tracker connects when the CLI is installed. The CLI installs as `codex.cmd` via npm, which needs a `PATHEXT` search Windows does not do on its own — if Codex reports `Codex CLI unavailable` while `codex` works in a terminal, that resolution is the thing to report.

8. **Claude capture end to end.** Connect capture, use Claude Code until it emits rate-limit data, and confirm readings appear. Check the `statusLine` command written into `%USERPROFILE%\.claude\settings.json` is correctly quoted — it must survive the space in `C:\Program Files`.

9. **Autostart across a reboot.** Enable start at login, reboot, and confirm the widget returns on its own with its position and mode intact.

## Report useful failures

Include the Agent Gauge version, the tracker status message, and the Config/Cache/Adapters paths shown under Diagnostics. For anything about where the widget sits on Windows, include the `--diagnose-window-layer` output. Do not include provider credentials or raw Claude status payloads.
