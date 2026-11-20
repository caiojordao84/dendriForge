//! World camera for the Simulation canvas.
//!
//! Coordinates are logical **grid units** (1 u = 4 mm). Zoom is applied only
//! at the view transform: `view = (world - pan) * (48 * zoom)`.

use crate::grid::{px_per_grid, PX_PER_GRID_AT_100};

/// Continuous zoom range (D3): 10% … 400%.
pub const ZOOM_MIN: f64 = 0.10;
pub const ZOOM_MAX: f64 = 4.00;
pub const ZOOM_DEFAULT: f64 = 1.00;

/// Point in the infinite world (grid units).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldPoint {
    pub x: f64,
    pub y: f64,
}

impl WorldPoint {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub fn snap_module(self) -> Self {
        Self {
            x: self.x.round(),
            y: self.y.round(),
        }
    }

    pub fn snap_half(self) -> Self {
        Self {
            x: (self.x * 2.0).round() / 2.0,
            y: (self.y * 2.0).round() / 2.0,
        }
    }
}

/// Size in world grid units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldSize {
    pub w: f64,
    pub h: f64,
}

impl WorldSize {
    pub const fn new(w: f64, h: f64) -> Self {
        Self { w, h }
    }
}

/// Axis-aligned world rectangle (min inclusive, max exclusive for iteration helpers).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldRect {
    pub min: WorldPoint,
    pub max: WorldPoint,
}

impl WorldRect {
    pub fn from_corners(a: WorldPoint, b: WorldPoint) -> Self {
        Self {
            min: WorldPoint::new(a.x.min(b.x), a.y.min(b.y)),
            max: WorldPoint::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }

    pub fn expand(self, margin_u: f64) -> Self {
        Self {
            min: WorldPoint::new(self.min.x - margin_u, self.min.y - margin_u),
            max: WorldPoint::new(self.max.x + margin_u, self.max.y + margin_u),
        }
    }
}

/// Camera: free pan (D4) + continuous zoom (D3). No page bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// World point mapped to the top-left of the viewport.
    pub pan: WorldPoint,
    /// Scale factor: 1.0 = 100% (48 px / grid unit).
    pub zoom: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            // Slight inset so the Uno at (1,2) is not flush against the chrome.
            pan: WorldPoint::new(-1.0, -1.0),
            zoom: ZOOM_DEFAULT,
        }
    }
}

impl Camera {
    pub fn clamp_zoom(zoom: f64) -> f64 {
        zoom.clamp(ZOOM_MIN, ZOOM_MAX)
    }

    pub fn with_zoom(mut self, zoom: f64) -> Self {
        self.zoom = Self::clamp_zoom(zoom);
        self
    }

    /// Pixels per grid unit at the current zoom.
    pub fn scale(&self) -> f64 {
        px_per_grid(self.zoom)
    }

    /// `view_px = (world - pan) * scale`.
    pub fn world_to_view(&self, world: WorldPoint) -> (f64, f64) {
        let s = self.scale();
        ((world.x - self.pan.x) * s, (world.y - self.pan.y) * s)
    }

    /// Inverse of [`Self::world_to_view`].
    pub fn view_to_world(&self, view_x: f64, view_y: f64) -> WorldPoint {
        let s = self.scale();
        if s == 0.0 {
            return self.pan;
        }
        WorldPoint::new(view_x / s + self.pan.x, view_y / s + self.pan.y)
    }

    /// World rectangle covered by a viewport of `width_px` × `height_px`.
    pub fn visible_world(&self, width_px: f64, height_px: f64) -> WorldRect {
        let tl = self.view_to_world(0.0, 0.0);
        let br = self.view_to_world(width_px, height_px);
        WorldRect::from_corners(tl, br)
    }

    /// Pan by a view-space pixel delta (middle-mouse drag).
    pub fn pan_by_view_delta(&mut self, dx_px: f64, dy_px: f64) {
        let s = self.scale();
        if s == 0.0 {
            return;
        }
        // Dragging the view right moves the world left under the camera.
        self.pan.x -= dx_px / s;
        self.pan.y -= dy_px / s;
    }

    /// Zoom while keeping the world point under `anchor_view` fixed (scroll-wheel).
    pub fn zoom_at_view(&mut self, new_zoom: f64, anchor_view_x: f64, anchor_view_y: f64) {
        let old_scale = self.scale();
        let world = self.view_to_world(anchor_view_x, anchor_view_y);
        self.zoom = Self::clamp_zoom(new_zoom);
        let new_scale = self.scale();
        if new_scale == 0.0 {
            return;
        }
        // pan' = world - anchor / new_scale  (so world stays under anchor)
        self.pan.x = world.x - anchor_view_x / new_scale;
        self.pan.y = world.y - anchor_view_y / new_scale;
        let _ = old_scale;
    }

    /// Multiply zoom by `factor` around a view anchor (e.g. wheel step).
    pub fn zoom_by_at_view(&mut self, factor: f64, anchor_view_x: f64, anchor_view_y: f64) {
        self.zoom_at_view(self.zoom * factor, anchor_view_x, anchor_view_y);
    }

    /// Baseline px/u at 100% (for diagnostics / overlays).
    pub fn px_per_grid_at_100() -> f64 {
        PX_PER_GRID_AT_100
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_clamped_to_product_range() {
        assert_eq!(Camera::clamp_zoom(0.01), ZOOM_MIN);
        assert_eq!(Camera::clamp_zoom(10.0), ZOOM_MAX);
        assert_eq!(Camera::clamp_zoom(1.0), 1.0);
    }

    #[test]
    fn world_view_roundtrip() {
        let cam = Camera {
            pan: WorldPoint::new(2.0, 3.0),
            zoom: 1.0,
        };
        let (vx, vy) = cam.world_to_view(WorldPoint::new(4.0, 5.0));
        // (4-2)*48 = 96, (5-3)*48 = 96
        assert!((vx - 96.0).abs() < 1e-9);
        assert!((vy - 96.0).abs() < 1e-9);
        let back = cam.view_to_world(vx, vy);
        assert!((back.x - 4.0).abs() < 1e-9);
        assert!((back.y - 5.0).abs() < 1e-9);
    }

    #[test]
    fn zoom_at_view_keeps_anchor_world() {
        let mut cam = Camera::default();
        let anchor = (240.0, 120.0);
        let before = cam.view_to_world(anchor.0, anchor.1);
        cam.zoom_at_view(2.0, anchor.0, anchor.1);
        let after = cam.view_to_world(anchor.0, anchor.1);
        assert!((before.x - after.x).abs() < 1e-9);
        assert!((before.y - after.y).abs() < 1e-9);
        assert!((cam.zoom - 2.0).abs() < 1e-9);
    }

    #[test]
    fn pan_by_view_delta_moves_world() {
        let mut cam = Camera {
            pan: WorldPoint::ZERO,
            zoom: 1.0,
        };
        // Drag 48 px right → pan.x decreases by 1 u
        cam.pan_by_view_delta(48.0, 0.0);
        assert!((cam.pan.x - (-1.0)).abs() < 1e-9);
    }

    #[test]
    fn visible_world_grows_when_zooming_out() {
        let cam = Camera::default().with_zoom(1.0);
        let near = cam.visible_world(480.0, 480.0);
        let far = cam.with_zoom(0.1).visible_world(480.0, 480.0);
        let near_w = near.max.x - near.min.x;
        let far_w = far.max.x - far.min.x;
        assert!(far_w > near_w * 5.0);
    }
}
