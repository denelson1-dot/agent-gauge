use std::{
    env, fs,
    path::{Path, PathBuf},
};

const APP_DIR: &str = "agent-gauge";

pub fn config_dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config").join(APP_DIR)
}

pub fn cache_dir() -> PathBuf {
    xdg_dir("XDG_CACHE_HOME", ".cache").join(APP_DIR)
}

pub fn state_dir() -> PathBuf {
    xdg_dir("XDG_STATE_HOME", ".local/state").join(APP_DIR)
}

pub fn adapters_dir() -> PathBuf {
    config_dir().join("adapters")
}

pub fn claude_settings_path() -> PathBuf {
    home_dir().join(".claude/settings.json")
}

pub fn claude_dispatcher_path() -> PathBuf {
    config_dir().join("claude-status-dispatcher.py")
}

pub fn autostart_path() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config")
        .join("autostart")
        .join("io.theforge.agent-gauge.desktop")
}

pub fn ensure_parent(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("could not create {}: {error}", parent.display()))
}

fn xdg_dir(variable: &str, fallback: &str) -> PathBuf {
    env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(fallback))
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
