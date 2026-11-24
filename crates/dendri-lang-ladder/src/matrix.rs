//! Conformance-driven feature-support matrix entries for `dendri-lang-ladder`
//! (`docs/lbc/ladder.md` §8, `04-frontends-visual-and-sync.md` §2).

use dendri_frontend_core::{FeatureMatrixEntry, FeatureSupport, LanguageId};

/// Returns the conformance-verified feature-matrix entries for the IEC 61131-3 Ladder Diagram
/// frontend (`LanguageId::Ladder`), each backed by a live test in `dendri-lang-ladder`.
#[must_use]
pub fn feature_matrix_entries() -> Vec<FeatureMatrixEntry> {
    vec![
        entry(
            "Structural IR",
            "First-class PouBody::Network (zero __rail_N in ASL)",
            FeatureSupport::Full,
            "proptest_lossless_asl_roundtrip_all_ladder_constructs",
            "Preserves exact NodeIds, rung.order, and (row, col) coordinates",
        ),
        entry(
            "Contacts",
            "NO | |, NC |/|, Positive |P|, Negative |N|",
            FeatureSupport::Full,
            "roundtrip_all_contacts_coils_wires_empty_and_nested_branches",
            "All 4 IEC 61131-3 LD contact variants with edge detection",
        ),
        entry(
            "Coils",
            "Normal ( ), Negated (/), Set (S), Reset (R), Pulse (P)/(N)",
            FeatureSupport::Full,
            "derived_projection_matches_vm_mid_rung_reset_and_unconditional_normal_coil",
            "Normal coil unconditionally writes true & false; pre-mutation rail preserved",
        ),
        entry(
            "Wiring & Branches",
            "Wire, Empty (open circuit = false), nested ParallelOr",
            FeatureSupport::Full,
            "derived_projection_empty_cell_is_open_circuit_and_never_conducts",
            "Arbitrary nested ParallelOr branches; Empty never conducts power",
        ),
        entry(
            "Function Block Boxes",
            "TON/TOF/TP, CTU/CTD/CTUD, R_TRIG/F_TRIG, SR/RS, Comm & User FBs",
            FeatureSupport::Full,
            "roundtrip_all_fb_compare_call_and_foreign_boxes",
            "Primary trigger input bound from rail; Q/QU/Q1/DONE/NDR/ENO propagated",
        ),
        entry(
            "Comparison Boxes",
            "Strictly-typed Compare boxes (EQ, NE, LT, LE, GT, GE)",
            FeatureSupport::Full,
            "derived_projection_compare_and_call_en_eno_parity_with_vm",
            "EN-gated evaluation; short-circuits to false when EN is low",
        ),
        entry(
            "Call / Math / Move Boxes",
            "EN/ENO-gated ADD/SUB/MUL/DIV/MOD/MOVE/Convert/User FC/Structured POU",
            FeatureSupport::Full,
            "derived_projection_compare_and_call_en_eno_parity_with_vm",
            "Executes only when EN == true and passes ENO downstream",
        ),
        entry(
            "Conservative Lifting",
            "Statements -> Network (canonical patterns only)",
            FeatureSupport::Partial,
            "conservative_lifting_canonical_patterns_and_non_liftable_rejection",
            "Lifts latches, set/reset, FBs, comparisons, EN-gated math/move; loops/switch return None",
        ),
        entry(
            "Derived Projection",
            "Network -> Materialized Power-Flow ASL Statements",
            FeatureSupport::Full,
            "derived_projection_matches_vm_mid_rung_reset_and_unconditional_normal_coil",
            "Matches dendri-vm load-time semantics bit-for-bit across scans",
        ),
        entry(
            "Hardware I/O Tags",
            "Bidirectional IEC direct address mapping (%IX/%QX/%IW <-> Ia/Qa/IW)",
            FeatureSupport::Full,
            "hardware_tag_table_plc_and_arduino_roundtrip_without_magic_comments",
            "Zero banned magic comments (ADR-020); validates port & capability",
        ),
        entry(
            "Comments (C1-C5)",
            "Rung Title/Body, Cell notes, Header/Dangling & C3 Hostile Corpus",
            FeatureSupport::Full,
            "comments_c1_c5_and_hostile_corpus_all_encodings",
            "Verbatim Visual + // + nested (* ... *) with C4 re-import idempotence",
        ),
        entry(
            "Foreign Boxes",
            "Locked Foreign box preservation (F0/F1/F2) & Cross-Language Export Guard",
            FeatureSupport::ForeignF0,
            "foreign_box_preservation_and_cross_language_export_guard",
            "Lossless in Ladder; ERR_FOREIGN_EXPORT_BLOCKED (4-part) on ST/C++ export without --allow-foreign",
        ),
        entry(
            "Error Diagnostics",
            "Frozen ErrorCode validation suite (docs/lbc/ladder.md §1.4)",
            FeatureSupport::Full,
            "negative_fixtures_cover_every_ladder_error_code",
            "Genuine negative fixture for every Ladder ErrorCode",
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
