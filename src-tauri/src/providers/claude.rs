use std::{
    fs,
    io::{Read, Write},
    path::Path,
    process::Stdio,
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    model::{
        ActionResult, ClaudeCaptureState, ClaudeCaptureStatus, ConnectionState, ProviderSnapshot,
        UsageWindow, WindowDisplay, SNAPSHOT_SCHEMA_VERSION,
    },
    paths,
    platform::{exec, shell},
    settings::atomic_write_json,
};

use super::{claude_usage, now_unix, timestamp, ProviderFailure};

const CAPTURE_FILE: &str = "claude-capture.json";
const INTEGRATION_FILE: &str = "claude-integration.json";
const INPUT_CAP: u64 = 256 * 1024;

/// Schema 1 pointed Claude Code at a generated Python script, which shelled out
/// through `/bin/sh` to chain any status line the user already had. Schema 2
/// points Claude Code straight at the Agent Gauge executable, which does the
/// capture and the chaining itself. That removes a Python dependency, a
/// generated file, and an install path that could half-succeed — and it is the
/// only version that can work on Windows, where neither `#!` nor `/bin/sh`
/// exists. See `migrate_legacy_install`.
const INTEGRATION_SCHEMA_VERSION: u32 = 2;

/// A status line is on the critical path of Claude Code's own rendering, so a
/// chained command that hangs must not hang Claude. Matches the timeout the
/// generated Python dispatcher used.
const CHAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Claude Code renders a single status line; anything beyond this is not
/// something it would display.
const CHAIN_OUTPUT_CAP: u64 = 64 * 1024;

/// How long a status-line capture is treated as current.
///
/// The status line repaints throughout an interactive session, so a capture
/// younger than this means a terminal is actively feeding us and a network
/// round trip would only confirm what we already know. Older than this and no
/// terminal is running — the steady state for anyone working in the desktop
/// app, which draws its interface in Electron and never renders a status line —
/// so the usage endpoint is asked instead.
const CAPTURE_TRUSTED_FOR: i64 = 120;

const FROM_CAPTURE: &str = "Captured from Claude Code";
const FROM_ENDPOINT: &str = "Read from your Claude account";

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ClaudeCapture {
    schema_version: u32,
    observed_at: i64,
    claude_version: Option<String>,
    five_hour: Option<CapturedWindow>,
    seven_day: Option<CapturedWindow>,
}

/// One rate-limit window, from either of the two sources. They report the same
/// two facts under different names, so they converge here.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct CapturedWindow {
    pub(super) used_percent: f64,
    pub(super) resets_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct IntegrationMetadata {
    schema_version: u32,
    had_previous: bool,
    previous: Option<Value>,
    installed: Value,
}

/// Claude usage, from whichever source can currently answer.
///
/// The status-line capture is preferred while it is current: it costs nothing,
/// needs no credentials and no network, and during an interactive session it is
/// the freshest thing there is. It is also, on its own, incomplete — it exists
/// only while a terminal is open, which left desktop-app users with a
/// permanently empty gauge. `claude_usage` covers that gap.
///
/// A capture that has gone stale is still kept as the fallback for an endpoint
/// that cannot answer. It carries its true age, and `UsageWindow::rolled_over`
/// drops it outright once its window turns over, so an old reading is shown as
/// old rather than mistaken for a current one.
pub fn read() -> Result<ProviderSnapshot, ProviderFailure> {
    let now = now_unix();
    let capture = load_capture()?;

    if let Some(capture) = capture.as_ref().filter(|capture| capture.is_current(now)) {
        return Ok(capture.snapshot(FROM_CAPTURE));
    }

    match claude_usage::read(now) {
        Ok(reading) => Ok(snapshot(
            reading.five_hour,
            reading.seven_day,
            reading.observed_at,
            FROM_ENDPOINT,
        )),
        Err(failure) => match capture {
            Some(capture) if capture.has_usage() => Ok(capture.snapshot(FROM_CAPTURE)),
            // Never captured anything and no sign-in to read: a fresh install,
            // not a fault. Keep the onboarding wording rather than reporting an
            // error against something the user has not set up yet.
            None if failure.code == "oauth_missing" => Ok(waiting_snapshot()),
            _ => Err(failure),
        },
    }
}

