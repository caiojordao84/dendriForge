//! Phase 6b — Real-Toolchain Differential Gate (`ADR-004`, `DEF-003`, CI Gate 10).
//!
//! Compiles and executes emitted Wave 1 code (C++, IEC 61131-3 ST, Ladder projected to ST/C++,
//! GRAFCET/SFC projected to ST, plus the full `C3` hostile comment corpus) across real external
//! toolchains and verifies bit-identical traces against `dendri-vm`:
//! - **Host C++ (`g++`):** Deterministic Arduino API shim (virtual clock, GPIO/ADC/PWM, Serial).
//! - **IEC 61131-3 ST / Ladder / GRAFCET (`RuSTy` `plc`):** Pure-Rust LLVM native compilation
//!   (zero C transpilation step) linked against a deterministic virtual-clock & I/O shim.
//! - **Arduino Official Core (`arduino-cli`):** Compile-only verification targeting `arduino:avr:uno`.
//! - **Independent AVR Ground-Truth Oracle (`avr-gcc` + `simavr`):** Real ATmega328P execution
//!   verifying 16-bit `int`, 10-bit ADC, Timer 0/1 PWM (`DEF-001`), and bare-metal `PORTB` GPIO.

pub mod arduino_cli;
pub mod avr_simavr;
pub mod cpp_host;
pub mod rusty_st;
pub mod scenarios;
pub mod toolchain;

use crate::arduino_cli::verify_cpp_scenario_arduino_cli;
use crate::avr_simavr::run_avr_simavr_oracle;
use crate::cpp_host::run_cpp_host_scenario;
use crate::rusty_st::run_st_rusty_scenario;
use crate::scenarios::{build_cpp_scenarios, build_st_rusty_scenarios};
pub use crate::toolchain::{DiffError, Toolchains};

/// Selects which real-toolchain differential gate suite(s) to execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSuite {
    /// Host `g++` + deterministic Arduino API shim (6 Wave 1 C++ scenarios incl. Ladder->C++ & C3).
    CppHost,
    /// RuSTy `plc` (LLVM native) + deterministic I/O shim (6 ST/Ladder/SFC scenarios incl. C3).
    RustySt,
    /// Default PR / workspace CI gate: `CppHost` + `RustySt`.
    PrGate,
    /// Official `arduino-cli` (`arduino:avr:uno`) compile-only gate across all Wave 1 C++ scenarios.
    ArduinoCli,
    /// Independent ATmega328P ground-truth oracle (`avr-gcc` + `simavr`, 4 AVR hardware scenarios).
    AvrSimavr,
    /// Complete Phase 6b Wave 1 Exit gate: all 4 real toolchains (`CppHost` + `RustySt` + `ArduinoCli` + `AvrSimavr`).
    All,
}

/// Outcome of one executed differential scenario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioOutcome {
    pub suite: &'static str,
    pub scenario: &'static str,
    pub detail: String,
}

/// Summary report returned by [`run_differential_suite`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffReport {
    pub outcomes: Vec<ScenarioOutcome>,
}

/// Executes the requested Phase 6b differential gate strictly (fails immediately on missing
/// required toolchain, compile error, runtime error, or any trace mismatch — zero silent skips).
pub fn run_differential_suite(
    suite: DiffSuite,
    toolchains: &Toolchains,
) -> Result<DiffReport, DiffError> {
    let mut outcomes = Vec::new();

    if matches!(
        suite,
        DiffSuite::CppHost | DiffSuite::PrGate | DiffSuite::All
    ) {
        let cpp_scenarios = build_cpp_scenarios()?;
        for sc in &cpp_scenarios {
            let trace = run_cpp_host_scenario(sc, toolchains)?;
            outcomes.push(ScenarioOutcome {
                suite: "cpp-host",
                scenario: sc.name,
                detail: format!("{} trace lines matched dendri-vm bit-for-bit", trace.len()),
            });
        }
    }

    if matches!(
        suite,
        DiffSuite::RustySt | DiffSuite::PrGate | DiffSuite::All
    ) {
        let st_scenarios = build_st_rusty_scenarios()?;
        for sc in &st_scenarios {
            let trace = run_st_rusty_scenario(sc, toolchains)?;
            outcomes.push(ScenarioOutcome {
                suite: "rusty-st",
                scenario: sc.name,
                detail: format!(
                    "{} scan traces matched dendri-vm bit-for-bit (LLVM native)",
                    trace.len()
                ),
            });
        }
    }

    if matches!(suite, DiffSuite::ArduinoCli | DiffSuite::All) {
        let cpp_scenarios = build_cpp_scenarios()?;
        for sc in &cpp_scenarios {
            let summary = verify_cpp_scenario_arduino_cli(sc, toolchains)?;
            outcomes.push(ScenarioOutcome {
                suite: "arduino-cli",
                scenario: sc.name,
                detail: summary,
            });
        }
    }

    if matches!(suite, DiffSuite::AvrSimavr | DiffSuite::All) {
        let avr_results = run_avr_simavr_oracle(toolchains)?;
        for res in avr_results {
            outcomes.push(ScenarioOutcome {
                suite: "avr-simavr",
                scenario: res.name,
                detail: res.detail,
            });
        }
    }

    Ok(DiffReport { outcomes })
}
