//! Cross-platform reduced-rational ADC / code emission helpers (`01` §3.3, `04` §2 #3).
//!
//! Normative emitter rule:
//! - Emitters **NEVER** emit `ASL_ANALOG_FULL_SCALE` (`M = 10^9`).
//! - Cross-platform native translations compute `g = gcd(D1, D2)`, `D1_red = D1 / g`,
//!   `D2_red = D2 / g`, and emit the clamped reduced rational
//!   `min(n_max2, floor(n * D2_red / D1_red))` (or signed truncation-toward-zero equivalent
//!   for `span_bipolar`).
//! - Intermediate integer width is selected by exact range analysis:
//!   - Unipolar: `n_max1 * D2_red < 2^32` -> 32-bit (`uint32_t` / `UDINT`), else 64-bit (`uint64_t` / `ULINT`).
//!   - Bipolar: `n_max1 * D2_red <= i32::MAX` -> 32-bit (`int32_t` / `DINT`), else 64-bit (`int64_t` / `LINT`).
//! - The clamp is mandatory whenever `floor(n_max1 * D2_red / D1_red) > n_max2` (always true for
//!   `span -> binned`) and omitted when range analysis proves the output stays within `n_max2`.

use dendri_ast::{ASL_ANALOG_FULL_SCALE, Codec, CodecError, CodecKind, gcd_u64, lcm_u128};

/// Selected intermediate integer width for emitted reduced-rational conversion code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmitIntWidth {
    Bits32,
    Bits64,
}

impl EmitIntWidth {
    /// C++ fixed-width type name (`uint32_t`, `uint64_t`, `int32_t`, or `int64_t`).
    #[must_use]
    pub const fn cpp_type(self, signed: bool) -> &'static str {
        match (self, signed) {
            (Self::Bits32, false) => "uint32_t",
            (Self::Bits64, false) => "uint64_t",
            (Self::Bits32, true) => "int32_t",
            (Self::Bits64, true) => "int64_t",
        }
    }

    /// C++ integer literal suffix safe even on 16-bit AVR (`UL`, `ULL`, `L`, `LL`).
    #[must_use]
    pub const fn cpp_literal_suffix(self, signed: bool) -> &'static str {
        match (self, signed) {
            (Self::Bits32, false) => "UL",
            (Self::Bits64, false) => "ULL",
            (Self::Bits32, true) => "L",
            (Self::Bits64, true) => "LL",
        }
    }

    /// IEC 61131-3 ST integer type name (`UDINT`, `ULINT`, `DINT`, or `LINT`).
    #[must_use]
    pub const fn st_type(self, signed: bool) -> &'static str {
        match (self, signed) {
            (Self::Bits32, false) => "UDINT",
            (Self::Bits64, false) => "ULINT",
            (Self::Bits32, true) => "DINT",
            (Self::Bits64, true) => "LINT",
        }
    }
}

/// Verified reduced-rational conversion plan between a source [`Codec`] and destination [`Codec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducedRationalPlan {
    /// Reduced source divisor `D1 / gcd(D1, D2)`.
    pub d1_red: u64,
    /// Reduced destination multiplier `D2 / gcd(D1, D2)`.
    pub d2_red: u64,
    /// Greatest common divisor `gcd(D1, D2)`.
    pub gcd: u64,
    /// Destination native maximum magnitude (`n_max2`).
    pub n_max2: u64,
    /// `true` iff range analysis requires clamping to `n_max2` (always true for `span -> binned`).
    pub needs_clamp: bool,
    /// `true` iff either codec is `CodecKind::SpanBipolar` (requires signed truncation toward zero).
    pub is_signed: bool,
    /// Selected intermediate width (`Bits32` vs `Bits64`) guaranteed free of overflow.
    pub int_width: EmitIntWidth,
    /// Exact right-shift bit count when unipolar, `d2_red == 1`, and `d1_red` is a power of two > 1.
    pub right_shift: Option<u32>,
    /// `true` iff conversion is an exact no-op identity (`D1 == D2` and `!needs_clamp`).
    pub is_identity: bool,
    /// `true` iff `lcm(D1, D2) <= M`, proving algebraic equivalence to Lemma 2 composition.
    pub lcm_satisfies_lemma2: bool,
}

