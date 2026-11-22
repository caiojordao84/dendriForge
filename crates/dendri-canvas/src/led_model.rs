//! Phase C1 — static DC LED electrical model (plan D-C3).
//!
//! Explicit exception to “no analog physics”: constant \(V_f\) approx only.
//! No thermal, I²t, pin impedance, or PWM averaging.
//!
//! \[
//! I = (V_{pin} - V_f) / R_{series}
//! \]
//! \(I = 0\) when \(V_{pin} \le V_f\). Burned when \(I > I_{max}\) (persists until Reset);
//! the burn scan is the last with non-zero \(I\); later scans report \(I = 0\).

use dendri_ast::ComponentInstance;
use toml::Value;

/// Electrical parameters resolved from LED profile defaults + instance overrides.
///
/// Defaults mirror `content/profiles/components/led.toml` when a key is absent.
/// Draw / Path code must not invent its own \(V_f\), \(R\), or \(I_{max}\).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LedElectricalParams {
    /// Forward voltage \(V_f\) (volts).
    pub vf: f64,
    /// Series resistance \(R_{series}\) (ohms) — profile `integrated_resistance`.
    pub r_series: f64,
    /// Absolute max forward current \(I_{max}\) (amperes) — profile `if_max`.
    pub i_max: f64,
    /// Logic-high pin voltage (volts), from board (Uno = 5 V).
    pub v_high: f64,
}

impl LedElectricalParams {
    /// Profile defaults from `led.toml` (`vf`, `integrated_resistance`, `if_max`).
    pub const PROFILE_DEFAULTS: Self = Self {
        vf: 2.1,
        r_series: 0.0,
        i_max: 0.03,
        v_high: 5.0,
    };

    /// Resolve instance param overrides on top of profile defaults.
    pub fn resolve(instance: &ComponentInstance, base: Self) -> Self {
        let mut p = base;
        if let Some(v) = param_f64(&instance.params, "vf") {
            p.vf = v;
        }
        if let Some(v) = param_f64(&instance.params, "integrated_resistance") {
            p.r_series = v;
        }
        if let Some(v) = param_f64(&instance.params, "if_max") {
            p.i_max = v;
        }
        p
    }

    /// Pin voltage for a digital level (high → `v_high`, low → 0).
    pub fn v_pin(&self, high: bool) -> f64 {
        if high {
            self.v_high
        } else {
            0.0
        }
    }
}

/// Result of one LED evaluation for a scan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LedElectricalState {
    /// Forward current in amperes (0 after burn or when off).
    pub current: f64,
    /// Burned this scan or already burned (persists until Reset).
    pub burned: bool,
}

/// Evaluate LED current / burn for one scan (D-C3 order).
///
/// 1. If already burned → \(I = 0\), burned stays true (open circuit).
/// 2. Else compute \(I = \max(0, (V_{pin}-V_f)/R)\).
/// 3. If \(I > I_{max}\) → burned; this scan still reports the non-zero \(I\)
///    that caused the burn (last non-zero scan); presentation shows burned.
pub fn evaluate_led(
    params: &LedElectricalParams,
    v_pin: f64,
    already_burned: bool,
) -> LedElectricalState {
    if already_burned {
        return LedElectricalState {
            current: 0.0,
            burned: true,
        };
    }
    let current = if params.r_series <= 0.0 {
        // No series resistance → treat as unbounded current when forward biased.
        if v_pin > params.vf {
            f64::INFINITY
        } else {
            0.0
        }
    } else if v_pin <= params.vf {
        0.0
    } else {
        (v_pin - params.vf) / params.r_series
    };
    let burned = current > params.i_max;
    LedElectricalState { current, burned }
}

/// Convert amperes to microamperes for Eq-friendly snapshot fields.
pub fn amperes_to_ua(current: f64) -> i64 {
    if !current.is_finite() {
        // Sentinel for “unbounded / immediate burn” (R≈0).
        return i64::MAX;
    }
    (current * 1_000_000.0).round() as i64
}

pub fn ua_to_amperes(ua: i64) -> f64 {
    if ua == i64::MAX {
        return f64::INFINITY;
    }
    ua as f64 / 1_000_000.0
}

