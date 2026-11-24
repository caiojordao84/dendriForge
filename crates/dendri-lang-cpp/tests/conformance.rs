//! Comprehensive Wave 1 conformance suite for `dendri-lang-cpp` (`docs/lbc/cpp.md`, `04` §2–§4).

use dendri_ast::{
    ActionQualifier, AuthoringLanguage, BinaryOp, Board, BoardLayout, BoardProgram, CasePattern,
    Codec, CodecKind, Comment, CommentAnchor, CommentEncoding, CommentOrigin,
    CommentPlacement as AstCommentPlacement, CommentStyle, DataDecl, EolStyle, ExecutionKind, Expr,
    ExprKind, ForeignFidelity, HardwareBinding, HardwareKind, InterruptEdge, LValue, LadderCell,
    LadderCellKind, LadderCoilKind, LadderContactKind, LadderNetwork, LadderRung, Literal, NodeId,
    ObKind, ParamDecl, ParamDirection, Pou, PouBody, PouKind, Project, ProjectMeta, PullMode,
    ScalarType, SfcAction, SfcChart, SfcStep, SfcTransition, Stmt, StmtKind, StorageScope,
    SwitchCase, TypeDecl, TypeRef, UnaryOp,
};
use dendri_frontend_core::conformance::{
    FeatureMatrixCollector, assert_c1_multiset_invariant, assert_c2_orphan_preservation,
    assert_c4_reimport_idempotence, assert_c5_comment_fields, assert_lossless_asl,
    contains_banned_magic_comment, verify_frontend_roundtrip, verify_hostile_corpus_on_frontend,
};
use dendri_frontend_core::foreign::{
    PackChecksums, PackManifest, check_program_foreign_export, collect_foreign_sites,
    is_four_part_diagnostic,
};
use dendri_frontend_core::numeric_emit::contains_analog_full_scale_literal;
use dendri_frontend_core::{
    BoardContext, BoardProgramBundle, LanguageFrontend, LanguageId, Paradigm,
};
use dendri_lang_cpp::{
    CppFrontend, emit_codec_conversion_cpp, emit_fixed_adc_read_cpp, feature_matrix_entries,
    merge_arduino_tabs, parse_multi_file_sketch,
};
use dendri_span::{ErrorCode, Span};
use dendri_validate::{MemoryProfileCatalog, ValidateOptions, validate_project};
use proptest::prelude::*;
use std::path::PathBuf;

fn packs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("packs")
}

fn load_verified_arduino_packs() -> Result<Vec<PackManifest>, String> {
    let dir = packs_dir();
    let checksums_src = std::fs::read_to_string(dir.join("checksums.toml"))
        .map_err(|e| format!("assets/packs/checksums.toml must exist: {e}"))?;
    let checksums = PackChecksums::parse_str(&checksums_src)
        .map_err(|e| format!("valid checksums.toml: {e:?}"))?;

    let mut manifests = Vec::new();
    for pack_id in ["arduino-wire", "arduino-spi", "arduino-servo"] {
        let lock = checksums
            .lock_for_id(pack_id)
            .ok_or_else(|| format!("pack lock entry `{pack_id}` must exist"))?;
        let pack_path = dir.join(format!("{pack_id}.pack.toml"));
        let manifest = PackManifest::from_file_verified(&pack_path, lock)
            .map_err(|e| format!("hash-locked pack `{pack_id}` verify failed: {e:?}"))?;
        manifests.push(manifest);
    }
    Ok(manifests)
}

#[test]
fn frontend_identity_and_capabilities_match_lbc_section_8() {
    let fe = CppFrontend::new();
    assert_eq!(fe.id(), LanguageId::Cpp);
    assert_eq!(fe.paradigm(), Paradigm::Textual);

    let caps = fe.capabilities();
    assert_eq!(
        caps.supported_comment_styles,
        vec![
            CommentStyle::Line,
            CommentStyle::Block,
            CommentStyle::DocLine,
            CommentStyle::DocBlock,
        ]
    );
    assert!(!caps.supports_nested_paren_comments);
    assert!(caps.supports_line_comments);
    assert_eq!(caps.comment_encoding, CommentEncoding::Utf8Passthrough);
    assert_eq!(caps.capabilities, vec!["asl-core@1".to_owned()]);
    assert_eq!(caps.st_dialect, None);
}

#[test]
fn roundtrip_arduino_blink_and_pin_reconciliation() {
    let fe = CppFrontend::new();
    let ctx = BoardContext::arduino_uno_r3("mcu_uno");
    let src = r#"// Blink sketch with button input
#include <Arduino.h>

#define LED_PIN 13
const uint8_t btnPin = 2;
const int16_t BLINK_MS = 250;

void setup() {
  pinMode(LED_PIN, OUTPUT);
  pinMode(btnPin, INPUT_PULLUP);
}

void loop() {
  bool pressed = !digitalRead(btnPin);
  if (pressed) {
    digitalWrite(LED_PIN, HIGH);
    delay(BLINK_MS);
    digitalWrite(LED_PIN, LOW);
    delay(BLINK_MS);
  }
}
"#;
    let parsed = fe.parse(src, &ctx).expect("parse blink sketch");
    let prog = &parsed.value.program;

    assert_eq!(prog.imports, vec!["std.arduino.*".to_owned()]);
    assert_eq!(prog.hardware.len(), 2);
    assert_eq!(prog.hardware[0].label, "LED_PIN");
    assert_eq!(prog.hardware[0].pin, "D13");
    assert_eq!(prog.hardware[0].kind, HardwareKind::DigitalOut);
    assert_eq!(prog.hardware[1].label, "btnPin");
    assert_eq!(prog.hardware[1].pin, "D2");
    assert_eq!(prog.hardware[1].kind, HardwareKind::DigitalIn);
    assert_eq!(prog.hardware[1].pull, PullMode::PullUp);

    // Non-pin `#define` / `const` stays in `data`, pin constants are reconciled strictly into `hardware`
    assert_eq!(prog.data.len(), 1);
    assert_eq!(prog.data[0].name, "BLINK_MS");
    assert_eq!(prog.data[0].scope, StorageScope::Const);

    // `setup()` -> OB_STARTUP (pinMode consumed into hardware), `loop()` -> OB_MAIN
    assert_eq!(prog.pous.len(), 2);
    assert_eq!(prog.pous[0].name, "OB_STARTUP");
    assert_eq!(
        prog.pous[0].kind,
        PouKind::Ob {
            ob: ObKind::Startup
        }
    );
    assert_eq!(prog.pous[1].name, "OB_MAIN");
    assert_eq!(prog.pous[1].kind, PouKind::Ob { ob: ObKind::Main });

    // Full LOSSLESS_ASL + C1 + zero magic comments + zero M=10^9 round-trip!
    let reparsed = verify_frontend_roundtrip(&fe, &parsed.value, &ctx)
        .expect("LOSSLESS_ASL roundtrip must succeed");
    assert_c5_comment_fields(&reparsed.comments).expect("C5 comment fields");

    let emitted = fe.emit(&parsed.value, &ctx).expect("emit").value;
    assert!(!contains_banned_magic_comment(&emitted));
    assert!(emitted.contains("#include <Arduino.h>"));
    assert!(emitted.contains("pinMode(LED_PIN, OUTPUT);"));
    assert!(emitted.contains("pinMode(btnPin, INPUT_PULLUP);"));
    assert!(emitted.contains("digitalWrite(LED_PIN, HIGH);"));
    assert!(emitted.contains("delay(BLINK_MS);"));
}

#[test]
fn attach_interrupt_promotes_isr_and_forbids_delay() {
    let fe = CppFrontend::new();
    let ctx = BoardContext::arduino_uno_r3("mcu_uno");
    let src = r#"#include <Arduino.h>

const uint8_t encPin = 2;
int32_t pulseCount = 0;

void onPulse() {
  pulseCount++;
}

void setup() {
  pinMode(encPin, INPUT_PULLUP);
  attachInterrupt(digitalPinToInterrupt(encPin), onPulse, RISING);
}

void loop() {
  delay(100);
}
"#;
    let parsed = fe.parse(src, &ctx).expect("parse attachInterrupt sketch");
    let isr = parsed
        .value
        .program
        .pous
        .iter()
        .find(|p| p.name == "onPulse")
        .expect("ISR POU must exist");
    assert_eq!(
        isr.kind,
        PouKind::Ob {
            ob: ObKind::HwInt {
                pin_label: "encPin".to_owned(),
                edge: InterruptEdge::Rising,
                priority: 1,
            }
        }
    );
    verify_frontend_roundtrip(&fe, &parsed.value, &ctx).expect("ISR round-trip");

    // Calling `delay()` inside `OB_HW_INT` MUST fail with `ERR_ISR_FORBIDDEN_OP`!
    let bad_isr_src = r#"#include <Arduino.h>
void onPulse() {
  delay(5);
}
void setup() {
  pinMode(2, INPUT_PULLUP);
  attachInterrupt(digitalPinToInterrupt(2), onPulse, FALLING);
}
void loop() {}
"#;
    let errs = fe.parse(bad_isr_src, &ctx).unwrap_err();
    assert!(
        errs.iter().any(|d| d.code == ErrorCode::ErrIsrForbiddenOp),
        "expected ERR_ISR_FORBIDDEN_OP, got {errs:?}"
    );
}

