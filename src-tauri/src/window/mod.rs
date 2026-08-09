mod geometry;

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, WebviewWindow, Window,
};
use tauri_runtime::ResizeDirection;

use geometry::{recover_geometry, MonitorBounds};

pub const WIDGET_LABEL: &str = "main";
pub const SETTINGS_LABEL: &str = "settings";
const STATE_SCHEMA_VERSION: u32 = 1;
const STATE_FILE: &str = "window.json";
const MIN_WIDTH: u32 = 360;
const MIN_HEIGHT: u32 = 380;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DisplayMode {
    Desktop,
    Pinned,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub monitor_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct WidgetState {
    pub schema_version: u32,
    pub mode: DisplayMode,
    pub locked: bool,
    pub visible: bool,
    pub geometry: Option<Geometry>,
}

impl Default for WidgetState {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            mode: DisplayMode::Desktop,
            locked: true,
            visible: true,
            geometry: None,
        }
    }
}

pub struct ManagedWindowState {
    value: Mutex<WidgetState>,
    path: PathBuf,
    save_generation: AtomicU64,
    save_worker_active: AtomicBool,
    geometry_transition_active: AtomicBool,
}

impl ManagedWindowState {
    pub fn load(app: &AppHandle) -> Result<Self, String> {
        let _ = app;
        let config_dir = crate::paths::config_dir();
        let path = config_dir.join(STATE_FILE);
        let value = load_state_file(&path);

        Ok(Self {
            value: Mutex::new(value),
            path,
            save_generation: AtomicU64::new(0),
            save_worker_active: AtomicBool::new(false),
            geometry_transition_active: AtomicBool::new(false),
        })
    }

    pub(crate) fn snapshot(&self) -> WidgetState {
        self.value
            .lock()
            .expect("window state mutex poisoned")
            .clone()
    }
}

pub fn open_settings(app: &AppHandle) -> Result<(), String> {
    let settings = app
        .get_webview_window(SETTINGS_LABEL)
        .ok_or_else(|| "settings window is unavailable".to_string())?;
    settings
        .show()
        .map_err(|error| format!("could not show settings: {error}"))?;
    settings
        .unminimize()
        .map_err(|error| format!("could not restore settings: {error}"))?;
    settings
        .set_focus()
        .map_err(|error| format!("could not focus settings: {error}"))
}

pub fn close_settings(app: &AppHandle) -> Result<(), String> {
    app.get_webview_window(SETTINGS_LABEL)
        .ok_or_else(|| "settings window is unavailable".to_string())?
        .hide()
        .map_err(|error| format!("could not hide settings: {error}"))
}

pub fn initialize_widget(app: &AppHandle) -> Result<(), String> {
    let window = widget(app)?;

    // Held across the whole sequence so the intermediate positions below are
    // not mistaken for the user moving the widget and written back to disk.
    let transition = &app.state::<ManagedWindowState>().geometry_transition_active;
    transition.store(true, Ordering::Release);

    let result = restore_geometry(app, &window)
        .and_then(|()| realize_widget(&window))
        .and_then(|()| install_map_policy(app, &window));
    if let Err(error) = result {
        transition.store(false, Ordering::Release);
        return Err(error);
    }

    let state = app.state::<ManagedWindowState>().snapshot();
    apply_window_policy(&window, &state)?;
    if state.visible {
        show_widget(&window)?;
        enforce_focus_policy(&window, state.locked)?;
    }

    // Apply the position again now the window is mapped at its real size.
    //
    // Before mapping, the window still has the default size from tauri.conf.
    // X clamps a requested position so the window stays on screen, and it does
    // that against whatever size the window has *at the time* — the default,
    // not the restored one. A right-aligned widget therefore lands short by the
    // difference between the two widths, and because the shortfall is then
    // captured and saved, it recurs on every start.
    //
    // This mirrors what the hide/show path already does, which is why showing
    // the widget again preserved its position while starting the app did not.
    match state.geometry.clone() {
        Some(geometry) if state.visible => {
            apply_geometry(&window, &geometry, state.mode)?;
            stabilize_geometry_after_show(app, geometry);
        }
        _ => transition.store(false, Ordering::Release),
    }

    crate::platform::window::start_layer_watchdog(app);

    emit_state(app);
    persist_soon(app);
    Ok(())
}

