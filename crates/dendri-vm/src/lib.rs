//! Deterministic ASL v2 virtual machine (`dendri-vm`, `02-simulation-and-electrical-engine.md` §2).
//!
//! Zero physics and zero board-profile dependencies: communicates exclusively via
//! [`BoardIoInterface`], maintains exact 128-bit cycle-based virtual time (`fiber_cycles`),
//! enforces lexical frames, per-scan instruction budgets, `max_call_depth`, bounded
//! trace rings, exact IEC 61131-3 Function Blocks, and load-time Ladder/GRAFCET lowering.

pub mod fb;
pub mod io;
mod lower;
pub mod value;
pub mod vm;

pub use fb::FbState;
pub use io::{AnalogReading, BoardIoInterface, ElectricalDigitalLevel, MemoryBoardIo};
pub use value::Value;
pub use vm::{BoardVm, StepOutcome};

use dendri_ast::{Codec, NodeId};
use dendri_span::ErrorCode;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use thiserror::Error;

/// Digital read resolution rule when electrical pin level is `Indeterminate` (`02` §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndeterminatePolicy {
    #[default]
    Previous,
    Low,
    High,
}

/// Integer overflow behavior policy (`01` §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverflowPolicy {
    #[default]
    Wrap,
    Trap,
}

/// Cycle costs per instruction / I/O class declared in board profile `[execution.cost]` (`02` §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleCostModel {
    pub stmt_cycles: u64,
    pub branch_cycles: u64,
    pub call_cycles: u64,
    pub io_read_cycles: u64,
    pub io_write_cycles: u64,
    pub adc_read_cycles: u64,
}

impl Default for CycleCostModel {
    fn default() -> Self {
        // Representative 16 MHz AVR defaults (62.5 ns / cycle -> 4 cycles = 250 ns)
        Self {
            stmt_cycles: 4,
            branch_cycles: 2,
            call_cycles: 8,
            io_read_cycles: 6,
            io_write_cycles: 6,
            adc_read_cycles: 1_664, // ~104 us AVR 10-bit ADC conversion
        }
    }
}

/// Configuration for a [`BoardVm`] instance.
#[derive(Debug, Clone)]
pub struct VmConfig {
    /// CPU clock frequency in Hz (`[cpu] clock_hz`, e.g., `16_000_000`).
    pub clock_hz: u64,
    /// Per-statement / I/O cycle cost model (`[execution.cost]`).
    pub cost: CycleCostModel,
    /// Policy for reading an electrically indeterminate digital input (`02` §2.3).
    pub indeterminate_reads_as: IndeterminatePolicy,
    /// Integer overflow policy (`Wrap` default or `Trap`).
    pub overflow: OverflowPolicy,
    /// Hard instruction budget reset at the start of every scan/resume quantum.
    pub max_steps_per_scan: usize,
    /// Hard call-stack depth limit (`02` §2.3, default 64).
    pub max_call_depth: usize,
    /// Maximum ring-buffer capacity for trace events (default `100_000`).
    pub max_trace_events: usize,
    /// Maximum transient SFC evolution steps per scan before instability error.
    pub max_sfc_stability_steps: usize,
    /// Board channel codecs for `ANALOG_TO_CODE` / `CODE_TO_ANALOG` / `ADC_CODE`.
    pub codecs: BTreeMap<String, Codec>,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            clock_hz: 16_000_000,
            cost: CycleCostModel::default(),
            indeterminate_reads_as: IndeterminatePolicy::Previous,
            overflow: OverflowPolicy::Wrap,
            max_steps_per_scan: 100_000,
            max_call_depth: 64,
            max_trace_events: 100_000,
            max_sfc_stability_steps: 64,
            codecs: BTreeMap::new(),
        }
    }
}

/// Structured runtime failure returned by [`BoardVm`] (NEVER a Rust panic!).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RuntimeError {
    #[error("maximum call depth ({limit}) exceeded calling `{callee}`")]
    StackOverflow { callee: String, limit: usize },
    #[error("per-scan instruction budget ({budget}) exceeded (possible infinite loop)")]
    ScanBudgetExceeded { budget: usize },
    #[error("division or modulo by zero")]
    DivisionByZero,
    #[error("integer overflow trap")]
    IntegerOverflow,
    #[error("array index {index} out of bounds (len {len})")]
    IndexOutOfBounds { index: i128, len: usize },
    #[error("runtime type error: {0}")]
    TypeError(String),
    #[error("undeclared runtime symbol `{0}`")]
    UndeclaredSymbol(String),
    #[error("unknown field `{0}`")]
    UnknownField(String),
    #[error("unresolved function `{0}`")]
    UnresolvedFunction(String),
    #[error("unknown codec `{0}`")]
    UnknownCodec(String),
    #[error("break statement outside loop")]
    BreakOutsideLoop,
    #[error("continue statement outside loop")]
    ContinueOutsideLoop,
    #[error("cyclic module import graph")]
    ImportCycle,
    #[error("invalid CPU clock_hz (must be > 0)")]
    InvalidClockHz,
    #[error("SFC chart `{chart}` exceeded {limit} transient stability evolutions in one scan")]
    SfcInstabilityLoop { chart: String, limit: usize },
    /// Four-part `Foreign` halt diagnostic: what, where, why, what now (`01` §6.2).
    #[error(
        "Foreign construct `{text}` ({language}) at node `{node_id}` halted simulation: fidelity `{fidelity}` is not modeled for execution. Action: record an explicit [[boards.assumptions]] entry, install an F2 pack, or replace with an ASL equivalent."
    )]
    ForeignHalt {
        node_id: NodeId,
        language: String,
        fidelity: String,
        text: String,
    },
}

/// Deterministic trace record captured in the bounded [`TraceRing`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEvent {
    pub time_us: u64,
    pub board_id: String,
    pub kind: TraceEventKind,
}

/// Typed trace event payload for golden comparisons & virtual logic analyzer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TraceEventKind {
    DigitalWrite {
        pin: String,
        high: bool,
    },
    AnalogWrite {
        pin: String,
        analog: i32,
    },
    PwmWrite {
        pin: String,
        duty: u32,
    },
    Diagnostic {
        code: ErrorCode,
        message: String,
    },
    SfcStepActive {
        chart: String,
        step: String,
        active: bool,
    },
    UartTx {
        channel: String,
        payload: String,
    },
    DisplayPrint {
        target: String,
        text: String,
    },
}

/// Bounded FIFO ring buffer preventing unbounded trace memory growth (`02` §2.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceRing {
    capacity: usize,
    events: VecDeque<TraceEvent>,
    dropped: u64,
}

impl TraceRing {
    /// Creates a ring buffer bounded to `capacity` events.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity,
            events: VecDeque::with_capacity(capacity.min(1024)),
            dropped: 0,
        }
    }

    /// Pushes an event, evicting the oldest event if at capacity.
    pub fn push(&mut self, event: TraceEvent) {
        if self.capacity == 0 {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        if self.events.len() >= self.capacity {
            self.events.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.events.push_back(event);
    }

    /// Returns an iterator over retained events in chronological order.
    pub fn iter(&self) -> impl Iterator<Item = &TraceEvent> {
        self.events.iter()
    }

    /// Number of currently retained events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Returns `true` if no events are retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Count of oldest events evicted due to ring capacity.
    #[must_use]
    pub const fn dropped_count(&self) -> u64 {
        self.dropped
    }
}
