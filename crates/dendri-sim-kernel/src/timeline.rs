//! Deterministic external stimulus timeline for bit-identical simulation replay (`02` §2.1).

use dendri_vm::{AnalogReading, ElectricalDigitalLevel};
use serde::{Deserialize, Serialize};

/// External electrical stimulus injected at an exact simulation microsecond.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StimulusKind {
    /// Drives a digital port (`High`, `Low`, or `Indeterminate`).
    Digital(ElectricalDigitalLevel),
    /// Releases an external digital driver so the port returns to its wired/pulled state.
    ReleaseDigital,
    /// Drives an analog channel reading (fixed-point `M = 10^9` + status).
    Analog(AnalogReading),
}

/// One timestamped external stimulus event targeting `<instance>.<port>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StimulusEvent {
    pub time_us: u64,
    pub instance: String,
    pub port: String,
    pub kind: StimulusKind,
}

/// Ordered deterministic external input timeline (`02` §2.1).
///
/// Given identical `(Project, seed, InputTimeline)`, `SimKernel` MUST produce
/// bit-identical digital and integer simulation traces across platforms.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputTimeline {
    pub events: Vec<StimulusEvent>,
}

impl InputTimeline {
    /// Creates an empty timeline.
    #[must_use]
    pub const fn new() -> Self {
        Self { events: Vec::new() }
    }

    /// Appends a digital stimulus event.
    pub fn push_digital(
        &mut self,
        time_us: u64,
        instance: impl Into<String>,
        port: impl Into<String>,
        level: ElectricalDigitalLevel,
    ) {
        self.events.push(StimulusEvent {
            time_us,
            instance: instance.into(),
            port: port.into(),
            kind: StimulusKind::Digital(level),
        });
    }

    /// Appends a release-digital stimulus event.
    pub fn push_release_digital(
        &mut self,
        time_us: u64,
        instance: impl Into<String>,
        port: impl Into<String>,
    ) {
        self.events.push(StimulusEvent {
            time_us,
            instance: instance.into(),
            port: port.into(),
            kind: StimulusKind::ReleaseDigital,
        });
    }

    /// Appends an analog stimulus event.
    pub fn push_analog(
        &mut self,
        time_us: u64,
        instance: impl Into<String>,
        port: impl Into<String>,
        reading: AnalogReading,
    ) {
        self.events.push(StimulusEvent {
            time_us,
            instance: instance.into(),
            port: port.into(),
            kind: StimulusKind::Analog(reading),
        });
    }
}
