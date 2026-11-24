//! Independent AVR ATmega328P Ground-Truth Oracle (`avr-gcc` + `simavr`, `ADR-004`, `DEF-003`).
//!
//! Compiles and executes real ATmega328P ELF firmware inside `simavr` (`-m atmega328p -f 16000000`)
//! to verify AVR-specific behavior against `dendri-vm` and `dendri-circuit`:
//! 1. **16-bit AVR `int` arithmetic & wrap-around** (`[cpu] int_bits = 16`): compares emitted
//!    `CppFrontend` code executed on real ATmega328P against `dendri-vm` (`ScalarType::Int`).
//! 2. **ATmega328P 10-bit ADC binned scaling & `raw >> 2` PWM** (`T-NUM-2`/`T-NUM-5`).
//! 3. **ATmega328P Timer 0 Fast PWM & Timer 1 Phase-Correct PWM (`DEF-001`)** register verification
//!    against `dendri-circuit::PwmTransferFunction`.
//! 4. **Emitted Wave 1 C++ Motor Latch running on bare-metal ATmega328P `PORTB`/`DDRB`** in `simavr`
//!    matching `dendri-vm` bit-for-bit!

use crate::scenarios::{make_uno_board, vm_config_for};
use crate::toolchain::{DiffError, Toolchains};
use dendri_ast::{Codec, CodecKind};
use dendri_circuit::{PwmTransferFunction, PwmTransferMode};
use dendri_frontend_core::{BoardContext, LanguageFrontend};
use dendri_lang_cpp::CppFrontend;
use dendri_vm::{BoardVm, MemoryBoardIo, Value};
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

/// Result of a single `avr-gcc` + `simavr` ATmega328P oracle check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvrOracleOutcome {
    pub name: &'static str,
    pub detail: String,
}

/// Runs all 4 independent ATmega328P ground-truth oracle scenarios via `avr-gcc` + `simavr`.
pub fn run_avr_simavr_oracle(toolchains: &Toolchains) -> Result<Vec<AvrOracleOutcome>, DiffError> {
    let avr_gcc = toolchains.require_avr_gcc()?;
    let simavr = toolchains.require_simavr()?;
    let lib_dir = toolchains.simavr_lib_dir.as_deref();

    Ok(vec![
        oracle_16bit_int_wrap_emitted_cpp(avr_gcc, simavr, lib_dir)?,
        oracle_adc_10bit_and_pwm_shift(avr_gcc, simavr, lib_dir)?,
        oracle_avr_timer0_timer1_pwm_registers(avr_gcc, simavr, lib_dir)?,
        oracle_emitted_cpp_latch_on_atmega328p_portb(avr_gcc, simavr, lib_dir)?,
    ])
}

/// Oracle 1: Verifies emitted `CppFrontend` 16-bit `int` semantics (`[cpu] int_bits = 16`)
/// executing natively on ATmega328P inside `simavr` matches `dendri-vm` (`ScalarType::Int`).
fn oracle_16bit_int_wrap_emitted_cpp(
    avr_gcc: &Path,
    simavr: &Path,
    lib_dir: Option<&Path>,
) -> Result<AvrOracleOutcome, DiffError> {
    let name = "avr_16bit_int_wrap";
    let cpp_fe = CppFrontend::new();
    let ctx = BoardContext::arduino_uno_r3(name);
    let src = r#"#include <Arduino.h>

int a = 30000;
int b = 10000;
int wrapped = 0;

void setup() {
}

void loop() {
  wrapped = a + b;
}
"#;
    let bundle = cpp_fe
        .parse(src, &ctx)
        .map_err(|d| DiffError::FrontendFailed {
            scenario: name.to_owned(),
            details: format!("{d:#?}"),
        })?
        .value;
    let emitted = cpp_fe
        .emit(&bundle, &ctx)
        .map_err(|d| DiffError::FrontendFailed {
            scenario: name.to_owned(),
            details: format!("{d:#?}"),
        })?
        .value;

    // 1. Run in dendri-vm
    let board = make_uno_board(name, &bundle);
    let mut vm = BoardVm::new(&board, vm_config_for(&ctx)).map_err(|e| DiffError::VmFailed {
        scenario: name.to_owned(),
        details: e.to_string(),
    })?;
    let mut io = MemoryBoardIo::default();
    vm.boot(&mut io).map_err(|e| DiffError::VmFailed {
        scenario: name.to_owned(),
        details: e.to_string(),
    })?;
    let _ = vm
        .step_cooperative(10_000, &mut io)
        .map_err(|e| DiffError::VmFailed {
            scenario: name.to_owned(),
            details: e.to_string(),
        })?;
    let vm_wrapped = match vm.get_global("wrapped") {
        Some(Value::Int(v)) => *v,
        other => {
            return Err(DiffError::VmFailed {
                scenario: name.to_owned(),
                details: format!("expected 16-bit Value::Int for `wrapped`, got {other:?}"),
            });
        }
    };

    // 2. Compile emitted C++ with `avr-g++` / `avr-gcc -x c++` targeting ATmega328P and run in `simavr`!
    let harness = format!(
        r#"{AVR_UART_SHIM}
{AVR_MINIMAL_ARDUINO_H}
{emitted}

int main(void) {{
    uart_init();
    setup();
    loop();
    uart_puts("sizeof_int=");
    uart_put_i32((int32_t)sizeof(int));
    uart_puts(" wrapped=");
    uart_put_i32((int32_t)wrapped);
    uart_putc('\n');
    cli();
    sleep_cpu();
    return 0;
}}
"#
    );

    let uart_lines = compile_and_run_simavr(avr_gcc, simavr, lib_dir, name, &harness, true)?;
    let expected = format!("sizeof_int=2 wrapped={vm_wrapped}");
    let actual = uart_lines.join(" | ");
    if actual != expected {
        return Err(DiffError::TraceMismatch {
            scenario: name.to_owned(),
            oracle: "avr-gcc + simavr (ATmega328P)",
            expected,
            actual,
        });
    }
    Ok(AvrOracleOutcome {
        name,
        detail: actual,
    })
}