#[test]
fn analog_read_write_resolution_and_time_intrinsics() {
    let fe = CppFrontend::new();
    let ctx = BoardContext::arduino_uno_r3("mcu_uno");
    let src = r#"#include <Arduino.h>

const uint8_t potPin = A0;
const uint8_t pwmPin = 5;
uint32_t lastTick = 0;

void setup() {
  pinMode(pwmPin, OUTPUT);
  analogReadResolution(10);
}

void loop() {
  uint32_t nowMs = millis();
  uint32_t nowUs = micros();
  if ((nowMs - lastTick) >= 10UL) {
    lastTick = nowMs;
    int16_t raw = analogRead(potPin);
    analogWrite(pwmPin, raw >> 2);
    delayMicroseconds(50);
  }
}
"#;
    let parsed = fe.parse(src, &ctx).expect("parse analog/time sketch");
    let prog = &parsed.value.program;

    // Verify hardware bindings: A0 -> AnalogIn (uno_10bit + 0..5V), D5 -> PwmOut (pwm_8bit)
    let pot_hw = prog.hardware.iter().find(|h| h.label == "potPin").unwrap();
    assert_eq!(pot_hw.pin, "A0");
    assert_eq!(pot_hw.kind, HardwareKind::AnalogIn);
    assert_eq!(pot_hw.codec.as_deref(), Some("uno_10bit"));
    assert!(pot_hw.range.is_some());

    let pwm_hw = prog.hardware.iter().find(|h| h.label == "pwmPin").unwrap();
    assert_eq!(pwm_hw.pin, "D5");
    assert_eq!(pwm_hw.kind, HardwareKind::PwmOut);
    assert_eq!(pwm_hw.codec.as_deref(), Some("pwm_8bit"));

    // Verify canonical `analogWrite` ASL chain: `PWM_WRITE(pwmPin, ANALOG_TO_DUTY(CODE_TO_ANALOG(..., "pwm_8bit")))`
    let main_pou = prog.pous.iter().find(|p| p.name == "OB_MAIN").unwrap();
    let PouBody::Statements(stmts) = &main_pou.body else {
        panic!("expected statements");
    };
    let if_stmt = stmts
        .iter()
        .find(|s| matches!(s.kind, StmtKind::If { .. }))
        .unwrap();
    let StmtKind::If { then_body, .. } = &if_stmt.kind else {
        panic!("expected if");
    };
    let pwm_call = then_body
        .iter()
        .find(|s| matches!(&s.kind, StmtKind::Call { callee, .. } if callee == "PWM_WRITE"))
        .expect("PWM_WRITE call expected");
    let StmtKind::Call { args, .. } = &pwm_call.kind else {
        panic!("expected Call");
    };
    assert_eq!(args.len(), 2);
    assert!(matches!(&args[0].value.kind, ExprKind::Ident(lbl) if lbl == "pwmPin"));
    let ExprKind::Call {
        callee: outer,
        args: outer_args,
    } = &args[1].value.kind
    else {
        panic!("expected ANALOG_TO_DUTY, got {:?}", args[1].value.kind);
    };
    assert_eq!(outer, "ANALOG_TO_DUTY");
    let ExprKind::Call {
        callee: mid,
        args: mid_args,
    } = &outer_args[0].value.kind
    else {
        panic!(
            "expected CODE_TO_ANALOG, got {:?}",
            outer_args[0].value.kind
        );
    };
    assert_eq!(mid, "CODE_TO_ANALOG");
    assert_eq!(
        mid_args[1].value.kind,
        ExprKind::Literal(Literal::String("pwm_8bit".to_owned()))
    );

    verify_frontend_roundtrip(&fe, &parsed.value, &ctx).expect("analog/time roundtrip");
}

#[test]
fn reduced_rational_cross_platform_adc_emission_never_emits_m() {
    let fe = CppFrontend::new();
    let ctx = BoardContext::arduino_uno_r3("mcu_uno");

    // 1. Direct helper checks
    let plc_27648 = Codec::new("plc_span_27648", CodecKind::Span, 27_648, 27_648).unwrap();
    let uno_10bit = Codec::new("uno_10bit", CodecKind::Binned, 1024, 1023).unwrap();
    let adc_12bit = Codec::new("adc_12bit", CodecKind::Binned, 4096, 4095).unwrap();

    let plc_to_uno = emit_codec_conversion_cpp(&plc_27648, &uno_10bit, "iw64").unwrap();
    assert!(!contains_analog_full_scale_literal(&plc_to_uno));
    assert!(plc_to_uno.contains("1023UL"));
    assert!(plc_to_uno.contains("27UL"));

    let c12_to_c10 = emit_codec_conversion_cpp(&adc_12bit, &uno_10bit, "raw").unwrap();
    assert!(!contains_analog_full_scale_literal(&c12_to_c10));
    assert_eq!(c12_to_c10, "(static_cast<uint32_t>(raw) >> 2u)");

    assert_eq!(
        emit_fixed_adc_read_cpp(10, 10, "A0").unwrap(),
        "analogRead(A0)"
    );
    let fixed_10_to_12 = emit_fixed_adc_read_cpp(10, 12, "A0").unwrap();
    assert!(!contains_analog_full_scale_literal(&fixed_10_to_12));
    assert!(fixed_10_to_12.contains("analogRead(A0)"));

    // 2. Full AST emit of `ANALOG_TO_CODE(CODE_TO_ANALOG(iw64, "plc_span_27648"), "uno_10bit")`
    let bundle = BoardProgramBundle::new(BoardProgram {
        imports: vec!["std.arduino.*".to_owned()],
        hardware: vec![],
        types: vec![],
        data: vec![
            DataDecl {
                id: NodeId::new("n_1"),
                name: "iw64".to_owned(),
                scope: StorageScope::Var,
                ty: TypeRef::scalar(ScalarType::Int),
                initial: Some(Expr::new(
                    ExprKind::Literal(Literal::Int(0)),
                    Span::default(),
                )),
                span: Span::default(),
            },
            DataDecl {
                id: NodeId::new("n_2"),
                name: "scaled".to_owned(),
                scope: StorageScope::Var,
                ty: TypeRef::scalar(ScalarType::Udint),
                initial: Some(Expr::new(
                    ExprKind::Literal(Literal::Int(0)),
                    Span::default(),
                )),
                span: Span::default(),
            },
        ],
        modules: vec![],
        pous: vec![Pou {
            id: NodeId::new("n_3"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: vec![],
            locals: vec![],
            body: PouBody::Statements(vec![Stmt::new(
                NodeId::new("n_4"),
                StmtKind::Assign {
                    target: LValue::Var {
                        name: "scaled".to_owned(),
                        span: Span::default(),
                    },
                    value: Expr::new(
                        ExprKind::Call {
                            callee: "ANALOG_TO_CODE".to_owned(),
                            args: vec![
                                dendri_ast::CallArg {
                                    name: None,
                                    value: Expr::new(
                                        ExprKind::Call {
                                            callee: "CODE_TO_ANALOG".to_owned(),
                                            args: vec![
                                                dendri_ast::CallArg {
                                                    name: None,
                                                    value: Expr::new(
                                                        ExprKind::Ident("iw64".to_owned()),
                                                        Span::default(),
                                                    ),
                                                    span: Span::default(),
                                                },
                                                dendri_ast::CallArg {
                                                    name: None,
                                                    value: Expr::new(
                                                        ExprKind::Literal(Literal::String(
                                                            "plc_span_27648".to_owned(),
                                                        )),
                                                        Span::default(),
                                                    ),
                                                    span: Span::default(),
                                                },
                                            ],
                                        },
                                        Span::default(),
                                    ),
                                    span: Span::default(),
                                },
                                dendri_ast::CallArg {
                                    name: None,
                                    value: Expr::new(
                                        ExprKind::Literal(Literal::String("uno_10bit".to_owned())),
                                        Span::default(),
                                    ),
                                    span: Span::default(),
                                },
                            ],
                        },
                        Span::default(),
                    ),
                },
                Span::default(),
            )]),
            span: Span::default(),
        }],
    });

    let emitted = fe.emit(&bundle, &ctx).expect("emit reduced rational").value;
    assert!(
        !contains_analog_full_scale_literal(&emitted),
        "M=10^9 leaked into C++: {emitted}"
    );
    assert!(emitted.contains("1023UL"));
    assert!(emitted.contains("27UL"));
}

#[test]
fn int_bits_16_avr_vs_32_target_mapping() {
    let fe = CppFrontend::new();
    let src = r#"enum Mode { IDLE = 0, RUN = 1 };

int signedVal = -10;
unsigned int unsignedVal = 42u;
double precision = 3.25;

int addOne(int x) {
  return x + 1;
}

void loop() {
  signedVal = addOne(signedVal);
}
"#;
    // 1. 16-bit AVR (`arduino-uno-r3`, int_bits = 16)
    let ctx16 = BoardContext::arduino_uno_r3("mcu_avr");
    let p16 = fe.parse(src, &ctx16).expect("parse 16-bit");
    assert_eq!(
        p16.value.program.data[0].ty,
        TypeRef::scalar(ScalarType::Int)
    );
    assert_eq!(
        p16.value.program.data[1].ty,
        TypeRef::scalar(ScalarType::Uint)
    );
    assert_eq!(
        p16.value.program.data[2].ty,
        TypeRef::scalar(ScalarType::Real)
    );
    assert!(matches!(
        &p16.value.program.types[0],
        TypeDecl::Enum {
            backing: ScalarType::Int,
            ..
        }
    ));
    assert_eq!(
        p16.value.program.pous[0].kind,
        PouKind::Fc {
            return_type: TypeRef::scalar(ScalarType::Int)
        }
    );
    verify_frontend_roundtrip(&fe, &p16.value, &ctx16).expect("16-bit roundtrip");

    // 2. 32-bit target (int_bits = 32)
    let ctx32 = BoardContext::new("mcu_32", "generic-mcu-32", 32);
    let p32 = fe.parse(src, &ctx32).expect("parse 32-bit");
    assert_eq!(
        p32.value.program.data[0].ty,
        TypeRef::scalar(ScalarType::Dint)
    );
    assert_eq!(
        p32.value.program.data[1].ty,
        TypeRef::scalar(ScalarType::Udint)
    );
    assert_eq!(
        p32.value.program.data[2].ty,
        TypeRef::scalar(ScalarType::Lreal)
    );
    assert!(matches!(
        &p32.value.program.types[0],
        TypeDecl::Enum {
            backing: ScalarType::Dint,
            ..
        }
    ));
    assert_eq!(
        p32.value.program.pous[0].kind,
        PouKind::Fc {
            return_type: TypeRef::scalar(ScalarType::Dint)
        }
    );
    verify_frontend_roundtrip(&fe, &p32.value, &ctx32).expect("32-bit roundtrip");
}

#[test]
fn roundtrip_functions_structs_enums_arrays_control_flow() {
    let fe = CppFrontend::new();
    let ctx = BoardContext::arduino_uno_r3("mcu_uno");
    let src = r#"#include <Arduino.h>

struct Sample {
  int16_t temp;
  uint8_t flags[4];
};

enum State : uint8_t {
  IDLE = 0,
  ACTIVE = 1,
  FAULT = 2
};

Sample current;
int16_t history[8];
uint8_t state = 0;

void bump(int16_t& target, int16_t delta) {
  target += delta;
}

int16_t compute(int16_t a, int16_t b) {
  int16_t acc = 0;
  int16_t i = 0;
  for (i = 0; i < 4; i++) {
    if (i == 2) {
      continue;
    }
    acc += (a & b) | (a ^ b);
  }
  while (acc > 100) {
    acc -= 10;
    if (acc < 50) {
      break;
    }
  }
  do {
    acc++;
  } while (acc < 5);

  int16_t out = (acc > 10) ? acc : 10;
  switch (state) {
    case 0:
      out += 1;
      break;
    case 1:
    case 2:
      out -= 1;
      break;
    default:
      out = 0;
      break;
  }
  return out;
}

void loop() {
  static int16_t counter = 0;
  bump(counter, 2);
  current.temp = compute(counter, 7);
  current.flags[0] = 0x0F;
  history[3] = current.temp;
}
"#;
    let parsed = fe.parse(src, &ctx).expect("parse rich C++ program");
    verify_frontend_roundtrip(&fe, &parsed.value, &ctx).expect("rich roundtrip");
}

#[test]
fn multi_tab_arduino_sketch_merge_and_hoisting() {
    let fe = CppFrontend::new();
    let ctx = BoardContext::arduino_uno_r3("mcu_uno");

    // Primary tab calls functions defined in secondary tabs `z_math.ino` and `a_io.ino`.
    let tabs = [
        (
            "blink.ino",
            r#"#include <Arduino.h>
#define LED_PIN 13

void setup() {
  pinMode(LED_PIN, OUTPUT);
}

void loop() {
  int16_t v = triple(4);
  driveLed(v > 10);
}
"#,
        ),
        (
            "z_math.ino",
            r#"int16_t triple(int16_t x) {
  return x * 3;
}
"#,
        ),
        (
            "a_io.ino",
            r#"void driveLed(bool on) {
  digitalWrite(LED_PIN, on);
}
"#,
        ),
    ];

    let merged = merge_arduino_tabs(&tabs);
    assert!(!contains_banned_magic_comment(&merged));
    // Verify canonical order: primary `blink.ino`, then `a_io.ino`, then `z_math.ino`
    let pos_loop = merged.find("void loop()").unwrap();
    let pos_drive = merged.find("void driveLed(bool on) {").unwrap();
    let pos_triple = merged.find("int16_t triple(int16_t x) {").unwrap();
    assert!(pos_loop < pos_drive);
    assert!(pos_drive < pos_triple);

    // Verify hoisted prototypes appear before `void setup()`
    let pos_setup = merged.find("void setup()").unwrap();
    let proto_drive = merged.find("void driveLed(bool on);").unwrap();
    let proto_triple = merged.find("int16_t triple(int16_t x);").unwrap();
    assert!(proto_drive < pos_setup);
    assert!(proto_triple < pos_setup);

    // Merged sketch parses and round-trips cleanly with zero duplicate POUs!
    let parsed = fe.parse(&merged, &ctx).expect("parse merged sketch");
    assert_eq!(parsed.value.program.pous.len(), 4);
    verify_frontend_roundtrip(&fe, &parsed.value, &ctx).expect("merged sketch roundtrip");

    // Also test `.h` / `.cpp` module pairing via `parse_multi_file_sketch`
    let multi_files = [
        ("app/app.ino", "#include <Arduino.h>\nvoid loop() {}\n"),
        ("motor_ctrl.h", "int16_t clampPwm(int16_t v);\n"),
        (
            "motor_ctrl.cpp",
            "int16_t clampPwm(int16_t v) {\n  return constrain(v, 0, 255);\n}\n",
        ),
    ];
    let multi = parse_multi_file_sketch(&multi_files, &ctx).expect("parse multi-file sketch");
    assert_eq!(multi.value.program.modules.len(), 1);
    assert_eq!(multi.value.program.modules[0].name, "motor_ctrl");
    assert_eq!(
        multi.value.program.modules[0].exports,
        vec!["clampPwm".to_owned()]
    );
    assert!(
        multi
            .value
            .presentation
            .origin_files
            .values()
            .any(|v| v == "motor_ctrl.cpp")
    );
}

