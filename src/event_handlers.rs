//! Event handling logic for window events and GUI callbacks.
//!
//! This module extracts the complex callback logic from main.rs into
//! well-structured, testable functions.

use crate::events::{EventSeverity, push_ui_event};
use once_cell::sync::OnceCell;
use std::sync::Arc;
use std::sync::Mutex;
use tracing::{error, info};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_DESTROY, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZESTART,
};

use crate::app_state::AppState;
use crate::context::{reopen_context, reopen_with_template};
use crate::gui::{
    SelectorType, get_selected_selector_type, get_selector_text, is_run_enabled, is_target_present,
    reflect_target_presence, set_tray_error, start_wait_timer, stop_wait_timer,
};
use crate::mapping::{apply_mapping, rect_to_logcontext};
use crate::winevent::{
    HookFilter, Target, find_existing_target, install_hooks, query_window_rect, update_target,
};
use crate::wintab::wt_get;

/// Type alias for window event hook callback function
type HookCallback = Arc<dyn Fn(HWND, u32, RECT) + Send + Sync>;

/// Handle a filtered window event (move, resize, foreground change, destroy, etc.).
///
/// This consolidates several concerns:
/// * Target lifecycle (destroy / minimize => reset mapping to full tablet).
/// * Degenerate rectangle repair (some events deliver 0x0 bounds temporarily).
/// * Conditional context reopen on foreground to mitigate driver resets.
/// * Aspect ratio logic via `apply_window_mapping`.
/// * Tray / button UI reflection of target presence.
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
        push_ui_event(EventSeverity::Info, "Target lost");
        start_wait_timer(app_state.host_window.0);
        return;
    }

    // If user disabled mapping, ignore further events (except destroy/minimize above)
    if !is_run_enabled() {
        return;
    }

    // Handle foreground events (reopen context to work around driver issues)
    if event == EVENT_SYSTEM_FOREGROUND
        && reopen_context(
            &app_state.wintab_context,
            app_state.host_window,
            app_state.base_context,
            app_state.final_options,
        )
        .is_err()
    {
        set_tray_error();
        push_ui_event(EventSeverity::Error, "Context reopen failed (foreground)");
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
    stop_wait_timer();
    if !is_target_present() {
        // if previously absent
        push_ui_event(EventSeverity::Info, "Target appeared");
    }

    // Optional debug dump
    dump_context_state_if_requested(&app_state);
}

/// Handle run/stop toggle from GUI.
///
/// Dispatches to enable / disable handlers which perform mapping and hook maintenance.
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

/// Handle aspect ratio toggle from GUI.
///
/// When the mapping is currently active we immediately re‑apply with the new aspect setting.
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
        push_ui_event(
            EventSeverity::Info,
            format!("Aspect mode {}", if enabled { "ON" } else { "OFF" }),
        );
    }
}
/// Reset mapping when target is destroyed or minimized.
///
/// Falls back to the original full‑tablet LOGCONTEXT so the user regains full area until
/// a new target becomes available again.
fn handle_target_destroyed(app_state: &AppState) {
    if let Ok(h) = app_state.wintab_context.lock()
        && let Err(e) = apply_mapping(*h, &app_state.base_context)
    {
        error!(?e, "reset mapping failed");
        set_tray_error();
        push_ui_event(EventSeverity::Error, "Mapping reset failed");
    }
}

/// Handle enabling the mapping.
///
/// Potentially installs hooks (if previously inactive), reopens context to ensure driver
/// state is fresh, and applies mapping immediately if the target window already exists.
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
            stop_wait_timer();
            push_ui_event(EventSeverity::Info, "Start enabled");
        } else {
            reflect_target_presence(HWND(std::ptr::null_mut()), false);
            start_wait_timer(app_state.host_window.0);
        }
    } else {
        // No target yet; show waiting state
        reflect_target_presence(HWND(std::ptr::null_mut()), false);
        start_wait_timer(app_state.host_window.0);
    }
}

/// Handle disabling the mapping.
///
/// Reverts to full‑tablet mapping but leaves hooks installed (so the next enable is fast).
fn handle_run_disabled(app_state: &AppState) {
    // Reset mapping to full tablet
    if let Ok(h) = app_state.wintab_context.lock() {
        let _ = apply_mapping(*h, &app_state.base_context);
    }

    // Update presence indicator
    reflect_target_presence(HWND(std::ptr::null_mut()), find_existing_target().is_some());
    push_ui_event(EventSeverity::Info, "Start disabled");
    start_wait_timer(app_state.host_window.0); // continue waiting in case user re-enables soon
}

