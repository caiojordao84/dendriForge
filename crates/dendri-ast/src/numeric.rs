//! ASL v2 Fixed-Point Numeric Contract (`01-asl-ir-and-project-schema.md` §3, ADR-025).
//!
//! - Frozen scale: `ASL_ANALOG_FULL_SCALE = 1_000_000_000` (10⁹).
//! - All intermediate products execute in 128-bit integer arithmetic (`u128` / `i128`).
//! - Implements Lemmas 1–2, mandatory `span -> binned` clamping, `span_bipolar`,
//!   signed band layer validation (`ERR_BAND_LIMIT_OVERFLOW`), and `T-NUM-1..5`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Frozen ASL analog full-scale constant `M = 10⁹` (parts per billion of channel range).
pub const ASL_ANALOG_FULL_SCALE: u64 = 1_000_000_000;

/// Maximum valid signed `i32` analog quantity magnitude bound (`i32::MIN ..= i32::MAX`).
pub const ANALOG_I32_MIN: i128 = i32::MIN as i128;
pub const ANALOG_I32_MAX: i128 = i32::MAX as i128;

/// Codec category describing how native integer codes relate to normalized range fractions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodecKind {
    /// Real ADC: `code = floor(fraction * 2^b)`, `D = 2^b`, `n_max = D - 1`.
    Binned,
    /// Endpoint-inclusive unipolar scale (e.g., Siemens S7 `0..27648`, `D = n_max`).
    Span,
    /// Symmetric endpoint-inclusive bipolar scale (e.g., Siemens S7 `-27648..+27648`, `D = n_max`).
    SpanBipolar,
}

/// Errors raised when constructing or validating an analog [`Codec`] or [`ChannelBand`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CodecError {
    #[error("codec divisions D={d} exceeds ASL_ANALOG_FULL_SCALE ({ASL_ANALOG_FULL_SCALE})")]
    ResolutionExceedsScale { d: u64 },
    #[error("invalid codec range: kind={kind:?}, D={d}, n_max={n_max}")]
    InvalidRange { kind: CodecKind, d: u64, n_max: u64 },
    #[error("channel band code {code} maps to ANALOG={mapped} which overflows signed i32")]
    BandLimitOverflow { code: i64, mapped: i128 },
    #[error("invalid channel band ordering: underrange_min ({min}) > overrange_max ({max})")]
    InvalidBandOrder { min: i64, max: i64 },
}

/// Pure integer codec mapping between native converter codes and ASL `ANALOG` (`M = 10⁹`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Codec {
    pub id: String,
    pub kind: CodecKind,
    pub d: u64,
    pub n_max: u64,
    #[serde(default)]
    pub source_ref: Option<String>,
}

impl Codec {
    /// Constructs and validates a codec against `01-asl-ir-and-project-schema.md` §3.1–3.2.
    pub fn new(
        id: impl Into<String>,
        kind: CodecKind,
        d: u64,
        n_max: u64,
    ) -> Result<Self, CodecError> {
        if d > ASL_ANALOG_FULL_SCALE {
            return Err(CodecError::ResolutionExceedsScale { d });
        }
        let valid_range = match kind {
            CodecKind::Binned => d >= 2 && n_max == d.saturating_sub(1),
            CodecKind::Span | CodecKind::SpanBipolar => d >= 1 && n_max == d,
        };
        if !valid_range {
            return Err(CodecError::InvalidRange { kind, d, n_max });
        }
        Ok(Self {
            id: id.into(),
            kind,
            d,
            n_max,
            source_ref: None,
        })
    }

    /// Validates an already-deserialized [`Codec`].
    pub fn validate(&self) -> Result<(), CodecError> {
        Self::new(self.id.clone(), self.kind, self.d, self.n_max).map(|_| ())
    }

    /// Unipolar `native -> ASL`: `ceil(n * M / D)`. Returns `None` outside `[0, n_max]`.
    #[must_use]
    pub fn to_analog_unipolar(&self, n: u64) -> Option<u64> {
        if n > self.n_max || self.d == 0 {
            return None;
        }
        let num = u128::from(n) * u128::from(ASL_ANALOG_FULL_SCALE);
        let d = u128::from(self.d);
        let res = num.saturating_add(d.saturating_sub(1)) / d;
        u64::try_from(res).ok()
    }

