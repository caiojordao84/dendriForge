//! Deterministic Host C++ Arduino API Shim & Differential Runner (`g++`, ADR-004).
//!
//! Compiles emitted Wave 1 Arduino C++ sketches with `g++ -std=c++17 -O2 -Wall -Wextra -Werror`
//! against a deterministic host `Arduino.h` (virtual microsecond clock, pin-event log,
//! 10-bit binned ADC, 8-bit PWM, UTF-8 `Serial`), executes them, and verifies bit-identical
//! pin, PWM, global-variable, and serial traces against `dendri-vm`.

use crate::scenarios::{CppDiffScenario, run_cpp_scenario_on_vm};
use crate::toolchain::{DiffError, Toolchains};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::process::Command;

/// Deterministic host `Arduino.h` shim providing exact virtual clock, GPIO, ADC, PWM, and Serial.
const HOST_ARDUINO_HEADER: &str = r#"#pragma once
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cmath>
#include <string>
#include <vector>

static constexpr uint8_t LOW = 0;
static constexpr uint8_t HIGH = 1;
static constexpr uint8_t INPUT = 0;
static constexpr uint8_t OUTPUT = 1;
static constexpr uint8_t INPUT_PULLUP = 2;
static constexpr uint8_t INPUT_PULLDOWN = 3;

static constexpr uint8_t RISING = 3;
static constexpr uint8_t FALLING = 2;
static constexpr uint8_t CHANGE = 1;

static constexpr uint8_t A0 = 14;
static constexpr uint8_t A1 = 15;
static constexpr uint8_t A2 = 16;
static constexpr uint8_t A3 = 17;
static constexpr uint8_t A4 = 18;
static constexpr uint8_t A5 = 19;

namespace dendri_host {
inline uint64_t virtual_us = 0;
inline uint8_t pin_mode[32] = {0};
inline uint8_t digital_in[32] = {0};
inline uint8_t digital_out[32] = {0};
inline bool digital_in_driven[32] = {false};
inline int32_t analog_in_code[32] = {0};
inline uint8_t pwm_out[32] = {0};
inline std::string serial_buffer;

inline void set_digital_input(uint8_t pin, bool high) {
    if (pin < 32) {
        digital_in[pin] = high ? 1 : 0;
        digital_in_driven[pin] = true;
    }
}

inline void set_analog_input_ppb(uint8_t pin, int32_t analog_ppb) {
    if (pin < 32) {
        // Exact Arduino Uno 10-bit binned codec (`T-NUM-2`/`T-NUM-3`): floor(a * 1024 / 10^9), clamped [0, 1023]
        int64_t clamped = analog_ppb < 0 ? 0 : static_cast<int64_t>(analog_ppb);
        int64_t code = (clamped * 1024LL) / 1000000000LL;
        if (code > 1023) code = 1023;
        analog_in_code[pin] = static_cast<int32_t>(code);
    }
}
} // namespace dendri_host

inline void pinMode(uint8_t pin, uint8_t mode) {
    if (pin < 32) dendri_host::pin_mode[pin] = mode;
}

inline int digitalRead(uint8_t pin) {
    if (pin >= 32) return LOW;
    if (dendri_host::digital_in_driven[pin]) {
        return dendri_host::digital_in[pin] ? HIGH : LOW;
    }
    return dendri_host::digital_out[pin] ? HIGH : LOW;
}

inline void digitalWrite(uint8_t pin, int val) {
    if (pin < 32) {
        dendri_host::digital_out[pin] = val ? 1 : 0;
    }
}

inline int analogRead(uint8_t pin) {
    if (pin >= 32) return 0;
    return static_cast<int>(dendri_host::analog_in_code[pin]);
}

inline void analogWrite(uint8_t pin, int val) {
    if (pin < 32) {
        int clamped = val < 0 ? 0 : (val > 255 ? 255 : val);
        dendri_host::pwm_out[pin] = static_cast<uint8_t>(clamped);
    }
}