#[test]
fn serial_utf8_and_hd44780_display_charset_enforcement() {
    let fe = CppFrontend::new();
    let ctx = BoardContext::arduino_uno_r3("mcu_uno");

    // Serial UTF-8 streams (emoji, accents) MUST NEVER fail with ERR_STRING_CHARSET (DEF-007)
    let serial_src = r#"#include <Arduino.h>

void setup() {
  Serial.begin(9600);
}

void loop() {
  if (Serial.available() > 0) {
    int16_t ch = Serial.read();
    Serial.print("Ação 🚀 24V ±5% ");
    Serial.println(ch);
  }
}
"#;
    let parsed = fe
        .parse(serial_src, &ctx)
        .expect("Serial UTF-8 must succeed");
    verify_frontend_roundtrip(&fe, &parsed.value, &ctx).expect("Serial roundtrip");

    // HD44780 display sink with emoji MUST raise `ERR_STRING_CHARSET`!
    let bad_lcd_src = r#"#include <Arduino.h>
void loop() {
  DISPLAY_PRINT("lcd_hd44780", "Temp 🚀");
}
"#;
    let errs = fe.parse(bad_lcd_src, &ctx).unwrap_err();
    assert!(
        errs.iter().any(|d| d.code == ErrorCode::ErrStringCharset),
        "expected ERR_STRING_CHARSET, got {errs:?}"
    );
}

#[test]
fn f1_pack_resolution_for_wire_spi_and_servo() {
    let fe = CppFrontend::new();
    let mut ctx = BoardContext::arduino_uno_r3("mcu_uno");
    ctx.loaded_packs = load_verified_arduino_packs().expect("verified arduino packs");

    let src = r#"#include <Arduino.h>
#include <Wire.h>
#include <SPI.h>
#include <Servo.h>

Servo arm;

void setup() {
  Wire.begin();
  SPI.begin();
  arm.attach(9);
}

void loop() {
  Wire.beginTransmission(0x68);
  Wire.write(0x00);
  Wire.endTransmission(true);
  uint8_t got = Wire.requestFrom(0x68, 2);
  int16_t b = Wire.read();
  SPI.beginTransaction(1000000UL, MSBFIRST, SPI_MODE0);
  uint8_t rx = SPI.transfer(0xAA);
  SPI.endTransaction();
  arm.write(90);
  arm.writeMicroseconds(1500);
  arm.detach();
}
"#;
    let parsed = fe
        .parse(src, &ctx)
        .expect("F1 Wire/SPI/Servo sketch must parse and typecheck cleanly");
    let sites = collect_foreign_sites(&parsed.value.program);
    // All pack headers, `Servo arm;`, and Wire/SPI/Servo statement calls must carry `F1` fidelity!
    let f1_stmt_sites: Vec<_> = sites
        .iter()
        .filter(|s| s.fidelity == ForeignFidelity::F1)
        .collect();
    assert!(
        f1_stmt_sites.len() >= 10,
        "expected >= 10 F1 sites from Wire/SPI/Servo packs, got {}",
        f1_stmt_sites.len()
    );
    assert!(
        f1_stmt_sites
            .iter()
            .any(|s| s.pack_hint == Some("arduino-wire"))
    );
    assert!(
        f1_stmt_sites
            .iter()
            .any(|s| s.pack_hint == Some("arduino-spi"))
    );
    assert!(
        f1_stmt_sites
            .iter()
            .any(|s| s.pack_hint == Some("arduino-servo"))
    );

    verify_frontend_roundtrip(&fe, &parsed.value, &ctx).expect("F1 pack sketch roundtrip");

    // Wrong arity on an F1 pack method MUST raise `ERR_ARITY_MISMATCH`!
    let bad_arity = r#"#include <Wire.h>
void loop() {
  Wire.beginTransmission(0x68, 1, 2);
}
"#;
    let errs = fe.parse(bad_arity, &ctx).unwrap_err();
    assert!(
        errs.iter().any(|d| d.code == ErrorCode::ErrArityMismatch),
        "expected ERR_ARITY_MISMATCH on Wire.beginTransmission, got {errs:?}"
    );
}

