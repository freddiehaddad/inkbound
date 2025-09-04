//! Event handling logic for window events and GUI callbacks.
//!
//! This module extracts the complex callback logic from main.rs into
//! well-structured, testable functions.

use std::sync::Arc;
use tracing::{error, info};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_DESTROY, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZESTART,
};

use crate::app_state::AppState;
use crate::context::{reopen_context, reopen_with_template};
use crate::gui::{
    SelectorType, get_selected_selector_type, get_selector_text, is_run_enabled,
    reflect_target_presence, set_tray_error,
};
use crate::mapping::{apply_mapping, rect_to_logcontext};
use crate::winevent::{
    HookFilter, Target, find_existing_target, install_hooks, query_window_rect, update_target,
};
use crate::wintab::wt_get;

/// Type alias for window event hook callback function
type HookCallback = Arc<dyn Fn(HWND, u32, RECT) + Send + Sync>;

/// Handle window events (move, resize, focus, destroy, etc.)
pub fn handle_window_event(app_state: Arc<AppState>, hwnd: HWND, event: u32, mut rect: RECT) {
    // If no target yet, ignore events
    if !app_state.has_target() {
        return;
    }

    info!(
        event,
        left = rect.left,
        top = rect.top,
        right = rect.right,
        bottom = rect.bottom,
        "window event"
    );

    // Handle target destruction/minimization
    if event == EVENT_OBJECT_DESTROY || event == EVENT_SYSTEM_MINIMIZESTART {
        handle_target_destroyed(&app_state);
        reflect_target_presence(HWND(std::ptr::null_mut()), false);
        return;
    }

    // If user disabled mapping, ignore further events (except destroy/minimize above)
    if !is_run_enabled() {
        return;
    }

    // Handle foreground events (reopen context to work around driver issues)
    if event == EVENT_SYSTEM_FOREGROUND
        && !reopen_context(
            &app_state.wintab_context,
            app_state.host_window,
            app_state.base_context,
            app_state.final_options,
        )
    {
        set_tray_error();
        return;
    }

    // Validate/fix rect if necessary
    if rect.right - rect.left <= 0 || rect.bottom - rect.top <= 0 {
        if let Some(r2) = query_window_rect(hwnd) {
            info!(
                left = r2.left,
                top = r2.top,
                right = r2.right,
                bottom = r2.bottom,
                "queried rect fallback after degenerate event rect"
            );
            rect = r2;
        } else {
            error!("failed to acquire valid rect; skipping mapping update");
            set_tray_error();
            return;
        }
    }

    // Apply the mapping
    apply_window_mapping(&app_state, rect);

    // Update UI to show target is present
    reflect_target_presence(HWND(std::ptr::null_mut()), true);

    // Optional debug dump
    dump_context_state_if_requested(&app_state);
}

/// Handle run/stop toggle from GUI
pub fn handle_run_toggle(
    app_state: Arc<AppState>,
    enabled: bool,
    hook_callback: Option<HookCallback>,
) {
    if enabled {
        handle_run_enabled(&app_state, hook_callback);
    } else {
        handle_run_disabled(&app_state);
    }
}

/// Handle aspect ratio toggle from GUI
pub fn handle_aspect_toggle(app_state: Arc<AppState>, enabled: bool) {
    app_state.set_preserve_aspect(enabled);

    if !is_run_enabled() {
        return;
    }

    // Reapply mapping with new aspect setting if target present
    if let Some(hwnd_cur) = find_existing_target()
        && let Some(rect) = query_window_rect(hwnd_cur)
    {
        apply_window_mapping(&app_state, rect);
        info!(
            keep_aspect = enabled,
            left = rect.left,
            top = rect.top,
            right = rect.right,
            bottom = rect.bottom,
            "aspect toggle re-mapped"
        );
        reflect_target_presence(HWND(std::ptr::null_mut()), true);
    }
}
/// Reset mapping when target is destroyed/minimized
fn handle_target_destroyed(app_state: &AppState) {
    if let Ok(h) = app_state.wintab_context.lock()
        && let Err(e) = apply_mapping(*h, &app_state.base_context)
    {
    error!(?e, "reset mapping failed");
    set_tray_error();
    }
}

