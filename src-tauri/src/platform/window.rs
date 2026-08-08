//! Putting the widget on the right layer: behind ordinary windows in Desktop
//! mode, above them in Pinned mode.
//!
//! Linux gets this from the window manager. `_NET_WM_STATE_BELOW` is a state
//! the WM maintains, so asking once is enough and it stays asked.
//!
//! Windows has no equivalent. `SetWindowPos(HWND_BOTTOM)` is a one-off
//! reordering, not a state, so the next window activation puts the widget back
//! under everything else — including under the desktop, where it is invisible.
//! The technique wallpaper applications use instead is to reparent into
//! `WorkerW`, the window Explorer paints the wallpaper into, which puts the
//! widget genuinely on the desktop layer.
//!
//! That reparenting is fragile in ways worth naming, because they are the
//! things that will break it:
//!
//! - Explorer restarting destroys `WorkerW` and takes the widget's parent with
//!   it, so attachment has to be re-checked rather than done once.
//! - A child window's coordinates are relative to its parent's client area, and
//!   `WorkerW` spans the whole virtual desktop, whose origin is *not* (0, 0)
//!   when a monitor sits above or to the left of the primary. Hence
//!   [`desktop_origin_offset`].
//! - It depends on Explorer's window structure, which is undocumented.
//!
//! So every path here degrades rather than fails: if `WorkerW` cannot be found
//! or attached to, the widget falls back to bottom-of-z-order, which behaves
//! correctly most of the time. An unattached widget is a widget that sits too
//! high; a widget that errored out is one the user cannot see at all.

use tauri::WebviewWindow;

use crate::window::DisplayMode;

/// Where the widget sits relative to other windows.
///
/// Reported back so the caller can tell the user what actually happened rather
/// than what was requested — see the `--diagnose-window-layer` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// Above ordinary windows.
    Pinned,
    /// Genuinely on the desktop layer, via `WorkerW`. Windows only — Linux
    /// gets the same effect from `_NET_WM_STATE_BELOW` and never reports this.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    DesktopParented,
    /// Below ordinary windows, but not parented to the desktop. What Linux
    /// always uses, and what Windows falls back to.
    DesktopBelow,
}

impl Layer {
    pub fn describe(self) -> &'static str {
        match self {
            Layer::Pinned => "pinned above other windows",
            Layer::DesktopParented => "attached to the desktop (WorkerW)",
            Layer::DesktopBelow => "below other windows",
        }
    }
}

// ---------------------------------------------------------------- Linux

#[cfg(not(target_os = "windows"))]
pub fn apply_layer(window: &WebviewWindow, mode: DisplayMode) -> Result<Layer, String> {
    match mode {
        DisplayMode::Desktop => {
            window
                .set_always_on_top(false)
                .map_err(|error| format!("could not clear pinned state: {error}"))?;
            window
                .set_always_on_bottom(true)
                .map_err(|error| format!("could not request desktop-below state: {error}"))?;
            Ok(Layer::DesktopBelow)
        }
        DisplayMode::Pinned => {
            window
                .set_always_on_bottom(false)
                .map_err(|error| format!("could not clear desktop-below state: {error}"))?;
            window
                .set_always_on_top(true)
                .map_err(|error| format!("could not request pinned-above state: {error}"))?;
            Ok(Layer::Pinned)
        }
    }
}

/// The window manager keeps the widget where it was put, so there is nothing to
/// re-assert and no watchdog to run.
#[cfg(not(target_os = "windows"))]
pub fn start_layer_watchdog(_app: &tauri::AppHandle) {}

/// Screen coordinates are the only coordinates on Linux.
#[cfg(not(target_os = "windows"))]
pub fn desktop_origin_offset(_mode: DisplayMode) -> (i32, i32) {
    (0, 0)
}