impl ReducedRationalPlan {
    /// Computes and verifies a reduced-rational conversion plan from `src` to `dst`.
    ///
    /// Rejects invalid codecs (`D > M`, invalid `n_max`) via [`Codec::validate`].
    pub fn compute(src: &Codec, dst: &Codec) -> Result<Self, CodecError> {
        src.validate()?;
        dst.validate()?;

        let g = gcd_u64(src.d, dst.d);
        let d1_red = src.d / g;
        let d2_red = dst.d / g;

        let is_signed = src.kind == CodecKind::SpanBipolar || dst.kind == CodecKind::SpanBipolar;

        let max_product = u128::from(src.n_max) * u128::from(d2_red);
        let int_width = if is_signed {
            if max_product <= (i32::MAX as u128) {
                EmitIntWidth::Bits32
            } else {
                EmitIntWidth::Bits64
            }
        } else if max_product < (1u128 << 32) {
            EmitIntWidth::Bits32
        } else {
            EmitIntWidth::Bits64
        };

        let max_unclamped = max_product / u128::from(d1_red);
        let needs_clamp = max_unclamped > u128::from(dst.n_max);

        // Right-shift optimization is valid ONLY for unsigned/unipolar codecs (signed right shift
        // of negative numbers rounds toward -infinity instead of truncating toward zero).
        let right_shift = if !is_signed && d2_red == 1 && d1_red > 1 && d1_red.is_power_of_two() {
            Some(d1_red.trailing_zeros())
        } else {
            None
        };

        let is_identity = d1_red == 1 && d2_red == 1 && !needs_clamp;
        let lcm_satisfies_lemma2 = lcm_u128(src.d, dst.d) <= u128::from(ASL_ANALOG_FULL_SCALE);

        Ok(Self {
            d1_red,
            d2_red,
            gcd: g,
            n_max2: dst.n_max,
            needs_clamp,
            is_signed,
            int_width,
            right_shift,
            is_identity,
            lcm_satisfies_lemma2,
        })
    }

    /// Evaluates the planned unipolar formula in the exact selected integer width (`u32` or `u64`).
    #[must_use]
    pub fn eval_unipolar(&self, n: u64) -> u64 {
        if self.is_identity {
            return n;
        }
        let raw = match self.int_width {
            EmitIntWidth::Bits32 => {
                let v = u32::try_from(n).unwrap_or(u32::MAX);
                let q = if let Some(shift) = self.right_shift {
                    v >> shift
                } else {
                    let mul = u32::try_from(self.d2_red).unwrap_or(u32::MAX);
                    let div = u32::try_from(self.d1_red).unwrap_or(1);
                    v.saturating_mul(mul) / div
                };
                u64::from(q)
            }
            EmitIntWidth::Bits64 => {
                if let Some(shift) = self.right_shift {
                    n >> shift
                } else {
                    n.saturating_mul(self.d2_red) / self.d1_red
                }
            }
        };
        if self.needs_clamp {
            raw.min(self.n_max2)
        } else {
            raw
        }
    }

    /// Evaluates the planned bipolar formula (truncation toward zero) in the exact selected
    /// signed integer width (`i32` or `i64`).
    #[must_use]
    pub fn eval_bipolar(&self, n: i64) -> i64 {
        if self.is_identity {
            return n;
        }
        let max_i = i64::try_from(self.n_max2).unwrap_or(i64::MAX);
        let raw = match self.int_width {
            EmitIntWidth::Bits32 => {
                let v = i32::try_from(n).unwrap_or(if n < 0 { i32::MIN } else { i32::MAX });
                let mul = i32::try_from(self.d2_red).unwrap_or(i32::MAX);
                let div = i32::try_from(self.d1_red).unwrap_or(1);
                i64::from(v.saturating_mul(mul) / div)
            }
            EmitIntWidth::Bits64 => {
                let mul = i64::try_from(self.d2_red).unwrap_or(i64::MAX);
                let div = i64::try_from(self.d1_red).unwrap_or(1);
                n.saturating_mul(mul) / div
            }
        };
        if self.needs_clamp {
            raw.clamp(-max_i, max_i)
        } else {
            raw
        }
    }

