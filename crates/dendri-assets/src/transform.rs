//! Canonical unified 2D transform matrix $\mathbf{M}$ (`03-asset-and-profile-contract.md` §2.1):
//!
//! $$\mathbf{M} = \mathbf{T}(x, y) \cdot \mathbf{T}(c_x, c_y) \cdot \mathbf{R}(\theta) \cdot \mathbf{S}(m_x, m_y) \cdot \mathbf{T}(-c_x, -c_y)$$
//!
//! Both canvas hit-testing/wire-anchor resolution AND GPU/SVG rendering MUST use this exact
//! single transform matrix so pins, bodies, quadrant rotations (`0°/90°/180°/270°`), and mirroring
//! (`mirror_x`, `mirror_y`) never drift.

use serde::{Deserialize, Serialize};

/// Placed component/board pose in world SVG/canvas coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Pose2D {
    pub x: f64,
    pub y: f64,
    /// Quadrant rotation in degrees (`0`, `90`, `180`, `270`).
    pub rotation_deg: u16,
    pub mirror_x: bool,
    pub mirror_y: bool,
}

/// 2D affine matrix `[[a, c, tx], [b, d, ty], [0, 0, 1]]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine2D {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Affine2D {
    /// Identity matrix.
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    /// Pure translation $\mathbf{T}(dx, dy)$.
    #[must_use]
    pub const fn translate(dx: f64, dy: f64) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx: dx,
            ty: dy,
        }
    }

    /// Scale/mirror $\mathbf{S}(sx, sy)$.
    #[must_use]
    pub const fn scale(sx: f64, sy: f64) -> Self {
        Self {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// Exact quadrant rotation $\mathbf{R}(\theta)$ (in SVG clockwise screen coordinates).
    #[must_use]
    pub fn rotate_deg(deg: u16) -> Self {
        let norm = deg % 360;
        let (cos_t, sin_t) = match norm {
            0 => (1.0, 0.0),
            90 => (0.0, 1.0),
            180 => (-1.0, 0.0),
            270 => (0.0, -1.0),
            other => {
                let rad = f64::from(other) * core::f64::consts::PI / 180.0;
                (libm::cos(rad), libm::sin(rad))
            }
        };
        Self {
            a: cos_t,
            b: sin_t,
            c: -sin_t,
            d: cos_t,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// Matrix composition `self * rhs`.
    #[must_use]
    pub fn compose(self, rhs: Self) -> Self {
        Self {
            a: self.a * rhs.a + self.c * rhs.b,
            b: self.b * rhs.a + self.d * rhs.b,
            c: self.a * rhs.c + self.c * rhs.d,
            d: self.b * rhs.c + self.d * rhs.d,
            tx: self.a * rhs.tx + self.c * rhs.ty + self.tx,
            ty: self.b * rhs.tx + self.d * rhs.ty + self.ty,
        }
    }

    /// Builds the normative unified matrix $\mathbf{M}$ for an asset of local size `(width, height)`:
    /// `T(x, y) * T(cx, cy) * R(theta) * S(mx, my) * T(-cx, -cy)`.
    #[must_use]
    pub fn from_pose(pose: Pose2D, width: f64, height: f64) -> Self {
        let cx = width * 0.5;
        let cy = height * 0.5;
        let mx = if pose.mirror_x { -1.0 } else { 1.0 };
        let my = if pose.mirror_y { -1.0 } else { 1.0 };

        Self::translate(pose.x, pose.y)
            .compose(Self::translate(cx, cy))
            .compose(Self::rotate_deg(pose.rotation_deg))
            .compose(Self::scale(mx, my))
            .compose(Self::translate(-cx, -cy))
    }

    /// Applies this transform to local point `(px, py)`.
    #[must_use]
    pub fn transform_point(self, px: f64, py: f64) -> (f64, f64) {
        (
            self.a * px + self.c * py + self.tx,
            self.b * px + self.d * py + self.ty,
        )
    }
}

impl core::ops::Mul for Affine2D {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        self.compose(rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_transform_preserves_center_and_matches_stepwise_composition() {
        let w = 96.0;
        let h = 144.0;
        let cx = w * 0.5;
        let cy = h * 0.5;
        let pin = (24.0, 48.0);

        for &rot in &[0u16, 90, 180, 270] {
            for &mx in &[false, true] {
                for &my in &[false, true] {
                    let pose = Pose2D {
                        x: 144.0,
                        y: 96.0,
                        rotation_deg: rot,
                        mirror_x: mx,
                        mirror_y: my,
                    };
                    let m = Affine2D::from_pose(pose, w, h);

                    // Center always maps to `(x + cx, y + cy)`
                    let (wcx, wcy) = m.transform_point(cx, cy);
                    assert!((wcx - (pose.x + cx)).abs() < 1e-9);
                    assert!((wcy - (pose.y + cy)).abs() < 1e-9);

                    // Stepwise oracle: T(-cx,-cy) -> S(mx,my) -> R(rot) -> T(cx,cy) -> T(x,y)
                    let mut x = pin.0 - cx;
                    let mut y = pin.1 - cy;
                    if mx {
                        x = -x;
                    }
                    if my {
                        y = -y;
                    }
                    let (rx, ry) = match rot {
                        0 => (x, y),
                        90 => (-y, x),
                        180 => (-x, -y),
                        270 => (y, -x),
                        _ => unreachable!(),
                    };
                    let exp_x = rx + cx + pose.x;
                    let exp_y = ry + cy + pose.y;
                    let (got_x, got_y) = m.transform_point(pin.0, pin.1);
                    assert!((got_x - exp_x).abs() < 1e-9);
                    assert!((got_y - exp_y).abs() < 1e-9);
                }
            }
        }
    }
}