// -------------------------------------------------------------- Windows

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::*;

    use std::ffi::c_void;

    use windows_sys::Win32::{
        Foundation::{BOOL, HWND, LPARAM, TRUE},
        UI::WindowsAndMessaging::{
            EnumWindows, FindWindowExW, FindWindowW, GetParent, GetSystemMetrics,
            SendMessageTimeoutW, SetParent, SetWindowPos, HWND_BOTTOM, HWND_NOTOPMOST,
            HWND_TOPMOST, SMTO_NORMAL, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE,
            SWP_NOMOVE, SWP_NOSIZE,
        },
    };

    /// Undocumented Progman message that asks Explorer to create the `WorkerW`
    /// window the wallpaper is painted into. Explorer only spawns it on demand.
    const WM_SPAWN_WORKER_W: u32 = 0x052C;

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn hwnd(window: &WebviewWindow) -> Result<HWND, String> {
        window
            .hwnd()
            .map(|handle| handle.0 as HWND)
            .map_err(|error| format!("could not access the widget window handle: {error}"))
    }

    /// Finds the `WorkerW` that sits behind the desktop icons.
    ///
    /// Explorer keeps two `WorkerW` windows. The one we want is the sibling
    /// *following* the window that hosts `SHELLDLL_DefView` (the icon view);
    /// the other one is in front of the icons and would hide them.
    unsafe fn find_worker_w() -> Option<HWND> {
        let progman = FindWindowW(wide("Progman").as_ptr(), std::ptr::null());
        if progman.is_null() {
            return None;
        }

        // Ask Explorer to spawn WorkerW. Ignore the result: on a desktop where
        // it already exists this is a no-op, and the enumeration below is the
        // real test of success.
        let mut result: usize = 0;
        SendMessageTimeoutW(
            progman,
            WM_SPAWN_WORKER_W,
            0,
            0,
            SMTO_NORMAL,
            1000,
            &mut result as *mut usize as *mut _,
        );

        unsafe extern "system" fn callback(window: HWND, target: LPARAM) -> BOOL {
            let shell_view = FindWindowExW(
                window,
                std::ptr::null_mut(),
                wide("SHELLDLL_DefView").as_ptr(),
                std::ptr::null(),
            );
            if !shell_view.is_null() {
                // The WorkerW after this one is the wallpaper layer.
                let worker = FindWindowExW(
                    std::ptr::null_mut(),
                    window,
                    wide("WorkerW").as_ptr(),
                    std::ptr::null(),
                );
                if !worker.is_null() {
                    *(target as *mut HWND) = worker;
                    return 0; // Stop enumerating.
                }
            }
            TRUE
        }

        let mut found: HWND = std::ptr::null_mut();
        EnumWindows(Some(callback), &mut found as *mut HWND as LPARAM);
        (!found.is_null()).then_some(found)
    }

    fn is_attached(window: HWND) -> bool {
        unsafe { !GetParent(window).is_null() }
    }

    fn detach(window: HWND) {
        if is_attached(window) {
            unsafe {
                SetParent(window, std::ptr::null_mut());
            }
        }
    }

    fn sink_to_bottom(window: HWND) {
        unsafe {
            SetWindowPos(
                window,
                HWND_BOTTOM,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    pub fn apply_layer(window: &WebviewWindow, mode: DisplayMode) -> Result<Layer, String> {
        let handle = hwnd(window)?;

        match mode {
            DisplayMode::Pinned => {
                // Leave the desktop layer first: a child of WorkerW cannot be
                // topmost, so the reparent has to be undone before asking.
                detach(handle);
                window
                    .set_always_on_bottom(false)
                    .map_err(|error| format!("could not clear desktop-below state: {error}"))?;
                window
                    .set_always_on_top(true)
                    .map_err(|error| format!("could not request pinned-above state: {error}"))?;
                unsafe {
                    SetWindowPos(
                        handle,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
                Ok(Layer::Pinned)
            }
            DisplayMode::Desktop => {
                window
                    .set_always_on_top(false)
                    .map_err(|error| format!("could not clear pinned state: {error}"))?;
                unsafe {
                    SetWindowPos(
                        handle,
                        HWND_NOTOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }

                let worker = unsafe { find_worker_w() };
                if let Some(worker) = worker {
                    let attached = unsafe { !SetParent(handle, worker).is_null() };
                    if attached {
                        return Ok(Layer::DesktopParented);
                    }
                }

                // Explorer would not cooperate. Sitting at the bottom of the
                // z-order is not identical, but it is visible and close enough
                // to be useful.
                sink_to_bottom(handle);
                Ok(Layer::DesktopBelow)
            }
        }
    }

    /// Watches for the desktop layer being lost and re-establishes it.
    ///
    /// Explorer restarting destroys `WorkerW` and orphans the widget, which
    /// would otherwise leave it floating above the desktop until the next
    /// restart. The check is a single `GetParent` call, so polling costs
    /// effectively nothing; the work only happens when something has actually
    /// broken.
    pub fn start_layer_watchdog(app: &tauri::AppHandle) {
        use tauri::Manager;

        let app = app.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(5));

            let mode = app
                .try_state::<crate::window::ManagedWindowState>()
                .map(|state| state.snapshot().mode);
            let Some(DisplayMode::Desktop) = mode else {
                continue;
            };

            let app_for_main = app.clone();
            let _ = app.run_on_main_thread(move || {
                let Some(window) = app_for_main.get_webview_window(crate::window::WIDGET_LABEL)
                else {
                    return;
                };
                let Ok(handle) = hwnd(&window) else { return };
                if is_attached(handle) {
                    return;
                }
                if let Err(error) = apply_layer(&window, DisplayMode::Desktop) {
                    eprintln!("Agent Gauge could not restore the desktop layer: {error}");
                }
            });
        });
    }

    /// How far the desktop's coordinate space is offset from screen coordinates.
    ///
    /// A window parented to `WorkerW` is positioned relative to that window's
    /// client area, which starts at the top-left of the *virtual* desktop. With
    /// a single monitor, or with additional monitors placed right and below,
    /// that is (0, 0) and this offset vanishes. Put a monitor above or to the
    /// left of the primary and the origin goes negative, at which point saved
    /// screen coordinates would place the widget on the wrong monitor without
    /// this correction.
    pub fn desktop_origin_offset(mode: DisplayMode) -> (i32, i32) {
        if mode != DisplayMode::Desktop {
            return (0, 0);
        }
        unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
            )
        }
    }

    /// A human-readable dump of the widget's actual window state.
    ///
    /// Windows hardware gets booted rarely during this project's development,
    /// so when it is, the priority is finding out what really happened rather
    /// than inferring it from how the widget looks.
    pub fn diagnose(window: &WebviewWindow, mode: DisplayMode) -> String {
        let Ok(handle) = hwnd(window) else {
            return "widget window handle unavailable".into();
        };
        let parent = unsafe { GetParent(handle) };
        let worker = unsafe { find_worker_w() };
        let (offset_x, offset_y) = desktop_origin_offset(mode);

        format!(
            "mode={mode:?}\n\
             hwnd={handle:p}\n\
             parent={parent:p} (attached={})\n\
             workerw={}\n\
             virtual-desktop-origin=({offset_x}, {offset_y})",
            !parent.is_null(),
            worker.map_or_else(|| "not found".to_string(), |worker| format!("{worker:p}")),
        )
    }

    /// Silences the unused-import warning for `c_void` on targets where the
    /// pointer type alias does not reference it directly.
    #[allow(dead_code)]
    type _Unused = *mut c_void;
}

