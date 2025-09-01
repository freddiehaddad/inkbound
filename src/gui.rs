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
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_BITFIELDS, BITMAPINFO, BITMAPV5HEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject, HBITMAP, HGDIOBJ,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, FillRect, GetStockObject, HBRUSH, PAINTSTRUCT, WHITE_BRUSH,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreatePopupMenu,
    CreateWindowExW, DefWindowProcW, DestroyIcon, ES_AUTOHSCROLL, GetClientRect, GetCursorPos,
    HMENU, MF_STRING, MoveWindow, PostQuitMessage, RegisterClassW, SIZE_MINIMIZED, SW_HIDE,
    SW_SHOW, SetWindowTextW, ShowWindow, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TrackPopupMenu,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_PAINT, WM_SIZE, WNDCLASSW,
    WS_CHILD, WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
};
use windows::Win32::UI::WindowsAndMessaging::{BM_SETCHECK, BS_AUTOCHECKBOX, SendMessageW};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};
use windows::core::PCWSTR;

/// Centralized GUI state container to replace global static variables
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
    run_enabled: AtomicBool,
    /// Whether the target window currently exists (for tray icon coloring)
    target_present: AtomicBool,
    /// Optional callback invoked whenever the user toggles Start/Stop
    run_toggle_cb: OnceCell<Arc<dyn Fn(bool) + Send + Sync>>,
    /// Callback for aspect ratio toggle
    aspect_toggle_cb: OnceCell<Arc<dyn Fn(bool) + Send + Sync>>,
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
        }
    }
}

// Thread-safe singleton for GUI state (temporary during transition)
static GUI_STATE: OnceCell<GuiState> = OnceCell::new();

