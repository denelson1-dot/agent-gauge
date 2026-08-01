use std::{fs, path::Path};

use crate::{model::ActionResult, paths};

pub fn enabled() -> bool {
    paths::autostart_path().is_file()
}

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

fn set_inner(enabled: bool) -> Result<(), String> {
    let path = paths::autostart_path();
    if !enabled {
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|error| format!("could not remove {}: {error}", path.display()))?;
        }
        return Ok(());
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not locate Agent Gauge: {error}"))?;
    let entry = format!(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Agent Gauge\nComment=Glanceable AI-agent usage\nExec={}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        desktop_quote(&executable)
    );
    paths::ensure_parent(&path)?;
    let temporary = path.with_extension("desktop.tmp");
    fs::write(&temporary, entry)
        .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, &path)
        .map_err(|error| format!("could not replace {}: {error}", path.display()))
}

fn desktop_quote(path: &Path) -> String {
    let escaped = path
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_exec_path_is_safely_quoted() {
        assert_eq!(
            desktop_quote(Path::new("/tmp/a b/$app")),
            "\"/tmp/a b/\\$app\""
        );
    }
}
