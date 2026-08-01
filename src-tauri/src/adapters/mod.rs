use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

use crate::{
    model::{
        ActionResult, AdapterInfo, AdapterTrust, Balance, ConnectionState, ProviderSnapshot,
        UsageWindow, WindowDisplay, SNAPSHOT_SCHEMA_VERSION,
    },
    paths,
    providers::{self, ProviderFailure, ProviderStore},
    settings::SettingsStore,
};

const OUTPUT_CAP: u64 = 64 * 1024;
const STDERR_CAP: u64 = 8 * 1024;
const MAX_TIMEOUT: u64 = 30;
const MAX_CUSTOM_ADAPTERS: usize = 5;

#[derive(Debug, Clone, Deserialize)]
struct Manifest {
    schema_version: u32,
    id: String,
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default = "default_interval")]
    refresh_interval_seconds: u64,
    #[serde(default = "default_timeout")]
    timeout_seconds: u64,
    accent: Option<String>,
}

#[derive(Debug, Clone)]
struct Discovered {
    directory: PathBuf,
    manifest: Manifest,
    executable: PathBuf,
    manifest_hash: String,
    executable_hash: String,
}

#[derive(Debug, Deserialize)]
struct AdapterSnapshot {
    schema_version: u32,
    observed_at: Option<String>,
    status: String,
    #[serde(default)]
    windows: Vec<AdapterWindow>,
    #[serde(default)]
    balances: Vec<AdapterBalance>,
}

#[derive(Debug, Deserialize)]
struct AdapterWindow {
    id: String,
    label: String,
    used_percent: f64,
    resets_at: Option<String>,
    window_minutes: Option<u64>,
    display: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AdapterBalance {
    id: String,
    label: String,
    amount: String,
    unit: String,
}

pub fn ensure_sample() {
    let directory = paths::adapters_dir().join("sample");
    let manifest = directory.join("manifest.json");
    let executable = directory.join("read-usage");
    if manifest.exists() || executable.exists() {
        return;
    }
    if let Err(error) = fs::create_dir_all(&directory) {
        eprintln!("Agent Gauge could not create sample adapter: {error}");
        return;
    }
    let manifest_value = serde_json::json!({
        "schema_version": 1,
        "id": "sample",
        "name": "Sample Adapter",
        "command": "./read-usage",
        "args": [],
        "refresh_interval_seconds": 300,
        "timeout_seconds": 10,
        "accent": "#8fc98a"
    });
    let script = r#"#!/usr/bin/env python3
import datetime, json
now = datetime.datetime.now(datetime.timezone.utc)
print(json.dumps({
  "schema_version": 1,
  "observed_at": now.isoformat().replace("+00:00", "Z"),
  "status": "connected",
  "windows": [
    {"id":"rolling","label":"5 hour","used_percent":32.0,"resets_at":(now+datetime.timedelta(hours=2)).isoformat().replace("+00:00","Z"),"window_minutes":300,"display":"ring"},
    {"id":"weekly","label":"Weekly","used_percent":14.0,"resets_at":(now+datetime.timedelta(days=4)).isoformat().replace("+00:00","Z"),"window_minutes":10080,"display":"bar"}
  ],
  "balances": [{"id":"credits","label":"Credits","amount":"12.50","unit":"USD"}]
}))
"#;
    if crate::settings::atomic_write_json(&manifest, &manifest_value).is_err()
        || fs::write(&executable, script).is_err()
    {
        eprintln!("Agent Gauge could not write the sample adapter");
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(&executable) {
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o700);
            let _ = fs::set_permissions(&executable, permissions);
        }
    }
}

pub fn ensure_sample_disabled(app: &AppHandle) {
    let settings = app.state::<SettingsStore>().snapshot();
    if settings.adapter_trust.contains_key("sample")
        || settings
            .disabled_providers
            .iter()
            .any(|provider| provider == "sample")
    {
        return;
    }
    let _ = app.state::<SettingsStore>().update(|settings| {
        settings.disabled_providers.push("sample".into());
    });
}

