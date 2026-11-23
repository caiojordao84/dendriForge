//! Comprehensive Phase 2 verification suite for `dendri-vm`:
//! cycle-accurate 128-bit clock, cooperative yield/resume, guardrails,
//! lexical shadowing, INDETERMINATE policy, Ladder materialized rails,
//! GRAFCET IEC 60848 Rule 5 + OR priority + qualifiers, and Foreign/assumptions.

use dendri_project::load_project_str;
use dendri_span::ErrorCode;
use dendri_vm::{
    BoardVm, CycleCostModel, ElectricalDigitalLevel, IndeterminatePolicy, MemoryBoardIo,
    OverflowPolicy, RuntimeError, StepOutcome, TraceEventKind, TraceRing, Value, VmConfig,
};
use proptest::prelude::*;

fn load_single_board(toml_src: &str, cfg: VmConfig) -> Result<BoardVm, String> {
    let proj = load_project_str(toml_src, None).map_err(|e| format!("{e:?}"))?;
    let board = proj
        .boards
        .first()
        .ok_or_else(|| "missing board".to_owned())?;
    BoardVm::new(board, cfg).map_err(|e| format!("{e:?}"))
}

#[test]
fn cycle_clock_accumulates_sub_microsecond_remainder_and_wait_never_wakes_early() {
    let toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "PollLoop"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[[boards.program.hardware]]
id = "n_btn"
label = "BTN"
pin = "D2"
kind = "digital_in"

[[boards.program.data]]
id = "n_cnt"
name = "iters"
scope = "var"
ty = { kind = "scalar", ty = { name = "dint" } }
initial = "0"

[[boards.program.pous]]
id = "n_main"
name = "Main"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "while"
id = "n_w1"
condition = "NOT BTN AND iters < 20"
body = [
  { kind = "assign", id = "n_s1", target = "iters", value = "iters + 1" },
]
"#;
    let cfg = VmConfig {
        clock_hz: 16_000_000, // 62.5 ns / cycle -> 16 cycles = 1 us
        cost: CycleCostModel {
            stmt_cycles: 4,
            branch_cycles: 2,
            ..CycleCostModel::default()
        },
        ..VmConfig::default()
    };
    let mut vm = load_single_board(toml, cfg).unwrap();
    let mut io = MemoryBoardIo::default();
    io.set_digital_in("D2", false);

    let outcome = vm.step_cooperative(0, &mut io).unwrap();
    assert_eq!(outcome, StepOutcome::Completed);
    assert_eq!(vm.get_global("iters"), Some(&Value::Dint(20)));
    // Each iteration = 2 (JumpIfFalse) + 4 (Assign) + 2 (Jump) = 8 cycles = 0.5 us;
    // 20 iterations + final JumpIfFalse (2) = 162 cycles -> exact floor(162*10^6/16e6) = 10 us!
    assert_eq!(vm.fiber_cycles(), 162);
    assert_eq!(vm.time_us(), 10);

    // Verify WAIT_TIME_US never wakes early on non-integer clock_hz
    let wake_cycles = vm.wake_cycles_for_wait_us(5);
    assert_eq!(wake_cycles, 162 + 80);
    // Test ceil on 333_333 Hz: ceil(1 * 333_333 / 10^6) = 1 cycle (>= 1 us)
    let odd_cfg = VmConfig {
        clock_hz: 333_333,
        ..VmConfig::default()
    };
    let odd_vm = load_single_board(toml, odd_cfg).unwrap();
    assert_eq!(odd_vm.wake_cycles_for_wait_us(1), 1);
}

#[test]
fn cooperative_wait_yields_mid_loop_and_resumes_preserving_locals() {
    let toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "CoopYield"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[[boards.program.hardware]]
id = "n_led"
label = "LED"
pin = "D13"
kind = "digital_out"

[[boards.program.data]]
id = "n_phase"
name = "phase"
scope = "var"
ty = { kind = "scalar", ty = { name = "dint" } }
initial = "0"

[[boards.program.pous]]
id = "n_main"
name = "Main"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.locals]]
id = "n_loc"
name = "local_step"
scope = "temp"
ty = { kind = "scalar", ty = { name = "dint" } }
initial = "0"

[[boards.program.pous.body]]
kind = "assign"
id = "n_s1"
target = "local_step"
value = "10"