    /// Emits a self-contained C++ expression evaluating `expr` strictly once, using exact
    /// 32-bit/64-bit integer width and NEVER emitting `M = 10^9`.
    #[must_use]
    pub fn emit_cpp(&self, expr: &str) -> String {
        if self.is_identity {
            return expr.to_owned();
        }
        let ty = self.int_width.cpp_type(self.is_signed);
        let suf = self.int_width.cpp_literal_suffix(self.is_signed);

        if !self.is_signed
            && !self.needs_clamp
            && let Some(shift) = self.right_shift
        {
            return format!("(static_cast<{ty}>({expr}) >> {shift}u)");
        }

        let scale_var = |var: &str| -> String {
            if let Some(shift) = self.right_shift {
                format!("({var} >> {shift}u)")
            } else if self.d1_red == 1 && self.d2_red == 1 {
                var.to_owned()
            } else if self.d1_red == 1 {
                format!("({var} * {}{suf})", self.d2_red)
            } else if self.d2_red == 1 {
                format!("({var} / {}{suf})", self.d1_red)
            } else {
                format!("(({var} * {}{suf}) / {}{suf})", self.d2_red, self.d1_red)
            }
        };

        if !self.needs_clamp {
            let cast_expr = format!("static_cast<{ty}>({expr})");
            return scale_var(&cast_expr);
        }

        // Clamped: immediately-invoked C++11 lambda evaluates `expr` strictly once with zero
        // extra headers (safe on both host `g++` and AVR `avr-gcc`).
        let scaled = scale_var("_v");
        let nmax = self.n_max2;
        if self.is_signed {
            format!(
                "([]({ty} _v) -> {ty} {{ {ty} _q = {scaled}; return _q > {nmax}{suf} ? {nmax}{suf} : (_q < -{nmax}{suf} ? -{nmax}{suf} : _q); }})(static_cast<{ty}>({expr}))"
            )
        } else {
            format!(
                "([]({ty} _v) -> {ty} {{ {ty} _q = {scaled}; return _q > {nmax}{suf} ? {nmax}{suf} : _q; }})(static_cast<{ty}>({expr}))"
            )
        }
    }

    /// Emits an IEC 61131-3 ST expression using exact `UDINT`/`ULINT`/`DINT`/`LINT` arithmetic
    /// and `MIN`/`LIMIT` clamping, NEVER emitting `M = 10^9`.
    #[must_use]
    pub fn emit_st(&self, expr: &str) -> String {
        if self.is_identity {
            return expr.to_owned();
        }
        let ty = self.int_width.st_type(self.is_signed);

        if !self.is_signed
            && !self.needs_clamp
            && let Some(shift) = self.right_shift
        {
            return format!("SHR(TO_{ty}({expr}), {shift})");
        }

        let cast_expr = format!("TO_{ty}({expr})");
        let scaled = if let Some(shift) = self.right_shift {
            format!("SHR({cast_expr}, {shift})")
        } else if self.d1_red == 1 && self.d2_red == 1 {
            cast_expr
        } else if self.d1_red == 1 {
            format!("({cast_expr} * {ty}#{})", self.d2_red)
        } else if self.d2_red == 1 {
            format!("({cast_expr} / {ty}#{})", self.d1_red)
        } else {
            format!(
                "(({cast_expr} * {ty}#{}) / {ty}#{})",
                self.d2_red, self.d1_red
            )
        };

        if !self.needs_clamp {
            return scaled;
        }

        if self.is_signed {
            format!(
                "LIMIT({ty}#-{}, {scaled}, {ty}#{})",
                self.n_max2, self.n_max2
            )
        } else {
            format!("MIN({ty}#{}, {scaled})", self.n_max2)
        }
    }
}

