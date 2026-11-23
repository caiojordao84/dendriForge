//! Cooperative multi-board & PLC-scan virtual machine executor (`02` §2).

use crate::io::{AnalogReading, BoardIoInterface, ElectricalDigitalLevel};
use crate::lower::{CompiledBoard, CompiledSfcChart, Instr, compile_board};
use crate::value::{Value, eval_binary, eval_stdlib_math, eval_unary};
use crate::{IndeterminatePolicy, RuntimeError, TraceEvent, TraceEventKind, TraceRing, VmConfig};
use dendri_ast::{
    ASL_ANALOG_FULL_SCALE, ActionQualifier, Assumption, AssumptionPolicy, BinaryOp, Board, CallArg,
    CasePattern, Expr, ExprKind, FbKind, HardwareKind, InterruptEdge, LValue, LadderCoilKind,
    LadderCompareOp, LadderContactKind, Literal, NodeId, TypeRef,
};
use dendri_span::ErrorCode;
use std::collections::{BTreeMap, BTreeSet};

/// Result of executing one cooperative step quantum on a [`BoardVm`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOutcome {
    /// Board completed its current OB/scan cleanly and is ready for the next scan.
    Completed,
    /// Cooperative fiber yielded at `WAIT_TIME` / `WAIT_TIME_US` until `wake_us` (`wake_cycles`).
    YieldWait { wake_us: u64, wake_cycles: u64 },
    /// Cooperative fiber reached the requested time slice boundary (`stop_at_us`) so the kernel
    /// can deliver external/electrical events before resuming (ensures polling loops observe edges).
    TimeSliceReached { now_us: u64 },
    /// Cooperative fiber changed an external output pin at `now_us` so the kernel can propagate
    /// electrical state and dispatch `OB_HW_INT` at the exact microsecond of the transition.
    IoOutputChanged { now_us: u64 },
}

/// Active call-stack frame preserving exact lexical scoping and cooperative PC cursor.
#[derive(Debug, Clone)]
struct ExecFrame {
    pou_idx: usize,
    pc: usize,
    locals: BTreeMap<String, Value>,
    rails: Vec<bool>,
}

/// Runtime state for one in-memory lowered GRAFCET/SFC chart (`04` §5.2).
#[derive(Debug, Clone, Default)]
struct SfcRuntimeState {
    initialized: bool,
    active_steps: BTreeSet<String>,
    prev_active_steps: BTreeSet<String>,
    step_elapsed_us: BTreeMap<String, i64>,
    n_owned_coils: BTreeSet<String>,
    s_latched_coils: BTreeSet<String>,
}

/// Deterministic single-board ASL v2 virtual machine.
#[derive(Debug, Clone)]
pub struct BoardVm {
    compiled: CompiledBoard,
    config: VmConfig,
    /// Local cooperative clock in CPU cycles (`02` §2.1, D2 — remainder NEVER discarded).
    fiber_cycles: u64,
    /// Global variables, constants, and stateful Function Block instances.
    globals: BTreeMap<String, Value>,
    /// Last resolved boolean per digital input pin (for `IndeterminatePolicy::Previous`).
    last_digital_in: BTreeMap<String, bool>,
    /// Previous boolean per Ladder edge-detection slot (`EdgeReg`).
    edge_slots: Vec<bool>,
    /// Persistent per-chart SFC runtime state (zero `__sfc_*` globals in ASL!).
    sfc_states: Vec<SfcRuntimeState>,
    /// Active cooperative fiber call stack.
    stack: Vec<ExecFrame>,
    /// Whether `OB_STARTUP` (and topological module initialization) has already executed.
    started: bool,
    /// Explicit simulation assumptions keyed by `(NodeId, expr_ordinal)`.
    assumptions: BTreeMap<(NodeId, Option<u32>), Assumption>,
    /// Bounded ring buffer of deterministic trace events.
    trace: TraceRing,
    /// Process Image Inputs (used during `PlcScan`).
    pii_digital: BTreeMap<String, bool>,
    pii_analog: BTreeMap<String, AnalogReading>,
    /// Process Image Outputs buffered during `PlcScan` and committed at Phase 4.
    piq_digital: BTreeMap<String, bool>,
    piq_analog: BTreeMap<String, i32>,
    piq_pwm: BTreeMap<String, u32>,
    /// True while inside `run_plc_scan` so reads/writes route through PII/PIQ.
    in_plc_scan: bool,
    /// Current scan delta time in microseconds for IEC timers.
    current_dt_us: i64,
    /// Tracks which FB instance names already received `current_dt_us` in this scan.
    fb_ticked_this_scan: BTreeSet<String>,
    /// Remaining instruction budget for the active scan/resume quantum.
    budget_remaining: usize,
    /// Optional cooperative time-slice deadline (`stop_at_us`) set by `SimKernel`.
    coop_stop_at_us: Option<u64>,
    /// If true, cooperative execution yields immediately when a pin output changes state.
    yield_on_io_write: bool,
    /// Flag set during an instruction when an output write occurred in cooperative mode.
    io_write_occurred: bool,
    /// Active SFC chart index during receptivity/action expression evaluation (`<Step>.X`/`.T`).
    active_sfc_eval_chart: Option<usize>,
    /// Frozen pre-evolution active steps snapshot for Rule 5 receptivity evaluation.
    frozen_sfc_steps: Option<BTreeSet<String>>,
}

impl BoardVm {
    /// Compiles a validated [`Board`] at load time into an executable [`BoardVm`].
    pub fn new(board: &Board, config: VmConfig) -> Result<Self, RuntimeError> {
        if config.clock_hz == 0 {
            return Err(RuntimeError::InvalidClockHz);
        }
        let compiled = compile_board(board)?;
        let edge_slots = vec![false; usize::try_from(compiled.edge_slot_count).unwrap_or(0)];
        let sfc_states = vec![SfcRuntimeState::default(); compiled.sfc_charts.len()];
        let trace = TraceRing::with_capacity(config.max_trace_events);

        let mut assumptions = BTreeMap::new();
        for a in &board.assumptions {
            assumptions.insert((a.node.clone(), a.expr_ordinal), a.clone());
        }

        let mut vm = Self {
            compiled,
            config,
            fiber_cycles: 0,
            globals: BTreeMap::new(),
            last_digital_in: BTreeMap::new(),
            edge_slots,
            sfc_states,
            stack: Vec::new(),
            started: false,
            assumptions,
            trace,
            pii_digital: BTreeMap::new(),
            pii_analog: BTreeMap::new(),
            piq_digital: BTreeMap::new(),
            piq_analog: BTreeMap::new(),
            piq_pwm: BTreeMap::new(),
            in_plc_scan: false,
            current_dt_us: 0,
            fb_ticked_this_scan: BTreeSet::new(),
            budget_remaining: 0,
            coop_stop_at_us: None,
            yield_on_io_write: false,
            io_write_occurred: false,
            active_sfc_eval_chart: None,
            frozen_sfc_steps: None,
        };

        // Pre-populate zero/default values for all globals in topological order
        let data_list = vm.compiled.ordered_data.clone();
        for d in &data_list {
            let def = Value::default_for_type(
                &d.ty,
                &vm.compiled.struct_defs,
                &vm.compiled.user_fb_fields,
            );
            vm.globals.insert(d.name.clone(), def);
        }

        Ok(vm)
    }

    /// Board instance ID.
    #[must_use]
    pub fn board_id(&self) -> &str {
        &self.compiled.board_id
    }

    /// Current cycle counter of this board's virtual CPU clock.
    #[must_use]
    pub const fn fiber_cycles(&self) -> u64 {
        self.fiber_cycles
    }

    /// Exact microsecond virtual time derived in 128-bit arithmetic: `floor(fiber_cycles * 10^6 / clock_hz)`.
    #[must_use]
    pub fn time_us(&self) -> u64 {
        let us = (u128::from(self.fiber_cycles) * 1_000_000) / u128::from(self.config.clock_hz);
        u64::try_from(us).unwrap_or(u64::MAX)
    }

    /// Computes exact wake cycle count for `WAIT_TIME_US(delay_us)`: `cycles_now + ceil(d * clock_hz / 10^6)`.
    /// Never wakes early (`02` §2.1, D2).
    #[must_use]
    pub fn wake_cycles_for_wait_us(&self, delay_us: u64) -> u64 {
        let num = u128::from(delay_us) * u128::from(self.config.clock_hz);
        let delta = num.saturating_add(999_999) / 1_000_000;
        self.fiber_cycles
            .saturating_add(u64::try_from(delta).unwrap_or(u64::MAX))
    }

    /// Advances `fiber_cycles` forward to at least `wake_cycles`, preserving exact sub-microsecond
    /// cycle remainder across `WAIT_TIME` / `WAIT_TIME_US` resumes (`02` §2.1, D2).
    pub fn resume_at_cycles(&mut self, wake_cycles: u64) {
        self.fiber_cycles = self.fiber_cycles.max(wake_cycles);
    }

    /// Advances `fiber_cycles` forward to at least `global_time_us` (when waking from idle/wait).
    pub fn sync_to_time_us(&mut self, global_time_us: u64) {
        if self.time_us() < global_time_us {
            let num = u128::from(global_time_us) * u128::from(self.config.clock_hz);
            let target_cycles = num.saturating_add(999_999) / 1_000_000;
            self.fiber_cycles = self
                .fiber_cycles
                .max(u64::try_from(target_cycles).unwrap_or(u64::MAX));
        }
    }

    /// Reads a global variable value by name (for inspection & tests).
    #[must_use]
    pub fn get_global(&self, name: &str) -> Option<&Value> {
        self.globals.get(name)
    }

    /// Returns the bounded deterministic trace ring.
    #[must_use]
    pub const fn trace(&self) -> &TraceRing {
        &self.trace
    }

    /// Returns registered cyclic interrupt periods `(pou_idx, period_us)`.
    #[must_use]
    pub fn cyclic_interrupts(&self) -> &[(usize, u64)] {
        &self.compiled.cyclic_ints
    }

    /// Returns registered hardware interrupts `(pou_idx, pin_label, edge, priority)`.
    #[must_use]
    pub fn hardware_interrupts(&self) -> &[(usize, String, InterruptEdge, u8)] {
        &self.compiled.hw_ints
    }

    /// Returns registered communication event OBs `(pou_idx, bus_id)` (`OB50`).
    #[must_use]
    pub fn comm_events(&self) -> &[(usize, String)] {
        &self.compiled.comm_events
    }

    /// Returns all hardware bindings declared on this board keyed by label.
    #[must_use]
    pub const fn hardware_bindings(&self) -> &BTreeMap<String, dendri_ast::HardwareBinding> {
        &self.compiled.hardware
    }

    /// Maps a board hardware label to its physical port ID (e.g., `"BTN"` -> `"D2"`).
    #[must_use]
    pub fn pin_for_label(&self, label: &str) -> Option<&str> {
        self.compiled.hardware.get(label).map(|h| h.pin.as_str())
    }