[[boards.program.pous.body]]
kind = "call"
id = "n_s2"
callee = "DIGITAL_WRITE"
args = [{ value = "LED" }, { value = "true" }]

[[boards.program.pous.body]]
kind = "wait"
id = "n_s3"
duration = "T#500ms"

[[boards.program.pous.body]]
kind = "assign"
id = "n_s4"
target = "phase"
value = "local_step + 5"

[[boards.program.pous.body]]
kind = "call"
id = "n_s5"
callee = "DIGITAL_WRITE"
args = [{ value = "LED" }, { value = "false" }]
"#;
    let mut vm = load_single_board(toml, VmConfig::default()).unwrap();
    let mut io = MemoryBoardIo::default();

    // First quantum runs up to Wait(500ms) -> LED=true, phase still 0
    let out1 = vm.step_cooperative(0, &mut io).unwrap();
    let StepOutcome::YieldWait { wake_us, .. } = out1 else {
        panic!("expected YieldWait, got {out1:?}");
    };
    assert!(wake_us >= 500_000);
    assert_eq!(io.digital_outputs.get("D13"), Some(&true));
    assert_eq!(vm.get_global("phase"), Some(&Value::Dint(0)));

    // Resume at wake_us -> local_step MUST still be 10, phase becomes 15, LED becomes false!
    vm.sync_to_time_us(wake_us);
    let out2 = vm.step_cooperative(500_000, &mut io).unwrap();
    assert_eq!(out2, StepOutcome::Completed);
    assert_eq!(vm.get_global("phase"), Some(&Value::Dint(15)));
    assert_eq!(io.digital_outputs.get("D13"), Some(&false));
}