pub fn create_scaffold(app: &AppHandle, name: &str) -> ActionResult {
    let name = name.trim();
    if name.len() < 2 || name.len() > 60 {
        return action_error(
            "adapter_name_invalid",
            "Tracker name must be between 2 and 60 characters",
        );
    }
    let custom_count = fs::read_dir(paths::adapters_dir())
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry.file_name().to_string_lossy() != "sample"
        })
        .count();
    if custom_count >= MAX_CUSTOM_ADAPTERS {
        return action_error(
            "adapter_limit",
            "Agent Gauge supports up to five additional trackers",
        );
    }

    let Some(slug) = slug(name) else {
        return action_error(
            "adapter_name_invalid",
            "Tracker name needs at least one letter or number",
        );
    };
    let base = format!("custom-{slug}");
    let id = (1..=99)
        .map(|suffix| {
            if suffix == 1 {
                base.clone()
            } else {
                format!("{base}-{suffix}")
            }
        })
        .find(|candidate| !paths::adapters_dir().join(candidate).exists());
    let Some(id) = id else {
        return action_error("adapter_exists", "Could not choose a unique tracker ID");
    };
    let directory = paths::adapters_dir().join(&id);
    if let Err(error) = fs::create_dir_all(&directory) {
        return action_error(
            "adapter_create_failed",
            format!("Could not create tracker folder: {error}"),
        );
    }
    let manifest = serde_json::json!({
        "schema_version": 1,
        "id": id,
        "name": name,
        "command": "./read-usage",
        "args": [],
        "refresh_interval_seconds": 300,
        "timeout_seconds": 10,
        "accent": "#9a8fc9"
    });
    let script = r#"#!/usr/bin/env python3
import json
print(json.dumps({
  "schema_version": 1,
  "status": "waiting",
  "windows": [],
  "balances": []
}))
"#;
    if let Err(error) =
        crate::settings::atomic_write_json(&directory.join("manifest.json"), &manifest)
            .and_then(|()| {
                fs::write(directory.join("read-usage"), script)
                    .map_err(|error| format!("could not write starter executable: {error}"))
            })
            .and_then(|()| set_executable(&directory.join("read-usage")))
    {
        return action_error("adapter_create_failed", error);
    }
    if let Err(error) = app.state::<SettingsStore>().update(|settings| {
        if !settings
            .provider_order
            .iter()
            .any(|provider| provider == &id)
        {
            settings.provider_order.push(id.clone());
        }
        if !settings
            .disabled_providers
            .iter()
            .any(|provider| provider == &id)
        {
            settings.disabled_providers.push(id.clone());
        }
    }) {
        return action_error("settings_write_failed", error);
    }
    providers::refresh_all(app);
    ActionResult {
        ok: true,
        code: "adapter_created".into(),
        message: format!(
            "Created disabled starter for {name}; connect it in {}",
            directory.display()
        ),
    }
}

pub fn list(app: &AppHandle) -> Vec<AdapterInfo> {
    let settings = app.state::<SettingsStore>().snapshot();
    discover()
        .into_iter()
        .map(|result| match result {
            Ok(adapter) => {
                let trust = settings.adapter_trust.get(&adapter.manifest.id);
                let trusted = trust
                    .map(|trust| trust_matches(trust, &adapter))
                    .unwrap_or(false);
                AdapterInfo {
                    id: adapter.manifest.id.clone(),
                    name: adapter.manifest.name,
                    command: adapter.executable.display().to_string(),
                    args: adapter.manifest.args,
                    refresh_interval_seconds: adapter.manifest.refresh_interval_seconds,
                    trusted,
                    trust_changed: trust.is_some() && !trusted,
                    valid: true,
                    diagnostic: if trusted {
                        None
                    } else if adapter.manifest.id == "sample" {
                        Some("Synthetic example only; disabled until explicitly trusted".into())
                    } else if adapter.manifest.id.starts_with("custom-") {
                        Some("Disabled starter; connect its local files before trusting".into())
                    } else {
                        Some("Review and trust before execution".into())
                    },
                }
            }
            Err((id, message)) => AdapterInfo {
                id: id.clone(),
                name: id,
                command: String::new(),
                args: Vec::new(),
                refresh_interval_seconds: 300,
                trusted: false,
                trust_changed: false,
                valid: false,
                diagnostic: Some(message),
            },
        })
        .collect()
}

pub fn trust(app: &AppHandle, id: &str) -> ActionResult {
    let adapter = match find(id) {
        Ok(adapter) => adapter,
        Err(error) => return action_error("adapter_invalid", error),
    };
    let trust = AdapterTrust {
        manifest_sha256: adapter.manifest_hash,
        executable_sha256: adapter.executable_hash,
    };
    let result = app.state::<SettingsStore>().update(|settings| {
        settings.adapter_trust.insert(id.into(), trust);
        if !settings
            .provider_order
            .iter()
            .any(|provider| provider == id)
        {
            settings.provider_order.push(id.into());
        }
        settings
            .disabled_providers
            .retain(|provider| provider != id);
    });
    match result {
        Ok(_) => {
            refresh(app, id);
            ActionResult {
                ok: true,
                code: "adapter_trusted".into(),
                message: format!("{} is trusted and enabled", adapter.manifest.name),
            }
        }
        Err(error) => action_error("settings_write_failed", error),
    }
}