    /// Unipolar `ASL -> native`: `min(n_max, floor(a * D / M))`; inputs above `M` clamp to `M`.
    #[must_use]
    pub fn to_code_unipolar(&self, a: u64) -> u64 {
        let clamped = a.min(ASL_ANALOG_FULL_SCALE);
        let n = (u128::from(clamped) * u128::from(self.d)) / u128::from(ASL_ANALOG_FULL_SCALE);
        u64::try_from(n).unwrap_or(self.n_max).min(self.n_max)
    }

    /// Bipolar (`span_bipolar`) `native -> ASL`, magnitude-symmetric: `sign(n) * ceil(|n| * M / D)`.
    #[must_use]
    pub fn to_analog_signed(&self, n: i64) -> Option<i64> {
        let mag = self.to_analog_unipolar(n.unsigned_abs())?;
        let signed = i64::try_from(mag).ok()?;
        Some(if n < 0 { -signed } else { signed })
    }

    /// Bipolar `ASL -> native`, truncation toward zero: `sign(a) * min(n_max, floor(|a| * D / M))`.
    #[must_use]
    pub fn to_code_signed(&self, a: i64) -> i64 {
        let mag = self.to_code_unipolar(a.unsigned_abs());
        let signed = i64::try_from(mag).unwrap_or(i64::MAX);
        if a < 0 { -signed } else { signed }
    }

    /// Extended signed band-layer conversion `native -> ANALOG (i128)` without nominal clamping,
    /// used for underrange/overrange checks (`01` §3.4).
    #[must_use]
    pub fn band_code_to_analog_i128(&self, code: i64) -> i128 {
        if self.d == 0 {
            return 0;
        }
        let d = u128::from(self.d);
        let num = u128::from(code.unsigned_abs()) * u128::from(ASL_ANALOG_FULL_SCALE);
        let mag = i128::try_from(num.saturating_add(d.saturating_sub(1)) / d).unwrap_or(i128::MAX);
        if code < 0 { -mag } else { mag }
    }

    /// Extended signed band-layer conversion `ANALOG (i32) -> native (i64)`, truncated toward zero.
    #[must_use]
    pub fn band_analog_to_code_i64(&self, analog: i32) -> i64 {
        let mag = (u128::from(analog.unsigned_abs()) * u128::from(self.d))
            / u128::from(ASL_ANALOG_FULL_SCALE);
        let signed = i64::try_from(mag).unwrap_or(i64::MAX);
        if analog < 0 { -signed } else { signed }
    }
}

/// Computes the exact clamped platform-ratio composition of Lemma 2 (`01` §3.3):
/// `min(n_max2, floor(n * D2 / D1))` in 128-bit integer arithmetic.
#[must_use]
pub fn composed_clamped_rational(n: u64, d1: u64, d2: u64, n_max2: u64) -> u64 {
    if d1 == 0 {
        return 0;
    }
    let q = (u128::from(n) * u128::from(d2)) / u128::from(d1);
    u64::try_from(q).unwrap_or(n_max2).min(n_max2)
}

/// Euclidean greatest common divisor.
#[must_use]
pub const fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

/// Least common multiple in `u128` (Lemma 2 condition: `lcm(D1, D2) <= M`).
#[must_use]
pub fn lcm_u128(a: u64, b: u64) -> u128 {
    if a == 0 || b == 0 {
        return 0;
    }
    let g = gcd_u64(a, b);
    u128::from(a / g) * u128::from(b)
}

/// Electrical domain of an analog channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    Voltage,
    Current,
}

/// Per-channel physical range descriptor (`01` §3.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelRange {
    pub signal: SignalKind,
    pub low: f64,
    pub high: f64,
    #[serde(default)]
    pub live_zero: bool,
    #[serde(default)]
    pub hardware_normalizes_live_zero: bool,
}

/// Extended signed band limits for underrange/overrange and fault diagnostics (`01` §3.4, `DEF-002`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelBand {
    pub underrange_min: i64,
    pub overrange_max: i64,
    pub overflow_code: Option<i64>,
    pub wire_break_code: Option<i64>,
    pub source_ref: String,
}

