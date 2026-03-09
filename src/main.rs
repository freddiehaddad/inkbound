mod geometry;
mod otd;
mod window;

use anyhow::{Context, Result};
use clap::Parser;
use geometry::DisplayArea;
use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Foundation::*;
use windows::Win32::System::Console::*;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Parser)]
#[command(name = "inkbound")]
#[command(about = "Automatically maps your tablet to a target window")]
struct Args {
    /// Process name or window title to track (case-insensitive substring match)
    #[arg(short, long)]
    target: String,

    /// Override tablet name (auto-detected from OTD settings if not provided)
    #[arg(long)]
    tablet: Option<String>,

    /// Tablet area rotation in degrees. If pen movements don't match your
    /// physical tablet orientation, try different values (0, 90, 180, 270).
    #[arg(short, long, default_value_t = 0, value_parser = parse_rotation)]
    rotation: u16,
}

fn parse_rotation(s: &str) -> Result<u16, String> {
    let v: u16 = s
        .parse()
        .map_err(|_| format!("'{s}' is not a valid number"))?;
    if !v.is_multiple_of(90) || v >= 360 {
        return Err("rotation must be 0, 90, 180, or 270".to_string());
    }
    Ok(v)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    WaitingForWindow,
    Tracking { hwnd: HWND },
    Suspended { hwnd: HWND },
}

struct AppState {
    target: String,
    state: State,
    otd: otd::OtdBridge,
    tablet_aspect_ratio: f64,
    in_move_size: bool,
    last_error_logged: std::time::Instant,
    last_applied_area: Option<DisplayArea>,
}

const DEBOUNCE_TIMER_ID: usize = 1;
const DEBOUNCE_MS: u32 = 100;
const OBJID_WINDOW: i32 = 0;
const ERROR_LOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

thread_local! {
    static APP: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

/// Stored globally so the Ctrl+C handler (which runs on a different thread)
/// can restore the original mapping before the process exits.
struct RestoreInfo {
    tablet_name: String,
    original_display_area: DisplayArea,
    daemon_pid: Option<u32>,
}

static RESTORE_INFO: OnceLock<RestoreInfo> = OnceLock::new();
static MAIN_THREAD_ID: AtomicU32 = AtomicU32::new(0);

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Enable per-monitor DPI awareness for accurate window coordinates
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let args = Args::parse();

    // Ensure OTD daemon is running (starts it if needed, stops on exit)
    let _daemon_guard = otd::ensure_daemon_running()?;

    // Detect tablet name
    let tablet_name = match args.tablet {
        Some(name) => name,
        None => otd::detect_tablet_name().context("Failed to auto-detect tablet")?,
    };

    // Create OTD bridge (saves original mapping and applies rotation)
    let otd_bridge = otd::OtdBridge::new(tablet_name.clone(), args.rotation as f64)?;
    let tablet_aspect_ratio = otd_bridge.tablet_aspect_ratio();

    // Store restore info globally for the Ctrl+C handler
    let daemon_pid = _daemon_guard.pid();
    RESTORE_INFO
        .set(RestoreInfo {
            tablet_name,
            original_display_area: otd_bridge.original_display_area().clone(),
            daemon_pid,
        })
        .ok();
    MAIN_THREAD_ID.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);

    log::info!("Target: \"{}\"", args.target);

    let app_state = AppState {
        target: args.target,
        state: State::WaitingForWindow,
        otd: otd_bridge,
        tablet_aspect_ratio,
        in_move_size: false,
        last_error_logged: std::time::Instant::now() - ERROR_LOG_INTERVAL,
        last_applied_area: None,
    };

    APP.with(|app| {
        *app.borrow_mut() = Some(app_state);
    });

    // Set up Ctrl+C / console close handler
    unsafe {
        let handler: PHANDLER_ROUTINE = Some(ctrl_handler);
        SetConsoleCtrlHandler(Some(handler), true).context("Failed to set console ctrl handler")?;
    }

    // Install event hooks FIRST (before finding the window — avoids race condition)
    let hooks = install_event_hooks()?;

    // Now search for the target window
    let initial_hwnd = APP.with(|app| {
        let app = app.borrow();
        let app = app.as_ref().unwrap();
        window::find_matching_window(&app.target)
    });

    if let Some(hwnd) = initial_hwnd {
        log::info!(
            "Found target window: \"{}\"",
            window::get_window_title(hwnd),
        );
        APP.with(|app| {
            let mut app = app.borrow_mut();
            if let Some(app) = app.as_mut() {
                transition_to_tracking(app, hwnd);
            }
        });
    } else {
        log::info!("Target window not found — waiting for it to appear...");
    }

    // Run the Win32 message loop (blocks until WM_QUIT)
    run_message_loop();

    // Cleanup: restore original mapping only if we didn't start the daemon
    // (if we started it, we're about to kill it — no point restoring)
    if daemon_pid.is_none() {
        APP.with(|app| {
            if let Some(app) = app.borrow().as_ref()
                && let Err(e) = app.otd.restore_original()
            {
                log::error!("Failed to restore original mapping: {e}");
            }
        });
    }

    for hook in hooks {
        unsafe {
            let _ = UnhookWinEvent(hook);
        }
    }

    log::info!("Exiting.");
    Ok(())
}

