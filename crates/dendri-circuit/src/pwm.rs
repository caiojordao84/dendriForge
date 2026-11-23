//! Verified hardware PWM transfer functions per timer mode (`01` §3.5, `02` §3.3, `DEF-001`).
//!
//! Strictly separates:
//! 1. **Codec (`dendri-ast::Codec`)**: pure integer mapping between language-visible codes
//!    (e.g., `analogWrite` `0..=255`) and ASL `DUTY` (`0..=M`, `M = 10⁹`).
//! 2. **Transfer Function (this module)**: exact physical waveform timing & duty fraction
//!    produced by real MCU/PLC timer hardware, verified against official datasheets and core
//!    source code.

use dendri_ast::{ASL_ANALOG_FULL_SCALE, Codec, CodecKind};
use serde::{Deserialize, Serialize};

/// Verified hardware timer PWM transfer mode (`DEF-001`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PwmTransferMode {
    /// AVR ATmega328P 8-bit Fast PWM (`WGM01:0 = 3`, `TOP = 255`) driven via Arduino core
    /// `analogWrite(pin, val)`:
    /// - `val == 0`: static `LOW` (`digitalWrite(pin, LOW)` bypass in `wiring_analog.c` -> `0/256`).
    /// - `val == 255`: static `HIGH` (`digitalWrite(pin, HIGH)` bypass in `wiring_analog.c` -> `256/256`).
    /// - `1 <= val <= 254`: non-inverting Fast PWM stays HIGH for `val + 1` timer ticks out of `256`
    ///   (`duty = (val + 1) / 256`).
    AvrFastPwm8ArduinoCore,
    /// Raw AVR 8-bit Fast PWM register semantics (`COMnx1 = 1` without `wiring_analog.c` bypass):
    /// - `val == 0` emits the datasheet's 1-tick narrow spike (`1/256`).
    /// - `val == 255` stays statically HIGH (`256/256`).
    /// - `0 <= val <= 255`: `duty = (val + 1) / 256`.
    AvrFastPwm8RawRegister,
    /// AVR ATmega328P 8-bit Phase-Correct PWM (`WGM10 = 1` / `WGM20 = 1`, `TOP = 255`, dual-slope
    /// `2 * TOP = 510` ticks/period):
    /// - `0 <= val <= 255`: symmetric pulse high for `2 * val` ticks out of `510` -> `duty = val / 255`.
    AvrPhaseCorrectPwm8,
    /// Ideal linear duty transfer (`duty_fraction = duty_asl / M`, e.g., PLC PWM/PTO output).
    Linear,
}

/// Complete physical PWM channel descriptor with datasheet/core citation (`DEF-001`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PwmTransferFunction {
    /// Timer mode defining the exact physical code -> duty transfer equation.
    pub mode: PwmTransferMode,
    /// Hardware PWM period in microseconds (exact integer for AVR 16 MHz / prescaler 64:
    /// `1024 us` on Timer 0, `2040 us` on Timers 1 & 2).
    pub period_us: u64,
    /// Codec divisor `D` for converting ASL `DUTY` (`0..=M`) to native OCR code (`256` for 8-bit).
    pub codec_d: u64,
    /// Maximum native OCR code (`255` for 8-bit timers).
    pub codec_n_max: u64,
    /// Authoritative datasheet & firmware core reference (`03` §6.2).
    pub source_ref: String,
}

/// Physical square-wave parameters computed from an ASL `DUTY` value (`0..=M`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PwmWaveform {
    /// Period in microseconds (`> 0`).
    pub period_us: u64,
    /// High-time per period in microseconds (`0 <= high_us <= period_us`).
    /// `0` means static LOW; `period_us` means static HIGH.
    pub high_us: u64,
    /// Exact mathematical duty fraction (`0.0 ..= 1.0`).
    pub duty_fraction: f64,
    /// Native timer register value (e.g., `OCRnx` in `0..=255`).
    pub native_code: u64,
}

impl PwmWaveform {
    /// Returns the instantaneous logic state (`true = HIGH`, `false = LOW`) at `time_us`.
    #[must_use]
    pub const fn is_high_at(&self, time_us: u64) -> bool {
        if self.high_us == 0 {
            return false;
        }
        if self.high_us >= self.period_us {
            return true;
        }
        (time_us % self.period_us) < self.high_us
    }

    /// Returns the next discrete PWM edge timestamp strictly greater than `time_us`,
    /// or `None` when the output is static (`0%` or `100%`).
    #[must_use]
    pub const fn next_edge_after(&self, time_us: u64) -> Option<u64> {
        if self.period_us == 0 || self.high_us == 0 || self.high_us >= self.period_us {
            return None;
        }
        let phase = time_us % self.period_us;
        let cycle_start = time_us - phase;
        if phase < self.high_us {
            Some(cycle_start.saturating_add(self.high_us))
        } else {
            Some(cycle_start.saturating_add(self.period_us))
        }
    }
}