/// Reports what the widget window is actually doing, for `--diagnose-window-layer`.
pub fn diagnose_layer(app: &AppHandle) -> Result<String, String> {
    let state = app.state::<ManagedWindowState>().snapshot();
    let window = widget(app)?;
    let layer = crate::platform::window::apply_layer(&window, state.mode)?;
    Ok(format!(
        "{}\nresolved-layer={}\nlocked={}  visible={}\ngeometry={:?}",
        crate::platform::window::diagnose(&window, state.mode),
        layer.describe(),
        state.locked,
        state.visible,
        state.geometry,
    ))
}

/// Reports the monitor layout as Agent Gauge sees it, and where saved geometry
/// would be restored to.
///
/// Geometry problems are almost always a disagreement between what the desktop
/// reports and what the widget was told, and that disagreement is invisible
/// from the outside — the window simply appears in the wrong place. This prints
/// both sides so they can be compared directly.
pub fn diagnose_geometry(app: &AppHandle) -> Result<String, String> {
    let window = widget(app)?;
    let monitors = monitor_bounds(&window)?;
    let saved = app.state::<ManagedWindowState>().snapshot().geometry;

    let mut report = String::from("monitors as reported to Agent Gauge:\n");
    for monitor in &monitors {
        report.push_str(&format!(
            "  {:?} at ({}, {}) size {}x{} scale {} primary={}\n",
            monitor.name.as_deref().unwrap_or("<unnamed>"),
            monitor.x,
            monitor.y,
            monitor.width,
            monitor.height,
            monitor.scale_factor,
            monitor.is_primary,
        ));
    }

    report.push_str(&format!("saved geometry: {saved:?}\n"));
    report.push_str(&format!(
        "would restore to: {:?}\n",
        recover_geometry(saved.as_ref(), &monitors, MIN_WIDTH, MIN_HEIGHT)
    ));
    if let Ok(position) = window.outer_position() {
        report.push_str(&format!("live outer position: {position:?}\n"));
    }
    if let Ok(size) = window.inner_size() {
        report.push_str(&format!("live inner size: {size:?}"));
    }
    Ok(report)
}

pub fn reassert_current_policy(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<ManagedWindowState>().snapshot();
    let window = widget(app)?;
    apply_window_policy(&window, &state)?;
    if state.visible {
        enforce_focus_policy(&window, state.locked)?;
    }
    Ok(())
}

#[tauri::command]
pub fn get_widget_state(state: State<'_, ManagedWindowState>) -> WidgetState {
    state.snapshot()
}

