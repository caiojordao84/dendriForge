//! Lumped first-order thermal chamber plant and IEC 60751 PT100 Callendar–Van Dusen sensor model
//! (`02-simulation-and-electrical-engine.md` §5, ADR-012).
//!
//! Differential equation:
//! $$C_{th} \frac{dT}{dt} = \frac{V_{\text{heater}}^2}{R_{\text{heater}}} - \frac{T - T_{\text{amb}}}{R_{th}}$$
//! Solved with exact per-step exponential integration for unconditional numerical stability.

use crate::{
    CircuitStampCtx, ComponentVisualState, DomainInputs, DomainOutputs, PhysicalComponent,
};
use serde::{Deserialize, Serialize};

/// Exact IEC 60751:2008 PT100 Callendar–Van Dusen resistance in Ohms (`0 °C ..= 850 °C` and `-200 °C .. 0 °C`).
#[must_use]
pub fn pt100_resistance_ohm(temp_c: f64) -> f64 {
    const R0: f64 = 100.0;
    const A: f64 = 3.908_3e-3;
    const B: f64 = -5.775e-7;
    const C: f64 = -4.183e-12;
    let t = temp_c.clamp(-200.0, 850.0);
    if t >= 0.0 {
        R0 * (1.0 + A * t + B * t * t)
    } else {
        R0 * (1.0 + A * t + B * t * t + C * (t - 100.0) * t * t * t)
    }
}

/// Physical parameters for [`ThermalChamber`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThermalChamberParams {
    /// Ambient temperature in °C (default `25.0 °C`).
    pub t_amb_c: f64,
    /// Thermal resistance to ambient in K/W (default `2.0 K/W`).
    pub r_th_k_per_w: f64,
    /// Thermal capacitance in J/K (default `10.0 J/K`, giving $\tau = R_{th} C_{th} = 20\text{ s}$).
    pub c_th_j_per_k: f64,
    /// Electrical heater resistance in Ohms (default `12.0 Ω`, `48 W` at `24 V`).
    pub r_heater_ohm: f64,
    /// Over-temperature fault limit in °C (default `150.0 °C`).
    pub t_fault_c: f64,
    /// Transmitter range low/high in °C for optional 4–20 mA current loop output.
    pub tx_min_c: f64,
    pub tx_max_c: f64,
    /// Optional SPICE `CurrentSourceDc` ID in `Circuit` updated with 4–20 mA output.
    pub transmitter_circuit_id: Option<String>,
}

impl Default for ThermalChamberParams {
    fn default() -> Self {
        Self {
            t_amb_c: 25.0,
            r_th_k_per_w: 2.0,
            c_th_j_per_k: 10.0,
            r_heater_ohm: 12.0,
            t_fault_c: 150.0,
            tx_min_c: 0.0,
            tx_max_c: 100.0,
            transmitter_circuit_id: None,
        }
    }
}

/// First-order lumped thermal chamber with PT100 and optional 4–20 mA current-loop output.
#[derive(Debug, Clone)]
pub struct ThermalChamber {
    params: ThermalChamberParams,
    temp_c: f64,
    last_power_w: f64,
}

impl ThermalChamber {
    #[must_use]
    pub const fn new(params: ThermalChamberParams) -> Self {
        let initial = params.t_amb_c;
        Self {
            params,
            temp_c: initial,
            last_power_w: 0.0,
        }
    }

    #[must_use]
    pub const fn temperature_c(&self) -> f64 {
        self.temp_c
    }

    /// 4–20 mA transmitter current in Amperes corresponding to current chamber temperature.
    #[must_use]
    pub fn transmitter_current_a(&self) -> f64 {
        let span = (self.params.tx_max_c - self.params.tx_min_c).max(1e-6);
        let frac = ((self.temp_c - self.params.tx_min_c) / span).clamp(0.0, 1.0);
        0.004 + frac * 0.016
    }
}

impl PhysicalComponent for ThermalChamber {
    fn stamp_electrical(&self, ctx: &mut CircuitStampCtx<'_>) {
        if let Some(ref tx_id) = self.params.transmitter_circuit_id {
            let _ = ctx.set_current_source(tx_id, self.transmitter_current_a());
        }
    }

    fn step_physics(&mut self, dt_us: u64, inputs: &DomainInputs) -> DomainOutputs {
        let v_heater = inputs.scalar_or("v_heater", 0.0);
        let p_ext = inputs.scalar_or("power_w", 0.0).max(0.0);
        let r_h = self.params.r_heater_ohm.max(1e-3);
        let p_in = (v_heater * v_heater) / r_h + p_ext;
        self.last_power_w = p_in;

        if dt_us > 0 {
            let dt_s = (dt_us as f64) * 1e-6;
            let r_th = self.params.r_th_k_per_w.max(1e-6);
            let c_th = self.params.c_th_j_per_k.max(1e-6);
            let tau = r_th * c_th;
            let t_inf = self.params.t_amb_c + p_in * r_th;
            let decay = libm::exp(-dt_s / tau);
            self.temp_c = t_inf + (self.temp_c - t_inf) * decay;
        }

        let mut out = DomainOutputs::default();
        out.scalars.insert("temperature_c".to_owned(), self.temp_c);
        out.scalars
            .insert("pt100_ohm".to_owned(), pt100_resistance_ohm(self.temp_c));
        out.scalars
            .insert("transmitter_a".to_owned(), self.transmitter_current_a());
        out.flags
            .insert("overtemp".to_owned(), self.temp_c >= self.params.t_fault_c);
        out
    }

    fn visual_state(&self) -> ComponentVisualState {
        let mut vis = ComponentVisualState::default();
        vis.scalars.insert("temperature_c".to_owned(), self.temp_c);
        vis.scalars.insert("power_w".to_owned(), self.last_power_w);
        vis.flags
            .insert("overtemp".to_owned(), self.temp_c >= self.params.t_fault_c);
        vis
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thermal_chamber_matches_closed_form_exponential_and_iec60751_pt100_table() {
        // IEC 60751 reference points: 0 °C -> 100.00 Ω, 100 °C -> 138.5055 Ω
        assert!((pt100_resistance_ohm(0.0) - 100.0).abs() < 1e-6);
        assert!((pt100_resistance_ohm(100.0) - 138.5055).abs() < 1e-3);

        let params = ThermalChamberParams {
            t_amb_c: 20.0,
            r_th_k_per_w: 2.0,
            c_th_j_per_k: 10.0, // tau = 20 s
            r_heater_ohm: 12.0, // 24 V -> 48 W -> dT_ss = 96 K -> T_ss = 116 C
            ..ThermalChamberParams::default()
        };
        let mut chamber = ThermalChamber::new(params);
        let mut inp = DomainInputs::default();
        inp.scalars.insert("v_heater".to_owned(), 24.0);

        // Step for exactly 1 time constant (20.0 s in 100 ms steps)
        for _ in 0..200 {
            chamber.step_physics(100_000, &inp);
        }
        let oracle_t = 20.0 + 48.0 * 2.0 * (1.0 - libm::exp(-1.0));
        assert!(
            (chamber.temperature_c() - oracle_t).abs() < 1e-6,
            "thermal oracle mismatch: expected {oracle_t:.6} C, got {:.6} C",
            chamber.temperature_c()
        );
    }
}
