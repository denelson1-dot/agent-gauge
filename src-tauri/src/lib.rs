mod adapters;
mod app;
mod autostart;
mod commands;
mod instance;
mod model;
mod native_widget;
mod paths;
mod providers;
mod settings;
mod tray;
mod window;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if std::env::args().any(|argument| argument == "--capture-claude") {
        if let Err(error) = providers::capture_status_line_stdin() {
            eprintln!("Agent Gauge Claude capture failed: {error}");
        }
        return;
    }

    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DMABUF_RENDERER_DISABLE_GBM").is_none()
        && std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none()
    {
        std::env::set_var("WEBKIT_DMABUF_RENDERER_DISABLE_GBM", "1");
    }

    let _instance = match instance::Guard::acquire() {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("Agent Gauge is already running or could not acquire its lock: {error}");
            return;
        }
    };
    app::run();
}