impl ClaudeCapture {
    fn has_usage(&self) -> bool {
        self.five_hour.is_some() || self.seven_day.is_some()
    }

    fn is_current(&self, now: i64) -> bool {
        self.has_usage() && now.saturating_sub(self.observed_at) < CAPTURE_TRUSTED_FOR
    }

    fn snapshot(&self, status_message: &str) -> ProviderSnapshot {
        snapshot(
            self.five_hour.clone(),
            self.seven_day.clone(),
            self.observed_at,
            status_message,
        )
    }
}

fn snapshot(
    five_hour: Option<CapturedWindow>,
    seven_day: Option<CapturedWindow>,
    observed_at: i64,
    status_message: &str,
) -> ProviderSnapshot {
    let mut windows = Vec::new();
    if let Some(window) = five_hour {
        windows.push(UsageWindow {
            id: "five-hour".into(),
            label: "5 hour".into(),
            used_percent: window.used_percent,
            resets_at: window.resets_at,
            window_minutes: Some(300),
            display: WindowDisplay::Ring,
        });
    }
    if let Some(window) = seven_day {
        windows.push(UsageWindow {
            id: "seven-day".into(),
            label: "Weekly".into(),
            used_percent: window.used_percent,
            resets_at: window.resets_at,
            window_minutes: Some(10_080),
            display: WindowDisplay::Bar,
        });
    }

    ProviderSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        id: "claude".into(),
        name: "Claude".into(),
        accent: Some("#d9986a".into()),
        state: if windows.is_empty() {
            ConnectionState::Waiting
        } else {
            ConnectionState::Connected
        },
        status_message: if windows.is_empty() {
            "Waiting for Claude Code rate-limit data".into()
        } else {
            status_message.into()
        },
        observed_at: Some(observed_at),
        last_attempt_at: None,
        error_code: None,
        windows,
        balances: Vec::new(),
        refreshing: false,
    }
}

/// The capture file, or `None` when no status line has ever reported in.
fn load_capture() -> Result<Option<ClaudeCapture>, ProviderFailure> {
    let path = paths::cache_dir().join(CAPTURE_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ProviderFailure::new(
                "capture_unreadable",
                format!("Claude capture is unreadable: {error}"),
            ))
        }
    };
    let capture: ClaudeCapture = serde_json::from_slice(&bytes)
        .map_err(|_| ProviderFailure::new("capture_malformed", "Claude capture is malformed"))?;
    if capture.schema_version != 1 {
        return Err(ProviderFailure::new(
            "capture_version",
            "Claude capture format is unsupported",
        ));
    }
    Ok(Some(capture))
}

fn waiting_snapshot() -> ProviderSnapshot {
    ProviderSnapshot::waiting(
        "claude",
        "Claude",
        "Waiting for Claude Code activity",
        Some("#d9986a"),
    )
}

/// Entry point for `--capture-claude`.
///
/// Claude Code runs this on every status-line update, so it stays headless and
/// cheap. Two responsibilities: record the usage payload, and honour whatever
/// status line the user had configured before Agent Gauge took the slot.
///
/// Chaining runs even if the capture fails, and its own failures are swallowed.
/// Agent Gauge borrowed this slot; a bug on our side must not silently delete
/// the status line the user actually asked for.
pub fn capture_status_line_stdin() -> Result<(), String> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(INPUT_CAP + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read Claude status input: {error}"))?;
    if bytes.len() as u64 > INPUT_CAP {
        return Err("Claude status input exceeded the safe limit".into());
    }

    let captured = capture_payload(&bytes);

    if let Some(command) = chained_command() {
        if let Some(output) = run_chained(&command, &bytes) {
            let mut stdout = std::io::stdout();
            let _ = stdout.write_all(&output);
            let _ = stdout.flush();
        }
    }

    captured
}