#[cfg(target_os = "windows")]
pub use windows_impl::{apply_layer, desktop_origin_offset, diagnose, start_layer_watchdog};

/// Nothing platform-specific to report on Linux; the window manager owns the
/// layer and reports it through the usual EWMH properties.
#[cfg(not(target_os = "windows"))]
pub fn diagnose(_window: &WebviewWindow, mode: DisplayMode) -> String {
    format!("mode={mode:?}\nlayer managed by the window manager (_NET_WM_STATE_BELOW/ABOVE)")
}

/// Converts a saved screen position into the coordinate space the window is
/// actually positioned in.
///
/// Geometry is persisted in screen coordinates, because that is what it means
/// to the user and what `window/geometry.rs` reasons about. A widget parented
/// to `WorkerW` is positioned relative to the virtual desktop instead, so the
/// two spaces have to be converted between on the way in and out. Everywhere
/// else — Linux, and Windows in Pinned mode — this is the identity.
pub fn to_native(mode: DisplayMode, x: i32, y: i32) -> (i32, i32) {
    let (offset_x, offset_y) = desktop_origin_offset(mode);
    (x - offset_x, y - offset_y)
}

/// The inverse of [`to_native`], for reading a position back off the window.
///
/// These must stay a matched pair. Converting on only one side would move the
/// widget by the offset on every save/restore cycle, walking it off screen on a
/// multi-monitor desktop.
pub fn to_screen(mode: DisplayMode, x: i32, y: i32) -> (i32, i32) {
    let (offset_x, offset_y) = desktop_origin_offset(mode);
    (x + offset_x, y + offset_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinate_conversion_round_trips() {
        // The property that matters: whatever the offset is on this platform,
        // saving and restoring a position must not move the widget.
        for mode in [DisplayMode::Desktop, DisplayMode::Pinned] {
            for (x, y) in [(0, 0), (1920, 0), (-1280, -400), (37, 1123)] {
                let (native_x, native_y) = to_native(mode, x, y);
                assert_eq!(
                    to_screen(mode, native_x, native_y),
                    (x, y),
                    "round trip failed for {mode:?} at ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn pinned_mode_uses_screen_coordinates_unchanged() {
        // Only the desktop layer can have a shifted origin; a pinned window is
        // always positioned in screen coordinates.
        assert_eq!(desktop_origin_offset(DisplayMode::Pinned), (0, 0));
        assert_eq!(to_native(DisplayMode::Pinned, 100, -50), (100, -50));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn linux_never_offsets_coordinates() {
        assert_eq!(desktop_origin_offset(DisplayMode::Desktop), (0, 0));
    }
}