impl PwmTransferFunction {
    /// Evaluates the physical [`PwmWaveform`] directly from a native timer code `ocr` (`0..=codec_n_max`).
    #[must_use]
    pub fn evaluate_native_code(&self, ocr: u64) -> PwmWaveform {
        let period_us = self.period_us.max(1);
        let code = ocr.min(self.codec_n_max);

        let (num, den): (u64, u64) = match self.mode {
            PwmTransferMode::AvrFastPwm8ArduinoCore => {
                if code == 0 {
                    (0, 256)
                } else if code >= 255 {
                    (256, 256)
                } else {
                    (code + 1, 256)
                }
            }
            PwmTransferMode::AvrFastPwm8RawRegister => {
                if code >= 255 {
                    (256, 256)
                } else {
                    (code + 1, 256)
                }
            }
            PwmTransferMode::AvrPhaseCorrectPwm8 => (code.min(255), 255),
            PwmTransferMode::Linear => (code, self.codec_n_max.max(1)),
        };

        // Exact integer arithmetic for high_us: round((period_us * num) / den).
        // For Uno Timer 0 (1024 * num / 256 = 4 * num) and Timer 1/2 (2040 * num / 255 = 8 * num),
        // the quotient is EXACT with zero remainder!
        let high_u128 =
            (u128::from(period_us) * u128::from(num) + u128::from(den) / 2) / u128::from(den);
        let high_us = u64::try_from(high_u128).unwrap_or(period_us).min(period_us);
        let duty_fraction = (num as f64) / (den as f64);

        PwmWaveform {
            period_us,
            high_us,
            duty_fraction,
            native_code: code,
        }
    }

    /// Evaluates the physical [`PwmWaveform`] from an ASL `DUTY` quantity (`0 ..= 1_000_000_000`).
    #[must_use]
    pub fn evaluate_asl_duty(&self, duty_asl: u32) -> PwmWaveform {
        let clamped = u64::from(duty_asl).min(ASL_ANALOG_FULL_SCALE);
        match self.mode {
            PwmTransferMode::Linear => {
                let period_us = self.period_us.max(1);
                let m = u128::from(ASL_ANALOG_FULL_SCALE);
                let high_u128 = (u128::from(period_us) * u128::from(clamped) + m / 2) / m;
                let high_us = u64::try_from(high_u128).unwrap_or(period_us).min(period_us);
                PwmWaveform {
                    period_us,
                    high_us,
                    duty_fraction: (clamped as f64) / (ASL_ANALOG_FULL_SCALE as f64),
                    native_code: clamped,
                }
            }
            PwmTransferMode::AvrFastPwm8ArduinoCore
            | PwmTransferMode::AvrFastPwm8RawRegister
            | PwmTransferMode::AvrPhaseCorrectPwm8 => {
                let kind = if self.codec_d == self.codec_n_max {
                    CodecKind::Span
                } else {
                    CodecKind::Binned
                };
                let code = Codec::new("pwm", kind, self.codec_d, self.codec_n_max)
                    .map(|c| c.to_code_unipolar(clamped))
                    .unwrap_or(0);
                self.evaluate_native_code(code)
            }
        }
    }
}

/// Verified Arduino Uno R3 Timer 0 Fast PWM (`D5` = `OC0B`, `D6` = `OC0A`).
///
/// - Clock: 16 MHz, Prescaler `N = 64` (`wiring.c:init()`)
/// - Period: `64 * 256 / 16_000_000 = 1024 us` (`976.5625 Hz`, exact integer microseconds)
/// - `analogWrite` uses `span` codec `D = 255, n_max = 255` (so `DUTY = M` maps to `255` = 100%)
///   with physical transfer `0 -> 0`, `255 -> 1`, `1..=254 -> (val + 1) / 256`.
#[must_use]
pub fn uno_timer0_fast_pwm() -> PwmTransferFunction {
    PwmTransferFunction {
        mode: PwmTransferMode::AvrFastPwm8ArduinoCore,
        period_us: 1_024,
        codec_d: 255,
        codec_n_max: 255,
        source_ref: "ATmega328P Datasheet DS40002061B §14.7.3; ArduinoCore-avr cores/arduino/wiring.c & wiring_analog.c".to_owned(),
    }
}

