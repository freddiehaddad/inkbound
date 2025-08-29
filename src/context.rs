//! WinTab context acquisition and reopen helpers.
//!
//! This module encapsulates:
//! * A deterministic fallback strategy for WTOpen option flags (some drivers reject CXO_SYSTEM).
//! * Re‑opening logic used on foreground activation to restore mapping when drivers silently
//!   reset context state.
//! * Small pure helpers with unit tests for option selection.
//!
//! Exposed functions are thread‑safe where necessary; the actual HCTX is stored behind a
//! `Mutex` provided by the caller and only manipulated on the originating thread.

use anyhow::{Result, anyhow};
use std::sync::{Arc, Mutex};
use tracing::{error, info};
use windows::Win32::Foundation::HWND;

use crate::wintab::{self, CXO_MESSAGES, HCTX, LOGCONTEXTA, wt_close, wt_info_defcontext, wt_open};

/// Wrapper to allow capturing an HWND inside a Send + Sync closure (only used on original thread).
#[derive(Copy, Clone)]
pub struct SendHwnd(pub HWND);
unsafe impl Send for SendHwnd {}
unsafe impl Sync for SendHwnd {}

/// Compute ordered fallback option sets for WTOpen attempts.
///
/// Order:
/// 1. Desired options (usually CXO_MESSAGES | CXO_SYSTEM | possibly others from driver).
/// 2. Desired without CXO_SYSTEM (some drivers reject system cursor integration initially).
/// 3. Minimal CXO_MESSAGES only.
fn fallback_options(desired: u32) -> [u32; 3] {
    [desired, desired & !wintab::CXO_SYSTEM, CXO_MESSAGES]
}

/// Iterate candidate option sets invoking `try_open` until success.
///
/// Returns the first successful option value or `None` if all candidates fail.
fn select_first_working_option<F>(desired: u32, mut try_open: F) -> Option<u32>
where
    F: FnMut(u32) -> bool,
{
    for opts in fallback_options(desired) {
        if try_open(opts) {
            return Some(opts);
        }
    }
    None
}

/// Open a WinTab context for `hwnd`, applying a fallback sequence of option flags.
///
/// Returns the opened context handle, the (possibly modified) LOGCONTEXT used, and the
/// final option flags that succeeded. Any failure across all attempts is surfaced as an error.
pub fn open_context_with_fallback(hwnd: HWND) -> Result<(HCTX, LOGCONTEXTA, u32)> {
    let mut base_ctx = wt_info_defcontext()?;
    base_ctx.lcOptions |= CXO_MESSAGES | wintab::CXO_SYSTEM; // desired starting flags
    let desired = base_ctx.lcOptions;
    let mut picked: Option<(HCTX, LOGCONTEXTA, u32)> = None;
    let _ = select_first_working_option(desired, |opts| {
        let mut ctx_attempt = base_ctx;
        ctx_attempt.lcOptions = opts;
        match wt_open(hwnd, &ctx_attempt) {
            Ok(h) => {
                info!(options = format!("0x{opts:08X}"), "WTOpen succeeded");
                picked = Some((h, ctx_attempt, opts));
                true
            }
            Err(e) => {
                error!(
                    options = format!("0x{opts:08X}"),
                    ?e,
                    "WTOpen attempt failed"
                );
                false
            }
        }
    });
    picked.ok_or_else(|| anyhow!("WTOpenA failed for all option combinations"))
}

/// Reopen an existing context after closing the previous handle, attempting the fallback
/// sequence starting from the last known working option set.
///
/// On success updates the handle in `hctx_cell` and returns true; on failure the old handle
/// has already been closed and false is returned (caller may skip mapping update).
pub fn reopen_context(
    hctx_cell: &Arc<Mutex<HCTX>>,
    hwnd: SendHwnd,
    base_ctx_template: LOGCONTEXTA,
    final_options: u32,
) -> bool {
    if let Ok(mut guard) = hctx_cell.lock() {
        let old = *guard;
        wt_close(old);
        for opts in fallback_options(final_options) {
            let mut ctx_attempt = base_ctx_template;
            ctx_attempt.lcOptions = opts;
            match wt_open(hwnd.0, &ctx_attempt) {
                Ok(hnew) => {
                    *guard = hnew;
                    info!(options = format!("0x{opts:08X}"), "reopen WTOpen succeeded");
                    return true;
                }
                Err(e) => {
                    error!(
                        options = format!("0x{opts:08X}"),
                        ?e,
                        "reopen WTOpen failed"
                    );
                }
            }
        }
        error!("all reopen attempts failed; mapping update skipped");
        false
    } else {
        error!("failed to lock context mutex for reopen");
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{fallback_options, select_first_working_option};

    #[test]
    fn fallback_order_contains_desired_then_reduced_then_messages() {
        let desired = 0b1110u32;
        let fo = fallback_options(desired);
        assert_eq!(fo[0], desired);
        assert_eq!(fo[1], desired & !crate::wintab::CXO_SYSTEM);
        assert_eq!(fo[2], crate::wintab::CXO_MESSAGES);
    }

    #[test]
    fn select_picks_first_success() {
        let desired = 0xAAu32;
        let mut calls = Vec::new();
        let picked = select_first_working_option(desired, |opts| {
            calls.push(opts);
            opts == desired
        });
        assert_eq!(picked, Some(desired));
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn select_skips_failed_and_picks_second() {
        let desired = 0xAAu32;
        let seq = fallback_options(desired);
        let mut idx = 0usize;
        let picked = select_first_working_option(desired, |_opts| {
            let attempt = idx;
            idx += 1;
            attempt == 1 // fail first (0), succeed second (1)
        });
        assert_eq!(picked, Some(seq[1]));
        assert_eq!(idx, 2);
    }

    #[test]
    fn select_returns_none_if_all_fail() {
        let desired = 0x55u32;
        let picked = select_first_working_option(desired, |_opts| false);
        assert!(picked.is_none());
    }
}
