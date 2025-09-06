//! Centralized application state management.
//!
//! This module provides a single, thread-safe container for all application state,
//! replacing the scattered Arc<Mutex<>> variables throughout the codebase.

use crate::context::SendHwnd;
use crate::mapping::MapConfig;
use crate::winevent::Target;
use crate::wintab::{HCTX, LOGCONTEXTA};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::HWND;

/// Centralized application state container.
///
/// This structure aggregates all mutable runtime state that needs to be shared across
/// WinEvent hook callbacks, GUI event handlers, and mapping logic. It intentionally
/// limits the number of synchronization primitives by grouping related data so we don't
/// scatter `Arc<Mutex<..>>` throughout the codebase. Only the WinTab context handle and
/// target specification require a mutex (they are mutated across code paths). The other
/// pieces rely on atomics for low‐overhead reads from high‑frequency callbacks.
pub struct AppState {
    /// WinTab context handle (thread-safe)
    pub wintab_context: Arc<Mutex<HCTX>>,

    /// Base WinTab context for resets and templates
    pub base_context: LOGCONTEXTA,

    /// Final options used for WinTab context
    pub final_options: u32,

    /// Current target window specification
    pub current_target: Arc<Mutex<Option<Target>>>,

    /// Whether to preserve aspect ratio (atomic for performance in callbacks)
    pub preserve_aspect: AtomicBool,

    /// Host window handle for WinTab
    pub host_window: SendHwnd,
}

impl AppState {
    /// Create a new application state instance.
    ///
    /// Parameters:
    /// * `wintab_context` – Initial WinTab context handle returned by `wt_open`.
    /// * `base_context` – Driver default LOGCONTEXT snapshot for use as a reset/template.
    /// * `final_options` – Option flag bitfield that succeeded during context open fallback.
    /// * `host_window` – HWND the context is bound to (also the GUI window).
    /// * `initial_target` – Optional pre‑selected target from CLI.
    /// * `preserve_aspect` – Initial aspect ratio preservation preference.
    pub fn new(
        wintab_context: HCTX,
        base_context: LOGCONTEXTA,
        final_options: u32,
        host_window: HWND,
        initial_target: Option<Target>,
        preserve_aspect: bool,
    ) -> Self {
        Self {
            wintab_context: Arc::new(Mutex::new(wintab_context)),
            base_context,
            final_options,
            current_target: Arc::new(Mutex::new(initial_target)),
            preserve_aspect: AtomicBool::new(preserve_aspect),
            host_window: SendHwnd(host_window),
        }
    }

    /// Get current mapping configuration (cheap copy of user‑controlled flags).
    pub fn get_mapping_config(&self) -> MapConfig {
        MapConfig {
            keep_aspect: self.preserve_aspect.load(Ordering::Relaxed),
        }
    }

    /// Update aspect ratio setting.
    ///
    /// This is atomic so GUI checkbox toggles can mutate the flag without contending
    /// on any other shared mutex.
    pub fn set_preserve_aspect(&self, enabled: bool) {
        self.preserve_aspect.store(enabled, Ordering::Relaxed);
    }

    /// Get current target (if any).
    ///
    /// Returns `None` if the mutex is poisoned or no target has been set.
    pub fn get_current_target(&self) -> Option<Target> {
        self.current_target.lock().ok()?.clone()
    }

    /// Set a new target (replacing any previously stored one).
    pub fn set_current_target(&self, target: Option<Target>) {
        if let Ok(mut guard) = self.current_target.lock() {
            *guard = target;
        }
    }

    /// Check whether a target has been configured.
    pub fn has_target(&self) -> bool {
        self.current_target
            .lock()
            .ok()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }
}
