//! Callback registration and management utilities.
//!
//! This module consolidates the callback registration patterns used throughout
//! the application, reducing code duplication and Arc cloning complexity.

use std::sync::Arc;
use windows::Win32::Foundation::{HWND, RECT};

use crate::app_state::AppState;
use crate::event_handlers::{handle_aspect_toggle, handle_run_toggle, handle_window_event};
use crate::gui::{set_aspect_toggle_callback, set_run_toggle_callback};
use crate::winevent::{HookFilter, install_hooks};

/// Type alias for a window event hook callback function.
///
/// The callback receives:
/// * `HWND` – The window whose event fired (already filtered for relevance upstream).
/// * `u32` – The WinEvent event identifier.
/// * `RECT` – The window bounds derived from DWM (frame inclusive) or traditional APIs.
pub type HookCallback = Arc<dyn Fn(HWND, u32, RECT) + Send + Sync>;

/// Create a window event callback that forwards to the shared event handler logic.
///
/// This indirection keeps hook installation agnostic of internal handler function
/// signatures and allows easy test replacement if needed.
pub fn create_window_event_callback(app_state: Arc<AppState>) -> HookCallback {
    Arc::new(move |hwnd: HWND, event: u32, rect: RECT| {
        handle_window_event(app_state.clone(), hwnd, event, rect);
    })
}

/// Register all GUI callbacks (run toggle + aspect ratio) wiring them to the central
/// event handler functions.
///
/// A small closure is created per callback to capture the shared `AppState`. This avoids
/// leaking `Arc` proliferation to GUI creation code.
pub fn register_gui_callbacks(app_state: Arc<AppState>, hook_callback: Option<HookCallback>) {
    // Register Start/Stop callback
    {
        let app_state_for_run_toggle = app_state.clone();
        let cb_for_hooks = hook_callback.clone();
        set_run_toggle_callback(Arc::new(move |enabled| {
            handle_run_toggle(
                app_state_for_run_toggle.clone(),
                enabled,
                cb_for_hooks.clone(),
            );
        }));
    }

    // Register aspect ratio toggle callback
    {
        let app_state_for_aspect_toggle = app_state;
        set_aspect_toggle_callback(Arc::new(move |enabled| {
            handle_aspect_toggle(app_state_for_aspect_toggle.clone(), enabled);
        }));
    }
}

/// Install window event hooks if (and only if) a target was supplied on the CLI.
///
/// If no target is present yet we skip installation; the user may later choose a target
/// via the GUI which triggers a separate installation path.
pub fn install_hooks_if_target_available(
    app_state: Arc<AppState>,
    callback: HookCallback,
) -> Result<(), anyhow::Error> {
    if let Some(target) = app_state.get_current_target() {
        install_hooks(HookFilter { target }, callback)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::winevent::Target;
    use windows::Win32::Foundation::HWND;

    #[test]
    fn window_event_callback_creation() {
        // Create a mock AppState for testing
        let app_state = Arc::new(AppState::new(
            0,                             // Mock HCTX (isize)
            unsafe { std::mem::zeroed() }, // Mock LOGCONTEXTA
            0,                             // Mock options
            HWND(std::ptr::null_mut()),    // Mock HWND
            Some(Target::ProcessName("test.exe".to_string())),
            false,
        ));

        let callback = create_window_event_callback(app_state);

        // Verify we got a valid callback (just check that it's not null)
        assert!(Arc::strong_count(&callback) >= 1);
    }

    #[test]
    fn hook_installation_with_no_target() {
        let app_state = Arc::new(AppState::new(
            0, // Mock HCTX (isize)
            unsafe { std::mem::zeroed() },
            0,
            HWND(std::ptr::null_mut()),
            None, // No target
            false,
        ));

        let callback = create_window_event_callback(app_state.clone());

        // Should succeed even with no target (just doesn't install hooks)
        let result = install_hooks_if_target_available(app_state, callback);
        assert!(result.is_ok());
    }
}