#[test]
fn three_level_foreign_preservation_and_export_gate() {
    let fe = CppFrontend::new();
    let ctx = BoardContext::arduino_uno_r3("mcu_uno");

    // All three Foreign levels in one sketch:
    // 1. `class` and `struct` with methods -> `Decl::Foreign(F0)`
    // 2. Direct AVR register `PORTB |= (1 << 5);` -> `Stmt::Foreign(F0)`
    // 3. Unknown vendor intrinsic `__builtin_avr_nops(4)` / `arm.read()` -> `Expr::Foreign(F0)`
    let src = r#"#include <Arduino.h>

class PidController {
public:
  float kp;
  float step(float err) { return kp * err; }
};

struct MotorHelper {
  int16_t speed;
  void stop() { speed = 0; }
};

void loop() {
  PORTB |= (1 << 5);
  int16_t reg = PINB;
  int16_t v = __builtin_avr_read_adc();
}
"#;
    let parsed = fe
        .parse(src, &ctx)
        .expect("three-level Foreign sketch must parse cleanly");
    let sites = collect_foreign_sites(&parsed.value.program);
    assert!(
        sites
            .iter()
            .any(|s| s.level == dendri_frontend_core::ForeignLevel::Decl
                && s.text.contains("class PidController"))
    );
    assert!(
        sites
            .iter()
            .any(|s| s.level == dendri_frontend_core::ForeignLevel::Decl
                && s.text.contains("struct MotorHelper"))
    );
    assert!(
        sites
            .iter()
            .any(|s| s.level == dendri_frontend_core::ForeignLevel::Stmt
                && s.text == "PORTB |= (1 << 5);")
    );
    assert!(
        sites
            .iter()
            .any(|s| s.level == dendri_frontend_core::ForeignLevel::Expr
                && s.text == "__builtin_avr_read_adc()")
    );

    // Same-language C++ round-trip is NEVER blocked!
    verify_frontend_roundtrip(&fe, &parsed.value, &ctx).expect("same-language Foreign roundtrip");

    // Cross-language export to ST MUST fail with 4-part `ERR_FOREIGN_EXPORT_BLOCKED`!
    let blocked =
        check_program_foreign_export(&parsed.value.program, LanguageId::St, &ctx).unwrap_err();
    assert!(!blocked.is_empty());
    for diag in &blocked {
        assert_eq!(diag.code, ErrorCode::ErrForeignExportBlocked);
        assert!(is_four_part_diagnostic(diag), "must be 4-part: {diag:?}");
    }

    // Also test emitting a foreign `"st"` node in `CppFrontend::emit`:
    let foreign_st_bundle = BoardProgramBundle::new(BoardProgram {
        imports: vec![],
        hardware: vec![],
        types: vec![],
        data: vec![],
        modules: vec![],
        pous: vec![Pou {
            id: NodeId::new("n_main"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: vec![],
            locals: vec![],
            body: PouBody::Statements(vec![Stmt::new(
                NodeId::new("n_st_f"),
                StmtKind::Foreign {
                    language: "st".to_owned(),
                    fidelity: ForeignFidelity::F0,
                    text: "VENDOR_FB(IN := TRUE);".to_owned(),
                    pack_hint: None,
                },
                Span::new(0, 22),
            )]),
            span: Span::default(),
        }],
    });
    // Without allow_foreign_export -> ErrForeignExportBlocked (4-part)
    let emit_blocked = fe.emit(&foreign_st_bundle, &ctx).unwrap_err();
    assert_eq!(emit_blocked[0].code, ErrorCode::ErrForeignExportBlocked);
    assert!(is_four_part_diagnostic(&emit_blocked[0]));

    // With allow_foreign_export = true -> demarcated "NOT TRANSLATED" comment block!
    let mut allow_ctx = ctx.clone();
    allow_ctx.allow_foreign_export = true;
    let emitted = fe
        .emit(&foreign_st_bundle, &allow_ctx)
        .expect("allow_foreign_export must succeed")
        .value;
    assert!(emitted.contains("NOT TRANSLATED"));
    assert!(emitted.contains("VENDOR_FB(IN := TRUE);"));
}

#[test]
fn hostile_comment_corpus_c1_through_c5_and_eol_conservation() {
    // 1. C3 hostile corpus across all three CommentEncoding policies!
    for encoding in [
        CommentEncoding::Utf8Passthrough,
        CommentEncoding::AsciiFold,
        CommentEncoding::WarnOnly,
    ] {
        let mut ctx = BoardContext::arduino_uno_r3("mcu_uno");
        ctx.comment_encoding = encoding;
        let fe = CppFrontend::with_comment_encoding(encoding);
        assert_eq!(
            verify_hostile_corpus_on_frontend(&fe, &ctx),
            Ok(()),
            "C3 hostile corpus failed under {encoding:?}"
        );
    }

    // 2. End-to-end CRLF + leading/trailing/inner/dangling/doc/emoji/NFD comment conservation!
    let fe = CppFrontend::new();
    let ctx = BoardContext::arduino_uno_r3("mcu_uno");
    let src_crlf = concat!(
        "//! File header doc comment 🚀\r\n",
        "\r\n",
        "/// Leading doc line 1\r\n",
        "/// Leading doc line 2\r\n",
        "int16_t x = 1; // trailing note café vs cafe\u{0301}\r\n",
        "\r\n",
        "void loop() {\r\n",
        "  // paragraph 1\r\n",
        "\r\n",
        "  // paragraph 2\r\n",
        "  x = x + /* inner gain */ 1;\r\n",
        "  // dangling comment at end of loop\r\n",
        "}\r\n"
    );

    let parsed = fe
        .parse(src_crlf, &ctx)
        .expect("parse CRLF commented sketch");
    assert_eq!(parsed.value.presentation.eol_style, EolStyle::Crlf);
    assert_c5_comment_fields(&parsed.value.comments).expect("C5 check");
    assert_c4_reimport_idempotence(&fe, &parsed.value.comments, &ctx).expect("C4 check");

    let emitted = fe.emit(&parsed.value, &ctx).expect("emit CRLF").value;
    assert!(emitted.contains("\r\n"), "CRLF EOL style must be restored");
    assert!(!contains_banned_magic_comment(&emitted));

    let reparsed = fe.parse(&emitted, &ctx).expect("reparse emitted");
    assert_lossless_asl(&parsed.value.program, &reparsed.value.program).expect("LOSSLESS_ASL");
    assert_c1_multiset_invariant(&parsed.value.comments, &reparsed.value.comments)
        .expect("C1 multiset conservation");

    // 3. C2 orphan preservation + cross-language ST nested comment & Inner relocation!
    let st_comments = vec![
        Comment {
            id: "c_nested".to_owned(),
            anchor: CommentAnchor::Node {
                id: NodeId::new("n_stmt"),
            },
            placement: AstCommentPlacement::Leading,
            token_ordinal: None,
            text: " outer (* inner *) outer ".to_owned(),
            origin: CommentOrigin {
                language: "st".to_owned(),
                style: CommentStyle::ParenBlock,
            },
            span: Span::new(0, 29),
        },
        Comment {
            id: "c_inner_st".to_owned(),
            anchor: CommentAnchor::Node {
                id: NodeId::new("n_stmt"),
            },
            placement: AstCommentPlacement::Inner,
            token_ordinal: Some(1),
            text: " path C:\\temp\\".to_owned(),
            origin: CommentOrigin {
                language: "st".to_owned(),
                style: CommentStyle::ParenBlock,
            },
            span: Span::new(30, 48),
        },
    ];
    assert_eq!(
        assert_c2_orphan_preservation(
            &st_comments,
            &NodeId::new("n_stmt"),
            &NodeId::new("n_pou"),
            Some("mcu_uno")
        ),
        Ok(2)
    );

    let bundle_with_st_comments = BoardProgramBundle {
        program: BoardProgram {
            imports: vec![],
            hardware: vec![],
            types: vec![],
            data: vec![DataDecl {
                id: NodeId::new("n_v"),
                name: "x".to_owned(),
                scope: StorageScope::Var,
                ty: TypeRef::scalar(ScalarType::Int),
                initial: Some(Expr::new(
                    ExprKind::Literal(Literal::Int(0)),
                    Span::default(),
                )),
                span: Span::default(),
            }],
            modules: vec![],
            pous: vec![Pou {
                id: NodeId::new("n_pou"),
                name: "OB_MAIN".to_owned(),
                kind: PouKind::Ob { ob: ObKind::Main },
                preferred_view: None,
                params: vec![],
                locals: vec![],
                body: PouBody::Statements(vec![Stmt::new(
                    NodeId::new("n_stmt"),
                    StmtKind::Assign {
                        target: LValue::Var {
                            name: "x".to_owned(),
                            span: Span::default(),
                        },
                        value: Expr::new(ExprKind::Literal(Literal::Int(1)), Span::default()),
                    },
                    Span::default(),
                )]),
                span: Span::default(),
            }],
        },
        comments: st_comments,
        assumptions: vec![],
        presentation: Default::default(),
    };
    let out = fe
        .emit(&bundle_with_st_comments, &ctx)
        .expect("emit with ST comments");
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::InfoCommentRelocated),
        "cross-language Inner comment must emit INFO_COMMENT_RELOCATED"
    );
    // Nested ST `(* ... *)` must emit as `//`, trailing `\` must emit as `/* ... */`!
    assert!(out.value.contains("// outer (* inner *) outer "));
    assert!(out.value.contains("/* path C:\\temp\\*/"));
    let re = fe.parse(&out.value, &ctx).expect("reparse hazard-avoided");
    assert_c1_multiset_invariant(&bundle_with_st_comments.comments, &re.value.comments)
        .expect("C1 preserved across hazard avoidance");
}

