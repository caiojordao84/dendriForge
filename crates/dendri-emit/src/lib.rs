//! Emit ASL / project TOML from the shared AST (AST equality round-trip).

use dendri_ast::{Program, SimulationProject};

/// Serialize a program to pretty TOML. Round-trip target is AST equality, not bytes.
pub fn emit_program(program: &Program) -> Result<String, toml::ser::Error> {
    toml::to_string_pretty(program)
}

pub fn emit_project(project: &SimulationProject) -> Result<String, toml::ser::Error> {
    toml::to_string_pretty(project)
}

/// Parse-emit-parse and compare ASTs.
pub fn roundtrip_program(program: &Program) -> Result<Program, String> {
    let text = emit_program(program).map_err(|e| e.to_string())?;
    let again: Program = toml::from_str(&text).map_err(|e| e.to_string())?;
    if &again != program {
        return Err("round-trip AST mismatch".into());
    }
    Ok(again)
}

pub fn roundtrip_project(project: &SimulationProject) -> Result<SimulationProject, String> {
    let text = emit_project(project).map_err(|e| e.to_string())?;
    let again: SimulationProject = toml::from_str(&text).map_err(|e| e.to_string())?;
    if &again != project {
        return Err("project round-trip AST mismatch".into());
    }
    Ok(again)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_ast::{DataEntry, DataType, HardwareEntry, HwType, IoMode, Metadata, Scope, Stmt};
    use dendri_ast::expr::Expr;

    #[test]
    fn minimal_program_roundtrips() {
        let p = Program {
            metadata: Metadata {
                project_name: Some("rt".into()),
                editor: Some("dendriForge".into()),
                author: None,
                notes: None,
                expect_error: None,
            },
            hardware: vec![HardwareEntry {
                pin: 13,
                hw_type: HwType::Digital,
                io_mode: IoMode::Do,
                label: "led".into(),
                target_role: "digital-out".into(),
            }],
            data: vec![DataEntry {
                name: "x".into(),
                data_type: DataType::Bool,
                scope: Scope::Var,
                init: "FALSE".into(),
                unit: "-".into(),
            }],
            setup: vec![Stmt::Call {
                name: "SET_BOOL".into(),
                args: vec![
                    Expr::Uint { value: 13 },
                    Expr::Bool { value: true },
                ],
            }],
            loop_body: vec![],
            functions: vec![],
            ladder: vec![],
            grafcet: vec![],
        };
        roundtrip_program(&p).unwrap();
    }
}
