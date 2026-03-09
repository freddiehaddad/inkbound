/// Display area in OTD format: width, height, center_x, center_y.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayArea {
    pub width: f64,
    pub height: f64,
    pub center_x: f64,
    pub center_y: f64,
}

/// Tablet area including rotation.
#[derive(Debug, Clone, PartialEq)]
pub struct TabletArea {
    pub width: f64,
    pub height: f64,
    pub center_x: f64,
    pub center_y: f64,
    pub rotation: f64,
}

/// Compute the smallest display area that fully covers the given window
/// while preserving the given aspect ratio, centered on the window.
///
/// This ensures the pen can reach every part of the window. The mapped area
/// may extend slightly beyond the window in one direction to maintain the
/// tablet's aspect ratio.
///
/// Returns `None` if the window has zero or negative dimensions.
pub fn fit_to_window(
    window_left: i32,
    window_top: i32,
    window_width: i32,
    window_height: i32,
    tablet_aspect_ratio: f64,
) -> Option<DisplayArea> {
    if window_width <= 0 || window_height <= 0 {
        return None;
    }

    let w = window_width as f64;
    let h = window_height as f64;
    let window_aspect = w / h;

    let (fit_w, fit_h) = if window_aspect > tablet_aspect_ratio {
        // Window is wider than tablet ratio — expand height to cover width
        (w, w / tablet_aspect_ratio)
    } else {
        // Window is taller than tablet ratio — expand width to cover height
        (h * tablet_aspect_ratio, h)
    };

    let center_x = window_left as f64 + w / 2.0;
    let center_y = window_top as f64 + h / 2.0;

    Some(DisplayArea {
        width: fit_w,
        height: fit_h,
        center_x,
        center_y,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wider_window_expands_height() {
        // Window 1920x1080 (16:9), tablet 4:3 (1.333...)
        // Window is wider than tablet, so height expands to cover width
        let area = fit_to_window(0, 0, 1920, 1080, 4.0 / 3.0).unwrap();
        assert!((area.width - 1920.0).abs() < 0.01); // matches window width
        assert!((area.height - 1440.0).abs() < 0.01); // extends beyond window height
        assert!((area.center_x - 960.0).abs() < 0.01);
        assert!((area.center_y - 540.0).abs() < 0.01);
    }

    #[test]
    fn taller_window_expands_width() {
        // Window 800x1200, tablet 1.6 (16:10)
        // Window is taller than tablet, so width expands to cover height
        let area = fit_to_window(100, 200, 800, 1200, 1.6).unwrap();
        assert!((area.width - 1920.0).abs() < 0.01); // extends beyond window width
        assert!((area.height - 1200.0).abs() < 0.01); // matches window height
        assert!((area.center_x - 500.0).abs() < 0.01);
        assert!((area.center_y - 800.0).abs() < 0.01);
    }

    #[test]
    fn exact_aspect_ratio_match() {
        let area = fit_to_window(0, 0, 1600, 1000, 1.6).unwrap();
        assert!((area.width - 1600.0).abs() < 0.01);
        assert!((area.height - 1000.0).abs() < 0.01);
    }

    #[test]
    fn zero_dimensions_returns_none() {
        assert!(fit_to_window(0, 0, 0, 100, 1.6).is_none());
        assert!(fit_to_window(0, 0, 100, 0, 1.6).is_none());
        assert!(fit_to_window(0, 0, -10, 100, 1.6).is_none());
    }

    #[test]
    fn small_window() {
        // 10x10 window, tablet 1.6 ratio → width expands to cover height
        let area = fit_to_window(500, 300, 10, 10, 1.6).unwrap();
        assert!((area.width - 16.0).abs() < 0.01); // 10 * 1.6
        assert!((area.height - 10.0).abs() < 0.01);
    }

    #[test]
    fn offset_window_centers_correctly() {
        // Window at (100, 200) with size 400x400, tablet 2:1
        // Square window, tablet wider → width matches, height expands
        let area = fit_to_window(100, 200, 400, 400, 2.0).unwrap();
        assert!((area.width - 800.0).abs() < 0.01); // 400 * 2.0
        assert!((area.height - 400.0).abs() < 0.01);
        assert!((area.center_x - 300.0).abs() < 0.01); // 100 + 200
        assert!((area.center_y - 400.0).abs() < 0.01); // 200 + 200
    }
}
