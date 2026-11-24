//! Canonical Wave 1 Golden Differential Scenarios (`05-ide-and-roadmap.md` §4 Gate 10, ADR-004).
//!
//! All scenarios are authored cleanly in dendriForge v2 (zero v1 fixture reuse, Principle P4),
//! emitted through the real Wave 1 frontends (`dendri-lang-cpp`, `dendri-lang-st`,
//! `dendri-lang-ladder`, `dendri-lang-sfc`), and include the complete `hostile_comment_corpus()`
//! (`C3`) to prove emitted comments compile and execute cleanly on real external toolchains.

use crate::toolchain::DiffError;
use dendri_ast::{
    AuthoringLanguage, Board, BoardLayout, Comment, CommentAnchor, CommentOrigin,
    CommentPlacement as AstCommentPlacement, ExecutionKind, PouBody,
};
use dendri_frontend_core::{
    BoardContext, BoardProgramBundle, LanguageFrontend, StCommentDialect,
    contains_analog_full_scale_literal, contains_banned_magic_comment, hostile_comment_corpus,
};
use dendri_lang_cpp::CppFrontend;
use dendri_lang_ladder::LadderFrontend;
use dendri_lang_sfc::{SfcFrontend, project_chart_state_decls, project_chart_to_statements};
use dendri_lang_st::StFrontend;
use dendri_span::Span;
use dendri_vm::{BoardVm, MemoryBoardIo, StepOutcome, Value, VmConfig};
use std::collections::BTreeMap;

