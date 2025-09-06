//! Visible GUI window for lifecycle control and status display.
//!
//! This module provides the main GUI window that serves as both the user interface
//! and the WinTab context host, eliminating the need for a separate hidden window.

use anyhow::{Result, anyhow};
use once_cell::sync::OnceCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use widestring::U16CString;

/// Selector type for radio button state
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SelectorType {
    Process,
    WindowClass,
    Title,
}
use crate::events::{EventSeverity, UiEvent, format_event_line, push_rate_limited, push_ui_event};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWINDOWATTRIBUTE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_BITFIELDS, BITMAPINFO, BITMAPV5HEADER, COLOR_WINDOW, CreateBitmap, CreateDIBSection,
    CreateFontIndirectW, DIB_RGB_COLORS, DeleteObject, FW_NORMAL, GetSysColorBrush, HBITMAP, HDC,
    HFONT, HGDIOBJ, LOGFONTW, SetBkMode, TRANSPARENT,
};
use windows::Win32::UI::Controls::SetWindowTheme;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreatePopupMenu,
    CreateWindowExW, DefWindowProcW, DestroyIcon, ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE,
    ES_READONLY, GetClientRect, GetCursorPos, GetWindowTextLengthW, HMENU, MF_STRING, MoveWindow,
    PostQuitMessage, RegisterClassW, SIZE_MINIMIZED, SW_HIDE, SW_SHOW, SetWindowTextW, ShowWindow,
    TPM_BOTTOMALIGN, TPM_LEFTALIGN, TrackPopupMenu, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE,
    WM_COMMAND, WM_DESTROY, WM_PAINT, WM_SIZE, WNDCLASSW, WS_CHILD, WS_EX_CLIENTEDGE,
    WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VSCROLL,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BM_SETCHECK, BS_AUTORADIOBUTTON, SendMessageW, WS_GROUP,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};
use windows::core::PCWSTR;
// removed SetWindowTheme usage; w! macro no longer needed

/// Centralized GUI state container to replace the previous ad‑hoc global statics.
///
/// All window/control handles are stored as `AtomicIsize` to allow cheap cross‑thread reads
/// (for example from callbacks) without additional locking. The GUI is still created and
/// manipulated on the main thread; we only read identifiers elsewhere.
pub struct GuiState {
    /// Window class name (cached once)
    main_class: OnceCell<U16CString>,
    /// Handle to the selector edit control (stored as isize for thread safety)
    selector_edit: AtomicIsize,
    /// Handle to the start/stop button (stored as isize for thread safety)
    start_stop_button: AtomicIsize,
    /// Handle to the main visible window (stored as isize for thread safety)
    visible_main: AtomicIsize,
    /// Radio button handles for selector type (stored as isize for thread safety)
    radio_process: AtomicIsize,
    radio_class: AtomicIsize,
    radio_title: AtomicIsize,
    /// User-controlled Start/Stop state (true = mapping active/desirable)
    ///
    /// Atomic ordering rationale:
    /// These atomic flags (`run_enabled`, `target_present`, plus handle fields) are read from
    /// auxiliary threads for quick status checks (e.g. event / hook callbacks) while all writes
    /// occur on the GUI thread. There is no cross-field invariant that depends on relative
    /// ordering of writes, and readers only need the most recently observed value – not a
    /// consistent snapshot across multiple fields. Therefore `Ordering::Relaxed` is sufficient
    /// everywhere they are accessed. If in the future a reader observes one flag and then makes
    /// decisions contingent on another flag's freshly written value, we can upgrade the write
    /// to `Release` and the read to `Acquire` (or wrap the group in a `Mutex`). For now keeping
    /// them relaxed avoids superfluous fences on the fast path.
    run_enabled: AtomicBool,
    /// Whether the target window currently exists (for tray icon coloring). See ordering note
    /// above for justification of relaxed loads/stores.
    target_present: AtomicBool,
    /// Optional callback invoked whenever the user toggles Start/Stop
    run_toggle_cb: OnceCell<Arc<dyn Fn(bool) + Send + Sync>>,
    /// Callback for aspect ratio toggle
    aspect_toggle_cb: OnceCell<Arc<dyn Fn(AspectMode) + Send + Sync>>,
    /// Handle to the events feed edit control
    events_edit: AtomicIsize,
    wait_timer_active: AtomicBool,
    selector_label: AtomicIsize,
    radio_letterbox: AtomicIsize, // letterbox radio
    radio_stretch: AtomicIsize,   // stretch radio
    /// RAII guard ensuring the tray icon is removed if initialization succeeded and code paths
    /// forget to explicitly delete it (e.g. early panic or future refactors). The guard lives
    /// for the program lifetime; explicit removal on user exit remains for immediate UX.
    tray_icon_guard: OnceCell<TrayIconGuard>,
}

impl GuiState {
    /// Create a new GUI state instance with default values
    pub fn new() -> Self {
        Self {
            main_class: OnceCell::new(),
            selector_edit: AtomicIsize::new(0),
            start_stop_button: AtomicIsize::new(0),
            visible_main: AtomicIsize::new(0),
            radio_process: AtomicIsize::new(0),
            radio_class: AtomicIsize::new(0),
            radio_title: AtomicIsize::new(0),
            run_enabled: AtomicBool::new(false),
            target_present: AtomicBool::new(false),
            run_toggle_cb: OnceCell::new(),
            aspect_toggle_cb: OnceCell::new(),
            events_edit: AtomicIsize::new(0),
            wait_timer_active: AtomicBool::new(false),
            selector_label: AtomicIsize::new(0),
            radio_letterbox: AtomicIsize::new(0),
            radio_stretch: AtomicIsize::new(0),
            tray_icon_guard: OnceCell::new(),
        }
    }
}

// Thread-safe singleton for GUI state (temporary during transition)
static GUI_STATE: OnceCell<GuiState> = OnceCell::new();

/// Get or initialize the GUI state singleton
fn get_gui_state() -> &'static GuiState {
    GUI_STATE.get_or_init(GuiState::new)
}