#[test]
fn safety_guardrails_recursion_empty_loop_div_zero_overflow_and_shadowing() {
    // 1. Infinite recursion -> StackOverflow (never Rust stack crash!)
    let rec_toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "Rec"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[[boards.program.pous]]
id = "n_f"
name = "Recurse"
kind = { pou_kind = "fc", return_type = { kind = "scalar", ty = { name = "dint" } } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "return"
id = "n_r"
value = "Recurse()"

[[boards.program.pous]]
id = "n_m"
name = "Main"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "call"
id = "n_c"
callee = "Recurse"
args = []
"#;
    let mut vm_rec = load_single_board(
        rec_toml,
        VmConfig {
            max_call_depth: 16,
            ..VmConfig::default()
        },
    )
    .unwrap();
    let mut io = MemoryBoardIo::default();
    assert!(matches!(
        vm_rec.step_cooperative(0, &mut io),
        Err(RuntimeError::StackOverflow { limit: 16, .. })
    ));

    // 2. Empty infinite `while` loop -> ScanBudgetExceeded AND advances fiber_cycles
    let inf_toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "Inf"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[[boards.program.pous]]
id = "n_m"
name = "Main"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "while"
id = "n_w"
condition = "true"
body = []
"#;
    let mut vm_inf = load_single_board(
        inf_toml,
        VmConfig {
            max_steps_per_scan: 200,
            ..VmConfig::default()
        },
    )
    .unwrap();
    assert!(matches!(
        vm_inf.step_cooperative(0, &mut io),
        Err(RuntimeError::ScanBudgetExceeded { budget: 200 })
    ));
    assert!(vm_inf.fiber_cycles() > 0);

    // 3. Lexical shadowing + break/continue + div-by-zero + overflow Trap
    let shadow_toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "Shadow"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[[boards.program.data]]
id = "n_g"
name = "x"
scope = "var"
ty = { kind = "scalar", ty = { name = "dint" } }
initial = "100"

[[boards.program.data]]
id = "n_sum"
name = "sum"
scope = "var"
ty = { kind = "scalar", ty = { name = "dint" } }
initial = "0"

[[boards.program.data]]
id = "n_i"
name = "i"
scope = "var"
ty = { kind = "scalar", ty = { name = "dint" } }
initial = "0"

[[boards.program.pous]]
id = "n_fc"
name = "MutateParam"
kind = { pou_kind = "fc", return_type = { kind = "scalar", ty = { name = "dint" } } }
params = [{ name = "x", direction = "in", ty = { kind = "scalar", ty = { name = "dint" } } }]
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_a1"
target = "x"
value = "x + 7"

[[boards.program.pous.body]]
kind = "return"
id = "n_r1"
value = "x"

[[boards.program.pous]]
id = "n_m"
name = "Main"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_c1"
target = "sum"
value = "MutateParam(5)"

[[boards.program.pous.body]]
kind = "for"
id = "n_for"
counter = "i"
from = "0"
to = "10"
step = "1"
inclusive = false
body = [
  { kind = "if", id = "n_if1", condition = "i == 2", then_body = [{ kind = "continue", id = "n_cont" }] },
  { kind = "if", id = "n_if2", condition = "i == 5", then_body = [{ kind = "break", id = "n_brk" }] },
  { kind = "assign", id = "n_add", target = "sum", value = "sum + i" },
]
"#;
    let mut vm_sh = load_single_board(shadow_toml, VmConfig::default()).unwrap();
    vm_sh.step_cooperative(0, &mut io).unwrap();
    // Global `x` MUST still be 100 (parameter `x` shadowed it without leaking!)
    assert_eq!(vm_sh.get_global("x"), Some(&Value::Dint(100)));
    // sum = MutateParam(5)=12 + (0 + 1 + [skip 2] + 3 + 4) [break at 5] = 12 + 8 = 20!
    assert_eq!(vm_sh.get_global("sum"), Some(&Value::Dint(20)));

    // 4. Division by zero & overflow trap
    let div0 = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "Div0"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[[boards.program.data]]
id = "n_d"
name = "y"
scope = "var"
ty = { kind = "scalar", ty = { name = "sint" } }
initial = "127"

[[boards.program.pous]]
id = "n_m"
name = "Main"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_s1"
target = "y"
value = "y + 1"
"#;
    let mut vm_trap = load_single_board(
        div0,
        VmConfig {
            overflow: OverflowPolicy::Trap,
            ..VmConfig::default()
        },
    )
    .unwrap();
    assert_eq!(
        vm_trap.step_cooperative(0, &mut io),
        Err(RuntimeError::IntegerOverflow)
    );
    let mut vm_wrap = load_single_board(
        div0,
        VmConfig {
            overflow: OverflowPolicy::Wrap,
            ..VmConfig::default()
        },
    )
    .unwrap();
    vm_wrap.step_cooperative(0, &mut io).unwrap();
    assert_eq!(vm_wrap.get_global("y"), Some(&Value::Sint(-128)));

    // 4b. Actual integer division by zero & modulo by zero -> RuntimeError::DivisionByZero
    let zero_div_toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "RealDiv0"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[[boards.program.data]]
id = "n_z"
name = "z"
scope = "var"
ty = { kind = "scalar", ty = { name = "dint" } }
initial = "0"

[[boards.program.pous]]
id = "n_m"
name = "Main"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_s1"
target = "z"
value = "42 / z"
"#;
    let mut vm_div0 = load_single_board(zero_div_toml, VmConfig::default()).unwrap();
    assert_eq!(
        vm_div0.step_cooperative(0, &mut io),
        Err(RuntimeError::DivisionByZero)
    );

    let zero_mod_toml = zero_div_toml.replace("42 / z", "42 MOD z");
    let mut vm_mod0 = load_single_board(&zero_mod_toml, VmConfig::default()).unwrap();
    assert_eq!(
        vm_mod0.step_cooperative(0, &mut io),
        Err(RuntimeError::DivisionByZero)
    );

    // 5. Bounded trace ring drops oldest cleanly
    let mut ring = TraceRing::with_capacity(2);
    for t in 0..5u64 {
        ring.push(dendri_vm::TraceEvent {
            time_us: t,
            board_id: "b".to_owned(),
            kind: TraceEventKind::DigitalWrite {
                pin: "D13".to_owned(),
                high: true,
            },
        });
    }
    assert_eq!(ring.len(), 2);
    assert_eq!(ring.dropped_count(), 3);
    assert_eq!(ring.iter().next().unwrap().time_us, 3);
}

#[test]
fn indeterminate_digital_input_follows_policy_and_emits_warning() {
    let toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "Indet"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[[boards.program.hardware]]
id = "n_in"
label = "IN_PIN"
pin = "D2"
kind = "digital_in"

[[boards.program.data]]
id = "n_out"
name = "sampled"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }

[[boards.program.pous]]
id = "n_m"
name = "Main"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_s1"
target = "sampled"
value = "IN_PIN"
"#;
    let mut vm = load_single_board(
        toml,
        VmConfig {
            indeterminate_reads_as: IndeterminatePolicy::Previous,
            ..VmConfig::default()
        },
    )
    .unwrap();
    let mut io = MemoryBoardIo::default();
    io.set_digital_in("D2", true);
    vm.step_cooperative(0, &mut io).unwrap();
    assert_eq!(vm.get_global("sampled"), Some(&Value::Bool(true)));

    // Now set pin to INDETERMINATE -> Previous policy keeps `true` AND logs WARN_INDETERMINATE_LEVEL
    io.set_digital_level("D2", ElectricalDigitalLevel::Indeterminate);
    vm.step_cooperative(0, &mut io).unwrap();
    assert_eq!(vm.get_global("sampled"), Some(&Value::Bool(true)));
    assert!(vm.trace().iter().any(|e| matches!(
        &e.kind,
        TraceEventKind::Diagnostic {
            code: ErrorCode::WarnIndeterminateLevel,
            ..
        }
    )));
}