#[test]
fn negative_fixtures_cover_every_produced_error_code() {
    let fe = CppFrontend::new();
    let ctx = BoardContext::arduino_uno_r3("mcu_uno");

    let cases: &[(&str, &str, ErrorCode)] = &[
        (
            "chained_comparison",
            "void loop() { int16_t a = 1; int16_t b = 2; int16_t c = 3; bool r = a < b < c; }",
            ErrorCode::ErrTomlSyntax,
        ),
        (
            "unterminated_string",
            "const char* s = \"unterminated;\nvoid loop() {}",
            ErrorCode::ErrTomlSyntax,
        ),
        (
            "unterminated_block_comment",
            "/* never closed\nvoid loop() {}",
            ErrorCode::ErrCommentField,
        ),
        (
            "unknown_type",
            "NonExistentType x = 0;\nvoid loop() {}",
            ErrorCode::ErrUnknownType,
        ),
        (
            "mixed_sign_comparison",
            "void loop() { int16_t a = -1; uint16_t b = 1; bool r = (a < b); }",
            ErrorCode::ErrTypeMismatch,
        ),
        (
            "invalid_cast_string_to_int",
            "void loop() { const char* s = \"hi\"; int16_t x = static_cast<int16_t>(s); }",
            ErrorCode::ErrInvalidCast,
        ),
        (
            "array_bounds_const",
            "int16_t arr[4];\nvoid loop() { arr[4] = 1; }",
            ErrorCode::ErrArrayBoundsConst,
        ),
        (
            "unknown_field_access",
            "struct Pt { int16_t x; };\nPt p;\nvoid loop() { int16_t z = p.nope; }",
            ErrorCode::ErrUnknownFieldAccess,
        ),
        (
            "undeclared_symbol",
            "void loop() { int16_t x = missingVar + 1; }",
            ErrorCode::ErrUndeclaredSymbol,
        ),
        (
            "const_write",
            "const int16_t LIMIT = 10;\nvoid loop() { LIMIT = 20; }",
            ErrorCode::ErrConstWrite,
        ),
        (
            "hardware_label_write",
            "const uint8_t led = 13;\nvoid setup() { pinMode(led, OUTPUT); }\nvoid loop() { led = 1; }",
            ErrorCode::ErrHardwareLabelWrite,
        ),
        (
            "duplicate_decl",
            "int16_t x = 1;\nint16_t x = 2;\nvoid loop() {}",
            ErrorCode::ErrDuplicateDecl,
        ),
        (
            "name_collision_hw_vs_fn",
            "const uint8_t led = 13;\nvoid setup() { pinMode(led, OUTPUT); }\nvoid led() {}",
            ErrorCode::ErrNameCollision,
        ),
        (
            "unresolved_function",
            "void loop() { totallyUnknownFunc(1, 2); }",
            ErrorCode::ErrUnresolvedFunction,
        ),
        (
            "arity_mismatch",
            "int16_t add(int16_t a, int16_t b) { return a + b; }\nvoid loop() { int16_t r = add(1); }",
            ErrorCode::ErrArityMismatch,
        ),
        (
            "return_type_mismatch",
            "int16_t f() { return true; }\nvoid loop() {}",
            ErrorCode::ErrReturnTypeMismatch,
        ),
        (
            "break_outside_loop",
            "void loop() { break; }",
            ErrorCode::ErrBreakOutsideLoop,
        ),
        (
            "continue_outside_loop",
            "void loop() { continue; }",
            ErrorCode::ErrContinueOutsideLoop,
        ),
        (
            "isr_forbidden_delay",
            "void isr() { delayMicroseconds(10); }\nvoid setup() { attachInterrupt(digitalPinToInterrupt(2), isr, RISING); }\nvoid loop() {}",
            ErrorCode::ErrIsrForbiddenOp,
        ),
        (
            "unresolved_isr",
            "void setup() { attachInterrupt(digitalPinToInterrupt(2), noSuchIsr, RISING); }\nvoid loop() {}",
            ErrorCode::ErrUnresolvedIsr,
        ),
        (
            "unknown_port",
            "void setup() { pinMode(99, OUTPUT); }\nvoid loop() {}",
            ErrorCode::ErrUnknownPort,
        ),
        (
            "pin_capability_mismatch_pwm",
            "void loop() { analogWrite(4, 128); }",
            ErrorCode::ErrPinCapabilityMismatch,
        ),
        (
            "pin_capability_mismatch_analog",
            "void loop() { int16_t v = analogRead(4); }",
            ErrorCode::ErrPinCapabilityMismatch,
        ),
        (
            "pin_capability_mismatch_int",
            "void isr() {}\nvoid setup() { attachInterrupt(digitalPinToInterrupt(4), isr, RISING); }\nvoid loop() {}",
            ErrorCode::ErrPinCapabilityMismatch,
        ),
        (
            "hardware_pin_conflict",
            "const uint8_t ledA = 13;\nconst uint8_t ledB = 13;\nvoid setup() { pinMode(ledA, OUTPUT); pinMode(ledB, OUTPUT); }\nvoid loop() {}",
            ErrorCode::ErrHardwarePinConflict,
        ),
        (
            "codec_resolution_exceeds_scale",
            "void setup() { analogReadResolution(30); }\nvoid loop() {}",
            ErrorCode::ErrCodecResolutionExceedsScale,
        ),
        (
            "foreign_untyped_4_part",
            "void loop() { auto x = __builtin_mystery_sensor(); }",
            ErrorCode::ErrForeignUntyped,
        ),
        (
            "display_string_charset",
            "void loop() { DISPLAY_PRINT(\"hd44780_lcd\", \"Error 🚀\"); }",
            ErrorCode::ErrStringCharset,
        ),
    ];

    for &(name, src, expected_code) in cases {
        let errs = fe.parse(src, &ctx).unwrap_err();
        assert!(
            errs.iter().any(|d| d.code == expected_code),
            "fixture `{name}` expected {expected_code:?}, got: {errs:?}"
        );
        if expected_code == ErrorCode::ErrForeignUntyped {
            let d = errs
                .iter()
                .find(|d| d.code == ErrorCode::ErrForeignUntyped)
                .unwrap();
            assert!(is_four_part_diagnostic(d), "must be 4-part: {d:?}");
        }
    }

    // Recursion depth guard (`ERR_RECURSION_DEPTH`): unary prefix chain AND nested parens
    let deep_unary = format!("void loop() {{ int16_t x = {}1; }}", "- ".repeat(150));
    let deep_errs = fe.parse(&deep_unary, &ctx).unwrap_err();
    assert!(
        deep_errs
            .iter()
            .any(|d| d.code == ErrorCode::ErrRecursionDepth),
        "expected ERR_RECURSION_DEPTH on unary chain, got {deep_errs:?}"
    );
    let deep_parens = format!(
        "void loop() {{ int16_t x = {}1{}; }}",
        "(".repeat(150),
        ")".repeat(150)
    );
    let paren_errs = fe.parse(&deep_parens, &ctx).unwrap_err();
    assert!(
        paren_errs
            .iter()
            .any(|d| d.code == ErrorCode::ErrRecursionDepth),
        "expected ERR_RECURSION_DEPTH on nested parens, got {paren_errs:?}"
    );

    // Non-fatal warnings: `WARN_DUTY_CLAMPED` and `WARN_COMMENT_BIDI`
    let warn_src = "// bidi \u{202E}alert\nvoid loop() { analogWrite(5, 300); }\n";
    let warn_out = fe
        .parse(warn_src, &ctx)
        .expect("warnings must not fail parse");
    assert!(
        warn_out
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::WarnDutyClamped)
    );
    assert!(
        warn_out
            .diagnostics
            .iter()
            .any(|d| d.code == ErrorCode::WarnCommentBidi)
    );
}

#[test]
fn iec_fb_instances_and_cross_paradigm_projections() {
    let fe = CppFrontend::new();
    let ctx = BoardContext::arduino_uno_r3("mcu_uno");

    // 1. IEC stateful FB instance in Arduino C++
    let fb_src = r#"#include <Arduino.h>
#include <stdint.h>

TON delayTimer;
bool trigger = false;
bool done = false;

void loop() {
  delayTimer({.IN = trigger, .PT = 500});
  done = delayTimer.Q;
}
"#;
    let parsed = fe.parse(fb_src, &ctx).expect("parse IEC FB in C++");
    verify_frontend_roundtrip(&fe, &parsed.value, &ctx).expect("IEC FB roundtrip");

    // 2. Derived C++ emission from `PouBody::Network` (Ladder) and `PouBody::Chart` (GRAFCET)
    let cross_bundle = BoardProgramBundle::new(BoardProgram {
        imports: vec!["std.iec.*".to_owned()],
        hardware: vec![],
        types: vec![],
        data: vec![
            DataDecl {
                id: NodeId::new("n_start"),
                name: "startBtn".to_owned(),
                scope: StorageScope::Var,
                ty: TypeRef::scalar(ScalarType::Bool),
                initial: Some(Expr::new(
                    ExprKind::Literal(Literal::Bool(false)),
                    Span::default(),
                )),
                span: Span::default(),
            },
            DataDecl {
                id: NodeId::new("n_motor"),
                name: "motorOut".to_owned(),
                scope: StorageScope::Var,
                ty: TypeRef::scalar(ScalarType::Bool),
                initial: Some(Expr::new(
                    ExprKind::Literal(Literal::Bool(false)),
                    Span::default(),
                )),
                span: Span::default(),
            },
        ],
        modules: vec![],
        pous: vec![
            Pou {
                id: NodeId::new("n_lad"),
                name: "OB_MAIN".to_owned(),
                kind: PouKind::Ob { ob: ObKind::Main },
                preferred_view: Some(AuthoringLanguage::Ladder),
                params: vec![],
                locals: vec![],
                body: PouBody::Network(LadderNetwork {
                    rungs: vec![LadderRung {
                        id: NodeId::new("n_r0"),
                        order: 0,
                        cells: vec![
                            LadderCell {
                                id: NodeId::new("n_c0"),
                                row: 0,
                                col: 0,
                                kind: LadderCellKind::Contact {
                                    kind: LadderContactKind::NormallyOpen,
                                    target: LValue::Var {
                                        name: "startBtn".to_owned(),
                                        span: Span::default(),
                                    },
                                },
                            },
                            LadderCell {
                                id: NodeId::new("n_c1"),
                                row: 0,
                                col: 1,
                                kind: LadderCellKind::Coil {
                                    kind: LadderCoilKind::Normal,
                                    target: LValue::Var {
                                        name: "motorOut".to_owned(),
                                        span: Span::default(),
                                    },
                                },
                            },
                        ],
                    }],
                }),
                span: Span::default(),
            },
            Pou {
                id: NodeId::new("n_sfc"),
                name: "seqStep".to_owned(),
                kind: PouKind::Fc {
                    return_type: TypeRef::Void,
                },
                preferred_view: Some(AuthoringLanguage::Sfc),
                params: vec![],
                locals: vec![],
                body: PouBody::Chart(SfcChart {
                    steps: vec![SfcStep {
                        id: NodeId::new("n_s0"),
                        name: "S0".to_owned(),
                        initial: true,
                        actions: vec![SfcAction {
                            id: NodeId::new("n_a0"),
                            qualifier: ActionQualifier::N,
                            duration: None,
                            target: Some(LValue::Var {
                                name: "motorOut".to_owned(),
                                span: Span::default(),
                            }),
                            body: vec![],
                        }],
                        macro_chart: None,
                    }],
                    transitions: vec![SfcTransition {
                        id: NodeId::new("n_t0"),
                        name: "T0".to_owned(),
                        from_steps: vec!["S0".to_owned()],
                        to_steps: vec!["S1".to_owned()],
                        priority: 0,
                        receptivity: Expr::new(
                            ExprKind::Ident("startBtn".to_owned()),
                            Span::default(),
                        ),
                    }],
                    evolve_to_stability: false,
                }),
                span: Span::default(),
            },
        ],
    });
    let emitted = fe
        .emit(&cross_bundle, &ctx)
        .expect("derived Ladder/SFC projection")
        .value;
    assert!(!contains_banned_magic_comment(&emitted));
    assert!(emitted.contains("bool _rail = true;"));
    assert!(emitted.contains("_rail = _rail && startBtn;"));
    assert!(emitted.contains("motorOut = _rail;"));
    assert!(emitted.contains("if (S0.X)"));
}