impl ChannelBand {
    /// Validates that both `underrange_min` and `overrange_max` map within signed `i32` `ANALOG`
    /// (`ERR_BAND_LIMIT_OVERFLOW`).
    pub fn validate_against(&self, codec: &Codec) -> Result<(), CodecError> {
        if self.underrange_min > self.overrange_max {
            return Err(CodecError::InvalidBandOrder {
                min: self.underrange_min,
                max: self.overrange_max,
            });
        }
        for code in [self.underrange_min, self.overrange_max] {
            let mapped = codec.band_code_to_analog_i128(code);
            if !(ANALOG_I32_MIN..=ANALOG_I32_MAX).contains(&mapped) {
                return Err(CodecError::BandLimitOverflow { code, mapped });
            }
        }
        Ok(())
    }
}

/// Status channel accompanying every analog read (`01` §3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalogStatus {
    Ok,
    Underrange,
    Overrange,
    Fault,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog_codecs() -> Vec<Codec> {
        vec![
            Codec::new("pwm_8bit", CodecKind::Binned, 256, 255).unwrap(),
            Codec::new("uno_10bit", CodecKind::Binned, 1024, 1023).unwrap(),
            Codec::new("adc_12bit", CodecKind::Binned, 4096, 4095).unwrap(),
            Codec::new("s7_span", CodecKind::Span, 27_648, 27_648).unwrap(),
            Codec::new("s7_bipolar", CodecKind::SpanBipolar, 27_648, 27_648).unwrap(),
            Codec::new("adc_16bit", CodecKind::Binned, 65_536, 65_535).unwrap(),
            Codec::new("adc_19bit", CodecKind::Binned, 1 << 19, (1 << 19) - 1).unwrap(),
            Codec::new("adc_20bit", CodecKind::Binned, 1 << 20, (1 << 20) - 1).unwrap(),
        ]
    }

    #[test]
    fn t_num_1_exhaustive_round_trip_catalog() {
        for codec in catalog_codecs() {
            match codec.kind {
                CodecKind::Binned | CodecKind::Span => {
                    for n in 0..=codec.n_max {
                        let a = codec.to_analog_unipolar(n).unwrap();
                        let rt = codec.to_code_unipolar(a);
                        assert_eq!(rt, n, "round-trip failed for {} at n={n}", codec.id);
                    }
                }
                CodecKind::SpanBipolar => {
                    let max_i = codec.n_max as i64;
                    for n in -max_i..=max_i {
                        let a = codec.to_analog_signed(n).unwrap();
                        let rt = codec.to_code_signed(a);
                        assert_eq!(rt, n, "bipolar round-trip failed for {} at n={n}", codec.id);
                    }
                }
            }
        }
    }

    /// Exhaustive 24-bit round-trip (`16_777_216` codes) — runs fast in release CI (`T-NUM-1`)
    /// and verifies boundaries + stride in debug builds.
    #[test]
    fn t_num_1_24bit_exhaustive_in_release_or_boundaries_in_debug() {
        let c24 = Codec::new("adc_24bit", CodecKind::Binned, 1 << 24, (1 << 24) - 1).unwrap();
        let step: u64 = if cfg!(debug_assertions) { 127 } else { 1 };
        let mut n = 0u64;
        while n <= c24.n_max {
            let a = c24.to_analog_unipolar(n).unwrap();
            assert_eq!(
                c24.to_code_unipolar(a),
                n,
                "24-bit round-trip failed at {n}"
            );
            n = n.saturating_add(step);
        }
        // Always check exact top & near-top endpoints in every build mode
        for edge in [0, 1, 2, c24.n_max - 2, c24.n_max - 1, c24.n_max] {
            let a = c24.to_analog_unipolar(edge).unwrap();
            assert_eq!(c24.to_code_unipolar(a), edge);
        }
    }

    #[test]
    fn t_num_2_and_t_num_3_composition_monotonicity_endpoints_and_clamp() {
        let unipolar: Vec<Codec> = catalog_codecs()
            .into_iter()
            .filter(|c| c.kind != CodecKind::SpanBipolar && c.d <= 65_536)
            .collect();

        for src in &unipolar {
            // T-NUM-3 endpoints & monotonicity
            assert_eq!(src.to_analog_unipolar(0), Some(0));
            assert_eq!(src.to_code_unipolar(0), 0);
            assert_eq!(src.to_code_unipolar(ASL_ANALOG_FULL_SCALE), src.n_max);
            assert_eq!(src.to_analog_unipolar(src.n_max + 1), None);

            let mut prev_a = 0u64;
            for n in 0..=src.n_max {
                let a = src.to_analog_unipolar(n).unwrap();
                assert!(a >= prev_a, "non-monotonic native->ASL for {}", src.id);
                prev_a = a;
            }

            // T-NUM-2 ordered pair composition vs clamped rational
            for dst in &unipolar {
                let lcm = lcm_u128(src.d, dst.d);
                assert!(
                    lcm <= u128::from(ASL_ANALOG_FULL_SCALE),
                    "expected catalog pair ({}, {}) to satisfy lcm <= M",
                    src.id,
                    dst.id
                );
                for n in 0..=src.n_max {
                    let via_asl = dst.to_code_unipolar(src.to_analog_unipolar(n).unwrap());
                    let rational = composed_clamped_rational(n, src.d, dst.d, dst.n_max);
                    assert_eq!(
                        via_asl, rational,
                        "composition mismatch {} -> {} at n={n}",
                        src.id, dst.id
                    );
                }

                // T-NUM-3 mandatory clamp verification for span -> binned at top code n = D1
                if src.kind == CodecKind::Span && dst.kind == CodecKind::Binned {
                    let unclamped = (u128::from(src.n_max) * u128::from(dst.d)) / u128::from(src.d);
                    assert_eq!(unclamped, u128::from(dst.n_max) + 1);
                    assert_eq!(
                        composed_clamped_rational(src.n_max, src.d, dst.d, dst.n_max),
                        dst.n_max
                    );
                }
            }
        }

        // Verify exact 12-bit -> 10-bit equals `n >> 2` for all 4096 codes
        let c12 = Codec::new("adc_12bit", CodecKind::Binned, 4096, 4095).unwrap();
        let c10 = Codec::new("uno_10bit", CodecKind::Binned, 1024, 1023).unwrap();
        for n in 0..=4095 {
            assert_eq!(
                c10.to_code_unipolar(c12.to_analog_unipolar(n).unwrap()),
                n >> 2
            );
        }
    }

    #[test]
    fn t_num_4_rejects_d_exceeding_m_and_i32_band_overflow() {
        assert!(matches!(
            Codec::new("too_wide", CodecKind::Binned, 1 << 30, (1 << 30) - 1),
            Err(CodecError::ResolutionExceedsScale { .. })
        ));

        let s7 = Codec::new("s7_span", CodecKind::Span, 27_648, 27_648).unwrap();
        // Realistic S7 overrange (+17.6%, ~32511) fits comfortably inside signed i32
        let ok_band = ChannelBand {
            underrange_min: -4_864,
            overrange_max: 32_511,
            overflow_code: Some(32_767),
            wire_break_code: Some(-32_768),
            source_ref: "Siemens S7-1200 System Manual".to_owned(),
        };
        assert!(ok_band.validate_against(&s7).is_ok());

        // Pathological band (> 2.147 * 27648 = 59373) overflows signed i32 ANALOG
        let bad_band = ChannelBand {
            underrange_min: 0,
            overrange_max: 60_000,
            overflow_code: None,
            wire_break_code: None,
            source_ref: "synthetic overflow test".to_owned(),
        };
        assert!(matches!(
            bad_band.validate_against(&s7),
            Err(CodecError::BandLimitOverflow { code: 60_000, .. })
        ));
    }

    #[test]
    fn t_num_5_worked_example_2v_on_0_to_10v_yields_s7_5529() {
        let s7 = Codec::new("s7_span", CodecKind::Span, 27_648, 27_648).unwrap();
        // 2 V on 0..10 V = exactly 20% = 200_000_000 ppb
        let a_2v: u64 = 200_000_000;
        assert_eq!(s7.to_code_unipolar(a_2v), 5529);
    }
}