pub fn revoke(app: &AppHandle, id: &str) -> ActionResult {
    match app.state::<SettingsStore>().update(|settings| {
        settings.adapter_trust.remove(id);
        if !settings
            .disabled_providers
            .iter()
            .any(|provider| provider == id)
        {
            settings.disabled_providers.push(id.into());
        }
    }) {
        Ok(_) => {
            let mut snapshot = adapter_waiting(id, id, ConnectionState::Untrusted, "Trust revoked");
            if let Some(existing) = app.state::<ProviderStore>().snapshot(id) {
                snapshot.name = existing.name;
            }
            app.state::<ProviderStore>().set(snapshot);
            providers::emit(app);
            ActionResult {
                ok: true,
                code: "adapter_untrusted".into(),
                message: "Adapter trust revoked".into(),
            }
        }
        Err(error) => action_error("settings_write_failed", error),
    }
}

pub fn test(app: &AppHandle, id: &str) -> ActionResult {
    let adapter = match trusted(app, id) {
        Ok(adapter) => adapter,
        Err(error) => return action_error("adapter_untrusted", error),
    };
    match run(&adapter) {
        Ok(snapshot) => ActionResult {
            ok: true,
            code: "adapter_ok".into(),
            message: format!(
                "{} returned {} usage window(s) and {} balance(s)",
                snapshot.name,
                snapshot.windows.len(),
                snapshot.balances.len()
            ),
        },
        Err(error) => action_error(&error.code, error.message),
    }
}

pub fn refresh_all(app: &AppHandle) {
    for adapter in discover().into_iter().flatten() {
        let id = adapter.manifest.id.clone();
        let trust = app
            .state::<SettingsStore>()
            .snapshot()
            .adapter_trust
            .get(&id)
            .cloned();
        if trust
            .as_ref()
            .is_some_and(|trust| trust_matches(trust, &adapter))
        {
            refresh(app, &id);
        } else {
            let snapshot = adapter_waiting(
                &id,
                &adapter.manifest.name,
                ConnectionState::Untrusted,
                "Trust this adapter in Settings before it can run",
            );
            app.state::<ProviderStore>().set(snapshot);
        }
    }
    providers::emit(app);
}

pub fn refresh(app: &AppHandle, id: &str) {
    let adapter = match trusted(app, id) {
        Ok(adapter) => adapter,
        Err(message) => {
            let snapshot = adapter_waiting(id, id, ConnectionState::Untrusted, &message);
            app.state::<ProviderStore>().set(snapshot);
            providers::emit(app);
            return;
        }
    };
    if app
        .state::<SettingsStore>()
        .snapshot()
        .disabled_providers
        .iter()
        .any(|provider| provider == id)
    {
        return;
    }
    {
        let store = app.state::<ProviderStore>();
        let mut in_flight = store
            .in_flight
            .lock()
            .expect("provider in-flight mutex poisoned");
        if !in_flight.insert(id.into()) {
            return;
        }
    }
    let app = app.clone();
    let id = id.to_string();
    thread::spawn(move || {
        let result = run(&adapter);
        providers::finish(&app, &id, result);
    });
}

fn discover() -> Vec<Result<Discovered, (String, String)>> {
    let directory = paths::adapters_dir();
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| {
            let id = entry.file_name().to_string_lossy().into_owned();
            load_adapter(&entry.path()).map_err(|error| (id, error))
        })
        .collect()
}

fn find(id: &str) -> Result<Discovered, String> {
    if !valid_id(id) {
        return Err("Adapter ID is invalid".into());
    }
    load_adapter(&paths::adapters_dir().join(id))
}

fn trusted(app: &AppHandle, id: &str) -> Result<Discovered, String> {
    let adapter = find(id)?;
    let settings = app.state::<SettingsStore>().snapshot();
    let trust = settings
        .adapter_trust
        .get(id)
        .ok_or_else(|| "Adapter is not trusted".to_string())?;
    if !trust_matches(trust, &adapter) {
        return Err("Adapter files changed and must be trusted again".into());
    }
    Ok(adapter)
}