/// Helper: load an HWND from an AtomicIsize (0 => None)
#[inline(always)]
fn load_hwnd(atom: &AtomicIsize) -> Option<HWND> {
    let raw = atom.load(Ordering::Relaxed);
    if raw == 0 {
        None
    } else {
        Some(HWND(raw as *mut _))
    }
}
const ID_START_STOP: usize = 2001;
const ID_RADIO_ASPECT_LETTERBOX: usize = 2101; // letterbox aspect radio id
const ID_RADIO_ASPECT_STRETCH: usize = 2102; // stretch radio id
const ID_RADIO_PROCESS: usize = 2201;
const ID_RADIO_CLASS: usize = 2202;
const ID_RADIO_TITLE: usize = 2203;
const WM_TRAYICON: u32 = 0x0400 + 1; // custom message id
const IDM_TRAY_RESTORE: usize = 1001;
const IDM_TRAY_EXIT: usize = 1002;
const IDM_TRAY_TOGGLE: usize = 1003; // dynamic Start/Stop
const TRAY_UID: u32 = 1;
const WAIT_TIMER_ID: usize = 0x9001;

/// Public status variants (currently only color coded square icons).
#[allow(dead_code)]
pub enum TrayStatus {
    Yellow,
    Green,
    Red,
}

// No global caching of icons; created on demand (low overhead, 16x16).

unsafe fn create_color_icon(r: u8, g: u8, b: u8) -> Option<HICON> {
    // 16x16 ARGB DIB section.
    let mut hdr: BITMAPV5HEADER = unsafe { std::mem::zeroed() };
    hdr.bV5Size = std::mem::size_of::<BITMAPV5HEADER>() as u32;
    hdr.bV5Width = 16;
    hdr.bV5Height = 16; // bottom-up
    hdr.bV5Planes = 1;
    hdr.bV5BitCount = 32;
    hdr.bV5Compression = BI_BITFIELDS; // enum value
    // RGBA channel masks
    hdr.bV5RedMask = 0x00FF0000;
    hdr.bV5GreenMask = 0x0000FF00;
    hdr.bV5BlueMask = 0x000000FF;
    hdr.bV5AlphaMask = 0xFF000000;
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let hbm_color = unsafe {
        CreateDIBSection(
            None,
            &hdr as *const _ as *const BITMAPINFO,
            DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        )
        .unwrap_or_default()
    };
    if hbm_color.0.is_null() || bits.is_null() {
        return None;
    }
    // Fill with solid color (premultiplied not required if alpha=255).
    let px = bits as *mut u32;
    let color = (0xFFu32 << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
    // Fill pixel buffer via slice instead of per‑index pointer arithmetic.
    unsafe {
        let slice = std::slice::from_raw_parts_mut(px, 16 * 16);
        slice.fill(color);
    }
    // Create a simple 1bpp mask (all zeros -> use alpha channel for shape).
    let hbm_mask = unsafe { CreateBitmap(16, 16, 1, 1, None) };
    if hbm_mask.0.is_null() {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(hbm_color.0));
        }
        return None;
    }
    let ii = ICONINFO {
        fIcon: true.into(),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: HBITMAP(hbm_mask.0),
        hbmColor: HBITMAP(hbm_color.0),
    };
    let hicon = unsafe { CreateIconIndirect(&ii).unwrap_or_default() };
    if hicon.0.is_null() {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(hbm_color.0));
            let _ = DeleteObject(HGDIOBJ(hbm_mask.0));
        }
        return None;
    }
    // Bitmaps can be deleted after icon creation.
    unsafe {
        let _ = DeleteObject(HGDIOBJ(hbm_color.0));
        let _ = DeleteObject(HGDIOBJ(hbm_mask.0));
    }
    Some(hicon)
}

unsafe fn status_icon(status: &TrayStatus) -> Option<HICON> {
    unsafe {
        match status {
            TrayStatus::Yellow => create_color_icon(255, 215, 0), // gold-ish
            TrayStatus::Green => create_color_icon(0, 170, 0),
            TrayStatus::Red => create_color_icon(200, 32, 32),
        }
    }
}

/// Update tray icon color (no-op if icon creation failed / not present yet).
#[allow(dead_code)]
pub fn set_tray_status(hwnd: HWND, status: TrayStatus) {
    unsafe {
        if let Some(hicon) = status_icon(&status) {
            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = hwnd;
            nid.uID = TRAY_UID;
            nid.uFlags = NIF_ICON;
            nid.hIcon = hicon;
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
            let _ = DestroyIcon(hicon); // shell copies the image; safe to destroy
        }
    }
}

/// Force tray icon into error (red) state. Does not modify internal run/target flags.
/// Subsequent normal state updates (e.g. mapping success / presence change) will overwrite it.
pub fn set_tray_error() {
    let hwnd = HWND(get_gui_state().visible_main.load(Ordering::Relaxed) as *mut _);
    if hwnd.0.is_null() {
        return;
    }
    set_tray_status(hwnd, TrayStatus::Red);
}

/// Compute the current toggle button label based on run state.
fn current_toggle_label() -> &'static str {
    let gui_state = get_gui_state();
    if gui_state.run_enabled.load(Ordering::Relaxed) {
        "Stop"
    } else {
        "Start"
    }
}

// Removed custom font management to show default system control fonts.

// Base logical metrics (96 DPI)
const BASE_MARGIN: i32 = 16;
const BASE_EDIT_HEIGHT: i32 = 24; // unify edit height with other controls for consistent vertical rhythm
// Removed absolute Y position constants; rows flow sequentially.
const BASE_BUTTON_HEIGHT: i32 = 32; // start/stop button height
const BASE_CONTROL_H: i32 = 24; // radio / label nominal height
const BASE_V_GAP: i32 = 12; // uniform vertical gap between rows
const BASE_RADIO_GAP: i32 = 16; // uniform horizontal gap between radios (text edge to next control)
const BASE_WINDOW_W: i32 = 600;
const BASE_WINDOW_H: i32 = 360;
const BASE_LABEL_MIN: i32 = 40; // min logical label width
const BASE_LABEL_MAX: i32 = 160; // max logical label width to avoid hogging row
const BASE_LABEL_PADDING: i32 = 12; // padding added to measured text (logical)
const BASE_LABEL_GAP: i32 = 8; // gap between label and textbox (logical)
/// Scale a logical (96‑DPI based) dimension to the current DPI with rounding.
fn scale(v: i32, dpi: u32) -> i32 {
    (v * dpi as i32 + 48) / 96
}
// Shared application font (recreated on DPI changes). Using Segoe UI 12pt logical.
static mut APP_FONT: HFONT = HFONT(0 as _);

