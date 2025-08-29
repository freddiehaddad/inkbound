//! WinEvent hook management and target window filtering.
//!
//! Provides a minimal abstraction over SetWinEventHook for a bounded list of event types that
//! influence tablet mapping: creation, visibility, location/size changes, foreground switches
//! and minimize transitions. A user‑supplied callback (Arc) is invoked for every matching event
//! affecting the configured target window. Non‑target foreground changes can optionally trigger
//! a synthetic reset callback (zero RECT) enabling temporary full‑tablet mapping.

use anyhow::{Result, anyhow};
use once_cell::sync::OnceCell;
use std::sync::{Arc, Mutex};
use tracing::debug;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_CREATE, EVENT_OBJECT_DESTROY, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_SHOW,
    EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, GA_ROOT,
    GW_HWNDNEXT, GetAncestor, GetClassNameW, GetDesktopWindow, GetForegroundWindow, GetWindow,
    GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
};

/// Window matching strategy (mutually exclusive CLI selectors).
pub enum Target {
    ByProcessName(String),
    ByWindowClass(String),
    ByTitleSubstring(String),
}
/// Static filter configuration applied to all installed hooks.
pub struct HookFilter {
    pub target: Target,
    pub reset_on_foreground_loss: bool,
}

/// User callback signature: (window handle, event id, rectangle).
pub type WinEventCallback = dyn Fn(HWND, u32, RECT) + Send + Sync + 'static;

static CALLBACK: OnceCell<Arc<WinEventCallback>> = OnceCell::new();
static FILTER: OnceCell<HookFilter> = OnceCell::new();
// Wrapper for hook handle so we can mark it Send/Sync (the handle is only used on the creating thread).
#[derive(Copy, Clone)]
struct HookHandle(HWINEVENTHOOK);
unsafe impl Send for HookHandle {}
unsafe impl Sync for HookHandle {}

static HOOKS: OnceCell<Mutex<Vec<HookHandle>>> = OnceCell::new();
// Delay logic removed; sequence no longer needed.

/// Raw WinEvent callback (FFI boundary). Performs filtering and rectangle acquisition then
/// dispatches to the registered safe Rust closure.
unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if hwnd.is_invalid() || id_object != 0 {
        return;
    }
    // Root / visible checks
    unsafe {
        if GetAncestor(hwnd, GA_ROOT) != hwnd {
            return;
        }
        if !IsWindowVisible(hwnd).as_bool() {
            return;
        }
    }
    let filter = match FILTER.get() {
        Some(f) => f,
        None => return,
    };
    let is_foreground = event == EVENT_SYSTEM_FOREGROUND;
    let is_match = matches_target(hwnd, &filter.target);
    if !is_match {
        if is_foreground && filter.reset_on_foreground_loss {
            if let Some(cb) = CALLBACK.get() {
                cb(hwnd, event, RECT::default()); // synthetic reset trigger (zero rect)
            }
        }
        return;
    }
    let mut rect = RECT::default();
    let ok_dwm = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok()
    };
    if !ok_dwm {
        unsafe {
            if !GetWindowRect(hwnd, &mut rect).is_ok() {
                return;
            }
        }
    }
    // (Debounce removed – every LOCATIONCHANGE generates a callback)
    if let Some(cb) = CALLBACK.get() {
        cb(hwnd, event, rect);
    }
}

/// Utility: read a UTF‑16 string via a provided fill closure returning number of u16 written.
fn read_wstr<F: FnOnce(&mut [u16]) -> i32>(cap: usize, fill: F) -> String {
    let mut buf = vec![0u16; cap];
    let len = fill(&mut buf) as usize;
    let slice = &buf[..buf.iter().position(|&c| c == 0).unwrap_or(len)];
    String::from_utf16_lossy(slice)
}

