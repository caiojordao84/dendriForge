//! Deterministic discrete-event multi-board orchestrator (`02-simulation-and-electrical-engine.md` §2).

use crate::backend::{DigitalWireFabric, ElectricalBackend};
use crate::timeline::{InputTimeline, StimulusEvent, StimulusKind};
use crate::{DEFAULT_PLC_SCAN_PERIOD_US, KernelConfig, KernelError};
use dendri_ast::{ExecutionKind, InterruptEdge, Project};
use dendri_vm::{
    AnalogReading, BoardIoInterface, BoardVm, ElectricalDigitalLevel, StepOutcome, TraceEvent,
    TraceRing, Value,
};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

/// Event priority ordering at equal timestamps (lower executes first).
const PRIO_STIMULUS: u8 = 0;
const PRIO_CYCLIC_INT: u8 = 1;
const PRIO_PLC_SCAN: u8 = 2;
const PRIO_COOP_WAKE: u8 = 3;

/// Event payload stored in the deterministic kernel event queue.
#[derive(Debug, Clone, PartialEq, Eq)]
enum EventKind {
    ExternalStimulus(StimulusEvent),
    CyclicInterrupt {
        board_idx: usize,
        pou_idx: usize,
        period_us: u64,
    },
    PlcScanTick {
        board_idx: usize,
        period_us: u64,
    },
    CooperativeWake {
        board_idx: usize,
        /// Exact target CPU cycle count when waking from `WAIT_TIME` / `WAIT_TIME_US`.
        resume_cycles: Option<u64>,
    },
}

/// Deterministic scheduled event ordered strictly by `(time_us, priority, seq)`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScheduledEvent {
    time_us: u64,
    priority: u8,
    seq: u64,
    kind: EventKind,
}

impl Ord for ScheduledEvent {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.time_us
            .cmp(&other.time_us)
            .then_with(|| self.priority.cmp(&other.priority))
            .then_with(|| self.seq.cmp(&other.seq))
    }
}

impl PartialOrd for ScheduledEvent {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Immutable per-board runtime snapshot published to UI/CLI observers (`02` §2.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardSnapshot {
    pub board_id: String,
    pub time_us: u64,
    pub fiber_cycles: u64,
    pub digital_outputs: BTreeMap<String, bool>,
    pub analog_outputs: BTreeMap<String, i32>,
    pub pwm_outputs: BTreeMap<String, u32>,
    pub active_sfc_steps: BTreeMap<String, BTreeSet<String>>,
    pub trace_len: usize,
}

/// Immutable kernel-wide simulation snapshot (`Arc<SimSnapshot>` in [`crate::SimWorker`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimSnapshot {
    pub time_us: u64,
    pub seed: u64,
    pub boards: BTreeMap<String, BoardSnapshot>,
    pub global_trace_len: usize,
    pub last_error: Option<String>,
}

/// Per-board runtime bookkeeping inside [`SimKernel`].
struct BoardRuntime {
    board_id: String,
    execution: ExecutionKind,
    scan_period_us: u64,
    vm: BoardVm,
    /// Last timestamp (µs) at which this board's timers / PLC scan ran.
    last_step_us: u64,
    /// Last observed boolean state per physical pin bound to an `OB_HW_INT`.
    last_hw_int_pin: BTreeMap<String, bool>,
    /// Mirrored driven outputs for fast snapshot construction and change detection.
    digital_outputs: BTreeMap<String, bool>,
    analog_outputs: BTreeMap<String, i32>,
    pwm_outputs: BTreeMap<String, u32>,
    /// Cursor into `vm.trace()` total pushed count for incremental global trace merging.
    synced_trace_total: u64,
}

/// Borrowed I/O adapter forwarding one board's reads/writes to the active [`ElectricalBackend`].
struct BoardIoBridge<'a> {
    board_id: &'a str,
    backend: &'a mut dyn ElectricalBackend,
    digital_outputs: &'a mut BTreeMap<String, bool>,
    analog_outputs: &'a mut BTreeMap<String, i32>,
    pwm_outputs: &'a mut BTreeMap<String, u32>,
    output_changed: bool,
}

