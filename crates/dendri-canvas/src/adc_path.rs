//! Minimal analog path for D2 (D-comp-13).
//!
//! Locked topology:
//! - Potentiometer: voltage divider on board AI (A → rail, B → GND, W → AI).
//! - Pressure TX 4–20 mA: TX OUT → one `resistor` → GND; AI across the resistor.
//!
//! Bridge drives [`dendri_sim::Simulator::set_adc`] with **0–65535** normalized
//! values. Platform counts use [`dendri_sim::norm_to_platform`].

use dendri_ast::{ComponentInstance, SimulationProject, Wire};
use dendri_sim::{volts_to_norm, Simulator, S7_ADC_MAX, UNO_ADC_MAX};

use crate::endpoint::split_endpoint;
use crate::sim_role::{ProjectSimHints, SimRole};

/// Board ADC scale (voltage full-scale + platform native max).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdcBoardScale {
    pub v_full_scale: f64,
    pub platform_max: u32,
}

impl AdcBoardScale {
    pub const UNO: Self = Self {
        v_full_scale: 5.0,
        platform_max: UNO_ADC_MAX,
    };
    pub const S7_0_10V: Self = Self {
        v_full_scale: 10.0,
        platform_max: S7_ADC_MAX,
    };

    pub fn norm_from_volts(self, volts: f64) -> u32 {
        volts_to_norm(volts, self.v_full_scale)
    }
}

impl Default for AdcBoardScale {
    fn default() -> Self {
        Self::UNO
    }
}

/// Live actuator values for `adc-source` instances (session state; D-comp-9).
#[derive(Debug, Clone, Default)]
pub struct AdcActuatorState {
    /// Potentiometer instance → wiper % (0–100).
    pub wiper_percent: std::collections::BTreeMap<String, f64>,
    /// Pressure TX instance → process pressure (engineering units).
    pub pressure: std::collections::BTreeMap<String, f64>,
}

impl AdcActuatorState {
    /// Seed from project component params (initial only; does not mutate project).
    pub fn from_project(project: &SimulationProject, hints: &ProjectSimHints) -> Self {
        let mut state = Self::default();
        for comp in &project.components {
            if hints.role_of(&comp.profile) != Some(SimRole::AdcSource) {
                continue;
            }
            if comp.profile == "potentiometer" {
                let w = param_f64(&comp.params, "wiper_position").unwrap_or(50.0);
                state
                    .wiper_percent
                    .insert(comp.instance_id.clone(), w.clamp(0.0, 100.0));
            } else if comp.profile == "sensor-pressure-4-20ma" {
                let p = param_f64(&comp.params, "pressure").unwrap_or(0.0);
                state.pressure.insert(comp.instance_id.clone(), p);
            }
        }
        state
    }
}

fn param_f64(params: &toml::map::Map<String, toml::Value>, key: &str) -> Option<f64> {
    params.get(key).and_then(|v| match v {
        toml::Value::Float(f) => Some(*f),
        toml::Value::Integer(i) => Some(*i as f64),
        toml::Value::String(s) => s.parse().ok(),
        _ => None,
    })
}

/// Drive simulator ADC inputs from session actuators + wiring topology.
pub fn sync_adc_sources_to_sim(
    sim: &mut Simulator,
    project: &SimulationProject,
    hints: &ProjectSimHints,
    actuators: &AdcActuatorState,
) {
    let scale = hints.adc_scale;
    for comp in &project.components {
        if hints.role_of(&comp.profile) != Some(SimRole::AdcSource) {
            continue;
        }
        if comp.profile == "potentiometer" {
            if let Some((pin, volts)) = pot_adc_drive(project, comp, actuators, scale, hints) {
                let _ = sim.set_adc(pin, scale.norm_from_volts(volts));
            }
        } else if comp.profile == "sensor-pressure-4-20ma" {
            if let Some((pin, volts)) = tx_shunt_adc_drive(project, comp, actuators, scale, hints)
            {
                let _ = sim.set_adc(pin, scale.norm_from_volts(volts));
            }
        }
    }
}

fn pot_adc_drive(
    project: &SimulationProject,
    pot: &ComponentInstance,
    actuators: &AdcActuatorState,
    scale: AdcBoardScale,
    hints: &ProjectSimHints,
) -> Option<(u32, f64)> {
    let wiper = format!("{}.W", pot.instance_id);
    let pin = find_board_adc_pin(&project.wiring, &wiper, hints)?;
    let pct = actuators
        .wiper_percent
        .get(&pot.instance_id)
        .copied()
        .unwrap_or(50.0)
        .clamp(0.0, 100.0);
    // Linear divider: A → rail (V_fs), B → GND, W → AI. 0% → 0 V, 100% → V_fs.
    let volts = scale.v_full_scale * (pct / 100.0);
    Some((pin, volts))
}

