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

use crate::events::{EventSeverity, push_ui_event};
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
/// 1. User desired / optimistic options (usually `CXO_MESSAGES | CXO_SYSTEM`).
/// 2. Desired minus `CXO_SYSTEM` (some drivers refuse system cursor integration initially).
/// 3. Minimal viability: `CXO_MESSAGES` only so we still receive packet messages.
///
/// Keeping the order deterministic ensures predictable logging and simplifies unit testing.
fn fallback_options(desired: u32) -> [u32; 3] {
    [desired, desired & !wintab::CXO_SYSTEM, CXO_MESSAGES]
}

/// Iterate candidate option sets invoking `try_open` until success.
///
/// The predicate receives each candidate option bitfield and returns `true` if an open
/// succeeded. The first `true` short‑circuits iteration; returning `None` signals all
/// variants failed.
fn select_first_working_option<F>(desired: u32, mut try_open: F) -> Option<u32>
where
    F: FnMut(u32) -> bool,
{
    fallback_options(desired)
        .into_iter()
        .find(|&opts| try_open(opts))
}

/// Open a WinTab context for `hwnd` applying a fallback sequence of option flags.
///
/// Returns `(HCTX, LOGCONTEXTA, options)` where `options` is the working flag combination.
/// The base `LOGCONTEXTA` is cloned per attempt so subsequent failures cannot mutate the
/// previously successful context template.
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
                push_ui_event(
                    EventSeverity::Info,
                    format!("WinTab context opened options=0x{opts:08X}"),
                );
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
    if picked.is_none() {
        push_ui_event(
            EventSeverity::Error,
            "WinTab context open failed for all option combinations",
        );
    }
    picked.ok_or_else(|| anyhow!("WTOpenA failed for all option combinations"))
}

/// Reopen the context after closing the previous handle using the original base template.
///
/// This is invoked on certain window activation events to work around drivers that reset
/// mapping unexpectedly when focus changes. The original `LOGCONTEXTA` template (with full
/// tablet extents) is reused; only option bits vary during fallback. Returns `true` if a new
/// context was opened.
pub fn reopen_context(
    hctx_cell: &Arc<Mutex<HCTX>>,
    hwnd: SendHwnd,
    base_ctx_template: LOGCONTEXTA,
    final_options: u32,
) -> Result<()> {
    let mut guard = hctx_cell
        .lock()
        .map_err(|_| anyhow!("context mutex poisoned (reopen)"))?;
    let old = *guard;
    wt_close(old);
    for opts in fallback_options(final_options) {
        let mut ctx_attempt = base_ctx_template;
        ctx_attempt.lcOptions = opts;
        match wt_open(hwnd.0, &ctx_attempt) {
            Ok(hnew) => {
                *guard = hnew;
                info!(options = format!("0x{opts:08X}"), "reopen WTOpen succeeded");
                push_ui_event(
                    EventSeverity::Info,
                    format!("Context reopened options=0x{opts:08X}"),
                );
                return Ok(());
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
    push_ui_event(EventSeverity::Error, "Context reopen failed (all options)");
    Err(anyhow!("all reopen attempts failed"))
}

/// Reopen context using an externally prepared `LOGCONTEXTA` template.
///
/// Unlike `reopen_context` this variant preserves caller‑provided geometry (e.g. aspect‑cropped
/// input extents) and only cycles the option flag fallback list. Used when re‑applying mapping
/// with aspect ratio preservation.
pub fn reopen_with_template(
    hctx_cell: &Arc<Mutex<HCTX>>,
    hwnd: SendHwnd,
    template: LOGCONTEXTA,
    final_options: u32,
) -> Result<()> {
    let mut guard = hctx_cell
        .lock()
        .map_err(|_| anyhow!("context mutex poisoned (reopen template)"))?;
    let old = *guard;
    wt_close(old);
    for opts in fallback_options(final_options) {
        let mut ctx_attempt = template;
        ctx_attempt.lcOptions = opts; // only vary options
        match wt_open(hwnd.0, &ctx_attempt) {
            Ok(hnew) => {
                *guard = hnew;
                info!(
                    options = format!("0x{opts:08X}"),
                    "reopen(template) succeeded"
                );
                return Ok(());
            }
            Err(e) => {
                error!(
                    options = format!("0x{opts:08X}"),
                    ?e,
                    "reopen(template) failed"
                );
            }
        }
    }
    error!("all reopen(template) attempts failed");
    push_ui_event(EventSeverity::Error, "Context reopen(template) failed");
    Err(anyhow!("all reopen(template) attempts failed"))
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