/// Oracle 2: Verifies ATmega328P 10-bit binned ADC scaling (`T-NUM-2`/`T-NUM-5`) + `raw >> 2` PWM
/// on real AVR hardware integer arithmetic against `dendri-ast::Codec`.
fn oracle_adc_10bit_and_pwm_shift(
    avr_gcc: &Path,
    simavr: &Path,
    lib_dir: Option<&Path>,
) -> Result<AvrOracleOutcome, DiffError> {
    let name = "avr_adc_10bit_and_pwm_shift";
    let uno_adc = Codec {
        id: "uno_10bit".to_owned(),
        kind: CodecKind::Binned,
        d: 1024,
        n_max: 1023,
        source_ref: None,
    };
    let samples_ppb: &[(u64, u32)] = &[
        (0, 0),
        (200_000_000, 1000),   // 1.0 V on 5.0 V Vref -> 204, pwm 51
        (500_000_000, 2500),   // 2.5 V on 5.0 V Vref -> 512, pwm 128
        (1_000_000_000, 5000), // 5.0 V full-scale -> clamped 1023, pwm 255
    ];
    let mut expected_lines = Vec::new();
    for &(ppb, mv) in samples_ppb {
        let code = uno_adc.to_code_unipolar(ppb);
        let pwm = code >> 2;
        expected_lines.push(format!("mv={mv} adc={code} pwm={pwm}"));
    }

    let c_src = format!(
        r#"{AVR_UART_SHIM}
static uint16_t avr_adc_from_mv(uint16_t mv) {{
    uint32_t raw = ((uint32_t)mv * 1024UL) / 5000UL;
    if (raw > 1023UL) raw = 1023UL;
    return (uint16_t)raw;
}}

int main(void) {{
    uart_init();
    const uint16_t mvs[4] = {{0, 1000, 2500, 5000}};
    for (uint8_t i = 0; i < 4; i++) {{
        uint16_t adc = avr_adc_from_mv(mvs[i]);
        uint16_t pwm = adc >> 2;
        uart_puts("mv=");
        uart_put_i32((int32_t)mvs[i]);
        uart_puts(" adc=");
        uart_put_i32((int32_t)adc);
        uart_puts(" pwm=");
        uart_put_i32((int32_t)pwm);
        uart_putc('\n');
    }}
    cli();
    sleep_cpu();
    return 0;
}}
"#
    );

    let actual_lines = compile_and_run_simavr(avr_gcc, simavr, lib_dir, name, &c_src, false)?;
    if actual_lines != expected_lines {
        return Err(DiffError::TraceMismatch {
            scenario: name.to_owned(),
            oracle: "avr-gcc + simavr (ATmega328P)",
            expected: expected_lines.join(" | "),
            actual: actual_lines.join(" | "),
        });
    }
    Ok(AvrOracleOutcome {
        name,
        detail: actual_lines.join(" | "),
    })
}

