//! Window -> tablet output area mapping logic.
//!
//! The core responsibility here is translating a target window's on‑screen rectangle into the
//! WinTab LOGCONTEXT output origin/extent fields while optionally preserving pen tablet aspect
//! ratio. The same extents are mirrored into the system output fields so that system cursor
//! alignment matches application packet coordinates.

use crate::wintab::{HCTX, LOGCONTEXTA, wt_set};
use anyhow::Result;
use tracing::trace;
use windows::Win32::Foundation::RECT;

/// Mapping configuration flags.
#[derive(Copy, Clone)]
pub struct MapConfig {
    pub keep_aspect: bool,
}

/// Derive an updated LOGCONTEXT from a base context and a window rectangle.
///
/// Behaviour:
/// * Clamps zero/negative window dimensions to 1 to avoid invalid extents.
/// * If `keep_aspect` is set, we crop the TABLET INPUT (virtually) by adjusting output extents to
///   fill the entire window while preserving the tablet aspect (scale uniformly). This means the
///   mapping always spans the full window rectangle; pen reaches every pixel (unused tablet edge
///   bands map outside the window logically).
/// * Centers the adjusted mapping if cropping is required (implemented by scaling factors only; output origin == window origin).
/// * Mirrors output fields into system fields to keep cursor behaviour consistent.
pub fn rect_to_logcontext(mut base: LOGCONTEXTA, rect: RECT, cfg: &MapConfig) -> LOGCONTEXTA {
    let win_w = (rect.right - rect.left).max(1);
    let win_h = (rect.bottom - rect.top).max(1);
    let out_w = win_w;
    let out_h = win_h;
    if cfg.keep_aspect {
        // True crop: adjust INPUT extents so aspect matches window; output always full window.
        let in_w_full = base.lcInExtX.abs().max(1);
        let in_h_full = base.lcInExtY.abs().max(1);
        let win_aspect = win_w as f64 / win_h as f64;
        let tab_aspect = in_w_full as f64 / in_h_full as f64;
        let mut in_w_new = in_w_full as f64;
        let mut in_h_new = in_h_full as f64;
        if win_aspect > tab_aspect {
            // Window wider -> need higher aspect -> crop tablet height
            in_h_new = (in_w_full as f64 / win_aspect).round().max(1.0);
        } else if win_aspect < tab_aspect {
            // Window taller -> crop tablet width
            in_w_new = (in_h_full as f64 * win_aspect).round().max(1.0);
        }
        // Center crop inside tablet input space. Preserve sign of original extents.
        let sign_w = if base.lcInExtX < 0 { -1 } else { 1 };
        let sign_h = if base.lcInExtY < 0 { -1 } else { 1 };
        let in_w_new_i = in_w_new as i32;
        let in_h_new_i = in_h_new as i32;
        let crop_dx = (in_w_full - in_w_new_i.abs()) / 2;
        let crop_dy = (in_h_full - in_h_new_i.abs()) / 2;
        // Shift origins (assuming lcInOrg* initially 0; if not we offset relative to original origin).
        if crop_dx > 0 {
            base.lcInOrgX += crop_dx * sign_w;
        }
        if crop_dy > 0 {
            base.lcInOrgY += crop_dy * sign_h;
        }
        base.lcInExtX = in_w_new_i * sign_w;
        base.lcInExtY = in_h_new_i * sign_h;
        trace!(
            win_w,
            win_h,
            in_w_full,
            in_h_full,
            in_w_new_i,
            in_h_new_i,
            crop_dx,
            crop_dy,
            "aspect crop input adjusted"
        );
    }
    base.lcOutOrgX = rect.left;
    base.lcOutOrgY = rect.top;
    base.lcOutExtX = out_w;
    base.lcOutExtY = out_h;

    // Always mirror into system output fields so system cursor mapping follows.
    base.lcSysExtX = base.lcOutExtX;
    base.lcSysExtY = base.lcOutExtY;
    base.lcSysOrgX = base.lcOutOrgX;
    base.lcSysOrgY = base.lcOutOrgY;
    base
}

