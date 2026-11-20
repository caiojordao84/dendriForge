//! Phase A world scene: Uno + LED placeholders in logical grid units.
//!
//! Terminals (bornes) sit on full-module intersections. Body centers may use
//! the half-module. No wire editor / page bounds — camera shows an infinite
//! line grid under these instances.

use crate::camera::WorldPoint;
use crate::grid::{is_terminal_intersection, snap_half_module};

/// One connection terminal on a full-module intersection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Terminal {
    pub id: &'static str,
    pub label: &'static str,
    pub pos: WorldPoint,
    pub ground: bool,
}

/// Arduino Uno R3 board placeholder (logical modules, not screen px).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnoPlaceholder {
    /// Top-left of the board body (module lattice).
    pub origin: WorldPoint,
    /// Body size in grid units.
    pub size: WorldPoint,
    pub d13: Terminal,
    pub gnd: Terminal,
}

impl Default for UnoPlaceholder {
    fn default() -> Self {
        Self {
            origin: WorldPoint::new(1.0, 2.0),
            // 7×8 modules — same footprint as the prior C01 block, without wires.
            size: WorldPoint::new(7.0, 8.0),
            d13: Terminal {
                id: "board.D13",
                label: "D13",
                pos: WorldPoint::new(8.0, 3.0),
                ground: false,
            },
            gnd: Terminal {
                id: "board.GND",
                label: "GND",
                pos: WorldPoint::new(8.0, 7.0),
                ground: true,
            },
        }
    }
}

/// LED module placeholder with anode / cathode bornes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LedPlaceholder {
    /// Body center (half-module allowed).
    pub center: WorldPoint,
    /// Body radius in grid units.
    pub radius_u: f64,
    pub anode: Terminal,
    pub cathode: Terminal,
}

impl Default for LedPlaceholder {
    fn default() -> Self {
        Self {
            center: WorldPoint::new(14.5, 3.5),
            radius_u: 1.35,
            anode: Terminal {
                id: "led1.A",
                label: "A",
                pos: WorldPoint::new(13.0, 3.0),
                ground: false,
            },
            cathode: Terminal {
                id: "led1.K",
                label: "K",
                pos: WorldPoint::new(13.0, 4.0),
                ground: true,
            },
        }
    }
}

/// Phase A scene graph: Uno + LED only (no wire editor).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhaseAScene {
    pub uno: UnoPlaceholder,
    pub led: LedPlaceholder,
}

impl Default for PhaseAScene {
    fn default() -> Self {
        let scene = Self {
            uno: UnoPlaceholder::default(),
            led: LedPlaceholder::default(),
        };
        scene.assert_terminal_contract();
        scene
    }
}

impl PhaseAScene {
    /// Debug / test: every borne must be on a full-module intersection.
    pub fn assert_terminal_contract(&self) {
        for t in [
            self.uno.d13,
            self.uno.gnd,
            self.led.anode,
            self.led.cathode,
        ] {
            assert!(
                is_terminal_intersection(t.pos.x) && is_terminal_intersection(t.pos.y),
                "{} ({},{}) not on full-module intersection",
                t.id,
                t.pos.x,
                t.pos.y
            );
        }
        let c = self.led.center;
        let sx = snap_half_module(c.x);
        let sy = snap_half_module(c.y);
        assert!(
            (sx - c.x).abs() < 1e-9 && (sy - c.y).abs() < 1e-9,
            "led center ({},{}) not on half-module",
            c.x,
            c.y
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_a_placeholders_snap_contract() {
        PhaseAScene::default().assert_terminal_contract();
    }
}
