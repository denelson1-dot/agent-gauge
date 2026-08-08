use std::collections::HashSet;

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, Window};

use crate::{
    adapters,
    model::{ActionResult, AppAggregate, DiagnosticPaths, Theme, SETTINGS_SCHEMA_VERSION},
    paths,
    platform::autostart,
    providers,
    settings::SettingsStore,
    window::{self, DisplayMode, ManagedWindowState},
};

#[derive(Debug, Deserialize)]
pub struct SettingsPatch {
    theme: Option<Theme>,
    refresh_interval_seconds: Option<u64>,
    provider_order: Option<Vec<String>>,
    disabled_providers: Option<Vec<String>>,
    onboarding_complete: Option<bool>,
}

#[tauri::command]
pub fn get_app_state(window: Window, app: AppHandle) -> AppAggregate {
    aggregate(&app, window.label())
}

#[tauri::command]
pub fn apply_settings(app: AppHandle, patch: SettingsPatch) -> Result<ActionResult, String> {
    if let Some(order) = patch.provider_order.as_ref() {
        let unique: HashSet<_> = order.iter().collect();
        if unique.len() != order.len() || order.iter().any(|id| !valid_provider_id(id)) {
            return Ok(action(
                false,
                "settings_invalid",
                "Provider order is invalid",
            ));
        }
    }
    if let Some(disabled) = patch.disabled_providers.as_ref() {
        if disabled.iter().any(|id| !valid_provider_id(id)) {
            return Ok(action(
                false,
                "settings_invalid",
                "Disabled provider list is invalid",
            ));
        }
    }
    let settings = app.state::<SettingsStore>().update(|settings| {
        if let Some(theme) = patch.theme {
            settings.theme = theme;
        }
        if let Some(interval) = patch.refresh_interval_seconds {
            settings.refresh_interval_seconds = interval;
        }
        if let Some(order) = patch.provider_order {
            settings.provider_order = order;
        }
        if let Some(disabled) = patch.disabled_providers {
            settings.disabled_providers = disabled;
        }
        if let Some(complete) = patch.onboarding_complete {
            settings.onboarding_complete = complete;
        }
    })?;
    app.emit("settings-changed", &settings)
        .map_err(|error| error.to_string())?;
    crate::native_widget::redraw(&app);
    providers::refresh_all(&app);
    Ok(action(true, "settings_saved", "Settings saved"))
}

#[tauri::command]
pub fn set_display_mode(app: AppHandle, mode: DisplayMode) -> Result<ActionResult, String> {
    window::set_display_mode(&app, mode)?;
    Ok(action(true, "mode_changed", "Display mode changed"))
}

#[tauri::command]
pub fn toggle_layout_lock(app: AppHandle) -> Result<ActionResult, String> {
    window::toggle_layout_lock(&app)?;
    Ok(action(true, "layout_lock_changed", "Layout lock changed"))
}

#[tauri::command]
pub fn set_widget_visible(app: AppHandle, visible: bool) -> Result<ActionResult, String> {
    window::set_widget_visible(&app, visible)?;
    Ok(action(
        true,
        "visibility_changed",
        "Widget visibility changed",
    ))
}

#[tauri::command]
pub fn open_settings(app: AppHandle) -> Result<ActionResult, String> {
    window::open_settings(&app)?;
    Ok(action(true, "settings_opened", "Settings opened"))
}

#[tauri::command]
pub fn close_settings(app: AppHandle) -> Result<ActionResult, String> {
    window::close_settings(&app)?;
    Ok(action(true, "settings_closed", "Settings closed"))
}

#[tauri::command]
pub fn refresh_provider(app: AppHandle, provider_id: Option<String>) -> ActionResult {
    if let Some(provider_id) = provider_id {
        providers::refresh_provider(&app, &provider_id);
    } else {
        providers::refresh_all(&app);
    }
    action(true, "refresh_started", "Refresh started")
}

#[tauri::command]
pub fn install_claude_capture(app: AppHandle) -> ActionResult {
    let result = providers::install_capture();
    if result.ok {
        providers::refresh_provider(&app, "claude");
    }
    result
}

#[tauri::command]
pub fn remove_claude_capture(app: AppHandle) -> ActionResult {
    let result = providers::remove_capture();
    if result.ok {
        providers::refresh_provider(&app, "claude");
    }
    result
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> ActionResult {
    let result = autostart::set(enabled);
    if result.ok {
        let _ = app.state::<SettingsStore>().update(|settings| {
            settings.onboarding_complete = true;
        });
    }
    result
}

#[tauri::command]
pub fn trust_adapter(app: AppHandle, adapter_id: String) -> ActionResult {
    adapters::trust(&app, &adapter_id)
}

#[tauri::command]
pub fn revoke_adapter(app: AppHandle, adapter_id: String) -> ActionResult {
    adapters::revoke(&app, &adapter_id)
}

#[tauri::command]
pub fn test_adapter(app: AppHandle, adapter_id: String) -> ActionResult {
    adapters::test(&app, &adapter_id)
}

#[tauri::command]
pub fn create_adapter_scaffold(app: AppHandle, name: String) -> ActionResult {
    adapters::create_scaffold(&app, &name)
}

pub fn aggregate(app: &AppHandle, surface: &str) -> AppAggregate {
    let settings = app.state::<SettingsStore>().snapshot();
    let mut providers = app.state::<providers::ProviderStore>().snapshots();
    providers.sort_by_key(|provider| {
        settings
            .provider_order
            .iter()
            .position(|id| id == &provider.id)
            .unwrap_or(usize::MAX)
    });
    let window = app.state::<ManagedWindowState>().snapshot();

    // The widget surface renders from this rather than from `providers`, so
    // that the React and Cairo painters cannot disagree about what the numbers
    // mean. `providers` remains for the settings surface, which needs the raw
    // connection detail.
    let widget_view = crate::render::widget_view(
        settings.theme,
        &settings.provider_order,
        &settings.disabled_providers,
        providers.clone(),
        &window,
        providers::now_unix(),
    );

    AppAggregate {
        schema_version: SETTINGS_SCHEMA_VERSION,
        surface: surface.into(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        settings,
        window,
        providers,
        widget_view,
        adapters: adapters::list(app),
        claude_capture: providers::read_capture_status(),
        autostart_enabled: autostart::enabled(),
        paths: DiagnosticPaths {
            config: paths::config_dir().display().to_string(),
            cache: paths::cache_dir().display().to_string(),
            state: paths::state_dir().display().to_string(),
            adapters: paths::adapters_dir().display().to_string(),
        },
    }
}

fn valid_provider_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn action(ok: bool, code: &str, message: &str) -> ActionResult {
    ActionResult {
        ok,
        code: code.into(),
        message: message.into(),
    }
}
