//! Lumped fluid level tank plant with Torricelli gravity outflow, inlet/drain valve commands,
//! high/low float switches, and hydrostatic 4–20 mA level transmitter (`02` §5, ADR-012).
//!
//! Differential equation:
//! $$A \frac{dh}{dt} = u_{\text{in}} Q_{\text{in,max}} - u_{\text{out}} C_d a_{\text{out}} \sqrt{2 g h}$$

use crate::{
    CircuitStampCtx, ComponentVisualState, DomainInputs, DomainOutputs, PhysicalComponent,
};
use serde::{Deserialize, Serialize};

const GRAVITY_M_S2: f64 = 9.806_65;

/// Physical parameters for [`FluidTank`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FluidTankParams {
    /// Tank cross-sectional area $A$ in $\text{m}^2$ (default `0.5 m²`).
    pub area_m2: f64,
    /// Maximum tank height $H_{\max}$ in meters (default `2.0 m`).
    pub max_height_m: f64,
    /// Initial fluid level in meters (default `0.0 m`).
    pub initial_height_m: f64,
    /// Maximum inlet volumetric flow $Q_{\text{in,max}}$ in $\text{m}^3/\text{s}$ (default `0.01 m³/s` = 10 L/s).
    pub max_inflow_m3_s: f64,
    /// Effective drain orifice area $C_d \cdot a_{\text{out}}$ in $\text{m}^2$ (default `0.002 m²`).
    pub eff_outflow_area_m2: f64,
    /// Low-level float switch threshold (normalized `0..1`, default `0.15`).
    pub low_switch_norm: f64,
    /// High-level float switch threshold (normalized `0..1`, default `0.85`).
    pub high_switch_norm: f64,
    /// Optional SPICE switch IDs & level transmitter current source ID in `Circuit`.
    pub low_switch_circuit_id: Option<String>,
    pub high_switch_circuit_id: Option<String>,
    pub level_tx_circuit_id: Option<String>,
}

impl Default for FluidTankParams {
    fn default() -> Self {
        Self {
            area_m2: 0.5,
            max_height_m: 2.0,
            initial_height_m: 0.0,
            max_inflow_m3_s: 0.01,
            eff_outflow_area_m2: 0.002,
            low_switch_norm: 0.15,
            high_switch_norm: 0.85,
            low_switch_circuit_id: None,
            high_switch_circuit_id: None,
            level_tx_circuit_id: None,
        }
    }
}

/// Gravity-drained fluid level tank model.
#[derive(Debug, Clone)]
pub struct FluidTank {
    params: FluidTankParams,
    height_m: f64,
    overflowing: bool,
}

impl FluidTank {
    #[must_use]
    pub fn new(params: FluidTankParams) -> Self {
        let h0 = params
            .initial_height_m
            .clamp(0.0, params.max_height_m.max(1e-3));
        Self {
            params,
            height_m: h0,
            overflowing: false,
        }
    }

    #[must_use]
    pub const fn height_m(&self) -> f64 {
        self.height_m
    }

    #[must_use]
    pub fn level_norm(&self) -> f64 {
        (self.height_m / self.params.max_height_m.max(1e-6)).clamp(0.0, 1.0)
    }

    #[must_use]
    pub fn is_low_level(&self) -> bool {
        self.level_norm() <= self.params.low_switch_norm
    }

    #[must_use]
    pub fn is_high_level(&self) -> bool {
        self.level_norm() >= self.params.high_switch_norm
    }

    #[must_use]
    pub fn transmitter_current_a(&self) -> f64 {
        0.004 + self.level_norm() * 0.016
    }
}

impl PhysicalComponent for FluidTank {
    fn stamp_electrical(&self, ctx: &mut CircuitStampCtx<'_>) {
        if let Some(ref sw) = self.params.low_switch_circuit_id {
            let _ = ctx.set_switch(sw, self.is_low_level());
        }
        if let Some(ref sw) = self.params.high_switch_circuit_id {
            let _ = ctx.set_switch(sw, self.is_high_level());
        }
        if let Some(ref tx) = self.params.level_tx_circuit_id {
            let _ = ctx.set_current_source(tx, self.transmitter_current_a());
        }
    }

    fn step_physics(&mut self, dt_us: u64, inputs: &DomainInputs) -> DomainOutputs {
        let max_h = self.params.max_height_m.max(1e-3);
        let area = self.params.area_m2.max(1e-4);
        let u_in = inputs.scalar_or("inlet_valve", 0.0).clamp(0.0, 1.0);
        let u_out = inputs.scalar_or("drain_valve", 1.0).clamp(0.0, 1.0);

        if dt_us > 0 {
            let total_s = (dt_us as f64) * 1e-6;
            let substeps = dt_us.div_ceil(10_000).max(1);
            let dt = total_s / (substeps as f64);

            for _ in 0..substeps {
                let q_in = u_in * self.params.max_inflow_m3_s.max(0.0);
                let q_out = u_out
                    * self.params.eff_outflow_area_m2.max(0.0)
                    * libm::sqrt((2.0 * GRAVITY_M_S2 * self.height_m).max(0.0));
                let dh = ((q_in - q_out) / area) * dt;
                self.height_m += dh;
                if self.height_m < 0.0 {
                    self.height_m = 0.0;
                    self.overflowing = false;
                } else if self.height_m > max_h {
                    self.height_m = max_h;
                    self.overflowing = true;
                } else {
                    self.overflowing = false;
                }
            }
        }

        let norm = self.level_norm();
        let mut out = DomainOutputs::default();
        out.scalars.insert("height_m".to_owned(), self.height_m);
        out.scalars.insert("level_norm".to_owned(), norm);
        out.scalars
            .insert("transmitter_a".to_owned(), self.transmitter_current_a());
        out.flags
            .insert("low_switch".to_owned(), self.is_low_level());
        out.flags
            .insert("high_switch".to_owned(), self.is_high_level());
        out.flags.insert("overflow".to_owned(), self.overflowing);
        out
    }

    fn visual_state(&self) -> ComponentVisualState {
        let mut vis = ComponentVisualState::default();
        vis.scalars
            .insert("level_norm".to_owned(), self.level_norm());
        vis.scalars.insert("height_m".to_owned(), self.height_m);
        vis.flags.insert("overflow".to_owned(), self.overflowing);
        vis
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fluid_tank_converges_to_analytical_torricelli_steady_state() {
        // Steady state: Q_in = Cd*a * sqrt(2*g*h_ss) => h_ss = (Q_in / (Cd*a))^2 / (2*g)
        // Q_in = 0.008 m^3/s, Cd*a = 0.002 m^2 => v = 4 m/s => h_ss = 16 / (2 * 9.80665) = 0.815773 m
        let params = FluidTankParams {
            area_m2: 0.25,
            max_height_m: 2.0,
            initial_height_m: 0.0,
            max_inflow_m3_s: 0.008,
            eff_outflow_area_m2: 0.002,
            ..FluidTankParams::default()
        };
        let mut tank = FluidTank::new(params);
        let mut inp = DomainInputs::default();
        inp.scalars.insert("inlet_valve".to_owned(), 1.0);
        inp.scalars.insert("drain_valve".to_owned(), 1.0);

        for _ in 0..4_000 {
            tank.step_physics(100_000, &inp); // 400 s total
        }
        let expected_h = (0.008_f64 / 0.002_f64).powi(2) / (2.0 * GRAVITY_M_S2);
        assert!(
            (tank.height_m() - expected_h).abs() / expected_h < 0.002,
            "Torricelli steady-state mismatch: expected {expected_h:.5} m, got {:.5} m",
            tank.height_m()
        );
    }
}
