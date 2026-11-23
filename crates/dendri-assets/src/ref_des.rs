//! IEC 81346-2 / IEC 61082 reference-designation scheme for electrotechnical schematics
//! (`03-asset-and-profile-contract.md` §3.2, closes `DEF-009`).
//!
//! Standardized product-aspect prefixes (`-<LETTER>`, numbered `-<LETTER><N>` on schematics):
//! - `-A`: Multi-function assembly / controller (MCU & PLC boards)
//! - `-B`: Converting physical input to signal (sensors, pressure transmitters, limit switches)
//! - `-C`: Storing energy/material (capacitors, batteries)
//! - `-G`: Generating electrical energy (power supplies, generators)
//! - `-H`: Providing perceptible information (LEDs, pilot lamps, buzzers, displays)
//! - `-K`: Processing signals/information (relays, contactors, timers)
//! - `-L`: Inductors / chokes
//! - `-M`: Providing mechanical energy (motors, actuators, solenoids)
//! - `-P`: Presenting/recording measurements (meters, oscilloscopes)
//! - `-Q`: Controlling energy flow (circuit breakers, power switches, isolators)
//! - `-R`: Restricting or stabilizing electrical flow (resistors, shunts, potentiometers)
//! - `-S`: Detecting human action (pushbuttons, tactile switches, selector switches, E-stop)
//! - `-T`: Transforming energy (transformers, DC-DC converters)
//! - `-V`: Semiconductor switching/rectifying elements (diodes, BJTs, MOSFETs, SCRs, TRIACs)
//! - `-X`: Connecting objects (terminal blocks, headers, breadboard rails)

use serde::{Deserialize, Serialize};

/// Normative IEC 81346-2 single-letter product-aspect class (`DEF-009`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RefDesClass {
    /// `-A`: Multi-function assembly / programmable controller (`-A1`, `-A2`).
    A,
    /// `-B`: Sensors & transducers (`-B1`, `-B2`).
    B,
    /// `-C`: Capacitors / storage elements (`-C1`).
    C,
    /// `-G`: Power sources / generators (`-G1`).
    G,
    /// `-H`: Optical / acoustic indicators (`-H1`, `-H2`).
    H,
    /// `-K`: Relays / signal processing devices (`-K1`).
    K,
    /// `-L`: Inductors / coils (`-L1`).
    L,
    /// `-M`: Motors / mechanical actuators (`-M1`).
    M,
    /// `-P`: Measuring & presenting instruments (`-P1`).
    P,
    /// `-Q`: Power switching & protective devices (`-Q1`).
    Q,
    /// `-R`: Resistors, shunts & potentiometers (`-R1`, `-R2`).
    R,
    /// `-S`: Human-operated switches & pushbuttons (`-S1`, `-S2`).
    S,
    /// `-T`: Transformers & galvanic converters (`-T1`).
    T,
    /// `-V`: Semiconductors & diodes (`-V1`).
    V,
    /// `-X`: Terminals, connectors & headers (`-X1`).
    X,
}

impl RefDesClass {
    /// All supported IEC 81346-2 single-letter classes.
    pub const ALL: &'static [Self] = &[
        Self::A,
        Self::B,
        Self::C,
        Self::G,
        Self::H,
        Self::K,
        Self::L,
        Self::M,
        Self::P,
        Self::Q,
        Self::R,
        Self::S,
        Self::T,
        Self::V,
        Self::X,
    ];

    /// Canonical product-aspect prefix (e.g., `"-S"`, `"-H"`, `"-R"`).
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::A => "-A",
            Self::B => "-B",
            Self::C => "-C",
            Self::G => "-G",
            Self::H => "-H",
            Self::K => "-K",
            Self::L => "-L",
            Self::M => "-M",
            Self::P => "-P",
            Self::Q => "-Q",
            Self::R => "-R",
            Self::S => "-S",
            Self::T => "-T",
            Self::V => "-V",
            Self::X => "-X",
        }
    }

    /// Parses a prefix string (e.g., `"-S"`, `"-H"`, `"-R"`) into [`RefDesClass`].
    #[must_use]
    pub fn from_prefix(prefix: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.prefix() == prefix)
    }

    /// Engineering class description per IEC 81346-2.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::A => "Two or more functions / programmable controller assembly",
            Self::B => "Converting physical input variable into a signal (sensor/transmitter)",
            Self::C => "Storing material or electrical energy (capacitor)",
            Self::G => "Initiating an energy or material flow (power supply)",
            Self::H => "Producing perceptible information (LED, pilot lamp, indicator)",
            Self::K => "Processing signals or information (relay, contactor coil)",
            Self::L => "Inductive energy element (inductor, choke)",
            Self::M => "Providing mechanical energy (motor, actuator)",
            Self::P => "Presenting information / measuring instrument",
            Self::Q => "Controlled switching or varying of energy flow (breaker/contactor)",
            Self::R => "Restricting or stabilizing flow (resistor, shunt, potentiometer)",
            Self::S => "Converting manual operation into signal (pushbutton, switch)",
            Self::T => "Transforming energy (transformer)",
            Self::V => "Processing electrical energy via semiconductors (diode, transistor)",
            Self::X => "Connecting objects (terminal block, header)",
        }
    }
}

/// Returns `true` iff `prefix` is a valid IEC 81346-2 schematic reference-designation prefix.
#[must_use]
pub fn is_valid_ref_designator_prefix(prefix: &str) -> bool {
    RefDesClass::from_prefix(prefix).is_some()
}

/// Formats an indexed IEC 81346-2 reference designator (e.g., `format_ref_designator(RefDesClass::S, 1) == "-S1"`).
#[must_use]
pub fn format_ref_designator(class: RefDesClass, index: u32) -> String {
    format!("{}{}", class.prefix(), index.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iec_81346_prefixes_round_trip_and_format_cleanly() {
        for &c in RefDesClass::ALL {
            assert!(is_valid_ref_designator_prefix(c.prefix()));
            assert_eq!(RefDesClass::from_prefix(c.prefix()), Some(c));
            assert!(!c.description().is_empty());
        }
        assert_eq!(format_ref_designator(RefDesClass::S, 1), "-S1");
        assert_eq!(format_ref_designator(RefDesClass::H, 2), "-H2");
        assert_eq!(format_ref_designator(RefDesClass::R, 3), "-R3");
        assert_eq!(format_ref_designator(RefDesClass::B, 1), "-B1");
        assert_eq!(format_ref_designator(RefDesClass::A, 1), "-A1");
        assert!(!is_valid_ref_designator_prefix("S1"));
        assert!(!is_valid_ref_designator_prefix("-Z"));
    }
}