inline void delay(uint32_t ms) {
    dendri_host::virtual_us += static_cast<uint64_t>(ms) * 1000ULL;
}

inline void delayMicroseconds(uint32_t us) {
    dendri_host::virtual_us += static_cast<uint64_t>(us);
}

inline uint32_t millis() {
    return static_cast<uint32_t>(dendri_host::virtual_us / 1000ULL);
}

inline uint32_t micros() {
    return static_cast<uint32_t>(dendri_host::virtual_us);
}

inline uint8_t digitalPinToInterrupt(uint8_t pin) { return pin; }
inline void attachInterrupt(uint8_t, void (*)(), int) {}

struct DendriSerial {
    void begin(uint32_t) {}
    void print(const char* s) { if (s) dendri_host::serial_buffer += s; }
    void print(int32_t v) { dendri_host::serial_buffer += std::to_string(v); }
    void println(const char* s) {
        if (s) dendri_host::serial_buffer += s;
        dendri_host::serial_buffer += "\n";
    }
    void println(int32_t v) {
        dendri_host::serial_buffer += std::to_string(v);
        dendri_host::serial_buffer += "\n";
    }
};

inline DendriSerial Serial;
"#;

/// Maps canonical Uno pin names (`"D2"`, `"D13"`, `"A0"`) to Arduino numeric pin IDs.
fn pin_to_arduino_num(pin: &str) -> Result<u8, DiffError> {
    if let Some(rest) = pin.strip_prefix('D')
        && let Ok(n) = rest.parse::<u8>()
    {
        return Ok(n);
    }
    if let Some(rest) = pin.strip_prefix('A')
        && let Ok(n) = rest.parse::<u8>()
    {
        return Ok(14u8.saturating_add(n));
    }
    Err(DiffError::FrontendFailed {
        scenario: "cpp_host".to_owned(),
        details: format!("unsupported Arduino pin `{pin}`"),
    })
}