unsafe fn recreate_font_for_dpi(dpi: u32) {
    unsafe {
        let mut lf: LOGFONTW = std::mem::zeroed();
        let pt: i32 = 12;
        lf.lfHeight = -((pt * dpi as i32 + 36) / 72);
        lf.lfWeight = FW_NORMAL.0 as i32;
        let face = U16CString::from_str("Segoe UI").unwrap();
        // Copy face name (truncating automatically) using zip for clarity.
        for (dst, src) in lf
            .lfFaceName
            .iter_mut()
            .zip(face.as_slice_with_nul().iter())
        {
            *dst = *src;
        }
        let f = CreateFontIndirectW(&lf);
        if !f.0.is_null() {
            if !APP_FONT.0.is_null() {
                let _ = DeleteObject(HGDIOBJ(APP_FONT.0));
            }
            APP_FONT = f;
        }
    }
}

fn apply_font(h: HWND) {
    unsafe {
        const WM_SETFONT: u32 = 0x0030;
        if !APP_FONT.0.is_null() {
            let _ = SendMessageW(
                h,
                WM_SETFONT,
                Some(WPARAM(APP_FONT.0 as usize)),
                Some(LPARAM(1)),
            );
        }
    }
}
/// Perform responsive layout for horizontally stretching controls.
///
/// Called on `WM_SIZE` and after window creation / DPI changes. The calculation is deliberately
/// minimal: we derive available width once per pass and guard against pathological (very small)
/// client rectangles.
fn layout_controls(hwnd: HWND, dpi: u32) {
    let gs = get_gui_state();
    unsafe {
        let margin = scale(BASE_MARGIN, dpi);
        let gap = scale(BASE_V_GAP, dpi);
        // Dynamically measure (approximate) selector label width based on its text length.
        let mut label_w = scale(90, dpi); // fallback
        let label_spacing = scale(BASE_LABEL_GAP, dpi);
        let lab_handle_val = gs.selector_label.load(Ordering::Relaxed);
        if lab_handle_val != 0 {
            let wh = HWND(lab_handle_val as *mut _);
            let txt_len = GetWindowTextLengthW(wh) as i32; // character count
            if txt_len > 0 {
                // Approximate character width at 96 DPI (~7px) then scale; add padding.
                let logical_w =
                    (txt_len * 7 + BASE_LABEL_PADDING).clamp(BASE_LABEL_MIN, BASE_LABEL_MAX);
                label_w = scale(logical_w, dpi);
            }
        }
        let edit_h = scale(BASE_EDIT_HEIGHT, dpi);
        let ctrl_h = scale(BASE_CONTROL_H, dpi);
        let btn_h = scale(BASE_BUTTON_HEIGHT, dpi);

        let mut rc = RECT::default();
        if GetClientRect(hwnd, &mut rc).is_err() {
            return;
        }
        let total_width = (rc.right - rc.left) - margin * 2;
        if total_width <= 40 {
            return;
        }

        let mut y = margin; // start below top margin

        // Row 1: label + edit (uniform height)
        let lab = lab_handle_val;
        if lab != 0 {
            let _ = MoveWindow(HWND(lab as *mut _), margin, y, label_w, ctrl_h, true);
        }
        let e = gs.selector_edit.load(Ordering::Relaxed);
        if e != 0 {
            let edit_x = margin + label_w + label_spacing;
            let avail_w = total_width - label_w - label_spacing;
            let final_w = avail_w.max(scale(80, dpi));
            let _ = MoveWindow(HWND(e as *mut _), edit_x, y, final_w, edit_h, true);
        }
        y += ctrl_h + gap;

        // Row 2: radios auto-sized approximately to text (char count heuristic) with uniform gap
        let radios = [
            gs.radio_process.load(Ordering::Relaxed),
            gs.radio_class.load(Ordering::Relaxed),
            gs.radio_title.load(Ordering::Relaxed),
        ];
        let gap_x = scale(BASE_RADIO_GAP, dpi);
        let mut x = margin;
        for h in radios.iter() {
            if *h == 0 {
                continue;
            }
            let wh = HWND(*h as *mut _);
            // Length of text
            let len = GetWindowTextLengthW(wh);
            // Approx width: 7px per char + 20px padding for radio circle & spacing
            let logical = (len * 7 + 20).max(48);
            let w_px = scale(logical, dpi);
            if x + w_px > margin + total_width {
                break;
            }
            let _ = MoveWindow(wh, x, y, w_px, ctrl_h, true);
            x += w_px + gap_x;
        }
        y += ctrl_h + gap;

        // Row 3: aspect radios (Letterbox / Stretch)
        let aspect_radios = [
            gs.radio_letterbox.load(Ordering::Relaxed),
            gs.radio_stretch.load(Ordering::Relaxed),
        ];
        if aspect_radios.iter().any(|h| *h != 0) {
            let mut x2 = margin;
            let gap_aspect = gap_x; // reuse radio gap spacing
            for h in aspect_radios {
                if h == 0 {
                    continue;
                }
                let wh = HWND(h as *mut _);
                let len = GetWindowTextLengthW(wh);
                let logical = (len * 7 + 28).max(60); // a bit wider for readability
                let w_px = scale(logical, dpi);
                if x2 + w_px > margin + total_width {
                    break;
                }
                let _ = MoveWindow(wh, x2, y, w_px, ctrl_h, true);
                x2 += w_px + gap_aspect;
            }
            y += ctrl_h + gap;
        }

        // Row 4: start/stop button
        let b = gs.start_stop_button.load(Ordering::Relaxed);
        if b != 0 {
            let _ = MoveWindow(HWND(b as *mut _), margin, y, total_width, btn_h, true);
        }
        y += btn_h + gap;

        // Events panel fills remainder
        let ev = gs.events_edit.load(Ordering::Relaxed);
        if ev != 0 {
            let mut rc2 = RECT::default();
            if GetClientRect(hwnd, &mut rc2).is_ok() {
                let remaining_h = (rc2.bottom - rc2.top) - y - margin;
                let min_h = scale(60, dpi);
                let final_h = if remaining_h < min_h {
                    min_h
                } else {
                    remaining_h
                };
                let _ = MoveWindow(HWND(ev as *mut _), margin, y, total_width, final_h, true);
            }
        }
    }
}

