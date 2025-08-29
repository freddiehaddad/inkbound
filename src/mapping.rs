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
/// * If `keep_aspect` is set, letter/pillar boxes the mapping inside the window while preserving
///   the tablet input aspect according to `lcInExtX` / `lcInExtY` of the base context.
/// * Centers the resulting output rectangle within the window bounds.
/// * Mirrors output fields into system fields to keep cursor behaviour consistent.
pub fn rect_to_logcontext(mut base: LOGCONTEXTA, rect: RECT, cfg: &MapConfig) -> LOGCONTEXTA {
    let win_w = (rect.right - rect.left).max(1);
    let win_h = (rect.bottom - rect.top).max(1);
    let mut out_w = win_w;
    let mut out_h = win_h;

    if cfg.keep_aspect {
        let in_w = base.lcInExtX.abs().max(1);
        let in_h = base.lcInExtY.abs().max(1);
        let in_aspect = in_w as f64 / in_h as f64;
        let win_aspect = win_w as f64 / win_h as f64;
        if win_aspect > in_aspect {
            // window wider -> reduce width
            out_w = (win_h as f64 * in_aspect).round() as i32;
        } else {
            // window taller -> reduce height
            out_h = (win_w as f64 / in_aspect).round() as i32;
        }
    }

    // Center inside window
    let offset_x = (win_w - out_w) / 2;
    let offset_y = (win_h - out_h) / 2;

    base.lcOutOrgX = rect.left + offset_x;
    base.lcOutOrgY = rect.top + offset_y;
    trace!(
        win_w,
        win_h, out_w, out_h, offset_x, offset_y, "mapping centered"
    );
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
        let mut ctx = LOGCONTEXTA::default();
        ctx.lcInExtX = in_w;
        ctx.lcInExtY = in_h;
        ctx
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
    fn keep_aspect_window_wider_centers_horizontally() {
        // Tablet is square (aspect 1). Window is wider (1.777...). Width should shrink, centered.
        let base = base_ctx(5000, 5000);
        let rc = rect(0, 0, 1600, 900);
        let cfg = MapConfig { keep_aspect: true };
        let out = rect_to_logcontext(base, rc, &cfg);
        // Expect width reduced to window height * aspect(=1) => 900; height full 900.
        assert_eq!(out.lcOutExtX, 900);
        assert_eq!(out.lcOutExtY, 900);
        // Centered: (1600 - 900)/2 = 350
        assert_eq!(out.lcOutOrgX, 350);
        assert_eq!(out.lcOutOrgY, 0);
    }

    #[test]
    fn keep_aspect_window_taller_centers_vertically() {
        // Tablet aspect 2.0 (wide). Square window 1000x1000 => height reduced.
        let base = base_ctx(10000, 5000); // aspect 2.0
        let rc = rect(0, 0, 1000, 1000);
        let cfg = MapConfig { keep_aspect: true };
        let out = rect_to_logcontext(base, rc, &cfg);
        // width stays 1000, height becomes 500.
        assert_eq!(out.lcOutExtX, 1000);
        assert_eq!(out.lcOutExtY, 500);
        // Centered vertically: (1000-500)/2 = 250
        assert_eq!(out.lcOutOrgX, 0);
        assert_eq!(out.lcOutOrgY, 250);
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
    fn extreme_ultrawide_window_with_square_tablet() {
        // Square tablet, ultra-wide window (32:9 example 5120x1440)
        let base = base_ctx(6000, 6000); // aspect 1
        let rc = rect(50, 20, 5170, 1460); // 5120 x 1440
        let cfg = MapConfig { keep_aspect: true };
        let out = rect_to_logcontext(base, rc, &cfg);
        // Height limits; width should become 1440 (match height * 1), centered horizontally.
        assert_eq!(out.lcOutExtY, 1440);
        assert_eq!(out.lcOutExtX, 1440);
        // Center offset: (5120-1440)/2 = 1840
        assert_eq!(out.lcOutOrgX, 50 + 1840);
        assert_eq!(out.lcOutOrgY, 20);
    }
}