impl BoardIoInterface for BoardIoBridge<'_> {
    fn read_digital(&self, pin: &str) -> ElectricalDigitalLevel {
        self.backend.read_board_digital_in(self.board_id, pin)
    }

    fn write_digital(&mut self, pin: &str, high: bool) {
        let prev = self.digital_outputs.insert(pin.to_owned(), high);
        if prev != Some(high) {
            self.output_changed = true;
        }
        self.backend.set_board_digital_out(self.board_id, pin, high);
    }

    fn read_analog(&self, pin: &str) -> AnalogReading {
        self.backend.read_board_analog_in(self.board_id, pin)
    }

    fn write_analog(&mut self, pin: &str, analog: i32) {
        let prev = self.analog_outputs.insert(pin.to_owned(), analog);
        if prev != Some(analog) {
            self.output_changed = true;
        }
        self.backend
            .set_board_analog_out(self.board_id, pin, analog);
    }

    fn write_pwm(&mut self, pin: &str, duty: u32) {
        let prev = self.pwm_outputs.insert(pin.to_owned(), duty);
        if prev != Some(duty) {
            self.output_changed = true;
        }
        self.backend.set_board_pwm_out(self.board_id, pin, duty);
    }
}

/// Deterministic multi-board co-simulation kernel (`02` §2).
pub struct SimKernel {
    config: KernelConfig,
    /// Master monotonically increasing simulation clock in microseconds.
    time_us: u64,
    /// Next monotonic sequence number for deterministic event tie-breaking.
    next_seq: u64,
    /// Deterministic min-heap discrete-event queue.
    queue: BinaryHeap<Reverse<ScheduledEvent>>,
    /// Per-board compiled VMs and runtime metadata in project declaration order.
    boards: Vec<BoardRuntime>,
    /// Lookup from `board.id` to index in `boards`.
    board_index: BTreeMap<String, usize>,
    /// Pluggable electrical backend (`DigitalWireFabric` in Phase 2, `dendri-circuit` in Phase 3).
    backend: Box<dyn ElectricalBackend>,
    /// Bounded kernel-wide deterministic trace ring.
    global_trace: TraceRing,
    /// Whether initial boot (`OB_STARTUP` + initial event scheduling) has run.
    booted: bool,
    /// Last error encountered (if any) for snapshot inspection.
    last_error: Option<String>,
}

impl SimKernel {
    /// Builds a new [`SimKernel`] using the default Phase 2 [`DigitalWireFabric`] backend.
    pub fn new(
        project: &Project,
        config: KernelConfig,
        timeline: InputTimeline,
    ) -> Result<Self, KernelError> {
        let fabric = DigitalWireFabric::from_project(project)?;
        Self::with_backend(project, config, timeline, Box::new(fabric))
    }