/// Flip the run enabled flag, update UI affordances, and invoke the registered callback.
fn perform_run_toggle() {
    let gui_state = get_gui_state();
    let new_state = !gui_state.run_enabled.load(Ordering::Relaxed);
    gui_state.run_enabled.store(new_state, Ordering::Relaxed);
    // Update button label if visible
    let btn_h = gui_state.start_stop_button.load(Ordering::Relaxed);
    if btn_h != 0
        && let Ok(w) = U16CString::from_str(if new_state { "Stop" } else { "Start" })
    {
        unsafe {
            let _ = SetWindowTextW(HWND(btn_h as *mut _), PCWSTR(w.as_ptr()));
        }
    }
    update_tray_icon_for_state();
    if let Some(cb) = gui_state.run_toggle_cb.get() {
        cb(new_state);
    }
}

/// Add (or replace) the system tray icon with the default (yellow) variant.
unsafe fn add_tray_icon(hwnd: HWND) {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_UID;
    nid.uFlags = NIF_MESSAGE | NIF_TIP | NIF_ICON;
    nid.uCallbackMessage = WM_TRAYICON;
    // Ensure custom icons exist and pick yellow as default; fall back to system if creation failed.
    let hicon = unsafe { status_icon(&TrayStatus::Yellow) }.unwrap_or_default();
    nid.hIcon = hicon;
    let tip = U16CString::from_str("InkBound Mapper").unwrap();
    let slice = tip.as_slice_with_nul();
    // Copy tooltip text (truncated if longer than buffer)
    for (dst, &src) in nid.szTip.iter_mut().zip(slice.iter()) {
        *dst = src;
    }
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }
    // Install RAII guard once (idempotent if called again for refresh scenarios).
    let gs = get_gui_state();
    let _ = gs.tray_icon_guard.set(TrayIconGuard { hwnd });
}

/// Remove the tray icon (idempotent if not present).
unsafe fn remove_tray_icon(hwnd: HWND) {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_UID;
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
    // Icon resources created for the tray are destroyed individually when updated/removed.
}

/// RAII guard that removes the tray icon on drop (best-effort / idempotent).
struct TrayIconGuard {
    hwnd: HWND,
}

impl Drop for TrayIconGuard {
    fn drop(&mut self) {
        unsafe { remove_tray_icon(self.hwnd) };
    }
}

// SAFETY: The guard performs only an idempotent Shell_NotifyIconW deletion in Drop; HWND handle
// value is copied and not mutated concurrently. Multiple drops (including OS process teardown)
// are harmless.
unsafe impl Send for TrayIconGuard {}
unsafe impl Sync for TrayIconGuard {}

/// Show the context menu for the tray icon at the current cursor location.
unsafe fn show_tray_menu(hwnd: HWND) {
    let hmenu = unsafe {
        match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => return,
        }
    };
    if let Ok(title) = U16CString::from_str("Restore") {
        unsafe {
            let _ = AppendMenuW(hmenu, MF_STRING, IDM_TRAY_RESTORE, PCWSTR(title.as_ptr()));
        }
    }
    if let Ok(title) = U16CString::from_str(current_toggle_label()) {
        unsafe {
            let _ = AppendMenuW(hmenu, MF_STRING, IDM_TRAY_TOGGLE, PCWSTR(title.as_ptr()));
        }
    }
    if let Ok(title) = U16CString::from_str("Exit") {
        unsafe {
            let _ = AppendMenuW(hmenu, MF_STRING, IDM_TRAY_EXIT, PCWSTR(title.as_ptr()));
        }
    }
    let mut pt: POINT = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut pt).is_ok() } {
        let _ = unsafe {
            TrackPopupMenu(
                hmenu,
                TPM_LEFTALIGN | TPM_BOTTOMALIGN,
                pt.x,
                pt.y,
                Some(0),
                hwnd,
                None::<*const RECT>,
            )
        };
    }
}