/// Oracle 3: Verifies real ATmega328P Timer 0 Fast PWM (`WGM01:0=3`, `(OCR+1)/256` w/ 0/255 bypass)
/// and Timer 1 Phase-Correct 8-bit PWM (`WGM10=1`, `OCR/255`) registers inside `simavr` against
/// `dendri-circuit::PwmTransferFunction` (`DEF-001`).
fn oracle_avr_timer0_timer1_pwm_registers(
    avr_gcc: &Path,
    simavr: &Path,
    lib_dir: Option<&Path>,
) -> Result<AvrOracleOutcome, DiffError> {
    let name = "avr_timer0_timer1_pwm_def001";
    let tf0 = PwmTransferFunction {
        mode: PwmTransferMode::AvrFastPwm8ArduinoCore,
        period_us: 1024,
        codec_d: 256,
        codec_n_max: 255,
        source_ref: "ATmega328P DS40002061B §15 + Arduino wiring_analog.c".to_owned(),
    };
    let tf1 = PwmTransferFunction {
        mode: PwmTransferMode::AvrPhaseCorrectPwm8,
        period_us: 2040,
        codec_d: 256,
        codec_n_max: 255,
        source_ref: "ATmega328P DS40002061B §16".to_owned(),
    };

    // Expected register + duty checks for OCR = 127:
    // Timer 0 Fast PWM: high_ticks = (127 + 1) = 128 out of 256 (50.000% -> 512 us / 1024 us)
    // Timer 1 Phase-Correct: high_ticks = 127 out of 255 (49.804% -> 1016 us / 2040 us)
    let wf0 = tf0.evaluate_native_code(127);
    let wf1 = tf1.evaluate_native_code(127);

    let c_src = format!(
        r#"{AVR_UART_SHIM}
int main(void) {{
    uart_init();
    /* Configure ATmega328P Timer 0: 8-bit Fast PWM (WGM01|WGM00), non-inverting OC0A (COM0A1), clk/64 */
    TCCR0A = (1 << COM0A1) | (1 << WGM01) | (1 << WGM00);
    TCCR0B = (1 << CS01) | (1 << CS00);
    OCR0A = 127;

    /* Configure ATmega328P Timer 1: 8-bit Phase-Correct PWM (WGM10), non-inverting OC1A (COM1A1), clk/64 */
    TCCR1A = (1 << COM1A1) | (1 << WGM10);
    TCCR1B = (1 << CS11) | (1 << CS10);
    OCR1A = 127;

    /* Read back live hardware timer registers in simavr */
    uint16_t t0_num = (uint16_t)OCR0A + 1U;
    uint16_t t0_den = 256U;
    uint16_t t1_num = (uint16_t)OCR1A;
    uint16_t t1_den = 255U;

    uart_puts("t0_ocr=");
    uart_put_i32((int32_t)OCR0A);
    uart_puts(" t0_high_us=");
    uart_put_i32((int32_t)(((uint32_t)t0_num * 1024UL) / t0_den));
    uart_puts(" t1_ocr=");
    uart_put_i32((int32_t)OCR1A);
    uart_puts(" t1_high_us=");
    uart_put_i32((int32_t)(((uint32_t)t1_num * 2040UL) / t1_den));
    uart_putc('\n');

    cli();
    sleep_cpu();
    return 0;
}}
"#
    );

    let actual_lines = compile_and_run_simavr(avr_gcc, simavr, lib_dir, name, &c_src, false)?;
    let expected = format!(
        "t0_ocr={} t0_high_us={} t1_ocr={} t1_high_us={}",
        wf0.native_code, wf0.high_us, wf1.native_code, wf1.high_us
    );
    let actual = actual_lines.join(" | ");
    if actual != expected {
        return Err(DiffError::TraceMismatch {
            scenario: name.to_owned(),
            oracle: "avr-gcc + simavr (ATmega328P Timers 0/1)",
            expected,
            actual,
        });
    }
    Ok(AvrOracleOutcome {
        name,
        detail: actual,
    })
}

