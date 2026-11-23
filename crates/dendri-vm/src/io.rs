//! Abstract hardware I/O boundary (`BoardIoInterface`) decoupling `dendri-vm`
//! completely from physics engines, SPICE netlists, and board profiles (`02` §1).

use dendri_ast::{ASL_ANALOG_FULL_SCALE, AnalogStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Three-valued electrical level read across the physical pin boundary (`02` §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElectricalDigitalLevel {
    Low,
    High,
    Indeterminate,
}

/// Analog channel reading crossing the Solver -> ASL boundary (`01` §3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AnalogReading {
    /// Fixed-point analog quantity at scale `M = 10⁹` (`i32` supports bipolar & S7 over/underrange).
    pub analog: i32,
    /// Separate status channel (`Ok | Underrange | Overrange | Fault`).
    pub status: AnalogStatus,
}

impl Default for AnalogReading {
    fn default() -> Self {
        Self {
            analog: 0,
            status: AnalogStatus::Ok,
        }
    }
}

/// Abstract hardware interface implemented by `dendri-sim-kernel` / `dendri-circuit` / test doubles.
///
/// `dendri-vm` knows ZERO physics and ZERO board profiles — every physical read or write
/// crosses this trait.
pub trait BoardIoInterface {
    /// Reads the electrical digital level of a physical board port (`D2`, `Ia.0`, …).
    fn read_digital(&self, pin: &str) -> ElectricalDigitalLevel;

    /// Drives a digital output level onto a physical board port (`D13`, `Qa.0`, …).
    fn write_digital(&mut self, pin: &str, high: bool);

    /// Reads the normalized fixed-point analog quantity (`M = 10⁹`) and status on a port (`A0`, `IW64`, …).
    fn read_analog(&self, pin: &str) -> AnalogReading;

    /// Writes a normalized fixed-point analog quantity (`M = 10⁹`) onto an analog output port.
    fn write_analog(&mut self, pin: &str, analog: i32);

    /// Drives an unsigned PWM duty cycle (`0 ..= ASL_ANALOG_FULL_SCALE`) onto a PWM port.
    fn write_pwm(&mut self, pin: &str, duty: u32);

    /// Transmits a UTF-8 byte payload on a UART/serial channel (used by `UART_SEND`).
    fn uart_send(&mut self, channel: &str, payload: &str) -> bool {
        let _ = (channel, payload);
        true
    }

    /// Receives the next pending UTF-8 payload on a UART/serial channel, if available.
    fn uart_recv(&mut self, channel: &str) -> Option<String> {
        let _ = channel;
        None
    }

    /// Writes a string to a display sink component (`DISPLAY_PRINT`).
    fn display_print(&mut self, target: &str, text: &str) {
        let _ = (target, text);
    }
}

/// Deterministic in-memory [`BoardIoInterface`] for unit tests and standalone VM execution.
#[derive(Debug, Clone, Default)]
pub struct MemoryBoardIo {
    pub digital_inputs: BTreeMap<String, ElectricalDigitalLevel>,
    pub digital_outputs: BTreeMap<String, bool>,
    pub analog_inputs: BTreeMap<String, AnalogReading>,
    pub analog_outputs: BTreeMap<String, i32>,
    pub pwm_outputs: BTreeMap<String, u32>,
    pub uart_tx_log: Vec<(String, String)>,
    pub uart_rx_queue: BTreeMap<String, Vec<String>>,
    pub display_log: Vec<(String, String)>,
}

impl MemoryBoardIo {
    /// Sets a digital input pin to a concrete boolean level.
    pub fn set_digital_in(&mut self, pin: impl Into<String>, high: bool) {
        let lvl = if high {
            ElectricalDigitalLevel::High
        } else {
            ElectricalDigitalLevel::Low
        };
        self.digital_inputs.insert(pin.into(), lvl);
    }

    /// Sets a digital input pin to an explicit [`ElectricalDigitalLevel`] (incl. `Indeterminate`).
    pub fn set_digital_level(&mut self, pin: impl Into<String>, level: ElectricalDigitalLevel) {
        self.digital_inputs.insert(pin.into(), level);
    }

    /// Sets an analog input pin reading (`M = 10⁹`).
    pub fn set_analog_in(&mut self, pin: impl Into<String>, analog: i32, status: AnalogStatus) {
        self.analog_inputs
            .insert(pin.into(), AnalogReading { analog, status });
    }
}

impl BoardIoInterface for MemoryBoardIo {
    fn read_digital(&self, pin: &str) -> ElectricalDigitalLevel {
        if let Some(&lvl) = self.digital_inputs.get(pin) {
            return lvl;
        }
        if let Some(&out) = self.digital_outputs.get(pin) {
            return if out {
                ElectricalDigitalLevel::High
            } else {
                ElectricalDigitalLevel::Low
            };
        }
        ElectricalDigitalLevel::Low
    }

    fn write_digital(&mut self, pin: &str, high: bool) {
        self.digital_outputs.insert(pin.to_owned(), high);
    }

    fn read_analog(&self, pin: &str) -> AnalogReading {
        self.analog_inputs.get(pin).copied().unwrap_or_default()
    }

    fn write_analog(&mut self, pin: &str, analog: i32) {
        self.analog_outputs.insert(pin.to_owned(), analog);
    }

    fn write_pwm(&mut self, pin: &str, duty: u32) {
        let clamped = duty.min(u32::try_from(ASL_ANALOG_FULL_SCALE).unwrap_or(u32::MAX));
        self.pwm_outputs.insert(pin.to_owned(), clamped);
    }

    fn uart_send(&mut self, channel: &str, payload: &str) -> bool {
        self.uart_tx_log
            .push((channel.to_owned(), payload.to_owned()));
        true
    }

    fn uart_recv(&mut self, channel: &str) -> Option<String> {
        let queue = self.uart_rx_queue.get_mut(channel)?;
        if queue.is_empty() {
            None
        } else {
            Some(queue.remove(0))
        }
    }

    fn display_print(&mut self, target: &str, text: &str) {
        self.display_log.push((target.to_owned(), text.to_owned()));
    }
}