#[test]
fn ladder_materialized_power_flow_mid_rung_reset_empty_open_and_ton_q() {
    let toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "LadderSemantics"

[[boards]]
id = "plc"
profile = "plc-1200-open-1214"
authoring_language = "ladder"
execution = "plc_scan"
scan_period_us = 10000

[[boards.program.hardware]]
id = "n_in"
label = "START"
pin = "Ia.0"
kind = "digital_in"

[[boards.program.hardware]]
id = "n_q0"
label = "LAMP"
pin = "Qa.0"
kind = "digital_out"

[[boards.program.data]]
id = "n_a"
name = "a"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "true"

[[boards.program.data]]
id = "n_b"
name = "b"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "false"

[[boards.program.data]]
id = "n_empty_out"
name = "empty_out"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "true"

[[boards.program.data]]
id = "n_ton"
name = "t1"
scope = "var"
ty = { kind = "fb", fb_kind = { name = "ton" } }

[[boards.program.pous]]
id = "n_main"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "network"

# Rung 0: [a] --(R a)-- (b)  -> mid-rung mutation MUST NOT clear rail seen by (b)!
[[boards.program.pous.network.rungs]]
id = "n_r0"
order = 0

[[boards.program.pous.network.rungs.cells]]
id = "n_c0"
row = 0
col = 0
kind = "contact"
contact_kind = "normally_open"
target = "a"

[[boards.program.pous.network.rungs.cells]]
id = "n_c1"
row = 0
col = 1
kind = "coil"
coil_kind = "reset"
target = "a"

[[boards.program.pous.network.rungs.cells]]
id = "n_c2"
row = 0
col = 2
kind = "coil"
coil_kind = "normal"
target = "b"

# Rung 1: [EMPTY] --(empty_out) -> Empty cell is OPEN CIRCUIT (`false`), Normal coil writes false!
[[boards.program.pous.network.rungs]]
id = "n_r1"
order = 1

[[boards.program.pous.network.rungs.cells]]
id = "n_c3"
row = 0
col = 0
kind = "empty"

[[boards.program.pous.network.rungs.cells]]
id = "n_c4"
row = 0
col = 1
kind = "coil"
coil_kind = "normal"
target = "empty_out"

# Rung 2: [START] --[TON t1 PT=20ms]-- (LAMP) -> Q propagates to LAMP!
[[boards.program.pous.network.rungs]]
id = "n_r2"
order = 2

[[boards.program.pous.network.rungs.cells]]
id = "n_c5"
row = 0
col = 0
kind = "contact"
contact_kind = "normally_open"
target = "START"

[[boards.program.pous.network.rungs.cells]]
id = "n_c6"
row = 0
col = 1
kind = "fb_box"
fb_kind = { name = "ton" }
instance = "t1"
inputs = [{ name = "PT", value = "T#20ms" }]

[[boards.program.pous.network.rungs.cells]]
id = "n_c7"
row = 0
col = 2
kind = "coil"
coil_kind = "normal"
target = "LAMP"
"#;
    let mut vm = load_single_board(toml, VmConfig::default()).unwrap();
    let mut io = MemoryBoardIo::default();
    io.set_digital_in("Ia.0", true);

    // Scan 1 (dt = 10ms):
    // - Rung 0: a was true -> (R a) sets a=false, AND (b) MUST become true from materialized pre-reset rail!
    // - Rung 1: Empty cell -> empty_out MUST reset from true to false!
    // - Rung 2: TON ET=10ms < 20ms -> LAMP = false!
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert_eq!(vm.get_global("a"), Some(&Value::Bool(false)));
    assert_eq!(vm.get_global("b"), Some(&Value::Bool(true)));
    assert_eq!(vm.get_global("empty_out"), Some(&Value::Bool(false)));
    assert_eq!(io.digital_outputs.get("Qa.0"), Some(&false));

    // Scan 2 (dt = 10ms -> total 20ms): TON Q becomes true -> LAMP = true!
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert_eq!(io.digital_outputs.get("Qa.0"), Some(&true));

    // Scan 3: START drops false -> TON resets -> Normal coil writes LAMP = false!
    io.set_digital_in("Ia.0", false);
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert_eq!(io.digital_outputs.get("Qa.0"), Some(&false));
}