fn load_adapter(directory: &Path) -> Result<Discovered, String> {
    let manifest_path = directory.join("manifest.json");
    let manifest_bytes =
        fs::read(&manifest_path).map_err(|error| format!("could not read manifest: {error}"))?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| "manifest is not valid JSON".to_string())?;
    if manifest.schema_version != 1 {
        return Err("unsupported manifest schema version".into());
    }
    let directory_id = directory
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if !valid_id(&manifest.id) || manifest.id != directory_id {
        return Err("manifest ID must match its lowercase adapter directory".into());
    }
    if manifest.name.trim().is_empty() || manifest.name.len() > 80 {
        return Err("adapter name is invalid".into());
    }
    if !(60..=86_400).contains(&manifest.refresh_interval_seconds) {
        return Err("refresh interval must be between 60 and 86400 seconds".into());
    }
    if !(1..=MAX_TIMEOUT).contains(&manifest.timeout_seconds) {
        return Err("timeout must be between 1 and 30 seconds".into());
    }
    if manifest.args.len() > 32 || manifest.args.iter().any(|arg| arg.len() > 2048) {
        return Err("adapter arguments exceed the safe limit".into());
    }

    let executable = resolve_executable(directory, &manifest.command)?;
    let executable_bytes = fs::read(&executable)
        .map_err(|error| format!("could not read adapter executable: {error}"))?;
    Ok(Discovered {
        directory: directory.to_path_buf(),
        manifest_hash: sha256(&manifest_bytes),
        executable_hash: sha256(&executable_bytes),
        manifest,
        executable,
    })
}

fn resolve_executable(directory: &Path, command: &str) -> Result<PathBuf, String> {
    let requested = PathBuf::from(command);
    let path = if requested.is_absolute() {
        requested
    } else {
        directory.join(requested)
    };
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("adapter executable is unavailable: {error}"))?;
    if !PathBuf::from(command).is_absolute() {
        let canonical_directory = directory
            .canonicalize()
            .map_err(|error| format!("adapter directory is unavailable: {error}"))?;
        if !canonical.starts_with(canonical_directory) {
            return Err("relative adapter command escapes its directory".into());
        }
    }
    if !canonical.is_file() {
        return Err("adapter command is not a file".into());
    }
    Ok(canonical)
}

fn run(adapter: &Discovered) -> Result<ProviderSnapshot, ProviderFailure> {
    let mut child = Command::new(&adapter.executable)
        .args(&adapter.manifest.args)
        .current_dir(&adapter.directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            ProviderFailure::new("adapter_spawn", format!("Adapter could not start: {error}"))
        })?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.take(OUTPUT_CAP + 1).read_to_end(&mut bytes);
        bytes
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.take(STDERR_CAP + 1).read_to_end(&mut bytes);
        bytes
    });
    let started = Instant::now();
    let timeout = Duration::from_secs(adapter.manifest.timeout_seconds);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(ProviderFailure::new("adapter_timeout", "Adapter timed out"));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ProviderFailure::new(
                    "adapter_wait",
                    format!("Adapter wait failed: {error}"),
                ));
            }
        }
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    if stdout.len() as u64 > OUTPUT_CAP {
        return Err(ProviderFailure::new(
            "adapter_output_too_large",
            "Adapter output exceeded 64 KiB",
        ));
    }
    if !status.success() {
        let tail = bounded_tail(&stderr, 512);
        let message = if tail.is_empty() {
            format!("Adapter exited with {status}")
        } else {
            format!("Adapter exited with {status}: {tail}")
        };
        return Err(ProviderFailure::new("adapter_nonzero", message));
    }
    let parsed: AdapterSnapshot = serde_json::from_slice(&stdout).map_err(|_| {
        ProviderFailure::new(
            "adapter_json",
            "Adapter stdout was not exactly one JSON object",
        )
    })?;
    normalize(adapter, parsed)
}

