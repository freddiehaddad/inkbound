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

mod app_state;
mod callbacks;
mod cli;
mod context;
mod event_handlers;
mod gui;
mod initialization;
mod logging;
mod mapping;
mod winevent;
mod wintab;

use anyhow::Result;
use clap::{ArgAction, ArgGroup, Parser};
use std::sync::Arc;
use tracing::info;

use app_state::AppState;
use cli::cli_to_selector_config;
use context::open_context_with_fallback;
use gui::{create_main_window, run_message_loop};
use initialization::setup_callbacks_and_initial_mapping;
use logging::configure_logging;
use mapping::MapConfig;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
use winevent::uninstall_hooks;
use wintab::wt_close;

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

    // Configure logging according to CLI flags
    configure_logging(cli.quiet, cli.verbose);

    info!(
        version = env!("CARGO_PKG_VERSION"),
        ?cli,
        "starting inkbound"
    );

    // Convert CLI arguments to selector configuration
    let selector_config = cli_to_selector_config(&cli.process, &cli.win_class, &cli.title_contains);

    // Try to create the GUI window first, which will serve as our WinTab host
    let window_title = format!("InkBound Mapper v{}", env!("CARGO_PKG_VERSION"));

    // Determine initial run state: enabled if CLI selector provided, disabled otherwise
    let initial_run_enabled = selector_config.target.is_some();

    let hwnd = create_main_window(
        &window_title,
        "Target",
        &selector_config.selector_value,
        cli.preserve_aspect,
        selector_config.selector_type,
        initial_run_enabled,
    )?;

    let (hctx, base_ctx, final_options) = open_context_with_fallback(hwnd)?;

    // Create centralized application state
    let app_state = Arc::new(AppState::new(
        hctx,
        base_ctx,
        final_options,
        hwnd,
        selector_config.target.clone(),
        cli.preserve_aspect,
    ));

    // Ctrl+C handler -> graceful quit (must post WM_QUIT to ORIGINAL thread, PostQuitMessage on handler thread is ineffective)
    let main_tid = unsafe { GetCurrentThreadId() };
    {
        let app_state_cleanup = app_state.clone();
        ctrlc::set_handler(move || {
            tracing::info!("Ctrl+C received, shutting down");
            uninstall_hooks();
            if let Ok(h) = app_state_cleanup.wintab_context.lock() {
                wt_close(*h);
            }
            unsafe {
                let _ = PostThreadMessageW(main_tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        })
        .expect("ctrlc handler");
    }

    // Keep these for backward compatibility with existing callbacks during transition
    let cfg_initial = MapConfig {
        keep_aspect: cli.preserve_aspect,
    };
    let base_ctx_for_cb = base_ctx; // template for mapping

    // Setup callbacks, hooks, and initial mapping (combined to reduce Arc cloning)
    let _cb = setup_callbacks_and_initial_mapping(app_state.clone(), base_ctx_for_cb, &cfg_initial);

    // Loop - use the GUI message loop which can handle both GUI and WinTab messages
    let _ = run_message_loop();
    // After loop exits, cleanup (if not already via Ctrl+C)
    uninstall_hooks();
    if let Ok(h) = app_state.wintab_context.lock() {
        wt_close(*h);
    }
    Ok(())
}
