mod claude;
mod claude_usage;
mod codex;

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::PathBuf,
    sync::Mutex,
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::{
    model::{ConnectionState, ProviderSnapshot, SNAPSHOT_SCHEMA_VERSION},
    paths,
    settings::{atomic_write_json, SettingsStore},
};

pub use claude::{
    capture_status_line_stdin, install_capture, migrate_legacy_install, read_capture_status,
    remove_capture,
};

const CACHE_FILE: &str = "snapshots.json";

#[derive(Debug, Clone)]
pub struct ProviderFailure {
    pub code: String,
    pub message: String,
    pub disconnected: bool,
}

impl ProviderFailure {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            disconnected: false,
        }
    }

    pub fn disconnected(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            disconnected: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct SnapshotCache {
    schema_version: u32,
    providers: BTreeMap<String, ProviderSnapshot>,
}

pub struct ProviderStore {
    snapshots: Mutex<BTreeMap<String, ProviderSnapshot>>,
    pub(crate) in_flight: Mutex<HashSet<String>>,
    persist_lock: Mutex<()>,
    cache_path: PathBuf,
}

impl ProviderStore {
    pub fn load() -> Self {
        let cache_path = paths::cache_dir().join(CACHE_FILE);
        let mut snapshots = fs::read(&cache_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<SnapshotCache>(&bytes).ok())
            .filter(|cache| cache.schema_version == SNAPSHOT_SCHEMA_VERSION)
            .map(|cache| cache.providers)
            .unwrap_or_default();

        snapshots.entry("codex".into()).or_insert_with(|| {
            ProviderSnapshot::waiting(
                "codex",
                "Codex",
                "Checking the local Codex CLI…",
                Some("#74a7ff"),
            )
        });
        snapshots.entry("claude".into()).or_insert_with(|| {
            ProviderSnapshot::waiting(
                "claude",
                "Claude",
                "Waiting for Claude Code activity",
                Some("#d9986a"),
            )
        });

        Self {
            snapshots: Mutex::new(snapshots),
            in_flight: Mutex::new(HashSet::new()),
            persist_lock: Mutex::new(()),
            cache_path,
        }
    }

    pub fn snapshots(&self) -> Vec<ProviderSnapshot> {
        self.snapshots
            .lock()
            .expect("provider snapshot mutex poisoned")
            .values()
            .cloned()
            .collect()
    }

    pub(crate) fn snapshot(&self, id: &str) -> Option<ProviderSnapshot> {
        self.snapshots
            .lock()
            .expect("provider snapshot mutex poisoned")
            .get(id)
            .cloned()
    }

    pub(crate) fn set(&self, snapshot: ProviderSnapshot) {
        self.snapshots
            .lock()
            .expect("provider snapshot mutex poisoned")
            .insert(snapshot.id.clone(), snapshot);
        self.persist();
    }

    fn persist(&self) {
        let _persist = self
            .persist_lock
            .lock()
            .expect("provider persistence mutex poisoned");
        let cache = SnapshotCache {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            providers: self
                .snapshots
                .lock()
                .expect("provider snapshot mutex poisoned")
                .clone(),
        };
        if let Err(error) = atomic_write_json(&self.cache_path, &cache) {
            eprintln!("Agent Gauge could not persist provider cache: {error}");
        }
    }
}

pub fn start(app: &AppHandle) {
    refresh_all(app);

    let app = app.clone();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(30));
        let interval = app
            .state::<SettingsStore>()
            .snapshot()
            .refresh_interval_seconds;
        let now = now_unix();
        let due = app
            .state::<ProviderStore>()
            .snapshots()
            .iter()
            .any(|provider| now - provider.last_attempt_at.unwrap_or(0) >= interval as i64);
        if due {
            refresh_all(&app);
        }
    });
}

pub fn refresh_all(app: &AppHandle) {
    refresh_provider(app, "codex");
    refresh_provider(app, "claude");
    crate::adapters::refresh_all(app);
}

pub fn refresh_provider(app: &AppHandle, id: &str) {
    if id != "codex" && id != "claude" {
        crate::adapters::refresh(app, id);
        return;
    }

    let disabled = app
        .state::<SettingsStore>()
        .snapshot()
        .disabled_providers
        .iter()
        .any(|disabled| disabled == id);
    if disabled {
        if let Some(mut snapshot) = app.state::<ProviderStore>().snapshot(id) {
            snapshot.state = ConnectionState::Disabled;
            snapshot.status_message = "Tracker disabled".into();
            snapshot.refreshing = false;
            app.state::<ProviderStore>().set(snapshot);
            emit(app);
        }
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

    if let Some(mut snapshot) = app.state::<ProviderStore>().snapshot(id) {
        snapshot.refreshing = true;
        app.state::<ProviderStore>().set(snapshot);
        emit(app);
    }

    let app = app.clone();
    let id = id.to_string();
    thread::spawn(move || {
        let result = match id.as_str() {
            "codex" => codex::read(),
            "claude" => claude::read(),
            _ => unreachable!(),
        };
        finish(&app, &id, result);
    });
}

pub(crate) fn finish(app: &AppHandle, id: &str, result: Result<ProviderSnapshot, ProviderFailure>) {
    let now = now_unix();
    let store = app.state::<ProviderStore>();
    match result {
        Ok(mut snapshot) => {
            snapshot.last_attempt_at = Some(now);
            snapshot.refreshing = false;
            store.set(snapshot);
        }
        Err(failure) => {
            let mut snapshot = store
                .snapshot(id)
                .unwrap_or_else(|| ProviderSnapshot::waiting(id, id, "Tracker unavailable", None));
            snapshot.state = if failure.disconnected {
                ConnectionState::Disconnected
            } else {
                ConnectionState::Error
            };
            snapshot.status_message = failure.message;
            snapshot.error_code = Some(failure.code);
            snapshot.last_attempt_at = Some(now);
            snapshot.refreshing = false;
            store.set(snapshot);
        }
    }
    store
        .in_flight
        .lock()
        .expect("provider in-flight mutex poisoned")
        .remove(id);
    emit(app);
}

pub(crate) fn emit(app: &AppHandle) {
    crate::native_widget::redraw(app);
    if let Err(error) = app.emit(
        "providers-changed",
        app.state::<ProviderStore>().snapshots(),
    ) {
        eprintln!("Agent Gauge could not emit providers: {error}");
    }
}

pub(crate) fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

/// A moment as any of the shapes a provider might report it in.
///
/// Shared because the two Claude sources disagree: the status line reports a
/// Unix integer and the usage endpoint an RFC 3339 string, for the same field
/// under the same name.
pub(crate) fn timestamp(value: &serde_json::Value) -> Option<i64> {
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