/// Apply a modified LOGCONTEXT to an open WinTab context.
///
/// Thin wrapper around WTSetA with anyhow error propagation.
pub fn apply_mapping(hctx: HCTX, ctx: &LOGCONTEXTA) -> Result<()> {
    wt_set(hctx, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::RECT;

    fn base_ctx(in_w: i32, in_h: i32) -> LOGCONTEXTA {
        LOGCONTEXTA {
            lcInExtX: in_w,
            lcInExtY: in_h,
            ..Default::default()
        }
    }

    fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
        RECT {
            left: l,
            top: t,
            right: r,
            bottom: b,
        }
    }

    #[test]
    fn no_aspect_basic_mapping_and_system_mirror() {
        let base = base_ctx(5000, 4000);
        let rc = rect(100, 200, 1100, 1800); // 1000 x 1600
        let cfg = MapConfig { keep_aspect: false };
        let out = rect_to_logcontext(base, rc, &cfg);
        assert_eq!(out.lcOutOrgX, 100);
        assert_eq!(out.lcOutOrgY, 200);
        assert_eq!(out.lcOutExtX, 1000);
        assert_eq!(out.lcOutExtY, 1600);
        // system mirrors
        assert_eq!(out.lcSysOrgX, out.lcOutOrgX);
        assert_eq!(out.lcSysOrgY, out.lcOutOrgY);
        assert_eq!(out.lcSysExtX, out.lcOutExtX);
        assert_eq!(out.lcSysExtY, out.lcOutExtY);
    }

    #[test]
    fn keep_aspect_window_wider_crops_input_height() {
        let base = base_ctx(5000, 5000); // square tablet
        let rc = rect(0, 0, 1600, 900); // 16:9 window
        let cfg = MapConfig { keep_aspect: true };
        let out = rect_to_logcontext(base, rc, &cfg);
        // Output fills window
        assert_eq!(out.lcOutExtX, 1600);
        assert_eq!(out.lcOutExtY, 900);
        // Input height reduced (crop) -> expect new lcInExtY < original 5000
        assert!(out.lcInExtY < 5000);
    }

    #[test]
    fn keep_aspect_window_taller_crops_input_width() {
        let base = base_ctx(10000, 5000); // wide 2:1
        let rc = rect(0, 0, 1000, 1600); // tall window
        let cfg = MapConfig { keep_aspect: true };
        let out = rect_to_logcontext(base, rc, &cfg);
        assert_eq!(out.lcOutExtX, 1000);
        assert_eq!(out.lcOutExtY, 1600);
        assert!(out.lcInExtX < 10000);
    }

    #[test]
    fn negative_window_coordinates_preserved_in_origin() {
        let base = base_ctx(8000, 8000);
        let rc = rect(-200, -100, 800, 900); // 1000x1000
        let cfg = MapConfig { keep_aspect: false };
        let out = rect_to_logcontext(base, rc, &cfg);
        assert_eq!(out.lcOutOrgX, -200);
        assert_eq!(out.lcOutOrgY, -100);
        assert_eq!(out.lcOutExtX, 1000);
        assert_eq!(out.lcOutExtY, 1000);
    }

    #[test]
    fn degenerate_zero_size_window_clamped_to_one() {
        let base = base_ctx(5000, 5000);
        // zero-size rectangle
        let rc = rect(100, 200, 100, 200);
        let cfg = MapConfig { keep_aspect: false };
        let out = rect_to_logcontext(base, rc, &cfg);
        assert_eq!(out.lcOutExtX, 1);
        assert_eq!(out.lcOutExtY, 1);
        assert_eq!(out.lcOutOrgX, 100);
        assert_eq!(out.lcOutOrgY, 200);
    }

    #[test]
    fn extreme_ultrawide_window_square_tablet_crops_input_height() {
        let base = base_ctx(6000, 6000);
        let rc = rect(50, 20, 5170, 1460); // 5120x1440
        let cfg = MapConfig { keep_aspect: true };
        let out = rect_to_logcontext(base, rc, &cfg);
        assert_eq!(out.lcOutExtX, 5120);
        assert_eq!(out.lcOutExtY, 1440);
        assert!(out.lcInExtY < 6000);
    }
}