fn capture_payload(bytes: &[u8]) -> Result<(), String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| "Claude status input was not valid JSON".to_string())?;
    let rate_limits = value.get("rate_limits").unwrap_or(&Value::Null);
    let capture = merge_capture(
        load_capture().ok().flatten(),
        value.get("version").and_then(Value::as_str).map(Into::into),
        parse_window(rate_limits.get("five_hour")),
        parse_window(rate_limits.get("seven_day")),
        now_unix(),
    );
    atomic_write_json(&paths::cache_dir().join(CAPTURE_FILE), &capture)
}

/// Folds a status-line payload into what was already captured.
///
/// Claude Code does not put `rate_limits` in every status-line payload: it
/// documents the field as absent whenever plan limits do not apply (API key,
/// Bedrock, Vertex), and it is simply not there yet on the repaints that happen
/// before a session's first API response. Taking each payload as the whole
/// truth meant one such repaint blanked a perfectly good reading — so the
/// widget went back to "waiting" at the start of every session, which is the
/// moment a usage gauge is most worth looking at.
///
/// `observed_at` therefore tracks the last payload that actually carried usage,
/// not the last repaint. A retained figure is presented as being as old as it
/// really is, and `UsageWindow::rolled_over` still discards it outright once
/// its own reset time passes.
fn merge_capture(
    previous: Option<ClaudeCapture>,
    claude_version: Option<String>,
    five_hour: Option<CapturedWindow>,
    seven_day: Option<CapturedWindow>,
    now: i64,
) -> ClaudeCapture {
    let carried_usage = five_hour.is_some() || seven_day.is_some();
    let previous = previous.filter(|capture| capture.schema_version == 1);

    ClaudeCapture {
        schema_version: 1,
        observed_at: match previous.as_ref() {
            Some(capture) if !carried_usage => capture.observed_at,
            _ => now,
        },
        claude_version,
        five_hour: five_hour.or_else(|| previous.as_ref().and_then(|c| c.five_hour.clone())),
        seven_day: seven_day.or_else(|| previous.as_ref().and_then(|c| c.seven_day.clone())),
    }
}

/// The status-line command that was configured before Agent Gauge replaced it,
/// if there was one.
fn chained_command() -> Option<String> {
    let metadata = read_metadata().ok()?;
    metadata.previous.as_ref().and_then(status_line_command)
}

/// Runs the user's previous status-line command and returns what it printed.
///
/// The command is shell source — that is what Claude Code stores and what the
/// user wrote — so it is handed back to a shell rather than parsed into an argv.
/// Returns `None` on any failure, including the timeout: a broken chained
/// command should cost the user their extra status text, nothing more.
fn run_chained(command: &str, payload: &[u8]) -> Option<Vec<u8>> {
    let mut child = exec::shell_command(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    // stdin and stdout are serviced on separate threads. Writing the payload
    // inline would deadlock as soon as it exceeded the pipe buffer and the
    // child blocked writing output nobody was reading.
    let mut stdin = child.stdin.take()?;
    let owned = payload.to_vec();
    thread::spawn(move || {
        let _ = stdin.write_all(&owned);
        // Dropping the handle closes the pipe, which is what signals EOF.
    });

    let stdout = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.take(CHAIN_OUTPUT_CAP).read_to_end(&mut bytes);
        bytes
    });

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < CHAIN_TIMEOUT => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Err(_) => return None,
        }
    }

    reader.join().ok()
}

fn parse_window(value: Option<&Value>) -> Option<CapturedWindow> {
    let value = value?;
    let used_percent = value
        .get("used_percentage")
        .or_else(|| value.get("used_percent"))?
        .as_f64()?;
    used_percent.is_finite().then(|| CapturedWindow {
        used_percent,
        resets_at: value.get("resets_at").and_then(timestamp),
    })
}