fn tx_shunt_adc_drive(
    project: &SimulationProject,
    tx: &ComponentInstance,
    actuators: &AdcActuatorState,
    scale: AdcBoardScale,
    hints: &ProjectSimHints,
) -> Option<(u32, f64)> {
    let out = format!("{}.OUT", tx.instance_id);
    // Shunt = resistor instance wired from TX OUT (D-comp-13 / D-comp-7).
    let (shunt_id, resistance) = find_shunt_resistor(project, &out)?;
    let shunt_a = format!("{shunt_id}.A");
    let shunt_b = format!("{shunt_id}.B");
    // AI across resistor: board AI wired to either shunt terminal (or TX OUT).
    let pin = find_board_adc_pin(&project.wiring, &out, hints)
        .or_else(|| find_board_adc_pin(&project.wiring, &shunt_a, hints))
        .or_else(|| find_board_adc_pin(&project.wiring, &shunt_b, hints))?;

    let p_min = param_f64(&tx.params, "pressure_min").unwrap_or(0.0);
    let p_max = param_f64(&tx.params, "pressure_max").unwrap_or(10.0);
    let i_min = param_f64(&tx.params, "output_current_min").unwrap_or(0.004);
    let i_max = param_f64(&tx.params, "output_current_max").unwrap_or(0.02);
    let pressure = actuators
        .pressure
        .get(&tx.instance_id)
        .copied()
        .unwrap_or_else(|| param_f64(&tx.params, "pressure").unwrap_or(0.0));
    let span = (p_max - p_min).abs().max(1e-12);
    let frac = ((pressure - p_min) / span).clamp(0.0, 1.0);
    let i_loop = i_min + (i_max - i_min) * frac;
    let volts = (i_loop * resistance).clamp(0.0, scale.v_full_scale);
    Some((pin, volts))
}

fn find_shunt_resistor(project: &SimulationProject, tx_out: &str) -> Option<(String, f64)> {
    let target = tx_out.to_ascii_lowercase();
    for w in &project.wiring {
        let from = w.from.to_ascii_lowercase();
        let to = w.to.to_ascii_lowercase();
        let other = if from == target {
            w.to.as_str()
        } else if to == target {
            w.from.as_str()
        } else {
            continue;
        };
        let (inst, _port) = split_endpoint(other)?;
        if inst.eq_ignore_ascii_case("board") {
            continue;
        }
        let comp = project
            .components
            .iter()
            .find(|c| c.instance_id.eq_ignore_ascii_case(inst))?;
        if comp.profile != "resistor" {
            continue;
        }
        let r = param_f64(&comp.params, "resistance").unwrap_or(500.0);
        return Some((comp.instance_id.clone(), r));
    }
    None
}

fn find_board_adc_pin(
    wiring: &[Wire],
    endpoint: &str,
    hints: &ProjectSimHints,
) -> Option<u32> {
    let target = endpoint.to_ascii_lowercase();
    for w in wiring {
        let from = w.from.to_ascii_lowercase();
        let to = w.to.to_ascii_lowercase();
        if from == target {
            if let Some(pin) = board_adc_pin_from_endpoint(&w.to, hints) {
                return Some(pin);
            }
        }
        if to == target {
            if let Some(pin) = board_adc_pin_from_endpoint(&w.from, hints) {
                return Some(pin);
            }
        }
    }
    None
}

fn board_adc_pin_from_endpoint(endpoint: &str, hints: &ProjectSimHints) -> Option<u32> {
    let (inst, port) = split_endpoint(endpoint)?;
    if !inst.eq_ignore_ascii_case("board") {
        return None;
    }
    hints.gpio_pin(port)
}

/// Expected platform counts for a normalized value (tests / docs).
pub fn platform_counts(norm: u32, scale: AdcBoardScale) -> u32 {
    dendri_sim::norm_to_platform(norm, scale.platform_max)
}

/// Documented mid-scale / live-zero helpers for fixtures.
pub fn pot_norm_at_percent(pct: f64, scale: AdcBoardScale) -> u32 {
    let volts = scale.v_full_scale * (pct.clamp(0.0, 100.0) / 100.0);
    scale.norm_from_volts(volts)
}

pub fn loop_norm_at_pressure(
    pressure: f64,
    p_min: f64,
    p_max: f64,
    r_shunt: f64,
    scale: AdcBoardScale,
) -> u32 {
    let span = (p_max - p_min).abs().max(1e-12);
    let frac = ((pressure - p_min) / span).clamp(0.0, 1.0);
    let i = 0.004 + 0.016 * frac;
    scale.norm_from_volts((i * r_shunt).clamp(0.0, scale.v_full_scale))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_sim::ADC_NORM_MAX;

    #[test]
    fn pot_mid_and_s7_live_zero_norms() {
        // 50% of 5 V → mid; round(0.5 * 65535) = 32768.
        assert_eq!(pot_norm_at_percent(50.0, AdcBoardScale::UNO), 32_768);
        // 4 mA × 500 Ω = 2 V on 0–10 V → 20% of full scale.
        let n = loop_norm_at_pressure(0.0, 0.0, 10.0, 500.0, AdcBoardScale::S7_0_10V);
        assert_eq!(n, 13_107);
        assert_eq!(platform_counts(n, AdcBoardScale::S7_0_10V), 5_529);
        let full = loop_norm_at_pressure(10.0, 0.0, 10.0, 500.0, AdcBoardScale::S7_0_10V);
        assert_eq!(full, ADC_NORM_MAX);
        assert_eq!(
            platform_counts(full, AdcBoardScale::S7_0_10V),
            S7_ADC_MAX
        );
    }
}
