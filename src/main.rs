//! Event‑driven Wacom (WinTab) tablet mapper.
//!
//! This binary opens a WinTab context bound to the GUI window and dynamically
//! remaps the tablet output area to match a user‑selected target window (process name, class
//! name, or title substring). The mapping updates immediately in response to WinEvent hook
//! notifications (size / move / foreground / creation events); no polling loops are used.
//!
//! High‑level flow:
//! 1. Parse CLI (one required selector + behavioural flags).
//! 2. Initialize tracing from RUST_LOG and create the GUI window as WinTab host.
//! 3. Query the default WinTab LOGCONTEXT (WTInfoA), set desired options, and open a context
//!    with fallback if the driver rejects flags.
//! 4. Install WinEvent hooks. A single callback applies or resets mapping as events arrive.
//! 5. On foreground re‑entry of the target window the context is explicitly re‑opened to work
//!    around some drivers resetting state when focus changes.
//! 6. On destroy / minimize we temporarily reset to the original full‑tablet mapping.
//! 7. Run the Win32 message loop until Ctrl+C (which posts WM_QUIT) or external termination.
//!
//! Logging surfaces hook installation, mapping application, fallback behaviour, and context
//! state (optionally via WINTAB_DUMP=1).

mod context;
mod gui;
mod mapping;
mod winevent;
mod wintab;

use anyhow::Result;
use clap::{ArgAction, ArgGroup, Parser};
use std::sync::{Arc, Mutex};
use tracing::{error, info};

use context::{SendHwnd, open_context_with_fallback, reopen_context, reopen_with_template};
use gui::{
    SelectorType, create_main_window, get_selected_selector_type, is_run_enabled,
    reflect_target_presence, run_message_loop, set_aspect_toggle_callback, set_run_toggle_callback,
};
use mapping::{MapConfig, apply_mapping, rect_to_logcontext};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
use winevent::{
    HookFilter, Target, find_existing_target, install_hooks, query_window_rect, uninstall_hooks,
    update_target,
};
use wintab::{wt_close, wt_get};

/// Command line interface definition.
#[derive(Parser, Debug)]
#[command(
    version,
    about = concat!(
        env!("CARGO_PKG_NAME"), " v", env!("CARGO_PKG_VERSION"),
        " - Map a Wacom tablet area dynamically to a chosen window (process, class, or title) without polling.",
    ),
    // Selector group no longer required to allow GUI-based selection.
    group = ArgGroup::new("selector").required(false).args(["process", "win_class", "title_contains"])
)]
struct Cli {
    #[arg(long = "process", alias = "proc")]
    /// Match target by process executable name (case‑insensitive, e.g. "photoshop.exe").
    process: Option<String>,
    #[arg(long = "win-class", alias = "class")]
    /// Match target by exact top‑level window class name.
    win_class: Option<String>,
    #[arg(long = "title-contains", alias = "title")]
    /// Match target by substring search within the window title.
    title_contains: Option<String>,
    #[arg(long = "preserve-aspect", alias = "keep-aspect")]
    /// Preserve tablet aspect ratio by CROPPING tablet input to match window aspect so the entire window is reachable (no letterboxing).
    preserve_aspect: bool,
    /// Increase verbosity (-v=debug, -vv=trace). Overrides RUST_LOG.
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count)]
    verbose: u8,
    /// Quiet mode: only warnings and errors. Overrides -v and RUST_LOG.
    #[arg(short = 'q', long = "quiet")]
    quiet: bool,
}