/// Verified Arduino Uno R3 Timer 1 (`D9`, `D10`) & Timer 2 (`D3`, `D11`) Phase-Correct PWM.
///
/// - Clock: 16 MHz, Prescaler `N = 64` (`wiring.c:init()`)
/// - Period: `2 * 64 * 255 / 16_000_000 = 2040 us` (`490.196078 Hz`, exact integer microseconds)
/// - `analogWrite` uses `span` codec `D = 255, n_max = 255` with symmetric physical transfer `val / 255`.
#[must_use]
pub fn uno_timer1_2_phase_correct_pwm() -> PwmTransferFunction {
    PwmTransferFunction {
        mode: PwmTransferMode::AvrPhaseCorrectPwm8,
        period_us: 2_040,
        codec_d: 255,
        codec_n_max: 255,
        source_ref: "ATmega328P Datasheet DS40002061B §15.9.4, §17.7.4; ArduinoCore-avr cores/arduino/wiring.c & wiring_analog.c".to_owned(),
    }
}

/// Linear hardware PWM transfer function (e.g., Siemens S7-1200 PTO/PWM channel).
#[must_use]
pub fn linear_pwm(period_us: u64, source_ref: impl Into<String>) -> PwmTransferFunction {
    PwmTransferFunction {
        mode: PwmTransferMode::Linear,
        period_us: period_us.max(1),
        codec_d: ASL_ANALOG_FULL_SCALE,
        codec_n_max: ASL_ANALOG_FULL_SCALE,
        source_ref: source_ref.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uno_timer0_fast_pwm_matches_avr_datasheet_and_arduino_core_across_all_256_codes() {
        let tf = uno_timer0_fast_pwm();
        assert_eq!(tf.period_us, 1_024);

        // Special case 0: digitalWrite(LOW) bypass -> 0% static (no narrow spike)
        let w0 = tf.evaluate_native_code(0);
        assert_eq!(w0.high_us, 0);
        assert_eq!(w0.duty_fraction, 0.0);
        assert_eq!(w0.next_edge_after(0), None);

        // Special case 255: digitalWrite(HIGH) bypass -> 100% static
        let w255 = tf.evaluate_native_code(255);
        assert_eq!(w255.high_us, 1_024);
        assert_eq!(w255.duty_fraction, 1.0);
        assert_eq!(w255.next_edge_after(0), None);

        // All interior codes 1..=254: exact (ocr + 1) / 256 and exact 4 * (ocr + 1) us
        for ocr in 1..=254u64 {
            let w = tf.evaluate_native_code(ocr);
            assert_eq!(w.high_us, 4 * (ocr + 1));
            assert!((w.duty_fraction - ((ocr + 1) as f64 / 256.0)).abs() < 1e-12);
            assert!(w.is_high_at(0));
            assert!(w.is_high_at(w.high_us - 1));
            assert!(!w.is_high_at(w.high_us));
            assert_eq!(w.next_edge_after(0), Some(w.high_us));
            assert_eq!(w.next_edge_after(w.high_us), Some(1_024));
        }

        // Code 127 on Fast PWM is EXACTLY 50.0% (128/256 = 0.5, 512 us high / 512 us low)!
        let w127 = tf.evaluate_native_code(127);
        assert_eq!(w127.high_us, 512);
        assert_eq!(w127.duty_fraction, 0.5);
    }

    #[test]
    fn uno_timer0_raw_register_exhibits_datasheet_narrow_spike_at_ocr_zero() {
        let tf = PwmTransferFunction {
            mode: PwmTransferMode::AvrFastPwm8RawRegister,
            ..uno_timer0_fast_pwm()
        };
        let w0 = tf.evaluate_native_code(0);
        assert_eq!(w0.high_us, 4);
        assert!((w0.duty_fraction - (1.0 / 256.0)).abs() < 1e-12);
    }

    #[test]
    fn uno_timer1_2_phase_correct_matches_avr_datasheet_across_all_256_codes() {
        let tf = uno_timer1_2_phase_correct_pwm();
        assert_eq!(tf.period_us, 2_040);

        for ocr in 0..=255u64 {
            let w = tf.evaluate_native_code(ocr);
            assert_eq!(w.high_us, 8 * ocr);
            assert!((w.duty_fraction - (ocr as f64 / 255.0)).abs() < 1e-12);
        }

        // Round-trip from ASL codec(255, 255) preserves exact OCR for every code 0..=255!
        let codec = Codec::new("uno_pwm8", CodecKind::Span, 255, 255).unwrap();
        for ocr in 0..=255u64 {
            let asl_duty = u32::try_from(codec.to_analog_unipolar(ocr).unwrap()).unwrap();
            let w = tf.evaluate_asl_duty(asl_duty);
            assert_eq!(w.native_code, ocr);
            assert_eq!(w.high_us, 8 * ocr);
        }
    }
}