    /// Builds a new [`SimKernel`] with a custom [`ElectricalBackend`] (e.g. Phase 3 SPICE MNA).
    pub fn with_backend(
        project: &Project,
        config: KernelConfig,
        timeline: InputTimeline,
        backend: Box<dyn ElectricalBackend>,
    ) -> Result<Self, KernelError> {
        let mut boards = Vec::with_capacity(project.boards.len());
        let mut board_index = BTreeMap::new();

        for board in &project.boards {
            if board_index.contains_key(&board.id) {
                return Err(KernelError::DuplicateBoardId(board.id.clone()));
            }
            let vm_cfg = config
                .board_configs
                .get(&board.id)
                .cloned()
                .unwrap_or_else(|| config.default_vm_config.clone());
            let vm = BoardVm::new(board, vm_cfg).map_err(|source| KernelError::Vm {
                board_id: board.id.clone(),
                source,
            })?;

            let scan_period_us = match board.execution {
                ExecutionKind::Cooperative => 0,
                ExecutionKind::PlcScan => {
                    let p = board.scan_period_us.unwrap_or(DEFAULT_PLC_SCAN_PERIOD_US);
                    if p == 0 {
                        return Err(KernelError::ZeroPlcScanPeriod {
                            board_id: board.id.clone(),
                        });
                    }
                    p
                }
                _ => {
                    return Err(KernelError::UnsupportedExecutionKind {
                        board_id: board.id.clone(),
                    });
                }
            };

            // Validate cyclic interrupt periods upfront.
            for &(_, period_us) in vm.cyclic_interrupts() {
                if period_us == 0 {
                    return Err(KernelError::ZeroCyclicPeriod {
                        board_id: board.id.clone(),
                    });
                }
            }

            let idx = boards.len();
            board_index.insert(board.id.clone(), idx);
            boards.push(BoardRuntime {
                board_id: board.id.clone(),
                execution: board.execution,
                scan_period_us,
                vm,
                last_step_us: 0,
                last_hw_int_pin: BTreeMap::new(),
                digital_outputs: BTreeMap::new(),
                analog_outputs: BTreeMap::new(),
                pwm_outputs: BTreeMap::new(),
                synced_trace_total: 0,
            });
        }

        let max_trace = config.max_global_trace_events;
        let mut kernel = Self {
            config,
            time_us: 0,
            next_seq: 0,
            queue: BinaryHeap::new(),
            boards,
            board_index,
            backend,
            global_trace: TraceRing::with_capacity(max_trace),
            booted: false,
            last_error: None,
        };

        for ev in timeline.events {
            kernel.schedule_stimulus(ev);
        }

        Ok(kernel)
    }

    /// Current master simulation clock in microseconds.
    #[must_use]
    pub const fn time_us(&self) -> u64 {
        self.time_us
    }