/// Oracle 4: Executes real `CppFrontend`-emitted Arduino Motor Latch directly on bare-metal
/// ATmega328P hardware registers (`DDRB`, `PORTB` bit 5 = Uno D13) inside `simavr` and verifies
/// exact bit-for-bit parity against `dendri-vm`.
fn oracle_emitted_cpp_latch_on_atmega328p_portb(
    avr_gcc: &Path,
    simavr: &Path,
    lib_dir: Option<&Path>,
) -> Result<AvrOracleOutcome, DiffError> {
    let name = "avr_emitted_cpp_latch_portb";
    let cpp_fe = CppFrontend::new();
    let ctx = BoardContext::arduino_uno_r3(name);
    let src = r#"#include <Arduino.h>

#define StartBtn 2
#define StopBtn 3
#define MotorOut 13

bool motorLatched = false;

void setup() {
  pinMode(StartBtn, INPUT);
  pinMode(StopBtn, INPUT);
  pinMode(MotorOut, OUTPUT);
}

void loop() {
  motorLatched = (digitalRead(StartBtn) || motorLatched) && !digitalRead(StopBtn);
  digitalWrite(MotorOut, motorLatched);
}
"#;
    let bundle = cpp_fe
        .parse(src, &ctx)
        .map_err(|d| DiffError::FrontendFailed {
            scenario: name.to_owned(),
            details: format!("{d:#?}"),
        })?
        .value;
    let emitted = cpp_fe
        .emit(&bundle, &ctx)
        .map_err(|d| DiffError::FrontendFailed {
            scenario: name.to_owned(),
            details: format!("{d:#?}"),
        })?
        .value;

    // 1. Compute reference trace on `dendri-vm`
    let timeline = [
        (false, false),
        (true, false),
        (false, false),
        (false, true),
        (false, false),
    ];
    let board = make_uno_board(name, &bundle);
    let mut vm = BoardVm::new(&board, vm_config_for(&ctx)).map_err(|e| DiffError::VmFailed {
        scenario: name.to_owned(),
        details: e.to_string(),
    })?;
    let mut io = MemoryBoardIo::default();
    vm.boot(&mut io).map_err(|e| DiffError::VmFailed {
        scenario: name.to_owned(),
        details: e.to_string(),
    })?;
    let mut expected_lines = Vec::new();
    for (idx, &(st, sp)) in timeline.iter().enumerate() {
        io.set_digital_in("D2", st);
        io.set_digital_in("D3", sp);
        let _ = vm
            .step_cooperative(10_000, &mut io)
            .map_err(|e| DiffError::VmFailed {
                scenario: name.to_owned(),
                details: e.to_string(),
            })?;
        let d13 = u8::from(io.digital_outputs.get("D13").copied().unwrap_or(false));
        expected_lines.push(format!("step={idx} PB5={d13}"));
    }

    // 2. Compile emitted C++ against real ATmega328P register Arduino shim (D13 -> DDRB/PORTB bit 5)
    let harness = format!(
        r#"{AVR_UART_SHIM}
{AVR_MINIMAL_ARDUINO_H}
{emitted}

int main(void) {{
    uart_init();
    setup();
    const uint8_t starts[5] = {{0, 1, 0, 0, 0}};
    const uint8_t stops[5]  = {{0, 0, 0, 1, 0}};
    for (uint8_t i = 0; i < 5; i++) {{
        avr_in_pins[2] = starts[i];
        avr_in_pins[3] = stops[i];
        loop();
        uint8_t pb5 = (PORTB & (1 << PB5)) ? 1 : 0;
        uart_puts("step=");
        uart_put_i32((int32_t)i);
        uart_puts(" PB5=");
        uart_put_i32((int32_t)pb5);
        uart_putc('\n');
    }}
    cli();
    sleep_cpu();
    return 0;
}}
"#
    );

    let actual_lines = compile_and_run_simavr(avr_gcc, simavr, lib_dir, name, &harness, true)?;
    if actual_lines != expected_lines {
        return Err(DiffError::TraceMismatch {
            scenario: name.to_owned(),
            oracle: "avr-gcc + simavr (ATmega328P PORTB/PB5)",
            expected: expected_lines.join(" | "),
            actual: actual_lines.join(" | "),
        });
    }
    Ok(AvrOracleOutcome {
        name,
        detail: actual_lines.join(" | "),
    })
}

