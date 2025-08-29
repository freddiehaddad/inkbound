//! Hidden message‑only window creation and message loop utilities.
//!
//! A single STATIC‑class message‑only window is used as the WinTab context owner and to anchor
//! WinEvent hooks. The message pump runs indefinitely until WM_QUIT is posted by the Ctrl+C
//! handler (or other termination path).

use anyhow::{Result, anyhow};
use widestring::U16CString;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE,
};
use windows::core::PCWSTR;

static mut MESSAGE_HWND: Option<HWND> = None;

/// Create (or return an existing) hidden message‑only host window.
pub fn create_message_window(_class_name: &str) -> Result<HWND> {
    unsafe {
        if let Some(h) = MESSAGE_HWND {
            return Ok(h);
        }
        let class_u16 = U16CString::from_str("STATIC")?; // use predefined class
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_u16.as_ptr()),
            PCWSTR(class_u16.as_ptr()),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )?;
        MESSAGE_HWND = Some(hwnd);
        Ok(hwnd)
    }
}

/// Standard GetMessage/Dispatch loop terminated by WM_QUIT.
pub fn run_message_loop() -> Result<()> {
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
