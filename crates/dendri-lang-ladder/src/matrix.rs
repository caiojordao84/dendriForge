//! Conformance-driven feature-support matrix entries for `dendri-lang-ladder`
//! (`docs/lbc/ladder.md` §8, `04-frontends-visual-and-sync.md` §2).

use dendri_frontend_core::{FeatureMatrixEntry, FeatureSupport, LanguageId};

/// Returns the conformance-verified feature-matrix entries for the IEC 61131-3 Ladder Diagram
/// frontend (`LanguageId::Ladder`), each backed by a live test in `dendri-lang-ladder`.
#[must_use]
pub fn feature_matrix_entries() -> Vec<FeatureMatrixEntry> {
    vec![
        entry(
            "Ladder (IEC 61131-3 LD)",
            "Structural PouBody::Network (zero __rail_N in ASL)",
            FeatureSupport::Full,
            "proptest_lossless_asl_roundtrip_all_ladder_constructs",
            "Preserves exact NodeIds, rung.order, and (row, col) coordinates",
        ),
        entry(
            "Ladder (IEC 61131-3 LD)",
            "Contacts (NO | |, NC |/|, Positive |P|, Negative |N|)",
            FeatureSupport::Full,
            "roundtrip_all_contacts_coils_wires_empty_and_nested_branches",
            "All 4 IEC 61131-3 LD contact variants with edge detection",
        ),
        entry(
            "Ladder (IEC 61131-3 LD)",
            "Coils (Normal, Negated, Set, Reset, Pulse P/N & mid-rung mutation immunity)",
            FeatureSupport::Full,
            "derived_projection_matches_vm_mid_rung_reset_and_unconditional_normal_coil",
            "Normal coil unconditionally writes true & false; pre-mutation rail preserved",
        ),
        entry(
            "Ladder (IEC 61131-3 LD)",
            "Wiring & Branches (Wire, Empty open-circuit = false, nested ParallelOr)",
            FeatureSupport::Full,
            "derived_projection_empty_cell_is_open_circuit_and_never_conducts",
            "Arbitrary nested ParallelOr branches; Empty never conducts power",
        ),
        entry(
            "Function Blocks",
            "Standard IEC 61131-3 FBs (TON/TOF/TP, CTU/CTD/CTUD, R_TRIG/F_TRIG, SR/RS, Comm)",
            FeatureSupport::Full,
            "roundtrip_all_fb_compare_call_and_foreign_boxes",
            "Primary trigger input bound from rail; Q/QU/Q1/DONE/NDR/ENO propagated",
        ),
        entry(
            "Ladder (IEC 61131-3 LD)",
            "Comparison Boxes (strictly typed EQ, NE, LT, LE, GT, GE)",
            FeatureSupport::Full,
            "derived_projection_compare_and_call_en_eno_parity_with_vm",
            "EN-gated evaluation; short-circuits to false when EN is low",
        ),
        entry(
            "Ladder (IEC 61131-3 LD)",
            "Call / Math / Move / Structured POU Boxes (EN/ENO gated)",
            FeatureSupport::Full,
            "derived_projection_compare_and_call_en_eno_parity_with_vm",
            "Executes only when EN == true and passes ENO downstream",
        ),
        entry(
            "POUs & Organization Blocks",
            "OB_STARTUP / OB_MAIN / OB_HW_INT & typed FC / FB POUs",
            FeatureSupport::Full,
            "roundtrip_all_fb_compare_call_and_foreign_boxes",
            "Multi-sheet OB/FC/FB networks preserving intact STRUCTURED_POU boxes",
        ),
        entry(
            "Ladder (IEC 61131-3 LD)",
            "Conservative Lifting (Statements -> Network, canonical patterns only)",
            FeatureSupport::Partial,
            "conservative_lifting_canonical_patterns_and_non_liftable_rejection",
            "Lifts latches, set/reset, FBs, comparisons, EN-gated math/move; loops/switch return None",
        ),
        entry(
            "Ladder (IEC 61131-3 LD)",
            "Derived projection (Network -> materialized power-flow ASL / ST / C++)",
            FeatureSupport::Full,
            "derived_projection_matches_vm_mid_rung_reset_and_unconditional_normal_coil",
            "Matches dendri-vm load-time semantics bit-for-bit across scans",
        ),
        entry(
            "Hardware & I/O",
            "Board I/O Tag Table binding (zero banned magic comments, ADR-020)",
            FeatureSupport::Full,
            "hardware_tag_table_plc_and_arduino_roundtrip_without_magic_comments",
            "Bidirectional IEC direct address mapping (%IX/%QX/%IW <-> Ia/Qa/IW) & Arduino ports",
        ),
        entry(
            "Comments (ADR-027)",
            "C1-C5 verbatim conservation & hostile corpus (C3)",
            FeatureSupport::Full,
            "comments_c1_c5_and_hostile_corpus_all_encodings",
            "Rung Title/Body, Cell notes, Visual + // + nested (* *) with C4 re-import idempotence",
        ),
        entry(
            "Foreign & Packs",
            "Three-level Foreign (F0) & cross-language export guard",
            FeatureSupport::ForeignF0,
            "foreign_box_preservation_and_cross_language_export_guard",
            "Locked Foreign box lossless in Ladder; 4-part ERR_FOREIGN_EXPORT_BLOCKED cross-lang",
        ),
        entry(
            "Conformance",
            "LOSSLESS_ASL round-trip (proptest)",
            FeatureSupport::Full,
            "proptest_lossless_asl_roundtrip_all_ladder_constructs",
            "Randomized parse(emit(M)) == M across all Ladder cell & branch topologies",
        ),
        entry(
            "Conformance",
            "Negative fixtures for every frozen ErrorCode",
            FeatureSupport::Full,
            "negative_fixtures_cover_every_ladder_error_code",
            "Genuine negative fixture for every Ladder ErrorCode (docs/lbc/ladder.md §1.4)",
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
        language: LanguageId::Ladder,
        support,
        evidence_test: evidence_test.to_owned(),
        notes: notes.to_owned(),
    }
}