    /// Returns active step names of a named SFC chart (for visualization & tests).
    #[must_use]
    pub fn active_sfc_steps(&self, chart_name: &str) -> Option<&BTreeSet<String>> {
        let &idx = self.compiled.sfc_chart_by_name.get(chart_name)?;
        self.sfc_states.get(idx).map(|s| &s.active_steps)
    }

    /// Returns a snapshot of active steps for every SFC chart on this board (`chart_name -> steps`).
    #[must_use]
    pub fn all_active_sfc_steps(&self) -> BTreeMap<String, BTreeSet<String>> {
        let mut out = BTreeMap::new();
        for (name, &idx) in &self.compiled.sfc_chart_by_name {
            if let Some(st) = self.sfc_states.get(idx) {
                out.insert(name.clone(), st.active_steps.clone());
            }
        }
        out
    }

    /// Runs topological module data initializers and `OB_STARTUP` once at boot (`t = 0`).
    pub fn boot(&mut self, io: &mut dyn BoardIoInterface) -> Result<(), RuntimeError> {
        if self.started {
            return Ok(());
        }
        self.started = true;
        self.budget_remaining = self.config.max_steps_per_scan;

        // 1. Evaluate data initializers in topological module dependency order (`01` §6.1)
        let decls = self.compiled.ordered_data.clone();
        for d in &decls {
            if let Some(init_expr) = &d.initial {
                let raw_val = self.eval_expr(init_expr, io, 0)?;
                let (coerced, duty_clamped) = raw_val.coerce_to_type(&d.ty)?;
                if duty_clamped {
                    self.record_diag(
                        ErrorCode::WarnDutyClamped,
                        format!("DUTY initializer for `{}` clamped to [0, M]", d.name),
                    );
                }
                self.globals.insert(d.name.clone(), coerced);
            }
        }

        // 2. Execute `OB_STARTUP` (`OB100`) if present
        if let Some(startup_idx) = self.compiled.startup_pou {
            self.push_frame(startup_idx, BTreeMap::new())?;
            let _ = self.run_stack_until_yield_or_done(io)?;
        }
        Ok(())
    }

    /// Executes one cooperative quantum on `OB_MAIN` (resuming from exact yield PC if previously suspended).
    pub fn step_cooperative(
        &mut self,
        dt_us: i64,
        io: &mut dyn BoardIoInterface,
    ) -> Result<StepOutcome, RuntimeError> {
        self.step_cooperative_until(None, false, dt_us, io)
    }

    /// Executes one cooperative quantum on `OB_MAIN` with an optional microsecond deadline
    /// (`stop_at_us`) and optional immediate yield on output pin writes (`yield_on_io_write`).
    ///
    /// Used by `dendri-sim-kernel` so polling loops (`while (!digitalRead(2));`) advance virtual
    /// time honestly and yield cleanly to observe external/electrical events (`02` §2.1).
    pub fn step_cooperative_until(
        &mut self,
        stop_at_us: Option<u64>,
        yield_on_io_write: bool,
        dt_us: i64,
        io: &mut dyn BoardIoInterface,
    ) -> Result<StepOutcome, RuntimeError> {
        if !self.started {
            self.boot(io)?;
        }
        self.in_plc_scan = false;
        self.coop_stop_at_us = stop_at_us;
        self.yield_on_io_write = yield_on_io_write;
        self.io_write_occurred = false;
        self.current_dt_us = dt_us.max(0);
        self.fb_ticked_this_scan.clear();
        self.budget_remaining = self.config.max_steps_per_scan;

        if self.stack.is_empty()
            && let Some(main_idx) = self.compiled.main_pou
        {
            self.push_frame(main_idx, BTreeMap::new())?;
        }

        let outcome = self.run_stack_until_yield_or_done(io);
        self.coop_stop_at_us = None;
        self.yield_on_io_write = false;
        let outcome = outcome?;
        self.advance_unticked_timers(self.current_dt_us);
        Ok(outcome)
    }

    /// Executes one complete 4-phase PLC scan cycle (`02` §2.2):
    /// 1. Sample Process Image Inputs (PII)
    /// 2. Advance IEC timers (`dt_us`)
    /// 3. Execute `OB_MAIN` to completion (fresh per-scan instruction budget)
    /// 4. Commit Process Image Outputs (PIQ)
    pub fn run_plc_scan(
        &mut self,
        dt_us: u64,
        io: &mut dyn BoardIoInterface,
    ) -> Result<(), RuntimeError> {
        if !self.started {
            self.boot(io)?;
        }
        self.in_plc_scan = true;
        self.coop_stop_at_us = None;
        self.yield_on_io_write = false;
        self.current_dt_us = i64::try_from(dt_us).unwrap_or(i64::MAX);
        self.fb_ticked_this_scan.clear();
        self.budget_remaining = self.config.max_steps_per_scan;

        // Advance board virtual clock by exact scan period (without drifting from instruction counts)
        if dt_us > 0 {
            let target_us = self.time_us().saturating_add(dt_us);
            self.sync_to_time_us(target_us);
        }

        // Phase 1: Sample Process Image Inputs (PII)
        self.pii_digital.clear();
        self.pii_analog.clear();
        let hw_list: Vec<dendri_ast::HardwareBinding> =
            self.compiled.hardware.values().cloned().collect();
        for hw in &hw_list {
            match hw.kind {
                HardwareKind::DigitalIn | HardwareKind::DigitalBidirectional => {
                    let level = io.read_digital(&hw.pin);
                    let resolved = self.resolve_digital_level(&hw.pin, level);
                    self.pii_digital.insert(hw.pin.clone(), resolved);
                }
                HardwareKind::AnalogIn => {
                    let reading = io.read_analog(&hw.pin);
                    self.pii_analog.insert(hw.pin.clone(), reading);
                }
                HardwareKind::DigitalOut
                | HardwareKind::AnalogOut
                | HardwareKind::PwmOut
                | HardwareKind::BusPin => {}
            }
        }

        // Phase 2 & 3: Execute `OB_MAIN` to completion
        self.piq_digital.clear();
        self.piq_analog.clear();
        self.piq_pwm.clear();

        if let Some(main_idx) = self.compiled.main_pou {
            self.push_frame(main_idx, BTreeMap::new())?;
            let _ = self.run_stack_until_yield_or_done(io)?;
        }

        // Advance any active IEC timers that were not explicitly invoked during this scan
        self.advance_unticked_timers(self.current_dt_us);

        // Phase 4: Commit Process Image Outputs (PIQ)
        self.in_plc_scan = false;
        let t = self.time_us();
        let bid = self.compiled.board_id.clone();

        let dig_outs: Vec<(String, bool)> = self
            .piq_digital
            .iter()
            .map(|(k, &v)| (k.clone(), v))
            .collect();
        for (pin, high) in dig_outs {
            io.write_digital(&pin, high);
            self.trace.push(TraceEvent {
                time_us: t,
                board_id: bid.clone(),
                kind: TraceEventKind::DigitalWrite { pin, high },
            });
        }
        let ana_outs: Vec<(String, i32)> = self
            .piq_analog
            .iter()
            .map(|(k, &v)| (k.clone(), v))
            .collect();
        for (pin, analog) in ana_outs {
            io.write_analog(&pin, analog);
            self.trace.push(TraceEvent {
                time_us: t,
                board_id: bid.clone(),
                kind: TraceEventKind::AnalogWrite { pin, analog },
            });
        }
        let pwm_outs: Vec<(String, u32)> =
            self.piq_pwm.iter().map(|(k, &v)| (k.clone(), v)).collect();
        for (pin, duty) in pwm_outs {
            io.write_pwm(&pin, duty);
            self.trace.push(TraceEvent {
                time_us: t,
                board_id: bid.clone(),
                kind: TraceEventKind::PwmWrite { pin, duty },
            });
        }

        Ok(())
    }

    /// Executes an interrupt Organization Block (`OB_HW_INT`, `OB_CYCLIC_INT`, `OB_COMM_EVENT`)
    /// to completion without clobbering any suspended cooperative `OB_MAIN` stack!
    pub fn run_interrupt_ob(
        &mut self,
        pou_idx: usize,
        io: &mut dyn BoardIoInterface,
    ) -> Result<(), RuntimeError> {
        if !self.started {
            self.boot(io)?;
        }
        let saved_stack = std::mem::take(&mut self.stack);
        let saved_budget = self.budget_remaining;
        self.budget_remaining = self.config.max_steps_per_scan;

        let res = (|| -> Result<(), RuntimeError> {
            self.push_frame(pou_idx, BTreeMap::new())?;
            let _ = self.run_stack_until_yield_or_done(io)?;
            Ok(())
        })();

        self.stack = saved_stack;
        self.budget_remaining = saved_budget;
        res
    }

    /// Executes all `OB_COMM_EVENT` (`OB50`) handlers bound to `bus_id`.
    pub fn run_comm_event_obs(
        &mut self,
        bus_id: &str,
        io: &mut dyn BoardIoInterface,
    ) -> Result<(), RuntimeError> {
        let handlers: Vec<usize> = self
            .compiled
            .comm_events
            .iter()
            .filter(|(_, bid)| bid == bus_id)
            .map(|&(idx, _)| idx)
            .collect();
        for pou_idx in handlers {
            self.run_interrupt_ob(pou_idx, io)?;
        }
        Ok(())
    }

    fn advance_unticked_timers(&mut self, dt_us: i64) {
        if dt_us <= 0 {
            return;
        }
        for (name, val) in &mut self.globals {
            if !self.fb_ticked_this_scan.contains(name)
                && let Value::Fb(fb) = val
            {
                fb.advance_time_us(dt_us);
            }
        }
    }

    fn push_frame(
        &mut self,
        pou_idx: usize,
        mut initial_locals: BTreeMap<String, Value>,
    ) -> Result<(), RuntimeError> {
        let Some(pou) = self.compiled.pous.get(pou_idx) else {
            return Err(RuntimeError::UnresolvedFunction(format!("pou#{pou_idx}")));
        };
        if self.stack.len() >= self.config.max_call_depth {
            return Err(RuntimeError::StackOverflow {
                callee: pou.name.clone(),
                limit: self.config.max_call_depth,
            });
        }
        for local in &pou.locals {
            initial_locals.entry(local.name.clone()).or_insert_with(|| {
                Value::default_for_type(
                    &local.ty,
                    &self.compiled.struct_defs,
                    &self.compiled.user_fb_fields,
                )
            });
        }
        let rail_len = usize::try_from(pou.rail_count).unwrap_or(0);
        self.stack.push(ExecFrame {
            pou_idx,
            pc: 0,
            locals: initial_locals,
            rails: vec![false; rail_len],
        });
        Ok(())
    }