/// Handle enabling the mapping
fn handle_run_enabled(app_state: &AppState, hook_callback: Option<HookCallback>) {
    // Update target from GUI input
    update_target_from_gui(app_state, hook_callback);

    // If target window exists, apply mapping immediately
    if let Some(hwnd_cur) = find_existing_target() {
        // Reopen context to handle any missed foreground events
        let _ = reopen_context(
            &app_state.wintab_context,
            app_state.host_window,
            app_state.base_context,
            app_state.final_options,
        );

        if let Some(rect) = query_window_rect(hwnd_cur) {
            apply_window_mapping(app_state, rect);
            info!("run re-enabled mapping applied");
            reflect_target_presence(HWND(std::ptr::null_mut()), true);
        } else {
            reflect_target_presence(HWND(std::ptr::null_mut()), false);
        }
    } else {
        // No target yet; show waiting state
        reflect_target_presence(HWND(std::ptr::null_mut()), false);
    }
}

/// Handle disabling the mapping
fn handle_run_disabled(app_state: &AppState) {
    // Reset mapping to full tablet
    if let Ok(h) = app_state.wintab_context.lock() {
        let _ = apply_mapping(*h, &app_state.base_context);
    }

    // Update presence indicator
    reflect_target_presence(HWND(std::ptr::null_mut()), find_existing_target().is_some());
}

/// Update target from GUI selector input
fn update_target_from_gui(app_state: &AppState, hook_callback: Option<HookCallback>) {
    if let Some(sel_txt) = get_selector_text() {
        let trimmed = sel_txt.trim();
        if !trimmed.is_empty() {
            let selector_type = get_selected_selector_type();
            let new_target = match selector_type {
                SelectorType::Process => Some(Target::ProcessName(trimmed.to_string())),
                SelectorType::WindowClass => Some(Target::WindowClass(trimmed.to_string())),
                SelectorType::Title => Some(Target::TitleSubstring(trimmed.to_string())),
            };

            if let Some(target) = new_target {
                let current = app_state.get_current_target();
                if current.as_ref() != Some(&target) {
                    let already_installed = current.is_some();
                    app_state.set_current_target(Some(target.clone()));

                    if already_installed {
                        let _ = update_target(target);
                    } else if let Some(callback) = hook_callback {
                        let _ = install_hooks(HookFilter { target }, callback);
                    }
                }
            }
        }
    }
}

/// Apply mapping for a specific window rectangle
fn apply_window_mapping(app_state: &AppState, rect: RECT) {
    let config = app_state.get_mapping_config();
    let ctx = rect_to_logcontext(app_state.base_context, rect, &config);

    if config.keep_aspect {
    if reopen_with_template(
            &app_state.wintab_context,
            app_state.host_window,
            ctx,
            app_state.final_options,
        ) {
            info!(
                left = rect.left,
                top = rect.top,
                right = rect.right,
                bottom = rect.bottom,
                "mapping applied via reopen_with_template(aspect)"
            );
        } else {
            error!("reopen_with_template failed; aspect mapping skipped");
            set_tray_error();
        }
    } else if let Ok(h) = app_state.wintab_context.lock() {
        if let Err(e) = apply_mapping(*h, &ctx) {
            error!(?e, "apply_mapping failed");
            set_tray_error();
        } else {
            info!(
                left = rect.left,
                top = rect.top,
                right = rect.right,
                bottom = rect.bottom,
                "mapping applied"
            );
        }
    }
}

/// Debug dump context state if WINTAB_DUMP=1
fn dump_context_state_if_requested(app_state: &AppState) {
    if std::env::var("WINTAB_DUMP").as_deref() == Ok("1")
        && let Ok(h) = app_state.wintab_context.lock()
        && let Ok(cur) = wt_get(*h)
    {
        info!(
            out_org_x = cur.lcOutOrgX,
            out_org_y = cur.lcOutOrgY,
            out_ext_x = cur.lcOutExtX,
            out_ext_y = cur.lcOutExtY,
            sys_org_x = cur.lcSysOrgX,
            sys_org_y = cur.lcSysOrgY,
            sys_ext_x = cur.lcSysExtX,
            sys_ext_y = cur.lcSysExtY,
            in_org_x = cur.lcInOrgX,
            in_org_y = cur.lcInOrgY,
            in_ext_x = cur.lcInExtX,
            in_ext_y = cur.lcInExtY,
            "post-apply context state"
        );
    }
}