fn install_event_hooks() -> Result<Vec<HWINEVENTHOOK>> {
    let mut hooks = Vec::new();

    let event_ranges = [
        (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND),
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
        (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
        (EVENT_OBJECT_SHOW, EVENT_OBJECT_HIDE),
        (EVENT_OBJECT_DESTROY, EVENT_OBJECT_DESTROY),
    ];

    for (min, max) in event_ranges {
        let hook = unsafe {
            SetWinEventHook(
                min,
                max,
                None,
                Some(win_event_callback),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if hook.is_invalid() {
            anyhow::bail!("Failed to install event hook for events {min:#x}-{max:#x}");
        }
        hooks.push(hook);
    }

    Ok(hooks)
}

unsafe extern "system" fn win_event_callback(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    // Only process window-level events (not caret/cursor)
    if event == EVENT_OBJECT_LOCATIONCHANGE && id_object != OBJID_WINDOW {
        return;
    }

    APP.with(|app| {
        let mut app = app.borrow_mut();
        let Some(app) = app.as_mut() else { return };

        match event {
            EVENT_SYSTEM_MOVESIZESTART => {
                if is_tracked(app, hwnd) {
                    app.in_move_size = true;
                }
            }

            EVENT_SYSTEM_MOVESIZEEND => {
                if is_tracked(app, hwnd) {
                    app.in_move_size = false;
                    unsafe {
                        let _ = KillTimer(None, DEBOUNCE_TIMER_ID);
                    }
                    update_mapping(app, hwnd);
                }
            }

            EVENT_OBJECT_LOCATIONCHANGE => {
                handle_location_change(app, hwnd);
            }

            EVENT_SYSTEM_FOREGROUND => {
                handle_foreground(app, hwnd);
            }

            EVENT_OBJECT_SHOW => {
                handle_show(app, hwnd);
            }

            EVENT_OBJECT_HIDE => {
                handle_hide(app, hwnd);
            }

            EVENT_OBJECT_DESTROY => {
                handle_destroy(app, hwnd);
            }

            _ => {}
        }
    });
}

// --- Event handlers ---

fn handle_location_change(app: &mut AppState, hwnd: HWND) {
    match app.state {
        State::Tracking { hwnd: tracked } if hwnd == tracked => {
            if window::is_minimized(hwnd) {
                log::info!("Target window minimized");
                app.state = State::Suspended { hwnd };
                restore_original_quietly(app);
            } else if !app.in_move_size {
                // Programmatic move (snapping, etc.) — debounce
                let timer_fn: TIMERPROC = Some(debounce_timer_callback);
                unsafe {
                    SetTimer(None, DEBOUNCE_TIMER_ID, DEBOUNCE_MS, Some(timer_fn));
                }
            }
        }
        State::Suspended { hwnd: tracked } if hwnd == tracked => {
            if !window::is_minimized(hwnd) {
                log::info!(
                    "Target window restored: \"{}\"",
                    window::get_window_title(hwnd)
                );
                transition_to_tracking(app, hwnd);
            }
        }
        _ => {}
    }
}

fn handle_foreground(app: &mut AppState, hwnd: HWND) {
    if window::matches_target(hwnd, &app.target) && window::is_valid_window(hwnd) {
        // Only log and update if we're switching to a different window
        let already_tracking =
            matches!(app.state, State::Tracking { hwnd: tracked } if tracked == hwnd);
        if !already_tracking {
            log::info!(
                "Target window focused: \"{}\"",
                window::get_window_title(hwnd)
            );
            transition_to_tracking(app, hwnd);
        }
    }
}

fn handle_show(app: &mut AppState, hwnd: HWND) {
    match app.state {
        State::WaitingForWindow => {
            if window::matches_target(hwnd, &app.target) && window::is_valid_window(hwnd) {
                log::info!(
                    "Target window appeared: \"{}\"",
                    window::get_window_title(hwnd)
                );
                transition_to_tracking(app, hwnd);
            }
        }
        State::Suspended { hwnd: tracked } if hwnd == tracked => {
            if !window::is_minimized(hwnd) {
                log::info!(
                    "Target window restored: \"{}\"",
                    window::get_window_title(hwnd)
                );
                transition_to_tracking(app, hwnd);
            }
        }
        _ => {}
    }
}

fn handle_hide(app: &mut AppState, hwnd: HWND) {
    if let State::Tracking { hwnd: tracked } = app.state
        && hwnd == tracked
    {
        log::info!("Target window hidden");
        app.state = State::Suspended { hwnd };
        restore_original_quietly(app);
    }
}

fn handle_destroy(app: &mut AppState, hwnd: HWND) {
    let is_tracked = matches!(
        app.state,
        State::Tracking { hwnd: tracked } | State::Suspended { hwnd: tracked }
        if hwnd == tracked
    );

    if is_tracked {
        log::info!("Target window closed — waiting for it to reappear...");
        app.state = State::WaitingForWindow;
        restore_original_quietly(app);
    }
}

// --- Helpers ---

fn is_tracked(app: &AppState, hwnd: HWND) -> bool {
    matches!(app.state, State::Tracking { hwnd: tracked } if tracked == hwnd)
}

fn transition_to_tracking(app: &mut AppState, hwnd: HWND) {
    app.state = State::Tracking { hwnd };
    app.in_move_size = false;
    update_mapping(app, hwnd);
}

fn update_mapping(app: &mut AppState, hwnd: HWND) {
    let Some((left, top, width, height)) = window::get_window_rect(hwnd) else {
        return;
    };

    let Some(area) = geometry::fit_to_window(left, top, width, height, app.tablet_aspect_ratio)
    else {
        return;
    };

    // Skip if the area hasn't changed (avoids spamming OTD)
    if app.last_applied_area.as_ref() == Some(&area) {
        return;
    }

    log::debug!(
        "Mapping tablet to [{:.0}x{:.0}@<{:.0}, {:.0}>]",
        area.width,
        area.height,
        area.center_x,
        area.center_y
    );

    if let Err(e) = app.otd.set_display_area(&area)
        && app.last_error_logged.elapsed() >= ERROR_LOG_INTERVAL
    {
        log::warn!("Failed to update display area: {e}");
        app.last_error_logged = std::time::Instant::now();
    } else {
        app.last_applied_area = Some(area);
    }
}

fn restore_original_quietly(app: &mut AppState) {
    app.last_applied_area = None;
    if let Err(e) = app.otd.restore_original() {
        log::warn!("Failed to restore original mapping: {e}");
    }
}

unsafe extern "system" fn debounce_timer_callback(_hwnd: HWND, _msg: u32, _id: usize, _time: u32) {
    unsafe {
        let _ = KillTimer(None, DEBOUNCE_TIMER_ID);
    }

    APP.with(|app| {
        let mut app = app.borrow_mut();
        let Some(app) = app.as_mut() else { return };
        if let State::Tracking { hwnd } = app.state {
            update_mapping(app, hwnd);
        }
    });
}

unsafe extern "system" fn ctrl_handler(ctrl_type: u32) -> BOOL {
    if ctrl_type == CTRL_C_EVENT || ctrl_type == CTRL_CLOSE_EVENT || ctrl_type == CTRL_BREAK_EVENT {
        // Restore original mapping directly (works from any thread)
        if let Some(info) = RESTORE_INFO.get() {
            if let Some(pid) = info.daemon_pid {
                // We started the daemon — kill it, no need to restore settings
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/F"])
                    .output();
            } else {
                // Daemon was already running — restore original mapping
                let _ = std::process::Command::new("OpenTabletDriver.Console.exe")
                    .args([
                        "setdisplayarea",
                        &info.tablet_name,
                        &info.original_display_area.width.to_string(),
                        &info.original_display_area.height.to_string(),
                        &info.original_display_area.center_x.to_string(),
                        &info.original_display_area.center_y.to_string(),
                    ])
                    .output();
            }
        }

        // Signal the main thread's message loop to exit
        let thread_id = MAIN_THREAD_ID.load(Ordering::SeqCst);
        if thread_id != 0 {
            unsafe {
                let _ = PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }

        return BOOL(1);
    }
    BOOL(0)
}

fn run_message_loop() {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