#[test]
fn grafcet_deterministic_or_priority_rule5_simultaneous_and_n_auto_reset() {
    let toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "GrafcetExact"

[[boards]]
id = "plc"
profile = "plc-1200-open-1214"
authoring_language = "sfc"
execution = "plc_scan"
scan_period_us = 10000

[[boards.program.data]]
id = "n_gate"
name = "gate"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "false"

[[boards.program.data]]
id = "n_lamp"
name = "lamp"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "false"

[[boards.program.data]]
id = "n_p_cnt"
name = "p_cnt"
scope = "var"
ty = { kind = "scalar", ty = { name = "dint" } }
initial = "0"

[[boards.program.pous]]
id = "n_main"
name = "Chart1"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "chart"

[boards.program.pous.chart]
evolve_to_stability = false

[[boards.program.pous.chart.steps]]
id = "n_s0"
name = "S0"
initial = true
[[boards.program.pous.chart.steps.actions]]
id = "n_a0"
qualifier = "N"
target = "lamp"

[[boards.program.pous.chart.steps]]
id = "n_s1"
name = "S1"
initial = false
[[boards.program.pous.chart.steps.actions]]
id = "n_a1"
qualifier = "P"
body = [{ kind = "assign", id = "n_inc", target = "p_cnt", value = "p_cnt + 1" }]

[[boards.program.pous.chart.steps]]
id = "n_s2"
name = "S2"
initial = false

# Competing OR transitions from S0 with identical receptivity `S0.T >= T#10ms`:
# T_low is declared FIRST with priority 5; T_high is declared SECOND with priority 1.
# Priority 1 MUST win deterministically!
[[boards.program.pous.chart.transitions]]
id = "n_tlo"
name = "T_low"
from_steps = ["S0"]
to_steps = ["S2"]
priority = 5
receptivity = "S0.T >= T#10ms"

[[boards.program.pous.chart.transitions]]
id = "n_thi"
name = "T_high"
from_steps = ["S0"]
to_steps = ["S1"]
priority = 1
receptivity = "S0.T >= T#10ms"

# Rule 5: once in S1, self-transition simultaneously deactivates S1 and reactivates S1 -> stays ACTIVE!
[[boards.program.pous.chart.transitions]]
id = "n_tr5"
name = "T_rule5"
from_steps = ["S1"]
to_steps = ["S1"]
priority = 0
receptivity = "S1.X"
"#;
    let mut vm = load_single_board(toml, VmConfig::default()).unwrap();
    let mut io = MemoryBoardIo::default();

    // Scan 1: S0 initializes active, S0.T = 0 -> stays in S0, `N` drives lamp = true!
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert!(vm.active_sfc_steps("Chart1").unwrap().contains("S0"));
    assert_eq!(vm.get_global("lamp"), Some(&Value::Bool(true)));
    assert_eq!(vm.get_global("p_cnt"), Some(&Value::Dint(0)));

    // Scan 2: S0.T advances to 10ms -> BOTH T_low and T_high receptivities are true!
    // Priority 1 (T_high -> S1) MUST win exclusively; S0 deactivates -> `N` auto-resets lamp = false!
    // S1 activates -> `P` fires ONCE -> p_cnt = 1!
    vm.run_plc_scan(10_000, &mut io).unwrap();
    let active2 = vm.active_sfc_steps("Chart1").unwrap();
    assert!(active2.contains("S1"));
    assert!(!active2.contains("S0"));
    assert!(
        !active2.contains("S2"),
        "OR priority violated: S2 must NOT activate!"
    );
    assert_eq!(
        vm.get_global("lamp"),
        Some(&Value::Bool(false)),
        "N qualifier must auto-reset coil!"
    );
    assert_eq!(vm.get_global("p_cnt"), Some(&Value::Dint(1)));

    // Scan 3: T_rule5 fires (from S1 -> to S1) -> Rule 5 keeps S1 ACTIVE!
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert!(vm.active_sfc_steps("Chart1").unwrap().contains("S1"));
    // Because S1 was already active at scan start, `P` does NOT re-fire!
    assert_eq!(vm.get_global("p_cnt"), Some(&Value::Dint(1)));
}

