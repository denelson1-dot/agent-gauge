//! Base directory resolution.
//!
//! Linux follows the XDG basedir spec. Windows follows the standard
//! `%APPDATA%` / `%LOCALAPPDATA%` split: roaming for things a user would want
//! to keep (settings, adapters, window placement), local for things that can be
//! regenerated (provider cache, instance lock).
//!
//! `home_dir` is deliberately separate from `config_dir`. Agent Gauge's own
//! state belongs in the platform's application-data location, but Claude Code
//! keeps its settings under the user's home directory on *every* platform, so
//! that path must be resolved from the home directory rather than from
//! `config_dir`.
//!
//! Resolution happens once, up front, via [`init`]. The previous implementation
//! silently fell back to `PathBuf::from(".")` when `HOME` was unset, which meant
//! a misconfigured environment quietly scattered application data next to the
//! working directory instead of failing. That is exactly the kind of failure
//! that is invisible on a developer's machine and mystifying on someone else's,
//! so an unresolvable environment is now a hard, explained error.

use std::{
    env,
    path::{Path, PathBuf},
    sync::OnceLock,
};

const APP_DIR: &str = "agent-gauge";

struct Base {
    config: PathBuf,
    cache: PathBuf,
    state: PathBuf,
    home: PathBuf,
}

static BASE: OnceLock<Base> = OnceLock::new();

/// Resolves and caches the base directories.
///
/// Call once during startup, before anything touches a path. Returning `Err`
/// means the environment cannot tell us where the user's data lives; the caller
/// should report the message and decline to start rather than guess.
pub fn init() -> Result<(), String> {
    let base = resolve()?;
    let _ = BASE.set(base);
    Ok(())
}

pub fn config_dir() -> PathBuf {
    base().config.clone()
}

pub fn cache_dir() -> PathBuf {
    base().cache.clone()
}

pub fn state_dir() -> PathBuf {
    base().state.clone()
}

pub fn home_dir() -> PathBuf {
    base().home.clone()
}

fn base() -> &'static Base {
    BASE.get_or_init(|| {
        resolve().expect("paths are resolved and validated by platform::dirs::init at startup")
    })
}

/// Reads an environment variable that must name a directory, rejecting the
/// empty string so that `VAR=` behaves the same as `VAR` being unset.
fn env_dir(variable: &str) -> Option<PathBuf> {
    env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(target_os = "linux")]
fn resolve() -> Result<Base, String> {
    let home = env_dir("HOME").ok_or_else(|| {
        "HOME is not set, so Agent Gauge cannot tell where your home directory is".to_string()
    })?;

    Ok(Base {
        config: xdg_dir("XDG_CONFIG_HOME", &home, ".config").join(APP_DIR),
        cache: xdg_dir("XDG_CACHE_HOME", &home, ".cache").join(APP_DIR),
        state: xdg_dir("XDG_STATE_HOME", &home, ".local/state").join(APP_DIR),
        home,
    })
}

#[cfg(target_os = "linux")]
fn xdg_dir(variable: &str, home: &Path, fallback: &str) -> PathBuf {
    env_dir(variable).unwrap_or_else(|| home.join(fallback))
}

#[cfg(target_os = "windows")]
fn resolve() -> Result<Base, String> {
    let home = env_dir("USERPROFILE").ok_or_else(|| {
        "USERPROFILE is not set, so Agent Gauge cannot tell where your user profile is".to_string()
    })?;

    // Fall back to the conventional layout under the profile rather than
    // failing outright: a stripped environment (a service account, a shell
    // launched without the usual variables) still has a correct answer here.
    let roaming = env_dir("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming"));
    let local = env_dir("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData").join("Local"));
    let local = local.join(APP_DIR);

    Ok(Base {
        config: roaming.join(APP_DIR),
        cache: local.join("cache"),
        state: local.join("state"),
        home,
    })
}

/// The parent-directory helper used before writing any file we own.
pub fn ensure_parent(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("could not create {}: {error}", parent.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_a_usable_layout_on_this_platform() {
        let base = resolve().expect("the test environment should provide a home directory");
        assert!(base.config.is_absolute());
        assert!(base.cache.is_absolute());
        assert!(base.state.is_absolute());
        assert!(base.home.is_absolute());
    }

    #[test]
    fn separates_agent_gauge_state_from_the_home_directory() {
        // Claude Code's settings live under `home`, ours do not; conflating the
        // two is the mistake this guards against.
        let base = resolve().expect("the test environment should provide a home directory");
        assert_ne!(base.config, base.home);
        assert!(base.config.ends_with(APP_DIR));
    }

    #[test]
    fn empty_environment_values_are_treated_as_unset() {
        // SAFETY-adjacent: this mutates process environment, so keep it to a
        // variable nothing else in the suite reads.
        let name = "AGENT_GAUGE_DIRS_EMPTY_PROBE";
        env::set_var(name, "");
        assert_eq!(env_dir(name), None);
        env::set_var(name, "/tmp");
        assert_eq!(env_dir(name), Some(PathBuf::from("/tmp")));
        env::remove_var(name);
    }
}