#[tauri::command]
pub fn begin_drag(
    window: WebviewWindow,
    state: State<'_, ManagedWindowState>,
) -> Result<(), String> {
    if state.snapshot().locked {
        return Err("layout is locked".into());
    }

    window
        .start_dragging()
        .map_err(|error| format!("could not start dragging: {error}"))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResizeEdge {
    North,
    NorthEast,
    East,
    SouthEast,
    South,
    SouthWest,
    West,
    NorthWest,
}

impl From<ResizeEdge> for ResizeDirection {
    fn from(edge: ResizeEdge) -> Self {
        match edge {
            ResizeEdge::North => Self::North,
            ResizeEdge::NorthEast => Self::NorthEast,
            ResizeEdge::East => Self::East,
            ResizeEdge::SouthEast => Self::SouthEast,
            ResizeEdge::South => Self::South,
            ResizeEdge::SouthWest => Self::SouthWest,
            ResizeEdge::West => Self::West,
            ResizeEdge::NorthWest => Self::NorthWest,
        }
    }
}

#[tauri::command]
pub fn begin_resize(
    window: Window,
    state: State<'_, ManagedWindowState>,
    edge: ResizeEdge,
) -> Result<(), String> {
    if state.snapshot().locked {
        return Err("layout is locked".into());
    }

    window
        .start_resize_dragging(edge.into())
        .map_err(|error| format!("could not start resizing: {error}"))
}

pub fn set_display_mode(app: &AppHandle, mode: DisplayMode) -> Result<(), String> {
    capture_geometry(app);
    let managed = app.state::<ManagedWindowState>();
    {
        let mut state = managed.value.lock().expect("window state mutex poisoned");
        state.mode = mode;
    }

    let snapshot = managed.snapshot();
    apply_window_policy(&widget(app)?, &snapshot)?;
    emit_state(app);
    persist_now(app);
    Ok(())
}

pub fn toggle_layout_lock(app: &AppHandle) -> Result<(), String> {
    capture_geometry(app);
    let managed = app.state::<ManagedWindowState>();
    {
        let mut state = managed.value.lock().expect("window state mutex poisoned");
        state.locked = !state.locked;
    }

    let snapshot = managed.snapshot();
    let window = widget(app)?;
    apply_window_policy(&window, &snapshot)?;
    if snapshot.visible && !snapshot.locked {
        window
            .set_focus()
            .map_err(|error| format!("could not focus unlocked widget: {error}"))?;
    }

    emit_state(app);
    persist_now(app);
    Ok(())
}

pub fn set_widget_visible(app: &AppHandle, visible: bool) -> Result<(), String> {
    let window = widget(app)?;
    if !visible {
        capture_geometry(app);
    }

    let managed = app.state::<ManagedWindowState>();
    managed
        .value
        .lock()
        .expect("window state mutex poisoned")
        .visible = visible;
    let snapshot = managed.snapshot();

    if visible {
        app.state::<ManagedWindowState>()
            .geometry_transition_active
            .store(true, Ordering::Release);
        if let Err(error) = restore_geometry(app, &window) {
            app.state::<ManagedWindowState>()
                .geometry_transition_active
                .store(false, Ordering::Release);
            return Err(error);
        }
        apply_window_policy(&window, &snapshot)?;
        show_widget(&window)?;
        if let Some(geometry) = app.state::<ManagedWindowState>().snapshot().geometry {
            apply_geometry(&window, &geometry, snapshot.mode)?;
            stabilize_geometry_after_show(app, geometry);
        } else {
            app.state::<ManagedWindowState>()
                .geometry_transition_active
                .store(false, Ordering::Release);
        }
        enforce_focus_policy(&window, snapshot.locked)?;
    } else {
        hide_widget(&window)?;
    }

    emit_state(app);
    persist_now(app);
    Ok(())
}

pub fn toggle_widget_visible(app: &AppHandle) -> Result<(), String> {
    let visible = app.state::<ManagedWindowState>().snapshot().visible;
    set_widget_visible(app, !visible)
}

pub fn reset_geometry(app: &AppHandle) -> Result<(), String> {
    {
        let managed = app.state::<ManagedWindowState>();
        managed
            .value
            .lock()
            .expect("window state mutex poisoned")
            .geometry = None;
    }

    let window = widget(app)?;
    restore_geometry(app, &window)?;
    emit_state(app);
    persist_now(app);
    Ok(())
}

pub fn capture_geometry(app: &AppHandle) {
    let managed = app.state::<ManagedWindowState>();
    let snapshot = managed.snapshot();
    if !snapshot.visible || managed.geometry_transition_active.load(Ordering::Acquire) {
        return;
    }
    let Ok(window) = widget(app) else {
        return;
    };
    let Ok(position) = window.outer_position() else {
        return;
    };
    let Ok(size) = window.inner_size() else {
        return;
    };

    // Converted back into screen coordinates, the space geometry is stored and
    // recovered in.
    let (position_x, position_y) =
        crate::platform::window::to_screen(snapshot.mode, position.x, position.y);

    let monitor = window.current_monitor().ok().flatten();
    let scale_factor = monitor
        .as_ref()
        .map(|monitor| monitor.scale_factor())
        .or_else(|| window.scale_factor().ok())
        .unwrap_or(1.0);
    let monitor_name = monitor
        .as_ref()
        .and_then(|monitor| monitor.name().map(ToOwned::to_owned));

    managed
        .value
        .lock()
        .expect("window state mutex poisoned")
        .geometry = Some(Geometry {
        x: position_x,
        y: position_y,
        width: size.width,
        height: size.height,
        scale_factor,
        monitor_name,
    });

    persist_soon(app);
}

pub fn persist_soon(app: &AppHandle) {
    let managed = app.state::<ManagedWindowState>();
    managed.save_generation.fetch_add(1, Ordering::Release);

    if managed
        .save_worker_active
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    let app = app.clone();
    thread::spawn(move || loop {
        let managed = app.state::<ManagedWindowState>();
        let observed = managed.save_generation.load(Ordering::Acquire);
        thread::sleep(Duration::from_millis(250));

        if managed.save_generation.load(Ordering::Acquire) == observed {
            persist_now(&app);
            managed.save_worker_active.store(false, Ordering::Release);

            if managed.save_generation.load(Ordering::Acquire) != observed {
                persist_soon(&app);
            }
            break;
        }
    });
}

pub fn persist_now(app: &AppHandle) {
    let managed = app.state::<ManagedWindowState>();
    if let Err(error) = write_state_file(&managed.path, &managed.snapshot()) {
        eprintln!("Agent Gauge could not persist widget state: {error}");
    }
}

fn widget(app: &AppHandle) -> Result<WebviewWindow, String> {
    app.get_webview_window(WIDGET_LABEL)
        .ok_or_else(|| "widget window is unavailable".into())
}

#[cfg(target_os = "linux")]
fn realize_widget(window: &WebviewWindow) -> Result<(), String> {
    use gtk::prelude::WidgetExt;

    let gtk_window = window
        .gtk_window()
        .map_err(|error| format!("could not access GTK widget window: {error}"))?;
    gtk_window.realize();
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn realize_widget(_window: &WebviewWindow) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_map_policy(app: &AppHandle, window: &WebviewWindow) -> Result<(), String> {
    use gtk::prelude::{GtkWindowExt, WidgetExt};

    let gtk_window = window
        .gtk_window()
        .map_err(|error| format!("could not access GTK widget window: {error}"))?;
    let app = app.clone();
    gtk_window.connect_map(move |mapped_window| {
        let locked = app.state::<ManagedWindowState>().snapshot().locked;
        let mapped_window = mapped_window.clone();
        gtk::glib::idle_add_local_once(move || {
            mapped_window.set_accept_focus(!locked);
            mapped_window.set_focus_on_map(!locked);
        });
    });
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn install_map_policy(_app: &AppHandle, _window: &WebviewWindow) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn show_widget(window: &WebviewWindow) -> Result<(), String> {
    use gtk::prelude::WidgetExt;

    let gtk_window = window
        .gtk_window()
        .map_err(|error| format!("could not access GTK widget window: {error}"))?;
    gtk_window.show();
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn show_widget(window: &WebviewWindow) -> Result<(), String> {
    window
        .show()
        .map_err(|error| format!("could not show widget: {error}"))
}

#[cfg(target_os = "linux")]
fn hide_widget(window: &WebviewWindow) -> Result<(), String> {
    use gtk::prelude::WidgetExt;

    let gtk_window = window
        .gtk_window()
        .map_err(|error| format!("could not access GTK widget window: {error}"))?;
    gtk_window.hide();
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn hide_widget(window: &WebviewWindow) -> Result<(), String> {
    window
        .hide()
        .map_err(|error| format!("could not hide widget: {error}"))
}

#[cfg(target_os = "linux")]
fn enforce_focus_policy(window: &WebviewWindow, locked: bool) -> Result<(), String> {
    use gtk::prelude::GtkWindowExt;

    let gtk_window = window
        .gtk_window()
        .map_err(|error| format!("could not access GTK widget window: {error}"))?;
    gtk_window.set_accept_focus(!locked);
    gtk_window.set_focus_on_map(!locked);
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn enforce_focus_policy(window: &WebviewWindow, locked: bool) -> Result<(), String> {
    window
        .set_focusable(!locked)
        .map_err(|error| format!("could not update focus state: {error}"))
}

fn apply_window_policy(window: &WebviewWindow, state: &WidgetState) -> Result<(), String> {
    window
        .set_skip_taskbar(true)
        .map_err(|error| format!("could not set skip-taskbar/pager: {error}"))?;
    window
        .set_visible_on_all_workspaces(false)
        .map_err(|error| format!("could not scope widget to current workspace: {error}"))?;
    window
        .set_decorations(false)
        .map_err(|error| format!("could not remove window decorations: {error}"))?;
    window
        .set_shadow(false)
        .map_err(|error| format!("could not remove window shadow: {error}"))?;

    // Layering is the one window property the two platforms cannot express the
    // same way, so it lives behind `platform::window`. On Windows it may report
    // a weaker layer than requested; that is a deliberate fallback rather than
    // a failure, and the widget stays visible either way.
    crate::platform::window::apply_layer(window, state.mode)?;

    window
        .set_resizable(!state.locked)
        .map_err(|error| format!("could not update resize state: {error}"))?;
    window
        .set_focusable(!state.locked)
        .map_err(|error| format!("could not update focus state: {error}"))?;
    window
        .set_ignore_cursor_events(state.locked)
        .map_err(|error| format!("could not update click-through state: {error}"))?;
    crate::native_widget::set_locked(window.app_handle(), state.locked);
    if state.locked {
        window
            .request_user_attention(None)
            .map_err(|error| format!("could not clear attention state: {error}"))?;
        clear_attention_after_policy(window)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn clear_attention_after_policy(window: &WebviewWindow) -> Result<(), String> {
    use gtk::prelude::GtkWindowExt;

    let gtk_window = window
        .gtk_window()
        .map_err(|error| format!("could not access GTK widget window: {error}"))?;
    gtk_window.set_urgency_hint(false);
    gtk::glib::timeout_add_local_once(Duration::from_millis(100), move || {
        gtk_window.set_urgency_hint(false);
    });
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn clear_attention_after_policy(_window: &WebviewWindow) -> Result<(), String> {
    Ok(())
}

fn restore_geometry(app: &AppHandle, window: &WebviewWindow) -> Result<(), String> {
    let monitors = monitor_bounds(window)?;
    if monitors.is_empty() {
        return Ok(());
    }

    let saved = app.state::<ManagedWindowState>().snapshot().geometry;
    let restored = recover_geometry(saved.as_ref(), &monitors, MIN_WIDTH, MIN_HEIGHT)
        .ok_or_else(|| "no monitor was available for geometry recovery".to_string())?;

    let mode = app.state::<ManagedWindowState>().snapshot().mode;
    apply_geometry(window, &restored, mode)?;

    app.state::<ManagedWindowState>()
        .value
        .lock()
        .expect("window state mutex poisoned")
        .geometry = Some(restored);
    Ok(())
}

fn apply_geometry(
    window: &WebviewWindow,
    geometry: &Geometry,
    mode: DisplayMode,
) -> Result<(), String> {
    window
        .set_size(PhysicalSize::new(geometry.width, geometry.height))
        .map_err(|error| format!("could not restore widget size: {error}"))?;
    // Saved geometry is in screen coordinates; see platform::window::to_native.
    let (x, y) = crate::platform::window::to_native(mode, geometry.x, geometry.y);
    window
        .set_position(PhysicalPosition::new(x, y))
        .map_err(|error| format!("could not restore widget position: {error}"))
}

fn stabilize_geometry_after_show(app: &AppHandle, geometry: Geometry) {
    let app = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(180));
        let app_for_main = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Ok(window) = widget(&app_for_main) {
                let mode = app_for_main.state::<ManagedWindowState>().snapshot().mode;
                let _ = apply_geometry(&window, &geometry, mode);
            }
        });
        thread::sleep(Duration::from_millis(180));
        app.state::<ManagedWindowState>()
            .geometry_transition_active
            .store(false, Ordering::Release);
        capture_geometry(&app);
        persist_now(&app);
    });
}

fn monitor_bounds(window: &WebviewWindow) -> Result<Vec<MonitorBounds>, String> {
    let primary_name = window
        .primary_monitor()
        .map_err(|error| format!("could not query primary monitor: {error}"))?
        .and_then(|monitor| monitor.name().map(ToOwned::to_owned));
    let monitors = window
        .available_monitors()
        .map_err(|error| format!("could not enumerate monitors: {error}"))?;

    Ok(monitors
        .into_iter()
        .map(|monitor| {
            let position = monitor.position();
            let size = monitor.size();
            let name = monitor.name().map(ToOwned::to_owned);
            MonitorBounds {
                is_primary: name == primary_name,
                name,
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
                scale_factor: monitor.scale_factor(),
            }
        })
        .collect())
}

fn emit_state(app: &AppHandle) {
    crate::native_widget::redraw(app);
    if let Err(error) = app.emit("widget-state", app.state::<ManagedWindowState>().snapshot()) {
        eprintln!("Agent Gauge could not emit window state: {error}");
    }
}

fn load_state_file(path: &Path) -> WidgetState {
    let Ok(bytes) = fs::read(path) else {
        return WidgetState::default();
    };

    match serde_json::from_slice::<WidgetState>(&bytes) {
        Ok(state) if state.schema_version == STATE_SCHEMA_VERSION => state,
        Ok(_) | Err(_) => {
            quarantine_corrupt_state(path);
            WidgetState::default()
        }
    }
}

fn quarantine_corrupt_state(path: &Path) {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let quarantine = path.with_extension(format!("corrupt-{timestamp}.json"));
    if let Err(error) = fs::rename(path, &quarantine) {
        eprintln!("Agent Gauge could not quarantine corrupt window state: {error}");
    }
}

fn write_state_file(path: &Path, state: &WidgetState) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "window state path has no parent directory".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("could not create state directory: {error}"))?;

    let temporary = path.with_extension("json.tmp");
    let payload = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("could not serialize window state: {error}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| format!("could not create temporary state file: {error}"))?;
    file.write_all(&payload)
        .map_err(|error| format!("could not write temporary state file: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("could not sync temporary state file: {error}"))?;
    fs::rename(&temporary, path)
        .map_err(|error| format!("could not atomically replace window state: {error}"))?;
    Ok(())
}
