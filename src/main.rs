//! Event‑driven Wacom (WinTab) tablet mapper.
//!
//! This binary opens a WinTab context bound to an invisible message window and dynamically
//! remaps the tablet output area to match a user‑selected target window (process name, class
//! name, or title substring). The mapping updates immediately in response to WinEvent hook
//! notifications (size / move / foreground / creation events); no polling loops are used.
//!
//! High‑level flow:
//! 1. Parse CLI (one required selector + behavioural flags).
//! 2. Initialize tracing from RUST_LOG and create a hidden message‑only host window.
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
mod mapping;
mod winevent;
mod winhost;
mod wintab;

use anyhow::Result;
use clap::{ArgAction, ArgGroup, Parser};
use std::sync::{Arc, Mutex};
use tracing::{error, info};

use context::{SendHwnd, open_context_with_fallback, reopen_context};
use mapping::{MapConfig, apply_mapping, rect_to_logcontext};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
use winevent::{
    HookFilter, Target, find_existing_target, install_hooks, query_window_rect, uninstall_hooks,
};
use winhost::{create_message_window, run_message_loop};
use wintab::{wt_close, wt_get};

/// Command line interface definition.
#[derive(Parser, Debug)]
#[command(version, about="Map a Wacom tablet area dynamically to a chosen window (process, class, or title) without polling.", group=ArgGroup::new("selector").required(true).args(["process", "win_class", "title_contains"]))]
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
    /// Preserve tablet aspect ratio; letter‑box / pillar‑box inside the window rectangle.
    preserve_aspect: bool,
    /// While another window is foreground, temporarily revert to full tablet mapping
    #[arg(long = "full-when-unfocused", alias = "reset-on-foreground-loss")]
    /// Temporarily reset mapping to the full tablet whenever another window becomes foreground.
    full_when_unfocused: bool,
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

    info!(?cli, "starting pentarget");

    let hwnd = match create_message_window("WINTAB_MAPPER_HOST") {
        Ok(h) => h,
        Err(e) => {
            error!(?e, "failed to create message window");
            return Err(e);
        }
    };

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

    let target = if let Some(p) = &cli.process {
        Target::ByProcessName(p.clone())
    } else if let Some(c) = &cli.win_class {
        Target::ByWindowClass(c.clone())
    } else {
        Target::ByTitleSubstring(cli.title_contains.clone().unwrap())
    };

    let cfg = MapConfig {
        keep_aspect: cli.preserve_aspect,
    };
    let base_ctx_clone = base_ctx; // original for reset
    let base_ctx_for_cb = base_ctx; // template for mapping
    let send_hwnd = SendHwnd(hwnd);
    let hctx_cell_for_cb = hctx_cell.clone();
    let cb = Arc::new(
        move |hwnd: HWND, event: u32, mut rect: windows::Win32::Foundation::RECT| {
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
            if (event == EVENT_OBJECT_DESTROY || event == EVENT_SYSTEM_MINIMIZESTART)
                || (event == EVENT_SYSTEM_FOREGROUND
                    && rect.left == 0
                    && rect.right == 0
                    && rect.top == 0
                    && rect.bottom == 0)
            {
                if let Ok(h) = hctx_cell_for_cb.lock() {
                    if let Err(e) = apply_mapping(*h, &base_ctx_clone) {
                        error!(?e, "reset mapping failed");
                    }
                } else {
                    error!("mutex poisoned on reset mapping");
                }
                return;
            }
            if event == EVENT_SYSTEM_FOREGROUND {
                if !reopen_context(&hctx_cell_for_cb, send_hwnd, base_ctx_for_cb, final_options) {
                    return;
                }
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
            let ctx = rect_to_logcontext(base_ctx_for_cb, rect, &cfg);
            if let Ok(h) = hctx_cell_for_cb.lock() {
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
                    if std::env::var("WINTAB_DUMP").as_deref() == Ok("1") {
                        if let Ok(cur) = wt_get(*h) {
                            info!(
                                out_org_x = cur.lcOutOrgX,
                                out_org_y = cur.lcOutOrgY,
                                out_ext_x = cur.lcOutExtX,
                                out_ext_y = cur.lcOutExtY,
                                sys_org_x = cur.lcSysOrgX,
                                sys_org_y = cur.lcSysOrgY,
                                sys_ext_x = cur.lcSysExtX,
                                sys_ext_y = cur.lcSysExtY,
                                "post-set context state"
                            );
                        }
                    }
                }
            } else {
                error!("mutex poisoned applying mapping");
            }
        },
    );
    if let Err(e) = install_hooks(
        HookFilter {
            target,
            reset_on_foreground_loss: cli.full_when_unfocused,
        },
        cb,
    ) {
        error!(?e, "install_hooks failed");
    }

    // Apply initial mapping immediately if target already exists.
    if let Some(hwnd_init) = find_existing_target() {
        if let Some(rect) = query_window_rect(hwnd_init) {
            info!(?rect, "initial target window found; applying mapping");
            let ctx = rect_to_logcontext(base_ctx_for_cb, rect, &cfg);
            if let Ok(h) = hctx_cell_outer.lock() {
                if let Err(e) = apply_mapping(*h, &ctx) {
                    error!(?e, "initial apply_mapping failed");
                }
            } else {
                error!("mutex poisoned during initial mapping");
            }
        }
    }

    // Loop
    let _ = run_message_loop();
    // After loop exits, cleanup (if not already via Ctrl+C)
    uninstall_hooks();
    if let Ok(h) = hctx_cell_outer.lock() {
        wt_close(*h);
    }
    Ok(())
}
