# Agent Gauge end-user QA

This checklist tests the packaged application. It does not require Rust, Node.js, compilers, or development libraries.

## 1. Install and first launch

1. Double-click `src-tauri/target/release/bundle/deb/Agent Gauge_0.1.4_amd64.deb` and install it with Software Manager.
2. Launch **Agent Gauge** from the Cinnamon application menu.
3. Confirm that Settings opens on first launch, the tray icon appears, and the widget appears without a terminal window.
4. Choose whether it should start at login, then select **Continue**.

Pass condition: Settings, tray, and widget all start normally. No SDK or build tool is requested.

## 2. Widget behavior

Use the tray menu or Settings to exercise these controls:

- Switch between **Desktop** and **Pinned**. Desktop should sit behind normal windows; Pinned should sit above them.
- Choose **Unlock layout**, then drag and resize the widget. Lock it again and confirm clicks pass through to the desktop or window beneath it.
- Resize down to the enforced minimum and confirm every provider-reported ring, bar, reset time, and known balance remains visible; missing metrics should collapse without leaving reserved blank slots.
- Hide and show the widget. The tray icon must remain available while it is hidden.
- Before hiding, note the exact widget position and size. Show it again and confirm both are unchanged.
- Select each theme: Glass, Cutout, and Signal.
- Quit from the tray, reopen Agent Gauge, and confirm the chosen theme, layer, position, size, visibility, and lock state return.

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

Automatic first-run connection deliberately edits `~/.claude/settings.json`. Agent Gauge records the previous status-line value and restores it on disconnect. If you manually change that setting after connection, disconnect should report a conflict and leave your newer value alone.

## 5. Start at login

Toggle **Start at login** on, close and reopen Settings, and confirm it remains on. Toggle it off and confirm it remains off. A full login/reboot check is the final real-desktop validation.

## 6. Optional adapter check

Confirm the example adapter is explained separately, does not appear as an enabled tracker, and does not count toward the five additional-tracker slots. Select **Add tracker**, create a named starter, and confirm it remains disabled and untrusted. Its folder appears under the adapters path shown in Diagnostics. Up to five starters may exist. After implementing and reviewing one, **Trust & enable** binds trust to its manifest and executable hashes; modifying either file must disable execution until it is trusted again.

## Report useful failures

Include the Agent Gauge version, the tracker status message, and the Config/Cache/Adapters paths shown under Diagnostics. Do not include provider credentials or raw Claude status payloads.
