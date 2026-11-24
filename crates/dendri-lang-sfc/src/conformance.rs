//! Auto-generated feature-support matrix entries driven directly by the
//! `dendri-lang-sfc` conformance test suite (`04-frontends-visual-and-sync.md` §2,
//! `docs/lbc/sfc.md` §8).

use dendri_frontend_core::conformance::{FeatureMatrixEntry, FeatureSupport};
use dendri_frontend_core::traits::LanguageId;

/// Returns the authoritative GRAFCET / SFC feature-support matrix entries backed
/// 1:1 by live tests in `tests/sfc_conformance.rs`.
#[must_use]
pub fn feature_matrix_entries() -> Vec<FeatureMatrixEntry> {
    let entry = |category: &str,
                 feature: &str,
                 support: FeatureSupport,
                 evidence_test: &str,
                 notes: &str| FeatureMatrixEntry {
        category: category.to_owned(),
        feature: feature.to_owned(),
        language: LanguageId::Sfc,
        support,
        evidence_test: evidence_test.to_owned(),
        notes: notes.to_owned(),
    };

    vec![
        entry(
            "GRAFCET / SFC",
            "Structural PouBody::Chart (Zero __sfc_* in ASL)",
            FeatureSupport::Full,
            "proptest_lossless_asl_roundtrip_across_topologies",
            "LOSSLESS_ASL parse(emit(M)) == M for sequential, OR, AND, and macro charts",
        ),
        entry(
            "GRAFCET / SFC",
            "IEC 60848 Rule 5 Two-Phase Evolution",
            FeatureSupport::Full,
            "rule5_simultaneous_deactivate_activate_and_frozen_receptivity_vm_equivalence",
            "Phase 1 frozen receptivity -> Phase 2a clear -> Phase 2b activate",
        ),
        entry(
            "GRAFCET / SFC",
            "Selective (OR) Divergence Deterministic Priority",
            FeatureSupport::Full,
            "or_divergence_priority_and_and_divergence_convergence_roundtrip_and_vm",
            "Ascending priority: u32 tie-broken deterministically by declaration order",
        ),
        entry(
            "GRAFCET / SFC",
            "Simultaneous (AND) Divergence & Convergence",
            FeatureSupport::Full,
            "or_divergence_priority_and_and_divergence_convergence_roundtrip_and_vm",
            "Multi-step FROM (...) / TO (...) synchronization barriers",
        ),
        entry(
            "GRAFCET / SFC",
            "IEC 61131-3 Table 47 Action Qualifiers (N/S/R/L/D/P/P1/P0)",
            FeatureSupport::Full,
            "all_eight_table47_action_qualifiers_roundtrip_and_vm_projection_match",
            "N auto-reset, S latch, R override, L/D TIME durations, P/P1/P0 edges",
        ),
        entry(
            "GRAFCET / SFC",
            "Native Step Attributes (<Step>.X, <Step>.T, <Macro>.DONE)",
            FeatureSupport::Full,
            "native_step_attributes_and_macro_step_entry_exit_roundtrip",
            "Read-only first-class attributes; direct writes rejected with ERR_CONST_WRITE",
        ),
        entry(
            "GRAFCET / SFC",
            "Macro-Steps (Enclosed Sub-Charts, Scan-1 Init & Re-Entry)",
            FeatureSupport::Full,
            "native_step_attributes_and_macro_step_entry_exit_roundtrip",
            "Clean sub-chart entry/exit reset with zero duplicate setup statements",
        ),
        entry(
            "GRAFCET / SFC",
            "Evolve-to-Stability & Instability Cycle Guard",
            FeatureSupport::Full,
            "evolve_to_stability_roundtrip_and_projection_bound",
            "EVOLVE_TO_STABILITY / STABILITY := TRUE with bounded iteration guard",
        ),
        entry(
            "GRAFCET / SFC",
            "Derived ST/ASL Statement Projection (RuSTy Gate Ready)",
            FeatureSupport::Full,
            "projection_matches_dendri_vm_native_sfc_bit_for_bit",
            "project_chart_to_statements verified bit-for-bit against dendri-vm",
        ),
        entry(
            "Hardware & I/O",
            "Board I/O Tag Table & IEC Direct Addresses (%IX/%QX/%IW)",
            FeatureSupport::Full,
            "electro_pneumatic_a_plus_b_plus_a_minus_b_minus_roundtrip_and_vm",
            "Deterministic port <-> IEC mapping with zero banned magic comments (ADR-020)",
        ),
        entry(
            "Comments",
            "ADR-027 Verbatim Conservation (C1-C5 & C3 Hostile Corpus)",
            FeatureSupport::Full,
            "hostile_comment_corpus_and_c1_c5_conservation",
            "Visual, Line (//), and nested ParenBlock ((* ... *)) across all encodings",
        ),
        entry(
            "Foreign & Packs",
            "Three-Level Foreign Preservation & Cross-Language Export Guard",
            FeatureSupport::ForeignF0,
            "three_level_foreign_roundtrip_and_cross_language_export_guard",
            "Same-language lossless round-trip; cross-language guarded by ERR_FOREIGN_EXPORT_BLOCKED",
        ),
        entry(
            "Diagnostics",
            "Negative Fixtures for Every Frozen SFC ErrorCode",
            FeatureSupport::Full,
            "negative_fixtures_cover_every_sfc_error_code",
            "Verified 1:1 coverage of docs/lbc/sfc.md §1.5 ErrorCodes",
        ),
    ]
}