#[test]
fn parsed_arduino_program_passes_dendri_validate() {
    let fe = CppFrontend::new();
    let ctx = BoardContext::arduino_uno_r3("mcu_uno");
    let src = r#"#include <Arduino.h>
#include <stdint.h>

const uint8_t led = 13;
const uint8_t btn = 2;
const uint8_t pot = A0;
const uint8_t pwm = 5;
volatile int16_t pulses = 0;

int16_t bump(int16_t v) {
  return v + 1;
}

void onBtn() {
  pulses = bump(pulses);
}

void setup() {
  pinMode(led, OUTPUT);
  pinMode(btn, INPUT_PULLUP);
  pinMode(pwm, OUTPUT);
  analogReadResolution(10);
  attachInterrupt(digitalPinToInterrupt(btn), onBtn, FALLING);
}

void loop() {
  bool pressed = !digitalRead(btn);
  uint32_t now = millis();
  if (pressed && now > 10UL) {
    digitalWrite(led, HIGH);
  } else {
    digitalWrite(led, LOW);
  }
  analogWrite(pwm, analogRead(pot));
  delay(20);
}
"#;
    let parsed = fe.parse(src, &ctx).expect("parse");

    let catalog = MemoryProfileCatalog::builtin_reference();

    let project = Project {
        meta: ProjectMeta {
            schema_version: "2.0".to_owned(),
            asl_version: "2.0".to_owned(),
            requires: vec!["asl-core@1".to_owned()],
            name: "uno_demo".to_owned(),
            author: String::new(),
            description: String::new(),
        },
        libraries: vec![],
        packs: vec![],
        comments: vec![],
        boards: vec![Board {
            id: "mcu_uno".to_owned(),
            profile: "arduino-uno-r3".to_owned(),
            authoring_language: AuthoringLanguage::Cpp,
            execution: ExecutionKind::Cooperative,
            scan_period_us: None,
            layout: BoardLayout::default(),
            program: parsed.value.program.clone(),
            comments: parsed.value.comments.clone(),
            assumptions: parsed.value.assumptions.clone(),
            presentation: parsed.value.presentation.clone(),
            span: Span::default(),
        }],
        components: vec![],
        wires: vec![],
    };

    let bag = validate_project(&project, &catalog, &ValidateOptions::default());
    let hard_errors: Vec<_> = bag
        .all()
        .iter()
        .filter(|d| d.severity == dendri_span::Severity::Error)
        .collect();
    assert!(
        hard_errors.is_empty(),
        "unexpected validator errors: {hard_errors:?}"
    );
}

#[test]
fn feature_matrix_generates_valid_markdown_with_verified_evidence() {
    let entries = feature_matrix_entries();
    assert!(entries.len() >= 15);
    let mut collector = FeatureMatrixCollector::new();
    for e in entries {
        assert_eq!(e.language, LanguageId::Cpp);
        assert!(!e.evidence_test.is_empty());
        collector.record(e);
    }
    let md = collector.to_markdown();
    assert!(md.contains("| Category | Feature | C++ | ST | Ladder | SFC | Evidence / Notes |"));
    assert!(md.contains("Foreign (F0)"));
    assert!(md.contains("Foreign (F1)"));
    assert!(md.contains("proptest_lossless_asl_roundtrip_16bit_and_32bit"));
}

// ---------------------------------------------------------------------------
// Property-based `LOSSLESS_ASL` Round-Trip Verification (16-bit AVR & 32-bit)
// ---------------------------------------------------------------------------

fn arb_ident() -> impl Strategy<Value = String> {
    prop::sample::select(vec![
        "alpha".to_owned(),
        "beta".to_owned(),
        "gamma".to_owned(),
        "delta".to_owned(),
        "count".to_owned(),
        "limit".to_owned(),
        "speed".to_owned(),
        "level".to_owned(),
    ])
}

fn arb_expr(var_names: Vec<String>, depth: u32) -> BoxedStrategy<Expr> {
    let vars = var_names.clone();
    let leaf = prop_oneof![
        (-500i128..=500i128)
            .prop_map(|n| Expr::new(ExprKind::Literal(Literal::Int(n)), Span::default())),
        prop::sample::select(vars).prop_map(|v| Expr::new(ExprKind::Ident(v), Span::default())),
    ]
    .boxed();

    if depth == 0 {
        return leaf;
    }

    let op_strat = prop::sample::select(vec![
        BinaryOp::Add,
        BinaryOp::Sub,
        BinaryOp::Mul,
        BinaryOp::BitAnd,
        BinaryOp::BitOr,
        BinaryOp::BitXor,
    ]);

    prop_oneof![
        leaf,
        (
            op_strat,
            arb_expr(var_names.clone(), depth - 1),
            arb_expr(var_names, depth - 1),
        )
            .prop_map(|(op, l, r)| Expr::new(
                ExprKind::Binary {
                    op,
                    left: Box::new(l),
                    right: Box::new(r),
                },
                Span::default(),
            )),
    ]
    .boxed()
}

