//! Where Agent Gauge keeps its files.
//!
//! The base directories are platform-specific and resolved in
//! `platform::dirs`; this module re-exports them and derives the individual
//! paths built on top. Keeping the derived paths here means the rest of the
//! crate asks for a *purpose* ("the adapters directory") rather than assembling
//! a layout, and only one file changes if a layout ever moves.

use std::path::PathBuf;

pub use crate::platform::dirs::{
    cache_dir, config_base, config_dir, ensure_parent, home_dir, state_dir,
};

pub fn adapters_dir() -> PathBuf {
    config_dir().join("adapters")
}

/// Claude Code's own settings file.
///
/// Anchored to the home directory rather than to `config_dir` because Claude
/// Code uses `~/.claude` on every platform, including Windows, where Agent
/// Gauge's own configuration lives under `%APPDATA%` instead.
pub fn claude_settings_path() -> PathBuf {
    home_dir().join(".claude").join("settings.json")
}

/// Claude Code's OAuth credentials.
///
/// Read for one thing only: the access token used to query the usage endpoint
/// when no terminal session is feeding the status-line capture. Agent Gauge
/// never writes this file. See `providers::claude_usage`.
pub fn claude_credentials_path() -> PathBuf {
    home_dir().join(".claude").join(".credentials.json")
}

/// The Claude desktop app's own record of plan usage.
///
/// The desktop app polls the same usage endpoint Agent Gauge does and leaves
/// the answers here, so this is a reading that costs no request and needs no
/// credentials of our own. Read-only, and never written: it belongs to another
/// application. See `providers::claude_desktop`.
///
/// The desktop app is Electron, so its data lives at Electron's `userData` —
/// `$XDG_CONFIG_HOME/Claude` on Linux and `%APPDATA%\Claude` on Windows, which
/// is the same root Agent Gauge's own `config_dir` is built on.
pub fn claude_desktop_history_path() -> PathBuf {
    config_base().join("Claude").join("plan-usage-history.json")
}

/// The generated Python status-line dispatcher.
///
/// Retained only so existing installations can be migrated off it; nothing
/// writes this file any more. See `providers::claude`.
pub fn legacy_claude_dispatcher_path() -> PathBuf {
    config_dir().join("claude-status-dispatcher.py")
}