    /// Deterministic replay seed.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.config.seed
    }

    /// Returns the kernel-wide bounded deterministic trace ring.
    #[must_use]
    pub const fn trace(&self) -> &TraceRing {
        &self.global_trace
    }

    /// Borrows the underlying [`ElectricalBackend`] for inspection (e.g. probes/voltages).
    #[must_use]
    pub fn backend(&self) -> &dyn ElectricalBackend {
        self.backend.as_ref()
    }

    /// Reads a global ASL variable on `board_id` (for assertions, probes & watch tables).
    #[must_use]
    pub fn board_global(&self, board_id: &str, var_name: &str) -> Option<&Value> {
        let &idx = self.board_index.get(board_id)?;
        self.boards.get(idx)?.vm.get_global(var_name)
    }

    /// Returns a board's VM handle for read-only inspection.
    #[must_use]
    pub fn board_vm(&self, board_id: &str) -> Option<&BoardVm> {
        let &idx = self.board_index.get(board_id)?;
        self.boards.get(idx).map(|b| &b.vm)
    }

    /// Schedules an external stimulus event onto the deterministic queue.
    pub fn schedule_stimulus(&mut self, event: StimulusEvent) {
        let time_us = event.time_us.max(self.time_us);
        self.push_event(time_us, PRIO_STIMULUS, EventKind::ExternalStimulus(event));
    }

    /// Dispatches `OB_COMM_EVENT` (`OB50`) handlers bound to `bus_id` on `board_id` at current `time_us`.
    pub fn dispatch_comm_event(&mut self, board_id: &str, bus_id: &str) -> Result<(), KernelError> {
        let &board_idx = self
            .board_index
            .get(board_id)
            .ok_or_else(|| KernelError::UnknownBoardId(board_id.to_owned()))?;
        let now_us = self.time_us;
        let output_changed = self.with_board_io(board_idx, |vm, bid, bridge| {
            vm.sync_to_time_us(now_us);
            vm.run_comm_event_obs(bus_id, bridge)
                .map_err(|source| KernelError::Vm {
                    board_id: bid.to_owned(),
                    source,
                })
        })?;
        self.sync_board_trace(board_idx);
        if output_changed {
            self.propagate_and_dispatch_hw_ints(now_us)?;
        }
        Ok(())
    }

    /// Captures an immutable [`SimSnapshot`] of the entire multi-board simulation state.
    #[must_use]
    pub fn snapshot(&self) -> SimSnapshot {
        let mut boards = BTreeMap::new();
        for br in &self.boards {
            boards.insert(
                br.board_id.clone(),
                BoardSnapshot {
                    board_id: br.board_id.clone(),
                    time_us: br.vm.time_us(),
                    fiber_cycles: br.vm.fiber_cycles(),
                    digital_outputs: br.digital_outputs.clone(),
                    analog_outputs: br.analog_outputs.clone(),
                    pwm_outputs: br.pwm_outputs.clone(),
                    active_sfc_steps: br.vm.all_active_sfc_steps(),
                    trace_len: br.vm.trace().len(),
                },
            );
        }
        SimSnapshot {
            time_us: self.time_us,
            seed: self.config.seed,
            boards,
            global_trace_len: self.global_trace.len(),
            last_error: self.last_error.clone(),
        }
    }

    /// Boots all boards at `t = 0` (`OB_STARTUP` + initial wire solve + initial event schedule).
    pub fn boot(&mut self) -> Result<(), KernelError> {
        if self.booted {
            return Ok(());
        }
        self.booted = true;

        // Solve baseline network before recording initial HW interrupt pin baselines.
        self.backend.solve_at(0);
        for br in &mut self.boards {
            let hw_pins: Vec<String> = br
                .vm
                .hardware_interrupts()
                .iter()
                .filter_map(|(_, label, _, _)| br.vm.pin_for_label(label).map(str::to_owned))
                .collect();
            for pin in hw_pins {
                let level = self.backend.read_board_digital_in(&br.board_id, &pin);
                br.last_hw_int_pin
                    .insert(pin, matches!(level, ElectricalDigitalLevel::High));
            }
        }

        // Boot each board in deterministic declaration order.
        let mut any_output = false;
        for idx in 0..self.boards.len() {
            let changed = self.with_board_io(idx, |vm, bid, bridge| {
                vm.boot(bridge).map_err(|source| KernelError::Vm {
                    board_id: bid.to_owned(),
                    source,
                })
            })?;
            self.sync_board_trace(idx);
            any_output |= changed;

            // Schedule initial board execution and cyclic interrupts.
            let (exec, scan_period, cyclic_list) = match self.boards.get(idx) {
                Some(br) => (
                    br.execution,
                    br.scan_period_us,
                    br.vm.cyclic_interrupts().to_vec(),
                ),
                None => continue,
            };

            match exec {
                ExecutionKind::Cooperative => {
                    self.push_event(
                        0,
                        PRIO_COOP_WAKE,
                        EventKind::CooperativeWake {
                            board_idx: idx,
                            resume_cycles: None,
                        },
                    );
                }
                ExecutionKind::PlcScan => {
                    self.push_event(
                        0,
                        PRIO_PLC_SCAN,
                        EventKind::PlcScanTick {
                            board_idx: idx,
                            period_us: scan_period,
                        },
                    );
                }
                _ => {}
            }

            for (pou_idx, period_us) in cyclic_list {
                self.push_event(
                    period_us,
                    PRIO_CYCLIC_INT,
                    EventKind::CyclicInterrupt {
                        board_idx: idx,
                        pou_idx,
                        period_us,
                    },
                );
            }
        }

        if any_output {
            self.propagate_and_dispatch_hw_ints(0)?;
        }
        Ok(())
    }

    /// Advances the deterministic co-simulation up to and including `target_us`.
    pub fn run_until(&mut self, target_us: u64) -> Result<(), KernelError> {
        if let Err(e) = self.run_until_inner(target_us) {
            self.last_error = Some(e.to_string());
            return Err(e);
        }
        Ok(())
    }

    /// Advances the simulation forward by `delta_us` microseconds from the current clock.
    pub fn step_delta(&mut self, delta_us: u64) -> Result<(), KernelError> {
        let target = self.time_us.saturating_add(delta_us);
        self.run_until(target)
    }

    fn run_until_inner(&mut self, target_us: u64) -> Result<(), KernelError> {
        if !self.booted {
            self.boot()?;
        }

        while let Some(Reverse(peeked)) = self.queue.peek() {
            if peeked.time_us > target_us {
                break;
            }
            let Some(Reverse(ev)) = self.queue.pop() else {
                break;
            };
            self.time_us = self.time_us.max(ev.time_us);
            let now_us = self.time_us;

            match ev.kind {
                EventKind::ExternalStimulus(stim) => {
                    match stim.kind {
                        StimulusKind::Digital(level) => {
                            self.backend
                                .set_external_digital(&stim.instance, &stim.port, level);
                        }
                        StimulusKind::ReleaseDigital => {
                            self.backend
                                .clear_external_digital(&stim.instance, &stim.port);
                        }
                        StimulusKind::Analog(reading) => {
                            self.backend
                                .set_external_analog(&stim.instance, &stim.port, reading);
                        }
                    }
                    self.propagate_and_dispatch_hw_ints(now_us)?;
                }
                EventKind::CyclicInterrupt {
                    board_idx,
                    pou_idx,
                    period_us,
                } => {
                    let changed = self.with_board_io(board_idx, |vm, bid, bridge| {
                        vm.sync_to_time_us(now_us);
                        vm.run_interrupt_ob(pou_idx, bridge)
                            .map_err(|source| KernelError::Vm {
                                board_id: bid.to_owned(),
                                source,
                            })
                    })?;
                    self.sync_board_trace(board_idx);
                    if changed {
                        self.propagate_and_dispatch_hw_ints(now_us)?;
                    }
                    let next_t = now_us.saturating_add(period_us);
                    self.push_event(
                        next_t,
                        PRIO_CYCLIC_INT,
                        EventKind::CyclicInterrupt {
                            board_idx,
                            pou_idx,
                            period_us,
                        },
                    );
                }
                EventKind::PlcScanTick {
                    board_idx,
                    period_us,
                } => {
                    let dt_us = self
                        .boards
                        .get(board_idx)
                        .map_or(0, |br| now_us.saturating_sub(br.last_step_us));
                    let changed = self.with_board_io(board_idx, |vm, bid, bridge| {
                        vm.run_plc_scan(dt_us, bridge)
                            .map_err(|source| KernelError::Vm {
                                board_id: bid.to_owned(),
                                source,
                            })
                    })?;
                    if let Some(br) = self.boards.get_mut(board_idx) {
                        br.last_step_us = now_us;
                    }
                    self.sync_board_trace(board_idx);
                    if changed {
                        self.propagate_and_dispatch_hw_ints(now_us)?;
                    }
                    let next_t = now_us.saturating_add(period_us);
                    self.push_event(
                        next_t,
                        PRIO_PLC_SCAN,
                        EventKind::PlcScanTick {
                            board_idx,
                            period_us,
                        },
                    );
                }
                EventKind::CooperativeWake {
                    board_idx,
                    resume_cycles,
                } => {
                    // Determine exact cooperative deadline so polling loops yield right when
                    // the next external/PLC/electrical event arrives (`02` §2.1).
                    let next_queue_us = self.queue.peek().map_or(u64::MAX, |Reverse(e)| e.time_us);
                    let next_elec_us = self.backend.next_electrical_event_us().unwrap_or(u64::MAX);
                    let slice_cap = now_us.saturating_add(self.config.coop_slice_us.max(1));
                    let stop_at_us = next_queue_us
                        .min(next_elec_us)
                        .min(slice_cap)
                        .min(target_us)
                        .max(now_us.saturating_add(1));

                    let (before_cycles, dt_us) = match self.boards.get_mut(board_idx) {
                        Some(br) => {
                            if let Some(wc) = resume_cycles {
                                br.vm.resume_at_cycles(wc);
                            } else {
                                br.vm.sync_to_time_us(now_us);
                            }
                            let dt = i64::try_from(now_us.saturating_sub(br.last_step_us))
                                .unwrap_or(i64::MAX);
                            (br.vm.fiber_cycles(), dt)
                        }
                        None => continue,
                    };

                    let (outcome, output_changed, after_cycles, after_us) =
                        self.step_cooperative_board(board_idx, stop_at_us, dt_us)?;

                    if let Some(br) = self.boards.get_mut(board_idx) {
                        br.last_step_us = after_us.max(now_us);
                    }
                    self.sync_board_trace(board_idx);

                    if output_changed {
                        let prop_t = after_us.max(now_us);
                        self.propagate_and_dispatch_hw_ints(prop_t)?;
                    }

                    match outcome {
                        StepOutcome::YieldWait {
                            wake_us,
                            wake_cycles,
                        } => {
                            self.push_event(
                                wake_us.max(now_us),
                                PRIO_COOP_WAKE,
                                EventKind::CooperativeWake {
                                    board_idx,
                                    resume_cycles: Some(wake_cycles),
                                },
                            );
                        }
                        StepOutcome::IoOutputChanged { now_us: io_us }
                        | StepOutcome::TimeSliceReached { now_us: io_us } => {
                            self.push_event(
                                io_us.max(now_us),
                                PRIO_COOP_WAKE,
                                EventKind::CooperativeWake {
                                    board_idx,
                                    resume_cycles: None,
                                },
                            );
                        }
                        StepOutcome::Completed => {
                            // Only re-schedule `OB_MAIN` if it actually executed instructions;
                            // an empty `OB_MAIN` stays idle while interrupts remain armed.
                            if after_cycles > before_cycles {
                                self.push_event(
                                    after_us.max(now_us),
                                    PRIO_COOP_WAKE,
                                    EventKind::CooperativeWake {
                                        board_idx,
                                        resume_cycles: None,
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }

        self.time_us = self.time_us.max(target_us);
        Ok(())
    }

    fn step_cooperative_board(
        &mut self,
        board_idx: usize,
        stop_at_us: u64,
        dt_us: i64,
    ) -> Result<(StepOutcome, bool, u64, u64), KernelError> {
        let Some(br) = self.boards.get_mut(board_idx) else {
            return Err(KernelError::UnknownBoardId(format!("board#{board_idx}")));
        };
        let mut bridge = BoardIoBridge {
            board_id: &br.board_id,
            backend: self.backend.as_mut(),
            digital_outputs: &mut br.digital_outputs,
            analog_outputs: &mut br.analog_outputs,
            pwm_outputs: &mut br.pwm_outputs,
            output_changed: false,
        };
        let outcome = br
            .vm
            .step_cooperative_until(Some(stop_at_us), true, dt_us, &mut bridge)
            .map_err(|source| KernelError::Vm {
                board_id: br.board_id.clone(),
                source,
            })?;
        let changed = bridge.output_changed;
        let after_cycles = br.vm.fiber_cycles();
        let after_us = br.vm.time_us();
        Ok((outcome, changed, after_cycles, after_us))
    }

    fn with_board_io<F>(&mut self, board_idx: usize, f: F) -> Result<bool, KernelError>
    where
        F: FnOnce(&mut BoardVm, &str, &mut BoardIoBridge<'_>) -> Result<(), KernelError>,
    {
        let Some(br) = self.boards.get_mut(board_idx) else {
            return Err(KernelError::UnknownBoardId(format!("board#{board_idx}")));
        };
        let mut bridge = BoardIoBridge {
            board_id: &br.board_id,
            backend: self.backend.as_mut(),
            digital_outputs: &mut br.digital_outputs,
            analog_outputs: &mut br.analog_outputs,
            pwm_outputs: &mut br.pwm_outputs,
            output_changed: false,
        };
        f(&mut br.vm, &br.board_id, &mut bridge)?;
        Ok(bridge.output_changed)
    }

    fn propagate_and_dispatch_hw_ints(&mut self, time_us: u64) -> Result<(), KernelError> {
        for _depth in 0..self.config.max_isr_cascade {
            self.backend.solve_at(time_us);

            // Detect edge transitions deterministically across all boards and update baselines
            // BEFORE executing handlers so multiple ISRs on one pin share a consistent snapshot.
            let mut triggered: Vec<(usize, usize)> = Vec::new();
            for (board_idx, br) in self.boards.iter_mut().enumerate() {
                let mut pin_transitions: BTreeMap<String, (bool, bool)> = BTreeMap::new();
                let hw_ints: Vec<(usize, String, InterruptEdge)> = br
                    .vm
                    .hardware_interrupts()
                    .iter()
                    .filter_map(|(pou_idx, label, edge, _prio)| {
                        br.vm
                            .pin_for_label(label)
                            .map(|pin| (*pou_idx, pin.to_owned(), *edge))
                    })
                    .collect();

                for &(pou_idx, ref pin, edge) in &hw_ints {
                    let (prev, next) = *pin_transitions.entry(pin.clone()).or_insert_with(|| {
                        let old_val = br.last_hw_int_pin.get(pin).copied().unwrap_or(false);
                        let raw = self.backend.read_board_digital_in(&br.board_id, pin);
                        let new_val = match raw {
                            ElectricalDigitalLevel::High => true,
                            ElectricalDigitalLevel::Low => false,
                            ElectricalDigitalLevel::Indeterminate => old_val,
                        };
                        (old_val, new_val)
                    });
                    if edge_matches(edge, prev, next) {
                        triggered.push((board_idx, pou_idx));
                    }
                }

                for (pin, (_, new_val)) in pin_transitions {
                    br.last_hw_int_pin.insert(pin, new_val);
                }
            }

            if triggered.is_empty() {
                return Ok(());
            }

            let mut any_output = false;
            for (board_idx, pou_idx) in triggered {
                let changed = self.with_board_io(board_idx, |vm, bid, bridge| {
                    vm.sync_to_time_us(time_us);
                    vm.run_interrupt_ob(pou_idx, bridge)
                        .map_err(|source| KernelError::Vm {
                            board_id: bid.to_owned(),
                            source,
                        })
                })?;
                self.sync_board_trace(board_idx);
                any_output |= changed;
            }

            if !any_output {
                return Ok(());
            }
        }

        Err(KernelError::IsrCascadeOverflow {
            time_us,
            limit: self.config.max_isr_cascade,
        })
    }

    fn sync_board_trace(&mut self, board_idx: usize) {
        let Some(br) = self.boards.get_mut(board_idx) else {
            return;
        };
        let trace = br.vm.trace();
        let retained = trace.len();
        let total = trace
            .dropped_count()
            .saturating_add(u64::try_from(retained).unwrap_or(u64::MAX));
        let new_count = total.saturating_sub(br.synced_trace_total);
        if new_count == 0 {
            return;
        }
        let to_take = usize::try_from(new_count).unwrap_or(retained).min(retained);
        let skip = retained.saturating_sub(to_take);
        let events: Vec<TraceEvent> = trace.iter().skip(skip).cloned().collect();
        br.synced_trace_total = total;
        for ev in events {
            self.global_trace.push(ev);
        }
    }

    fn push_event(&mut self, time_us: u64, priority: u8, kind: EventKind) {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        self.queue.push(Reverse(ScheduledEvent {
            time_us,
            priority,
            seq,
            kind,
        }));
    }
}

const fn edge_matches(edge: InterruptEdge, prev: bool, next: bool) -> bool {
    match edge {
        InterruptEdge::Rising => !prev && next,
        InterruptEdge::Falling => prev && !next,
        InterruptEdge::Change => prev != next,
        InterruptEdge::Low => prev && !next,
        InterruptEdge::High => !prev && next,
    }
}
