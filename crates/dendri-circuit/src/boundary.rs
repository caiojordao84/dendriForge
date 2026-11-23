//! MCU / PLC pin electrical boundary coupling and exact Solver -> ASL numeric boundary
//! (`02-simulation-and-electrical-engine.md` §3.3, ADR-025, ADR-029).

use crate::pwm::{
    PwmTransferFunction, linear_pwm, uno_timer0_fast_pwm, uno_timer1_2_phase_correct_pwm,
};
use dendri_ast::{ASL_ANALOG_FULL_SCALE, AnalogStatus, ChannelRange, PullMode, SignalKind};
use dendri_vm::{AnalogReading, ElectricalDigitalLevel};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Exact IEEE-754 **round-half-even** (banker's rounding) implemented purely in software
/// using `libm::floor` so it is 100% deterministic across platforms and independent of
/// host FPU rounding-mode registers (`02` §3.3, D6).
#[must_use]
pub fn round_half_even_f64(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let f = libm::floor(x);
    let frac = x - f;
    if frac < 0.5 {
        f
    } else if frac > 0.5 {
        f + 1.0
    } else {
        // Exact halfway tie (`frac == 0.5`): round to whichever neighbor (`f` or `f + 1`) is even.
        if libm::fmod(f, 2.0) == 0.0 {
            f
        } else {
            f + 1.0
        }
    }
}

/// Converts a solved MNA node voltage `v` into an ASL `ANALOG` fixed-point quantity (`M = 10⁹`)
/// using unbiased **round-half-even** (`02` §3.3, `D6`):
///
/// $$a = \operatorname{clamp}_{i32}\!\left(\operatorname{round\_half\_even}\!\left(\frac{v - v_{\text{low}}}{v_{\text{high}} - v_{\text{low}}} \cdot M\right)\right)$$
///
/// - Non-finite `v` (`NaN`, `±inf`) or degenerate span (`v_high <= v_low`) returns `AnalogStatus::Fault`.
/// - When `live_zero == true && hardware_normalizes_live_zero == true` (e.g., industrial 4–20 mA module),
///   `v_low` maps to `0` and `v_high` maps to `M` (`1_000_000_000`).
/// - When `live_zero == false` or `hardware_normalizes_live_zero == false` (e.g., 0–10 V input
///   measuring a 4–20 mA loop across a 500 Ω shunt where 4 mA = 2 V = 20%), `0 V -> 0` and
///   `10 V -> M`, so `2.0 V -> 200_000_000` (`T-NUM-5` -> industrial 27648-count code `5529`!).
#[must_use]
pub fn voltage_to_analog_round_half_even(v: f64, range: &ChannelRange) -> AnalogReading {
    if !v.is_finite() || !range.low.is_finite() || !range.high.is_finite() {
        return AnalogReading {
            analog: 0,
            status: AnalogStatus::Fault,
        };
    }

    let (lo, hi) = if range.live_zero && !range.hardware_normalizes_live_zero {
        (0.0, range.high)
    } else {
        (range.low, range.high)
    };

    let span = hi - lo;
    if !span.is_finite() || span <= 0.0 {
        return AnalogReading {
            analog: 0,
            status: AnalogStatus::Fault,
        };
    }

    let m = ASL_ANALOG_FULL_SCALE as f64;
    let normalized = (v - lo) / span;
    let scaled = normalized * m;
    if !scaled.is_finite() {
        return AnalogReading {
            analog: 0,
            status: AnalogStatus::Fault,
        };
    }

    let rounded = round_half_even_f64(scaled);
    let clamped = rounded.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;

    let status = if range.low < 0.0 {
        // Bipolar channel: nominal [-M, +M]
        let m_i32 = i32::try_from(ASL_ANALOG_FULL_SCALE).unwrap_or(i32::MAX);
        if clamped < -m_i32 {
            AnalogStatus::Underrange
        } else if clamped > m_i32 {
            AnalogStatus::Overrange
        } else {
            AnalogStatus::Ok
        }
    } else {
        let m_i32 = i32::try_from(ASL_ANALOG_FULL_SCALE).unwrap_or(i32::MAX);
        if clamped < 0 {
            AnalogStatus::Underrange
        } else if clamped > m_i32 {
            AnalogStatus::Overrange
        } else {
            AnalogStatus::Ok
        }
    };

    AnalogReading {
        analog: clamped,
        status,
    }
}

/// Electrical parameters for a single board family (`02` §3.3, `03` §6.2).
/// Never hardcoded inside the solver — supplied per board instance!
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardElectricalProfile {
    pub profile_id: String,
    /// Logic HIGH output voltage ($V_{OH}$, e.g. `5.0 V` Uno, `24.0 V` PLC).
    pub logic_high_v: f64,
    /// Logic LOW output voltage ($V_{OL}$, typically `0.0 V`).
    pub logic_low_v: f64,
    /// Schmitt digital input HIGH threshold ($V_{IH}$).
    pub v_ih_v: f64,
    /// Schmitt digital input LOW threshold ($V_{IL}$).
    pub v_il_v: f64,
    /// Digital output Thevenin series resistance ($R_{out}$ in Ω).
    pub pin_r_out_ohm: f64,
    /// Digital/analog input impedance ($R_{in}$ in Ω, typically `10 MΩ`).
    pub pin_r_in_ohm: f64,
    /// Maximum allowed continuous pin sink/source current ($I_{max}$ in A).
    pub pin_i_max_a: f64,
    /// Absolute maximum pin voltage rating ($V_{abs\_max}$ in V); exceeding raises `PinOvervoltage`.
    pub v_abs_max_v: f64,
    /// Absolute minimum pin voltage rating ($V_{abs\_min}$ in V, typically `-0.5 V`).
    pub v_abs_min_v: f64,
    /// Internal pull-up resistance ($R_{pu}$ in Ω).
    pub pullup_ohm: f64,
    /// Internal pull-down resistance ($R_{pd}$ in Ω).
    pub pulldown_ohm: f64,
    /// Default ADC full-scale reference voltage (e.g. `5.0 V` Uno, `10.0 V` PLC).
    pub default_adc_range: ChannelRange,
    /// Verified per-pin hardware PWM transfer functions (`DEF-001`).
    pub pwm_channels: BTreeMap<String, PwmTransferFunction>,
    /// Datasheet reference string.
    pub source_ref: String,
}

impl BoardElectricalProfile {
    /// Classifies a solved pin voltage against Schmitt thresholds ($V_{IL}, V_{IH}$) (`02` §3.3).
    #[must_use]
    pub fn classify_digital_voltage(&self, v: f64) -> ElectricalDigitalLevel {
        if !v.is_finite() {
            return ElectricalDigitalLevel::Indeterminate;
        }
        if v >= self.v_ih_v {
            ElectricalDigitalLevel::High
        } else if v <= self.v_il_v {
            ElectricalDigitalLevel::Low
        } else {
            ElectricalDigitalLevel::Indeterminate
        }
    }
}

/// Per-pin active configuration for a board attached to [`crate::SpiceCircuitBackend`].
#[derive(Debug, Clone, PartialEq)]
pub struct BoardPinSpec {
    pub pin: String,
    pub is_output: bool,
    pub is_analog: bool,
    pub is_pwm: bool,
    pub pull: PullMode,
    pub analog_range: Option<ChannelRange>,
}

/// Reference electrical profile for `arduino-uno-r3` (ATmega328P @ 5 V, 16 MHz).
///
/// Datasheet refs: Microchip ATmega328P DS40002061B §29.1–§29.2 (DC Characteristics:
/// `V_CC = 5 V`, `V_IH = 0.6 * V_CC = 3.0 V`, `V_IL = 0.3 * V_CC = 1.5 V`,
/// `I_max = 40 mA` abs max per I/O pin, `V_abs_max = 5.5 V`, `R_pu = 20–50 kΩ` typ `35 kΩ`,
/// `R_out ≈ 25 Ω` typ at 5 V).
#[must_use]
pub fn arduino_uno_r3_electrical() -> BoardElectricalProfile {
    let mut pwm_channels = BTreeMap::new();
    let t0 = uno_timer0_fast_pwm();
    let t12 = uno_timer1_2_phase_correct_pwm();
    pwm_channels.insert("D5".to_owned(), t0.clone());
    pwm_channels.insert("D6".to_owned(), t0);
    pwm_channels.insert("D3".to_owned(), t12.clone());
    pwm_channels.insert("D9".to_owned(), t12.clone());
    pwm_channels.insert("D10".to_owned(), t12.clone());
    pwm_channels.insert("D11".to_owned(), t12);

    BoardElectricalProfile {
        profile_id: "arduino-uno-r3".to_owned(),
        logic_high_v: 5.0,
        logic_low_v: 0.0,
        v_ih_v: 3.0,
        v_il_v: 1.5,
        pin_r_out_ohm: 25.0,
        pin_r_in_ohm: 10_000_000.0,
        pin_i_max_a: 0.040,
        v_abs_max_v: 5.5,
        v_abs_min_v: -0.5,
        pullup_ohm: 35_000.0,
        pulldown_ohm: 35_000.0,
        default_adc_range: ChannelRange {
            signal: SignalKind::Voltage,
            low: 0.0,
            high: 5.0,
            live_zero: false,
            hardware_normalizes_live_zero: false,
        },
        pwm_channels,
        source_ref:
            "Microchip ATmega328P Datasheet DS40002061B §29.1-§29.2, §14.7.3, §15.9.4, §17.7.4"
                .to_owned(),
    }
}

/// Reference electrical profile for `plc-1200-open-1214` (24 V DC / IEC 61131-2 Type 1 DI, 0–10 V AI).
///
/// Engineering spec: IEC 61131-2 Type 1 24 V DC PLC (`V_OH = 24 V`, DI `V_IH >= 15.0 V`,
/// `V_IL <= 5.0 V`, DI sink `4 mA @ 24 V` / `R_in ≈ 6 kΩ`, DO `0.5 A` max, `V_abs_max = 30.0 V`,
/// onboard AI `0..10 V` / `100 kΩ`, 27,648-count industrial analog representation).
#[must_use]
pub fn plc_1200_open_1214_electrical() -> BoardElectricalProfile {
    let mut pwm_channels = BTreeMap::new();
    let pto = linear_pwm(
        1_000,
        "IEC 61131-2 / Industrial PLC 24V DC linear PTO/PWM output (1 kHz nominal)",
    );
    pwm_channels.insert("Qa.0".to_owned(), pto.clone());
    pwm_channels.insert("Qa.1".to_owned(), pto);

    BoardElectricalProfile {
        profile_id: "plc-1200-open-1214".to_owned(),
        logic_high_v: 24.0,
        logic_low_v: 0.0,
        v_ih_v: 15.0,
        v_il_v: 5.0,
        pin_r_out_ohm: 1.0,
        pin_r_in_ohm: 100_000.0,
        pin_i_max_a: 0.500,
        v_abs_max_v: 30.0,
        v_abs_min_v: -0.5,
        pullup_ohm: 10_000.0,
        pulldown_ohm: 6_000.0,
        default_adc_range: ChannelRange {
            signal: SignalKind::Voltage,
            low: 0.0,
            high: 10.0,
            live_zero: false,
            hardware_normalizes_live_zero: false,
        },
        pwm_channels,
        source_ref: "IEC 61131-2 Type 1 (24V DC) & Industrial 27648-count PLC Analog Specification"
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_ast::{Codec, CodecKind};

    #[test]
    fn round_half_even_matches_ieee754_bankers_rounding() {
        assert_eq!(round_half_even_f64(0.5), 0.0);
        assert_eq!(round_half_even_f64(1.5), 2.0);
        assert_eq!(round_half_even_f64(2.5), 2.0);
        assert_eq!(round_half_even_f64(3.5), 4.0);
        assert_eq!(round_half_even_f64(-0.5), 0.0);
        assert_eq!(round_half_even_f64(-1.5), -2.0);
        assert_eq!(round_half_even_f64(-2.5), -2.0);
        assert_eq!(round_half_even_f64(2.4999), 2.0);
        assert_eq!(round_half_even_f64(2.5001), 3.0);
    }

    #[test]
    fn solver_to_asl_boundary_satisfies_t_num_5_and_fault_on_non_finite() {
        let range = ChannelRange {
            signal: SignalKind::Voltage,
            low: 0.0,
            high: 10.0,
            live_zero: false,
            hardware_normalizes_live_zero: false,
        };
        let reading = voltage_to_analog_round_half_even(2.0, &range);
        assert_eq!(reading.status, AnalogStatus::Ok);
        assert_eq!(reading.analog, 200_000_000);

        let plc_codec = Codec::new("plc_span_27648", CodecKind::Span, 27_648, 27_648).unwrap();
        let code = plc_codec.to_code_unipolar(u64::try_from(reading.analog).unwrap());
        assert_eq!(code, 5_529);

        let nan_reading = voltage_to_analog_round_half_even(f64::NAN, &range);
        assert_eq!(nan_reading.status, AnalogStatus::Fault);
        let inf_reading = voltage_to_analog_round_half_even(f64::INFINITY, &range);
        assert_eq!(inf_reading.status, AnalogStatus::Fault);
    }
}
