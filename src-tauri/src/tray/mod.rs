use tauri::{menu::MenuBuilder, tray::TrayIconBuilder, AppHandle};

use crate::{
    platform::autostart,
    providers,
    window::{self, DisplayMode},
};

const SHOW_HIDE: &str = "show-hide";
const OPEN_SETTINGS: &str = "open-settings";
const REFRESH_NOW: &str = "refresh-now";
const DESKTOP_MODE: &str = "desktop-mode";
const PINNED_MODE: &str = "pinned-mode";
const LOCK_UNLOCK: &str = "lock-unlock";
const RESET_GEOMETRY: &str = "reset-geometry";
const AUTOSTART: &str = "autostart";
const QUIT: &str = "quit";

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let menu = MenuBuilder::new(app)
        .text(OPEN_SETTINGS, "Settings…")
        .text(REFRESH_NOW, "Refresh Now")
        .separator()
        .text(SHOW_HIDE, "Show / Hide Widget")
        .separator()
        .text(DESKTOP_MODE, "Desktop Mode")
        .text(PINNED_MODE, "Pinned Mode")
        .text(LOCK_UNLOCK, "Lock / Unlock Layout")
        .text(RESET_GEOMETRY, "Reset Geometry")
        .separator()
        .text(AUTOSTART, "Toggle Start at Login")
        .separator()
        .text(QUIT, "Quit Agent Gauge")
        .build()?;

    let mut builder = TrayIconBuilder::with_id("agent-gauge")
        .menu(&menu)
        // The tray menu is the only way to reach Agent Gauge when the widget is
        // hidden or locked, so it has to open on the click people actually try.
        // Windows convention is a left click; the default only opens on right.
        .show_menu_on_left_click(true)
        .tooltip("Agent Gauge")
        .on_menu_event(|app, event| {
            let result = match event.id().as_ref() {
                OPEN_SETTINGS => window::open_settings(app),
                REFRESH_NOW => {
                    providers::refresh_all(app);
                    Ok(())
                }
                SHOW_HIDE => window::toggle_widget_visible(app),
                DESKTOP_MODE => window::set_display_mode(app, DisplayMode::Desktop),
                PINNED_MODE => window::set_display_mode(app, DisplayMode::Pinned),
                LOCK_UNLOCK => window::toggle_layout_lock(app),
                RESET_GEOMETRY => window::reset_geometry(app),
                AUTOSTART => {
                    let result = autostart::set(!autostart::enabled());
                    if result.ok {
                        Ok(())
                    } else {
                        Err(result.message)
                    }
                }
                QUIT => {
                    window::capture_geometry(app);
                    window::persist_now(app);
                    app.exit(0);
                    Ok(())
                }
                _ => Ok(()),
            };

            if let Err(error) = result {
                eprintln!("Agent Gauge tray action failed: {error}");
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}