pub fn read_capture_status() -> ClaudeCaptureStatus {
    let settings_path = paths::claude_settings_path();
    let metadata_path = paths::config_dir().join(INTEGRATION_FILE);
    let state = match (read_json(&settings_path), read_json(&metadata_path)) {
        (Err(_), _) if settings_path.exists() => ClaudeCaptureState::SettingsInvalid,
        (_, Ok(metadata)) => {
            let installed = metadata.get("installed");
            let live = read_json(&settings_path)
                .ok()
                .and_then(|settings| settings.get("statusLine").cloned());
            if live.as_ref() == installed {
                ClaudeCaptureState::Installed
            } else {
                ClaudeCaptureState::Conflict
            }
        }
        _ => ClaudeCaptureState::NotInstalled,
    };
    let message = match state {
        ClaudeCaptureState::NotInstalled => "Claude capture is not connected",
        ClaudeCaptureState::Installed => "Claude capture is connected",
        ClaudeCaptureState::Conflict => "Claude status-line settings changed after setup",
        ClaudeCaptureState::SettingsInvalid => "Claude settings contain invalid JSON",
    };
    ClaudeCaptureStatus {
        state,
        message: message.into(),
        settings_path: settings_path.display().to_string(),
    }
}

pub fn install_capture() -> ActionResult {
    match install_capture_inner() {
        Ok(message) => ActionResult {
            ok: true,
            code: "installed".into(),
            message,
        },
        Err((code, message)) => ActionResult {
            ok: false,
            code,
            message,
        },
    }
}

fn install_capture_inner() -> Result<String, (String, String)> {
    let settings_path = paths::claude_settings_path();
    let mut settings = if settings_path.exists() {
        read_json(&settings_path).map_err(|message| ("settings_invalid".into(), message))?
    } else {
        Value::Object(Default::default())
    };
    let object = settings.as_object_mut().ok_or_else(|| {
        (
            "settings_invalid".into(),
            "Claude settings must contain a JSON object".into(),
        )
    })?;
    let previous = object.get("statusLine").cloned();
    let previous_command = previous.as_ref().and_then(status_line_command);
    if previous.is_some() && previous_command.is_none() {
        return Err((
            "status_line_unsupported".into(),
            "The existing Claude status line is not a command Agent Gauge can safely preserve"
                .into(),
        ));
    }

    let installed = installed_status_line()?;
    object.insert("statusLine".into(), installed.clone());
    let metadata = IntegrationMetadata {
        schema_version: INTEGRATION_SCHEMA_VERSION,
        had_previous: previous.is_some(),
        previous,
        installed,
    };
    atomic_write_json(&paths::config_dir().join(INTEGRATION_FILE), &metadata)
        .map_err(|message| ("metadata_write_failed".into(), message))?;
    if let Err(message) = atomic_write_json(&settings_path, &settings) {
        // Claude's settings are the source of truth. If they could not be
        // updated, our metadata claims an install that does not exist, so drop
        // it rather than leave the two disagreeing.
        let _ = fs::remove_file(paths::config_dir().join(INTEGRATION_FILE));
        return Err(("settings_write_failed".into(), message));
    }
    Ok("Claude capture connected; data appears after normal Claude Code activity".into())
}

pub fn remove_capture() -> ActionResult {
    match remove_capture_inner() {
        Ok(message) => ActionResult {
            ok: true,
            code: "removed".into(),
            message,
        },
        Err((code, message)) => ActionResult {
            ok: false,
            code,
            message,
        },
    }
}