/// Returns `true` if `emitted_code` contains any forbidden representation of
/// `ASL_ANALOG_FULL_SCALE` (`1000000000`, `1_000_000_000`, `1e9`, `1E9`).
#[must_use]
pub fn contains_analog_full_scale_literal(emitted_code: &str) -> bool {
    emitted_code.contains("1000000000")
        || emitted_code.contains("1_000_000_000")
        || emitted_code.contains("1e9")
        || emitted_code.contains("1E9")
}

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_ast::composed_clamped_rational;

    #[test]
    fn reduced_rational_12bit_to_10bit_emits_right_shift_and_never_m() {
        let c12 = Codec::new("adc_12bit", CodecKind::Binned, 4096, 4095).unwrap();
        let c10 = Codec::new("uno_10bit", CodecKind::Binned, 1024, 1023).unwrap();

        let plan = ReducedRationalPlan::compute(&c12, &c10).unwrap();
        assert_eq!(plan.gcd, 1024);
        assert_eq!(plan.d1_red, 4);
        assert_eq!(plan.d2_red, 1);
        assert!(!plan.needs_clamp);
        assert_eq!(plan.int_width, EmitIntWidth::Bits32);
        assert_eq!(plan.right_shift, Some(2));
        assert!(plan.lcm_satisfies_lemma2);

        let cpp = plan.emit_cpp("raw");
        let st = plan.emit_st("raw");
        assert_eq!(cpp, "(static_cast<uint32_t>(raw) >> 2u)");
        assert_eq!(st, "SHR(TO_UDINT(raw), 2)");
        assert!(!contains_analog_full_scale_literal(&cpp));
        assert!(!contains_analog_full_scale_literal(&st));

        for n in 0..=4095 {
            assert_eq!(plan.eval_unipolar(n), n >> 2);
            assert_eq!(
                plan.eval_unipolar(n),
                c10.to_code_unipolar(c12.to_analog_unipolar(n).unwrap())
            );
        }
    }

    #[test]
    fn plc_27648_to_uno_10bit_enforces_mandatory_span_to_binned_clamp() {
        let plc = Codec::new("plc_span_27648", CodecKind::Span, 27_648, 27_648).unwrap();
        let uno = Codec::new("uno_10bit", CodecKind::Binned, 1024, 1023).unwrap();

        let plan = ReducedRationalPlan::compute(&plc, &uno).unwrap();
        assert_eq!(plan.gcd, 1024);
        assert_eq!(plan.d1_red, 27);
        assert_eq!(plan.d2_red, 1);
        assert!(plan.needs_clamp, "span -> binned MUST clamp top code");
        assert_eq!(plan.int_width, EmitIntWidth::Bits32);

        let cpp = plan.emit_cpp("iw64");
        let st = plan.emit_st("IW64");
        assert!(cpp.contains("1023UL"));
        assert!(cpp.contains("27UL"));
        assert!(st.contains("MIN(UDINT#1023"));
        assert!(st.contains("UDINT#27"));
        assert!(!contains_analog_full_scale_literal(&cpp));
        assert!(!contains_analog_full_scale_literal(&st));

        // Exhaustive equivalence against ASL M=10^9 round-trip for all 27,649 codes!
        for n in 0..=27_648 {
            let via_asl = uno.to_code_unipolar(plc.to_analog_unipolar(n).unwrap());
            let planned = plan.eval_unipolar(n);
            let ref_rational = composed_clamped_rational(n, plc.d, uno.d, uno.n_max);
            assert_eq!(planned, via_asl, "mismatch at n={n}");
            assert_eq!(planned, ref_rational, "rational mismatch at n={n}");
        }
        // Top code 27648 clamps cleanly to 1023 (NOT 1024!)
        assert_eq!(plan.eval_unipolar(27_648), 1023);
    }

    #[test]
    fn selects_64bit_width_precisely_when_product_reaches_2_pow_32() {
        // 24-bit ADC (n_max = 16_777_215) -> 100_000-count span:
        // gcd(16_777_216, 100_000) = 32 -> d2_red = 3125
        // n_max1 * d2_red = 52_428_796_875 >= 2^32 -> MUST select Bits64!
        let adc24 = Codec::new("adc_24bit", CodecKind::Binned, 1 << 24, (1 << 24) - 1).unwrap();
        let span100k = Codec::new("span_100k", CodecKind::Span, 100_000, 100_000).unwrap();
        let plan64 = ReducedRationalPlan::compute(&adc24, &span100k).unwrap();
        assert_eq!(plan64.int_width, EmitIntWidth::Bits64);
        assert_eq!(plan64.d2_red, 3_125);
        assert_eq!(plan64.d1_red, 524_288);
        assert!(!plan64.needs_clamp);
        let cpp64 = plan64.emit_cpp("adc");
        let st64 = plan64.emit_st("adc");
        assert!(cpp64.contains("uint64_t"));
        assert!(cpp64.contains("3125ULL"));
        assert!(st64.contains("ULINT#3125"));
        assert!(!contains_analog_full_scale_literal(&cpp64));
        assert!(!contains_analog_full_scale_literal(&st64));
        assert_eq!(plan64.eval_unipolar(adc24.n_max), 99_999);

        // Uno 10-bit -> PLC 27648: n_max1 * d2_red = 1023 * 27 = 27_621 < 2^32 -> Bits32!
        let uno = Codec::new("uno_10bit", CodecKind::Binned, 1024, 1023).unwrap();
        let plc = Codec::new("plc_span_27648", CodecKind::Span, 27_648, 27_648).unwrap();
        let plan32 = ReducedRationalPlan::compute(&uno, &plc).unwrap();
        assert_eq!(plan32.int_width, EmitIntWidth::Bits32);
        assert_eq!(plan32.d1_red, 1);
        assert_eq!(plan32.d2_red, 27);
        assert!(!plan32.needs_clamp);
        assert_eq!(plan32.emit_cpp("x"), "(static_cast<uint32_t>(x) * 27UL)");
        assert_eq!(plan32.emit_st("x"), "(TO_UDINT(x) * UDINT#27)");
    }

    #[test]
    fn bipolar_truncation_toward_zero_never_uses_right_shift() {
        let bip_big = Codec::new("bip_big", CodecKind::SpanBipolar, 27_648, 27_648).unwrap();
        let bip_small = Codec::new("bip_small", CodecKind::SpanBipolar, 6_912, 6_912).unwrap();
        // d1 / d2 = 4 (power of two), but bipolar MUST NOT use right shift!
        let plan = ReducedRationalPlan::compute(&bip_big, &bip_small).unwrap();
        assert!(plan.is_signed);
        assert_eq!(plan.right_shift, None);
        assert_eq!(plan.d1_red, 4);
        assert_eq!(plan.d2_red, 1);
        assert_eq!(plan.eval_bipolar(-3), 0); // truncation toward zero! (-3 >> 2 would be -1)
        assert_eq!(plan.eval_bipolar(-4), -1);
        assert_eq!(plan.eval_bipolar(-27_648), -6_912);
        assert_eq!(plan.eval_bipolar(27_648), 6_912);
        assert!(plan.emit_cpp("v").contains("int32_t"));
        assert!(plan.emit_st("v").contains("DINT#4"));
    }

    #[test]
    fn identical_codecs_emit_pure_identity() {
        let uno = Codec::new("uno_10bit", CodecKind::Binned, 1024, 1023).unwrap();
        let plan = ReducedRationalPlan::compute(&uno, &uno).unwrap();
        assert!(plan.is_identity);
        assert_eq!(plan.emit_cpp("val"), "val");
        assert_eq!(plan.emit_st("val"), "val");
    }

    #[test]
    fn all_unipolar_catalog_pairs_match_asl_composition_and_never_emit_m() {
        let catalog = [
            Codec::new("pwm_8bit", CodecKind::Binned, 256, 255).unwrap(),
            Codec::new("uno_10bit", CodecKind::Binned, 1024, 1023).unwrap(),
            Codec::new("adc_12bit", CodecKind::Binned, 4096, 4095).unwrap(),
            Codec::new("plc_span_27648", CodecKind::Span, 27_648, 27_648).unwrap(),
            Codec::new("adc_16bit", CodecKind::Binned, 65_536, 65_535).unwrap(),
        ];

        for src in &catalog {
            for dst in &catalog {
                let plan = ReducedRationalPlan::compute(src, dst).unwrap();
                assert!(plan.lcm_satisfies_lemma2);
                let cpp = plan.emit_cpp("raw_code");
                let st = plan.emit_st("raw_code");
                assert!(!contains_analog_full_scale_literal(&cpp), "{cpp}");
                assert!(!contains_analog_full_scale_literal(&st), "{st}");

                // Sample boundaries + dense stride across the full source domain
                let step = (src.n_max / 512).max(1);
                let mut n = 0u64;
                while n <= src.n_max {
                    let via_asl = dst.to_code_unipolar(src.to_analog_unipolar(n).unwrap());
                    assert_eq!(
                        plan.eval_unipolar(n),
                        via_asl,
                        "pair {} -> {} mismatch at n={n}",
                        src.id,
                        dst.id
                    );
                    n = n.saturating_add(step);
                }
                let via_asl_top = dst.to_code_unipolar(src.to_analog_unipolar(src.n_max).unwrap());
                assert_eq!(plan.eval_unipolar(src.n_max), via_asl_top);
            }
        }
    }
}
