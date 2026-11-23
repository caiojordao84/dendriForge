//! CI Gate 4 — Real Negative Error Corpus (45 / 45 frozen `ErrorCode` variants).
//!
//! Zero synthetic `fixture_inject` harnesses: every single `ErrorCode` variant
//! has a genuine `.toml` fixture file under `tests/fixtures/negative/<CODE>.toml`
//! executed through the real `dendri-project` + `dendri-validate` pipeline.

use dendri_ast::{AuthoringLanguage, ChannelBand, Codec, CodecKind, CommentEncoding};
use dendri_project::load_project_str;
use dendri_span::{ErrorCode, SourceMap};
use dendri_validate::{
    BoardProfileMeta, MemoryProfileCatalog, PortMeta, ValidateOptions, validate_project,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("negative")
}

fn test_catalog() -> MemoryProfileCatalog {
    let mut cat = MemoryProfileCatalog::builtin_reference();

    // Real board profile with an intentionally invalid wide codec (D = 2^30 > M)
    let mut wide_codecs = BTreeMap::new();
    wide_codecs.insert(
        "too_wide_30bit".to_owned(),
        Codec {
            id: "too_wide_30bit".to_owned(),
            kind: CodecKind::Binned,
            d: 1 << 30,
            n_max: (1 << 30) - 1,
            source_ref: Some("synthetic 30-bit codec profile".to_owned()),
        },
    );
    cat.insert_board(BoardProfileMeta {
        id: "test-board-wide-codec".to_owned(),
        ports: BTreeMap::new(),
        codecs: wide_codecs,
        channel_bands: BTreeMap::new(),
    });

    // Real board profile with a channel band limit that overflows signed i32 ANALOG
    let mut band_codecs = BTreeMap::new();
    band_codecs.insert(
        "plc_span_27648".to_owned(),
        Codec {
            id: "plc_span_27648".to_owned(),
            kind: CodecKind::Span,
            d: 27_648,
            n_max: 27_648,
            source_ref: Some("IEC 61131-2 / Industrial 27648-count analog scale".to_owned()),
        },
    );
    let mut bands = BTreeMap::new();
    bands.insert(
        "IW64".to_owned(),
        ChannelBand {
            underrange_min: 0,
            overrange_max: 60_000, // 60000/27648 * 10^9 > i32::MAX
            overflow_code: None,
            wire_break_code: None,
            source_ref: "overflow band profile".to_owned(),
        },
    );
    let mut ports = BTreeMap::new();
    ports.insert(
        "IW64".to_owned(),
        PortMeta {
            digital_in: false,
            digital_out: false,
            pullup: false,
            pulldown: false,
            analog_in: true,
            analog_out: false,
            pwm: false,
            bus: false,
            nominal_v: 10.0,
            v_abs_max_v: 12.0,
        },
    );
    cat.insert_board(BoardProfileMeta {
        id: "test-board-overflow-band".to_owned(),
        ports,
        codecs: band_codecs,
        channel_bands: bands,
    });

    cat
}

#[test]
fn every_single_frozen_error_code_has_a_real_producer_fixture() {
    let dir = corpus_dir();
    let catalog = test_catalog();

    assert_eq!(ErrorCode::ALL.len(), 45);
    for &expected_code in ErrorCode::ALL {
        let filename = format!("{}.toml", expected_code.as_str());
        let path = dir.join(&filename);
        assert!(
            path.is_file(),
            "missing real negative fixture file `{}` for {}",
            path.display(),
            expected_code
        );

        let src = std::fs::read_to_string(&path).unwrap();
        let mut sm = SourceMap::new();
        let fid = sm.add_file(&filename, src.clone());

        let mut opts = ValidateOptions {
            file_id: Some(fid),
            ..ValidateOptions::default()
        };
        if expected_code == ErrorCode::WarnCommentEncoding {
            opts.comment_encoding = CommentEncoding::AsciiFold;
        }
        if matches!(
            expected_code,
            ErrorCode::ErrForeignExportBlocked | ErrorCode::InfoCommentRelocated
        ) {
            opts.export_target_language = Some(AuthoringLanguage::St);
        }

        let produced_codes: Vec<ErrorCode> = match load_project_str(&src, Some(fid)) {
            Err(err) => {
                let diag = err
                    .as_diagnostic()
                    .unwrap_or_else(|| panic!("{filename}: expected Diagnostic, got {err:?}"));
                assert!(
                    !diag.message.is_empty(),
                    "{filename}: diagnostic message must be non-empty"
                );
                vec![diag.code]
            }
            Ok(proj) => {
                let bag = validate_project(&proj, &catalog, &opts);
                bag.all().iter().map(|d| d.code).collect()
            }
        };

        assert!(
            produced_codes.contains(&expected_code),
            "fixture `{filename}` did NOT produce `{expected_code}`! Got: {produced_codes:?}"
        );
    }
}