unsafe extern "system" fn main_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        WM_SIZE => unsafe {
            if wparam.0 == SIZE_MINIMIZED as usize {
                let _ = ShowWindow(hwnd, SW_HIDE);
            } else {
                let dpi = GetDpiForWindow(hwnd);
                layout_controls(hwnd, dpi);
            }
            LRESULT(0)
        },
        // WM_CTLCOLORSTATIC (0x0138) -> labels transparent; events EDIT opaque (avoid ClearType bold overdraw)
        // Removed WM_CTLCOLORSTATIC customization to let default theming draw controls.
        // (0x0135 WM_CTLCOLORBTN falls through to default proc)
        0x02E0 => unsafe {
            // WM_DPICHANGED
            let new_dpi = (wparam.0 & 0xFFFF) as u32;
            let suggested = lparam.0 as *const RECT;
            if !suggested.is_null() {
                let r = *suggested;
                let _ = MoveWindow(
                    hwnd,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    true,
                );
            }
            recreate_font_for_dpi(new_dpi);
            // Reapply font to all controls
            let gs = get_gui_state();
            for atom in [
                &gs.selector_label,
                &gs.selector_edit,
                &gs.radio_process,
                &gs.radio_class,
                &gs.radio_title,
                &gs.radio_letterbox,
                &gs.radio_stretch,
                &gs.start_stop_button,
                &gs.events_edit,
            ] {
                if let Some(h) = load_hwnd(atom) {
                    apply_font(h);
                }
            }
            layout_controls(hwnd, new_dpi);
            push_ui_event(EventSeverity::Info, format!("DPI change: {new_dpi}"));
            LRESULT(0)
        },
        0x0138 | 0x0135 => {
            // WM_CTLCOLORSTATIC / WM_CTLCOLORBTN
            unsafe {
                let ctrl = lparam.0;
                let gs = get_gui_state();
                let targets = [
                    gs.selector_label.load(Ordering::Relaxed),
                    gs.radio_process.load(Ordering::Relaxed),
                    gs.radio_class.load(Ordering::Relaxed),
                    gs.radio_title.load(Ordering::Relaxed),
                    gs.radio_letterbox.load(Ordering::Relaxed),
                    gs.radio_stretch.load(Ordering::Relaxed),
                ];
                if targets.contains(&ctrl) {
                    let hdc = HDC(wparam.0 as *mut _);
                    if !hdc.0.is_null() {
                        let _ = SetBkMode(hdc, TRANSPARENT);
                    }
                    let brush = GetSysColorBrush(COLOR_WINDOW);
                    return LRESULT(brush.0 as isize);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        0x0113 => {
            // WM_TIMER
            let timer_id = wparam.0;
            if timer_id == WAIT_TIMER_ID && is_run_enabled() && !is_target_present() {
                use std::time::Duration;
                push_rate_limited(
                    "wait_target",
                    Duration::from_secs(5),
                    EventSeverity::Info,
                    "Waiting for target...",
                );
            }
            LRESULT(0)
        }
        WM_CLOSE => unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        },
        WM_COMMAND => unsafe {
            match wparam.0 & 0xFFFF {
                IDM_TRAY_RESTORE => {
                    let _ = ShowWindow(hwnd, SW_SHOW);
                    LRESULT(0)
                }
                IDM_TRAY_EXIT => {
                    // Immediate icon removal for UX (in addition to RAII guarantee at shutdown).
                    remove_tray_icon(hwnd);
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                ID_START_STOP => {
                    perform_run_toggle();
                    LRESULT(0)
                }
                IDM_TRAY_TOGGLE => {
                    perform_run_toggle();
                    LRESULT(0)
                }
                ID_RADIO_ASPECT_LETTERBOX | ID_RADIO_ASPECT_STRETCH => {
                    // Determine selected aspect mode directly from control ID.
                    let mode = if (wparam.0 & 0xFFFF) == ID_RADIO_ASPECT_LETTERBOX {
                        AspectMode::Letterbox
                    } else {
                        AspectMode::Stretch
                    };
                    // Manually enforce mutual exclusivity (style already radios, but we ensure state).
                    const BM_SETCHECK: u32 = 0x00F1;
                    const BST_CHECKED: usize = 1;
                    let gs = get_gui_state();
                    let (this_handle, other_handle) = if matches!(mode, AspectMode::Letterbox) {
                        (
                            gs.radio_letterbox.load(Ordering::Relaxed),
                            gs.radio_stretch.load(Ordering::Relaxed),
                        )
                    } else {
                        (
                            gs.radio_stretch.load(Ordering::Relaxed),
                            gs.radio_letterbox.load(Ordering::Relaxed),
                        )
                    };
                    if this_handle != 0 {
                        let _ = SendMessageW(
                            HWND(this_handle as *mut _),
                            BM_SETCHECK,
                            Some(WPARAM(BST_CHECKED)),
                            Some(LPARAM(0)),
                        );
                    }
                    if other_handle != 0 {
                        let _ = SendMessageW(
                            HWND(other_handle as *mut _),
                            BM_SETCHECK,
                            Some(WPARAM(0)),
                            Some(LPARAM(0)),
                        );
                    }
                    if let Some(cb) = get_gui_state().aspect_toggle_cb.get() {
                        cb(mode);
                    }
                    LRESULT(0)
                }
                ID_RADIO_PROCESS | ID_RADIO_CLASS | ID_RADIO_TITLE => {
                    // Radio button clicked - no special handling needed, just let it update selection
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        },
        m if m == WM_TRAYICON => unsafe {
            let mouse_msg = lparam.0 as u32;
            match mouse_msg {
                0x0205 => {
                    show_tray_menu(hwnd);
                    LRESULT(0)
                } // WM_RBUTTONUP
                0x0203 => {
                    let _ = ShowWindow(hwnd, SW_SHOW);
                    LRESULT(0)
                } // WM_LBUTTONDBLCLK
                _ => LRESULT(0),
            }
        },
        WM_DESTROY => unsafe {
            // Ensure timer cleaned up
            stop_wait_timer();
            // Font cleanup no longer required (default fonts in use)
            // no dark brush cleanup needed
            PostQuitMessage(0);
            LRESULT(0)
        },
        0x0024 => {
            // WM_GETMINMAXINFO
            use windows::Win32::UI::WindowsAndMessaging::MINMAXINFO;
            let mmi = lparam.0 as *mut MINMAXINFO;
            if !mmi.is_null() {
                unsafe {
                    let dpi = GetDpiForWindow(hwnd);
                    (*mmi).ptMinTrackSize.x = scale(BASE_WINDOW_W * 6 / 10, dpi) as i32;
                    (*mmi).ptMinTrackSize.y = scale(BASE_WINDOW_H * 6 / 10, dpi) as i32;
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Register the main window class (once) and return its UTF‑16 name.
fn register_main_class() -> Result<&'static U16CString> {
    get_gui_state().main_class.get_or_try_init(|| {
        let name = U16CString::from_str("InkBoundWindow")?;
        unsafe {
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(main_wnd_proc),
                lpszClassName: PCWSTR(name.as_ptr()),
                hbrBackground: GetSysColorBrush(COLOR_WINDOW),
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                return Err(anyhow!("RegisterClassW failed"));
            }
        }
        Ok(name)
    })
}

/// Create a visible overlapped window (no controls yet). Closing it posts WM_QUIT.
/// Create the underlying overlapped window (no child controls yet) and set up tray + font.
fn create_raw_main_window(title: &str) -> Result<HWND> {
    let class = register_main_class()?;
    let title_u16 = U16CString::from_str(title)?;
    unsafe {
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR(title_u16.as_ptr()),
            WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            BASE_WINDOW_W,
            BASE_WINDOW_H,
            None,
            None,
            None,
            None,
        )?;
        let dpi = GetDpiForWindow(hwnd) as u32;
        // Apply light-mode + Mica backdrop (Win11). These calls are best-effort; failures are ignored on older builds.
        // Apply Mica + force light mode (best-effort; ignore failures on unsupported systems)
        const DWMWA_USE_IMMERSIVE_DARK_MODE: DWMWINDOWATTRIBUTE = DWMWINDOWATTRIBUTE(20);
        const DWMWA_SYSTEMBACKDROP_TYPE: DWMWINDOWATTRIBUTE = DWMWINDOWATTRIBUTE(38); // 2 = Mica
        let dark_off: i32 = 0; // FALSE
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark_off as *const _ as *const _,
            std::mem::size_of::<i32>() as u32,
        );
        let mica_type: i32 = 2; // Mica
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &mica_type as *const _ as *const _,
            std::mem::size_of::<i32>() as u32,
        );
        get_gui_state()
            .visible_main
            .store(hwnd.0 as isize, Ordering::Relaxed);
        add_tray_icon(hwnd);
        layout_controls(hwnd, dpi); // initial layout before first paint
        Ok(hwnd)
    }
}

/// Show all child controls (after initial layout) to avoid pre-layout flicker / misplaced visuals.
fn show_child_controls(hwnd: HWND) {
    let gs = get_gui_state();
    let ids = [
        gs.selector_label.load(Ordering::Relaxed),
        gs.selector_edit.load(Ordering::Relaxed),
        gs.radio_process.load(Ordering::Relaxed),
        gs.radio_class.load(Ordering::Relaxed),
        gs.radio_title.load(Ordering::Relaxed),
        gs.radio_letterbox.load(Ordering::Relaxed),
        gs.radio_stretch.load(Ordering::Relaxed),
        gs.start_stop_button.load(Ordering::Relaxed),
        gs.events_edit.load(Ordering::Relaxed),
    ];
    for h in ids {
        if h != 0 {
            unsafe {
                let _ = ShowWindow(HWND(h as *mut _), SW_SHOW);
            }
        }
    }
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
}

/// Create the full GUI window (selector textbox, selector radios, aspect radios, start/stop button) in one call.
/// High‑level convenience to build the full GUI (text box, selector radios, aspect radios, button) in order.
pub fn create_main_window(
    title: &str,
    selector_label: &str,
    selector_value: &str,
    preserve_aspect: bool,
    selector_type: SelectorType,
    initial_run_enabled: bool,
) -> Result<HWND> {
    // Set initial run state before creating GUI
    get_gui_state()
        .run_enabled
        .store(initial_run_enabled, Ordering::Relaxed);

    let hwnd = create_raw_main_window(title)?;
    let _ = add_selector_textbox(hwnd, selector_label, selector_value);
    let _ = add_selector_radio_buttons(hwnd, selector_type);
    let _ = add_aspect_radios(hwnd, preserve_aspect);
    let _ = add_start_stop_button(hwnd, initial_run_enabled);
    let _ = add_events_panel(hwnd);
    unsafe {
        layout_controls(hwnd, GetDpiForWindow(hwnd));
    }
    show_child_controls(hwnd);
    Ok(hwnd)
}

/// Add a Start/Stop toggle button with initial caption based on run state.
/// Add the Start/Stop push button reflecting the initial run state.
pub fn add_start_stop_button(parent: HWND, initial_run_enabled: bool) -> Result<()> {
    unsafe {
        let caption_text = if initial_run_enabled { "Stop" } else { "Start" };
        let caption = U16CString::from_str(caption_text).unwrap();
        let hwnd_btn = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(U16CString::from_str("BUTTON").unwrap().as_ptr()),
            PCWSTR(caption.as_ptr()),
            WINDOW_STYLE(WS_CHILD.0 | WS_TABSTOP.0 | (BS_PUSHBUTTON as u32)),
            0,
            0,
            0,
            0,
            Some(parent),
            Some(HMENU(ID_START_STOP as *mut _)),
            None,
            None,
        );
        if let Ok(hb) = hwnd_btn {
            get_gui_state()
                .start_stop_button
                .store(hb.0 as isize, Ordering::Relaxed);
            let _ = SetWindowTheme(
                hb,
                PCWSTR(U16CString::from_str("Explorer").unwrap().as_ptr()),
                PCWSTR(std::ptr::null()),
            );
        }
    }
    Ok(())
}

/// Add the multiline read-only events panel (initially empty, resizes with window).
pub fn add_events_panel(parent: HWND) -> Result<()> {
    unsafe {
        // Initial placeholder geometry; will be stretched in layout_controls.
        let style = WINDOW_STYLE(
            WS_CHILD.0
                | WS_VSCROLL.0
                | (ES_MULTILINE as u32)
                | (ES_AUTOVSCROLL as u32)
                | (ES_READONLY as u32),
        );
        let hwnd_edit = CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_CLIENTEDGE.0),
            PCWSTR(U16CString::from_str("EDIT").unwrap().as_ptr()),
            PCWSTR(U16CString::from_str("").unwrap().as_ptr()),
            style,
            0,
            0,
            0,
            0,
            Some(parent),
            None,
            None,
            None,
        );
        if let Ok(h) = hwnd_edit {
            get_gui_state()
                .events_edit
                .store(h.0 as isize, Ordering::Relaxed);
        }
    }
    Ok(())
}

