mod adapters;
mod app;
mod commands;
mod model;
mod native_widget;
mod paths;
mod platform;
mod providers;
mod render;
mod settings;
mod tray;
mod window;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Resolved before anything reads or writes a file, including the capture
    // path below, which writes to the cache directory. An environment we cannot
    // make sense of is reported and declined rather than guessed at.
    if let Err(error) = platform::dirs::init() {
        eprintln!("Agent Gauge could not determine where to store its files: {error}");
        return;
    }

    // Claude Code invokes the executable this way on every status-line update,
    // many times a minute. It must stay a cheap, headless read-and-exit: no
    // window, no lock, no GUI initialisation.
    if std::env::args().any(|argument| argument == "--capture-claude") {
        if let Err(error) = providers::capture_status_line_stdin() {
            eprintln!("Agent Gauge Claude capture failed: {error}");
        }
        return;
    }

    platform::pre_init();

    let _instance = match platform::instance::acquire() {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("Agent Gauge is already running or could not acquire its lock: {error}");
            return;
        }
    };
    app::run();
}
