use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::window::WidgetState;

pub const SETTINGS_SCHEMA_VERSION: u32 = 1;
pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Glass,
    Cutout,
    Signal,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AdapterTrust {
    pub manifest_sha256: String,
    pub executable_sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct AppSettings {
    pub schema_version: u32,
    pub theme: Theme,
    pub refresh_interval_seconds: u64,
    pub provider_order: Vec<String>,
    pub disabled_providers: Vec<String>,
    pub adapter_trust: BTreeMap<String, AdapterTrust>,
    pub onboarding_complete: bool,
    pub claude_auto_connect_attempted: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            theme: Theme::Signal,
            refresh_interval_seconds: 300,
            provider_order: vec!["codex".into(), "claude".into()],
            disabled_providers: Vec::new(),
            adapter_trust: BTreeMap::new(),
            onboarding_complete: false,
            claude_auto_connect_attempted: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Waiting,
    Connected,
    Disconnected,
    Error,
    Disabled,
    Untrusted,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WindowDisplay {
    Ring,
    Bar,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct UsageWindow {
    pub id: String,
    pub label: String,
    pub used_percent: f64,
    pub resets_at: Option<i64>,
    pub window_minutes: Option<u64>,
    pub display: WindowDisplay,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Balance {
    pub id: String,
    pub label: String,
    pub amount: Option<String>,
    pub unit: Option<String>,
    pub known: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ProviderSnapshot {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub accent: Option<String>,
    pub state: ConnectionState,
    pub status_message: String,
    pub observed_at: Option<i64>,
    pub last_attempt_at: Option<i64>,
    pub error_code: Option<String>,
    pub windows: Vec<UsageWindow>,
    pub balances: Vec<Balance>,
    pub refreshing: bool,
}

impl ProviderSnapshot {
    pub fn waiting(id: &str, name: &str, message: &str, accent: Option<&str>) -> Self {
        Self {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            id: id.into(),
            name: name.into(),
            accent: accent.map(Into::into),
            state: ConnectionState::Waiting,
            status_message: message.into(),
            observed_at: None,
            last_attempt_at: None,
            error_code: None,
            windows: Vec::new(),
            balances: Vec::new(),
            refreshing: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AdapterInfo {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub refresh_interval_seconds: u64,
    pub trusted: bool,
    pub trust_changed: bool,
    pub valid: bool,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeCaptureState {
    NotInstalled,
    Installed,
    Conflict,
    SettingsInvalid,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ClaudeCaptureStatus {
    pub state: ClaudeCaptureState,
    pub message: String,
    pub settings_path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DiagnosticPaths {
    pub config: String,
    pub cache: String,
    pub state: String,
    pub adapters: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AppAggregate {
    pub schema_version: u32,
    pub surface: String,
    pub app_version: String,
    pub settings: AppSettings,
    pub window: WidgetState,
    pub providers: Vec<ProviderSnapshot>,
    pub adapters: Vec<AdapterInfo>,
    pub claude_capture: ClaudeCaptureStatus,
    pub autostart_enabled: bool,
    pub paths: DiagnosticPaths,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ActionResult {
    pub ok: bool,
    pub code: String,
    pub message: String,
}
