//! ADC normalized scale and platform conversion helpers (D2 / D-comp-13).
//!
//! ASL / `SimConfig.initial_adc` / `set_adc` / `READ_ADC` use **0–65535** normalized.
//! Platform-native counts (Uno 10-bit 0–1023, S7 0–10 V ≈ 0–27648) are derived
//! for documentation and tests — see store note `sim-components-d2-pr.md`.

/// Normalized full scale (ASL `READ_ADC`, `set_adc`, `initial_adc`).
pub const ADC_NORM_MAX: u32 = 65_535;

/// Arduino Uno R3 10-bit ADC full scale.
pub const UNO_ADC_MAX: u32 = 1_023;

/// S7-1200 built-in AI typical 0–10 V raw full scale.
pub const S7_ADC_MAX: u32 = 27_648;

/// Clamp a normalized ADC value into 0..=ADC_NORM_MAX.
pub fn clamp_norm(value: u32) -> u32 {
    value.min(ADC_NORM_MAX)
}

/// Convert normalized 0–65535 → platform counts (0..=platform_max).
pub fn norm_to_platform(norm: u32, platform_max: u32) -> u32 {
    if platform_max == 0 {
        return 0;
    }
    let n = u64::from(clamp_norm(norm));
    let max = u64::from(platform_max);
    ((n * max) / u64::from(ADC_NORM_MAX)) as u32
}

/// Convert platform counts → normalized 0–65535.
pub fn platform_to_norm(counts: u32, platform_max: u32) -> u32 {
    if platform_max == 0 {
        return 0;
    }
    let c = u64::from(counts.min(platform_max));
    let max = u64::from(platform_max);
    ((c * u64::from(ADC_NORM_MAX)) / max) as u32
}

/// Volts on an AI channel → normalized 0–65535 given board full-scale volts.
pub fn volts_to_norm(volts: f64, v_full_scale: f64) -> u32 {
    if !volts.is_finite() || !v_full_scale.is_finite() || v_full_scale <= 0.0 {
        return 0;
    }
    let frac = (volts / v_full_scale).clamp(0.0, 1.0);
    clamp_norm((frac * f64::from(ADC_NORM_MAX)).round() as u32)
}

/// Normalized → volts for a board full-scale.
pub fn norm_to_volts(norm: u32, v_full_scale: f64) -> f64 {
    if !v_full_scale.is_finite() || v_full_scale <= 0.0 {
        return 0.0;
    }
    f64::from(clamp_norm(norm)) / f64::from(ADC_NORM_MAX) * v_full_scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mid_scale_uno_and_s7() {
        let mid = ADC_NORM_MAX / 2; // 32767
        assert_eq!(norm_to_platform(mid, UNO_ADC_MAX), 511);
        // 32767 * 27648 / 65535 → 13823 (integer).
        assert_eq!(norm_to_platform(mid, S7_ADC_MAX), 13_823);
        // Full scale round-trips exactly.
        assert_eq!(norm_to_platform(ADC_NORM_MAX, UNO_ADC_MAX), UNO_ADC_MAX);
        assert_eq!(norm_to_platform(ADC_NORM_MAX, S7_ADC_MAX), S7_ADC_MAX);
        assert_eq!(platform_to_norm(UNO_ADC_MAX, UNO_ADC_MAX), ADC_NORM_MAX);
        assert_eq!(platform_to_norm(S7_ADC_MAX, S7_ADC_MAX), ADC_NORM_MAX);
        // Flooring loses one count on the reverse mid path — document, don't over-assert.
        assert_eq!(platform_to_norm(511, UNO_ADC_MAX), 32_735);
        let _ = mid;
    }

    #[test]
    fn volts_full_and_live_zero_s7_shunt() {
        // 10 V → full scale; 2 V (4 mA × 500 Ω) → 20% of 0–10 V.
        assert_eq!(volts_to_norm(10.0, 10.0), ADC_NORM_MAX);
        let live_zero = volts_to_norm(2.0, 10.0);
        assert_eq!(live_zero, 13_107);
        assert_eq!(norm_to_platform(live_zero, S7_ADC_MAX), 5_529);
    }

    #[test]
    fn clamp_rejects_over_range() {
        assert_eq!(clamp_norm(70_000), ADC_NORM_MAX);
    }
}
