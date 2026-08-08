//! Start-at-login.
//!
//! Linux writes a freedesktop autostart entry; Windows sets a value under the
//! per-user `Run` key. Both are per-user and require no elevation, which
//! matters because Agent Gauge should never ask for administrator rights.
//!
//! The `enabled`/`set` contract is shared so `commands.rs` and the settings UI
//! stay platform-agnostic.

use crate::model::ActionResult;

pub fn set(enabled: bool) -> ActionResult {
    match set_inner(enabled) {
        Ok(()) => ActionResult {
            ok: true,
            code: if enabled {
                "autostart_enabled"
            } else {
                "autostart_disabled"
            }
            .into(),
            message: if enabled {
                "Agent Gauge will start when you sign in"
            } else {
                "Agent Gauge will not start automatically"
            }
            .into(),
        },
        Err(message) => ActionResult {
            ok: false,
            code: "autostart_failed".into(),
            message,
        },
    }
}

fn executable() -> Result<std::path::PathBuf, String> {
    std::env::current_exe().map_err(|error| format!("could not locate Agent Gauge: {error}"))
}

// ---------------------------------------------------------------- Linux

#[cfg(target_os = "linux")]
pub fn enabled() -> bool {
    autostart_path().is_file()
}

#[cfg(target_os = "linux")]
fn autostart_path() -> std::path::PathBuf {
    // Deliberately not under our own config directory: the desktop environment
    // only scans the XDG autostart location.
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| crate::platform::dirs::home_dir().join(".config"));
    base.join("autostart")
        .join("io.theforge.agent-gauge.desktop")
}

#[cfg(target_os = "linux")]
fn set_inner(enabled: bool) -> Result<(), String> {
    use std::fs;

    let path = autostart_path();
    if !enabled {
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|error| format!("could not remove {}: {error}", path.display()))?;
        }
        return Ok(());
    }

    let entry = format!(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Agent Gauge\nComment=Glanceable AI-agent usage\nExec={}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        desktop_quote(&executable()?)
    );
    crate::platform::dirs::ensure_parent(&path)?;
    let temporary = path.with_extension("desktop.tmp");
    fs::write(&temporary, entry)
        .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, &path)
        .map_err(|error| format!("could not replace {}: {error}", path.display()))
}

/// `Exec=` is parsed with a quoting scheme defined by the desktop entry spec,
/// which is close to but not the same as shell quoting.
#[cfg(target_os = "linux")]
fn desktop_quote(path: &std::path::Path) -> String {
    let escaped = path
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$");
    format!("\"{escaped}\"")
}

// -------------------------------------------------------------- Windows

#[cfg(target_os = "windows")]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(target_os = "windows")]
const RUN_VALUE: &str = "Agent Gauge";

#[cfg(target_os = "windows")]
pub fn enabled() -> bool {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};

    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(RUN_KEY)
        .and_then(|key| key.get_value::<String, _>(RUN_VALUE))
        .is_ok()
}

#[cfg(target_os = "windows")]
fn set_inner(enabled: bool) -> Result<(), String> {
    use winreg::{
        enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE},
        RegKey,
    };

    let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey_with_flags(RUN_KEY, KEY_READ | KEY_WRITE)
        .map_err(|error| format!("could not open the Windows startup settings: {error}"))?;

    if !enabled {
        return match key.delete_value(RUN_VALUE) {
            Ok(()) => Ok(()),
            // Already absent is the desired end state, not a failure.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("could not clear the startup entry: {error}")),
        };
    }

    key.set_value(RUN_VALUE, &run_value(&executable()?))
        .map_err(|error| format!("could not write the startup entry: {error}"))
}

/// The `Run` value is parsed like a command line, so a path containing spaces
/// (the default `C:\Program Files\...` install does) must be quoted or Windows
/// launches the wrong thing.
#[cfg(target_os = "windows")]
fn run_value(path: &std::path::Path) -> String {
    format!("\"{}\"", path.display())
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn desktop_exec_path_is_safely_quoted() {
        use super::desktop_quote;
        use std::path::Path;

        assert_eq!(
            desktop_quote(Path::new("/tmp/a b/$app")),
            "\"/tmp/a b/\\$app\""
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn run_value_quotes_program_files_paths() {
        use super::run_value;
        use std::path::Path;

        assert_eq!(
            run_value(Path::new(r"C:\Program Files\Agent Gauge\agent-gauge.exe")),
            r#""C:\Program Files\Agent Gauge\agent-gauge.exe""#
        );
    }
}