/// Update target from the current GUI selector input.
///
/// If the target type/value changed we either update the existing installed hook filter or
/// install hooks for the first time (when the mapping was just enabled).
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

/// Apply mapping for a specific window rectangle.
///
/// Aspect‑preserved mappings require a full context reopen with a geometry‑modified template;
/// non‑aspect mappings apply directly via `apply_mapping` (WTSetA wrapper).
fn apply_window_mapping(app_state: &AppState, rect: RECT) {
    let config = app_state.get_mapping_config();
    let ctx = rect_to_logcontext(app_state.base_context, rect, &config);
    // Rect-change guard (GUI emission only):
    // Some WinEvents (notably EVENT_OBJECT_LOCATIONCHANGE) can fire repeatedly on right-click
    // or non-client interactions even when the window geometry is unchanged. Previously each
    // of those events resulted in a visible pair of lines in the GUI event feed (context reopen
    // + mapping applied) creating noise without conveying new information. We deliberately do
    // NOT change functional behavior (the mapping / reopen path still executes exactly as before)
    // but we suppress redundant user-facing "Mapping applied" lines when the (left,top,right,bottom)
    // rectangle and aspect mode are identical to the last emitted mapping. This keeps the feed
    // high-signal while preserving identical runtime semantics.
    // Cached last emitted rectangle + aspect flag to suppress duplicate GUI lines.
    type LastEmittedRect = (i32, i32, i32, i32, bool);
    static LAST_EMITTED: OnceCell<Mutex<Option<LastEmittedRect>>> = OnceCell::new();
    let aspect_flag = config.keep_aspect;
    let should_emit = {
        let cell = LAST_EMITTED.get_or_init(|| Mutex::new(None));
        let mut guard = cell.lock().unwrap();
        match *guard {
            Some((l, t, r, b, a))
                if l == rect.left
                    && t == rect.top
                    && r == rect.right
                    && b == rect.bottom
                    && a == aspect_flag =>
            {
                false
            }
            _ => {
                *guard = Some((rect.left, rect.top, rect.right, rect.bottom, aspect_flag));
                true
            }
        }
    };

    if config.keep_aspect {
        if reopen_with_template(
            &app_state.wintab_context,
            app_state.host_window,
            ctx,
            app_state.final_options,
        )
        .is_ok()
        {
            info!(
                left = rect.left,
                top = rect.top,
                right = rect.right,
                bottom = rect.bottom,
                "mapping applied via reopen_with_template(aspect)"
            );
            if should_emit {
                push_ui_event(
                    EventSeverity::Info,
                    format!(
                        "Mapping applied (aspect) {}/{}/{}/{}",
                        rect.left, rect.top, rect.right, rect.bottom
                    ),
                );
            }
        } else {
            error!("reopen_with_template failed; aspect mapping skipped");
            set_tray_error();
            push_ui_event(EventSeverity::Error, "Aspect mapping failed");
        }
    } else if let Ok(h) = app_state.wintab_context.lock() {
        if let Err(e) = apply_mapping(*h, &ctx) {
            error!(?e, "apply_mapping failed");
            set_tray_error();
            push_ui_event(EventSeverity::Error, "Mapping apply failed");
        } else {
            info!(
                left = rect.left,
                top = rect.top,
                right = rect.right,
                bottom = rect.bottom,
                "mapping applied"
            );
            if should_emit {
                push_ui_event(
                    EventSeverity::Info,
                    format!(
                        "Mapping applied {}/{}/{}/{}",
                        rect.left, rect.top, rect.right, rect.bottom
                    ),
                );
            }
        }
    }
}

/// Debug dump context state if `WINTAB_DUMP=1`.
///
/// Intended for troubleshooting geometry or driver behaviour; kept cheap by only executing
/// when the environment variable is explicitly set.
fn dump_context_state_if_requested(app_state: &AppState) {
    let dump = matches!(std::env::var("WINTAB_DUMP"), Ok(ref v) if v == "1");
    if dump
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
