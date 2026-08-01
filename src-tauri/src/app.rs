use std::io;

use tauri::{Manager, WindowEvent};

use crate::{
    adapters, commands, model::ClaudeCaptureState, native_widget, providers,
    settings::SettingsStore, tray, window,
};

pub fn run() {
    let app = tauri::Builder::default()
        .on_page_load(|webview, _payload| {
            if webview.label() == window::WIDGET_LABEL {
                if let Err(error) = window::reassert_current_policy(webview.app_handle()) {
                    eprintln!("Agent Gauge could not reassert loaded window policy: {error}");
                }
            }
        })
        .setup(|app| {
            app.manage(SettingsStore::load());
            auto_connect_claude(app.handle());
            adapters::ensure_sample();
            adapters::ensure_sample_disabled(app.handle());
            app.manage(providers::ProviderStore::load());
            let managed =
                window::ManagedWindowState::load(app.handle()).map_err(io::Error::other)?;
            app.manage(managed);

            let widget = app
                .get_webview_window(window::WIDGET_LABEL)
                .ok_or_else(|| io::Error::other("widget window was not created"))?;

            native_widget::install(app.handle(), &widget).map_err(io::Error::other)?;
            window::initialize_widget(app.handle()).map_err(io::Error::other)?;
            tray::create(app.handle())?;

            let settings_window = app
                .get_webview_window(window::SETTINGS_LABEL)
                .ok_or_else(|| io::Error::other("settings window was not created"))?;

            let app_handle = app.handle().clone();
            widget.on_window_event(move |event| match event {
                WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
                    window::capture_geometry(&app_handle);
                }
                WindowEvent::ScaleFactorChanged { .. } => {
                    window::capture_geometry(&app_handle);
                    window::persist_soon(&app_handle);
                }
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    let _ = window::set_widget_visible(&app_handle, false);
                }
                WindowEvent::Destroyed => {
                    window::persist_now(&app_handle);
                }
                _ => {}
            });

            let settings_handle = app.handle().clone();
            settings_window.on_window_event(move |event| {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window::close_settings(&settings_handle);
                }
            });

            providers::start(app.handle());
            if !app.state::<SettingsStore>().snapshot().onboarding_complete {
                window::open_settings(app.handle()).map_err(io::Error::other)?;
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            window::get_widget_state,
            window::begin_drag,
            window::begin_resize,
            commands::get_app_state,
            commands::apply_settings,
            commands::set_display_mode,
            commands::toggle_layout_lock,
            commands::set_widget_visible,
            commands::open_settings,
            commands::close_settings,
            commands::refresh_provider,
            commands::install_claude_capture,
            commands::remove_claude_capture,
            commands::set_autostart,
            commands::trust_adapter,
            commands::revoke_adapter,
            commands::test_adapter,
            commands::create_adapter_scaffold,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Agent Gauge");

    app.run(|app, event| {
        if matches!(event, tauri::RunEvent::Ready) {
            if let Err(error) = window::reassert_current_policy(app) {
                eprintln!("Agent Gauge could not reassert ready window policy: {error}");
            }
        }
    });
}

fn auto_connect_claude(app: &tauri::AppHandle) {
    let settings = app.state::<SettingsStore>().snapshot();
    if settings.claude_auto_connect_attempted {
        return;
    }

    let status = providers::read_capture_status();
    if status.state == ClaudeCaptureState::NotInstalled {
        let result = providers::install_capture();
        if !result.ok {
            eprintln!(
                "Agent Gauge could not auto-connect Claude capture: {}",
                result.message
            );
        }
    }
    if let Err(error) = app.state::<SettingsStore>().update(|settings| {
        settings.claude_auto_connect_attempted = true;
    }) {
        eprintln!("Agent Gauge could not remember the Claude connection attempt: {error}");
    }
}