fn param_f64(params: &toml::map::Map<String, Value>, key: &str) -> Option<f64> {
    match params.get(key)? {
        Value::Float(f) => Some(*f),
        Value::Integer(i) => Some(*i as f64),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Read profile-table defaults from a loaded profile `rest["params"]` map when present.
pub fn defaults_from_profile_params(
    params_table: &toml::map::Map<String, Value>,
    v_high: f64,
) -> LedElectricalParams {
    let mut p = LedElectricalParams::PROFILE_DEFAULTS;
    p.v_high = v_high;
    p.vf = nested_default_f64(params_table, "vf").unwrap_or(p.vf);
    p.r_series = nested_default_f64(params_table, "integrated_resistance").unwrap_or(p.r_series);
    p.i_max = nested_default_f64(params_table, "if_max").unwrap_or(p.i_max);
    p
}

fn nested_default_f64(params: &toml::map::Map<String, Value>, key: &str) -> Option<f64> {
    let table = params.get(key)?.as_table()?;
    match table.get("default")? {
        Value::Float(f) => Some(*f),
        Value::Integer(i) => Some(*i as f64),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_ast::ComponentInstance;

    fn led_inst(r: i64, vf: f64) -> ComponentInstance {
        let mut params = toml::map::Map::new();
        params.insert("integrated_resistance".into(), Value::Integer(r));
        params.insert("vf".into(), Value::Float(vf));
        ComponentInstance {
            instance_id: "led1".into(),
            profile: "led".into(),
            label: None,
            params,
            layout: Default::default(),
        }
    }

    #[test]
    fn c01_r220_vf2_at_5v_is_about_13_6_ma_not_burned() {
        // Plan C01: R=220Ω, V_pin=5V, V_f=2.0V → I≈13.6mA, not burned.
        let params = LedElectricalParams::resolve(
            &led_inst(220, 2.0),
            LedElectricalParams::PROFILE_DEFAULTS,
        );
        assert!((params.vf - 2.0).abs() < 1e-12);
        assert!((params.r_series - 220.0).abs() < 1e-12);
        let st = evaluate_led(&params, 5.0, false);
        assert!((st.current - 0.013636363636).abs() < 1e-9, "I={}", st.current);
        assert!(!st.burned);
        assert!(st.current <= params.i_max);
    }

    #[test]
    fn low_r_burns_and_later_scans_open_circuit() {
        let params = LedElectricalParams {
            vf: 2.0,
            r_series: 10.0, // I = 0.3 A > 0.03
            i_max: 0.03,
            v_high: 5.0,
        };
        let burn_scan = evaluate_led(&params, 5.0, false);
        assert!(burn_scan.burned);
        assert!(burn_scan.current > params.i_max);
        assert!(burn_scan.current > 0.0);

        let after = evaluate_led(&params, 5.0, true);
        assert!(after.burned);
        assert_eq!(after.current, 0.0);

        // Toggling pin does not revive.
        let off = evaluate_led(&params, 0.0, true);
        assert!(off.burned);
        assert_eq!(off.current, 0.0);
        let on_again = evaluate_led(&params, 5.0, true);
        assert!(on_again.burned);
        assert_eq!(on_again.current, 0.0);
    }

    #[test]
    fn below_vf_is_zero_current() {
        let params = LedElectricalParams::PROFILE_DEFAULTS;
        let st = evaluate_led(&params, 1.0, false);
        assert_eq!(st.current, 0.0);
        assert!(!st.burned);
    }

    #[test]
    fn integrated_resistance_zero_no_nan_or_panic() {
        // Mandatory D-comp-5 / D0a: R=0 with forward bias → unbounded I, burn; no NaN.
        let params = LedElectricalParams {
            vf: 2.0,
            r_series: 0.0,
            i_max: 0.03,
            v_high: 24.0,
        };
        let st = evaluate_led(&params, 24.0, false);
        assert!(st.current.is_infinite(), "I={}", st.current);
        assert!(!st.current.is_nan());
        assert!(st.burned);
        let ua = amperes_to_ua(st.current);
        assert_eq!(ua, i64::MAX);
        let after = evaluate_led(&params, 24.0, true);
        assert_eq!(after.current, 0.0);
        assert!(!after.current.is_nan());
        assert!(after.burned);
    }

    #[test]
    fn pilot_defaults_at_24v_about_20_ma_not_burned() {
        // signal-pilot-lamp.toml: vf=2.0, R=1100, if_max=0.03 @ 24 V → I≈20 mA.
        let params = LedElectricalParams {
            vf: 2.0,
            r_series: 1100.0,
            i_max: 0.03,
            v_high: 24.0,
        };
        let st = evaluate_led(&params, params.v_pin(true), false);
        assert!((st.current - 0.02).abs() < 1e-9, "I={}", st.current);
        assert!(!st.burned);
        assert!(st.current <= params.i_max);
    }
}
