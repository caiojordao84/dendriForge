//! Electro-pneumatic multi-domain models (`02-simulation-and-electrical-engine.md` §5, ISO 1219-1):
//! - [`PneumaticValve52`]: 5/2-way directional solenoid valve (single-solenoid spring return or
//!   double-solenoid bistable `Y1`/`Y2`, 24 V DC coils, 6 bar supply).
//! - [`DoubleActingCylinder`]: Newtonian double-acting linear pneumatic actuator integrating real
//!   piston stroke $x(t) \in [0, L]$ from chamber pressures $P_A, P_B$.
//! - [`LimitSwitch`]: Mechanical end-of-stroke switch stamping a physical electrical contact into
//!   `dendri-circuit` when $x / L$ enters its trigger zone (`A0`/`A1`/`B0`/`B1`).

use crate::{
    CircuitStampCtx, ComponentVisualState, DomainInputs, DomainOutputs, PhysicalComponent,
};
use serde::{Deserialize, Serialize};

/// Parameters for a 5/2-way solenoid directional control valve (`pneu-valve-5-2`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PneumaticValve52Params {
    /// If `true`, double-solenoid bistable (`Y1` extends, `Y2` retracts, holds state when both off).
    /// If `false`, single-solenoid spring return (`Y1` extends, spring retracts when `Y1` de-energized).
    pub bistable: bool,
    /// Compressed-air supply pressure on port `1` (P) in bar gauge (default `6.0 bar`).
    pub supply_bar: f64,
    /// Minimum coil voltage (V) to shift the valve spool (default `16.0 V` for 24 V DC solenoid).
    pub v_actuate_v: f64,
    /// Release voltage (V) below which a single-solenoid spring returns (default `6.0 V`).
    pub v_release_v: f64,
}

impl Default for PneumaticValve52Params {
    fn default() -> Self {
        Self {
            bistable: false,
            supply_bar: 6.0,
            v_actuate_v: 16.0,
            v_release_v: 6.0,
        }
    }
}

/// 5/2-way electrically actuated pneumatic directional valve (`ISO 1219-1`).
///
/// Expected [`DomainInputs`]:
/// - `"v_y1"`: differential voltage across solenoid coil `Y1` (`Y1+` - `Y1-`).
/// - `"v_y2"`: differential voltage across solenoid coil `Y2` (when `bistable = true`).
/// - `"supply_bar"` (optional): overrides `params.supply_bar`.
///
/// Emitted [`DomainOutputs`]:
/// - `"pressure_a"`: pressure at outlet port `4` (A) in bar (`supply_bar` when extended, `0.0` when retracted).
/// - `"pressure_b"`: pressure at outlet port `2` (B) in bar (`0.0` when extended, `supply_bar` when retracted).
/// - `"spool_extended"`: `1.0` when extended, `0.0` when retracted.
#[derive(Debug, Clone)]
pub struct PneumaticValve52 {
    params: PneumaticValve52Params,
    extended: bool,
    y1_energized: bool,
    y2_energized: bool,
}

impl PneumaticValve52 {
    #[must_use]
    pub const fn new(params: PneumaticValve52Params) -> Self {
        Self {
            params,
            extended: false,
            y1_energized: false,
            y2_energized: false,
        }
    }

    #[must_use]
    pub const fn is_extended(&self) -> bool {
        self.extended
    }
}

impl PhysicalComponent for PneumaticValve52 {
    fn stamp_electrical(&self, _ctx: &mut CircuitStampCtx<'_>) {
        // Solenoid coils are modeled as physical resistors/inductors in the MNA circuit netlist;
        // valve reads coil voltages via `VoltageTap`s in `PhysicsWorld`.
    }