#[test]
fn utf8_serial_uart_strings_never_trigger_display_charset_error() {
    let toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "UartUtf8Ok"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[boards.program]
imports = ["std.iec.comm"]

[[boards.program.data]]
id = "n_tx"
name = "tx"
scope = "var"
ty = { kind = "fb", fb_kind = { name = "uart_send" } }

[[boards.program.pous]]
id = "n_main"
name = "Main"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "fb_invoke"
id = "n_s1"
instance = "tx"
inputs = [{ name = "REQ", value = "true" }, { name = "DATA", value = "\"Ação 🚀 25°C\"" }]
"#;
    let proj = load_project_str(toml, None).unwrap();
    let bag = validate_project(&proj, &test_catalog(), &ValidateOptions::default());
    assert!(
        bag.all().is_empty(),
        "UART_SEND with UTF-8 must produce zero diagnostics, got: {:?}",
        bag.all()
    );
}

#[test]
fn c5_comment_field_and_unknown_key_negative_checks() {
    let cat = test_catalog();

    // 1. Forbidden token_ordinal on non-inner placement
    let extra_ordinal = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "C5"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[[boards.program.pous]]
id = "n_p1"
name = "Main"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.comments]]
id = "c1"
anchor = { kind = "node", id = "n_p1" }
placement = "leading"
token_ordinal = 1
text = " illegal ordinal"
origin = { language = "cpp", style = "line" }
"#;
    let p1 = load_project_str(extra_ordinal, None).unwrap();
    let b1 = validate_project(&p1, &cat, &ValidateOptions::default());
    assert!(
        b1.all()
            .iter()
            .any(|d| d.code == ErrorCode::ErrCommentField)
    );

    // 2. Non-node comment with placement = "leading" (only header/body allowed)
    let bad_proj_placement = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "C5"

[[comments]]
id = "c_p"
anchor = { kind = "project" }
placement = "leading"
text = " invalid placement on project anchor"
origin = { language = "asl", style = "visual" }
"#;
    let p2 = load_project_str(bad_proj_placement, None).unwrap();
    let b2 = validate_project(&p2, &cat, &ValidateOptions::default());
    assert!(
        b2.all()
            .iter()
            .any(|d| d.code == ErrorCode::ErrCommentField)
    );

    // 3. Unknown key inside [[comments]] rejected by deny_unknown_fields
    let unknown_comment_key = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "C5"

[[comments]]
id = "c_u"
anchor = { kind = "project" }
placement = "header"
text = " ok"
origin = { language = "asl", style = "line" }
rogue_comment_key = true
"#;
    let err = load_project_str(unknown_comment_key, None).unwrap_err();
    assert_eq!(
        err.as_diagnostic().unwrap().code,
        ErrorCode::ErrUnknownField
    );
}

#[test]
fn allow_foreign_flag_permits_cross_language_export_cleanly() {
    let path = corpus_dir().join("ERR_FOREIGN_EXPORT_BLOCKED.toml");
    let src = std::fs::read_to_string(&path).unwrap();
    let proj = load_project_str(&src, None).unwrap();

    let opts = ValidateOptions {
        export_target_language: Some(AuthoringLanguage::St),
        allow_foreign_export: true,
        ..ValidateOptions::default()
    };
    let bag = validate_project(&proj, &test_catalog(), &opts);
    assert!(
        !bag.has_errors(),
        "expected --allow-foreign to succeed, got {:?}",
        bag.all()
    );
}

#[test]
fn clean_valid_multi_board_project_passes_with_zero_diagnostics() {
    let toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "CleanCell"

[[comments]]
id = "c_hdr"
anchor = { kind = "project" }
placement = "header"
text = " Valid multi-board cell"
origin = { language = "asl", style = "line" }

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[boards.program]
imports = ["std.iec.*", "std.arduino.*"]

[[boards.program.hardware]]
id = "n_btn"
label = "BTN"
pin = "D2"
kind = "digital_in"
pull = "pull_up"

[[boards.program.hardware]]
id = "n_led"
label = "LED"
pin = "D13"
kind = "digital_out"

[[boards.program.pous]]
id = "n_main"
name = "Main"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "call"
id = "n_s1"
callee = "DIGITAL_WRITE"
args = [{ value = "LED" }, { value = "BTN" }]

[[boards.comments]]
id = "c_b1"
anchor = { kind = "node", id = "n_s1" }
placement = "leading"
text = " Mirror button to LED"
origin = { language = "cpp", style = "line" }

[[components]]
id = "led1"
profile = "led"
label = "D1"

[[wires]]
id = "w1"
from = "uno.D13"
to = "led1.A"
signal = "digital"
"#;
    let proj = load_project_str(toml, None).unwrap();
    let bag = validate_project(&proj, &test_catalog(), &ValidateOptions::default());
    assert!(
        bag.all().is_empty(),
        "expected 0 diagnostics on valid project, got: {:?}",
        bag.all()
    );
}