/// Append a formatted event line to the events panel (if present). Auto-scrolls to end.
pub fn append_event_line(ev: &UiEvent) {
    let Some(hwnd) = load_hwnd(&get_gui_state().events_edit) else {
        return;
    };
    let line = format!("{}\r\n", format_event_line(ev));
    // Select end
    unsafe {
        // Using explicit length instead of -1/-1 sentinel prevents occasional repaint glitches
        // where rapid successive EM_REPLACESEL calls appear to overdraw in place.
        // Sequence: length -> EM_SETSEL(len,len) -> EM_REPLACESEL -> EM_SCROLLCARET.
        use windows::Win32::UI::WindowsAndMessaging::GetWindowTextLengthW;
        const EM_SETSEL: u32 = 0x00B1; // (start,end)
        const EM_REPLACESEL: u32 = 0x00C2; // append selected
        const EM_SCROLLCARET: u32 = 0x00B7; // ensure caret/viewport scrolls
        let len = GetWindowTextLengthW(hwnd) as usize;
        let _ = SendMessageW(
            hwnd,
            EM_SETSEL,
            Some(WPARAM(len)),
            Some(LPARAM(len as isize)),
        );
        if let Ok(w) = U16CString::from_str(line) {
            let _ = SendMessageW(
                hwnd,
                EM_REPLACESEL,
                Some(WPARAM(1)),
                Some(LPARAM(w.as_ptr() as isize)),
            );
            let _ = SendMessageW(hwnd, EM_SCROLLCARET, Some(WPARAM(0)), Some(LPARAM(0)));
        }
    }
}