fn normalize(
    adapter: &Discovered,
    parsed: AdapterSnapshot,
) -> Result<ProviderSnapshot, ProviderFailure> {
    if parsed.schema_version != 1 {
        return Err(ProviderFailure::new(
            "adapter_schema",
            "Adapter snapshot schema version is unsupported",
        ));
    }
    let observed_at = parsed
        .observed_at
        .as_deref()
        .map(parse_timestamp)
        .transpose()?
        .unwrap_or_else(providers::now_unix);
    if observed_at > providers::now_unix() + 300 {
        return Err(ProviderFailure::new(
            "adapter_future_time",
            "Adapter observation time is too far in the future",
        ));
    }
    let state = match parsed.status.as_str() {
        "connected" => ConnectionState::Connected,
        "waiting" => ConnectionState::Waiting,
        "disconnected" => ConnectionState::Disconnected,
        _ => {
            return Err(ProviderFailure::new(
                "adapter_status",
                "Adapter status is invalid",
            ))
        }
    };
    let windows = parsed
        .windows
        .into_iter()
        .map(|window| {
            if !valid_id(&window.id)
                || window.label.trim().is_empty()
                || !window.used_percent.is_finite()
                || !(0.0..=100.0).contains(&window.used_percent)
            {
                return Err(ProviderFailure::new(
                    "adapter_window",
                    "Adapter returned an invalid usage window",
                ));
            }
            Ok(UsageWindow {
                id: window.id,
                label: window.label,
                used_percent: window.used_percent,
                resets_at: window
                    .resets_at
                    .as_deref()
                    .map(parse_timestamp)
                    .transpose()?,
                window_minutes: window.window_minutes,
                display: match window.display.as_deref() {
                    Some("bar") => WindowDisplay::Bar,
                    Some("ring") | None => WindowDisplay::Ring,
                    Some(_) => {
                        return Err(ProviderFailure::new(
                            "adapter_window",
                            "Adapter display hint is invalid",
                        ))
                    }
                },
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let balances = parsed
        .balances
        .into_iter()
        .map(|balance| {
            if !valid_id(&balance.id)
                || balance.label.trim().is_empty()
                || balance.unit.trim().is_empty()
                || !valid_decimal(&balance.amount)
            {
                return Err(ProviderFailure::new(
                    "adapter_balance",
                    "Adapter returned an invalid balance",
                ));
            }
            Ok(Balance {
                id: balance.id,
                label: balance.label,
                amount: Some(balance.amount),
                unit: Some(balance.unit),
                known: true,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ProviderSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        id: adapter.manifest.id.clone(),
        name: adapter.manifest.name.clone(),
        accent: adapter.manifest.accent.clone(),
        state,
        status_message: match state {
            ConnectionState::Connected => "Connected",
            ConnectionState::Waiting => "Waiting for adapter data",
            ConnectionState::Disconnected => "Adapter reports disconnected",
            _ => unreachable!(),
        }
        .into(),
        observed_at: Some(observed_at),
        last_attempt_at: None,
        error_code: None,
        windows,
        balances,
        refreshing: false,
    })
}

fn parse_timestamp(value: &str) -> Result<i64, ProviderFailure> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|time| time.timestamp())
        .map_err(|_| ProviderFailure::new("adapter_time", "Adapter timestamp is invalid"))
}

fn trust_matches(trust: &AdapterTrust, adapter: &Discovered) -> bool {
    trust.manifest_sha256 == adapter.manifest_hash
        && trust.executable_sha256 == adapter.executable_hash
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn slug(value: &str) -> Option<String> {
    let mut output = String::new();
    let mut separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !output.is_empty() {
                output.push('-');
            }
            separator = false;
            output.push(character.to_ascii_lowercase());
            if output.len() >= 48 {
                break;
            }
        } else {
            separator = true;
        }
    }
    (!output.is_empty()).then_some(output)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("could not read starter permissions: {error}"))?
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("could not make starter executable: {error}"))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn valid_decimal(value: &str) -> bool {
    let mut seen_digit = false;
    let mut seen_dot = false;
    for (index, character) in value.chars().enumerate() {
        match character {
            '-' if index == 0 => {}
            '.' if !seen_dot => seen_dot = true,
            '0'..='9' => seen_digit = true,
            _ => return false,
        }
    }
    seen_digit && value.len() <= 64
}

fn bounded_tail(bytes: &[u8], count: usize) -> String {
    let start = bytes.len().saturating_sub(count);
    String::from_utf8_lossy(&bytes[start..])
        .trim()
        .replace(['\n', '\r'], " ")
}

fn adapter_waiting(
    id: &str,
    name: &str,
    state: ConnectionState,
    message: &str,
) -> ProviderSnapshot {
    let mut snapshot = ProviderSnapshot::waiting(id, name, message, None);
    snapshot.state = state;
    snapshot
}

fn action_error(code: &str, message: impl Into<String>) -> ActionResult {
    ActionResult {
        ok: false,
        code: code.into(),
        message: message.into(),
    }
}

fn default_interval() -> u64 {
    300
}

fn default_timeout() -> u64 {
    10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_adapter_ids() {
        assert!(valid_id("deepseek-2"));
        assert!(!valid_id("DeepSeek"));
        assert!(!valid_id("../escape"));
    }

    #[test]
    fn validates_decimal_strings_without_inventing_numbers() {
        assert!(valid_decimal("0"));
        assert!(valid_decimal("13.72"));
        assert!(!valid_decimal("NaN"));
        assert!(!valid_decimal("$0"));
    }

    #[test]
    fn creates_safe_custom_adapter_slugs() {
        assert_eq!(slug("Gemini Pro").as_deref(), Some("gemini-pro"));
        assert_eq!(slug("  A / B  ").as_deref(), Some("a-b"));
        assert_eq!(slug("---"), None);
    }
}