/// Program entry point.
///
/// Sets up hooks, opens the WinTab context with fallback, performs initial mapping if the target
/// already exists, then dispatches the Win32 message loop until shutdown. Errors surfaced early
/// result in a non‑zero exit code via anyhow.
fn main() -> Result<()> {
    let cli = Cli::parse();
    // Configure logging according to -q / -v occurrences; fall back to env filter.
    {
        use tracing::Level;
        let builder = tracing_subscriber::fmt::Subscriber::builder();
        if cli.quiet {
            builder.with_max_level(Level::WARN).init();
        } else if cli.verbose > 1 {
            builder.with_max_level(Level::TRACE).init();
        } else if cli.verbose == 1 {
            builder.with_max_level(Level::DEBUG).init();
        } else {
            builder
                .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
                .with_max_level(Level::INFO)
                .init();
        }
    }

    info!(
        version = env!("CARGO_PKG_VERSION"),
        ?cli,
        "starting pentarget"
    );

    // Determine optional initial target from CLI.
    let target_cli: Option<Target> = if let Some(p) = &cli.process {
        Some(Target::ProcessName(p.clone()))
    } else if let Some(c) = &cli.win_class {
        Some(Target::WindowClass(c.clone()))
    } else {
        cli.title_contains
            .as_ref()
            .map(|t| Target::TitleSubstring(t.clone()))
    };

    // Try to create the GUI window first, which will serve as our WinTab host
    let window_title = format!("PenTarget Mapper v{}", env!("CARGO_PKG_VERSION"));

    // Determine selector type and display value
    let (selector_type, selector_value) = if let Some(t) = &target_cli {
        match t {
            Target::ProcessName(s) => (SelectorType::Process, s.clone()),
            Target::WindowClass(s) => (SelectorType::WindowClass, s.clone()),
            Target::TitleSubstring(s) => (SelectorType::Title, s.clone()),
        }
    } else {
        (SelectorType::Process, String::new()) // Default to Process when no CLI selector
    };

    // Determine initial run state: enabled if CLI selector provided, disabled otherwise
    let initial_run_enabled = target_cli.is_some();

    let hwnd = create_main_window(
        &window_title,
        "Target",
        &selector_value,
        cli.preserve_aspect,
        selector_type,
        initial_run_enabled,
    )?;

    let (hctx, base_ctx, final_options) = open_context_with_fallback(hwnd)?;
    let hctx_cell = Arc::new(Mutex::new(hctx));
    let hctx_cell_outer = hctx_cell.clone();

    // Ctrl+C handler -> graceful quit (must post WM_QUIT to ORIGINAL thread, PostQuitMessage on handler thread is ineffective)
    let main_tid = unsafe { GetCurrentThreadId() };
    {
        let hctx_arc = hctx_cell.clone();
        ctrlc::set_handler(move || {
            tracing::info!("Ctrl+C received, shutting down");
            uninstall_hooks();
            if let Ok(h) = hctx_arc.lock() {
                wt_close(*h);
            }
            unsafe {
                let _ = PostThreadMessageW(main_tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        })
        .expect("ctrlc handler");
    }

    use std::sync::atomic::{AtomicBool, Ordering};
    let keep_aspect_flag = Arc::new(AtomicBool::new(cli.preserve_aspect));
    let keep_aspect_flag_for_events = keep_aspect_flag.clone();
    let cfg_initial = MapConfig {
        keep_aspect: cli.preserve_aspect,
    };
    let base_ctx_clone = base_ctx; // original for reset
    let base_ctx_for_cb = base_ctx; // template for mapping
    let send_hwnd = SendHwnd(hwnd);
    let hctx_cell_for_cb = hctx_cell.clone();
    // Shared optional dynamic target (initialized either from CLI or set after GUI input on first Start),
    // stored in an Arc<Mutex<Option<Target>>> so callbacks can consult current target.
    use std::sync::Mutex as StdMutex;
    let current_target: Arc<StdMutex<Option<Target>>> = Arc::new(StdMutex::new(target_cli));

    let current_target_for_cb = current_target.clone();
    let cb = Arc::new(
        move |hwnd: HWND, event: u32, mut rect: windows::Win32::Foundation::RECT| {
            // If no target yet, ignore events (hooks may be installed only after target set, but guard anyway).
            let has_target = {
                if let Ok(g) = current_target_for_cb.lock() {
                    g.is_some()
                } else {
                    false
                }
            };
            if !has_target {
                return;
            }
            use windows::Win32::UI::WindowsAndMessaging::{
                EVENT_OBJECT_DESTROY, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZESTART,
            };
            info!(
                event,
                left = rect.left,
                top = rect.top,
                right = rect.right,
                bottom = rect.bottom,
                "window event"
            );
            if event == EVENT_OBJECT_DESTROY || event == EVENT_SYSTEM_MINIMIZESTART {
                // Target gone regardless of run state.
                if let Ok(h) = hctx_cell_for_cb.lock()
                    && let Err(e) = apply_mapping(*h, &base_ctx_clone)
                {
                    error!(?e, "reset mapping failed");
                }
                reflect_target_presence(HWND(std::ptr::null_mut()), false);
                return;
            }
            // If user disabled mapping, ignore further events (still update presence when foreground/destroy handled above).
            if !is_run_enabled() {
                return;
            }
            if event == EVENT_SYSTEM_FOREGROUND
                && !reopen_context(&hctx_cell_for_cb, send_hwnd, base_ctx_for_cb, final_options)
            {
                return;
            }
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
                    return;
                }
            }
            let cfg_dyn = MapConfig {
                keep_aspect: keep_aspect_flag_for_events.load(Ordering::Relaxed),
            };
            let ctx = rect_to_logcontext(base_ctx_for_cb, rect, &cfg_dyn);
            if cfg_dyn.keep_aspect {
                if reopen_with_template(&hctx_cell_for_cb, send_hwnd, ctx, final_options) {
                    info!(
                        left = rect.left,
                        top = rect.top,
                        right = rect.right,
                        bottom = rect.bottom,
                        "mapping applied via reopen_with_template(aspect)"
                    );
                } else {
                    error!("reopen_with_template failed; aspect mapping skipped");
                    return;
                }
            } else if let Ok(h) = hctx_cell_for_cb.lock() {
                if let Err(e) = apply_mapping(*h, &ctx) {
                    error!(?e, "apply_mapping failed");
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
            reflect_target_presence(HWND(std::ptr::null_mut()), true);
            if let Ok(h) = hctx_cell_for_cb.lock()
                && std::env::var("WINTAB_DUMP").as_deref() == Ok("1")
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
        },
    );
    // Install hooks immediately only if we have a target from CLI.
    {
        let maybe_t = current_target.lock().ok().and_then(|g| g.clone());
        if let Some(t) = maybe_t
            && let Err(e) = install_hooks(HookFilter { target: t }, cb.clone())
        {
            error!(?e, "install_hooks failed");
        }
    }

    // Register Start/Stop callback: when enabling, attempt to reapply mapping if target present; when disabling, reset mapping immediately.
    {
        let hctx_cell_cb = hctx_cell_outer.clone();
        let keep_aspect_flag_cb = keep_aspect_flag.clone();
        // Capture send_hwnd + final_options so we can force a context reopen when re-enabling mapping.
        let send_hwnd_for_toggle = send_hwnd;
        let final_options_for_toggle = final_options;
        let current_target_clone = current_target.clone();
        set_run_toggle_callback(Arc::new(move |enabled| {
            if enabled {
                // Get selector text and type from GUI
                if let Some(sel_txt) = gui::get_selector_text() {
                    let trimmed = sel_txt.trim();
                    if !trimmed.is_empty() {
                        let selector_type = get_selected_selector_type();
                        let parsed = match selector_type {
                            SelectorType::Process => Some(Target::ProcessName(trimmed.to_string())),
                            SelectorType::WindowClass => {
                                Some(Target::WindowClass(trimmed.to_string()))
                            }
                            SelectorType::Title => {
                                Some(Target::TitleSubstring(trimmed.to_string()))
                            }
                        };
                        if let Some(new_target) = parsed {
                            let mut guard = current_target_clone.lock().unwrap();
                            if guard.as_ref() != Some(&new_target) {
                                let already_installed = guard.is_some();
                                *guard = Some(new_target.clone());
                                if already_installed {
                                    let _ = update_target(new_target.clone());
                                } else {
                                    let _ = install_hooks(
                                        HookFilter {
                                            target: new_target.clone(),
                                        },
                                        cb.clone(),
                                    );
                                }
                            }
                        }
                    }
                }
                // If a target window is currently present, re-apply mapping (fresh rect query).
                if let Some(hwnd_cur) = winevent::find_existing_target() {
                    // Reopen the context explicitly because a foreground event may have occurred
                    // while mapping was disabled (and thus ignored), meaning we would miss the
                    // usual reopen_context() path used to work around driver resets on focus
                    // transitions. If reopening fails we still attempt to apply the mapping.
                    let _ = reopen_context(
                        &hctx_cell_cb,
                        send_hwnd_for_toggle,
                        base_ctx_for_cb,
                        final_options_for_toggle,
                    );
                    if let Some(rect) = winevent::query_window_rect(hwnd_cur) {
                        let ctx = rect_to_logcontext(
                            base_ctx_for_cb,
                            rect,
                            &MapConfig {
                                keep_aspect: keep_aspect_flag_cb.load(Ordering::Relaxed),
                            },
                        );
                        if keep_aspect_flag_cb.load(Ordering::Relaxed) {
                            let _ = reopen_with_template(
                                &hctx_cell_cb,
                                send_hwnd_for_toggle,
                                ctx,
                                final_options_for_toggle,
                            );
                        } else if let Ok(h) = hctx_cell_cb.lock() {
                            let _ = apply_mapping(*h, &ctx);
                        }
                        info!(
                            keep_aspect = keep_aspect_flag_cb.load(Ordering::Relaxed),
                            "run re-enabled mapping applied"
                        );
                        reflect_target_presence(HWND(std::ptr::null_mut()), true);
                    } else {
                        reflect_target_presence(HWND(std::ptr::null_mut()), false);
                    }
                } else {
                    // No target yet; just update presence (will show waiting state with yellow icon).
                    reflect_target_presence(HWND(std::ptr::null_mut()), false);
                }
            } else {
                // Disabled: reset mapping to full tablet.
                if let Ok(h) = hctx_cell_cb.lock() {
                    let _ = apply_mapping(*h, &base_ctx_clone);
                }
                // Presence might still be true, but tray coloring handled in GUI module; reflect presence again to update icon.
                reflect_target_presence(
                    HWND(std::ptr::null_mut()),
                    winevent::find_existing_target().is_some(),
                );
            }
        }));
    }

    // Aspect checkbox callback: update flag and reapply mapping if currently enabled and target present.
    {
        let keep_aspect_flag_cb = keep_aspect_flag.clone();
        let hctx_cell_cb = hctx_cell_outer.clone();
        let send_hwnd_for_toggle = send_hwnd; // reuse same capture names
        let final_options_for_toggle = final_options;
        set_aspect_toggle_callback(Arc::new(move |enabled| {
            keep_aspect_flag_cb.store(enabled, Ordering::Relaxed);
            if !is_run_enabled() {
                return;
            }
            if let Some(hwnd_cur) = winevent::find_existing_target()
                && let Some(rect) = winevent::query_window_rect(hwnd_cur)
            {
                let ctx = rect_to_logcontext(
                    base_ctx_for_cb,
                    rect,
                    &MapConfig {
                        keep_aspect: enabled,
                    },
                );
                if enabled {
                    if keep_aspect_flag_cb.load(Ordering::Relaxed) {
                        let _ = reopen_with_template(
                            &hctx_cell_cb,
                            send_hwnd_for_toggle,
                            ctx,
                            final_options_for_toggle,
                        );
                    } else if let Ok(h) = hctx_cell_cb.lock() {
                        let _ = apply_mapping(*h, &ctx);
                    }
                }
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
        }));
    }

    // Apply initial mapping immediately if target already exists.
    if current_target.lock().ok().and_then(|t| t.clone()).is_some() {
        if let Some(hwnd_init) = find_existing_target() {
            if let Some(rect) = query_window_rect(hwnd_init) {
                info!(?rect, "initial target window found; applying mapping");
                let ctx = rect_to_logcontext(base_ctx_for_cb, rect, &cfg_initial);
                if let Ok(h) = hctx_cell_outer.lock() {
                    if let Err(e) = apply_mapping(*h, &ctx) {
                        error!(?e, "initial apply_mapping failed");
                    }
                } else {
                    error!("mutex poisoned during initial mapping");
                }
                reflect_target_presence(HWND(std::ptr::null_mut()), true);
            }
        } else {
            reflect_target_presence(HWND(std::ptr::null_mut()), false);
        }
    } else {
        reflect_target_presence(HWND(std::ptr::null_mut()), false);
    }

    // Loop - use the GUI message loop which can handle both GUI and WinTab messages
    let _ = run_message_loop();
    // After loop exits, cleanup (if not already via Ctrl+C)
    uninstall_hooks();
    if let Ok(h) = hctx_cell_outer.lock() {
        wt_close(*h);
    }
    Ok(())
}
