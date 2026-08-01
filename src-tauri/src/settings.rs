use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    model::{AppSettings, SETTINGS_SCHEMA_VERSION},
    paths,
};

const SETTINGS_FILE: &str = "settings.json";

pub struct SettingsStore {
    value: Mutex<AppSettings>,
    path: PathBuf,
}

impl SettingsStore {
    pub fn load() -> Self {
        let path = paths::config_dir().join(SETTINGS_FILE);
        let value = load_json(&path).unwrap_or_default();
        Self {
            value: Mutex::new(value),
            path,
        }
    }

    pub fn snapshot(&self) -> AppSettings {
        self.value.lock().expect("settings mutex poisoned").clone()
    }

    pub fn update(&self, update: impl FnOnce(&mut AppSettings)) -> Result<AppSettings, String> {
        let snapshot = {
            let mut settings = self.value.lock().expect("settings mutex poisoned");
            update(&mut settings);
            settings.schema_version = SETTINGS_SCHEMA_VERSION;
            settings.refresh_interval_seconds = settings.refresh_interval_seconds.clamp(60, 3600);
            settings.clone()
        };
        atomic_write_json(&self.path, &snapshot)?;
        Ok(snapshot)
    }
}

pub fn atomic_write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    paths::ensure_parent(path)?;
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("could not serialize {}: {error}", path.display()))?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| format!("could not create {}: {error}", temporary.display()))?;
    file.write_all(&bytes)
        .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
    file.sync_all()
        .map_err(|error| format!("could not sync {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path)
        .map_err(|error| format!("could not replace {}: {error}", path.display()))
}

fn load_json(path: &Path) -> Option<AppSettings> {
    let bytes = fs::read(path).ok()?;
    match serde_json::from_slice::<AppSettings>(&bytes) {
        Ok(settings) if settings.schema_version == SETTINGS_SCHEMA_VERSION => Some(settings),
        Ok(_) | Err(_) => {
            quarantine(path);
            None
        }
    }
}

fn quarantine(path: &Path) {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let quarantine = path.with_extension(format!("corrupt-{timestamp}.json"));
    if let Err(error) = fs::rename(path, &quarantine) {
        eprintln!("Agent Gauge could not quarantine settings: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_refresh_interval_is_safe() {
        let settings = AppSettings::default();
        assert_eq!(settings.refresh_interval_seconds, 300);
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
    }
}