fn remove_capture_inner() -> Result<String, (String, String)> {
    let metadata_path = paths::config_dir().join(INTEGRATION_FILE);
    let metadata: IntegrationMetadata = fs::read(&metadata_path)
        .map_err(|error| ("not_installed".into(), error.to_string()))
        .and_then(|bytes| {
            serde_json::from_slice(&bytes).map_err(|_| {
                (
                    "metadata_invalid".into(),
                    "Capture metadata is invalid".into(),
                )
            })
        })?;
    let settings_path = paths::claude_settings_path();
    let mut settings =
        read_json(&settings_path).map_err(|message| ("settings_invalid".into(), message))?;
    let object = settings.as_object_mut().ok_or_else(|| {
        (
            "settings_invalid".into(),
            "Claude settings must contain a JSON object".into(),
        )
    })?;
    if object.get("statusLine") != Some(&metadata.installed) {
        return Err((
            "settings_conflict".into(),
            "Claude status-line settings changed; Agent Gauge left them untouched".into(),
        ));
    }
    if metadata.had_previous {
        if let Some(previous) = metadata.previous {
            object.insert("statusLine".into(), previous);
        }
    } else {
        object.remove("statusLine");
    }
    atomic_write_json(&settings_path, &settings)
        .map_err(|message| ("settings_write_failed".into(), message))?;
    let _ = fs::remove_file(metadata_path);
    let _ = fs::remove_file(paths::legacy_claude_dispatcher_path());
    Ok("Claude capture disconnected and the prior status line restored".into())
}

/// The `statusLine` value Agent Gauge writes into Claude Code's settings.
///
/// Claude Code hands this string to a shell, so it is shell source and must be
/// quoted for the host's shell — an installation under `C:\Program Files` or a
/// Linux home directory with a space in it both depend on getting this right.
fn installed_status_line() -> Result<Value, (String, String)> {
    let executable = std::env::current_exe()
        .map_err(|error| ("executable_missing".into(), error.to_string()))?;
    let command = shell::command_line(&executable, &["--capture-claude"])
        .map_err(|message| ("executable_unquotable".into(), message))?;

    Ok(serde_json::json!({
        "type": "command",
        "command": command,
        "padding": 0
    }))
}

#[derive(Debug, PartialEq, Eq)]
enum MigrationDecision {
    /// Already current, or nothing of ours to migrate.
    NotNeeded,
    /// Out of date, but the live status line is no longer the one we wrote.
    /// The user has taken the slot back; leave it exactly as found.
    Blocked,
    Proceed,
}

fn migration_decision(
    schema_version: u32,
    live_status_line: Option<&Value>,
    installed: &Value,
) -> MigrationDecision {
    if schema_version >= INTEGRATION_SCHEMA_VERSION {
        return MigrationDecision::NotNeeded;
    }
    if live_status_line != Some(installed) {
        return MigrationDecision::Blocked;
    }
    MigrationDecision::Proceed
}

fn read_metadata() -> Result<IntegrationMetadata, String> {
    let path = paths::config_dir().join(INTEGRATION_FILE);
    let bytes =
        fs::read(&path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|_| "Capture metadata is invalid".to_string())
}

/// Moves an existing schema-1 installation onto the self-exec status line.
///
/// Runs at startup. The generated Python dispatcher still works on Linux, so
/// this is not urgent for a running install — but leaving two mechanisms alive
/// means every future change has to be made twice, which is exactly the drift
/// this port exists to avoid.
///
/// Deliberately conservative: if the live status line is not the one we
/// installed, the user has since changed it themselves and we leave it alone.
/// `previous` is carried across unchanged, so disconnecting still restores
/// whatever was there before Agent Gauge.
pub fn migrate_legacy_install() {
    let Ok(metadata) = read_metadata() else {
        return;
    };

    let settings_path = paths::claude_settings_path();
    let Ok(mut settings) = read_json(&settings_path) else {
        return;
    };
    let Some(object) = settings.as_object_mut() else {
        return;
    };

    if migration_decision(
        metadata.schema_version,
        object.get("statusLine"),
        &metadata.installed,
    ) != MigrationDecision::Proceed
    {
        return;
    }

    let Ok(installed) = installed_status_line() else {
        return;
    };
    object.insert("statusLine".into(), installed.clone());

    let migrated = IntegrationMetadata {
        schema_version: INTEGRATION_SCHEMA_VERSION,
        had_previous: metadata.had_previous,
        previous: metadata.previous,
        installed,
    };
    if atomic_write_json(&paths::config_dir().join(INTEGRATION_FILE), &migrated).is_err() {
        return;
    }
    if atomic_write_json(&settings_path, &settings).is_err() {
        return;
    }

    let _ = fs::remove_file(paths::legacy_claude_dispatcher_path());
}