    fn charge_step(&mut self, cycles: u64) -> Result<(), RuntimeError> {
        if self.budget_remaining == 0 {
            return Err(RuntimeError::ScanBudgetExceeded {
                budget: self.config.max_steps_per_scan,
            });
        }
        self.budget_remaining = self.budget_remaining.saturating_sub(1);
        if !self.in_plc_scan {
            self.fiber_cycles = self.fiber_cycles.saturating_add(cycles);
        }
        Ok(())
    }

    fn run_stack_until_yield_or_done(
        &mut self,
        io: &mut dyn BoardIoInterface,
    ) -> Result<StepOutcome, RuntimeError> {
        while let Some(frame_top) = self.stack.last() {
            let pou_idx = frame_top.pou_idx;
            let pc = frame_top.pc;
            let instr_opt = self
                .compiled
                .pous
                .get(pou_idx)
                .and_then(|p| p.instrs.get(pc))
                .cloned();

            let Some(instr) = instr_opt else {
                self.stack.pop();
                continue;
            };

            // Advance PC optimistically; jumps/branches overwrite it cleanly
            if let Some(frame) = self.stack.last_mut() {
                frame.pc = pc.saturating_add(1);
            }

            if let Some(outcome) = self.exec_instr(&instr, io)? {
                return Ok(outcome);
            }

            if self.yield_on_io_write && self.io_write_occurred {
                self.io_write_occurred = false;
                return Ok(StepOutcome::IoOutputChanged {
                    now_us: self.time_us(),
                });
            }

            if let Some(limit_us) = self.coop_stop_at_us {
                let now_us = self.time_us();
                if now_us >= limit_us {
                    return Ok(StepOutcome::TimeSliceReached { now_us });
                }
            }
        }
        Ok(StepOutcome::Completed)
    }

