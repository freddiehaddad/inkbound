use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PWSTR;

/// Get the title of a window.
pub fn get_window_title(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len == 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        let actual = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..actual as usize])
    }
}

/// Get the process executable name for a window's owning process.
pub fn get_process_name(hwnd: HWND) -> String {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return String::new();
        }

        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };

        let mut buf = vec![0u16; 260];
        let mut size = buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(process);

        if result.is_ok() {
            let full_path = String::from_utf16_lossy(&buf[..size as usize]);
            full_path
                .rsplit('\\')
                .next()
                .unwrap_or(&full_path)
                .to_string()
        } else {
            String::new()
        }
    }
}

/// Check if a window matches the target (case-insensitive substring match
/// against window title or process name).
pub fn matches_target(hwnd: HWND, target: &str) -> bool {
    let target_lower = target.to_lowercase();

    let title = get_window_title(hwnd);
    if !title.is_empty() && title.to_lowercase().contains(&target_lower) {
        return true;
    }

    let process = get_process_name(hwnd);
    if !process.is_empty() && process.to_lowercase().contains(&target_lower) {
        return true;
    }

    false
}

/// Check if a window is visible and not minimized.
pub fn is_valid_window(hwnd: HWND) -> bool {
    unsafe { IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool() }
}

/// Check if a window is minimized (iconic).
pub fn is_minimized(hwnd: HWND) -> bool {
    unsafe { IsIconic(hwnd).as_bool() }
}

/// Get the visible window rectangle as (left, top, width, height).
/// Uses DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS) to get the actual
/// visible bounds, excluding invisible DPI-scaled borders on Windows 10/11.
/// Falls back to GetWindowRect if DWM is unavailable.
/// Returns `None` if the rect has zero or negative dimensions.
pub fn get_window_rect(hwnd: HWND) -> Option<(i32, i32, i32, i32)> {
    unsafe {
        let mut rect = RECT::default();

        // Try DWM first for accurate visible bounds
        let got_rect = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &raw mut rect as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok()
            || GetWindowRect(hwnd, &mut rect).is_ok();

        if got_rect {
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            if width > 0 && height > 0 {
                return Some((rect.left, rect.top, width, height));
            }
        }
        None
    }
}

/// Find the first matching window. Prefers the foreground window if it matches.
pub fn find_matching_window(target: &str) -> Option<HWND> {
    let fg = unsafe { GetForegroundWindow() };
    if !fg.0.is_null() && is_valid_window(fg) && matches_target(fg, target) {
        return Some(fg);
    }

    let mut windows: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(collect_windows_callback),
            LPARAM(&raw mut windows as isize),
        );
    }

    windows
        .into_iter()
        .find(|&hwnd| is_valid_window(hwnd) && matches_target(hwnd, target))
}

unsafe extern "system" fn collect_windows_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        let windows = &mut *(lparam.0 as *mut Vec<HWND>);
        windows.push(hwnd);
    }
    BOOL(1)
}