/// Start periodic waiting timer (5s rate-limited emission) if not already running.
pub fn start_wait_timer(hwnd: HWND) {
    unsafe {
        let gs = get_gui_state();
        if !gs.wait_timer_active.load(Ordering::Relaxed) {
            use windows::Win32::UI::WindowsAndMessaging::SetTimer;
            if SetTimer(Some(hwnd), WAIT_TIMER_ID, 1000, None) != 0 {
                gs.wait_timer_active.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// Stop waiting timer if active.
pub fn stop_wait_timer() {
    unsafe {
        let gs = get_gui_state();
        if gs.wait_timer_active.load(Ordering::Relaxed) {
            let hwnd = HWND(gs.visible_main.load(Ordering::Relaxed) as *mut _);
            if !hwnd.0.is_null() {
                use windows::Win32::UI::WindowsAndMessaging::KillTimer;
                let _ = KillTimer(Some(hwnd), WAIT_TIMER_ID);
            }
            gs.wait_timer_active.store(false, Ordering::Relaxed);
        }
    }
}

/// Public accessor for target_present flag.
pub fn is_target_present() -> bool {
    get_gui_state().target_present.load(Ordering::Relaxed)
}

/// Update tray icon + button label to reflect whether target window is active.
pub fn reflect_target_presence(main_hwnd: HWND, present: bool) {
    let gui_state = get_gui_state();
    gui_state.target_present.store(present, Ordering::Relaxed);
    update_tray_icon_for_state_with(main_hwnd);
    // Only update button label if target presence changed AND run is enabled (color may change independently).
    let btn_h = gui_state.start_stop_button.load(Ordering::Relaxed);
    if btn_h != 0 {
        let run = gui_state.run_enabled.load(Ordering::Relaxed);
        let label = if run { "Stop" } else { "Start" };
        if let Ok(caption) = U16CString::from_str(label) {
            unsafe {
                let _ = SetWindowTextW(HWND(btn_h as *mut _), PCWSTR(caption.as_ptr()));
            }
        }
    }
}

/// Convenience wrapper when we don't already have a window handle to update.
fn update_tray_icon_for_state() {
    update_tray_icon_for_state_with(HWND(std::ptr::null_mut()));
}

/// Update the tray icon to reflect (run_enabled, target_present) state tuple.
fn update_tray_icon_for_state_with(main_hwnd: HWND) {
    let gui_state = get_gui_state();
    let run = gui_state.run_enabled.load(Ordering::Relaxed);
    let present = gui_state.target_present.load(Ordering::Relaxed);
    // Updated color logic per UX: Red reserved for explicit error only (not automatic).
    // Green => enabled + present; Yellow => all other normal states (stopped, waiting, or target gone while running)
    let status = if run && present {
        TrayStatus::Green
    } else {
        TrayStatus::Yellow
    };
    let use_hwnd = if main_hwnd.is_invalid() {
        HWND(gui_state.visible_main.load(Ordering::Relaxed) as *mut _)
    } else {
        main_hwnd
    };
    set_tray_status(use_hwnd, status);
}

/// Register a callback to be invoked when Start/Stop is toggled. Ignored if already set.
/// Register the run toggle callback. Subsequent calls are ignored (first wins).
pub fn set_run_toggle_callback(cb: Arc<dyn Fn(bool) + Send + Sync>) {
    let _ = get_gui_state().run_toggle_cb.set(cb);
}

/// Query whether mapping is currently enabled (user wants mapping if target exists).
/// Return whether the user currently wants mapping active.
pub fn is_run_enabled() -> bool {
    get_gui_state().run_enabled.load(Ordering::Relaxed)
}

/// Add a simple labeled read‑only textbox displaying the selected target spec.
/// Add the inline label + editable textbox for specifying / editing the target selector value.
pub fn add_selector_textbox(parent: HWND, label: &str, value: &str) -> Result<()> {
    // Initial positions are placeholders; layout_controls will move & size.
    let label_w =
        U16CString::from_str(label).unwrap_or_else(|_| U16CString::from_str("Selector").unwrap());
    let value_w = U16CString::from_str(value).unwrap_or_else(|_| U16CString::from_str("").unwrap());
    unsafe {
        // Static label
        let label_x = 0;
        let label_y = 0; // placeholder
        let label_width = 0; // placeholder width; real size in layout
        let label_height = BASE_CONTROL_H;
        let textbox_x = 0;
        let textbox_y = 0; // placeholder
        let textbox_width = 0; // placeholder
        let textbox_height = BASE_EDIT_HEIGHT;

        let _h_static = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(U16CString::from_str("STATIC").unwrap().as_ptr()),
            PCWSTR(label_w.as_ptr()),
            WINDOW_STYLE(WS_CHILD.0 | 0x0000_0200), // SS_CENTERIMAGE for vertical centering
            label_x,
            label_y,
            label_width,
            label_height,
            Some(parent),
            None,
            None,
            None,
        );
        if let Ok(hs) = _h_static {
            get_gui_state()
                .selector_label
                .store(hs.0 as isize, Ordering::Relaxed);
            let _ = SetWindowTheme(
                hs,
                PCWSTR(U16CString::from_str("Explorer").unwrap().as_ptr()),
                PCWSTR(std::ptr::null()),
            );
        }
        // Edit box (always editable)
        let style = WINDOW_STYLE(WS_CHILD.0 | (ES_AUTOHSCROLL as u32));
        let h_edit = CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_CLIENTEDGE.0),
            PCWSTR(U16CString::from_str("EDIT").unwrap().as_ptr()),
            PCWSTR(value_w.as_ptr()),
            style,
            textbox_x,
            textbox_y,
            textbox_width, // initial width; resized on WM_SIZE
            textbox_height,
            Some(parent),
            None,
            None,
            None,
        );
        if let Ok(hwnd_edit) = h_edit {
            get_gui_state()
                .selector_edit
                .store(hwnd_edit.0 as isize, Ordering::Relaxed);
            let _ = SetWindowTheme(
                hwnd_edit,
                PCWSTR(U16CString::from_str("Explorer").unwrap().as_ptr()),
                PCWSTR(std::ptr::null()),
            );
        }
    }
    Ok(())
}

/// Add radio buttons for selector type selection.
/// Add horizontally laid-out radio buttons selecting the interpretation of the selector textbox.
pub fn add_selector_radio_buttons(parent: HWND, selected_type: SelectorType) -> Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{BS_AUTORADIOBUTTON, WS_GROUP};

    unsafe {
        // Helper to create a radio button
        let create_radio =
            |text: &str, x: i32, y: i32, id: usize, is_first: bool| -> Result<HWND> {
                let wstr = U16CString::from_str(text)?;
                let mut style = WINDOW_STYLE(WS_CHILD.0 | (BS_AUTORADIOBUTTON as u32));
                if is_first {
                    style = WINDOW_STYLE(style.0 | WS_GROUP.0); // First radio button starts a new group
                }
                let button_class = U16CString::from_str("BUTTON")?;
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    PCWSTR(button_class.as_ptr()),
                    PCWSTR(wstr.as_ptr()),
                    style,
                    x,
                    y,
                    80, // Slightly narrower width for horizontal layout
                    24, // taller to prevent text clipping
                    Some(parent),
                    None,
                    None,
                    Some(std::ptr::addr_of!(id) as *const _),
                )
                .map_err(|e| anyhow!("Failed to create radio button: {}", e))
            };

        // Create radio buttons horizontally
        // Initial Y placeholder 0; real position determined by layout_controls flow.
        let radio_process = create_radio("Process", 16, 0, ID_RADIO_PROCESS, true)?;
        let radio_class = create_radio("Class", 112, 0, ID_RADIO_CLASS, false)?;
        let radio_title = create_radio("Title", 192, 0, ID_RADIO_TITLE, false)?;
        for rb in [radio_process, radio_class, radio_title] {
            let _ = SetWindowTheme(
                rb,
                PCWSTR(U16CString::from_str("Explorer").unwrap().as_ptr()),
                PCWSTR(std::ptr::null()),
            );
        }

        // Store handles (compressed to a small loop for clarity)
        let gui_state = get_gui_state();
        for (cell, hwnd) in [
            (&gui_state.radio_process, radio_process),
            (&gui_state.radio_class, radio_class),
            (&gui_state.radio_title, radio_title),
        ] {
            cell.store(hwnd.0 as isize, Ordering::Relaxed);
        }

        // Select the appropriate radio button
        const BM_SETCHECK: u32 = 0x00F1;
        const BST_CHECKED: usize = 1;
        let selected_radio = match selected_type {
            SelectorType::Process => radio_process,
            SelectorType::WindowClass => radio_class,
            SelectorType::Title => radio_title,
        };
        let _ = SendMessageW(
            selected_radio,
            BM_SETCHECK,
            Some(WPARAM(BST_CHECKED)),
            Some(LPARAM(0)),
        );
    }
    Ok(())
}

