//! Logical grid contract for the Simulation canvas.
//!
//! Author layout in **grid units** (1 u = 4 mm). Apply zoom only at render time:
//! `px = grid_units * 48 * zoom`. Connection terminals (bornes) sit on exact
//! full-module intersections; component bodies may center on the half-module.

/// Millimetres between adjacent connection terminals (one grid unit).
pub const GRID_UNIT_MM: f64 = 4.0;

/// Pixels for one grid unit at 100% zoom.
pub const PX_PER_GRID_AT_100: f64 = 48.0;

/// Half-module in grid units (body centering / sub-grid).
pub const HALF_MODULE: f64 = 0.5;

/// Pixels for a half-module at 100% zoom.
pub const PX_PER_HALF_AT_100: f64 = PX_PER_GRID_AT_100 * HALF_MODULE; // 24

/// Convert millimetres → grid units.
pub fn mm_to_grid(mm: f64) -> f64 {
    mm / GRID_UNIT_MM
}

/// Convert grid units → millimetres.
pub fn grid_to_mm(grid: f64) -> f64 {
    grid * GRID_UNIT_MM
}

/// Render-time scale: pixels per grid unit at the given zoom.
pub fn px_per_grid(zoom: f64) -> f64 {
    PX_PER_GRID_AT_100 * zoom
}

/// `px = grid_units * 48 * zoom`.
pub fn grid_to_px(grid: f64, zoom: f64) -> f64 {
    grid * px_per_grid(zoom)
}

/// Inverse of [`grid_to_px`].
pub fn px_to_grid(px: f64, zoom: f64) -> f64 {
    let s = px_per_grid(zoom);
    if s == 0.0 {
        0.0
    } else {
        px / s
    }
}

/// Snap to a full-module intersection (connection terminal / borne).
pub fn snap_terminal(grid: f64) -> f64 {
    grid.round()
}

/// Snap to the half-module sub-grid (body centers).
pub fn snap_half_module(grid: f64) -> f64 {
    (grid * 2.0).round() / 2.0
}

/// True when `grid` lies on a full-module intersection (within ε).
pub fn is_terminal_intersection(grid: f64) -> bool {
    (grid - grid.round()).abs() < 1e-9
}

/// Default C01 scene extent in grid units (20 × 13 → 960 × 624 px @ 100%).
pub const DEFAULT_SCENE_WIDTH_U: f64 = 20.0;
pub const DEFAULT_SCENE_HEIGHT_U: f64 = 13.0;

/// Pixel size of the default scene at `zoom`.
pub fn default_scene_px(zoom: f64) -> (f64, f64) {
    (
        grid_to_px(DEFAULT_SCENE_WIDTH_U, zoom),
        grid_to_px(DEFAULT_SCENE_HEIGHT_U, zoom),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_contract_at_100_percent() {
        assert_eq!(GRID_UNIT_MM, 4.0);
        assert_eq!(PX_PER_GRID_AT_100, 48.0);
        assert_eq!(PX_PER_HALF_AT_100, 24.0);
        assert_eq!(grid_to_px(1.0, 1.0), 48.0);
        assert_eq!(grid_to_px(0.5, 1.0), 24.0);
        assert_eq!(mm_to_grid(4.0), 1.0);
        assert_eq!(grid_to_mm(1.0), 4.0);
    }

    #[test]
    fn zoom_scales_without_breaking_ratios() {
        let z = 1.5;
        assert!((grid_to_px(1.0, z) - 72.0).abs() < 1e-9);
        assert!((grid_to_px(0.5, z) - 36.0).abs() < 1e-9);
        // Terminals stay on integer grid; px stay multiples of full-module px.
        let t = snap_terminal(3.2);
        assert_eq!(t, 3.0);
        assert!(is_terminal_intersection(t));
        let px = grid_to_px(t, z);
        assert!((px / px_per_grid(z) - t).abs() < 1e-9);
    }

    #[test]
    fn half_module_snap() {
        assert_eq!(snap_half_module(3.2), 3.0);
        assert_eq!(snap_half_module(3.3), 3.5);
        assert_eq!(snap_half_module(3.7), 3.5);
        assert_eq!(snap_half_module(3.8), 4.0);
    }

    #[test]
    fn default_scene_size() {
        let (w, h) = default_scene_px(1.0);
        assert_eq!(w, 960.0);
        assert_eq!(h, 624.0);
    }
}