fn arb_board_bundle(int_bits: u8) -> impl Strategy<Value = BoardProgramBundle> {
    let scalar = if int_bits <= 16 {
        ScalarType::Int
    } else {
        ScalarType::Dint
    };
    let comment_text = prop::sample::select(vec![
        " human note".to_owned(),
        " line one\n line two".to_owned(),
        " path C:\\temp\\".to_owned(),
        " contains */ hazard".to_owned(),
        " ação 🚀 500Ω".to_owned(),
    ]);
    let comment_style = prop::sample::select(vec![
        CommentStyle::Line,
        CommentStyle::Block,
        CommentStyle::DocLine,
    ]);

    (
        arb_ident(),
        -100i128..=100i128,
        1i128..=8i128,
        any::<bool>(),
        any::<bool>(),
        comment_text,
        comment_style,
    )
        .prop_flat_map(
            move |(var_name, init_val, for_bound, inclusive, use_crlf, ctext, cstyle)| {
                let v1 = format!("{var_name}_a");
                let v2 = format!("{var_name}_b");
                let expr_strat = arb_expr(vec![v1.clone(), v2.clone()], 2);
                let sc = scalar.clone();
                expr_strat.prop_map(move |rhs_expr| {
                    let program = BoardProgram {
                        imports: vec!["std.arduino.*".to_owned()],
                        hardware: vec![HardwareBinding {
                            id: NodeId::new("n_hw_1"),
                            label: "ledOut".to_owned(),
                            pin: "D13".to_owned(),
                            kind: HardwareKind::DigitalOut,
                            pull: PullMode::None,
                            active_low: false,
                            codec: None,
                            range: None,
                            span: Span::default(),
                        }],
                        types: vec![],
                        data: vec![
                            DataDecl {
                                id: NodeId::new("n_d1"),
                                name: v1.clone(),
                                scope: StorageScope::Var,
                                ty: TypeRef::scalar(sc.clone()),
                                initial: Some(Expr::new(
                                    ExprKind::Literal(Literal::Int(init_val)),
                                    Span::default(),
                                )),
                                span: Span::default(),
                            },
                            DataDecl {
                                id: NodeId::new("n_d2"),
                                name: v2.clone(),
                                scope: StorageScope::Var,
                                ty: TypeRef::scalar(sc.clone()),
                                initial: Some(Expr::new(
                                    ExprKind::Literal(Literal::Int(1)),
                                    Span::default(),
                                )),
                                span: Span::default(),
                            },
                        ],
                        modules: vec![],
                        pous: vec![
                            Pou {
                                id: NodeId::new("n_setup"),
                                name: "OB_STARTUP".to_owned(),
                                kind: PouKind::Ob {
                                    ob: ObKind::Startup,
                                },
                                preferred_view: None,
                                params: vec![],
                                locals: vec![],
                                body: PouBody::Statements(vec![]),
                                span: Span::default(),
                            },
                            Pou {
                                id: NodeId::new("n_helper"),
                                name: "scaleVal".to_owned(),
                                kind: PouKind::Fc {
                                    return_type: TypeRef::scalar(sc.clone()),
                                },
                                preferred_view: None,
                                params: vec![ParamDecl {
                                    name: "x".to_owned(),
                                    direction: ParamDirection::In,
                                    ty: TypeRef::scalar(sc.clone()),
                                }],
                                locals: vec![],
                                body: PouBody::Statements(vec![Stmt::new(
                                    NodeId::new("n_ret"),
                                    StmtKind::Return {
                                        value: Some(Expr::new(
                                            ExprKind::Binary {
                                                op: BinaryOp::Add,
                                                left: Box::new(Expr::new(
                                                    ExprKind::Ident("x".to_owned()),
                                                    Span::default(),
                                                )),
                                                right: Box::new(Expr::new(
                                                    ExprKind::Literal(Literal::Int(1)),
                                                    Span::default(),
                                                )),
                                            },
                                            Span::default(),
                                        )),
                                    },
                                    Span::default(),
                                )]),
                                span: Span::default(),
                            },
                            Pou {
                                id: NodeId::new("n_main"),
                                name: "OB_MAIN".to_owned(),
                                kind: PouKind::Ob { ob: ObKind::Main },
                                preferred_view: None,
                                params: vec![],
                                locals: vec![DataDecl {
                                    id: NodeId::new("n_idx"),
                                    name: "idx".to_owned(),
                                    scope: StorageScope::Temp,
                                    ty: TypeRef::scalar(sc.clone()),
                                    initial: None,
                                    span: Span::default(),
                                }],
                                body: PouBody::Statements(vec![
                                    Stmt::new(
                                        NodeId::new("n_s1"),
                                        StmtKind::Assign {
                                            target: LValue::Var {
                                                name: v1.clone(),
                                                span: Span::default(),
                                            },
                                            value: rhs_expr.clone(),
                                        },
                                        Span::default(),
                                    ),
                                    Stmt::new(
                                        NodeId::new("n_s2"),
                                        StmtKind::For {
                                            counter: "idx".to_owned(),
                                            from: Expr::new(
                                                ExprKind::Literal(Literal::Int(0)),
                                                Span::default(),
                                            ),
                                            to: Expr::new(
                                                ExprKind::Literal(Literal::Int(for_bound)),
                                                Span::default(),
                                            ),
                                            step: Expr::new(
                                                ExprKind::Literal(Literal::Int(1)),
                                                Span::default(),
                                            ),
                                            inclusive,
                                            body: vec![Stmt::new(
                                                NodeId::new("n_s3"),
                                                StmtKind::Assign {
                                                    target: LValue::Var {
                                                        name: v2.clone(),
                                                        span: Span::default(),
                                                    },
                                                    value: Expr::new(
                                                        ExprKind::Call {
                                                            callee: "scaleVal".to_owned(),
                                                            args: vec![dendri_ast::CallArg {
                                                                name: None,
                                                                value: Expr::new(
                                                                    ExprKind::Ident(
                                                                        "idx".to_owned(),
                                                                    ),
                                                                    Span::default(),
                                                                ),
                                                                span: Span::default(),
                                                            }],
                                                        },
                                                        Span::default(),
                                                    ),
                                                },
                                                Span::default(),
                                            )],
                                        },
                                        Span::default(),
                                    ),
                                    Stmt::new(
                                        NodeId::new("n_s4"),
                                        StmtKind::Call {
                                            callee: "DIGITAL_WRITE".to_owned(),
                                            args: vec![
                                                dendri_ast::CallArg {
                                                    name: None,
                                                    value: Expr::new(
                                                        ExprKind::Ident("ledOut".to_owned()),
                                                        Span::default(),
                                                    ),
                                                    span: Span::default(),
                                                },
                                                dendri_ast::CallArg {
                                                    name: None,
                                                    value: Expr::new(
                                                        ExprKind::Binary {
                                                            op: BinaryOp::Gt,
                                                            left: Box::new(Expr::new(
                                                                ExprKind::Ident(v1.clone()),
                                                                Span::default(),
                                                            )),
                                                            right: Box::new(Expr::new(
                                                                ExprKind::Literal(Literal::Int(0)),
                                                                Span::default(),
                                                            )),
                                                        },
                                                        Span::default(),
                                                    ),
                                                    span: Span::default(),
                                                },
                                            ],
                                        },
                                        Span::default(),
                                    ),
                                ]),
                                span: Span::default(),
                            },
                        ],
                    };
                    let mut bundle = BoardProgramBundle::new(program);
                    bundle.comments.push(Comment {
                        id: "c_1".to_owned(),
                        anchor: CommentAnchor::Node {
                            id: NodeId::new("n_s1"),
                        },
                        placement: AstCommentPlacement::Leading,
                        token_ordinal: None,
                        text: ctext.clone(),
                        origin: CommentOrigin {
                            language: "cpp".to_owned(),
                            style: cstyle,
                        },
                        span: Span::default(),
                    });
                    bundle.presentation.eol_style = if use_crlf {
                        EolStyle::Crlf
                    } else {
                        EolStyle::Lf
                    };
                    bundle
                })
            },
        )
}

#[test]
fn do_while_continue_evaluates_condition_and_preserves_nested_loop_continue() {
    let fe = CppFrontend::new();
    let ctx = BoardContext::arduino_uno_r3("mcu_uno");
    let src = r#"#include <Arduino.h>
#include <stdint.h>

int16_t x = 10;

void loop() {
  do {
    if (x >= 10) {
      continue;
    }
    for (int16_t i = 0; i < 3; i++) {
      if (i == 1) {
        continue;
      }
      x++;
    }
    x++;
  } while (false);
}
"#;
    let parsed = fe.parse(src, &ctx).expect("parse do-while with continue");
    let loop_pou = parsed
        .value
        .program
        .pous
        .iter()
        .find(|p| p.name == "OB_MAIN")
        .expect("OB_MAIN");
    let PouBody::Statements(stmts) = &loop_pou.body else {
        panic!("expected statement body");
    };
    let StmtKind::While {
        condition,
        body: while_body,
    } = &stmts[0].kind
    else {
        panic!("expected desugared While loop, got {:?}", stmts[0].kind);
    };
    assert_eq!(condition.kind, ExprKind::Literal(Literal::Bool(true)));

    // 1. Direct `continue;` inside outer `if (x >= 10)` MUST be preceded by `if (!false) { break; }`
    let StmtKind::If {
        then_body: outer_if_body,
        ..
    } = &while_body[0].kind
    else {
        panic!("expected outer if, got {:?}", while_body[0].kind);
    };
    assert_eq!(
        outer_if_body.len(),
        2,
        "continue must be preceded by condition break guard: {outer_if_body:?}"
    );
    let StmtKind::If {
        condition: guard_cond,
        then_body: guard_then,
        ..
    } = &outer_if_body[0].kind
    else {
        panic!(
            "expected break guard before continue, got {:?}",
            outer_if_body[0].kind
        );
    };
    assert!(matches!(
        &guard_cond.kind,
        ExprKind::Unary {
            op: UnaryOp::Not,
            operand
        } if operand.kind == ExprKind::Literal(Literal::Bool(false))
    ));
    assert!(matches!(guard_then[0].kind, StmtKind::Break));
    assert!(matches!(outer_if_body[1].kind, StmtKind::Continue));

    // 2. Nested `for` loop's `continue;` MUST NOT be rewritten with the outer do-while guard
    let StmtKind::For { body: for_body, .. } = &while_body[1].kind else {
        panic!("expected nested for loop, got {:?}", while_body[1].kind);
    };
    let StmtKind::If {
        then_body: inner_if_body,
        ..
    } = &for_body[0].kind
    else {
        panic!("expected inner if inside for loop");
    };
    assert_eq!(
        inner_if_body.len(),
        1,
        "nested loop continue must remain untouched: {inner_if_body:?}"
    );
    assert!(matches!(inner_if_body[0].kind, StmtKind::Continue));

    // Round-trip AND full static validation must pass with zero errors
    verify_frontend_roundtrip(&fe, &parsed.value, &ctx).expect("do-while roundtrip");

    let catalog = MemoryProfileCatalog::builtin_reference();
    let project = Project {
        meta: ProjectMeta {
            schema_version: "2.0".to_owned(),
            asl_version: "2.0".to_owned(),
            requires: vec!["asl-core@1".to_owned()],
            name: "do_while_demo".to_owned(),
            author: String::new(),
            description: String::new(),
        },
        libraries: vec![],
        packs: vec![],
        comments: vec![],
        boards: vec![Board {
            id: "mcu_uno".to_owned(),
            profile: "arduino-uno-r3".to_owned(),
            authoring_language: AuthoringLanguage::Cpp,
            execution: ExecutionKind::Cooperative,
            scan_period_us: None,
            layout: BoardLayout::default(),
            program: parsed.value.program.clone(),
            comments: parsed.value.comments.clone(),
            assumptions: parsed.value.assumptions.clone(),
            presentation: parsed.value.presentation.clone(),
            span: Span::default(),
        }],
        components: vec![],
        wires: vec![],
    };
    let bag = validate_project(&project, &catalog, &ValidateOptions::default());
    assert!(
        !bag.has_errors(),
        "unexpected validator errors: {:?}",
        bag.all()
    );
}