/// Add the two aspect mode radios (Letterbox / Stretch). Hidden-first creation avoids flicker.
pub fn add_aspect_radios(parent: HWND, preserve_aspect: bool) -> Result<()> {
    unsafe {
        let make_radio = |text: &str, id: usize, first: bool| -> Option<HWND> {
            let wstr = U16CString::from_str(text).ok()?;
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(U16CString::from_str("BUTTON").unwrap().as_ptr()),
                PCWSTR(wstr.as_ptr()),
                WINDOW_STYLE(
                    WS_CHILD.0 | (if first { WS_GROUP.0 } else { 0 }) | (BS_AUTORADIOBUTTON as u32),
                ),
                0,
                0,
                0,
                24,
                Some(parent),
                Some(HMENU(id as *mut _)),
                None,
                None,
            )
            .ok()
        };
        const BST_CHECKED: usize = 1;
        if let Some(letterbox) = make_radio("Letterbox", ID_RADIO_ASPECT_LETTERBOX, true) {
            get_gui_state()
                .radio_letterbox
                .store(letterbox.0 as isize, Ordering::Relaxed);
            if preserve_aspect {
                let _ = SendMessageW(
                    letterbox,
                    BM_SETCHECK,
                    Some(WPARAM(BST_CHECKED)),
                    Some(LPARAM(0)),
                );
            }
        }
        if let Some(stretch) = make_radio("Stretch", ID_RADIO_ASPECT_STRETCH, false) {
            get_gui_state()
                .radio_stretch
                .store(stretch.0 as isize, Ordering::Relaxed);
            if !preserve_aspect {
                let _ = SendMessageW(
                    stretch,
                    BM_SETCHECK,
                    Some(WPARAM(BST_CHECKED)),
                    Some(LPARAM(0)),
                );
            }
        }
    }
    Ok(())
}

/// Register the aspect mode change callback. Ignored if already set.
use crate::cli::AspectMode;

pub fn set_aspect_toggle_callback(cb: Arc<dyn Fn(AspectMode) + Send + Sync>) {
    let _ = get_gui_state().aspect_toggle_cb.set(cb);
}

/// Retrieve current selector textbox contents as UTF-8 (None if control missing).
/// Retrieve the current selector textbox contents (UTF‑16 -> UTF‑8). Returns empty string if control exists but has no text.
pub fn get_selector_text() -> Option<String> {
    let h = get_gui_state().selector_edit.load(Ordering::Relaxed);
    if h == 0 {
        return None;
    }
    let hwnd = HWND(h as *mut _);
    // Allocate buffer (reasonable max length)
    let mut buf: Vec<u16> = vec![0u16; 512];
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::GetWindowTextW;
        let len = GetWindowTextW(hwnd, &mut buf) as usize;
        if len == 0 {
            return Some(String::new());
        }
        let slice = &buf[..len.min(buf.len())];
        Some(String::from_utf16_lossy(slice))
    }
}

/// Get the currently selected selector type from radio buttons.
/// Determine which selector radio button is currently checked.
pub fn get_selected_selector_type() -> SelectorType {
    use windows::Win32::UI::WindowsAndMessaging::BM_GETCHECK;

    unsafe {
        let check_radio = |handle: &AtomicIsize| -> bool {
            let h = handle.load(Ordering::Relaxed);
            if h == 0 {
                return false;
            }
            let state = SendMessageW(
                HWND(h as *mut _),
                BM_GETCHECK,
                Some(WPARAM(0)),
                Some(LPARAM(0)),
            );
            state.0 == 1 // BST_CHECKED
        };

        let gui_state = get_gui_state();
        if check_radio(&gui_state.radio_process) {
            SelectorType::Process
        } else if check_radio(&gui_state.radio_class) {
            SelectorType::WindowClass
        } else if check_radio(&gui_state.radio_title) {
            SelectorType::Title
        } else {
            // Default to Process if nothing is selected (shouldn't happen)
            SelectorType::Process
        }
    }
}

/// Run the Windows message loop (can handle both GUI and WinTab messages).
/// This replaces the separate winhost message loop when using the GUI window.
/// Run the main (blocking) Win32 message loop until `WM_QUIT` is received.
pub fn run_message_loop() -> Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MSG, TranslateMessage,
    };

    unsafe {
        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 == -1 {
                return Err(anyhow!("GetMessageW failed"));
            }
            if r.0 == 0 {
                return Ok(());
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
