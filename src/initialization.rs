//! Application initialization utilities.
//!
//! This module handles complex initialization logic including initial mapping
//! application and target detection, extracted from main() for better maintainability.

use std::sync::Arc;
use tracing::{error, info};
use windows::Win32::Foundation::HWND;

use crate::app_state::AppState;
use crate::callbacks::{
    HookCallback, create_window_event_callback, install_hooks_if_target_available,
    register_gui_callbacks,
};
use crate::gui::{reflect_target_presence, set_tray_error};
use crate::mapping::{MapConfig, apply_mapping, rect_to_logcontext};
use crate::winevent::{find_existing_target, query_window_rect};
use crate::wintab::LOGCONTEXTA;

/// Combined initialization: setup callbacks, optionally install hooks, and apply initial mapping.
///
/// Encapsulates the ordered side‑effects required after creating the WinTab context and GUI.
/// By batching them we minimize repeated `Arc` cloning boilerplate in `main` and create a
/// single place to evolve startup behaviour.
pub fn setup_callbacks_and_initial_mapping(
    app_state: Arc<AppState>,
    base_context: LOGCONTEXTA,
    config: &MapConfig,
) -> HookCallback {
    // Create window event callback
    let cb = create_window_event_callback(app_state.clone());

    // Install hooks immediately only if we have a target from CLI
    if let Err(e) = install_hooks_if_target_available(app_state.clone(), cb.clone()) {
        error!(?e, "install_hooks failed");
        set_tray_error();
    }

    // Register GUI callbacks
    register_gui_callbacks(app_state.clone(), Some(cb.clone()));

    // Apply initial mapping immediately if target already exists
    apply_initial_mapping_if_target_exists(app_state, base_context, config);

    cb
}

/// Apply initial mapping if a target window already exists at startup.
///
/// We opportunistically map immediately so the user has a working setup before the
/// first relevant WinEvent fires. Failures surface a tray error state but are otherwise
/// non‑fatal.
pub fn apply_initial_mapping_if_target_exists(
    app_state: Arc<AppState>,
    base_context: LOGCONTEXTA,
    config: &MapConfig,
) {
    if !app_state.has_target() {
        reflect_target_presence(HWND(std::ptr::null_mut()), false);
        return;
    }

    if let Some(hwnd_init) = find_existing_target() {
        if let Some(rect) = query_window_rect(hwnd_init) {
            info!(?rect, "initial target window found; applying mapping");

            let ctx = rect_to_logcontext(base_context, rect, config);

            if let Ok(h) = app_state.wintab_context.lock() {
                if let Err(e) = apply_mapping(*h, &ctx) {
                    error!(?e, "initial apply_mapping failed");
                    set_tray_error();
                }
            } else {
                error!("mutex poisoned during initial mapping");
                set_tray_error();
            }

            reflect_target_presence(HWND(std::ptr::null_mut()), true);
        } else {
            // Target window found but couldn't get rect
            reflect_target_presence(HWND(std::ptr::null_mut()), false);
        }
    } else {
        // No target window found
        reflect_target_presence(HWND(std::ptr::null_mut()), false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::winevent::Target;
    use windows::Win32::Foundation::HWND;

    #[test]
    fn initial_mapping_with_no_target() {
        let app_state = Arc::new(AppState::new(
            0,                             // Mock HCTX (isize)
            unsafe { std::mem::zeroed() }, // Mock LOGCONTEXTA
            0,                             // Mock options
            HWND(std::ptr::null_mut()),    // Mock HWND
            None,                          // No target
            false,
        ));

        let config = MapConfig { keep_aspect: false };

        // Should not panic and should handle no target gracefully
        apply_initial_mapping_if_target_exists(app_state, unsafe { std::mem::zeroed() }, &config);
    }

    #[test]
    fn initial_mapping_with_target() {
        let app_state = Arc::new(AppState::new(
            0,                                                 // Mock HCTX (isize)
            unsafe { std::mem::zeroed() },                     // Mock LOGCONTEXTA
            0,                                                 // Mock options
            HWND(std::ptr::null_mut()),                        // Mock HWND
            Some(Target::ProcessName("test.exe".to_string())), // Has target
            false,
        ));

        let config = MapConfig { keep_aspect: true };

        // Should not panic and should handle target lookup gracefully
        // (will likely fail to find target in test environment, but shouldn't crash)
        apply_initial_mapping_if_target_exists(app_state, unsafe { std::mem::zeroed() }, &config);
    }
}
