use std::{fs, io::Read, path::Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    model::{
        ActionResult, ClaudeCaptureState, ClaudeCaptureStatus, ConnectionState, ProviderSnapshot,
        UsageWindow, WindowDisplay, SNAPSHOT_SCHEMA_VERSION,
    },
    paths,
    settings::atomic_write_json,
};

use super::{now_unix, ProviderFailure};

const CAPTURE_FILE: &str = "claude-capture.json";
const INTEGRATION_FILE: &str = "claude-integration.json";
const INPUT_CAP: u64 = 256 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ClaudeCapture {
    schema_version: u32,
    observed_at: i64,
    claude_version: Option<String>,
    five_hour: Option<CapturedWindow>,
    seven_day: Option<CapturedWindow>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CapturedWindow {
    used_percent: f64,
    resets_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct IntegrationMetadata {
    schema_version: u32,
    had_previous: bool,
    previous: Option<Value>,
    installed: Value,
}

pub fn read() -> Result<ProviderSnapshot, ProviderFailure> {
    let path = paths::cache_dir().join(CAPTURE_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(waiting_snapshot()),
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

    let mut windows = Vec::new();
    if let Some(window) = capture.five_hour {
        windows.push(UsageWindow {
            id: "five-hour".into(),
            label: "5 hour".into(),
            used_percent: window.used_percent,
            resets_at: window.resets_at,
            window_minutes: Some(300),
            display: WindowDisplay::Ring,
        });
    }
    if let Some(window) = capture.seven_day {
        windows.push(UsageWindow {
            id: "seven-day".into(),
            label: "Weekly".into(),
            used_percent: window.used_percent,
            resets_at: window.resets_at,
            window_minutes: Some(10_080),
            display: WindowDisplay::Bar,
        });
    }

    Ok(ProviderSnapshot {
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
            "Captured from Claude Code".into()
        },
        observed_at: Some(capture.observed_at),
        last_attempt_at: None,
        error_code: None,
        windows,
        balances: Vec::new(),
        refreshing: false,
    })
}

fn waiting_snapshot() -> ProviderSnapshot {
    ProviderSnapshot::waiting(
        "claude",
        "Claude",
        "Waiting for Claude Code activity",
        Some("#d9986a"),
    )
}

pub fn capture_status_line_stdin() -> Result<(), String> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(INPUT_CAP + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read Claude status input: {error}"))?;
    if bytes.len() as u64 > INPUT_CAP {
        return Err("Claude status input exceeded the safe limit".into());
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "Claude status input was not valid JSON".to_string())?;
    let rate_limits = value.get("rate_limits").unwrap_or(&Value::Null);
    let capture = ClaudeCapture {
        schema_version: 1,
        observed_at: now_unix(),
        claude_version: value.get("version").and_then(Value::as_str).map(Into::into),
        five_hour: parse_window(rate_limits.get("five_hour")),
        seven_day: parse_window(rate_limits.get("seven_day")),
    };
    atomic_write_json(&paths::cache_dir().join(CAPTURE_FILE), &capture)
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

fn timestamp(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| {
            value
                .as_str()
                .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
                .map(|time| time.timestamp())
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
    let dispatcher = paths::claude_dispatcher_path();
    let executable = std::env::current_exe()
        .map_err(|error| ("executable_missing".into(), error.to_string()))?;
    let previous_command = previous.as_ref().and_then(status_line_command);
    if previous.is_some() && previous_command.is_none() {
        return Err((
            "status_line_unsupported".into(),
            "The existing Claude status line is not a command Agent Gauge can safely preserve"
                .into(),
        ));
    }
    let script = dispatcher_script(&executable, previous_command.as_deref());
    atomic_write_bytes(&dispatcher, script.as_bytes())
        .map_err(|message| ("dispatcher_write_failed".into(), message))?;
    set_executable(&dispatcher).map_err(|message| ("dispatcher_write_failed".into(), message))?;

    let installed = serde_json::json!({
        "type": "command",
        "command": dispatcher.display().to_string(),
        "padding": 0
    });
    object.insert("statusLine".into(), installed.clone());
    let metadata = IntegrationMetadata {
        schema_version: 1,
        had_previous: previous.is_some(),
        previous,
        installed,
    };
    atomic_write_json(&paths::config_dir().join(INTEGRATION_FILE), &metadata)
        .map_err(|message| ("metadata_write_failed".into(), message))?;
    if let Err(message) = atomic_write_json(&settings_path, &settings) {
        let _ = fs::remove_file(paths::config_dir().join(INTEGRATION_FILE));
        let _ = fs::remove_file(&dispatcher);
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
    let _ = fs::remove_file(paths::claude_dispatcher_path());
    Ok("Claude capture disconnected and the prior status line restored".into())
}

fn dispatcher_script(executable: &Path, previous: Option<&str>) -> String {
    let executable = serde_json::to_string(&executable.display().to_string()).unwrap();
    let previous = serde_json::to_string(&previous).unwrap();
    format!(
        r#"#!/usr/bin/env python3
import subprocess, sys
payload = sys.stdin.buffer.read(262145)
if len(payload) > 262144:
    raise SystemExit(0)
subprocess.run([{executable}, "--capture-claude"], input=payload, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=2, check=False)
previous = {previous}
if previous:
    result = subprocess.run(["/bin/sh", "-c", previous], input=payload, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=2, check=False)
    sys.stdout.buffer.write(result.stdout[:65536])
"#
    )
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

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    crate::paths::ensure_parent(path)?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes)
        .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path)
        .map_err(|error| format!("could not replace {}: {error}", path.display()))
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|error| error.to_string())?
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
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

    #[test]
    fn missing_window_is_honestly_absent() {
        assert!(parse_window(None).is_none());
        assert!(parse_window(Some(&serde_json::json!({}))).is_none());
    }
}