    fn exec_instr(
        &mut self,
        instr: &Instr,
        io: &mut dyn BoardIoInterface,
    ) -> Result<Option<StepOutcome>, RuntimeError> {
        match instr {
            Instr::Assign { target, value } => {
                self.charge_step(self.config.cost.stmt_cycles)?;
                let val = self.eval_expr(value, io, 0)?;
                self.write_lvalue(target, val, io)?;
            }
            Instr::CallStmt { callee, args } => {
                self.charge_step(self.config.cost.call_cycles)?;
                if let Some(&target_pou_idx) = self.compiled.pou_by_name.get(callee) {
                    let params = self
                        .compiled
                        .pous
                        .get(target_pou_idx)
                        .map(|p| p.params.clone())
                        .unwrap_or_default();
                    let bound = self.bind_call_args(&params, args, io, 0)?;
                    self.push_frame(target_pou_idx, bound)?;
                } else if callee == "WAIT_TIME" {
                    let ms = args.first().ok_or_else(|| {
                        RuntimeError::TypeError("WAIT_TIME expects ms".to_owned())
                    })?;
                    let ms_val = self.eval_expr(&ms.value, io, 0)?.as_i128()?;
                    let delay_us = u64::try_from(ms_val.max(0))
                        .unwrap_or(0)
                        .saturating_mul(1_000);
                    let wake_cycles = self.wake_cycles_for_wait_us(delay_us);
                    let wake_us = u64::try_from(
                        (u128::from(wake_cycles) * 1_000_000) / u128::from(self.config.clock_hz),
                    )
                    .unwrap_or(u64::MAX);
                    return Ok(Some(StepOutcome::YieldWait {
                        wake_us,
                        wake_cycles,
                    }));
                } else if callee == "WAIT_TIME_US" {
                    let us = args.first().ok_or_else(|| {
                        RuntimeError::TypeError("WAIT_TIME_US expects us".to_owned())
                    })?;
                    let us_val = self.eval_expr(&us.value, io, 0)?.as_i128()?;
                    let delay_us = u64::try_from(us_val.max(0)).unwrap_or(0);
                    let wake_cycles = self.wake_cycles_for_wait_us(delay_us);
                    let wake_us = u64::try_from(
                        (u128::from(wake_cycles) * 1_000_000) / u128::from(self.config.clock_hz),
                    )
                    .unwrap_or(u64::MAX);
                    return Ok(Some(StepOutcome::YieldWait {
                        wake_us,
                        wake_cycles,
                    }));
                } else {
                    let _ = self.eval_builtin_call(callee, args, io, 0)?;
                }
            }
            Instr::FbInvoke { instance, inputs } => {
                self.charge_step(self.config.cost.call_cycles)?;
                let mut eval_inputs = BTreeMap::new();
                for (name, expr) in inputs {
                    eval_inputs.insert(name.clone(), self.eval_expr(expr, io, 0)?);
                }
                self.invoke_fb_lvalue(instance, &eval_inputs, io)?;
            }
            Instr::Jump(target) => {
                self.charge_step(self.config.cost.branch_cycles)?;
                if let Some(frame) = self.stack.last_mut() {
                    frame.pc = *target;
                }
            }
            Instr::JumpIfFalse { condition, target } => {
                self.charge_step(self.config.cost.branch_cycles)?;
                let cond = self.eval_expr(condition, io, 0)?.as_bool()?;
                if !cond && let Some(frame) = self.stack.last_mut() {
                    frame.pc = *target;
                }
            }
            Instr::Switch {
                scrutinee,
                arms,
                default_target,
            } => {
                self.charge_step(self.config.cost.branch_cycles)?;
                let scrut = self.eval_expr(scrutinee, io, 0)?;
                let mut dest = *default_target;
                'arm_search: for (patterns, target) in arms {
                    for pat in patterns {
                        match pat {
                            CasePattern::Value(e) => {
                                let v = self.eval_expr(e, io, 0)?;
                                let eq = eval_binary(
                                    BinaryOp::Eq,
                                    scrut.clone(),
                                    v,
                                    self.config.overflow,
                                )?
                                .as_bool()?;
                                if eq {
                                    dest = *target;
                                    break 'arm_search;
                                }
                            }
                            CasePattern::Range { start, end } => {
                                let lo = self.eval_expr(start, io, 0)?;
                                let hi = self.eval_expr(end, io, 0)?;
                                let ge = eval_binary(
                                    BinaryOp::Ge,
                                    scrut.clone(),
                                    lo,
                                    self.config.overflow,
                                )?
                                .as_bool()?;
                                let le = eval_binary(
                                    BinaryOp::Le,
                                    scrut.clone(),
                                    hi,
                                    self.config.overflow,
                                )?
                                .as_bool()?;
                                if ge && le {
                                    dest = *target;
                                    break 'arm_search;
                                }
                            }
                        }
                    }
                }
                if let Some(frame) = self.stack.last_mut() {
                    frame.pc = dest;
                }
            }
            Instr::ForInit {
                counter,
                from,
                to,
                step,
                inclusive,
                end_target,
            } => {
                self.charge_step(self.config.cost.branch_cycles)?;
                let start_v = self.eval_expr(from, io, 0)?.as_i128()?;
                let end_v = self.eval_expr(to, io, 0)?.as_i128()?;
                let step_v = self.eval_expr(step, io, 0)?.as_i128()?;
                let template = self.read_var(counter)?.clone();
                let init_val = Value::from_i128_like(&template, start_v)?;
                self.write_var(counter, init_val)?;
                if !for_cond_holds(start_v, end_v, step_v, *inclusive)
                    && let Some(frame) = self.stack.last_mut()
                {
                    frame.pc = *end_target;
                }
            }
            Instr::ForStep {
                counter,
                to,
                step,
                inclusive,
                body_start,
            } => {
                self.charge_step(self.config.cost.branch_cycles)?;
                let cur_val = self.read_var(counter)?.clone();
                let cur = cur_val.as_i128()?;
                let end_v = self.eval_expr(to, io, 0)?.as_i128()?;
                let step_v = self.eval_expr(step, io, 0)?.as_i128()?;
                let next = cur.wrapping_add(step_v);
                let next_val = Value::from_i128_like(&cur_val, next)?;
                self.write_var(counter, next_val)?;
                if for_cond_holds(next, end_v, step_v, *inclusive)
                    && let Some(frame) = self.stack.last_mut()
                {
                    frame.pc = *body_start;
                }
            }
            Instr::Return { value } => {
                self.charge_step(self.config.cost.branch_cycles)?;
                if let Some(expr) = value {
                    let _ = self.eval_expr(expr, io, 0)?;
                }
                self.stack.pop();
            }
            Instr::Wait { duration } => {
                self.charge_step(self.config.cost.stmt_cycles)?;
                let dur_us = self.eval_expr(duration, io, 0)?.as_time_us()?;
                let delay_us = u64::try_from(dur_us.max(0)).unwrap_or(0);
                let wake_cycles = self.wake_cycles_for_wait_us(delay_us);
                let wake_us = u64::try_from(
                    (u128::from(wake_cycles) * 1_000_000) / u128::from(self.config.clock_hz),
                )
                .unwrap_or(u64::MAX);
                return Ok(Some(StepOutcome::YieldWait {
                    wake_us,
                    wake_cycles,
                }));
            }
            Instr::Foreign {
                node_id,
                language,
                fidelity,
                text,
            } => {
                self.charge_step(self.config.cost.stmt_cycles)?;
                if let Some(assumption) = self.assumptions.get(&(node_id.clone(), None)).cloned() {
                    match assumption.policy {
                        AssumptionPolicy::NoEffect
                        | AssumptionPolicy::ReturnValue
                        | AssumptionPolicy::UserModel => {}
                    }
                } else {
                    return Err(RuntimeError::ForeignHalt {
                        node_id: node_id.clone(),
                        language: language.clone(),
                        fidelity: format!("{fidelity:?}"),
                        text: text.clone(),
                    });
                }
            }
            // --- Materialized Ladder instructions ---
            Instr::RailConst { dst, val } => {
                self.charge_step(self.config.cost.stmt_cycles)?;
                self.set_rail(*dst, *val);
            }
            Instr::RailCopy { dst, src } => {
                self.charge_step(self.config.cost.stmt_cycles)?;
                let v = self.get_rail(*src);
                self.set_rail(*dst, v);
            }
            Instr::RailOr { dst, a, b } => {
                self.charge_step(self.config.cost.stmt_cycles)?;
                let v = self.get_rail(*a) || self.get_rail(*b);
                self.set_rail(*dst, v);
            }
            Instr::LadderContact {
                in_rail,
                out_rail,
                kind,
                target,
                edge_slot,
            } => {
                self.charge_step(self.config.cost.io_read_cycles)?;
                let incoming = self.get_rail(*in_rail);
                let bit = self.read_lvalue(target, io)?.as_bool()?;
                let pass = match kind {
                    LadderContactKind::NormallyOpen => bit,
                    LadderContactKind::NormallyClosed => !bit,
                    LadderContactKind::PositiveTransition => {
                        let prev = edge_slot
                            .and_then(|s| self.edge_slots.get(s.0 as usize).copied())
                            .unwrap_or(false);
                        if let Some(s) = edge_slot
                            && let Some(slot) = self.edge_slots.get_mut(s.0 as usize)
                        {
                            *slot = bit;
                        }
                        bit && !prev
                    }
                    LadderContactKind::NegativeTransition => {
                        let prev = edge_slot
                            .and_then(|s| self.edge_slots.get(s.0 as usize).copied())
                            .unwrap_or(false);
                        if let Some(s) = edge_slot
                            && let Some(slot) = self.edge_slots.get_mut(s.0 as usize)
                        {
                            *slot = bit;
                        }
                        !bit && prev
                    }
                };
                self.set_rail(*out_rail, incoming && pass);
            }
            Instr::LadderCoil {
                in_rail,
                out_rail,
                kind,
                target,
                edge_slot,
            } => {
                self.charge_step(self.config.cost.io_write_cycles)?;
                let incoming = self.get_rail(*in_rail);
                match kind {
                    LadderCoilKind::Normal => {
                        // Unconditionally writes BOTH true AND false!
                        self.write_lvalue(target, Value::Bool(incoming), io)?;
                    }
                    LadderCoilKind::Negated => {
                        self.write_lvalue(target, Value::Bool(!incoming), io)?;
                    }
                    LadderCoilKind::Set => {
                        if incoming {
                            self.write_lvalue(target, Value::Bool(true), io)?;
                        }
                    }
                    LadderCoilKind::Reset => {
                        if incoming {
                            self.write_lvalue(target, Value::Bool(false), io)?;
                        }
                    }
                    LadderCoilKind::PositiveTransition => {
                        let prev = edge_slot
                            .and_then(|s| self.edge_slots.get(s.0 as usize).copied())
                            .unwrap_or(false);
                        if let Some(s) = edge_slot
                            && let Some(slot) = self.edge_slots.get_mut(s.0 as usize)
                        {
                            *slot = incoming;
                        }
                        self.write_lvalue(target, Value::Bool(incoming && !prev), io)?;
                    }
                    LadderCoilKind::NegativeTransition => {
                        let prev = edge_slot
                            .and_then(|s| self.edge_slots.get(s.0 as usize).copied())
                            .unwrap_or(false);
                        if let Some(s) = edge_slot
                            && let Some(slot) = self.edge_slots.get_mut(s.0 as usize)
                        {
                            *slot = incoming;
                        }
                        self.write_lvalue(target, Value::Bool(!incoming && prev), io)?;
                    }
                }
                // Preserve pre-mutation materialized rail for downstream elements (`04` §5.1)!
                self.set_rail(*out_rail, incoming);
            }
            Instr::LadderCompare {
                in_rail,
                out_rail,
                op,
                left,
                right,
            } => {
                self.charge_step(self.config.cost.stmt_cycles)?;
                let en = self.get_rail(*in_rail);
                if !en {
                    self.set_rail(*out_rail, false);
                } else {
                    let lv = self.eval_expr(left, io, 0)?;
                    let rv = self.eval_expr(right, io, 0)?;
                    let bin = match op {
                        LadderCompareOp::Eq => BinaryOp::Eq,
                        LadderCompareOp::Ne => BinaryOp::Ne,
                        LadderCompareOp::Lt => BinaryOp::Lt,
                        LadderCompareOp::Le => BinaryOp::Le,
                        LadderCompareOp::Gt => BinaryOp::Gt,
                        LadderCompareOp::Ge => BinaryOp::Ge,
                    };
                    let res = eval_binary(bin, lv, rv, self.config.overflow)?.as_bool()?;
                    self.set_rail(*out_rail, en && res);
                }
            }
            Instr::LadderFbBox {
                in_rail,
                out_rail,
                fb_kind,
                instance,
                inputs,
            } => {
                self.charge_step(self.config.cost.call_cycles)?;
                let incoming = self.get_rail(*in_rail);
                let mut eval_inputs = BTreeMap::new();
                for (name, expr) in inputs {
                    eval_inputs.insert(name.clone(), self.eval_expr(expr, io, 0)?);
                }
                // Drive the primary IEC boolean input pin from the incoming power rail
                let rail_pin = match fb_kind {
                    FbKind::Ton | FbKind::Tof | FbKind::Tp => "IN",
                    FbKind::Ctu | FbKind::Ctud => "CU",
                    FbKind::Ctd => "CD",
                    FbKind::RTrig | FbKind::FTrig => "CLK",
                    FbKind::Sr => "S1",
                    FbKind::Rs => "S",
                    FbKind::UartSend | FbKind::I2cXfer | FbKind::SpiXfer | FbKind::MbClient => {
                        "REQ"
                    }
                    FbKind::UartRecv => "EN_R",
                    FbKind::MbServer => "ENABLE",
                    FbKind::User { .. } => "EN",
                };
                eval_inputs
                    .entry(rail_pin.to_owned())
                    .or_insert(Value::Bool(incoming));

                let out_q = self.invoke_fb_lvalue(instance, &eval_inputs, io)?;
                self.set_rail(*out_rail, out_q);
            }
            Instr::LadderCallBox {
                in_rail,
                out_rail,
                callee,
                args,
                output,
            } => {
                self.charge_step(self.config.cost.call_cycles)?;
                let en = self.get_rail(*in_rail);
                if en {
                    let call_args: Vec<CallArg> = args
                        .iter()
                        .map(|(name, value)| CallArg {
                            name: name.clone(),
                            value: value.clone(),
                            span: value.span,
                        })
                        .collect();
                    let ret = self.eval_call_sync(callee, &call_args, io, 0)?;
                    if let Some(out_lv) = output {
                        self.write_lvalue(out_lv, ret, io)?;
                    }
                }
                self.set_rail(*out_rail, en);
            }
            Instr::RunSfcChart { chart_idx } => {
                self.charge_step(self.config.cost.stmt_cycles)?;
                self.step_sfc_chart(*chart_idx, io)?;
            }
        }
        Ok(None)
    }

    // =========================================================================
    // IEC 60848 GRAFCET / SFC Rule 5 Engine (`04` §5.2)
    // =========================================================================

    fn step_sfc_chart(
        &mut self,
        chart_idx: usize,
        io: &mut dyn BoardIoInterface,
    ) -> Result<(), RuntimeError> {
        let Some(chart) = self.compiled.sfc_charts.get(chart_idx).cloned() else {
            return Ok(());
        };

        // 1. First-scan initialization (including initial macro-step entry on Scan 1!)
        let already_init = self
            .sfc_states
            .get(chart_idx)
            .is_some_and(|s| s.initialized);
        if !already_init {
            let mut initial_macro_subcharts = Vec::new();
            if let Some(st) = self.sfc_states.get_mut(chart_idx) {
                st.initialized = true;
                st.active_steps.clear();
                st.prev_active_steps.clear();
                st.step_elapsed_us.clear();
                for step in &chart.steps {
                    if step.initial {
                        st.active_steps.insert(step.name.clone());
                        st.step_elapsed_us.insert(step.name.clone(), 0);
                        if let Some(sub) = &step.macro_chart {
                            initial_macro_subcharts.push(sub.clone());
                        }
                    }
                }
            }
            for sub in initial_macro_subcharts {
                if let Some(&sub_idx) = self.compiled.sfc_chart_by_name.get(&sub) {
                    self.reset_and_init_sfc_chart(sub_idx);
                }
            }
        } else {
            // Advance elapsed `<Step>.T` timers for currently active steps
            let dt = self.current_dt_us;
            if let Some(st) = self.sfc_states.get_mut(chart_idx) {
                let active_list: Vec<String> = st.active_steps.iter().cloned().collect();
                for s_name in active_list {
                    let entry = st.step_elapsed_us.entry(s_name).or_insert(0);
                    *entry = entry.saturating_add(dt);
                }
            }
        }

        // Snapshot previous scan's active set for P/P1/P0 edge qualifiers
        let scan_start_active = self
            .sfc_states
            .get(chart_idx)
            .map(|s| s.prev_active_steps.clone())
            .unwrap_or_default();

        // 2. IEC 60848 Rule 5 Two-Phase Evolution (+ optional stability loop with cycle guard)
        let max_iters = if chart.evolve_to_stability {
            self.config.max_sfc_stability_steps
        } else {
            1
        };

        let mut iter_count = 0usize;
        loop {
            let frozen_active = self
                .sfc_states
                .get(chart_idx)
                .map(|s| s.active_steps.clone())
                .unwrap_or_default();

            // Phase 1: Evaluate all transitions against FROZEN pre-evolution state with OR priority
            let mut claimed_from_steps: BTreeSet<String> = BTreeSet::new();
            let mut fired_indices: Vec<usize> = Vec::new();

            self.active_sfc_eval_chart = Some(chart_idx);
            self.frozen_sfc_steps = Some(frozen_active.clone());

            for (t_idx, tr) in chart.transitions.iter().enumerate() {
                // AND convergence: ALL from_steps must be active
                let all_from_active = !tr.from_steps.is_empty()
                    && tr.from_steps.iter().all(|s| frozen_active.contains(s));
                if !all_from_active {
                    continue;
                }
                // Deterministic OR priority: skip if any from_step was already claimed by a higher-priority transition
                if tr.from_steps.iter().any(|s| claimed_from_steps.contains(s)) {
                    continue;
                }
                let receptivity = self.eval_expr(&tr.receptivity, io, 0)?.as_bool()?;
                if receptivity {
                    for s in &tr.from_steps {
                        claimed_from_steps.insert(s.clone());
                    }
                    fired_indices.push(t_idx);
                }
            }

            self.active_sfc_eval_chart = None;
            self.frozen_sfc_steps = None;

            if fired_indices.is_empty() {
                break;
            }

            iter_count = iter_count.saturating_add(1);
            if iter_count > max_iters {
                return Err(RuntimeError::SfcInstabilityLoop {
                    chart: chart.name.clone(),
                    limit: self.config.max_sfc_stability_steps,
                });
            }

            // Phase 2a: Deactivate ALL `from_steps` of fired transitions
            let mut newly_entered_macros = Vec::new();
            if let Some(st) = self.sfc_states.get_mut(chart_idx) {
                for &t_idx in &fired_indices {
                    if let Some(tr) = chart.transitions.get(t_idx) {
                        for s in &tr.from_steps {
                            st.active_steps.remove(s);
                        }
                    }
                }
                // Phase 2b: Activate ALL `to_steps` AFTER all clears (IEC 60848 Rule 5!)
                for &t_idx in &fired_indices {
                    if let Some(tr) = chart.transitions.get(t_idx) {
                        for s in &tr.to_steps {
                            st.active_steps.insert(s.clone());
                            st.step_elapsed_us.insert(s.clone(), 0);
                            if let Some(step_def) = chart.steps.iter().find(|sd| &sd.name == s)
                                && let Some(sub) = &step_def.macro_chart
                            {
                                newly_entered_macros.push(sub.clone());
                            }
                        }
                    }
                }
            }

            // Cleanly reset macro sub-charts on entry/re-entry (even same-scan re-entry!)
            for sub in newly_entered_macros {
                if let Some(&sub_idx) = self.compiled.sfc_chart_by_name.get(&sub) {
                    self.reset_and_init_sfc_chart(sub_idx);
                }
            }

            if !chart.evolve_to_stability {
                break;
            }
        }

        // 3. Execute IEC 61131-3 Table 47 Action Qualifiers (N, S, R, L, D, P, P1, P0)
        let current_active = self
            .sfc_states
            .get(chart_idx)
            .map(|s| s.active_steps.clone())
            .unwrap_or_default();

        // Record trace events for step state changes
        let t_now = self.time_us();
        let bid = self.compiled.board_id.clone();
        for step in &chart.steps {
            let was = scan_start_active.contains(&step.name);
            let now = current_active.contains(&step.name);
            if was != now {
                self.trace.push(TraceEvent {
                    time_us: t_now,
                    board_id: bid.clone(),
                    kind: TraceEventKind::SfcStepActive {
                        chart: chart.name.clone(),
                        step: step.name.clone(),
                        active: now,
                    },
                });
            }
        }

        let mut n_active_targets: BTreeSet<String> = BTreeSet::new();
        let mut s_targets: Vec<LValue> = Vec::new();
        let mut r_targets: Vec<LValue> = Vec::new();
        let mut n_target_lvalues: BTreeMap<String, LValue> = BTreeMap::new();

        for step in &chart.steps {
            let was_active = scan_start_active.contains(&step.name);
            let is_active = current_active.contains(&step.name);
            let elapsed = self
                .sfc_states
                .get(chart_idx)
                .and_then(|s| s.step_elapsed_us.get(&step.name).copied())
                .unwrap_or(0);

            // Step active macro-step's sub-chart
            if is_active
                && let Some(sub) = &step.macro_chart
                && let Some(&sub_idx) = self.compiled.sfc_chart_by_name.get(sub)
            {
                self.step_sfc_chart(sub_idx, io)?;
            }

            for action in &step.actions {
                let fire_action = match action.qualifier {
                    ActionQualifier::N | ActionQualifier::S | ActionQualifier::R => is_active,
                    ActionQualifier::L => {
                        if is_active {
                            let limit = match &action.duration {
                                Some(d) => self.eval_expr(d, io, 0)?.as_time_us()?,
                                None => 0,
                            };
                            elapsed < limit
                        } else {
                            false
                        }
                    }
                    ActionQualifier::D => {
                        if is_active {
                            let delay = match &action.duration {
                                Some(d) => self.eval_expr(d, io, 0)?.as_time_us()?,
                                None => 0,
                            };
                            elapsed >= delay
                        } else {
                            false
                        }
                    }
                    ActionQualifier::P | ActionQualifier::P1 => is_active && !was_active,
                    ActionQualifier::P0 => !is_active && was_active,
                };

                if let Some(target) = &action.target {
                    let key = target.root_name().to_owned();
                    match action.qualifier {
                        ActionQualifier::N | ActionQualifier::L | ActionQualifier::D => {
                            if fire_action {
                                n_active_targets.insert(key.clone());
                                n_target_lvalues.insert(key, target.clone());
                            } else {
                                n_target_lvalues
                                    .entry(key)
                                    .or_insert_with(|| target.clone());
                            }
                        }
                        ActionQualifier::S => {
                            if fire_action {
                                s_targets.push(target.clone());
                            }
                        }
                        ActionQualifier::R => {
                            if fire_action {
                                r_targets.push(target.clone());
                            }
                        }
                        ActionQualifier::P | ActionQualifier::P1 | ActionQualifier::P0 => {
                            if fire_action {
                                self.write_lvalue(target, Value::Bool(true), io)?;
                            }
                        }
                    }
                }

                if fire_action && !action.body_instrs.is_empty() {
                    self.run_inline_instrs(&action.body_instrs, io)?;
                }
            }
        }

        // Apply N / L / D automatic coil reset, S latch, and overriding R reset!
        let prev_n_owned = self
            .sfc_states
            .get(chart_idx)
            .map(|s| s.n_owned_coils.clone())
            .unwrap_or_default();

        for target_key in &n_active_targets {
            if let Some(lv) = n_target_lvalues.get(target_key) {
                self.write_lvalue(lv, Value::Bool(true), io)?;
            }
        }
        for lv in &s_targets {
            if let Some(st) = self.sfc_states.get_mut(chart_idx) {
                st.s_latched_coils.insert(lv.root_name().to_owned());
            }
            self.write_lvalue(lv, Value::Bool(true), io)?;
        }

        // Reset any previously N-owned coil no longer driven by N/L/D and not S-latched!
        let s_latched = self
            .sfc_states
            .get(chart_idx)
            .map(|s| s.s_latched_coils.clone())
            .unwrap_or_default();
        for old_key in &prev_n_owned {
            if !n_active_targets.contains(old_key)
                && !s_latched.contains(old_key)
                && let Some(lv) = n_target_lvalues.get(old_key)
            {
                self.write_lvalue(lv, Value::Bool(false), io)?;
            }
        }

        // Overriding Reset (`R`) wins over both `N` and `S`!
        for lv in &r_targets {
            if let Some(st) = self.sfc_states.get_mut(chart_idx) {
                st.s_latched_coils.remove(lv.root_name());
            }
            self.write_lvalue(lv, Value::Bool(false), io)?;
        }

        if let Some(st) = self.sfc_states.get_mut(chart_idx) {
            st.n_owned_coils = n_active_targets;
            st.prev_active_steps = current_active;
        }

        Ok(())
    }

    fn reset_and_init_sfc_chart(&mut self, chart_idx: usize) {
        let Some(chart) = self.compiled.sfc_charts.get(chart_idx).cloned() else {
            return;
        };
        if let Some(st) = self.sfc_states.get_mut(chart_idx) {
            st.initialized = true;
            st.active_steps.clear();
            st.prev_active_steps.clear();
            st.step_elapsed_us.clear();
            st.n_owned_coils.clear();
            st.s_latched_coils.clear();
            for step in &chart.steps {
                if step.initial {
                    st.active_steps.insert(step.name.clone());
                    st.step_elapsed_us.insert(step.name.clone(), 0);
                }
            }
        }
    }

    fn is_macro_chart_done(&self, chart: &CompiledSfcChart, chart_idx: usize) -> bool {
        // Exit step of an enclosed sub-chart is its final declared step (`04` §5.2)
        let Some(exit_step) = chart.steps.last() else {
            return true;
        };
        self.sfc_states
            .get(chart_idx)
            .is_some_and(|st| st.active_steps.contains(&exit_step.name))
    }

    fn run_inline_instrs(
        &mut self,
        instrs: &[Instr],
        io: &mut dyn BoardIoInterface,
    ) -> Result<(), RuntimeError> {
        // Execute inline SFC action statements in an isolated frame
        let saved_stack = std::mem::take(&mut self.stack);
        let mut pc = 0usize;
        let res = (|| -> Result<(), RuntimeError> {
            while let Some(instr) = instrs.get(pc) {
                pc = pc.saturating_add(1);
                let _ = self.exec_instr(instr, io)?;
            }
            Ok(())
        })();
        self.stack = saved_stack;
        res
    }

    // =========================================================================
    // Expressions, Calls, LValues & Hardware I/O
    // =========================================================================

    fn eval_expr(
        &mut self,
        expr: &Expr,
        io: &mut dyn BoardIoInterface,
        expr_depth: usize,
    ) -> Result<Value, RuntimeError> {
        match &expr.kind {
            ExprKind::Literal(lit) => match lit {
                Literal::Bool(b) => Ok(Value::Bool(*b)),
                Literal::Int(n) => {
                    if let Ok(d) = i32::try_from(*n) {
                        Ok(Value::Dint(d))
                    } else if let Ok(l) = i64::try_from(*n) {
                        Ok(Value::Lint(l))
                    } else {
                        Err(RuntimeError::IntegerOverflow)
                    }
                }
                Literal::Float(f) => Ok(Value::Lreal(*f)),
                Literal::TimeUs(us) => Ok(Value::TimeUs(*us)),
                Literal::String(s) => Ok(Value::String {
                    text: s.clone(),
                    max_len: 255,
                }),
            },
            ExprKind::Ident(name) => self.read_ident_or_hardware(name, io),
            ExprKind::Index { base, index } => {
                let arr = self.eval_expr(base, io, expr_depth)?;
                let idx_val = self.eval_expr(index, io, expr_depth)?.as_i128()?;
                let Value::Array(items) = arr else {
                    return Err(RuntimeError::TypeError("cannot index non-array".to_owned()));
                };
                let idx = usize::try_from(idx_val).map_err(|_| RuntimeError::IndexOutOfBounds {
                    index: idx_val,
                    len: items.len(),
                })?;
                items
                    .get(idx)
                    .cloned()
                    .ok_or(RuntimeError::IndexOutOfBounds {
                        index: idx_val,
                        len: items.len(),
                    })
            }
            ExprKind::Field { base, field } => {
                // First-class `<Step>.X`, `<Step>.T`, `<MacroStep>.DONE` (`04` §5.2 — zero `__sfc_*`!)
                if let ExprKind::Ident(step_name) = &base.kind
                    && let Some(val) = self.try_read_sfc_step_field(step_name, field)?
                {
                    return Ok(val);
                }
                let val = self.eval_expr(base, io, expr_depth)?;
                if let Value::Struct(map) = val {
                    map.get(field)
                        .cloned()
                        .ok_or_else(|| RuntimeError::UnknownField(field.clone()))
                } else if let Value::Fb(fb) = val {
                    fb.get_field(field)
                } else {
                    Err(RuntimeError::UnknownField(field.clone()))
                }
            }
            ExprKind::Call { callee, args } => self.eval_call_sync(callee, args, io, expr_depth),
            ExprKind::Unary { op, operand } => {
                let v = self.eval_expr(operand, io, expr_depth)?;
                eval_unary(*op, v, self.config.overflow)
            }
            ExprKind::Binary { op, left, right } => {
                // Short-circuit evaluation for AND / OR (`01` §4.2)
                if *op == BinaryOp::And {
                    let lv = self.eval_expr(left, io, expr_depth)?.as_bool()?;
                    if !lv {
                        return Ok(Value::Bool(false));
                    }
                    let rv = self.eval_expr(right, io, expr_depth)?.as_bool()?;
                    return Ok(Value::Bool(rv));
                }
                if *op == BinaryOp::Or {
                    let lv = self.eval_expr(left, io, expr_depth)?.as_bool()?;
                    if lv {
                        return Ok(Value::Bool(true));
                    }
                    let rv = self.eval_expr(right, io, expr_depth)?.as_bool()?;
                    return Ok(Value::Bool(rv));
                }
                let lv = self.eval_expr(left, io, expr_depth)?;
                let rv = self.eval_expr(right, io, expr_depth)?;
                eval_binary(*op, lv, rv, self.config.overflow)
            }
            ExprKind::Foreign { language, text } => {
                // Check for expression-level assumption on current statement
                let node_id = NodeId::new("expr_foreign");
                if let Some(a) = self
                    .assumptions
                    .values()
                    .find(|a| a.policy == AssumptionPolicy::ReturnValue)
                    && let Some(raw_val) = &a.value
                {
                    return parse_assumption_value(raw_val);
                }
                Err(RuntimeError::ForeignHalt {
                    node_id,
                    language: language.clone(),
                    fidelity: "F0".to_owned(),
                    text: text.clone(),
                })
            }
        }
    }

    fn try_read_sfc_step_field(
        &self,
        step_name: &str,
        field: &str,
    ) -> Result<Option<Value>, RuntimeError> {
        let chart_indices: Vec<usize> = if let Some(active_idx) = self.active_sfc_eval_chart {
            vec![active_idx]
        } else {
            (0..self.compiled.sfc_charts.len()).collect()
        };

        for c_idx in chart_indices {
            let Some(chart) = self.compiled.sfc_charts.get(c_idx) else {
                continue;
            };
            let Some(step) = chart.steps.iter().find(|s| s.name == step_name) else {
                continue;
            };
            let active = if self.active_sfc_eval_chart == Some(c_idx)
                && let Some(frozen) = &self.frozen_sfc_steps
            {
                frozen.contains(step_name)
            } else {
                self.sfc_states
                    .get(c_idx)
                    .is_some_and(|st| st.active_steps.contains(step_name))
            };
            match field {
                "X" => return Ok(Some(Value::Bool(active))),
                "T" => {
                    let t = self
                        .sfc_states
                        .get(c_idx)
                        .and_then(|st| st.step_elapsed_us.get(step_name).copied())
                        .unwrap_or(0);
                    return Ok(Some(Value::TimeUs(t)));
                }
                "DONE" => {
                    if let Some(sub_name) = &step.macro_chart
                        && let Some(&sub_idx) = self.compiled.sfc_chart_by_name.get(sub_name)
                        && let Some(sub_chart) = self.compiled.sfc_charts.get(sub_idx)
                    {
                        return Ok(Some(Value::Bool(
                            self.is_macro_chart_done(sub_chart, sub_idx),
                        )));
                    }
                    return Ok(Some(Value::Bool(false)));
                }
                _ => return Err(RuntimeError::UnknownField(field.to_owned())),
            }
        }
        Ok(None)
    }

    fn eval_call_sync(
        &mut self,
        callee: &str,
        args: &[CallArg],
        io: &mut dyn BoardIoInterface,
        expr_depth: usize,
    ) -> Result<Value, RuntimeError> {
        if let Some(&pou_idx) = self.compiled.pou_by_name.get(callee) {
            if self.stack.len().saturating_add(expr_depth) >= self.config.max_call_depth {
                return Err(RuntimeError::StackOverflow {
                    callee: callee.to_owned(),
                    limit: self.config.max_call_depth,
                });
            }
            let pou = self
                .compiled
                .pous
                .get(pou_idx)
                .cloned()
                .ok_or_else(|| RuntimeError::UnresolvedFunction(callee.to_owned()))?;
            let bound = self.bind_call_args(&pou.params, args, io, expr_depth.saturating_add(1))?;
            self.push_frame(pou_idx, bound)?;

            let mut ret_val = Value::Void;
            let base_depth = self.stack.len();
            while self.stack.len() >= base_depth {
                let Some(top) = self.stack.last() else {
                    break;
                };
                let cur_pou = top.pou_idx;
                let pc = top.pc;
                let instr_opt = self
                    .compiled
                    .pous
                    .get(cur_pou)
                    .and_then(|p| p.instrs.get(pc))
                    .cloned();
                let Some(instr) = instr_opt else {
                    self.stack.pop();
                    continue;
                };
                if let Some(f) = self.stack.last_mut() {
                    f.pc = pc.saturating_add(1);
                }
                if let Instr::Return { value } = &instr {
                    self.charge_step(self.config.cost.branch_cycles)?;
                    ret_val = match value {
                        Some(expr) => self.eval_expr(expr, io, expr_depth.saturating_add(1))?,
                        None => Value::Void,
                    };
                    self.stack.pop();
                } else {
                    let _ = self.exec_instr(&instr, io)?;
                }
            }
            return Ok(ret_val);
        }

        self.eval_builtin_call(callee, args, io, expr_depth)
    }

    fn eval_builtin_call(
        &mut self,
        callee: &str,
        args: &[CallArg],
        io: &mut dyn BoardIoInterface,
        expr_depth: usize,
    ) -> Result<Value, RuntimeError> {
        match callee {
            "DIGITAL_READ" => {
                self.charge_step(self.config.cost.io_read_cycles)?;
                let arg = args.first().ok_or_else(|| {
                    RuntimeError::TypeError("DIGITAL_READ expects pin".to_owned())
                })?;
                if let ExprKind::Ident(label) = &arg.value.kind {
                    return self.read_hardware_label(label, io);
                }
                let v = self.eval_expr(&arg.value, io, expr_depth)?;
                Ok(Value::Bool(v.as_bool()?))
            }
            "DIGITAL_WRITE" => {
                self.charge_step(self.config.cost.io_write_cycles)?;
                let pin_arg = args.first().ok_or_else(|| {
                    RuntimeError::TypeError("DIGITAL_WRITE expects 2 args".to_owned())
                })?;
                let val_arg = args.get(1).ok_or_else(|| {
                    RuntimeError::TypeError("DIGITAL_WRITE expects 2 args".to_owned())
                })?;
                let high = self.eval_expr(&val_arg.value, io, expr_depth)?.as_bool()?;
                let pin = self.resolve_pin_arg(&pin_arg.value)?;
                self.emit_digital_write(&pin, high, io);
                Ok(Value::Void)
            }
            "ADC_CODE" => {
                self.charge_step(self.config.cost.adc_read_cycles)?;
                let pin_arg = args
                    .first()
                    .ok_or_else(|| RuntimeError::TypeError("ADC_CODE expects pin".to_owned()))?;
                let ExprKind::Ident(label) = &pin_arg.value.kind else {
                    return Err(RuntimeError::TypeError(
                        "ADC_CODE requires hardware label".to_owned(),
                    ));
                };
                let label = label.as_str();
                let hw = self
                    .compiled
                    .hardware
                    .get(label)
                    .cloned()
                    .ok_or_else(|| RuntimeError::UndeclaredSymbol(label.to_owned()))?;
                let codec_id = hw.codec.clone().unwrap_or_else(|| "uno_10bit".to_owned());
                let codec = self
                    .config
                    .codecs
                    .get(&codec_id)
                    .ok_or_else(|| RuntimeError::UnknownCodec(codec_id.clone()))?;
                let reading = if self.in_plc_scan {
                    self.pii_analog.get(&hw.pin).copied().unwrap_or_default()
                } else {
                    io.read_analog(&hw.pin)
                };
                let code = codec.band_analog_to_code_i64(reading.analog);
                Ok(Value::Code {
                    value: code,
                    codec_id,
                })
            }
            "PWM_WRITE" => {
                self.charge_step(self.config.cost.io_write_cycles)?;
                let pin_arg = args.first().ok_or_else(|| {
                    RuntimeError::TypeError("PWM_WRITE expects 2 args".to_owned())
                })?;
                let duty_arg = args.get(1).ok_or_else(|| {
                    RuntimeError::TypeError("PWM_WRITE expects 2 args".to_owned())
                })?;
                let pin = self.resolve_pin_arg(&pin_arg.value)?;
                let raw_duty = self.eval_expr(&duty_arg.value, io, expr_depth)?;
                let (coerced, clamped) =
                    raw_duty.coerce_to_type(&TypeRef::scalar(dendri_ast::ScalarType::Duty))?;
                if clamped {
                    self.record_diag(
                        ErrorCode::WarnDutyClamped,
                        format!("PWM_WRITE duty on `{pin}` clamped to [0, M]"),
                    );
                }
                let Value::Duty(duty) = coerced else {
                    return Err(RuntimeError::TypeError("PWM_WRITE expects DUTY".to_owned()));
                };
                self.emit_pwm_write(&pin, duty, io);
                Ok(Value::Void)
            }
            "MILLIS" => {
                self.charge_step(self.config.cost.stmt_cycles)?;
                Ok(Value::Udint((self.time_us() / 1_000) as u32))
            }
            "MICROS" => {
                self.charge_step(self.config.cost.stmt_cycles)?;
                Ok(Value::Udint(self.time_us() as u32))
            }
            "DISPLAY_PRINT" => {
                self.charge_step(self.config.cost.io_write_cycles)?;
                let target = self
                    .eval_expr(
                        &args
                            .first()
                            .ok_or_else(|| {
                                RuntimeError::TypeError("DISPLAY_PRINT expects 2 args".to_owned())
                            })?
                            .value,
                        io,
                        expr_depth,
                    )?
                    .as_str()?
                    .to_owned();
                let text = self
                    .eval_expr(
                        &args
                            .get(1)
                            .ok_or_else(|| {
                                RuntimeError::TypeError("DISPLAY_PRINT expects 2 args".to_owned())
                            })?
                            .value,
                        io,
                        expr_depth,
                    )?
                    .as_str()?
                    .to_owned();
                io.display_print(&target, &text);
                self.trace.push(TraceEvent {
                    time_us: self.time_us(),
                    board_id: self.compiled.board_id.clone(),
                    kind: TraceEventKind::DisplayPrint { target, text },
                });
                Ok(Value::Void)
            }
            "BOOL_TO_INT" => {
                let b = self
                    .eval_expr(
                        &args
                            .first()
                            .ok_or_else(|| {
                                RuntimeError::TypeError("BOOL_TO_INT expects 1 arg".to_owned())
                            })?
                            .value,
                        io,
                        expr_depth,
                    )?
                    .as_bool()?;
                Ok(Value::Int(i16::from(b)))
            }
            "INT_TO_DINT" => {
                let n = self
                    .eval_expr(
                        &args
                            .first()
                            .ok_or_else(|| {
                                RuntimeError::TypeError("INT_TO_DINT expects 1 arg".to_owned())
                            })?
                            .value,
                        io,
                        expr_depth,
                    )?
                    .as_i128()?;
                Ok(Value::Dint(n as i32))
            }
            "DINT_TO_REAL" => {
                let n = self
                    .eval_expr(
                        &args
                            .first()
                            .ok_or_else(|| {
                                RuntimeError::TypeError("DINT_TO_REAL expects 1 arg".to_owned())
                            })?
                            .value,
                        io,
                        expr_depth,
                    )?
                    .as_i128()?;
                Ok(Value::Real(n as f32))
            }
            "REAL_TO_DINT" => {
                let f = self
                    .eval_expr(
                        &args
                            .first()
                            .ok_or_else(|| {
                                RuntimeError::TypeError("REAL_TO_DINT expects 1 arg".to_owned())
                            })?
                            .value,
                        io,
                        expr_depth,
                    )?
                    .as_f64()?;
                Ok(Value::Dint(f as i32))
            }
            "ANALOG_TO_CODE" => {
                let a = self
                    .eval_expr(
                        &args
                            .first()
                            .ok_or_else(|| {
                                RuntimeError::TypeError("ANALOG_TO_CODE expects 2 args".to_owned())
                            })?
                            .value,
                        io,
                        expr_depth,
                    )?
                    .as_i128()? as i32;
                let codec_id = self
                    .eval_expr(
                        &args
                            .get(1)
                            .ok_or_else(|| {
                                RuntimeError::TypeError("ANALOG_TO_CODE expects 2 args".to_owned())
                            })?
                            .value,
                        io,
                        expr_depth,
                    )?
                    .as_str()?
                    .to_owned();
                let codec = self
                    .config
                    .codecs
                    .get(&codec_id)
                    .ok_or_else(|| RuntimeError::UnknownCodec(codec_id.clone()))?;
                let code = codec.band_analog_to_code_i64(a);
                Ok(Value::Code {
                    value: code,
                    codec_id,
                })
            }
            "CODE_TO_ANALOG" => {
                let code_val = self
                    .eval_expr(
                        &args
                            .first()
                            .ok_or_else(|| {
                                RuntimeError::TypeError("CODE_TO_ANALOG expects 2 args".to_owned())
                            })?
                            .value,
                        io,
                        expr_depth,
                    )?
                    .as_i128()? as i64;
                let codec_id = self
                    .eval_expr(
                        &args
                            .get(1)
                            .ok_or_else(|| {
                                RuntimeError::TypeError("CODE_TO_ANALOG expects 2 args".to_owned())
                            })?
                            .value,
                        io,
                        expr_depth,
                    )?
                    .as_str()?
                    .to_owned();
                let codec = self
                    .config
                    .codecs
                    .get(&codec_id)
                    .ok_or(RuntimeError::UnknownCodec(codec_id))?;
                let mapped = codec.band_code_to_analog_i128(code_val);
                let clamped = mapped.clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32;
                Ok(Value::Analog(clamped))
            }
            "ANALOG_TO_DUTY" => {
                let a = self
                    .eval_expr(
                        &args
                            .first()
                            .ok_or_else(|| {
                                RuntimeError::TypeError("ANALOG_TO_DUTY expects 1 arg".to_owned())
                            })?
                            .value,
                        io,
                        expr_depth,
                    )?
                    .as_i128()?;
                let duty_max = i128::from(ASL_ANALOG_FULL_SCALE);
                if a < 0 {
                    self.record_diag(
                        ErrorCode::WarnDutyClamped,
                        "negative ANALOG clamped to DUTY 0".to_owned(),
                    );
                    Ok(Value::Duty(0))
                } else if a > duty_max {
                    self.record_diag(
                        ErrorCode::WarnDutyClamped,
                        "ANALOG > M clamped to DUTY M".to_owned(),
                    );
                    Ok(Value::Duty(
                        u32::try_from(ASL_ANALOG_FULL_SCALE).unwrap_or(u32::MAX),
                    ))
                } else {
                    Ok(Value::Duty(a as u32))
                }
            }
            other => {
                let mut eval_args = Vec::with_capacity(args.len());
                for a in args {
                    eval_args.push(self.eval_expr(&a.value, io, expr_depth)?);
                }
                if let Some(res) = eval_stdlib_math(other, &eval_args)? {
                    Ok(res)
                } else {
                    Err(RuntimeError::UnresolvedFunction(other.to_owned()))
                }
            }
        }
    }

    fn bind_call_args(
        &mut self,
        params: &[dendri_ast::ParamDecl],
        args: &[CallArg],
        io: &mut dyn BoardIoInterface,
        expr_depth: usize,
    ) -> Result<BTreeMap<String, Value>, RuntimeError> {
        let mut bound = BTreeMap::new();
        for (idx, arg) in args.iter().enumerate() {
            let val = self.eval_expr(&arg.value, io, expr_depth)?;
            if let Some(name) = &arg.name {
                bound.insert(name.clone(), val);
            } else if let Some(param) = params.get(idx) {
                let (coerced, _) = val.coerce_to_type(&param.ty)?;
                bound.insert(param.name.clone(), coerced);
            }
        }
        Ok(bound)
    }

    fn resolve_pin_arg(&self, expr: &Expr) -> Result<String, RuntimeError> {
        if let ExprKind::Ident(label) = &expr.kind {
            self.compiled
                .hardware
                .get(label)
                .map(|h| h.pin.clone())
                .ok_or_else(|| RuntimeError::UndeclaredSymbol(label.clone()))
        } else if let ExprKind::Literal(Literal::String(pin)) = &expr.kind {
            Ok(pin.clone())
        } else {
            Err(RuntimeError::TypeError(
                "expected hardware pin label".to_owned(),
            ))
        }
    }

    fn read_ident_or_hardware(
        &mut self,
        name: &str,
        io: &dyn BoardIoInterface,
    ) -> Result<Value, RuntimeError> {
        // Strict lexical lookup: top local frame shadows globals cleanly (`02` §2.3)!
        if let Some(frame) = self.stack.last()
            && let Some(val) = frame.locals.get(name)
        {
            return Ok(val.clone());
        }
        if let Some(val) = self.globals.get(name) {
            return Ok(val.clone());
        }
        if self.compiled.hardware.contains_key(name) {
            return self.read_hardware_label(name, io);
        }
        Err(RuntimeError::UndeclaredSymbol(name.to_owned()))
    }

    fn read_hardware_label(
        &mut self,
        label: &str,
        io: &dyn BoardIoInterface,
    ) -> Result<Value, RuntimeError> {
        let hw = self
            .compiled
            .hardware
            .get(label)
            .cloned()
            .ok_or_else(|| RuntimeError::UndeclaredSymbol(label.to_owned()))?;
        match hw.kind {
            HardwareKind::DigitalIn
            | HardwareKind::DigitalOut
            | HardwareKind::DigitalBidirectional => {
                if self.in_plc_scan
                    && let Some(&b) = self.pii_digital.get(&hw.pin)
                {
                    return Ok(Value::Bool(b));
                }
                let level = io.read_digital(&hw.pin);
                Ok(Value::Bool(self.resolve_digital_level(&hw.pin, level)))
            }
            HardwareKind::AnalogIn | HardwareKind::AnalogOut => {
                let reading = if self.in_plc_scan {
                    self.pii_analog.get(&hw.pin).copied().unwrap_or_default()
                } else {
                    io.read_analog(&hw.pin)
                };
                Ok(Value::Analog(reading.analog))
            }
            HardwareKind::PwmOut => Ok(Value::Duty(0)),
            HardwareKind::BusPin => Ok(Value::Byte(0)),
        }
    }

    fn resolve_digital_level(&mut self, pin: &str, level: ElectricalDigitalLevel) -> bool {
        match level {
            ElectricalDigitalLevel::High => {
                self.last_digital_in.insert(pin.to_owned(), true);
                true
            }
            ElectricalDigitalLevel::Low => {
                self.last_digital_in.insert(pin.to_owned(), false);
                false
            }
            ElectricalDigitalLevel::Indeterminate => {
                self.record_diag(
                    ErrorCode::WarnIndeterminateLevel,
                    format!(
                        "digital read on pin `{pin}` encountered INDETERMINATE level (resolved via {:?})",
                        self.config.indeterminate_reads_as
                    ),
                );
                match self.config.indeterminate_reads_as {
                    IndeterminatePolicy::Previous => {
                        self.last_digital_in.get(pin).copied().unwrap_or(false)
                    }
                    IndeterminatePolicy::Low => false,
                    IndeterminatePolicy::High => true,
                }
            }
        }
    }

    fn read_var(&self, name: &str) -> Result<&Value, RuntimeError> {
        if let Some(frame) = self.stack.last()
            && let Some(v) = frame.locals.get(name)
        {
            return Ok(v);
        }
        self.globals
            .get(name)
            .ok_or_else(|| RuntimeError::UndeclaredSymbol(name.to_owned()))
    }

    fn write_var(&mut self, name: &str, val: Value) -> Result<(), RuntimeError> {
        if let Some(frame) = self.stack.last_mut()
            && frame.locals.contains_key(name)
        {
            frame.locals.insert(name.to_owned(), val);
            return Ok(());
        }
        if self.globals.contains_key(name) {
            self.globals.insert(name.to_owned(), val);
            return Ok(());
        }
        Err(RuntimeError::UndeclaredSymbol(name.to_owned()))
    }

    fn read_lvalue(
        &mut self,
        lv: &LValue,
        io: &mut dyn BoardIoInterface,
    ) -> Result<Value, RuntimeError> {
        match lv {
            LValue::Var { name, .. } => self.read_ident_or_hardware(name, io),
            LValue::Index { base, index, .. } => {
                let arr = self.read_lvalue(base, io)?;
                let idx_val = self.eval_expr(index, io, 0)?.as_i128()?;
                let Value::Array(items) = arr else {
                    return Err(RuntimeError::TypeError("cannot index non-array".to_owned()));
                };
                let idx = usize::try_from(idx_val).map_err(|_| RuntimeError::IndexOutOfBounds {
                    index: idx_val,
                    len: items.len(),
                })?;
                items
                    .get(idx)
                    .cloned()
                    .ok_or(RuntimeError::IndexOutOfBounds {
                        index: idx_val,
                        len: items.len(),
                    })
            }
            LValue::Field { base, field, .. } => {
                let v = self.read_lvalue(base, io)?;
                if let Value::Struct(map) = v {
                    map.get(field)
                        .cloned()
                        .ok_or_else(|| RuntimeError::UnknownField(field.clone()))
                } else if let Value::Fb(fb) = v {
                    fb.get_field(field)
                } else {
                    Err(RuntimeError::UnknownField(field.clone()))
                }
            }
        }
    }

    fn write_lvalue(
        &mut self,
        lv: &LValue,
        val: Value,
        io: &mut dyn BoardIoInterface,
    ) -> Result<(), RuntimeError> {
        if let LValue::Var { name, .. } = lv {
            // Check local first (lexical shadowing!), then globals, then writable hardware tag (Ladder/SFC coil)
            if let Some(frame) = self.stack.last_mut()
                && let Some(existing) = frame.locals.get(name)
            {
                let coerced = coerce_like_existing(existing, val)?;
                frame.locals.insert(name.clone(), coerced);
                return Ok(());
            }
            if let Some(existing) = self.globals.get(name) {
                let coerced = coerce_like_existing(existing, val)?;
                self.globals.insert(name.clone(), coerced);
                return Ok(());
            }
            if let Some(hw) = self.compiled.hardware.get(name).cloned() {
                match hw.kind {
                    HardwareKind::DigitalOut | HardwareKind::DigitalBidirectional => {
                        self.emit_digital_write(&hw.pin, val.as_bool()?, io);
                        return Ok(());
                    }
                    HardwareKind::AnalogOut => {
                        let a = val.as_i128()? as i32;
                        self.emit_analog_write(&hw.pin, a, io);
                        return Ok(());
                    }
                    HardwareKind::PwmOut => {
                        let (coerced, clamped) =
                            val.coerce_to_type(&TypeRef::scalar(dendri_ast::ScalarType::Duty))?;
                        if clamped {
                            self.record_diag(
                                ErrorCode::WarnDutyClamped,
                                format!("PWM write on `{}` clamped to [0, M]", hw.pin),
                            );
                        }
                        if let Value::Duty(d) = coerced {
                            self.emit_pwm_write(&hw.pin, d, io);
                            return Ok(());
                        }
                    }
                    HardwareKind::DigitalIn | HardwareKind::AnalogIn | HardwareKind::BusPin => {
                        return Err(RuntimeError::TypeError(format!(
                            "cannot write to read-only hardware label `{name}`"
                        )));
                    }
                }
            }
            return Err(RuntimeError::UndeclaredSymbol(name.clone()));
        }

        // Nested Index / Field write: resolve path steps cleanly without panicking
        let root_name = lv.root_name().to_owned();
        let mut steps = Vec::new();
        self.collect_lvalue_steps(lv, io, &mut steps)?;

        let in_local = self
            .stack
            .last()
            .is_some_and(|f| f.locals.contains_key(&root_name));
        let root_mut = if in_local {
            self.stack
                .last_mut()
                .and_then(|f| f.locals.get_mut(&root_name))
                .ok_or_else(|| RuntimeError::UndeclaredSymbol(root_name.clone()))?
        } else {
            self.globals
                .get_mut(&root_name)
                .ok_or_else(|| RuntimeError::UndeclaredSymbol(root_name.clone()))?
        };

        apply_lvalue_write(root_mut, &steps, val)
    }

    fn collect_lvalue_steps(
        &mut self,
        lv: &LValue,
        io: &mut dyn BoardIoInterface,
        out: &mut Vec<LValueStep>,
    ) -> Result<(), RuntimeError> {
        match lv {
            LValue::Var { .. } => Ok(()),
            LValue::Index { base, index, .. } => {
                self.collect_lvalue_steps(base, io, out)?;
                let idx = self.eval_expr(index, io, 0)?.as_i128()?;
                out.push(LValueStep::Index(idx));
                Ok(())
            }
            LValue::Field { base, field, .. } => {
                self.collect_lvalue_steps(base, io, out)?;
                out.push(LValueStep::Field(field.clone()));
                Ok(())
            }
        }
    }

    fn invoke_fb_lvalue(
        &mut self,
        instance: &LValue,
        inputs: &BTreeMap<String, Value>,
        io: &mut dyn BoardIoInterface,
    ) -> Result<bool, RuntimeError> {
        let root_name = instance.root_name().to_owned();
        let dt_us = if self.fb_ticked_this_scan.insert(root_name.clone()) {
            self.current_dt_us
        } else {
            0
        };

        let in_local = self
            .stack
            .last()
            .is_some_and(|f| f.locals.contains_key(&root_name));
        let mut fb_val = if in_local {
            self.stack
                .last()
                .and_then(|f| f.locals.get(&root_name))
                .cloned()
                .ok_or_else(|| RuntimeError::UndeclaredSymbol(root_name.clone()))?
        } else {
            self.globals
                .get(&root_name)
                .cloned()
                .ok_or_else(|| RuntimeError::UndeclaredSymbol(root_name.clone()))?
        };

        let out_q = if let Value::Fb(fb) = &mut fb_val {
            fb.invoke(inputs, dt_us, io)?;
            fb.ladder_out_rail()
        } else {
            return Err(RuntimeError::TypeError(format!(
                "`{root_name}` is not a Function Block instance"
            )));
        };

        // Write updated FB state back to storage
        if in_local {
            if let Some(frame) = self.stack.last_mut() {
                frame.locals.insert(root_name, fb_val);
            }
        } else {
            self.globals.insert(root_name, fb_val);
        }
        Ok(out_q)
    }

    fn emit_digital_write(&mut self, pin: &str, high: bool, io: &mut dyn BoardIoInterface) {
        if self.in_plc_scan {
            self.piq_digital.insert(pin.to_owned(), high);
        } else {
            io.write_digital(pin, high);
            self.io_write_occurred = true;
            self.trace.push(TraceEvent {
                time_us: self.time_us(),
                board_id: self.compiled.board_id.clone(),
                kind: TraceEventKind::DigitalWrite {
                    pin: pin.to_owned(),
                    high,
                },
            });
        }
    }

    fn emit_analog_write(&mut self, pin: &str, analog: i32, io: &mut dyn BoardIoInterface) {
        if self.in_plc_scan {
            self.piq_analog.insert(pin.to_owned(), analog);
        } else {
            io.write_analog(pin, analog);
            self.io_write_occurred = true;
            self.trace.push(TraceEvent {
                time_us: self.time_us(),
                board_id: self.compiled.board_id.clone(),
                kind: TraceEventKind::AnalogWrite {
                    pin: pin.to_owned(),
                    analog,
                },
            });
        }
    }

    fn emit_pwm_write(&mut self, pin: &str, duty: u32, io: &mut dyn BoardIoInterface) {
        if self.in_plc_scan {
            self.piq_pwm.insert(pin.to_owned(), duty);
        } else {
            io.write_pwm(pin, duty);
            self.io_write_occurred = true;
            self.trace.push(TraceEvent {
                time_us: self.time_us(),
                board_id: self.compiled.board_id.clone(),
                kind: TraceEventKind::PwmWrite {
                    pin: pin.to_owned(),
                    duty,
                },
            });
        }
    }

    fn record_diag(&mut self, code: ErrorCode, message: String) {
        self.trace.push(TraceEvent {
            time_us: self.time_us(),
            board_id: self.compiled.board_id.clone(),
            kind: TraceEventKind::Diagnostic { code, message },
        });
    }

    fn get_rail(&self, reg: crate::lower::RailReg) -> bool {
        self.stack
            .last()
            .and_then(|f| f.rails.get(reg.0 as usize).copied())
            .unwrap_or(false)
    }

    fn set_rail(&mut self, reg: crate::lower::RailReg, val: bool) {
        if let Some(frame) = self.stack.last_mut()
            && let Some(slot) = frame.rails.get_mut(reg.0 as usize)
        {
            *slot = val;
        }
    }
}

