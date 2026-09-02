use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::window::WidgetState;

pub const SETTINGS_SCHEMA_VERSION: u32 = 1;
pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// Ten minutes: six requests an hour at most, and only while no terminal
/// session is running. A five-hour window moves about a third of a percent
/// per minute at a sustained pace, so a reading this old is still the right
/// number to make a decision on.
pub const DEFAULT_CLAUDE_USAGE_POLL_SECONDS: u64 = 600;

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
    /// The shortest gap between two reads of Claude's usage endpoint, used
    /// only while no terminal session is feeding the status-line capture.
    pub claude_usage_poll_seconds: u64,
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
            claude_usage_poll_seconds: DEFAULT_CLAUDE_USAGE_POLL_SECONDS,
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

impl UsageWindow {
    /// A window whose reported reset time has passed has rolled over. The
    /// captured percent describes the window that ended, so it no longer
    /// describes current usage even when the provider has gone quiet.
    pub fn rolled_over(&self, now: i64) -> bool {
        self.resets_at.is_some_and(|resets_at| now >= resets_at)
    }

    /// Usage to present. A rolled-over window starts empty; that is derived
    /// from the provider's own reset time rather than invented.
    pub fn effective_used_percent(&self, now: i64) -> f64 {
        if self.rolled_over(now) {
            0.0
        } else {
            self.used_percent
        }
    }
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
    /// Raw provider state, used by the settings surface.
    pub providers: Vec<ProviderSnapshot>,
    /// What the widget should display. Derived once in `render` so that both
    /// painters — Cairo on Linux, React on Windows — show the same thing.
    pub widget_view: crate::render::WidgetView,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn window(resets_at: Option<i64>) -> UsageWindow {
        UsageWindow {
            id: "five-hour".into(),
            label: "5 hour".into(),
            used_percent: 87.0,
            resets_at,
            window_minutes: Some(300),
            display: WindowDisplay::Ring,
        }
    }

    #[test]
    fn live_window_reports_captured_usage() {
        let window = window(Some(2_000));
        assert!(!window.rolled_over(1_000));
        assert_eq!(window.effective_used_percent(1_000), 87.0);
    }

    #[test]
    fn expired_window_reports_empty_usage_and_unknown_reset() {
        let window = window(Some(2_000));
        assert!(window.rolled_over(2_000));
        assert_eq!(window.effective_used_percent(2_000), 0.0);
    }

    #[test]
    fn window_without_a_reset_time_keeps_captured_usage() {
        let window = window(None);
        assert!(!window.rolled_over(9_999));
        assert_eq!(window.effective_used_percent(9_999), 87.0);
    }
}