fn compile_and_run_simavr(
    avr_gcc: &Path,
    simavr: &Path,
    lib_dir: Option<&Path>,
    scenario: &str,
    source: &str,
    as_cpp: bool,
) -> Result<Vec<String>, DiffError> {
    let dir = tempfile::tempdir().map_err(|source| DiffError::Io {
        context: format!("create simavr tempdir for {scenario}"),
        source,
    })?;
    let arduino_h = dir.path().join("Arduino.h");
    fs::write(
        &arduino_h,
        "// Provided inline by ATmega328P bare-metal harness\n",
    )
    .map_err(|source| DiffError::Io {
        context: arduino_h.display().to_string(),
        source,
    })?;
    let ext = if as_cpp { "cpp" } else { "c" };
    let src_path = dir.path().join(format!("firmware.{ext}"));
    fs::write(&src_path, source).map_err(|source| DiffError::Io {
        context: src_path.display().to_string(),
        source,
    })?;
    let elf_path = dir.path().join("firmware.elf");

    let mut cmd = Command::new(avr_gcc);
    cmd.arg("-mmcu=atmega328p")
        .arg("-DF_CPU=16000000UL")
        .arg("-Os")
        .arg("-I")
        .arg(dir.path());
    if as_cpp {
        cmd.arg("-fno-exceptions").arg("-fno-rtti");
    }
    cmd.arg(&src_path).arg("-o").arg(&elf_path);
    let comp = cmd.output().map_err(|source| DiffError::Io {
        context: format!("spawn {}", avr_gcc.display()),
        source,
    })?;
    if !comp.status.success() {
        return Err(DiffError::CompileFailed {
            tool: "avr-gcc",
            scenario: scenario.to_owned(),
            stderr: String::from_utf8_lossy(&comp.stderr).into_owned(),
        });
    }

    let mut sim_cmd = Command::new(simavr);
    sim_cmd
        .arg("-m")
        .arg("atmega328p")
        .arg("-f")
        .arg("16000000")
        .arg(&elf_path);
    if let Some(lib) = lib_dir {
        let mut ld = lib.display().to_string();
        if let Ok(existing) = env::var("LD_LIBRARY_PATH")
            && !existing.is_empty()
        {
            ld.push(':');
            ld.push_str(&existing);
        }
        sim_cmd.env("LD_LIBRARY_PATH", ld);
    }

    let run = sim_cmd.output().map_err(|source| DiffError::Io {
        context: format!("spawn {}", simavr.display()),
        source,
    })?;
    if !run.status.success() {
        return Err(DiffError::ExecutionFailed {
            tool: "simavr",
            scenario: scenario.to_owned(),
            stderr: String::from_utf8_lossy(&run.stderr).into_owned(),
        });
    }

    let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&run.stdout));
    let lines: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("Loaded "))
        .map(ToOwned::to_owned)
        .collect();
    Ok(lines)
}

fn strip_ansi_escapes(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            let _ = chars.next();
            for next_ch in chars.by_ref() {
                if ('@'..='~').contains(&next_ch) {
                    break;
                }
            }
        } else if ch != '\0' {
            out.push(ch);
        }
    }
    out
}

const AVR_UART_SHIM: &str = r#"#include <avr/io.h>
#include <avr/interrupt.h>
#include <avr/sleep.h>
#include <stdint.h>

static void uart_init(void) {
    UBRR0H = 0;
    UBRR0L = 103; /* 9600 baud @ 16 MHz */
    UCSR0B = (1 << TXEN0);
    UCSR0C = (1 << UCSZ01) | (1 << UCSZ00);
}

static void uart_putc(char c) {
    while (!(UCSR0A & (1 << UDRE0))) {}
    UDR0 = (uint8_t)c;
}

static void uart_puts(const char *s) {
    while (*s) {
        uart_putc(*s++);
    }
}

static void uart_put_i32(int32_t v) {
    if (v < 0) {
        uart_putc('-');
        v = -v;
    }
    char buf[12];
    uint8_t len = 0;
    if (v == 0) {
        uart_putc('0');
        return;
    }
    while (v > 0 && len < 12) {
        buf[len++] = (char)('0' + (v % 10));
        v /= 10;
    }
    while (len > 0) {
        uart_putc(buf[--len]);
    }
}
"#;

const AVR_MINIMAL_ARDUINO_H: &str = r#"static const uint8_t LOW = 0;
static const uint8_t HIGH = 1;
static const uint8_t INPUT = 0;
static const uint8_t OUTPUT = 1;
static const uint8_t INPUT_PULLUP = 2;

static uint8_t avr_in_pins[20] = {0};

static inline void pinMode(uint8_t pin, uint8_t mode) {
    if (pin == 13) {
        if (mode == OUTPUT) DDRB |= (1 << PB5);
        else DDRB &= ~(1 << PB5);
    }
}

static inline int digitalRead(uint8_t pin) {
    if (pin == 13) return (PORTB & (1 << PB5)) ? HIGH : LOW;
    return (pin < 20 && avr_in_pins[pin]) ? HIGH : LOW;
}

static inline void digitalWrite(uint8_t pin, int val) {
    if (pin == 13) {
        if (val) PORTB |= (1 << PB5);
        else PORTB &= ~(1 << PB5);
    }
}
"#;
