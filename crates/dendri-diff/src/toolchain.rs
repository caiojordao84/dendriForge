//! External toolchain discovery and error types for Phase 6b (`ADR-004`, `DEF-003`).
//!
//! Zero `env!("CARGO_MANIFEST_DIR")` in production code (Invariant 6): binaries resolve strictly
//! from explicit environment overrides, `PATH`, or standard user-local paths (`$HOME/.local/bin`).

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;

/// Discovered external toolchain binaries for Phase 6b differential verification.
#[derive(Debug, Clone, Default)]
pub struct Toolchains {
    pub gxx: Option<PathBuf>,
    pub cc: Option<PathBuf>,
    pub plc: Option<PathBuf>,
    pub avr_gcc: Option<PathBuf>,
    pub simavr: Option<PathBuf>,
    pub simavr_lib_dir: Option<PathBuf>,
    pub arduino_cli: Option<PathBuf>,
}

/// Errors produced by Phase 6b differential verification harnesses.
#[derive(Debug, Error)]
pub enum DiffError {
    #[error("required external toolchain `{tool}` not found ({hint})")]
    ToolchainMissing { tool: &'static str, hint: String },

    #[error("frontend parse/emit failed in scenario `{scenario}`: {details}")]
    FrontendFailed { scenario: String, details: String },

    #[error("external compiler `{tool}` failed for scenario `{scenario}`:\n{stderr}")]
    CompileFailed {
        tool: &'static str,
        scenario: String,
        stderr: String,
    },

    #[error("external runner `{tool}` failed for scenario `{scenario}`:\n{stderr}")]
    ExecutionFailed {
        tool: &'static str,
        scenario: String,
        stderr: String,
    },

    #[error("dendri-vm execution failed in scenario `{scenario}`: {details}")]
    VmFailed { scenario: String, details: String },

    #[error(
        "differential trace mismatch in scenario `{scenario}`:\n  expected (dendri-vm): {expected}\n  actual ({oracle}): {actual}"
    )]
    TraceMismatch {
        scenario: String,
        oracle: &'static str,
        expected: String,
        actual: String,
    },

    #[error("I/O error in `{context}`: {source}")]
    Io {
        context: String,
        source: std::io::Error,
    },
}

impl Toolchains {
    /// Probes explicit env overrides, `PATH`, and `$HOME/.local/bin` for all Phase 6b toolchains.
    #[must_use]
    pub fn discover() -> Self {
        let simavr = find_binary("DENDRI_SIMAVR_BIN", &["simavr", "run_avr"]);
        let simavr_lib_dir = discover_simavr_lib_dir(simavr.as_deref());
        Self {
            gxx: find_binary("DENDRI_GXX_BIN", &["g++", "clang++"]),
            cc: find_binary("DENDRI_CC_BIN", &["cc", "gcc", "clang"]),
            plc: find_binary("DENDRI_PLC_BIN", &["plc"]),
            avr_gcc: find_binary("DENDRI_AVR_GCC_BIN", &["avr-gcc"]),
            simavr,
            simavr_lib_dir,
            arduino_cli: find_binary("DENDRI_ARDUINO_CLI_BIN", &["arduino-cli"]),
        }
    }

    /// Requires host C++ compiler (`g++` or `clang++`).
    pub fn require_gxx(&self) -> Result<&Path, DiffError> {
        self.gxx
            .as_deref()
            .ok_or_else(|| DiffError::ToolchainMissing {
                tool: "g++",
                hint: "install g++ or set DENDRI_GXX_BIN".to_owned(),
            })
    }

    /// Requires host C linker driver (`cc`, `gcc`, or `clang`) for linking RuSTy `.o` files.
    pub fn require_cc(&self) -> Result<&Path, DiffError> {
        self.cc
            .as_deref()
            .ok_or_else(|| DiffError::ToolchainMissing {
                tool: "cc",
                hint: "install gcc/clang or set DENDRI_CC_BIN".to_owned(),
            })
    }

    /// Requires RuSTy (`plc`) IEC 61131-3 LLVM compiler (`DEF-003`).
    pub fn require_plc(&self) -> Result<&Path, DiffError> {
        self.plc.as_deref().ok_or_else(|| DiffError::ToolchainMissing {
            tool: "plc (RuSTy)",
            hint: "install PLC-lang/rusty `plc` into PATH or ~/.local/bin/plc, or set DENDRI_PLC_BIN".to_owned(),
        })
    }

    /// Requires `avr-gcc` compiler (`DEF-003`).
    pub fn require_avr_gcc(&self) -> Result<&Path, DiffError> {
        self.avr_gcc
            .as_deref()
            .ok_or_else(|| DiffError::ToolchainMissing {
                tool: "avr-gcc",
                hint: "install avr-gcc + avr-libc or set DENDRI_AVR_GCC_BIN".to_owned(),
            })
    }

    /// Requires `simavr` ATmega328P simulator (`DEF-003`).
    pub fn require_simavr(&self) -> Result<&Path, DiffError> {
        self.simavr
            .as_deref()
            .ok_or_else(|| DiffError::ToolchainMissing {
                tool: "simavr",
                hint: "install simavr into PATH or ~/.local/bin/simavr, or set DENDRI_SIMAVR_BIN"
                    .to_owned(),
            })
    }

    /// Requires `arduino-cli` with `arduino:avr` core (`DEF-003`).
    pub fn require_arduino_cli(&self) -> Result<&Path, DiffError> {
        self.arduino_cli
            .as_deref()
            .ok_or_else(|| DiffError::ToolchainMissing {
                tool: "arduino-cli",
                hint: "install arduino-cli + `arduino-cli core install arduino:avr`, or set DENDRI_ARDUINO_CLI_BIN".to_owned(),
            })
    }

    /// Probes version string for diagnostics/reporting.
    #[must_use]
    pub fn version_line(bin: &Path, flag: &str) -> String {
        Command::new(bin)
            .arg(flag)
            .output()
            .ok()
            .map(|o| {
                let s = String::from_utf8_lossy(&o.stdout);
                let first = s.lines().next().unwrap_or("").trim();
                if first.is_empty() {
                    String::from_utf8_lossy(&o.stderr)
                        .lines()
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_owned()
                } else {
                    first.to_owned()
                }
            })
            .unwrap_or_default()
    }
}

fn find_binary(env_var: &str, names: &[&str]) -> Option<PathBuf> {
    if let Ok(explicit) = env::var(env_var) {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            let p = PathBuf::from(trimmed);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    if let Some(path_os) = env::var_os("PATH") {
        for dir in env::split_paths(&path_os) {
            for &name in names {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    if let Some(home) = env::var_os("HOME") {
        let local_bin = PathBuf::from(home).join(".local").join("bin");
        for &name in names {
            let candidate = local_bin.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

fn discover_simavr_lib_dir(simavr_bin: Option<&Path>) -> Option<PathBuf> {
    if let Ok(explicit) = env::var("DENDRI_SIMAVR_LIB_DIR") {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            let p = PathBuf::from(trimmed);
            if p.is_dir() {
                return Some(p);
            }
        }
    }
    if let Some(bin) = simavr_bin
        && let Some(parent) = bin.parent()
        && let Some(prefix) = parent.parent()
    {
        let lib = prefix.join("lib");
        if lib.is_dir() {
            return Some(lib);
        }
    }
    if let Some(home) = env::var_os("HOME") {
        let local_lib = PathBuf::from(home).join(".local").join("lib");
        if local_lib.is_dir() {
            return Some(local_lib);
        }
    }
    None
}