    fn step_physics(&mut self, _dt_us: u64, inputs: &DomainInputs) -> DomainOutputs {
        let v_y1 = libm::fabs(inputs.scalar_or("v_y1", 0.0));
        let v_y2 = libm::fabs(inputs.scalar_or("v_y2", 0.0));
        let p_sup = inputs
            .scalar_or("supply_bar", self.params.supply_bar)
            .max(0.0);

        self.y1_energized = if self.y1_energized {
            v_y1 >= self.params.v_release_v
        } else {
            v_y1 >= self.params.v_actuate_v
        };
        self.y2_energized = if self.y2_energized {
            v_y2 >= self.params.v_release_v
        } else {
            v_y2 >= self.params.v_actuate_v
        };

        if self.params.bistable {
            // Double solenoid: Y1 sets extended, Y2 resets retracted; if both off, hold memory!
            if self.y1_energized && !self.y2_energized {
                self.extended = true;
            } else if self.y2_energized && !self.y1_energized {
                self.extended = false;
            }
        } else {
            // Single solenoid spring return
            self.extended = self.y1_energized;
        }

        let (p_a, p_b) = if self.extended {
            (p_sup, 0.0)
        } else {
            (0.0, p_sup)
        };

        let mut out = DomainOutputs::default();
        out.scalars.insert("pressure_a".to_owned(), p_a);
        out.scalars.insert("pressure_b".to_owned(), p_b);
        out.scalars.insert(
            "spool_extended".to_owned(),
            if self.extended { 1.0 } else { 0.0 },
        );
        out.flags.insert("extended".to_owned(), self.extended);
        out
    }

    fn visual_state(&self) -> ComponentVisualState {
        let mut vis = ComponentVisualState::default();
        vis.scalars.insert(
            "spool_pos".to_owned(),
            if self.extended { 1.0 } else { 0.0 },
        );
        vis.flags
            .insert("y1_energized".to_owned(), self.y1_energized);
        vis.flags
            .insert("y2_energized".to_owned(), self.y2_energized);
        vis.flags.insert("extended".to_owned(), self.extended);
        vis
    }
}

/// Physical parameters for a double-acting linear pneumatic cylinder (`pneu-cylinder-da`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoubleActingCylinderParams {
    /// Piston bore diameter in mm (default `32.0 mm`).
    pub bore_mm: f64,
    /// Piston rod diameter in mm (default `12.0 mm`).
    pub rod_mm: f64,
    /// Full mechanical stroke length $L$ in mm (default `100.0 mm`).
    pub stroke_mm: f64,
    /// Equivalent moving mass (piston + rod + load) in kg (default `1.0 kg`).
    pub mass_kg: f64,
    /// Effective viscous flow/seal damping in N·s/m (default `1000.0 N·s/m` -> ~220 ms stroke @ 6 bar).
    pub damping_n_s_per_m: f64,
    /// Static seal friction force in N (default `15.0 N`).
    pub static_friction_n: f64,
    /// Constant external opposing axial load in N (default `0.0 N`).
    pub load_force_n: f64,
}

impl Default for DoubleActingCylinderParams {
    fn default() -> Self {
        Self {
            bore_mm: 32.0,
            rod_mm: 12.0,
            stroke_mm: 100.0,
            mass_kg: 1.0,
            damping_n_s_per_m: 1_000.0,
            static_friction_n: 15.0,
            load_force_n: 0.0,
        }
    }
}

/// Double-acting linear pneumatic cylinder (`ISO 1219-1`).
///
/// Expected [`DomainInputs`]: `"pressure_a"` (cap-side bar), `"pressure_b"` (rod-side bar).
/// Emitted [`DomainOutputs`]: `"position_mm"`, `"stroke_norm"` (`0.0..=1.0`), `"velocity_mm_s"`.
#[derive(Debug, Clone)]
pub struct DoubleActingCylinder {
    params: DoubleActingCylinderParams,
    position_m: f64,
    velocity_m_s: f64,
}

impl DoubleActingCylinder {
    #[must_use]
    pub const fn new(params: DoubleActingCylinderParams) -> Self {
        Self {
            params,
            position_m: 0.0,
            velocity_m_s: 0.0,
        }
    }

    /// Returns normalized stroke position $x / L \in [0.0, 1.0]$.
    #[must_use]
    pub fn stroke_norm(&self) -> f64 {
        let max_m = (self.params.stroke_mm * 1e-3).max(1e-6);
        (self.position_m / max_m).clamp(0.0, 1.0)
    }