/// Runs a single [`CppDiffScenario`] against host `g++` + Arduino shim and compares with `dendri-vm`.
pub fn run_cpp_host_scenario(
    scenario: &CppDiffScenario,
    toolchains: &Toolchains,
) -> Result<Vec<String>, DiffError> {
    let gxx = toolchains.require_gxx()?;
    let vm_trace = run_cpp_scenario_on_vm(scenario)?;

    let dir = tempfile::tempdir().map_err(|source| DiffError::Io {
        context: format!("create tempdir for {}", scenario.name),
        source,
    })?;

    let header_path = dir.path().join("Arduino.h");
    fs::write(&header_path, HOST_ARDUINO_HEADER).map_err(|source| DiffError::Io {
        context: header_path.display().to_string(),
        source,
    })?;

    let sketch_path = dir.path().join("sketch_emitted.inc.cpp");
    fs::write(&sketch_path, &scenario.emitted_cpp).map_err(|source| DiffError::Io {
        context: sketch_path.display().to_string(),
        source,
    })?;

    let driver_src = build_cpp_driver_source(scenario)?;
    let driver_path = dir.path().join("driver.cpp");
    fs::write(&driver_path, &driver_src).map_err(|source| DiffError::Io {
        context: driver_path.display().to_string(),
        source,
    })?;

    let bin_path = dir.path().join("runner");
    compile_cpp_host(gxx, dir.path(), &driver_path, &bin_path, scenario.name)?;

    let output = Command::new(&bin_path)
        .output()
        .map_err(|source| DiffError::Io {
            context: format!("execute {}", bin_path.display()),
            source,
        })?;
    if !output.status.success() {
        return Err(DiffError::ExecutionFailed {
            tool: "g++ host runner",
            scenario: scenario.name.to_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let host_trace: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    if host_trace != vm_trace {
        return Err(DiffError::TraceMismatch {
            scenario: scenario.name.to_owned(),
            oracle: "g++ host Arduino shim",
            expected: vm_trace.join(" | "),
            actual: host_trace.join(" | "),
        });
    }

    Ok(host_trace)
}

fn compile_cpp_host(
    gxx: &Path,
    include_dir: &Path,
    driver_path: &Path,
    bin_path: &Path,
    scenario: &str,
) -> Result<(), DiffError> {
    let out = Command::new(gxx)
        .arg("-std=c++17")
        .arg("-O2")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-Werror")
        .arg("-I")
        .arg(include_dir)
        .arg(driver_path)
        .arg("-o")
        .arg(bin_path)
        .output()
        .map_err(|source| DiffError::Io {
            context: format!("spawn {}", gxx.display()),
            source,
        })?;

    if !out.status.success() {
        return Err(DiffError::CompileFailed {
            tool: "g++",
            scenario: scenario.to_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(())
}

fn build_cpp_driver_source(scenario: &CppDiffScenario) -> Result<String, DiffError> {
    let mut s = String::new();
    s.push_str("#include \"Arduino.h\"\n");
    s.push_str("#include <iostream>\n");
    s.push_str("#include <iomanip>\n\n");
    s.push_str("template <typename T>\n");
    s.push_str("[[maybe_unused]] static std::string dendri_fmt(const T& v) { return std::to_string(v); }\n");
    s.push_str(
        "[[maybe_unused]] static std::string dendri_fmt(bool v) { return v ? \"1\" : \"0\"; }\n\n",
    );
    s.push_str("#include \"sketch_emitted.inc.cpp\"\n\n");
    s.push_str("[[maybe_unused]] static std::string rust_debug_quote(const std::string& raw) {\n");
    s.push_str("    std::string out = \"\\\"\";\n");
    s.push_str("    for (unsigned char c : raw) {\n");
    s.push_str("        if (c == '\\n') out += \"\\\\n\";\n");
    s.push_str("        else if (c == '\\r') out += \"\\\\r\";\n");
    s.push_str("        else if (c == '\"') out += \"\\\\\\\"\";\n");
    s.push_str("        else if (c == '\\\\') out += \"\\\\\\\\\";\n");
    s.push_str("        else out.push_back(static_cast<char>(c));\n");
    s.push_str("    }\n");
    s.push_str("    out += \"\\\"\";\n");
    s.push_str("    return out;\n");
    s.push_str("}\n\n");
    s.push_str("int main() {\n");
    s.push_str("    setup();\n");

    for (step_idx, stim) in scenario.steps.iter().enumerate() {
        for &(pin, high) in &stim.digital_in {
            let pin_num = pin_to_arduino_num(pin)?;
            let val = if high { "true" } else { "false" };
            let _ = writeln!(s, "    dendri_host::set_digital_input({pin_num}, {val});");
        }
        for &(pin, ppb) in &stim.analog_in {
            let pin_num = pin_to_arduino_num(pin)?;
            let _ = writeln!(
                s,
                "    dendri_host::set_analog_input_ppb({pin_num}, {ppb});"
            );
        }
        s.push_str("    loop();\n");
        let _ = write!(s, "    std::cout << \"step={step_idx}\"");
        for &pin in &scenario.sample_digital_pins {
            let pin_num = pin_to_arduino_num(pin)?;
            let _ = write!(
                s,
                " << \" {pin}=\" << static_cast<int>(dendri_host::digital_out[{pin_num}])"
            );
        }
        for &pin in &scenario.sample_pwm_pins {
            let pin_num = pin_to_arduino_num(pin)?;
            let _ = write!(
                s,
                " << \" pwm_{pin}=\" << static_cast<int>(dendri_host::pwm_out[{pin_num}])"
            );
        }
        for &g in &scenario.sample_globals {
            let _ = write!(s, " << \" {g}=\" << dendri_fmt({g})");
        }
        s.push_str(" << \"\\n\";\n");
    }

    if scenario.check_serial {
        s.push_str("    std::cout << \"serial=\" << rust_debug_quote(dendri_host::serial_buffer) << \"\\n\";\n");
    }
    s.push_str("    return 0;\n}\n");
    Ok(s)
}
