//! RuSTy (`plc`, pure-Rust IEC 61131-3 -> LLVM) Native Differential Runner (`ADR-004`, `DEF-003`).
//!
//! Compiles emitted Wave 1 IEC 61131-3 ST (including Ladder `PouBody::Network` and GRAFCET/SFC
//! `PouBody::Chart` projected deterministically to ST, plus the full `C3` hostile comment corpus)
//! directly to native machine code via RuSTy `plc -c` (LLVM, zero C transpilation step!), links
//! the resulting ELF object against a minimal deterministic I/O driver, executes it natively, and
//! verifies bit-identical traces against `dendri-vm`.

use crate::scenarios::{StDiffScenario, StVarKind, run_st_scenario_on_vm};
use crate::toolchain::{DiffError, Toolchains};
use std::fmt::Write as _;
use std::fs;
use std::process::Command;

/// Deterministic virtual-clock IEC 61131-3 standard FB runtime written in pure ST and compiled
/// directly by RuSTy `plc` into LLVM alongside emitted user ST programs.
const DENDRI_IEC_RUNTIME_ST: &str = r#"VAR_GLOBAL
    __DENDRI_SCAN_DT : TIME := T#10ms;
END_VAR

FUNCTION_BLOCK TON
VAR_INPUT
    IN : BOOL;
    PT : TIME;
END_VAR
VAR_OUTPUT
    Q : BOOL;
    ET : TIME;
END_VAR
    IF NOT IN THEN
        ET := T#0ms;
        Q := FALSE;
    ELSIF PT <= T#0ms THEN
        ET := T#0ms;
        Q := TRUE;
    ELSE
        ET := ET + __DENDRI_SCAN_DT;
        IF ET >= PT THEN
            ET := PT;
            Q := TRUE;
        ELSE
            Q := FALSE;
        END_IF;
    END_IF;
END_FUNCTION_BLOCK

FUNCTION_BLOCK TOF
VAR_INPUT
    IN : BOOL;
    PT : TIME;
END_VAR
VAR_OUTPUT
    Q : BOOL;
    ET : TIME;
END_VAR
VAR
    _seen_true : BOOL := FALSE;