    /// Returns current piston position in millimeters.
    #[must_use]
    pub fn position_mm(&self) -> f64 {
        self.position_m * 1_000.0
    }
}

impl PhysicalComponent for DoubleActingCylinder {
    fn stamp_electrical(&self, _ctx: &mut CircuitStampCtx<'_>) {}

    fn step_physics(&mut self, dt_us: u64, inputs: &DomainInputs) -> DomainOutputs {
        let max_m = (self.params.stroke_mm * 1e-3).max(1e-6);
        if dt_us > 0 {
            let p_a_pa = inputs.scalar_or("pressure_a", 0.0).max(0.0) * 1e5;
            let p_b_pa = inputs.scalar_or("pressure_b", 0.0).max(0.0) * 1e5;

            let d_bore = (self.params.bore_mm * 1e-3).max(1e-4);
            let d_rod = (self.params.rod_mm * 1e-3).clamp(0.0, d_bore * 0.9);
            let area_cap = 0.25 * core::f64::consts::PI * d_bore * d_bore;
            let area_rod = 0.25 * core::f64::consts::PI * (d_bore * d_bore - d_rod * d_rod);

            let f_pneu = p_a_pa * area_cap - p_b_pa * area_rod - self.params.load_force_n;
            let mass = self.params.mass_kg.max(1e-3);
            let damp = self.params.damping_n_s_per_m.max(1.0);
            let f_s = self.params.static_friction_n.max(0.0);

            // Sub-step in <= 1 ms increments for unconditional stability with high damping.
            let total_s = (dt_us as f64) * 1e-6;
            let substeps = (dt_us.div_ceil(1_000)).max(1);
            let dt = total_s / (substeps as f64);

            for _ in 0..substeps {
                let f_drive = if libm::fabs(self.velocity_m_s) < 1e-6 && libm::fabs(f_pneu) <= f_s {
                    0.0
                } else {
                    let sign = if libm::fabs(self.velocity_m_s) >= 1e-6 {
                        if self.velocity_m_s > 0.0 { 1.0 } else { -1.0 }
                    } else if f_pneu > 0.0 {
                        1.0
                    } else {
                        -1.0
                    };
                    f_pneu - sign * f_s
                };
                // Semi-implicit exponential damping update: exact & unconditionally stable for any `damp/mass`!
                let tau = mass / damp;
                let decay = libm::exp(-dt / tau);
                let v_terminal = f_drive / damp;
                self.velocity_m_s = v_terminal + (self.velocity_m_s - v_terminal) * decay;
                self.position_m += self.velocity_m_s * dt;

                if self.position_m <= 0.0 {
                    self.position_m = 0.0;
                    if self.velocity_m_s < 0.0 {
                        self.velocity_m_s = 0.0;
                    }
                } else if self.position_m >= max_m {
                    self.position_m = max_m;
                    if self.velocity_m_s > 0.0 {
                        self.velocity_m_s = 0.0;
                    }
                }
            }
        }

        let norm = (self.position_m / max_m).clamp(0.0, 1.0);
        let mut out = DomainOutputs::default();
        out.scalars
            .insert("position_mm".to_owned(), self.position_m * 1_000.0);
        out.scalars.insert("stroke_norm".to_owned(), norm);
        out.scalars
            .insert("velocity_mm_s".to_owned(), self.velocity_m_s * 1_000.0);
        out.flags.insert("at_retracted".to_owned(), norm <= 0.01);
        out.flags.insert("at_extended".to_owned(), norm >= 0.99);
        out
    }

    fn visual_state(&self) -> ComponentVisualState {
        let mut vis = ComponentVisualState::default();
        vis.scalars
            .insert("stroke_norm".to_owned(), self.stroke_norm());
        vis.scalars
            .insert("position_mm".to_owned(), self.position_mm());
        vis
    }
}

/// Parameters for a mechanical end-of-stroke limit switch (`sensor-limit-switch`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitSwitchParams {
    /// Component ID of the corresponding SPICE `Switch` inside `dendri-circuit::Circuit`.
    pub circuit_switch_id: String,
    /// Target normalized stroke position to actuate (`0.0` for retracted `A0`, `1.0` for extended `A1`).
    pub trigger_position_norm: f64,
    /// Half-width actuation band around `trigger_position_norm` (default `0.02`).
    pub trigger_band: f64,
    /// If `true`, normally-closed contact (opens when actuated); if `false`, normally-open (closes when actuated).
    pub normally_closed: bool,
}