/// Stimulus applied before a single C++ `loop()` iteration / `dendri-vm` step.
#[derive(Debug, Clone)]
pub struct CppStepStimulus {
    /// Digital pin inputs (`"D2" -> true/false`).
    pub digital_in: Vec<(&'static str, bool)>,
    /// Normalized fixed-point analog inputs (`"A0" -> 0..=1_000_000_000` ppb).
    pub analog_in: Vec<(&'static str, i32)>,
}

/// Complete differential scenario for C++ (`g++` host shim + `arduino-cli` core + `dendri-vm`).
#[derive(Debug, Clone)]
pub struct CppDiffScenario {
    pub name: &'static str,
    pub bundle: BoardProgramBundle,
    pub emitted_cpp: String,
    pub ctx: BoardContext,
    /// Pins sampled after each loop iteration (`"D13"`, `"D5"`, …).
    pub sample_digital_pins: Vec<&'static str>,
    /// PWM pins sampled after each loop iteration (`"D5"`, …) — returns 0..=255 8-bit duty code.
    pub sample_pwm_pins: Vec<&'static str>,
    /// Global scalar variable names sampled after each loop iteration.
    pub sample_globals: Vec<&'static str>,
    /// Whether to compare `Serial` UTF-8 output lines too.
    pub check_serial: bool,
    /// Ordered step stimuli executed identically on `dendri-vm` and the compiled C++ host binary.
    pub steps: Vec<CppStepStimulus>,
    /// Fixed cooperative step budget in microseconds per loop iteration (`dendri-vm`).
    pub vm_step_us: u64,
}

/// Type of a hardware-bound or plain global variable observed in a RuSTy ST differential scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StVarKind {
    /// `AT %IX*` / `AT %QX*` boolean pointer global in RuSTy (`uint8_t *`).
    IoBool,
    /// `AT %IW*` / `AT %QW*` 16-bit integer pointer global in RuSTy (`int16_t *`).
    IoInt16,
    /// Plain `VAR_GLOBAL` `BOOL` (`uint8_t`).
    GlobalBool,
    /// Plain `VAR_GLOBAL` `INT` (`int16_t`).
    GlobalInt16,
    /// Plain `VAR_GLOBAL` `DINT` (`int32_t`).
    GlobalInt32,
}

/// Descriptor for binding & sampling a RuSTy ST global or hardware tag.
#[derive(Debug, Clone)]
pub struct StVarBinding {
    /// Variable identifier as declared in ST (`StartBtn`, `MotorOut`, `acc`, …).
    pub st_name: &'static str,
    /// Board hardware pin ID for `dendri-vm` (`"Ia.0"`, `"Qa.0"`, `"IW64"`), or `None` for plain global.
    pub vm_pin: Option<&'static str>,
    pub kind: StVarKind,
}

/// Stimulus applied before a single PLC scan (`scan_dt_us` microseconds).
#[derive(Debug, Clone)]
pub struct StScanStimulus {
    pub bool_inputs: Vec<(&'static str, bool)>,
    pub int16_inputs: Vec<(&'static str, i16)>,
    /// Analog normalized fixed-point inputs (`"IW64" -> i32 ppb`) paired with `int16_inputs`.
    pub vm_analog_inputs: Vec<(&'static str, i32)>,
}

/// Complete differential scenario for RuSTy `plc` (LLVM native) vs `dendri-vm`.
///
/// Covers native IEC ST, structural Ladder (`PouBody::Network`) projected to ST,
/// structural GRAFCET/SFC (`PouBody::Chart`) projected to ST, and hostile comments (`C3`).
#[derive(Debug, Clone)]
pub struct StDiffScenario {
    pub name: &'static str,
    /// Canonical ASL bundle executed natively by `dendri-vm` (may hold `Network` or `Chart`!).
    pub vm_bundle: BoardProgramBundle,
    /// Authoring language tag for `Board` metadata.
    pub authoring_language: AuthoringLanguage,
    /// Emitted IEC 61131-3 ST source (`StCommentDialect::RuSTy`) compiled by RuSTy `plc`.
    pub emitted_st: String,
    pub ctx: BoardContext,
    /// Whether this scenario uses IEC standard FBs (`TON`/`CTU`/etc.) from `dendri_iec_runtime.st`.
    pub uses_iec_fbs: bool,
    /// Input & output hardware bindings (`AT %IX/%QX/%IW`) initialized to isolated PII/PIQ slots.
    pub io_bindings: Vec<StVarBinding>,
    /// Variables/outputs sampled after each PLC scan in deterministic order.
    pub outputs: Vec<StVarBinding>,
    /// Scan period in microseconds (e.g. `10_000` = 10 ms).
    pub scan_period_us: u64,
    /// Ordered scan stimuli.
    pub scans: Vec<StScanStimulus>,
}

/// Attaches the complete ADR-027 hostile comment corpus (`C3`) onto a bundle as board header/body
/// comments so every hostile sequence (`*/`, `*)`, trailing `\`, CRLF, combining chars, emoji,
/// bidi controls, very long lines) flows through the real emitter into the external compiler.
pub fn attach_hostile_corpus_comments(
    bundle: &mut BoardProgramBundle,
    board_id: &str,
    origin_language: &str,
) {
    let corpus = hostile_comment_corpus();
    for (idx, case) in corpus.into_iter().enumerate() {
        bundle.comments.push(Comment {
            id: format!("c_c3_hostile_{idx}"),
            anchor: CommentAnchor::Board {
                id: board_id.to_owned(),
            },
            placement: if idx % 2 == 0 {
                AstCommentPlacement::Header
            } else {
                AstCommentPlacement::Body
            },
            token_ordinal: None,
            text: case.text,
            origin: CommentOrigin {
                language: origin_language.to_owned(),
                style: case.style,
            },
            span: Span::default(),
        });
    }
}

/// Builds a [`VmConfig`] wired with every codec declared in `ctx`.
#[must_use]
pub fn vm_config_for(ctx: &BoardContext) -> VmConfig {
    let mut cfg = VmConfig::default();
    for (id, codec) in &ctx.codecs {
        cfg.codecs.insert(id.clone(), codec.clone());
    }
    cfg
}

/// Builds a [`Board`] for `arduino-uno-r3` cooperative execution.
#[must_use]
pub fn make_uno_board(id: &str, bundle: &BoardProgramBundle) -> Board {
    Board {
        id: id.to_owned(),
        profile: "arduino-uno-r3".to_owned(),
        authoring_language: AuthoringLanguage::Cpp,
        execution: ExecutionKind::Cooperative,
        scan_period_us: None,
        layout: BoardLayout::default(),
        program: bundle.program.clone(),
        comments: bundle.comments.clone(),
        assumptions: bundle.assumptions.clone(),
        presentation: bundle.presentation.clone(),
        span: Span::default(),
    }
}

/// Builds a [`Board`] for `plc-1200-open-1214` cyclic PLC scan execution.
#[must_use]
pub fn make_plc_board(
    id: &str,
    lang: AuthoringLanguage,
    scan_period_us: u64,
    bundle: &BoardProgramBundle,
) -> Board {
    Board {
        id: id.to_owned(),
        profile: "plc-1200-open-1214".to_owned(),
        authoring_language: lang,
        execution: ExecutionKind::PlcScan,
        scan_period_us: Some(scan_period_us),
        layout: BoardLayout::default(),
        program: bundle.program.clone(),
        comments: bundle.comments.clone(),
        assumptions: bundle.assumptions.clone(),
        presentation: bundle.presentation.clone(),
        span: Span::default(),
    }
}

/// Builds all canonical Wave 1 C++ differential scenarios (including derived Ladder→C++ and `C3`).
pub fn build_cpp_scenarios() -> Result<Vec<CppDiffScenario>, DiffError> {
    let cpp_fe = CppFrontend::new();
    let lad_fe = LadderFrontend::new();
    let mut scenarios = Vec::new();

    // 1. Motor Start/Stop Seal-In Latch
    {
        let name = "cpp_motor_latch";
        let ctx = BoardContext::arduino_uno_r3(name);
        let src = r#"#include <Arduino.h>

#define StartBtn 2
#define StopBtn 3
#define MotorOut 13

bool motorLatched = false;

void setup() {
  pinMode(StartBtn, INPUT);
  pinMode(StopBtn, INPUT);
  pinMode(MotorOut, OUTPUT);
}

void loop() {
  motorLatched = (digitalRead(StartBtn) || motorLatched) && !digitalRead(StopBtn);
  digitalWrite(MotorOut, motorLatched);
}
"#;
        let bundle = parse_cpp(&cpp_fe, src, &ctx, name)?;
        let emitted_cpp = emit_cpp_checked(&cpp_fe, &bundle, &ctx, name)?;
        scenarios.push(CppDiffScenario {
            name,
            bundle,
            emitted_cpp,
            ctx,
            sample_digital_pins: vec!["D13"],
            sample_pwm_pins: vec![],
            sample_globals: vec!["motorLatched"],
            check_serial: false,
            steps: vec![
                CppStepStimulus {
                    digital_in: vec![("D2", false), ("D3", false)],
                    analog_in: vec![],
                },
                CppStepStimulus {
                    digital_in: vec![("D2", true), ("D3", false)],
                    analog_in: vec![],
                },
                CppStepStimulus {
                    digital_in: vec![("D2", false), ("D3", false)],
                    analog_in: vec![],
                },
                CppStepStimulus {
                    digital_in: vec![("D2", false), ("D3", true)],
                    analog_in: vec![],
                },
                CppStepStimulus {
                    digital_in: vec![("D2", false), ("D3", false)],
                    analog_in: vec![],
                },
            ],
            vm_step_us: 10_000,
        });
    }

    // 2. Cooperative delay() + millis() blink state machine
    {
        let name = "cpp_blink_virtual_clock";
        let ctx = BoardContext::arduino_uno_r3(name);
        let src = r#"#include <Arduino.h>
#include <stdint.h>

#define LED_PIN 13

bool ledState = false;
int32_t toggles = 0;

void setup() {
  pinMode(LED_PIN, OUTPUT);
}

void loop() {
  ledState = !ledState;
  toggles = toggles + 1;
  digitalWrite(LED_PIN, ledState);
  delay(20);
}
"#;
        let bundle = parse_cpp(&cpp_fe, src, &ctx, name)?;
        let emitted_cpp = emit_cpp_checked(&cpp_fe, &bundle, &ctx, name)?;
        scenarios.push(CppDiffScenario {
            name,
            bundle,
            emitted_cpp,
            ctx,
            sample_digital_pins: vec!["D13"],
            sample_pwm_pins: vec![],
            sample_globals: vec!["ledState", "toggles"],
            check_serial: false,
            steps: (0..6)
                .map(|_| CppStepStimulus {
                    digital_in: vec![],
                    analog_in: vec![],
                })
                .collect(),
            vm_step_us: 20_000,
        });
    }

    // 3. Control flow (for, while, do-while continue, switch, helper FC) + UTF-8 Serial
    {
        let name = "cpp_control_flow_and_serial_utf8";
        let ctx = BoardContext::arduino_uno_r3(name);
        let src = r#"#include <Arduino.h>
#include <stdint.h>

#define STATUS_LED 13

int32_t total = 0;
int32_t cycle = 0;

int32_t classify(int32_t x) {
  int32_t acc = 0;
  for (int32_t i = 0; i < 4; i++) {
    acc = acc + i;
  }
  int32_t k = 0;
  do {
    k = k + 1;
    if (k == 2) {
      continue;
    }
    acc = acc + k;
  } while (k < 3);

  switch (x) {
    case 0:
      return acc;
    case 1:
      return acc + 10;
    default:
      return acc + 100;
  }
}

void setup() {
  pinMode(STATUS_LED, OUTPUT);
  Serial.begin(9600);
}

void loop() {
  total = classify(cycle);
  if (total > 15) {
    digitalWrite(STATUS_LED, HIGH);
    Serial.println("HIGH cafe");
  } else {
    digitalWrite(STATUS_LED, LOW);
    Serial.println("LOW");
  }
  cycle = cycle + 1;
}
"#;
        let bundle = parse_cpp(&cpp_fe, src, &ctx, name)?;
        let emitted_cpp = emit_cpp_checked(&cpp_fe, &bundle, &ctx, name)?;
        scenarios.push(CppDiffScenario {
            name,
            bundle,
            emitted_cpp,
            ctx,
            sample_digital_pins: vec!["D13"],
            sample_pwm_pins: vec![],
            sample_globals: vec!["total", "cycle"],
            check_serial: true,
            steps: (0..4)
                .map(|_| CppStepStimulus {
                    digital_in: vec![],
                    analog_in: vec![],
                })
                .collect(),
            vm_step_us: 10_000,
        });
    }

    // 4. Analog Read (10-bit binned T-NUM-5) -> PWM Write (8-bit binned clamp)
    {
        let name = "cpp_adc_pwm_scaling";
        let ctx = BoardContext::arduino_uno_r3(name);
        let src = r#"#include <Arduino.h>
#include <stdint.h>

#define POT_PIN A0
#define PWM_PIN 5

void setup() {
  pinMode(PWM_PIN, OUTPUT);
}

void loop() {
  analogWrite(PWM_PIN, analogRead(POT_PIN));
}
"#;
        let bundle = parse_cpp(&cpp_fe, src, &ctx, name)?;
        let emitted_cpp = emit_cpp_checked(&cpp_fe, &bundle, &ctx, name)?;
        scenarios.push(CppDiffScenario {
            name,
            bundle,
            emitted_cpp,
            ctx,
            sample_digital_pins: vec![],
            sample_pwm_pins: vec!["D5"],
            sample_globals: vec![],
            check_serial: false,
            steps: vec![
                CppStepStimulus {
                    digital_in: vec![],
                    analog_in: vec![("A0", 0)],
                },
                CppStepStimulus {
                    digital_in: vec![],
                    analog_in: vec![("A0", 100_000_000)],
                },
                CppStepStimulus {
                    digital_in: vec![],
                    analog_in: vec![("A0", 200_000_000)],
                },
                CppStepStimulus {
                    digital_in: vec![],
                    analog_in: vec![("A0", 1_000_000_000)],
                },
            ],
            vm_step_us: 10_000,
        });
    }

    // 5. Structural Ladder (`PouBody::Network`) projected to Arduino C++
    {
        let name = "cpp_derived_from_ladder";
        let plc_ctx = BoardContext::plc_1200_open_1214(name);
        let uno_ctx = BoardContext::arduino_uno_r3(name);
        let ladder_src = r#"VAR_GLOBAL
    StartBtn AT %IX0.0 : BOOL;
    StopBtn AT %IX0.1 : BOOL;
    MotorOut AT %QX0.0 : BOOL;
END_VAR

NETWORK OB_MAIN
    RUNG 0 (n_r0): BRANCH @(0, 0, n_br) { | | StartBtn @(0, 0, n_c_start) || | | MotorOut @(1, 0, n_c_seal) } -- |/| StopBtn @(0, 1, n_c_stop) -- ( ) MotorOut @(0, 2, n_coil_motor);
END_NETWORK
"#;
        let mut lad_bundle = lad_fe
            .parse_and_validate(ladder_src, &plc_ctx)
            .map_err(|diags| DiffError::FrontendFailed {
                scenario: name.to_owned(),
                details: format!("{diags:#?}"),
            })?
            .value;
        // Rebind pins to Arduino Uno R3 ports (`D2`, `D3`, `D13`)
        for hw in &mut lad_bundle.program.hardware {
            match hw.label.as_str() {
                "StartBtn" => hw.pin = "D2".to_owned(),
                "StopBtn" => hw.pin = "D3".to_owned(),
                "MotorOut" => hw.pin = "D13".to_owned(),
                _ => {}
            }
        }
        let emitted_cpp = emit_cpp_checked(&cpp_fe, &lad_bundle, &uno_ctx, name)?;
        scenarios.push(CppDiffScenario {
            name,
            bundle: lad_bundle,
            emitted_cpp,
            ctx: uno_ctx,
            sample_digital_pins: vec!["D13"],
            sample_pwm_pins: vec![],
            sample_globals: vec![],
            check_serial: false,
            steps: vec![
                CppStepStimulus {
                    digital_in: vec![("D2", false), ("D3", false)],
                    analog_in: vec![],
                },
                CppStepStimulus {
                    digital_in: vec![("D2", true), ("D3", false)],
                    analog_in: vec![],
                },
                CppStepStimulus {
                    digital_in: vec![("D2", false), ("D3", false)],
                    analog_in: vec![],
                },
                CppStepStimulus {
                    digital_in: vec![("D2", false), ("D3", true)],
                    analog_in: vec![],
                },
                CppStepStimulus {
                    digital_in: vec![("D2", false), ("D3", false)],
                    analog_in: vec![],
                },
            ],
            vm_step_us: 10_000,
        });
    }

    // 6. Full Hostile Comment Corpus (`C3`) on C++ (`-Wall -Wextra -Werror` + `arduino-cli`!)
    {
        let name = "cpp_hostile_comments_c3";
        let ctx = BoardContext::arduino_uno_r3(name);
        let src = r#"#include <Arduino.h>
#include <stdint.h>

#define BTN_PIN 2
#define LED_PIN 13

int32_t hits = 0;

void setup() {
  pinMode(BTN_PIN, INPUT);
  pinMode(LED_PIN, OUTPUT);
}

void loop() {
  if (digitalRead(BTN_PIN)) {
    hits = hits + 1;
    digitalWrite(LED_PIN, HIGH);
  } else {
    digitalWrite(LED_PIN, LOW);
  }
}
"#;
        let mut bundle = parse_cpp(&cpp_fe, src, &ctx, name)?;
        attach_hostile_corpus_comments(&mut bundle, name, "cpp");
        let emitted_cpp = emit_cpp_checked(&cpp_fe, &bundle, &ctx, name)?;
        scenarios.push(CppDiffScenario {
            name,
            bundle,
            emitted_cpp,
            ctx,
            sample_digital_pins: vec!["D13"],
            sample_pwm_pins: vec![],
            sample_globals: vec!["hits"],
            check_serial: false,
            steps: vec![
                CppStepStimulus {
                    digital_in: vec![("D2", false)],
                    analog_in: vec![],
                },
                CppStepStimulus {
                    digital_in: vec![("D2", true)],
                    analog_in: vec![],
                },
                CppStepStimulus {
                    digital_in: vec![("D2", true)],
                    analog_in: vec![],
                },
                CppStepStimulus {
                    digital_in: vec![("D2", false)],
                    analog_in: vec![],
                },
            ],
            vm_step_us: 10_000,
        });
    }

    Ok(scenarios)
}

/// Builds all canonical Wave 1 IEC ST / Ladder / GRAFCET scenarios compiled natively via RuSTy (`plc`).
pub fn build_st_rusty_scenarios() -> Result<Vec<StDiffScenario>, DiffError> {
    let st_fe = StFrontend::new();
    let lad_fe = LadderFrontend::new();
    let sfc_fe = SfcFrontend::new();
    let mut scenarios = Vec::new();

    // 1. Native IEC 61131-3 ST Motor Start/Stop Seal-In Latch
    {
        let name = "st_motor_latch";
        let ctx = rusty_plc_ctx(name);
        let src = r#"VAR_GLOBAL
    StartBtn AT %IX0.0 : BOOL;
    StopBtn AT %IX0.1 : BOOL;
    MotorOut AT %QX0.0 : BOOL;
END_VAR

PROGRAM OB_MAIN
    MotorOut := (StartBtn OR MotorOut) AND NOT StopBtn;
END_PROGRAM
"#;
        let bundle = parse_st(&st_fe, src, &ctx, name)?;
        let emitted_st = emit_st_checked(&st_fe, &bundle, &ctx, name)?;
        scenarios.push(StDiffScenario {
            name,
            vm_bundle: bundle,
            authoring_language: AuthoringLanguage::St,
            emitted_st,
            ctx,
            uses_iec_fbs: false,
            io_bindings: vec![
                StVarBinding {
                    st_name: "StartBtn",
                    vm_pin: Some("Ia.0"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "StopBtn",
                    vm_pin: Some("Ia.1"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "MotorOut",
                    vm_pin: Some("Qa.0"),
                    kind: StVarKind::IoBool,
                },
            ],
            outputs: vec![StVarBinding {
                st_name: "MotorOut",
                vm_pin: Some("Qa.0"),
                kind: StVarKind::IoBool,
            }],
            scan_period_us: 10_000,
            scans: vec![
                StScanStimulus {
                    bool_inputs: vec![("StartBtn", false), ("StopBtn", false)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                StScanStimulus {
                    bool_inputs: vec![("StartBtn", true), ("StopBtn", false)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                StScanStimulus {
                    bool_inputs: vec![("StartBtn", false), ("StopBtn", false)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                StScanStimulus {
                    bool_inputs: vec![("StartBtn", false), ("StopBtn", true)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                StScanStimulus {
                    bool_inputs: vec![("StartBtn", false), ("StopBtn", false)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
            ],
        });
    }

    // 2. IEC 61131-3 TON (20 ms) + CTU (PV=2) + Comparison
    {
        let name = "st_timer_ton_counter_ctu";
        let ctx = rusty_plc_ctx(name);
        let src = r#"VAR_GLOBAL
    EnableIn AT %IX0.0 : BOOL;
    ResetIn AT %IX0.1 : BOOL;
    TimerDone AT %QX0.0 : BOOL;
    BatchReady AT %QX0.1 : BOOL;
    tonDelay : TON;
    batchCnt : CTU;
    cntVal : DINT := 0;
END_VAR

PROGRAM OB_MAIN
    tonDelay(IN := EnableIn, PT := T#20ms);
    TimerDone := tonDelay.Q;
    batchCnt(CU := tonDelay.Q, R := ResetIn, PV := 2);
    cntVal := batchCnt.CV;
    BatchReady := batchCnt.Q AND (cntVal >= 2);
END_PROGRAM
"#;
        let bundle = parse_st(&st_fe, src, &ctx, name)?;
        let emitted_st = emit_st_checked(&st_fe, &bundle, &ctx, name)?;
        scenarios.push(StDiffScenario {
            name,
            vm_bundle: bundle,
            authoring_language: AuthoringLanguage::St,
            emitted_st,
            ctx,
            uses_iec_fbs: true,
            io_bindings: vec![
                StVarBinding {
                    st_name: "EnableIn",
                    vm_pin: Some("Ia.0"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "ResetIn",
                    vm_pin: Some("Ia.1"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "TimerDone",
                    vm_pin: Some("Qa.0"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "BatchReady",
                    vm_pin: Some("Qa.1"),
                    kind: StVarKind::IoBool,
                },
            ],
            outputs: vec![
                StVarBinding {
                    st_name: "TimerDone",
                    vm_pin: Some("Qa.0"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "BatchReady",
                    vm_pin: Some("Qa.1"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "cntVal",
                    vm_pin: None,
                    kind: StVarKind::GlobalInt32,
                },
            ],
            scan_period_us: 10_000,
            scans: vec![
                // Scan 0 (t=0): Enable=false -> Q=false, CV=0
                StScanStimulus {
                    bool_inputs: vec![("EnableIn", false), ("ResetIn", false)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                // Scan 1 (t=10ms): Enable=true -> TON starts (ET=0)
                StScanStimulus {
                    bool_inputs: vec![("EnableIn", true), ("ResetIn", false)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                // Scan 2 (t=20ms): ET=10ms < 20ms -> Q=false
                StScanStimulus {
                    bool_inputs: vec![("EnableIn", true), ("ResetIn", false)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                // Scan 3 (t=30ms): ET=20ms >= 20ms -> Q=true, CTU rising edge -> CV=1
                StScanStimulus {
                    bool_inputs: vec![("EnableIn", true), ("ResetIn", false)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                // Scan 4 (t=40ms): Drop Enable -> Q=false, CV stays 1
                StScanStimulus {
                    bool_inputs: vec![("EnableIn", false), ("ResetIn", false)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                // Scan 5..7: Re-arm TON for 20ms -> second rising edge -> CV=2, BatchReady=true
                StScanStimulus {
                    bool_inputs: vec![("EnableIn", true), ("ResetIn", false)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                StScanStimulus {
                    bool_inputs: vec![("EnableIn", true), ("ResetIn", false)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                StScanStimulus {
                    bool_inputs: vec![("EnableIn", true), ("ResetIn", false)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                // Scan 8: Pulse Reset -> CV=0, BatchReady=false
                StScanStimulus {
                    bool_inputs: vec![("EnableIn", false), ("ResetIn", true)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
            ],
        });
    }

    // 3. Control flow (FUNCTION with non-tail return, CASE..OF, FOR..BY, REPEAT..UNTIL + CONTINUE, MOD, negative literals)
    {
        let name = "st_control_flow_functions_math";
        let ctx = rusty_plc_ctx(name);
        let src = r#"VAR_GLOBAL
    StepIn AT %IX0.0 : BOOL;
    HighFlag AT %QX0.0 : BOOL;
    scanIdx : DINT := 0;
    resultVal : DINT := 0;
END_VAR

FUNCTION ComputeScore : DINT
VAR_INPUT
    mode : DINT;
END_VAR
VAR_TEMP
    i : DINT := 0;
    acc : DINT := 0;
END_VAR
    ComputeScore := DINT#-1;
    FOR i := 1 TO 6 BY 2 DO
        acc := acc + i;
    END_FOR;

    i := 0;
    REPEAT
        i := i + 1;
        IF (i MOD 2) = 0 THEN
            CONTINUE;
        END_IF;
        acc := acc + i;
    UNTIL i >= 4
    END_REPEAT;

    CASE mode OF
        0:
            ComputeScore := acc;
        1..2:
            ComputeScore := acc * 2;
        ELSE
            ComputeScore := acc + 100;
    END_CASE;
END_FUNCTION

PROGRAM OB_MAIN
    resultVal := ComputeScore(scanIdx);
    HighFlag := StepIn AND (resultVal >= 20);
    scanIdx := scanIdx + 1;
END_PROGRAM
"#;
        let bundle = parse_st(&st_fe, src, &ctx, name)?;
        let emitted_st = emit_st_checked(&st_fe, &bundle, &ctx, name)?;
        scenarios.push(StDiffScenario {
            name,
            vm_bundle: bundle,
            authoring_language: AuthoringLanguage::St,
            emitted_st,
            ctx,
            uses_iec_fbs: false,
            io_bindings: vec![
                StVarBinding {
                    st_name: "StepIn",
                    vm_pin: Some("Ia.0"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "HighFlag",
                    vm_pin: Some("Qa.0"),
                    kind: StVarKind::IoBool,
                },
            ],
            outputs: vec![
                StVarBinding {
                    st_name: "HighFlag",
                    vm_pin: Some("Qa.0"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "resultVal",
                    vm_pin: None,
                    kind: StVarKind::GlobalInt32,
                },
                StVarBinding {
                    st_name: "scanIdx",
                    vm_pin: None,
                    kind: StVarKind::GlobalInt32,
                },
            ],
            scan_period_us: 10_000,
            scans: vec![
                StScanStimulus {
                    bool_inputs: vec![("StepIn", true)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                StScanStimulus {
                    bool_inputs: vec![("StepIn", true)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                StScanStimulus {
                    bool_inputs: vec![("StepIn", false)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                StScanStimulus {
                    bool_inputs: vec![("StepIn", true)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
            ],
        });
    }

    // 4. Structural Ladder (`PouBody::Network`) -> derived ST -> RuSTy LLVM vs native VM `Network`!
    // Exercises seal-in latch + mid-rung reset immunity (`| | a -- (R) a -- ( ) b`) + TON + Compare!
    {
        let name = "ladder_projected_to_st_rusty";
        let ctx = rusty_plc_ctx(name);
        let ladder_src = r#"VAR_GLOBAL
    StartBtn AT %IX0.0 : BOOL;
    StopBtn AT %IX0.1 : BOOL;
    ResetTrigger AT %IX0.2 : BOOL;
    MotorOut AT %QX0.0 : BOOL;
    MidRungOut AT %QX0.1 : BOOL;
    DelayLamp AT %QX0.2 : BOOL;
    latchFlag : BOOL := FALSE;
    tonRun : TON;
END_VAR

NETWORK OB_MAIN
    RUNG 0 (n_r0): BRANCH @(0, 0, n_br) { | | StartBtn @(0, 0, n_s) || | | MotorOut @(1, 0, n_m) } -- |/| StopBtn @(0, 1, n_st) -- ( ) MotorOut @(0, 2, n_coil);
    RUNG 1 (n_r1): | | ResetTrigger @(0, 0, n_rt) -- (S) latchFlag @(0, 1, n_set);
    RUNG 2 (n_r2): | | latchFlag @(0, 0, n_lf) -- (R) latchFlag @(0, 1, n_rst) -- ( ) MidRungOut @(0, 2, n_mr);
    RUNG 3 (n_r3): | | MotorOut @(0, 0, n_m2) -- FB TON tonRun(PT := T#20ms) @(0, 1, n_ton) -- ( ) DelayLamp @(0, 2, n_dl);
END_NETWORK
"#;
        let lad_bundle = lad_fe
            .parse_and_validate(ladder_src, &ctx)
            .map_err(|diags| DiffError::FrontendFailed {
                scenario: name.to_owned(),
                details: format!("{diags:#?}"),
            })?
            .value;
        // Native `dendri-vm` executes `PouBody::Network`; RuSTy compiles derived ST from `StFrontend::emit`!
        let emitted_st = emit_st_checked(&st_fe, &lad_bundle, &ctx, name)?;
        scenarios.push(StDiffScenario {
            name,
            vm_bundle: lad_bundle,
            authoring_language: AuthoringLanguage::Ladder,
            emitted_st,
            ctx,
            uses_iec_fbs: true,
            io_bindings: vec![
                StVarBinding {
                    st_name: "StartBtn",
                    vm_pin: Some("Ia.0"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "StopBtn",
                    vm_pin: Some("Ia.1"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "ResetTrigger",
                    vm_pin: Some("Ia.2"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "MotorOut",
                    vm_pin: Some("Qa.0"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "MidRungOut",
                    vm_pin: Some("Qa.1"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "DelayLamp",
                    vm_pin: Some("Qa.2"),
                    kind: StVarKind::IoBool,
                },
            ],
            outputs: vec![
                StVarBinding {
                    st_name: "MotorOut",
                    vm_pin: Some("Qa.0"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "MidRungOut",
                    vm_pin: Some("Qa.1"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "DelayLamp",
                    vm_pin: Some("Qa.2"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "latchFlag",
                    vm_pin: None,
                    kind: StVarKind::GlobalBool,
                },
            ],
            scan_period_us: 10_000,
            scans: vec![
                StScanStimulus {
                    bool_inputs: vec![
                        ("StartBtn", false),
                        ("StopBtn", false),
                        ("ResetTrigger", false),
                    ],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                // Pulse Start + ResetTrigger -> MotorOut latches, latchFlag sets in Rung 1 then resets in Rung 2 while MidRungOut sees PRE-RESET rail=true!
                StScanStimulus {
                    bool_inputs: vec![
                        ("StartBtn", true),
                        ("StopBtn", false),
                        ("ResetTrigger", true),
                    ],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                // Release Start + ResetTrigger -> MotorOut stays latched, MidRungOut drops false, TON ET=10ms
                StScanStimulus {
                    bool_inputs: vec![
                        ("StartBtn", false),
                        ("StopBtn", false),
                        ("ResetTrigger", false),
                    ],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                // Scan 3: TON reaches 20ms -> DelayLamp=true!
                StScanStimulus {
                    bool_inputs: vec![
                        ("StartBtn", false),
                        ("StopBtn", false),
                        ("ResetTrigger", false),
                    ],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                // Scan 4: Stop -> MotorOut=false, DelayLamp=false
                StScanStimulus {
                    bool_inputs: vec![
                        ("StartBtn", false),
                        ("StopBtn", true),
                        ("ResetTrigger", false),
                    ],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
            ],
        });
    }

    // 5. Structural GRAFCET/SFC (`PouBody::Chart`) -> projected ST -> RuSTy LLVM vs native VM `Chart`!
    // Exercises IEC 60848 Rule 5 simultaneous deactivate+activate, deterministic OR priority,
    // AND divergence/convergence, and N/S/R/P/L/D action qualifiers!
    {
        let name = "sfc_projected_to_st_rusty";
        let ctx = rusty_plc_ctx(name);
        let sfc_src = r#"VAR_GLOBAL
    go_fast AT %IX0.0 : BOOL;
    go_slow AT %IX0.1 : BOOL;
    fork_now AT %IX0.2 : BOOL;
    done_a AT %IX0.3 : BOOL;
    done_b AT %IX0.4 : BOOL;
    shift_r5 AT %IX0.5 : BOOL;
    ValveOut AT %QX0.0 : BOOL;
    AlarmOut AT %QX0.1 : BOOL;
    coil_n : BOOL := FALSE;
    coil_latch : BOOL := FALSE;
    pulse_hits : DINT := 0;
END_VAR

SFC_CHART OB_MAIN
    INITIAL_STEP Idle:
    END_STEP

    STEP SyncStart:
        N: coil_n;
        N: ValveOut;
    END_STEP

    STEP SlowAlt:
        N: AlarmOut;
    END_STEP

    STEP BranchA:
        S: coil_latch;
        S: AlarmOut;
    END_STEP

    STEP BranchB:
        P: coil_n { pulse_hits := pulse_hits + 1; }
    END_STEP

    STEP WaitA:
    END_STEP

    STEP WaitB:
    END_STEP

    STEP R5_Left:
    END_STEP

    STEP R5_Mid:
        N: ValveOut;
    END_STEP

    STEP R5_Right:
        R: coil_latch;
        R: AlarmOut;
    END_STEP

    TRANSITION T_prio2 PRIORITY 2 FROM Idle TO SlowAlt := go_slow;
    END_TRANSITION

    TRANSITION T_prio1 PRIORITY 1 FROM Idle TO SyncStart := go_fast;
    END_TRANSITION

    TRANSITION T_fork FROM SyncStart TO (BranchA, BranchB) := fork_now;
    END_TRANSITION

    TRANSITION T_a_done FROM BranchA TO WaitA := done_a;
    END_TRANSITION

    TRANSITION T_b_done FROM BranchB TO WaitB := done_b;
    END_TRANSITION

    TRANSITION T_join FROM (WaitA, WaitB) TO (R5_Left, R5_Mid) := TRUE;
    END_TRANSITION

    TRANSITION T_r5_a PRIORITY 0 FROM R5_Left TO R5_Mid := shift_r5;
    END_TRANSITION

    TRANSITION T_r5_b PRIORITY 0 FROM R5_Mid TO R5_Right := shift_r5;
    END_TRANSITION
END_SFC_CHART
"#;
        let sfc_bundle = sfc_fe
            .parse(sfc_src, &ctx)
            .map_err(|diags| DiffError::FrontendFailed {
                scenario: name.to_owned(),
                details: format!("{diags:#?}"),
            })?
            .value;

        // Project `PouBody::Chart` into derived ASL Statements -> emit clean IEC ST for RuSTy!
        let mut st_bundle = sfc_bundle.clone();
        let mut extra_data = Vec::new();
        for pou in &mut st_bundle.program.pous {
            if let PouBody::Chart(chart) = &pou.body {
                let mut state_decls = project_chart_state_decls(chart);
                extra_data.append(&mut state_decls);
                let stmts = project_chart_to_statements(chart);
                pou.body = PouBody::Statements(stmts);
            }
        }
        st_bundle.program.data.extend(extra_data);
        let emitted_st = emit_st_checked(&st_fe, &st_bundle, &ctx, name)?;

        let make_sfc_scan = |go_fast, go_slow, fork_now, done_a, done_b, shift_r5| StScanStimulus {
            bool_inputs: vec![
                ("go_fast", go_fast),
                ("go_slow", go_slow),
                ("fork_now", fork_now),
                ("done_a", done_a),
                ("done_b", done_b),
                ("shift_r5", shift_r5),
            ],
            int16_inputs: vec![],
            vm_analog_inputs: vec![],
        };
        let scan0 = make_sfc_scan(false, false, false, false, false, false);
        // Competing OR priorities: both true -> T_prio1 (SyncStart) MUST win over T_prio2 (SlowAlt)!
        let scan1 = make_sfc_scan(true, true, false, false, false, false);
        let scan2 = make_sfc_scan(false, false, true, false, false, false); // AND fork
        let scan3 = make_sfc_scan(false, false, false, true, false, false); // WaitA only
        let scan4 = make_sfc_scan(false, false, false, false, true, false); // WaitB ready
        let scan5 = make_sfc_scan(false, false, false, false, false, false); // T_join -> R5_Left + R5_Mid
        // Rule 5: R5_Mid deactivated by T_r5_b AND activated by T_r5_a -> stays ACTIVE (ValveOut=true)!
        let scan6 = make_sfc_scan(false, false, false, false, false, true);
        let scan7 = make_sfc_scan(false, false, false, false, false, false);

        scenarios.push(StDiffScenario {
            name,
            vm_bundle: sfc_bundle,
            authoring_language: AuthoringLanguage::Sfc,
            emitted_st,
            ctx,
            uses_iec_fbs: false,
            io_bindings: vec![
                StVarBinding {
                    st_name: "go_fast",
                    vm_pin: Some("Ia.0"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "go_slow",
                    vm_pin: Some("Ia.1"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "fork_now",
                    vm_pin: Some("Ia.2"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "done_a",
                    vm_pin: Some("Ia.3"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "done_b",
                    vm_pin: Some("Ia.4"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "shift_r5",
                    vm_pin: Some("Ia.5"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "ValveOut",
                    vm_pin: Some("Qa.0"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "AlarmOut",
                    vm_pin: Some("Qa.1"),
                    kind: StVarKind::IoBool,
                },
            ],
            outputs: vec![
                StVarBinding {
                    st_name: "ValveOut",
                    vm_pin: Some("Qa.0"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "AlarmOut",
                    vm_pin: Some("Qa.1"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "coil_n",
                    vm_pin: None,
                    kind: StVarKind::GlobalBool,
                },
                StVarBinding {
                    st_name: "coil_latch",
                    vm_pin: None,
                    kind: StVarKind::GlobalBool,
                },
                StVarBinding {
                    st_name: "pulse_hits",
                    vm_pin: None,
                    kind: StVarKind::GlobalInt32,
                },
            ],
            scan_period_us: 10_000,
            scans: vec![scan0, scan1, scan2, scan3, scan4, scan5, scan6, scan7],
        });
    }

    // 6. Full Hostile Comment Corpus (`C3`) compiled natively via RuSTy `plc`!
    {
        let name = "st_hostile_comments_c3_rusty";
        let ctx = rusty_plc_ctx(name);
        let src = r#"VAR_GLOBAL
    RunCmd AT %IX0.0 : BOOL;
    LampOut AT %QX0.0 : BOOL;
    hits : DINT := 0;
END_VAR

PROGRAM OB_MAIN
    IF RunCmd THEN
        hits := hits + 1;
        LampOut := TRUE;
    ELSE
        LampOut := FALSE;
    END_IF;
END_PROGRAM
"#;
        let mut bundle = parse_st(&st_fe, src, &ctx, name)?;
        attach_hostile_corpus_comments(&mut bundle, name, "st");
        let emitted_st = emit_st_checked(&st_fe, &bundle, &ctx, name)?;
        scenarios.push(StDiffScenario {
            name,
            vm_bundle: bundle,
            authoring_language: AuthoringLanguage::St,
            emitted_st,
            ctx,
            uses_iec_fbs: false,
            io_bindings: vec![
                StVarBinding {
                    st_name: "RunCmd",
                    vm_pin: Some("Ia.0"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "LampOut",
                    vm_pin: Some("Qa.0"),
                    kind: StVarKind::IoBool,
                },
            ],
            outputs: vec![
                StVarBinding {
                    st_name: "LampOut",
                    vm_pin: Some("Qa.0"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "hits",
                    vm_pin: None,
                    kind: StVarKind::GlobalInt32,
                },
            ],
            scan_period_us: 10_000,
            scans: vec![
                StScanStimulus {
                    bool_inputs: vec![("RunCmd", false)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                StScanStimulus {
                    bool_inputs: vec![("RunCmd", true)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                StScanStimulus {
                    bool_inputs: vec![("RunCmd", true)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                StScanStimulus {
                    bool_inputs: vec![("RunCmd", false)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
            ],
        });
    }

    // 7. PLC 27,648-count -> Uno 10-bit reduced-rational scaling (zero `M = 10^9`, `T-NUM-2`/`T-NUM-5`)
    {
        let name = "st_analog_reduced_rational_rusty";
        let ctx = rusty_plc_ctx(name);
        let src = r#"VAR_GLOBAL
    StepCode AT %IX0.0 : BOOL;
    inAlarm AT %QX0.0 : BOOL;
    scanNum : DINT := 0;
    rawPlc : DINT := 0;
    scaledUno10 : DINT := 0;
END_VAR

PROGRAM OB_MAIN
    (* Cycle across canonical T-NUM-5 / T-NUM-2 PLC 27,648 codes: 0, 5529 (2V), 13824 (5V), 27648 (10V) *)
    CASE scanNum OF
        0: rawPlc := 0;
        1: rawPlc := 5529;
        2: rawPlc := 13824;
        ELSE rawPlc := 27648;
    END_CASE;

    (* Reduced rational plc_span_27648 (D=27648) -> uno_10bit (D=1024, n_max=1023):
       gcd=1024 -> floor(n / 27) clamped to [0, 1023], NEVER emitting M = 10^9! *)
    IF rawPlc <= 0 THEN
        scaledUno10 := 0;
    ELSIF (rawPlc / 27) > 1023 THEN
        scaledUno10 := 1023;
    ELSE
        scaledUno10 := rawPlc / 27;
    END_IF;
    inAlarm := StepCode AND (scaledUno10 >= 512);
    scanNum := scanNum + 1;
END_PROGRAM
"#;
        let bundle = parse_st(&st_fe, src, &ctx, name)?;
        let emitted_st = emit_st_checked(&st_fe, &bundle, &ctx, name)?;
        scenarios.push(StDiffScenario {
            name,
            vm_bundle: bundle,
            authoring_language: AuthoringLanguage::St,
            emitted_st,
            ctx,
            uses_iec_fbs: false,
            io_bindings: vec![
                StVarBinding {
                    st_name: "StepCode",
                    vm_pin: Some("Ia.0"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "inAlarm",
                    vm_pin: Some("Qa.0"),
                    kind: StVarKind::IoBool,
                },
            ],
            outputs: vec![
                StVarBinding {
                    st_name: "inAlarm",
                    vm_pin: Some("Qa.0"),
                    kind: StVarKind::IoBool,
                },
                StVarBinding {
                    st_name: "rawPlc",
                    vm_pin: None,
                    kind: StVarKind::GlobalInt32,
                },
                StVarBinding {
                    st_name: "scaledUno10",
                    vm_pin: None,
                    kind: StVarKind::GlobalInt32,
                },
            ],
            scan_period_us: 10_000,
            scans: vec![
                // 0 V -> 0 -> 0, alarm=false
                StScanStimulus {
                    bool_inputs: vec![("StepCode", true)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                // 2 V -> T-NUM-5 code 5529 -> floor(5529/27) = 204, alarm=false
                StScanStimulus {
                    bool_inputs: vec![("StepCode", true)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                // 5 V -> code 13824 -> floor(13824/27) = 512, alarm=true
                StScanStimulus {
                    bool_inputs: vec![("StepCode", true)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
                // 10 V -> code 27648 -> mandatory span->binned clamp to 1023 (NOT 1024!), alarm=true
                StScanStimulus {
                    bool_inputs: vec![("StepCode", true)],
                    int16_inputs: vec![],
                    vm_analog_inputs: vec![],
                },
            ],
        });
    }

    Ok(scenarios)
}

/// Formats a scalar `dendri-vm` [`Value`] into a canonical string for trace comparison.
#[must_use]
pub fn format_vm_value(val: &Value) -> String {
    match val {
        Value::Bool(b) => {
            if *b {
                "1".to_owned()
            } else {
                "0".to_owned()
            }
        }
        Value::Sint(n) => n.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Dint(n) => n.to_string(),
        Value::Lint(n) => n.to_string(),
        Value::Usint(n) => n.to_string(),
        Value::Uint(n) => n.to_string(),
        Value::Udint(n) => n.to_string(),
        Value::Ulint(n) => n.to_string(),
        Value::Byte(n) => n.to_string(),
        Value::Word(n) => n.to_string(),
        Value::Dword(n) => n.to_string(),
        Value::Lword(n) => n.to_string(),
        Value::Real(f) => format!("{f:.4}"),
        Value::Lreal(f) => format!("{f:.6}"),
        Value::TimeUs(us) => us.to_string(),
        Value::Analog(a) => a.to_string(),
        Value::Duty(d) => d.to_string(),
        Value::Code { value, .. } => value.to_string(),
        Value::String { text, .. } => text.clone(),
        Value::Void | Value::Array(_) | Value::Struct(_) | Value::Fb(_) => format!("{val:?}"),
    }
}

/// Executes a [`CppDiffScenario`] on `dendri-vm` and produces canonical per-step trace lines.
pub fn run_cpp_scenario_on_vm(scenario: &CppDiffScenario) -> Result<Vec<String>, DiffError> {
    let board = make_uno_board(scenario.name, &scenario.bundle);
    let mut vm =
        BoardVm::new(&board, vm_config_for(&scenario.ctx)).map_err(|e| DiffError::VmFailed {
            scenario: scenario.name.to_owned(),
            details: format!("compile BoardVm: {e}"),
        })?;
    let mut io = MemoryBoardIo::default();
    vm.boot(&mut io).map_err(|e| DiffError::VmFailed {
        scenario: scenario.name.to_owned(),
        details: format!("boot BoardVm: {e}"),
    })?;

    let mut lines = Vec::with_capacity(scenario.steps.len());
    for (step_idx, stim) in scenario.steps.iter().enumerate() {
        for &(pin, high) in &stim.digital_in {
            io.set_digital_in(pin, high);
        }
        for &(pin, analog) in &stim.analog_in {
            io.set_analog_in(pin, analog, dendri_ast::AnalogStatus::Ok);
        }
        let step_us = i64::try_from(scenario.vm_step_us).unwrap_or(i64::MAX);
        // Advance cooperative fiber until one complete `loop()` iteration finishes (including any
        // `delay()` / `WAIT_TIME` yields inside `loop()`), matching 1:1 with host `loop();`.
        let mut completed = false;
        for _ in 0..16 {
            let outcome =
                vm.step_cooperative(step_us, &mut io)
                    .map_err(|e| DiffError::VmFailed {
                        scenario: scenario.name.to_owned(),
                        details: format!("step {step_idx}: {e}"),
                    })?;
            match outcome {
                StepOutcome::Completed => {
                    completed = true;
                    break;
                }
                StepOutcome::YieldWait { wake_us, .. } => {
                    let advance_us =
                        i64::try_from(wake_us.saturating_sub(vm.time_us()).saturating_add(1))
                            .unwrap_or(step_us);
                    let _ = vm.step_cooperative(advance_us, &mut io).map_err(|e| {
                        DiffError::VmFailed {
                            scenario: scenario.name.to_owned(),
                            details: format!("wake wait on step {step_idx}: {e}"),
                        }
                    })?;
                    completed = true;
                    break;
                }
                StepOutcome::TimeSliceReached { .. } | StepOutcome::IoOutputChanged { .. } => {}
            }
        }
        if !completed {
            return Err(DiffError::VmFailed {
                scenario: scenario.name.to_owned(),
                details: format!("cooperative loop did not complete on step {step_idx}"),
            });
        }

        let mut parts: Vec<String> = Vec::new();
        parts.push(format!("step={step_idx}"));
        for &pin in &scenario.sample_digital_pins {
            let bit = u8::from(io.digital_outputs.get(pin).copied().unwrap_or(false));
            parts.push(format!("{pin}={bit}"));
        }
        for &pin in &scenario.sample_pwm_pins {
            let duty_ppb = io.pwm_outputs.get(pin).copied().unwrap_or(0);
            // Convert M=10^9 duty back to exact 8-bit PWM code (`pwm_8bit`, D=256, n_max=255)
            let code = if let Some(pwm_codec) = scenario.ctx.codecs.get("pwm_8bit") {
                pwm_codec.to_code_unipolar(u64::from(duty_ppb))
            } else {
                0
            };
            parts.push(format!("pwm_{pin}={code}"));
        }
        for &g in &scenario.sample_globals {
            let v = vm
                .get_global(g)
                .map_or_else(|| "?".to_owned(), format_vm_value);
            parts.push(format!("{g}={v}"));
        }
        lines.push(parts.join(" "));
    }

    if scenario.check_serial {
        let serial_text: String = io
            .uart_tx_log
            .iter()
            .map(|(_, payload)| payload.as_str())
            .collect();
        lines.push(format!("serial={serial_text:?}"));
    }

    Ok(lines)
}

/// Executes an [`StDiffScenario`] natively on `dendri-vm` (`Statements`, `Network`, or `Chart`)
/// and produces canonical per-scan trace lines.
pub fn run_st_scenario_on_vm(scenario: &StDiffScenario) -> Result<Vec<String>, DiffError> {
    let board = make_plc_board(
        scenario.name,
        scenario.authoring_language,
        scenario.scan_period_us,
        &scenario.vm_bundle,
    );
    let mut vm =
        BoardVm::new(&board, vm_config_for(&scenario.ctx)).map_err(|e| DiffError::VmFailed {
            scenario: scenario.name.to_owned(),
            details: format!("compile BoardVm: {e}"),
        })?;
    let mut io = MemoryBoardIo::default();
    vm.boot(&mut io).map_err(|e| DiffError::VmFailed {
        scenario: scenario.name.to_owned(),
        details: format!("boot BoardVm: {e}"),
    })?;

    let pin_map: BTreeMap<&str, &str> = scenario
        .io_bindings
        .iter()
        .filter_map(|b| b.vm_pin.map(|p| (b.st_name, p)))
        .collect();

    let mut lines = Vec::with_capacity(scenario.scans.len());
    for (scan_idx, stim) in scenario.scans.iter().enumerate() {
        for &(st_name, high) in &stim.bool_inputs {
            if let Some(&pin) = pin_map.get(st_name) {
                io.set_digital_in(pin, high);
            }
        }
        for &(pin, analog) in &stim.vm_analog_inputs {
            io.set_analog_in(pin, analog, dendri_ast::AnalogStatus::Ok);
        }
        vm.run_plc_scan(scenario.scan_period_us, &mut io)
            .map_err(|e| DiffError::VmFailed {
                scenario: scenario.name.to_owned(),
                details: format!("scan {scan_idx}: {e}"),
            })?;

        let mut parts: Vec<String> = Vec::new();
        parts.push(format!("scan={scan_idx}"));
        for out in &scenario.outputs {
            let val_s = match out.kind {
                StVarKind::IoBool => {
                    let pin = out.vm_pin.unwrap_or(out.st_name);
                    let b = io.digital_outputs.get(pin).copied().unwrap_or(false);
                    if b { "1".to_owned() } else { "0".to_owned() }
                }
                StVarKind::IoInt16 => {
                    let pin = out.vm_pin.unwrap_or(out.st_name);
                    let a = io.analog_outputs.get(pin).copied().unwrap_or(0);
                    a.to_string()
                }
                StVarKind::GlobalBool | StVarKind::GlobalInt16 | StVarKind::GlobalInt32 => vm
                    .get_global(out.st_name)
                    .map_or_else(|| "?".to_owned(), format_vm_value),
            };
            parts.push(format!("{}={val_s}", out.st_name));
        }
        lines.push(parts.join(" "));
    }

    Ok(lines)
}

fn rusty_plc_ctx(board_id: &str) -> BoardContext {
    let mut ctx = BoardContext::plc_1200_open_1214(board_id);
    ctx.st_dialect = StCommentDialect::RuSTy;
    ctx
}

fn parse_cpp(
    fe: &CppFrontend,
    src: &str,
    ctx: &BoardContext,
    name: &str,
) -> Result<BoardProgramBundle, DiffError> {
    fe.parse(src, ctx)
        .map(|o| o.value)
        .map_err(|diags| DiffError::FrontendFailed {
            scenario: name.to_owned(),
            details: format!("C++ parse: {diags:#?}"),
        })
}

fn emit_cpp_checked(
    fe: &CppFrontend,
    bundle: &BoardProgramBundle,
    ctx: &BoardContext,
    name: &str,
) -> Result<String, DiffError> {
    let out = fe
        .emit(bundle, ctx)
        .map_err(|diags| DiffError::FrontendFailed {
            scenario: name.to_owned(),
            details: format!("C++ emit: {diags:#?}"),
        })?
        .value;
    if contains_banned_magic_comment(&out) {
        return Err(DiffError::FrontendFailed {
            scenario: name.to_owned(),
            details: "emitted C++ contains banned machine magic comment".to_owned(),
        });
    }
    if contains_analog_full_scale_literal(&out) {
        return Err(DiffError::FrontendFailed {
            scenario: name.to_owned(),
            details: "emitted C++ leaks forbidden M = 10^9 literal".to_owned(),
        });
    }
    Ok(out)
}

fn parse_st(
    fe: &StFrontend,
    src: &str,
    ctx: &BoardContext,
    name: &str,
) -> Result<BoardProgramBundle, DiffError> {
    fe.parse(src, ctx)
        .map(|o| o.value)
        .map_err(|diags| DiffError::FrontendFailed {
            scenario: name.to_owned(),
            details: format!("ST parse: {diags:#?}"),
        })
}

fn emit_st_checked(
    fe: &StFrontend,
    bundle: &BoardProgramBundle,
    ctx: &BoardContext,
    name: &str,
) -> Result<String, DiffError> {
    let out = fe
        .emit(bundle, ctx)
        .map_err(|diags| DiffError::FrontendFailed {
            scenario: name.to_owned(),
            details: format!("ST emit: {diags:#?}"),
        })?
        .value;
    if contains_banned_magic_comment(&out) {
        return Err(DiffError::FrontendFailed {
            scenario: name.to_owned(),
            details: "emitted ST contains banned machine magic comment".to_owned(),
        });
    }
    if contains_analog_full_scale_literal(&out) {
        return Err(DiffError::FrontendFailed {
            scenario: name.to_owned(),
            details: "emitted ST leaks forbidden M = 10^9 literal".to_owned(),
        });
    }
    Ok(out)
}