enum LValueStep {
    Index(i128),
    Field(String),
}

fn apply_lvalue_write(
    root: &mut Value,
    steps: &[LValueStep],
    new_val: Value,
) -> Result<(), RuntimeError> {
    let mut cur = root;
    for step in steps {
        match (cur, step) {
            (Value::Array(items), LValueStep::Index(idx_i)) => {
                let len = items.len();
                let idx = usize::try_from(*idx_i)
                    .map_err(|_| RuntimeError::IndexOutOfBounds { index: *idx_i, len })?;
                cur = items
                    .get_mut(idx)
                    .ok_or(RuntimeError::IndexOutOfBounds { index: *idx_i, len })?;
            }
            (Value::Struct(map), LValueStep::Field(f)) => {
                cur = map
                    .get_mut(f)
                    .ok_or_else(|| RuntimeError::UnknownField(f.clone()))?;
            }
            (_, LValueStep::Index(_)) => {
                return Err(RuntimeError::TypeError("cannot index non-array".to_owned()));
            }
            (_, LValueStep::Field(f)) => {
                return Err(RuntimeError::UnknownField(f.clone()));
            }
        }
    }
    *cur = coerce_like_existing(cur, new_val)?;
    Ok(())
}

fn coerce_like_existing(existing: &Value, incoming: Value) -> Result<Value, RuntimeError> {
    match (existing, incoming) {
        (Value::Sint(_), Value::Dint(n)) => Ok(Value::Sint(n as i8)),
        (Value::Int(_), Value::Dint(n)) => Ok(Value::Int(n as i16)),
        (Value::Lint(_), Value::Dint(n)) => Ok(Value::Lint(i64::from(n))),
        (Value::Usint(_), Value::Dint(n)) => Ok(Value::Usint(n as u8)),
        (Value::Uint(_), Value::Dint(n)) => Ok(Value::Uint(n as u16)),
        (Value::Udint(_), Value::Dint(n)) => Ok(Value::Udint(n as u32)),
        (Value::Ulint(_), Value::Dint(n)) => Ok(Value::Ulint(n as u64)),
        (Value::Byte(_), Value::Dint(n)) => Ok(Value::Byte(n as u8)),
        (Value::Word(_), Value::Dint(n)) => Ok(Value::Word(n as u16)),
        (Value::Dword(_), Value::Dint(n)) => Ok(Value::Dword(n as u32)),
        (Value::Lword(_), Value::Dint(n)) => Ok(Value::Lword(n as u64)),
        (Value::Real(_), Value::Lreal(f)) => Ok(Value::Real(f as f32)),
        (Value::Lreal(_), Value::Real(f)) => Ok(Value::Lreal(f64::from(f))),
        (Value::Analog(_), Value::Dint(n)) => Ok(Value::Analog(n)),
        (Value::Duty(_), Value::Dint(n)) => {
            let max = u32::try_from(ASL_ANALOG_FULL_SCALE).unwrap_or(u32::MAX);
            Ok(Value::Duty(if n < 0 { 0 } else { (n as u32).min(max) }))
        }
        (_, other) => Ok(other),
    }
}

const fn for_cond_holds(cur: i128, to: i128, step: i128, inclusive: bool) -> bool {
    if step == 0 {
        return false;
    }
    if step > 0 {
        if inclusive { cur <= to } else { cur < to }
    } else if inclusive {
        cur >= to
    } else {
        cur > to
    }
}

fn parse_assumption_value(raw: &str) -> Result<Value, RuntimeError> {
    let trimmed = raw.trim();
    if trimmed == "true" {
        return Ok(Value::Bool(true));
    }
    if trimmed == "false" {
        return Ok(Value::Bool(false));
    }
    if let Ok(n) = trimmed.parse::<i32>() {
        return Ok(Value::Dint(n));
    }
    if let Ok(f) = trimmed.parse::<f64>() {
        return Ok(Value::Lreal(f));
    }
    Ok(Value::String {
        text: trimmed.to_owned(),
        max_len: 255,
    })
}