#[test]
fn switch_large_and_non_const_ranges_emit_if_else_chain_without_dropping_cases() {
    let fe = CppFrontend::new();
    let ctx = BoardContext::arduino_uno_r3("mcu_uno");

    let program = BoardProgram {
        imports: vec!["std.arduino.*".to_owned()],
        hardware: vec![],
        types: vec![],
        data: vec![
            DataDecl {
                id: NodeId::new("n_x"),
                name: "x".to_owned(),
                scope: StorageScope::Var,
                ty: TypeRef::scalar(ScalarType::Int),
                initial: Some(Expr::new(
                    ExprKind::Literal(Literal::Int(50)),
                    Span::default(),
                )),
                span: Span::default(),
            },
            DataDecl {
                id: NodeId::new("n_lo"),
                name: "lo".to_owned(),
                scope: StorageScope::Var,
                ty: TypeRef::scalar(ScalarType::Int),
                initial: Some(Expr::new(
                    ExprKind::Literal(Literal::Int(5)),
                    Span::default(),
                )),
                span: Span::default(),
            },
            DataDecl {
                id: NodeId::new("n_hi"),
                name: "hi".to_owned(),
                scope: StorageScope::Var,
                ty: TypeRef::scalar(ScalarType::Int),
                initial: Some(Expr::new(
                    ExprKind::Literal(Literal::Int(20)),
                    Span::default(),
                )),
                span: Span::default(),
            },
            DataDecl {
                id: NodeId::new("n_out"),
                name: "outVal".to_owned(),
                scope: StorageScope::Var,
                ty: TypeRef::scalar(ScalarType::Int),
                initial: Some(Expr::new(
                    ExprKind::Literal(Literal::Int(0)),
                    Span::default(),
                )),
                span: Span::default(),
            },
        ],
        modules: vec![],
        pous: vec![Pou {
            id: NodeId::new("n_main"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: None,
            params: vec![],
            locals: vec![],
            body: PouBody::Statements(vec![
                // Switch 1: range `0..=100` (> 64 values) + discrete `200` + default
                Stmt::new(
                    NodeId::new("n_sw1"),
                    StmtKind::Switch {
                        scrutinee: Expr::new(ExprKind::Ident("x".to_owned()), Span::default()),
                        cases: vec![
                            SwitchCase {
                                patterns: vec![CasePattern::Range {
                                    start: Expr::new(
                                        ExprKind::Literal(Literal::Int(0)),
                                        Span::default(),
                                    ),
                                    end: Expr::new(
                                        ExprKind::Literal(Literal::Int(100)),
                                        Span::default(),
                                    ),
                                }],
                                body: vec![Stmt::new(
                                    NodeId::new("n_a1"),
                                    StmtKind::Assign {
                                        target: LValue::Var {
                                            name: "outVal".to_owned(),
                                            span: Span::default(),
                                        },
                                        value: Expr::new(
                                            ExprKind::Literal(Literal::Int(1)),
                                            Span::default(),
                                        ),
                                    },
                                    Span::default(),
                                )],
                            },
                            SwitchCase {
                                patterns: vec![CasePattern::Value(Expr::new(
                                    ExprKind::Literal(Literal::Int(200)),
                                    Span::default(),
                                ))],
                                body: vec![Stmt::new(
                                    NodeId::new("n_a2"),
                                    StmtKind::Assign {
                                        target: LValue::Var {
                                            name: "outVal".to_owned(),
                                            span: Span::default(),
                                        },
                                        value: Expr::new(
                                            ExprKind::Literal(Literal::Int(2)),
                                            Span::default(),
                                        ),
                                    },
                                    Span::default(),
                                )],
                            },
                        ],
                        default: Some(vec![Stmt::new(
                            NodeId::new("n_a3"),
                            StmtKind::Assign {
                                target: LValue::Var {
                                    name: "outVal".to_owned(),
                                    span: Span::default(),
                                },
                                value: Expr::new(
                                    ExprKind::Literal(Literal::Int(3)),
                                    Span::default(),
                                ),
                            },
                            Span::default(),
                        )]),
                    },
                    Span::default(),
                ),
                // Switch 2: non-const range `lo..=hi` with compound scrutinee `(x + 1)`
                Stmt::new(
                    NodeId::new("n_sw2"),
                    StmtKind::Switch {
                        scrutinee: Expr::new(
                            ExprKind::Binary {
                                op: BinaryOp::Add,
                                left: Box::new(Expr::new(
                                    ExprKind::Ident("x".to_owned()),
                                    Span::default(),
                                )),
                                right: Box::new(Expr::new(
                                    ExprKind::Literal(Literal::Int(1)),
                                    Span::default(),
                                )),
                            },
                            Span::default(),
                        ),
                        cases: vec![SwitchCase {
                            patterns: vec![CasePattern::Range {
                                start: Expr::new(ExprKind::Ident("lo".to_owned()), Span::default()),
                                end: Expr::new(ExprKind::Ident("hi".to_owned()), Span::default()),
                            }],
                            body: vec![Stmt::new(
                                NodeId::new("n_a4"),
                                StmtKind::Assign {
                                    target: LValue::Var {
                                        name: "outVal".to_owned(),
                                        span: Span::default(),
                                    },
                                    value: Expr::new(
                                        ExprKind::Literal(Literal::Int(4)),
                                        Span::default(),
                                    ),
                                },
                                Span::default(),
                            )],
                        }],
                        default: None,
                    },
                    Span::default(),
                ),
            ]),
            span: Span::default(),
        }],
    };

    let bundle = BoardProgramBundle::new(program);
    let emitted = fe.emit(&bundle, &ctx).expect("emit switch ranges").value;
    assert!(!contains_banned_magic_comment(&emitted));
    assert!(
        !emitted.contains("case 0:"),
        "0..=100 must NOT degrade into single `case 0:`: {emitted}"
    );
    assert!(
        emitted.contains("if (((x >= 0) && (x <= 100)))"),
        "expected full inclusive range check 0..=100, got:\n{emitted}"
    );
    assert!(
        emitted.contains("else if ((x == 200))"),
        "expected discrete arm in else-if chain, got:\n{emitted}"
    );
    assert!(
        emitted.contains("else {"),
        "expected default else arm, got:\n{emitted}"
    );
    assert!(
        emitted.contains("auto _switch_val = (x + 1);"),
        "expected single-eval temporary for non-ident scrutinee, got:\n{emitted}"
    );
    assert!(
        emitted.contains("if (((_switch_val >= lo) && (_switch_val <= hi)))"),
        "expected non-const range bounds check, got:\n{emitted}"
    );

    // Emitted C++ must re-parse cleanly with zero errors
    let reparsed = fe
        .parse(&emitted, &ctx)
        .expect("emitted if/else chain must parse");
    assert!(!reparsed.value.program.pous.is_empty());
}

#[test]
fn ladder_projection_sorts_cells_and_materializes_rail_against_mid_rung_reset() {
    let fe = CppFrontend::new();
    let ctx = BoardContext::arduino_uno_r3("mcu_uno");

    // Deliberately supply cells out of `(col, row)` order for `[a] --(R a)-- (b)`
    let bundle = BoardProgramBundle::new(BoardProgram {
        imports: vec!["std.arduino.*".to_owned()],
        hardware: vec![],
        types: vec![],
        data: vec![
            DataDecl {
                id: NodeId::new("n_a"),
                name: "a".to_owned(),
                scope: StorageScope::Var,
                ty: TypeRef::scalar(ScalarType::Bool),
                initial: Some(Expr::new(
                    ExprKind::Literal(Literal::Bool(true)),
                    Span::default(),
                )),
                span: Span::default(),
            },
            DataDecl {
                id: NodeId::new("n_b"),
                name: "b".to_owned(),
                scope: StorageScope::Var,
                ty: TypeRef::scalar(ScalarType::Bool),
                initial: Some(Expr::new(
                    ExprKind::Literal(Literal::Bool(false)),
                    Span::default(),
                )),
                span: Span::default(),
            },
        ],
        modules: vec![],
        pous: vec![Pou {
            id: NodeId::new("n_main"),
            name: "OB_MAIN".to_owned(),
            kind: PouKind::Ob { ob: ObKind::Main },
            preferred_view: Some(AuthoringLanguage::Ladder),
            params: vec![],
            locals: vec![],
            body: PouBody::Network(LadderNetwork {
                rungs: vec![LadderRung {
                    id: NodeId::new("n_r0"),
                    order: 0,
                    cells: vec![
                        // col 2 first (unsorted!)
                        LadderCell {
                            id: NodeId::new("n_c2"),
                            row: 0,
                            col: 2,
                            kind: LadderCellKind::Coil {
                                kind: LadderCoilKind::Normal,
                                target: LValue::Var {
                                    name: "b".to_owned(),
                                    span: Span::default(),
                                },
                            },
                        },
                        // col 1 second
                        LadderCell {
                            id: NodeId::new("n_c1"),
                            row: 0,
                            col: 1,
                            kind: LadderCellKind::Coil {
                                kind: LadderCoilKind::Reset,
                                target: LValue::Var {
                                    name: "a".to_owned(),
                                    span: Span::default(),
                                },
                            },
                        },
                        // col 0 last
                        LadderCell {
                            id: NodeId::new("n_c0"),
                            row: 0,
                            col: 0,
                            kind: LadderCellKind::Contact {
                                kind: LadderContactKind::NormallyOpen,
                                target: LValue::Var {
                                    name: "a".to_owned(),
                                    span: Span::default(),
                                },
                            },
                        },
                    ],
                }],
            }),
            span: Span::default(),
        }],
    });

    let emitted = fe
        .emit(&bundle, &ctx)
        .expect("emit ladder projection")
        .value;
    assert!(!contains_banned_magic_comment(&emitted));

    let pos_decl = emitted
        .find("bool _rail = true;")
        .expect("must declare materialized local _rail");
    let pos_contact = emitted
        .find("_rail = _rail && a;")
        .expect("must update _rail from [a]");
    let pos_reset = emitted
        .find("if (_rail) { a = false; }")
        .expect("must reset a using materialized _rail");
    let pos_coil_b = emitted
        .find("b = _rail;")
        .expect("must assign b from pre-reset materialized _rail");

    assert!(
        pos_decl < pos_contact && pos_contact < pos_reset && pos_reset < pos_coil_b,
        "cells must execute in sorted (col, row) order with pre-reset _rail:\n{emitted}"
    );
    assert!(
        !emitted.contains("b = (a);"),
        "b must NOT re-read post-reset `a`:\n{emitted}"
    );

    // Projected C++ must also parse cleanly as valid C++
    let reparsed = fe
        .parse(&emitted, &ctx)
        .expect("projected Ladder C++ must parse");
    assert!(!reparsed.value.program.pous.is_empty());
}

proptest! {
    #[test]
    fn proptest_lossless_asl_roundtrip_16bit_and_32bit(
        b16 in arb_board_bundle(16),
        b32 in arb_board_bundle(32),
    ) {
        let fe = CppFrontend::new();
        let ctx16 = BoardContext::arduino_uno_r3("mcu_avr");
        let ctx32 = BoardContext::new("mcu_32", "generic-mcu-32", 32);

        let r16 = verify_frontend_roundtrip(&fe, &b16, &ctx16);
        prop_assert!(r16.is_ok(), "b16 failed: {:?}", r16.err());
        let r32 = verify_frontend_roundtrip(&fe, &b32, &ctx32);
        prop_assert!(r32.is_ok(), "b32 failed: {:?}", r32.err());
    }
}