/// Get or initialize the GUI state singleton
fn get_gui_state() -> &'static GuiState {
    GUI_STATE.get_or_init(GuiState::new)
}
const ID_START_STOP: usize = 2001;
const ID_CB_KEEP_ASPECT: usize = 2101;
const ID_RADIO_PROCESS: usize = 2201;
const ID_RADIO_CLASS: usize = 2202;
const ID_RADIO_TITLE: usize = 2203;
const WM_TRAYICON: u32 = 0x0400 + 1; // custom message id
const IDM_TRAY_RESTORE: usize = 1001;
const IDM_TRAY_EXIT: usize = 1002;
const IDM_TRAY_TOGGLE: usize = 1003; // dynamic Start/Stop
const TRAY_UID: u32 = 1;

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
    for i in 0..(16 * 16) {
        unsafe {
            *px.add(i) = color;
        }
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

fn current_toggle_label() -> &'static str {
    let gui_state = get_gui_state();
    if gui_state.run_enabled.load(Ordering::Relaxed) {
        "Stop"
    } else {
        "Start"
    }
}

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
    let tip = U16CString::from_str("PenTarget Mapper").unwrap();
    let slice = tip.as_slice_with_nul();
    for (i, &c) in slice.iter().enumerate() {
        if i < nid.szTip.len() {
            nid.szTip[i] = c;
        }
    }
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

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
        WM_PAINT => unsafe {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            let brush = HBRUSH(GetStockObject(WHITE_BRUSH).0);
            FillRect(
                hdc,
                &RECT {
                    left: 0,
                    top: 0,
                    right: ps.rcPaint.right,
                    bottom: ps.rcPaint.bottom,
                },
                brush,
            );
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        },
        WM_SIZE => unsafe {
            if wparam.0 == SIZE_MINIMIZED as usize {
                let _ = ShowWindow(hwnd, SW_HIDE);
            } else {
                let gui_state = get_gui_state();
                let stored = gui_state.selector_edit.load(Ordering::Relaxed);
                if stored != 0 {
                    let edit = HWND(stored as *mut core::ffi::c_void);
                    let mut rc = RECT::default();
                    if GetClientRect(hwnd, &mut rc).is_ok() {
                        let margin = 12;
                        let edit_top = 32;
                        let edit_height = 24;
                        let new_width = (rc.right - rc.left) - margin * 2;
                        if new_width > 40 {
                            let _ =
                                MoveWindow(edit, margin, edit_top, new_width, edit_height, true);
                        }
                    }
                }
                // Resize Start/Stop button horizontally as well.
                let btn_stored = gui_state.start_stop_button.load(Ordering::Relaxed);
                if btn_stored != 0 {
                    let btn = HWND(btn_stored as *mut core::ffi::c_void);
                    let mut rc = RECT::default();
                    if GetClientRect(hwnd, &mut rc).is_ok() {
                        let margin = 12;
                        let btn_top = 120;
                        let btn_height = 26;
                        let new_width = (rc.right - rc.left) - margin * 2;
                        if new_width > 60 {
                            // allow a slightly larger minimum for button label space
                            let _ = MoveWindow(btn, margin, btn_top, new_width, btn_height, true);
                        }
                    }
                }
            }
            LRESULT(0)
        },
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
                ID_CB_KEEP_ASPECT => {
                    // Query checkbox state (BM_GETCHECK = 0x00F0). Returns BST_CHECKED (1) when checked.
                    const BM_GETCHECK: u32 = 0x00F0;
                    let state = SendMessageW(
                        HWND(lparam.0 as *mut _),
                        BM_GETCHECK,
                        Some(WPARAM(0)),
                        Some(LPARAM(0)),
                    );
                    let checked = state.0 == 1; // BST_CHECKED
                    if let Some(cb) = get_gui_state().aspect_toggle_cb.get() {
                        cb(checked);
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
            remove_tray_icon(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn register_main_class() -> Result<&'static U16CString> {
    get_gui_state().main_class.get_or_try_init(|| {
        let name = U16CString::from_str("PenTargetWindow")?;
        unsafe {
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(main_wnd_proc),
                lpszClassName: PCWSTR(name.as_ptr()),
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
fn create_raw_main_window(title: &str) -> Result<HWND> {
    let class = register_main_class()?;
    let title_u16 = U16CString::from_str(title)?;
    unsafe {
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR(title_u16.as_ptr()),
            WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0 | WS_VISIBLE.0),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            640,
            480,
            None,
            None,
            None,
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        get_gui_state()
            .visible_main
            .store(hwnd.0 as isize, Ordering::Relaxed);
        add_tray_icon(hwnd);
        Ok(hwnd)
    }
}

/// Create the full GUI window (selector textbox, radio buttons, option checkboxes, start/stop button) in one call.
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
    let _ = add_option_checkboxes(hwnd, preserve_aspect);
    let _ = add_start_stop_button(hwnd, initial_run_enabled);
    Ok(hwnd)
}

/// Add a Start/Stop toggle button with initial caption based on run state.
pub fn add_start_stop_button(parent: HWND, initial_run_enabled: bool) -> Result<()> {
    unsafe {
        let caption_text = if initial_run_enabled { "Stop" } else { "Start" };
        let caption = U16CString::from_str(caption_text).unwrap();
        let hwnd_btn = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(U16CString::from_str("BUTTON").unwrap().as_ptr()),
            PCWSTR(caption.as_ptr()),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | (BS_PUSHBUTTON as u32)),
            12,
            120,
            100,
            26,
            Some(parent),
            Some(HMENU(ID_START_STOP as *mut _)),
            None,
            None,
        );
        if let Ok(hb) = hwnd_btn {
            get_gui_state()
                .start_stop_button
                .store(hb.0 as isize, Ordering::Relaxed);
        }
    }
    Ok(())
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

fn update_tray_icon_for_state() {
    update_tray_icon_for_state_with(HWND(std::ptr::null_mut()));
}

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
pub fn set_run_toggle_callback(cb: Arc<dyn Fn(bool) + Send + Sync>) {
    let _ = get_gui_state().run_toggle_cb.set(cb);
}

/// Query whether mapping is currently enabled (user wants mapping if target exists).
pub fn is_run_enabled() -> bool {
    get_gui_state().run_enabled.load(Ordering::Relaxed)
}

/// Add a simple labeled read‑only textbox displaying the selected target spec.
pub fn add_selector_textbox(parent: HWND, label: &str, value: &str) -> Result<()> {
    // Positions are static for now; no DPI handling yet.
    let label_w =
        U16CString::from_str(label).unwrap_or_else(|_| U16CString::from_str("Selector").unwrap());
    let value_w = U16CString::from_str(value).unwrap_or_else(|_| U16CString::from_str("").unwrap());
    unsafe {
        // Static label
        let _h_static = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(U16CString::from_str("STATIC").unwrap().as_ptr()),
            PCWSTR(label_w.as_ptr()),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
            12,
            12,
            120,
            18,
            Some(parent),
            None,
            None,
            None,
        );
        // Edit box (always editable)
        let style = WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | (ES_AUTOHSCROLL as u32));
        let h_edit = CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_CLIENTEDGE.0),
            PCWSTR(U16CString::from_str("EDIT").unwrap().as_ptr()),
            PCWSTR(value_w.as_ptr()),
            style,
            12,
            32,
            360, // initial width; resized on WM_SIZE
            24,
            Some(parent),
            None,
            None,
            None,
        );
        if let Ok(hwnd_edit) = h_edit {
            get_gui_state()
                .selector_edit
                .store(hwnd_edit.0 as isize, Ordering::Relaxed);
        }
    }
    Ok(())
}