/// Determine whether `hwnd` satisfies the configured Target strategy.
fn matches_target(hwnd: HWND, target: &Target) -> bool {
    match target {
        Target::ByWindowClass(expected) => {
            let class = read_wstr(256, |b| unsafe { GetClassNameW(hwnd, b) });
            &class == expected
        }
        Target::ByTitleSubstring(substr) => {
            let title = read_wstr(512, |b| unsafe { GetWindowTextW(hwnd, b) });
            title.contains(substr)
        }
        Target::ByProcessName(name) => {
            // Resolve process name for hwnd
            let mut pid: u32 = 0;
            unsafe {
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
            }
            if pid == 0 {
                return false;
            }
            if let Some(actual) = process_name_from_pid(pid) {
                actual.eq_ignore_ascii_case(name)
            } else {
                false
            }
        }
    }
}

/// Resolve process executable name for a PID using ToolHelp snapshot enumeration.
fn process_name_from_pid(pid: u32) -> Option<String> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..std::mem::zeroed()
        };
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                if entry.th32ProcessID == pid {
                    // Convert exe file name
                    let raw = &entry.szExeFile;
                    let len = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
                    let slice = &raw[..len];
                    let s = String::from_utf16_lossy(slice);
                    return Some(s);
                }
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
    }
    None
}

/// Install WinEvent hooks for the fixed event set.
///
/// Stores filter + callback in OnceCell singletons (subsequent calls fail). Partial hook
/// installation is tolerated; failures are logged but not escalated to the caller.
pub fn install_hooks(filter: HookFilter, cb: Arc<WinEventCallback>) -> Result<()> {
    CALLBACK
        .set(cb)
        .map_err(|_| anyhow!("callback already set"))?;
    FILTER
        .set(filter)
        .map_err(|_| anyhow!("filter already set"))?;
    let events = [
        EVENT_OBJECT_SHOW,
        EVENT_OBJECT_CREATE,
        EVENT_OBJECT_DESTROY,
        EVENT_SYSTEM_FOREGROUND,
        EVENT_OBJECT_LOCATIONCHANGE,
        EVENT_SYSTEM_MINIMIZESTART,
        EVENT_SYSTEM_MINIMIZEEND,
    ];
    unsafe {
        HOOKS.set(Mutex::new(Vec::new())).ok();
        let mut any_fail = false;
        for &ev in &events {
            let h = SetWinEventHook(ev, ev, None, Some(win_event_proc), 0, 0, 0);
            if h.0.is_null() {
                any_fail = true;
                debug!(event = ev, "failed to install hook");
            } else {
                debug!(event = ev, ?h, "hook installed");
                if let Some(list) = HOOKS.get() {
                    list.lock().unwrap().push(HookHandle(h));
                }
            }
        }
        if any_fail {
            // We proceed even if some hooks failed; caller can decide whether partial coverage is acceptable.
        }
    }
    Ok(())
}

/// Unregister all installed hooks (idempotent).
pub fn uninstall_hooks() {
    if let Some(list) = HOOKS.get() {
        for HookHandle(h) in list.lock().unwrap().drain(..) {
            unsafe {
                let _ = UnhookWinEvent(h);
            }
        }
    }
}

/// Attempt to find an existing window matching the target criteria (foreground first, else enumerate).
/// Attempt to locate an existing target window prior to receiving events.
///
/// Checks current foreground first for faster startup then walks top‑level windows in z‑order.
pub fn find_existing_target() -> Option<HWND> {
    let filter = FILTER.get()?;
    unsafe {
        let fg = GetForegroundWindow();
        if !fg.is_invalid() && matches_target(fg, &filter.target) {
            return Some(fg);
        }
        // Walk top-level windows in z-order.
        let mut current = GetWindow(GetDesktopWindow(), GW_HWNDNEXT).ok();
        let mut count = 0;
        while let Some(h) = current {
            if IsWindowVisible(h).as_bool() && GetAncestor(h, GA_ROOT) == h {
                if matches_target(h, &filter.target) {
                    return Some(h);
                }
            }
            current = GetWindow(h, GW_HWNDNEXT).ok();
            count += 1;
            if count > 4096 {
                break;
            }
        }
    }
    None
}

/// Retrieve a RECT for a window using DWM frame bounds fallback to GetWindowRect.
/// Retrieve a window rectangle using extended frame bounds with a GetWindowRect fallback.
pub fn query_window_rect(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok()
            || GetWindowRect(hwnd, &mut rect).is_ok()
    };
    if ok { Some(rect) } else { None }
}