END_VAR
    IF IN THEN
        _seen_true := TRUE;
        Q := TRUE;
        ET := T#0ms;
    ELSIF (NOT _seen_true) OR (PT <= T#0ms) THEN
        Q := FALSE;
        ET := T#0ms;
    ELSE
        ET := ET + __DENDRI_SCAN_DT;
        IF ET >= PT THEN
            ET := PT;
            Q := FALSE;
        ELSE
            Q := TRUE;
        END_IF;
    END_IF;
END_FUNCTION_BLOCK

FUNCTION_BLOCK TP
VAR_INPUT
    IN : BOOL;
    PT : TIME;
END_VAR
VAR_OUTPUT
    Q : BOOL;
    ET : TIME;
END_VAR
VAR
    _prev_in : BOOL := FALSE;
    _running : BOOL := FALSE;
END_VAR
    IF _running THEN
        ET := ET + __DENDRI_SCAN_DT;
        IF ET >= PT THEN
            ET := PT;
            _running := FALSE;
            Q := FALSE;
            IF NOT IN THEN
                ET := T#0ms;
            END_IF;
        END_IF;
    ELSIF IN AND NOT _prev_in AND (PT > T#0ms) THEN
        _running := TRUE;
        Q := TRUE;
        ET := __DENDRI_SCAN_DT;
        IF ET >= PT THEN
            ET := PT;
            _running := FALSE;
            Q := FALSE;
        END_IF;
    ELSIF NOT IN THEN
        Q := FALSE;
        ET := T#0ms;
    END_IF;
    _prev_in := IN;
END_FUNCTION_BLOCK

FUNCTION_BLOCK CTU
VAR_INPUT
    CU : BOOL;
    R : BOOL;
    PV : DINT;
END_VAR
VAR_OUTPUT
    Q : BOOL;
    CV : DINT;
END_VAR
VAR
    _prev_cu : BOOL := FALSE;
END_VAR
    IF R THEN
        CV := 0;
    ELSIF CU AND NOT _prev_cu THEN
        CV := CV + 1;
    END_IF;
    _prev_cu := CU;
    Q := CV >= PV;
END_FUNCTION_BLOCK

FUNCTION_BLOCK CTD
VAR_INPUT
    CD : BOOL;
    LD : BOOL;
    PV : DINT;
END_VAR
VAR_OUTPUT
    Q : BOOL;
    CV : DINT;
END_VAR
VAR
    _prev_cd : BOOL := FALSE;
END_VAR
    IF LD THEN
        CV := PV;
    ELSIF CD AND NOT _prev_cd THEN
        CV := CV - 1;
    END_IF;
    _prev_cd := CD;
    Q := CV <= 0;
END_FUNCTION_BLOCK

FUNCTION_BLOCK R_TRIG
VAR_INPUT
    CLK : BOOL;
END_VAR
VAR_OUTPUT
    Q : BOOL;
END_VAR
VAR
    _prev : BOOL := FALSE;
END_VAR
    Q := CLK AND NOT _prev;
    _prev := CLK;
END_FUNCTION_BLOCK

FUNCTION_BLOCK F_TRIG
VAR_INPUT
    CLK : BOOL;
END_VAR
VAR_OUTPUT
    Q : BOOL;
END_VAR
VAR
    _prev : BOOL := FALSE;
END_VAR
    Q := (NOT CLK) AND _prev;
    _prev := CLK;
END_FUNCTION_BLOCK

FUNCTION_BLOCK SR
VAR_INPUT
    S1 : BOOL;
    R : BOOL;
END_VAR
VAR_OUTPUT
    Q1 : BOOL;
END_VAR
    Q1 := S1 OR (Q1 AND NOT R);
END_FUNCTION_BLOCK

FUNCTION_BLOCK RS
VAR_INPUT
    S : BOOL;
    R1 : BOOL;
END_VAR
VAR_OUTPUT
    Q1 : BOOL;
END_VAR
    Q1 := (NOT R1) AND (S OR Q1);
END_FUNCTION_BLOCK
"#;

/// Runs a single [`StDiffScenario`] natively via RuSTy `plc` (LLVM) and compares with `dendri-vm`.
pub fn run_st_rusty_scenario(
    scenario: &StDiffScenario,
    toolchains: &Toolchains,
) -> Result<Vec<String>, DiffError> {
    let plc = toolchains.require_plc()?;
    let cc = toolchains.require_cc()?;
    let vm_trace = run_st_scenario_on_vm(scenario)?;

    let dir = tempfile::tempdir().map_err(|source| DiffError::Io {
        context: format!("create tempdir for {}", scenario.name),
        source,
    })?;

    let st_path = dir.path().join("program.st");
    fs::write(&st_path, &scenario.emitted_st).map_err(|source| DiffError::Io {
        context: st_path.display().to_string(),
        source,
    })?;

    let runtime_path = dir.path().join("dendri_iec_runtime.st");
    if scenario.uses_iec_fbs {
        fs::write(&runtime_path, DENDRI_IEC_RUNTIME_ST).map_err(|source| DiffError::Io {
            context: runtime_path.display().to_string(),
            source,
        })?;
    }

    let obj_path = dir.path().join("program.o");
    let mut plc_cmd = Command::new(plc);
    plc_cmd.arg(&st_path);
    if scenario.uses_iec_fbs {
        plc_cmd.arg(&runtime_path);
    }
    plc_cmd.arg("-c").arg("-o").arg(&obj_path);
    let plc_out = plc_cmd.output().map_err(|source| DiffError::Io {
        context: format!("spawn {}", plc.display()),
        source,
    })?;

    if !plc_out.status.success() {
        let stderr = format!(
            "stdout:\n{}\nstderr:\n{}\n--- emitted ST ---\n{}",
            String::from_utf8_lossy(&plc_out.stdout),
            String::from_utf8_lossy(&plc_out.stderr),
            scenario.emitted_st
        );
        return Err(DiffError::CompileFailed {
            tool: "plc (RuSTy)",
            scenario: scenario.name.to_owned(),
            stderr,
        });
    }

    let driver_src = build_rusty_driver_c(scenario);
    let driver_path = dir.path().join("driver.c");
    fs::write(&driver_path, &driver_src).map_err(|source| DiffError::Io {
        context: driver_path.display().to_string(),
        source,
    })?;

    let bin_path = dir.path().join("rusty_runner");
    let link_out = Command::new(cc)
        .arg("-O2")
        .arg(&driver_path)
        .arg(&obj_path)
        .arg("-lm")
        .arg("-o")
        .arg(&bin_path)
        .output()
        .map_err(|source| DiffError::Io {
            context: format!("spawn {} to link RuSTy object", cc.display()),
            source,
        })?;
    if !link_out.status.success() {
        return Err(DiffError::CompileFailed {
            tool: "cc (linking RuSTy LLVM object)",
            scenario: scenario.name.to_owned(),
            stderr: String::from_utf8_lossy(&link_out.stderr).into_owned(),
        });
    }

    let run_out = Command::new(&bin_path)
        .output()
        .map_err(|source| DiffError::Io {
            context: format!("execute {}", bin_path.display()),
            source,
        })?;
    if !run_out.status.success() {
        return Err(DiffError::ExecutionFailed {
            tool: "RuSTy native binary",
            scenario: scenario.name.to_owned(),
            stderr: String::from_utf8_lossy(&run_out.stderr).into_owned(),
        });
    }

    let stdout = String::from_utf8_lossy(&run_out.stdout);
    let rusty_trace: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    if rusty_trace != vm_trace {
        return Err(DiffError::TraceMismatch {
            scenario: scenario.name.to_owned(),
            oracle: "RuSTy (plc LLVM native)",
            expected: vm_trace.join(" | "),
            actual: rusty_trace.join(" | "),
        });
    }

    Ok(rusty_trace)
}

fn build_rusty_driver_c(scenario: &StDiffScenario) -> String {
    let mut s = String::new();
    s.push_str("#include <stdint.h>\n");
    s.push_str("#include <stdio.h>\n\n");
    s.push_str("extern char OB_MAIN_instance[];\n");
    s.push_str("extern void OB_MAIN(void *instance);\n");
    if scenario.uses_iec_fbs {
        s.push_str("extern int64_t __DENDRI_SCAN_DT;\n");
    }

    // Emit extern declarations for all hardware pointer globals and plain globals
    for b in &scenario.io_bindings {
        match b.kind {
            StVarKind::IoBool => {
                let _ = writeln!(s, "extern uint8_t *{};", b.st_name);
                let _ = writeln!(s, "static uint8_t slot_{} = 0;", b.st_name);
            }
            StVarKind::IoInt16 => {
                let _ = writeln!(s, "extern int16_t *{};", b.st_name);
                let _ = writeln!(s, "static int16_t slot_{} = 0;", b.st_name);
            }
            StVarKind::GlobalBool | StVarKind::GlobalInt16 | StVarKind::GlobalInt32 => {}
        }
    }
    for out in &scenario.outputs {
        match out.kind {
            StVarKind::GlobalBool => {
                let _ = writeln!(s, "extern uint8_t {};", out.st_name);
            }
            StVarKind::GlobalInt16 => {
                let _ = writeln!(s, "extern int16_t {};", out.st_name);
            }
            StVarKind::GlobalInt32 => {
                let _ = writeln!(s, "extern int32_t {};", out.st_name);
            }
            StVarKind::IoBool | StVarKind::IoInt16 => {}
        }
    }

    s.push_str("\nint main(void) {\n");
    // Bind each hardware AT %IX/%QX/%IW pointer to its dedicated isolated PII/PIQ slot
    for b in &scenario.io_bindings {
        if matches!(b.kind, StVarKind::IoBool | StVarKind::IoInt16) {
            let _ = writeln!(s, "    {} = &slot_{};", b.st_name, b.st_name);
        }
    }
    if scenario.uses_iec_fbs {
        let dt_ns = i64::try_from(scenario.scan_period_us.saturating_mul(1_000)).unwrap_or(0);
        let _ = writeln!(s, "    __DENDRI_SCAN_DT = {dt_ns}LL;");
    }

    for (scan_idx, stim) in scenario.scans.iter().enumerate() {
        for &(name, high) in &stim.bool_inputs {
            let v = u8::from(high);
            let _ = writeln!(s, "    slot_{name} = {v};");
        }
        for &(name, val) in &stim.int16_inputs {
            let _ = writeln!(s, "    slot_{name} = {val};");
        }
        s.push_str("    OB_MAIN(OB_MAIN_instance);\n");
        let _ = writeln!(s, "    printf(\"scan={scan_idx}\");");
        for out in &scenario.outputs {
            match out.kind {
                StVarKind::IoBool => {
                    let _ = writeln!(
                        s,
                        "    printf(\" {}=%u\", (unsigned)(slot_{} != 0 ? 1 : 0));",
                        out.st_name, out.st_name
                    );
                }
                StVarKind::IoInt16 => {
                    let _ = writeln!(
                        s,
                        "    printf(\" {}=%d\", (int)slot_{});",
                        out.st_name, out.st_name
                    );
                }
                StVarKind::GlobalBool => {
                    let _ = writeln!(
                        s,
                        "    printf(\" {}=%u\", (unsigned)({} != 0 ? 1 : 0));",
                        out.st_name, out.st_name
                    );
                }
                StVarKind::GlobalInt16 => {
                    let _ = writeln!(
                        s,
                        "    printf(\" {}=%d\", (int){});",
                        out.st_name, out.st_name
                    );
                }
                StVarKind::GlobalInt32 => {
                    let _ = writeln!(
                        s,
                        "    printf(\" {}=%d\", (int){});",
                        out.st_name, out.st_name
                    );
                }
            }
        }
        s.push_str("    printf(\"\\n\");\n");
    }

    s.push_str("    return 0;\n}\n");
    s
}
