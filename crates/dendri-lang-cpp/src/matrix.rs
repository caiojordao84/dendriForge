//! Auto-generated feature matrix entries backed directly by the `dendri-lang-cpp`
//! conformance suite (`docs/lbc/cpp.md` §8, `04-frontends-visual-and-sync.md` §2).

use dendri_frontend_core::{FeatureMatrixEntry, FeatureSupport, LanguageId};

/// Returns all conformance-verified feature-matrix rows for `dendri-lang-cpp` (`LanguageId::Cpp`).
///
/// Every `evidence_test` name maps 1:1 to a live test function in `tests/conformance.rs`.
#[must_use]
pub fn feature_matrix_entries() -> Vec<FeatureMatrixEntry> {
    vec![
        entry(
            "POUs & Organization Blocks",
            "OB_STARTUP / OB_MAIN / OB_HW_INT & typed FC / FB POUs",
            FeatureSupport::Full,
            "roundtrip_arduino_blink_and_pin_reconciliation",
            "setup() <-> OB_STARTUP, loop() <-> OB_MAIN, typed FC (value & T& InOut)",
        ),
        entry(
            "POUs & Organization Blocks",
            "attachInterrupt <-> OB_HW_INT (ISR blocking guard)",
            FeatureSupport::Full,
            "attach_interrupt_promotes_isr_and_forbids_delay",
            "RISING/FALLING/CHANGE/LOW/HIGH with ISR blocking guard",
        ),
        entry(
            "Hardware & I/O",
            "Board I/O Tag Table binding (zero banned magic comments, ADR-020)",
            FeatureSupport::Full,
            "roundtrip_arduino_blink_and_pin_reconciliation",
            "#define / const uint8_t + pinMode reconcile [[boards.program.hardware]] cleanly",
        ),
        entry(
            "Hardware & I/O",
            "digitalRead / digitalWrite / analogRead / analogWrite",
            FeatureSupport::Full,
            "analog_read_write_resolution_and_time_intrinsics",
            "analogWrite lowers to PWM_WRITE(ANALOG_TO_DUTY(CODE_TO_ANALOG(..., \"pwm_8bit\")))",
        ),
        entry(
            "Hardware & I/O",
            "delay / delayMicroseconds / millis / micros",
            FeatureSupport::Full,
            "analog_read_write_resolution_and_time_intrinsics",
            "Maps to WAIT_TIME / WAIT_TIME_US / MILLIS / MICROS",
        ),
        entry(
            "Numeric Contract",
            "Cross-platform reduced-rational ADC scaling (never emits M=10^9)",
            FeatureSupport::Full,
            "reduced_rational_cross_platform_adc_emission_never_emits_m",
            "Uses ReducedRationalPlan::emit_cpp with mandatory span->binned clamp",
        ),
        entry(
            "Types & Widths",
            "[cpu] int_bits mapping (16-bit AVR vs 32-bit targets)",
            FeatureSupport::Full,
            "int_bits_16_avr_vs_32_target_mapping",
            "int -> INT/DINT, unsigned int -> UINT/UDINT, double -> REAL/LREAL",
        ),
        entry(
            "Structured Statements & Types",
            "Control flow (if/for/while/switch/case), expressions, fixed-width types, arrays, structs & enums",
            FeatureSupport::Full,
            "roundtrip_functions_structs_enums_arrays_control_flow",
            "if/else, for, while, do-while, switch/case, compound assigns, ++/--, ternary ?:, bitwise ops",
        ),
        entry(
            "Function Blocks",
            "Standard IEC 61131-3 FBs (TON/TOF/TP, CTU/CTD/CTUD, R_TRIG/F_TRIG, SR/RS, Comm)",
            FeatureSupport::Full,
            "iec_fb_instances_and_cross_paradigm_projections",
            "Typed FB instances with .IN/.Q/.ET/.CV pins across C++ and cross-paradigm views",
        ),
        entry(
            "Ladder (IEC 61131-3 LD)",
            "Derived projection (Network -> materialized power-flow ASL / ST / C++)",
            FeatureSupport::Full,
            "ladder_projection_sorts_cells_and_materializes_rail_against_mid_rung_reset",
            "Sorts rungs/cells by (col, row), materializes local _rail, binds digitalRead/digitalWrite",
        ),
        entry(
            "Modules & Sketches",
            "Multi-tab .ino sketch merger & forward-declaration hoisting",
            FeatureSupport::Full,
            "multi_tab_arduino_sketch_merge_and_hoisting",
            "Primary tab first, remaining .ino tabs sorted lexicographically",
        ),
        entry(
            "Modules & Sketches",
            "#include <Arduino.h> -> std.arduino.* & Serial.* UTF-8",
            FeatureSupport::Full,
            "serial_utf8_and_hd44780_display_charset_enforcement",
            "Serial UTF-8 never triggers ERR_STRING_CHARSET (DEF-007)",
        ),
        entry(
            "Foreign & Packs",
            "F1 hash-locked pack.toml symbols (DEF-008)",
            FeatureSupport::ForeignF1,
            "f1_pack_resolution_for_wire_spi_and_servo",
            "Resolves Wire.h / SPI.h / Servo.h signatures via BoardContext::lookup_pack_symbol",
        ),
        entry(
            "Foreign & Packs",
            "Three-level Foreign (F0) & cross-language export guard",
            FeatureSupport::ForeignF0,
            "three_level_foreign_preservation_and_export_gate",
            "Classes/methods, AVR PORTB, vendor intrinsics; 4-part ERR_FOREIGN_EXPORT_BLOCKED",
        ),
        entry(
            "Comments (ADR-027)",
            "C1-C5 verbatim conservation & hostile corpus (C3)",
            FeatureSupport::Full,
            "hostile_comment_corpus_c1_through_c5_and_eol_conservation",
            "Trailing \\, */, nested ST (* *), bidi escaping, CRLF/LF, C4 idempotence",
        ),
        entry(
            "Conformance",
            "LOSSLESS_ASL round-trip (proptest)",
            FeatureSupport::Full,
            "proptest_lossless_asl_roundtrip_16bit_and_32bit",
            "Randomized parse(emit(M)) == M structural identity across 16-bit AVR & 32-bit targets",
        ),
        entry(
            "Conformance",
            "Negative fixtures for every frozen ErrorCode",
            FeatureSupport::Full,
            "negative_fixtures_cover_every_produced_error_code",
            "Genuine negative fixture for every C++ ErrorCode with exact byte span",
        ),
    ]
}

fn entry(
    category: &str,
    feature: &str,
    support: FeatureSupport,
    evidence_test: &str,
    notes: &str,
) -> FeatureMatrixEntry {
    FeatureMatrixEntry {
        category: category.to_owned(),
        feature: feature.to_owned(),
        language: LanguageId::Cpp,
        support,
        evidence_test: evidence_test.to_owned(),
        notes: notes.to_owned(),
    }
}
