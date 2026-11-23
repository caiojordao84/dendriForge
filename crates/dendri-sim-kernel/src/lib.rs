//! Deterministic multi-board co-simulation kernel (`02-simulation-and-electrical-engine.md` §1–§2).
//!
//! Orchestrates heterogeneous boards (`ExecutionKind::Cooperative` MCUs and
//! `ExecutionKind::PlcScan` PLCs) under a single monotonic microsecond master clock
//! (`time_us: u64`), a deterministic discrete-event queue, a pluggable [`ElectricalBackend`],
//! deterministic [`InputTimeline`] replay, and a dedicated background [`SimWorker`] thread
//! publishing immutable `Arc<SimSnapshot>` frames (`02` §2.4).

pub mod backend;
pub mod kernel;
pub mod timeline;
pub mod worker;

pub use backend::{DigitalWireFabric, ElectricalBackend};
pub use kernel::{BoardSnapshot, SimKernel, SimSnapshot};
pub use timeline::{InputTimeline, StimulusEvent, StimulusKind};
pub use worker::{SimWorker, WorkerCommand, WorkerState};

use dendri_vm::{RuntimeError, VmConfig};
use std::collections::BTreeMap;
use thiserror::Error;

/// Default cooperative time slice in microseconds before yielding back to the event queue
/// when no earlier event is scheduled (`02` §2.1).
pub const DEFAULT_COOP_SLICE_US: u64 = 1_000;

/// Default PLC scan period in microseconds when a `PlcScan` board omits `scan_period_us` (`02` §2.2).
pub const DEFAULT_PLC_SCAN_PERIOD_US: u64 = 10_000;

/// Configuration for constructing a [`SimKernel`].
#[derive(Debug, Clone)]
pub struct KernelConfig {
    /// Deterministic replay seed (`02` §2.1).
    pub seed: u64,
    /// Maximum trace events retained in the kernel-wide bounded ring buffer.
    pub max_global_trace_events: usize,
    /// Maximum cooperative slice duration (µs) before returning to the event queue.
    pub coop_slice_us: u64,
    /// Maximum ISR cascade depth at one instant before halting oscillation loops.
    pub max_isr_cascade: usize,
    /// Default [`VmConfig`] applied to boards without a specific entry in `board_configs`.
    pub default_vm_config: VmConfig,
    /// Optional per-board [`VmConfig`] overrides keyed by `board.id`.
    pub board_configs: BTreeMap<String, VmConfig>,
}

impl Default for KernelConfig {
    fn default() -> Self {
        Self {
            seed: 0,
            max_global_trace_events: 100_000,
            coop_slice_us: DEFAULT_COOP_SLICE_US,
            max_isr_cascade: 32,
            default_vm_config: VmConfig::default(),
            board_configs: BTreeMap::new(),
        }
    }
}

/// Typed runtime errors emitted by [`SimKernel`] without panicking.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum KernelError {
    #[error("board `{board_id}` VM error: {source}")]
    Vm {
        board_id: String,
        #[source]
        source: RuntimeError,
    },
    #[error("duplicate board id `{0}` in project")]
    DuplicateBoardId(String),
    #[error("unknown board id `{0}`")]
    UnknownBoardId(String),
    #[error("unsupported execution kind on board `{board_id}`")]
    UnsupportedExecutionKind { board_id: String },
    #[error("malformed wire endpoint `{endpoint}` on wire `{wire_id}`: {reason}")]
    MalformedWireEndpoint {
        wire_id: String,
        endpoint: String,
        reason: String,
    },
    #[error("hardware interrupt cascade depth exceeded ({limit}) at t={time_us}us")]
    IsrCascadeOverflow { time_us: u64, limit: usize },
    #[error("invalid cyclic interrupt period 0 on board `{board_id}`")]
    ZeroCyclicPeriod { board_id: String },
    #[error("invalid PLC scan period 0 on board `{board_id}`")]
    ZeroPlcScanPeriod { board_id: String },
}
