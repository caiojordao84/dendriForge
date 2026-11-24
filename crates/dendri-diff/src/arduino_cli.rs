//! Official Arduino Uno Core (`arduino-cli compile --fqbn arduino:avr:uno`) Gate (`ADR-004`).
//!
//! Compiles every emitted Wave 1 Arduino C++ scenario (including derived Ladder→C++ and the full
//! `C3` hostile comment corpus) against the genuine `arduino:avr:uno` core toolchain.

use crate::scenarios::CppDiffScenario;
use crate::toolchain::{DiffError, Toolchains};
use std::fs;
use std::process::Command;

/// Compiles a single emitted [`CppDiffScenario`] against the official `arduino:avr:uno` core.
pub fn verify_cpp_scenario_arduino_cli(
    scenario: &CppDiffScenario,
    toolchains: &Toolchains,
) -> Result<String, DiffError> {
    let arduino_cli = toolchains.require_arduino_cli()?;
    let dir = tempfile::tempdir().map_err(|source| DiffError::Io {
        context: format!("create arduino-cli tempdir for {}", scenario.name),
        source,
    })?;

    let sketch_dir = dir.path().join(scenario.name);
    fs::create_dir_all(&sketch_dir).map_err(|source| DiffError::Io {
        context: sketch_dir.display().to_string(),
        source,
    })?;
    let ino_path = sketch_dir.join(format!("{}.ino", scenario.name));
    fs::write(&ino_path, &scenario.emitted_cpp).map_err(|source| DiffError::Io {
        context: ino_path.display().to_string(),
        source,
    })?;

    let build_dir = dir.path().join("build");
    let out = Command::new(arduino_cli)
        .arg("compile")
        .arg("--fqbn")
        .arg("arduino:avr:uno")
        .arg("--build-path")
        .arg(&build_dir)
        .arg(&sketch_dir)
        .output()
        .map_err(|source| DiffError::Io {
            context: format!("spawn {}", arduino_cli.display()),
            source,
        })?;

    if !out.status.success() {
        return Err(DiffError::CompileFailed {
            tool: "arduino-cli (arduino:avr:uno)",
            scenario: scenario.name.to_owned(),
            stderr: format!(
                "stdout:\n{}\nstderr:\n{}\n--- emitted C++ ---\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
                scenario.emitted_cpp
            ),
        });
    }

    let summary = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("compiled OK")
        .trim()
        .to_owned();
    Ok(summary)
}