#[test]
fn foreign_halts_with_four_part_message_unless_assumption_recorded() {
    let without_assumption = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "ForeignHalt"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[[boards.program.pous]]
id = "n_m"
name = "Main"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "foreign"
id = "n_f1"
language = "cpp"
fidelity = "F0"
text = "servo.write(90);"
"#;
    let mut vm1 = load_single_board(without_assumption, VmConfig::default()).unwrap();
    let mut io = MemoryBoardIo::default();
    let err = vm1.step_cooperative(0, &mut io).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("servo.write(90);"));
    assert!(msg.contains("n_f1"));
    assert!(msg.contains("F0"));
    assert!(msg.contains("[[boards.assumptions]]"));

    // With explicit assumption, simulation proceeds cleanly!
    let with_assumption = format!(
        "{without_assumption}\n[[boards.assumptions]]\nnode = \"n_f1\"\npolicy = \"no_effect\"\nnote = \"Servo unmodeled\"\n"
    );
    let mut vm2 = load_single_board(&with_assumption, VmConfig::default()).unwrap();
    assert_eq!(
        vm2.step_cooperative(0, &mut io).unwrap(),
        StepOutcome::Completed
    );
}

#[test]
fn libm_transcendentals_time_diff_and_comm_event_ob50() {
    let toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "MathAndOb50"

[[boards]]
id = "plc"
profile = "plc-1200-open-1214"
authoring_language = "st"
execution = "plc_scan"
scan_period_us = 10000

[[boards.program.data]]
id = "n_s"
name = "s"
scope = "var"
ty = { kind = "scalar", ty = { name = "lreal" } }
initial = "0.0"

[[boards.program.data]]
id = "n_td"
name = "td"
scope = "var"
ty = { kind = "scalar", ty = { name = "udint" } }
initial = "0"

[[boards.program.data]]
id = "n_rx"
name = "rx_count"
scope = "var"
ty = { kind = "scalar", ty = { name = "dint" } }
initial = "0"

[[boards.program.pous]]
id = "n_main"
name = "Main"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_a1"
target = "s"
value = "SQRT(9.0) + SCALE_X(0.0, NORM_X(0.0, 2.5, 10.0), 100.0)"

[[boards.program.pous.body]]
kind = "assign"
id = "n_a2"
target = "td"
value = "TIME_DIFF(5, 4294967290)"

[[boards.program.pous]]
id = "n_ob50"
name = "OnUart"
kind = { pou_kind = "ob", ob = { type = "comm_event", bus_id = "uart0" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_a3"
target = "rx_count"
value = "rx_count + 1"
"#;
    let mut vm = load_single_board(toml, VmConfig::default()).unwrap();
    let mut io = MemoryBoardIo::default();
    vm.run_plc_scan(10_000, &mut io).unwrap();

    // SQRT(9.0) = 3.0; NORM_X(0, 10, 2.5) = 0.25; SCALE_X(0, 100, 0.25) = 25.0 -> 28.0
    if let Some(Value::Lreal(v)) = vm.get_global("s") {
        assert!((v - 28.0).abs() < 1e-12, "expected 28.0, got {v}");
    } else {
        panic!("expected LREAL for s");
    }
    // Unsigned 32-bit wrap: 5u32.wrapping_sub(4294967290u32) == 11!
    assert_eq!(vm.get_global("td"), Some(&Value::Udint(11)));
    assert_eq!(vm.comm_events().len(), 1);
    vm.run_comm_event_obs("uart0", &mut io).unwrap();
    assert_eq!(vm.get_global("rx_count"), Some(&Value::Dint(1)));
}

#[test]
fn grafcet_qualifiers_and_convergence_stability_guard() {
    let toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "GrafcetQuals"

[[boards]]
id = "plc"
profile = "plc-1200-open-1214"
authoring_language = "sfc"
execution = "plc_scan"
scan_period_us = 10000

[[boards.program.data]]
id = "n_lim"
name = "q_limited"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "false"

[[boards.program.data]]
id = "n_del"
name = "q_delayed"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "false"

[[boards.program.data]]
id = "n_lat"
name = "q_latched"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "false"

[[boards.program.data]]
id = "n_fall"
name = "fall_cnt"
scope = "var"
ty = { kind = "scalar", ty = { name = "dint" } }
initial = "0"

[[boards.program.pous]]
id = "n_main"
name = "QualChart"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "chart"

[boards.program.pous.chart]
evolve_to_stability = false

[[boards.program.pous.chart.steps]]
id = "n_s0"
name = "S0"
initial = true
[[boards.program.pous.chart.steps.actions]]
id = "n_al"
qualifier = "L"
duration = "T#20ms"
target = "q_limited"
[[boards.program.pous.chart.steps.actions]]
id = "n_ad"
qualifier = "D"
duration = "T#20ms"
target = "q_delayed"
[[boards.program.pous.chart.steps.actions]]
id = "n_as"
qualifier = "S"
target = "q_latched"
[[boards.program.pous.chart.steps.actions]]
id = "n_ap0"
qualifier = "P0"
body = [{ kind = "assign", id = "n_finc", target = "fall_cnt", value = "fall_cnt + 1" }]

[[boards.program.pous.chart.steps]]
id = "n_s1a"
name = "S1A"
initial = false

[[boards.program.pous.chart.steps]]
id = "n_s1b"
name = "S1B"
initial = false

[[boards.program.pous.chart.steps]]
id = "n_s2"
name = "S2"
initial = false
[[boards.program.pous.chart.steps.actions]]
id = "n_ar"
qualifier = "R"
target = "q_latched"

# AND divergence: S0 -> (S1A, S1B) after 30ms
[[boards.program.pous.chart.transitions]]
id = "n_t0"
name = "T0"
from_steps = ["S0"]
to_steps = ["S1A", "S1B"]
priority = 0
receptivity = "S0.T >= T#30ms"

# AND convergence: (S1A, S1B) -> S2
[[boards.program.pous.chart.transitions]]
id = "n_t1"
name = "T1"
from_steps = ["S1A", "S1B"]
to_steps = ["S2"]
priority = 0
receptivity = "S1A.X AND S1B.X"
"#;
    let mut vm = load_single_board(toml, VmConfig::default()).unwrap();
    let mut io = MemoryBoardIo::default();

    // Scan 1 (S0.T = 0ms): L(20ms) ON, D(20ms) OFF, S latched ON, P0=0
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert_eq!(vm.get_global("q_limited"), Some(&Value::Bool(true)));
    assert_eq!(vm.get_global("q_delayed"), Some(&Value::Bool(false)));
    assert_eq!(vm.get_global("q_latched"), Some(&Value::Bool(true)));
    assert_eq!(vm.get_global("fall_cnt"), Some(&Value::Dint(0)));

    // Scan 2 (S0.T = 10ms): L ON, D OFF
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert_eq!(vm.get_global("q_limited"), Some(&Value::Bool(true)));
    assert_eq!(vm.get_global("q_delayed"), Some(&Value::Bool(false)));

    // Scan 3 (S0.T = 20ms): L expires -> OFF! D activates -> ON!
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert_eq!(vm.get_global("q_limited"), Some(&Value::Bool(false)));
    assert_eq!(vm.get_global("q_delayed"), Some(&Value::Bool(true)));

    // Scan 4 (S0.T = 30ms): T0 fires AND divergence -> S1A & S1B both active! P0 fires on S0 exit! S stays latched!
    vm.run_plc_scan(10_000, &mut io).unwrap();
    let act4 = vm.active_sfc_steps("QualChart").unwrap();
    assert!(act4.contains("S1A") && act4.contains("S1B") && !act4.contains("S0"));
    assert_eq!(vm.get_global("fall_cnt"), Some(&Value::Dint(1)));
    assert_eq!(vm.get_global("q_latched"), Some(&Value::Bool(true)));

    // Scan 5: T1 fires AND convergence -> S2 active! R resets q_latched -> false!
    vm.run_plc_scan(10_000, &mut io).unwrap();
    assert!(vm.active_sfc_steps("QualChart").unwrap().contains("S2"));
    assert_eq!(vm.get_global("q_latched"), Some(&Value::Bool(false)));

    // Stability-cycle guard: evolve_to_stability = true with unconditional cycle -> SfcInstabilityLoop
    let cycle_toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "SfcCycle"

[[boards]]
id = "plc"
profile = "plc-1200-open-1214"
authoring_language = "sfc"
execution = "plc_scan"
scan_period_us = 10000

[[boards.program.pous]]
id = "n_main"
name = "LoopChart"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "chart"

[boards.program.pous.chart]
evolve_to_stability = true

[[boards.program.pous.chart.steps]]
id = "n_s0"
name = "S0"
initial = true

[[boards.program.pous.chart.steps]]
id = "n_s1"
name = "S1"
initial = false

[[boards.program.pous.chart.transitions]]
id = "n_t0"
name = "T0"
from_steps = ["S0"]
to_steps = ["S1"]
priority = 0
receptivity = "true"

[[boards.program.pous.chart.transitions]]
id = "n_t1"
name = "T1"
from_steps = ["S1"]
to_steps = ["S0"]
priority = 0
receptivity = "true"
"#;
    let mut vm_cyc = load_single_board(cycle_toml, VmConfig::default()).unwrap();
    assert!(matches!(
        vm_cyc.run_plc_scan(10_000, &mut io),
        Err(RuntimeError::SfcInstabilityLoop { .. })
    ));
}

proptest! {
    #[test]
    fn fuzz_arithmetic_and_for_loops_never_panic(
        a in -5000i32..=5000i32,
        b in -5000i32..=5000i32,
        start in -20i32..=20i32,
        end in -20i32..=20i32,
        step in -5i32..=5i32,
        inclusive in any::<bool>(),
    ) {
        let toml = format!(r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "FuzzVm"

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[[boards.program.data]]
id = "n_a"
name = "a"
scope = "var"
ty = {{ kind = "scalar", ty = {{ name = "dint" }} }}
initial = "{a}"

[[boards.program.data]]
id = "n_b"
name = "b"
scope = "var"
ty = {{ kind = "scalar", ty = {{ name = "dint" }} }}
initial = "{b}"

[[boards.program.data]]
id = "n_acc"
name = "acc"
scope = "var"
ty = {{ kind = "scalar", ty = {{ name = "dint" }} }}
initial = "0"

[[boards.program.data]]
id = "n_i"
name = "i"
scope = "var"
ty = {{ kind = "scalar", ty = {{ name = "dint" }} }}
initial = "0"

[[boards.program.pous]]
id = "n_m"
name = "Main"
kind = {{ pou_kind = "ob", ob = {{ type = "main" }} }}
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_s1"
target = "acc"
value = "(a + b) * (a - b) / b"

[[boards.program.pous.body]]
kind = "for"
id = "n_f"
counter = "i"
from = "{start}"
to = "{end}"
step = "{step}"
inclusive = {inclusive}
body = [
  {{ kind = "assign", id = "n_s2", target = "acc", value = "acc + i" }},
]
"#);
        let mut vm = load_single_board(
            &toml,
            VmConfig {
                max_steps_per_scan: 2_000,
                ..VmConfig::default()
            },
        ).unwrap();
        let mut io = MemoryBoardIo::default();
        let res = vm.step_cooperative(0, &mut io);
        if b == 0 {
            prop_assert_eq!(res, Err(RuntimeError::DivisionByZero));
        } else {
            prop_assert!(res.is_ok());
        }
    }
}
