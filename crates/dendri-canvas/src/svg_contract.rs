//! SVG asset contract for Simulation canvas (Phase B0).
//!
//! ## Dimensional rule
//!
//! Authored SVG **width/height in millimetres** must be **exact multiples of 2 mm**
//! (half-module). At 100% zoom that is a multiple of **24 px**
//! ([`crate::PX_PER_HALF_AT_100`]).
//!
//! | Asset | Path(s) | Declared size | 2 mm multiple? |
//! |---|---|---|---|
//! | LED | `content/assets/svg/components/led.svg` (+ `core/`) | 10 × 10 mm | yes |
//! | Pushbutton | `…/cmd-pushbutton-no-start.svg` | 24 × 24 mm | yes |
//! | Arduino Uno R3 | `…/boards/arduino-uno-r3.svg` | ≈74.80 × 53.40 mm | **no** (physical outline; left as-authored — resize not a small fix) |
//!
//! Bodies may still center on the half-module; **terminals** snap only to full
//! modules (4 mm / 48 px @ 100%).
//!
//! ## Stable ids for dynamic layers (usvg → `canvas::Path`)
//!
//! Phase B/C extract these by `id` via [`crate::svg_dyn`]. Keep them stable.
//!
//! ### LED (`profile = led`)
//! - `led-body` — die / luminous fill (on/off class hooks)
//! - `lens` — optical lens geometry
//! - `df-glow` — filter def for glow (static defs; glow draw may use Path in C)
//!
//! ### Pushbutton (`cmd-pushbutton-no-start`)
//! - `actuator` — moving button cap (press translation in C)
//! - `contact` — NO contact state (`open` / `closed` classes)
//!
//! ### Uno R3 board
//! - `led-pin13` — onboard LED tied to D13 (`data-state`)
//! - `board` / `board-body` — static chassis (Camada A `draw_svg`)
//! - Pin pad groups use descriptive ids (e.g. `D13 / LED / SCK`); prefer
//!   `data-pin` / class hooks over renaming pads.
//!
//! Breadboard asset is missing — out of Phase B scope.

use crate::{HALF_MODULE, PX_PER_HALF_AT_100};

/// Millimetres of one half-module (contract quantum for SVG outer size).
pub const SVG_SIZE_QUANTUM_MM: f64 = 2.0;

/// Pixels of one half-module at 100% zoom (24).
pub const SVG_SIZE_QUANTUM_PX_AT_100: f64 = PX_PER_HALF_AT_100;

/// True when `mm` is an exact multiple of 2 mm (within ε).
pub fn is_half_module_multiple_mm(mm: f64) -> bool {
    let q = mm / SVG_SIZE_QUANTUM_MM;
    (q - q.round()).abs() < 1e-6
}

/// Grid units for a half-module (`0.5`).
pub fn half_module_grid() -> f64 {
    HALF_MODULE
}

/// Required dynamic-layer ids per MVP asset.
pub mod required_ids {
    pub const LED: &[&str] = &["led-body", "lens"];
    pub const BUTTON: &[&str] = &["actuator", "contact"];
    pub const UNO: &[&str] = &["led-pin13", "board"];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantum_matches_grid_half_module() {
        assert_eq!(SVG_SIZE_QUANTUM_MM, 2.0);
        assert_eq!(SVG_SIZE_QUANTUM_PX_AT_100, 24.0);
        assert!(is_half_module_multiple_mm(10.0));
        assert!(is_half_module_multiple_mm(24.0));
        assert!(!is_half_module_multiple_mm(74.801872));
    }
}