/// Mechanical limit switch mechanically coupled to an actuator's normalized stroke position
/// and electrically stamped into `dendri-circuit` (`02` §5).
#[derive(Debug, Clone)]
pub struct LimitSwitch {
    params: LimitSwitchParams,
    actuated: bool,
}

impl LimitSwitch {
    #[must_use]
    pub fn new(params: LimitSwitchParams) -> Self {
        let initially_actuated =
            libm::fabs(0.0 - params.trigger_position_norm) <= params.trigger_band;
        Self {
            params,
            actuated: initially_actuated,
        }
    }

    /// Returns whether the electrical contact is currently closed.
    #[must_use]
    pub const fn is_contact_closed(&self) -> bool {
        if self.params.normally_closed {
            !self.actuated
        } else {
            self.actuated
        }
    }
}

impl PhysicalComponent for LimitSwitch {
    fn stamp_electrical(&self, ctx: &mut CircuitStampCtx<'_>) {
        let _ = ctx.set_switch(&self.params.circuit_switch_id, self.is_contact_closed());
    }

    fn step_physics(&mut self, _dt_us: u64, inputs: &DomainInputs) -> DomainOutputs {
        let pos = inputs.scalar_or("position_norm", 0.0);
        self.actuated =
            libm::fabs(pos - self.params.trigger_position_norm) <= self.params.trigger_band;
        let closed = self.is_contact_closed();
        let mut out = DomainOutputs::default();
        out.flags.insert("actuated".to_owned(), self.actuated);
        out.flags.insert("closed".to_owned(), closed);
        out
    }

    fn visual_state(&self) -> ComponentVisualState {
        let mut vis = ComponentVisualState::default();
        vis.flags.insert("actuated".to_owned(), self.actuated);
        vis.flags
            .insert("closed".to_owned(), self.is_contact_closed());
        vis
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valve_and_double_acting_cylinder_extend_and_retract_physically() {
        let mut valve = PneumaticValve52::new(PneumaticValve52Params::default());
        let mut cyl = DoubleActingCylinder::new(DoubleActingCylinderParams::default());

        // Energize Y1 with 24 V -> extend over 500 ms
        let mut v_in = DomainInputs::default();
        v_in.scalars.insert("v_y1".to_owned(), 24.0);
        let v_out = valve.step_physics(10_000, &v_in);
        assert_eq!(v_out.scalars.get("pressure_a"), Some(&6.0));
        assert_eq!(v_out.scalars.get("pressure_b"), Some(&0.0));

        let mut c_in = DomainInputs::default();
        c_in.scalars.insert("pressure_a".to_owned(), 6.0);
        c_in.scalars.insert("pressure_b".to_owned(), 0.0);
        for _ in 0..50 {
            cyl.step_physics(10_000, &c_in);
        }
        assert!(
            cyl.stroke_norm() > 0.99,
            "cylinder should reach full 100 mm extension, got {} mm",
            cyl.position_mm()
        );

        // De-energize Y1 -> spring return retracts cylinder back to 0 mm
        v_in.scalars.insert("v_y1".to_owned(), 0.0);
        let v_ret = valve.step_physics(10_000, &v_in);
        assert_eq!(v_ret.scalars.get("pressure_a"), Some(&0.0));
        assert_eq!(v_ret.scalars.get("pressure_b"), Some(&6.0));
        c_in.scalars.insert("pressure_a".to_owned(), 0.0);
        c_in.scalars.insert("pressure_b".to_owned(), 6.0);
        for _ in 0..60 {
            cyl.step_physics(10_000, &c_in);
        }
        assert!(
            cyl.stroke_norm() < 0.01,
            "cylinder should retract to 0 mm, got {} mm",
            cyl.position_mm()
        );
    }
}