fn status_line_command(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(Into::into)
        .or_else(|| value.get("command").and_then(Value::as_str).map(Into::into))
}

fn read_json(path: &Path) -> Result<Value, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| format!("{} does not contain valid JSON", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_only_documented_rate_limit_fields() {
        let value = serde_json::json!({
            "used_percentage": 12.5,
            "resets_at": "2030-01-01T00:00:00Z",
            "secret": "ignored"
        });
        let parsed = parse_window(Some(&value)).unwrap();
        assert_eq!(parsed.used_percent, 12.5);
        assert!(parsed.resets_at.is_some());
    }

    fn captured(used_percent: f64, resets_at: i64) -> CapturedWindow {
        CapturedWindow {
            used_percent,
            resets_at: Some(resets_at),
        }
    }

    #[test]
    fn a_payload_without_rate_limits_does_not_blank_the_last_reading() {
        // The regression this guards: Claude Code repaints its status line
        // before the session's first API response, and that payload has no
        // `rate_limits`. Treating it as "usage is now unknown" wiped the file.
        let previous = merge_capture(None, None, Some(captured(42.0, 9_000)), None, 1_000);

        let merged = merge_capture(previous.clone().into(), None, None, None, 5_000);

        assert_eq!(merged.five_hour.unwrap().used_percent, 42.0);
        assert_eq!(
            merged.observed_at, 1_000,
            "a repaint that carried no usage must not restamp the reading as fresh"
        );
    }

    #[test]
    fn fresh_usage_replaces_the_retained_figure_and_restamps_it() {
        let previous = merge_capture(None, None, Some(captured(42.0, 9_000)), None, 1_000);

        let merged = merge_capture(
            previous.into(),
            None,
            Some(captured(55.0, 9_000)),
            None,
            5_000,
        );

        assert_eq!(merged.five_hour.unwrap().used_percent, 55.0);
        assert_eq!(merged.observed_at, 5_000);
    }

    #[test]
    fn retention_never_invents_a_reading_that_was_never_taken() {
        let merged = merge_capture(None, None, None, None, 5_000);

        assert!(merged.five_hour.is_none());
        assert!(merged.seven_day.is_none());
        assert_eq!(merged.observed_at, 5_000);
    }

    #[test]
    fn a_capture_written_by_an_unknown_schema_is_not_carried_forward() {
        // `read` refuses such a file, so retention must refuse it too rather
        // than mixing fields it cannot vouch for into a schema-1 capture.
        let mut future = merge_capture(None, None, Some(captured(42.0, 9_000)), None, 1_000);
        future.schema_version = 99;

        let merged = merge_capture(future.into(), None, None, None, 5_000);

        assert!(merged.five_hour.is_none());
        assert_eq!(merged.observed_at, 5_000);
    }

    #[test]
    fn a_live_terminal_session_is_answered_from_the_capture_alone() {
        // The status line repaints constantly while a session is open, so a
        // young capture must not trigger a network round trip.
        let capture = merge_capture(None, None, Some(captured(42.0, 9_000)), None, 1_000);

        assert!(capture.is_current(1_000 + CAPTURE_TRUSTED_FOR - 1));
    }

    #[test]
    fn a_capture_no_terminal_is_feeding_stops_being_current() {
        // The desktop app renders its UI in Electron and never runs a status
        // line, so its sessions leave the capture frozen. Past this point the
        // usage endpoint has to be asked or the gauge simply stops moving.
        let capture = merge_capture(None, None, Some(captured(42.0, 9_000)), None, 1_000);

        assert!(!capture.is_current(1_000 + CAPTURE_TRUSTED_FOR));
    }

    #[test]
    fn a_capture_holding_no_usage_is_never_treated_as_current() {
        // Freshly written by a repaint that carried no rate limits: recent, but
        // with nothing in it to show.
        let capture = merge_capture(None, None, None, None, 1_000);

        assert!(!capture.is_current(1_000));
        assert!(!capture.has_usage());
    }

    #[test]
    fn a_snapshot_reports_when_its_numbers_were_taken_not_when_it_was_built() {
        let capture = merge_capture(None, None, Some(captured(42.0, 9_000)), None, 1_000);

        let snapshot = capture.snapshot(FROM_CAPTURE);

        assert_eq!(snapshot.observed_at, Some(1_000));
        assert_eq!(snapshot.state, ConnectionState::Connected);
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.status_message, FROM_CAPTURE);
    }

    #[test]
    fn missing_window_is_honestly_absent() {
        assert!(parse_window(None).is_none());
        assert!(parse_window(Some(&serde_json::json!({}))).is_none());
    }

    fn legacy_installed() -> Value {
        serde_json::json!({
            "type": "command",
            "command": "/home/someone/.config/agent-gauge/claude-status-dispatcher.py",
            "padding": 0
        })
    }

    #[test]
    fn migration_upgrades_an_installation_we_still_own() {
        let installed = legacy_installed();
        assert_eq!(
            migration_decision(1, Some(&installed), &installed),
            MigrationDecision::Proceed
        );
    }

    #[test]
    fn migration_leaves_a_status_line_the_user_has_taken_back() {
        // The single most important case: Claude Code's settings belong to the
        // user, and a status line we no longer own must survive untouched.
        let installed = legacy_installed();
        let theirs = serde_json::json!({ "type": "command", "command": "my-own-script" });

        assert_eq!(
            migration_decision(1, Some(&theirs), &installed),
            MigrationDecision::Blocked
        );
        assert_eq!(
            migration_decision(1, None, &installed),
            MigrationDecision::Blocked
        );
    }

    #[test]
    fn migration_is_a_no_op_once_current() {
        let installed = legacy_installed();
        assert_eq!(
            migration_decision(INTEGRATION_SCHEMA_VERSION, Some(&installed), &installed),
            MigrationDecision::NotNeeded
        );
        // And stays a no-op if a future version writes a newer schema.
        assert_eq!(
            migration_decision(INTEGRATION_SCHEMA_VERSION + 1, None, &installed),
            MigrationDecision::NotNeeded
        );
    }

    #[test]
    fn the_installed_command_reinvokes_this_executable_for_capture() {
        let installed = installed_status_line().expect("current_exe should be resolvable");

        let command = installed
            .get("command")
            .and_then(Value::as_str)
            .expect("the status line must carry a command string");

        assert!(command.contains("--capture-claude"));
        assert!(
            !command.contains("python") && !command.contains("/bin/sh"),
            "the status line must not depend on an interpreter or a shell being installed"
        );

        // Quoted for the host shell, since Claude Code hands this to a shell.
        let quoted = if cfg!(target_os = "windows") {
            '"'
        } else {
            '\''
        };
        assert!(
            command.starts_with(quoted),
            "the executable path must be quoted: {command}"
        );
    }

    #[test]
    fn a_previous_status_line_is_recovered_from_both_recorded_shapes() {
        // Claude Code accepts a bare string or an object with `command`;
        // chaining has to understand whichever the user had.
        assert_eq!(
            status_line_command(&serde_json::json!("my-script --flag")).as_deref(),
            Some("my-script --flag")
        );
        assert_eq!(
            status_line_command(&serde_json::json!({ "type": "command", "command": "my-script" }))
                .as_deref(),
            Some("my-script")
        );
        assert_eq!(
            status_line_command(&serde_json::json!({ "type": "other" })),
            None
        );
    }
}
