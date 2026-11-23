//! Comprehensive Phase 2 integration tests for `dendri-sim-kernel`:
//! - 2-board heterogeneous co-simulation (Cooperative Arduino Uno <-> PlcScan Siemens S7-1200 Ladder)
//! - `WAIT_TIME` suspending only the calling fiber while PLC scans, `OB_HW_INT`, and `OB_CYCLIC_INT` stay live
//! - Cooperative polling loop (`while (NOT BTN)`) advancing virtual time & observing `InputTimeline` edges
//! - Bit-identical deterministic replay across independent kernel instances
//! - Wire bus contention producing `INDETERMINATE` + `WARN_INDETERMINATE_LEVEL`
//! - Dedicated background `SimWorker` thread with immutable `Arc<SimSnapshot>` frames & pause semantics

use dendri_project::load_project_str;
use dendri_sim_kernel::{
    InputTimeline, KernelConfig, SimKernel, SimWorker, StimulusEvent, StimulusKind, WorkerCommand,
    WorkerState,
};
use dendri_span::ErrorCode;
use dendri_vm::{ElectricalDigitalLevel, TraceEvent, TraceEventKind, Value};
use std::time::Duration;

const TWO_BOARD_PROJECT_TOML: &str = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "UnoS7CoSim"

# Board 1: Cooperative MCU (Arduino Uno R3, 16 MHz)
# - OB_MAIN blinks D8 every 50 ms (100 ms period -> rising edges at t=0ms, t=100ms, t=200ms)
# - OB_HW_INT on D2 (Rising) increments `ack_count` even while OB_MAIN sleeps in WAIT_TIME(50)
# - OB_CYCLIC_INT every 25 ms increments `cyclic_ticks` even while OB_MAIN sleeps
[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[[boards.program.hardware]]
id = "n_u_tx"
label = "PULSE_OUT"
pin = "D8"
kind = "digital_out"

[[boards.program.hardware]]
id = "n_u_ack"
label = "PLC_ACK"
pin = "D2"
kind = "digital_in"

[[boards.program.data]]
id = "n_u_ack_cnt"
name = "ack_count"
scope = "var"
ty = { kind = "scalar", ty = { name = "dint" } }
initial = "0"

[[boards.program.data]]
id = "n_u_cyc"
name = "cyclic_ticks"
scope = "var"
ty = { kind = "scalar", ty = { name = "dint" } }
initial = "0"

[[boards.program.data]]
id = "n_u_cycles"
name = "blink_cycles"
scope = "var"
ty = { kind = "scalar", ty = { name = "dint" } }
initial = "0"

[[boards.program.pous]]
id = "n_u_main"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "call"
id = "n_u_s1"
callee = "DIGITAL_WRITE"
args = [{ value = "PULSE_OUT" }, { value = "true" }]

[[boards.program.pous.body]]
kind = "wait"
id = "n_u_s2"
duration = "T#50ms"

[[boards.program.pous.body]]
kind = "call"
id = "n_u_s3"
callee = "DIGITAL_WRITE"
args = [{ value = "PULSE_OUT" }, { value = "false" }]

[[boards.program.pous.body]]
kind = "wait"
id = "n_u_s4"
duration = "T#50ms"

[[boards.program.pous.body]]
kind = "assign"
id = "n_u_s5"
target = "blink_cycles"
value = "blink_cycles + 1"

[[boards.program.pous]]
id = "n_u_hwint"
name = "OnPlcAck"
kind = { pou_kind = "ob", ob = { type = "hw_int", pin_label = "PLC_ACK", edge = "rising", priority = 1 } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_u_h1"
target = "ack_count"
value = "ack_count + 1"

[[boards.program.pous]]
id = "n_u_cycint"
name = "OnTick25ms"
kind = { pou_kind = "ob", ob = { type = "cyclic_int", period_us = 25000 } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_u_c1"
target = "cyclic_ticks"
value = "cyclic_ticks + 1"

# Board 2: Industrial PLC (Siemens S7-1200, 10 ms scan cycle, Ladder)
# - OB_STARTUP sets `booted = true`
# - OB_MAIN (Ladder) feeds Ia.0 into CTU (PV=2) and drives Qa.0 from CTU Q!
[[boards]]
id = "plc"
profile = "siemens-s7-1200-cpu-1214c"
authoring_language = "ladder"
execution = "plc_scan"
scan_period_us = 10000

[[boards.program.hardware]]
id = "n_p_in"
label = "PULSE_IN"
pin = "Ia.0"
kind = "digital_in"

[[boards.program.hardware]]
id = "n_p_out"
label = "ACK_OUT"
pin = "Qa.0"
kind = "digital_out"

[[boards.program.data]]
id = "n_p_boot"
name = "booted"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "false"

[[boards.program.data]]
id = "n_p_ctu"
name = "edge_cnt"
scope = "var"
ty = { kind = "fb", fb_kind = { name = "ctu" } }

[[boards.program.pous]]
id = "n_p_start"
name = "OB_STARTUP"
kind = { pou_kind = "ob", ob = { type = "startup" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_p_b1"
target = "booted"
value = "true"

[[boards.program.pous]]
id = "n_p_main"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "network"

[[boards.program.pous.network.rungs]]
id = "n_p_r0"
order = 0

[[boards.program.pous.network.rungs.cells]]
id = "n_p_c0"
row = 0
col = 0
kind = "contact"
contact_kind = "normally_open"
target = "PULSE_IN"

[[boards.program.pous.network.rungs.cells]]
id = "n_p_c1"
row = 0
col = 1
kind = "fb_box"
fb_kind = { name = "ctu" }
instance = "edge_cnt"
inputs = [{ name = "R", value = "false" }, { name = "PV", value = "2" }]

[[boards.program.pous.network.rungs.cells]]
id = "n_p_c2"
row = 0
col = 2
kind = "coil"
coil_kind = "normal"
target = "ACK_OUT"

# Bidirectional physical wires connecting Arduino Uno <-> Siemens S7-1200
[[wires]]
id = "w_pulse"
from = "uno.D8"
to = "plc.Ia.0"
signal = "digital"

[[wires]]
id = "w_ack"
from = "plc.Qa.0"
to = "uno.D2"
signal = "digital"
"#;

#[test]
fn two_board_heterogeneous_cosim_wait_yields_only_uno_while_plc_and_interrupts_stay_live() {
    let proj = load_project_str(TWO_BOARD_PROJECT_TOML, None).unwrap();
    let mut kernel = SimKernel::new(&proj, KernelConfig::default(), InputTimeline::new()).unwrap();

    // Run 250 ms (`250_000 us`):
    // - t=0ms: plc boots (`booted=true`); uno drives D8=true and sleeps until 50ms.
    //          plc scan at t=0ms or t=10ms sees D8=true -> CTU CV=1, Q=false.
    // - t=50ms: uno wakes, drives D8=false, sleeps until 100ms.
    //           plc scan at t=50ms/60ms sees D8=false.
    // - t=100ms: uno wakes, completes cycle 1, drives D8=true, sleeps until 150ms.
    //            plc scan at t=110ms (or 100ms) sees 2nd rising edge -> CTU CV=2, Q=true -> Qa.0=true!
    //            IMMEDIATELY triggers uno's OB_HW_INT (`OnPlcAck`) WHILE uno sleeps in WAIT_TIME(50)!
    // - t=200ms: uno drives D8=true for 3rd rising edge -> CTU CV=3, Q stays true.
    // - Cyclic interrupt on uno fires at 25, 50, 75, 100, 125, 150, 175, 200, 225, 250 ms = 10 ticks!
    kernel.run_until(250_000).unwrap();

    assert_eq!(kernel.time_us(), 250_000);
    assert_eq!(
        kernel.board_global("plc", "booted"),
        Some(&Value::Bool(true))
    );
    assert_eq!(
        kernel.board_global("uno", "blink_cycles"),
        Some(&Value::Dint(2))
    );
    assert_eq!(
        kernel.board_global("uno", "ack_count"),
        Some(&Value::Dint(1)),
        "uno OB_HW_INT must fire once when plc Qa.0 rises even while uno sleeps in WAIT_TIME"
    );
    assert_eq!(
        kernel.board_global("uno", "cyclic_ticks"),
        Some(&Value::Dint(10)),
        "uno OB_CYCLIC_INT (25ms) must fire 10 times across 250ms regardless of WAIT_TIME"
    );

    let snap = kernel.snapshot();
    assert_eq!(
        snap.boards
            .get("plc")
            .and_then(|b| b.digital_outputs.get("Qa.0")),
        Some(&true)
    );
}

#[test]
fn cooperative_polling_loop_advances_virtual_clock_and_observes_input_timeline_edge() {
    let toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "PollWaitEdge"

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

[[boards.program.hardware]]
id = "n_led"
label = "LED"
pin = "D13"
kind = "digital_out"

[[boards.program.data]]
id = "n_polls"
name = "polls"
scope = "var"
ty = { kind = "scalar", ty = { name = "dint" } }
initial = "0"

[[boards.program.data]]
id = "n_done"
name = "done"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "false"

[[boards.program.pous]]
id = "n_main"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "while"
id = "n_w"
condition = "NOT BTN"
body = [
  { kind = "assign", id = "n_inc", target = "polls", value = "polls + 1" },
]

[[boards.program.pous.body]]
kind = "call"
id = "n_wr"
callee = "DIGITAL_WRITE"
args = [{ value = "LED" }, { value = "true" }]

[[boards.program.pous.body]]
kind = "assign"
id = "n_d"
target = "done"
value = "true"

[[boards.program.pous.body]]
kind = "wait"
id = "n_end"
duration = "T#10ms"
"#;
    let proj = load_project_str(toml, None).unwrap();
    let mut timeline = InputTimeline::new();
    // Press external button at exact t = 500 us!
    timeline.push_digital(500, "uno", "D2", ElectricalDigitalLevel::High);

    let mut kernel = SimKernel::new(&proj, KernelConfig::default(), timeline).unwrap();
    kernel.run_until(1_000).unwrap();

    assert_eq!(kernel.board_global("uno", "done"), Some(&Value::Bool(true)));
    let polls = match kernel.board_global("uno", "polls") {
        Some(Value::Dint(n)) => *n,
        other => panic!("expected Dint polls, got {other:?}"),
    };
    assert!(
        polls > 100,
        "polling loop must execute honestly over 500us before button press, got {polls}"
    );

    // Verify LED write timestamp occurred right at/after t = 500 us (never before!)
    let led_write_t = kernel
        .trace()
        .iter()
        .find_map(|e| {
            if let TraceEventKind::DigitalWrite { pin, high: true } = &e.kind
                && pin == "D13"
            {
                Some(e.time_us)
            } else {
                None
            }
        })
        .expect("expected LED DigitalWrite event");
    assert!(
        (500..=510).contains(&led_write_t),
        "LED must turn on immediately after t=500us edge, got t={led_write_t}us"
    );
}

#[test]
fn deterministic_replay_is_bit_identical_given_same_project_seed_and_timeline() {
    let proj = load_project_str(TWO_BOARD_PROJECT_TOML, None).unwrap();
    let mut tl = InputTimeline::new();
    tl.push_digital(15_000, "plc", "Ia.0", ElectricalDigitalLevel::High);
    tl.push_release_digital(25_000, "plc", "Ia.0");

    let cfg = KernelConfig {
        seed: 0xDEAD_BEEF_CAFE_BABE,
        ..KernelConfig::default()
    };

    let mut k1 = SimKernel::new(&proj, cfg.clone(), tl.clone()).unwrap();
    k1.run_until(220_000).unwrap();
    let trace1: Vec<TraceEvent> = k1.trace().iter().cloned().collect();
    let snap1 = k1.snapshot();

    let mut k2 = SimKernel::new(&proj, cfg, tl).unwrap();
    k2.run_until(220_000).unwrap();
    let trace2: Vec<TraceEvent> = k2.trace().iter().cloned().collect();
    let snap2 = k2.snapshot();

    assert!(!trace1.is_empty());
    assert_eq!(trace1, trace2, "replay traces must be bit-identical");
    assert_eq!(snap1, snap2, "replay snapshots must be bit-identical");
}

#[test]
fn conflicting_wire_drivers_produce_indeterminate_warning() {
    let toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "BusContention"

[[boards]]
id = "drv_hi"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "plc_scan"
scan_period_us = 10000

[[boards.program.hardware]]
id = "n_o1"
label = "OUT1"
pin = "D4"
kind = "digital_out"

[[boards.program.pous]]
id = "n_m1"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "call"
id = "n_w1"
callee = "DIGITAL_WRITE"
args = [{ value = "OUT1" }, { value = "true" }]

[[boards]]
id = "drv_lo"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "plc_scan"
scan_period_us = 10000

[[boards.program.hardware]]
id = "n_o2"
label = "OUT2"
pin = "D4"
kind = "digital_out"

[[boards.program.pous]]
id = "n_m2"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "call"
id = "n_w2"
callee = "DIGITAL_WRITE"
args = [{ value = "OUT2" }, { value = "false" }]

[[boards]]
id = "reader"
profile = "siemens-s7-1200-cpu-1214c"
authoring_language = "st"
execution = "plc_scan"
scan_period_us = 10000

[[boards.program.hardware]]
id = "n_r_in"
label = "BUS_IN"
pin = "Ia.0"
kind = "digital_in"

[[boards.program.data]]
id = "n_r_v"
name = "sampled"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "false"

[[boards.program.pous]]
id = "n_m3"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_a3"
target = "sampled"
value = "BUS_IN"

[[wires]]
id = "w1"
from = "drv_hi.D4"
to = "reader.Ia.0"
signal = "digital"

[[wires]]
id = "w2"
from = "drv_lo.D4"
to = "reader.Ia.0"
signal = "digital"
"#;
    let proj = load_project_str(toml, None).unwrap();
    let mut kernel = SimKernel::new(&proj, KernelConfig::default(), InputTimeline::new()).unwrap();
    kernel.run_until(20_000).unwrap();

    assert!(
        kernel.trace().iter().any(|e| matches!(
            &e.kind,
            TraceEventKind::Diagnostic {
                code: ErrorCode::WarnIndeterminateLevel,
                ..
            }
        )),
        "bus contention between HIGH and LOW drivers must emit WARN_INDETERMINATE_LEVEL"
    );
}

#[test]
fn background_sim_worker_publishes_immutable_snapshots_and_honors_pause() {
    let proj = load_project_str(TWO_BOARD_PROJECT_TOML, None).unwrap();
    let kernel = SimKernel::new(&proj, KernelConfig::default(), InputTimeline::new()).unwrap();
    let worker = SimWorker::spawn(kernel).unwrap();

    // Step forward 150 ms asynchronously
    worker.send(WorkerCommand::StepDelta(150_000)).unwrap();
    let snap1 = worker
        .recv_snapshot_timeout(Duration::from_secs(2))
        .expect("worker must publish snapshot after StepDelta");
    assert_eq!(snap1.time_us, 150_000);
    assert_eq!(
        snap1
            .boards
            .get("plc")
            .and_then(|b| b.digital_outputs.get("Qa.0")),
        Some(&true)
    );

    // Pause -> StepDelta MUST be ignored cleanly
    worker.send(WorkerCommand::Pause).unwrap();
    let _ = worker.recv_snapshot_timeout(Duration::from_secs(2));
    assert_eq!(worker.state(), WorkerState::Paused);
    worker.send(WorkerCommand::StepDelta(50_000)).unwrap();

    // Inject stimulus while paused, then Resume and step to 200ms
    worker
        .send(WorkerCommand::InjectStimulus(StimulusEvent {
            time_us: 160_000,
            instance: "uno".to_owned(),
            port: "D2".to_owned(),
            kind: StimulusKind::Digital(ElectricalDigitalLevel::High),
        }))
        .unwrap();
    let _ = worker.recv_snapshot_timeout(Duration::from_secs(2));
    assert_eq!(worker.latest_snapshot().time_us, 150_000);

    worker.send(WorkerCommand::Resume).unwrap();
    let _ = worker.recv_snapshot_timeout(Duration::from_secs(2));
    worker.send(WorkerCommand::StepUntil(200_000)).unwrap();
    let snap2 = worker
        .recv_snapshot_timeout(Duration::from_secs(2))
        .expect("worker must publish snapshot after Resume + StepUntil");
    assert_eq!(snap2.time_us, 200_000);

    worker.shutdown();
}

#[test]
fn isr_cascade_overflow_halts_oscillating_interrupt_loop() {
    let toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "IsrCascade"

[[boards]]
id = "a"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[[boards.program.hardware]]
id = "n_a_in"
label = "IN_A"
pin = "D2"
kind = "digital_in"

[[boards.program.hardware]]
id = "n_a_out"
label = "OUT_A"
pin = "D8"
kind = "digital_out"

[[boards.program.data]]
id = "n_a_v"
name = "st_a"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "true"

[[boards.program.pous]]
id = "n_a_m"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "call"
id = "n_a_kick"
callee = "DIGITAL_WRITE"
args = [{ value = "OUT_A" }, { value = "true" }]

[[boards.program.pous.body]]
kind = "wait"
id = "n_a_w"
duration = "T#10ms"

[[boards.program.pous]]
id = "n_a_isr"
name = "OnChangeA"
kind = { pou_kind = "ob", ob = { type = "hw_int", pin_label = "IN_A", edge = "change", priority = 0 } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_a_tog"
target = "st_a"
value = "NOT st_a"

[[boards.program.pous.body]]
kind = "call"
id = "n_a_wr"
callee = "DIGITAL_WRITE"
args = [{ value = "OUT_A" }, { value = "st_a" }]

[[boards]]
id = "b"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[[boards.program.hardware]]
id = "n_b_in"
label = "IN_B"
pin = "D2"
kind = "digital_in"

[[boards.program.hardware]]
id = "n_b_out"
label = "OUT_B"
pin = "D8"
kind = "digital_out"

[[boards.program.data]]
id = "n_b_v"
name = "st_b"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "false"

[[boards.program.pous]]
id = "n_b_m"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous]]
id = "n_b_isr"
name = "OnChangeB"
kind = { pou_kind = "ob", ob = { type = "hw_int", pin_label = "IN_B", edge = "change", priority = 0 } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_b_tog"
target = "st_b"
value = "NOT st_b"

[[boards.program.pous.body]]
kind = "call"
id = "n_b_wr"
callee = "DIGITAL_WRITE"
args = [{ value = "OUT_B" }, { value = "st_b" }]

[[wires]]
id = "w_ab"
from = "a.D8"
to = "b.D2"
signal = "digital"

[[wires]]
id = "w_ba"
from = "b.D8"
to = "a.D2"
signal = "digital"
"#;
    let proj = load_project_str(toml, None).unwrap();
    let cfg = KernelConfig {
        max_isr_cascade: 8,
        ..KernelConfig::default()
    };
    let mut kernel = SimKernel::new(&proj, cfg, InputTimeline::new()).unwrap();
    let err = kernel.run_until(1_000).unwrap_err();
    assert!(matches!(
        err,
        dendri_sim_kernel::KernelError::IsrCascadeOverflow { limit: 8, .. }
    ));
}

#[test]
fn comm_event_ob50_propagates_output_across_wires() {
    let toml = r#"
[project]
schema_version = "2.0"
asl_version = "0.1.0"
requires = ["asl-core@1"]
name = "Ob50Wire"

[[boards]]
id = "plc"
profile = "siemens-s7-1200-cpu-1214c"
authoring_language = "st"
execution = "plc_scan"
scan_period_us = 10000

[[boards.program.hardware]]
id = "n_p_out"
label = "NOTIFY"
pin = "Qa.0"
kind = "digital_out"

[[boards.program.pous]]
id = "n_p_m"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous]]
id = "n_p_comm"
name = "OnUartRx"
kind = { pou_kind = "ob", ob = { type = "comm_event", bus_id = "uart0" } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "call"
id = "n_p_wr"
callee = "DIGITAL_WRITE"
args = [{ value = "NOTIFY" }, { value = "true" }]

[[boards]]
id = "uno"
profile = "arduino-uno-r3"
authoring_language = "cpp"
execution = "cooperative"

[[boards.program.hardware]]
id = "n_u_in"
label = "TRIG"
pin = "D2"
kind = "digital_in"

[[boards.program.data]]
id = "n_u_flag"
name = "notified"
scope = "var"
ty = { kind = "scalar", ty = { name = "bool" } }
initial = "false"

[[boards.program.pous]]
id = "n_u_m"
name = "OB_MAIN"
kind = { pou_kind = "ob", ob = { type = "main" } }
body_kind = "statements"

[[boards.program.pous]]
id = "n_u_hw"
name = "OnTrig"
kind = { pou_kind = "ob", ob = { type = "hw_int", pin_label = "TRIG", edge = "rising", priority = 0 } }
body_kind = "statements"

[[boards.program.pous.body]]
kind = "assign"
id = "n_u_set"
target = "notified"
value = "true"

[[wires]]
id = "w_notify"
from = "plc.Qa.0"
to = "uno.D2"
signal = "digital"
"#;
    let proj = load_project_str(toml, None).unwrap();
    let mut kernel = SimKernel::new(&proj, KernelConfig::default(), InputTimeline::new()).unwrap();
    kernel.boot().unwrap();
    assert_eq!(
        kernel.board_global("uno", "notified"),
        Some(&Value::Bool(false))
    );

    // Dispatch OB50 on PLC -> drives Qa.0=true -> wire triggers Uno's OB40 -> notified=true!
    kernel.dispatch_comm_event("plc", "uart0").unwrap();
    assert_eq!(
        kernel.board_global("uno", "notified"),
        Some(&Value::Bool(true))
    );
}