/// Add radio buttons for selector type selection.
pub fn add_selector_radio_buttons(parent: HWND, selected_type: SelectorType) -> Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{BS_AUTORADIOBUTTON, WS_GROUP};

    unsafe {
        // Helper to create a radio button
        let create_radio =
            |text: &str, x: i32, y: i32, id: usize, is_first: bool| -> Result<HWND> {
                let wstr = U16CString::from_str(text)?;
                let mut style =
                    WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | (BS_AUTORADIOBUTTON as u32));
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
                    20,
                    Some(parent),
                    None,
                    None,
                    Some(std::ptr::addr_of!(id) as *const _),
                )
                .map_err(|e| anyhow!("Failed to create radio button: {}", e))
            };

        // Create radio buttons horizontally
        let radio_process = create_radio("Process", 12, 60, ID_RADIO_PROCESS, true)?;
        let radio_class = create_radio("Class", 100, 60, ID_RADIO_CLASS, false)?;
        let radio_title = create_radio("Title", 180, 60, ID_RADIO_TITLE, false)?;

        // Store handles
        let gui_state = get_gui_state();
        gui_state
            .radio_process
            .store(radio_process.0 as isize, Ordering::Relaxed);
        gui_state
            .radio_class
            .store(radio_class.0 as isize, Ordering::Relaxed);
        gui_state
            .radio_title
            .store(radio_title.0 as isize, Ordering::Relaxed);

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

/// Add two read-only state checkboxes reflecting CLI options.
pub fn add_option_checkboxes(parent: HWND, preserve_aspect: bool) -> Result<()> {
    unsafe {
        // Helper to create a checkbox
        let make_cb = |text: &str, y: i32, id: Option<usize>| -> Option<HWND> {
            let wstr = U16CString::from_str(text).ok()?;
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(U16CString::from_str("BUTTON").unwrap().as_ptr()),
                PCWSTR(wstr.as_ptr()),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | (BS_AUTOCHECKBOX as u32)),
                12,
                y,
                500,
                20,
                Some(parent),
                id.map(|v| HMENU(v as *mut _)),
                None,
                None,
            )
            .ok()
        };
        // BM_SETCHECK expects wParam = BST_* (0/1/2)
        const BST_CHECKED: usize = 1;
        if let Some(cb1) = make_cb("Keep tablet aspect", 90, Some(ID_CB_KEEP_ASPECT))
            && preserve_aspect
        {
            let _ = SendMessageW(cb1, BM_SETCHECK, Some(WPARAM(BST_CHECKED)), Some(LPARAM(0)));
        }
        // (Second checkbox for removed feature intentionally omitted.)
    }
    Ok(())
}

/// Register callback invoked when aspect checkbox toggled.
pub fn set_aspect_toggle_callback(cb: Arc<dyn Fn(bool) + Send + Sync>) {
    let _ = get_gui_state().aspect_toggle_cb.set(cb);
}

/// Retrieve current selector textbox contents as UTF-8 (None if control missing).
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
