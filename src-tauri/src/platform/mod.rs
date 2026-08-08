//! Everything that differs between Linux and Windows.
//!
//! The rule this module exists to enforce: platform-conditional code lives
//! here, and nowhere else. The rest of the crate is written once and compiled
//! everywhere. When a caller needs behaviour that only one platform can
//! provide, the contract is named here and each platform satisfies it, rather
//! than sprinkling `#[cfg]` through the application logic.
//!
//! Submodules are organised by topic rather than by operating system, so both
//! implementations of a contract sit next to each other in one file. That
//! adjacency is deliberate: the failure mode for a two-platform app is the two
//! sides quietly drifting apart, and it is much harder to change one without
//! the other when they share a screen.
//!
//! Where logic is pure — quoting rules, `PATH` search — both platforms'
//! implementations are compiled and tested on both targets, so Windows
//! behaviour is verifiable from a Linux checkout.

pub mod autostart;
pub mod dirs;
pub mod exec;
pub mod instance;
pub mod shell;
pub mod window;

/// Process-wide setup that must happen before any window is created.
///
/// Runs before the Tauri builder, and before the single-instance lock, because
/// on Linux it sets an environment variable the webview reads during its own
/// initialisation.
pub fn pre_init() {
    #[cfg(target_os = "linux")]
    {
        // WebKitGTK's DMA-BUF renderer produces a black or garbled surface on
        // some NVIDIA drivers, which is fatal for a transparent widget. Opt out
        // unless the user has expressed their own preference, so a working
        // machine can still override it.
        if std::env::var_os("WEBKIT_DMABUF_RENDERER_DISABLE_GBM").is_none()
            && std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none()
        {
            std::env::set_var("WEBKIT_DMABUF_RENDERER_DISABLE_GBM", "1");
        }
    }
}
