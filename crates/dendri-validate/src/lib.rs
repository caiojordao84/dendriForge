//! Semantic validation for ASL programs → unified [`ErrorCode`].

use std::collections::{HashMap, HashSet};

use dendri_ast::expr::{BinaryOp, Expr};
use dendri_ast::stmt::Stmt;
use dendri_ast::{ErrorCode, Program, Scope, SimulationProject};
use dendri_parse::{parse_expr_text, FixtureInject};
use dendri_stdlib;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct ValidationResult {
    pub diagnostics: Vec<Diagnostic>,
}

impl ValidationResult {
    pub fn ok(&self) -> bool {
        self.diagnostics.is_empty()
    }

    pub fn codes(&self) -> Vec<ErrorCode> {
        self.diagnostics.iter().map(|d| d.code).collect()
    }

    pub fn primary_code(&self) -> Option<ErrorCode> {
        self.diagnostics.first().map(|d| d.code)
    }
}

/// Validate a typed program (optionally with error-corpus inject).
pub fn validate_program(program: &Program, inject: &FixtureInject) -> ValidationResult {
    let mut out = ValidationResult::default();

    if program.metadata.project_name.is_none()
        || program
            .metadata
            .project_name
            .as_ref()
            .map(|s| s.is_empty())
            .unwrap_or(true)
    {
        out.diagnostics.push(Diagnostic {
            code: ErrorCode::ErrMetadataMissingField,
            message: "metadata.project_name is required".into(),
        });
    }

    // Block-count mismatches from legacy TOON header counts.
    if let Some(n) = inject.declared_setup_count {
        if n != program.setup.len() {
            out.diagnostics.push(Diagnostic {
                code: ErrorCode::ErrBlockCountMismatch,
                message: format!(
                    "declared setup count {n} != actual {}",
                    program.setup.len()
                ),
            });
        }
    }
    if let Some(n) = inject.declared_loop_count {
        if n != program.loop_body.len() {
            out.diagnostics.push(Diagnostic {
                code: ErrorCode::ErrBlockCountMismatch,
                message: format!(
                    "declared loop count {n} != actual {}",
                    program.loop_body.len()
                ),
            });
        }
    }
    if let Some(n) = inject.declared_hardware_count {
        if n != program.hardware.len() {
            out.diagnostics.push(Diagnostic {
                code: ErrorCode::ErrBlockCountMismatch,
                message: format!(
                    "declared hardware count {n} != actual {}",
                    program.hardware.len()
                ),
            });
        }
    }
    if let Some(n) = inject.declared_data_count {
        if n != program.data.len() {
            out.diagnostics.push(Diagnostic {
                code: ErrorCode::ErrBlockCountMismatch,
                message: format!(
                    "declared data count {n} != actual {}",
                    program.data.len()
                ),
            });
        }
    }

    let mut pins = HashSet::new();
    for hw in &program.hardware {
        if !pins.insert(hw.pin) {
            out.diagnostics.push(Diagnostic {
                code: ErrorCode::ErrHardwarePinConflict,
                message: format!("duplicate hardware pin {}", hw.pin),
            });
        }
    }

    let mut names: HashMap<String, NameInfo> = HashMap::new();
    for hw in &program.hardware {
        names.insert(
            hw.label.clone(),
            NameInfo {
                scope: Scope::Var,
                kind: NameKind::Hardware,
            },
        );
    }
    for d in &program.data {
        names.insert(
            d.name.clone(),
            NameInfo {
                scope: d.scope,
                kind: NameKind::Data,
            },
        );
    }

    let mut functions: HashMap<String, usize> = HashMap::new();
    for f in &program.functions {
        functions.insert(f.name.clone(), f.params.len());
        // Params are local to the function body — checked separately.
    }

    {
        let mut ctx = CheckCtx {
            names: &mut names,
            functions: &functions,
            out: &mut out,
            allow_implicit_temp: false,
        };
        for stmt in &program.setup {
            check_stmt(stmt, &mut ctx, true);
        }
        for stmt in &program.loop_body {
            check_stmt(stmt, &mut ctx, true);
        }
    }
    for f in &program.functions {
        let mut local = names.clone();
        for p in &f.params {
            local.insert(
                p.clone(),
                NameInfo {
                    scope: Scope::Temp,
                    kind: NameKind::Param,
                },
            );
        }
        let mut fctx = CheckCtx {
            names: &mut local,
            functions: &functions,
            out: &mut out,
            allow_implicit_temp: true,
        };
        for stmt in &f.body {
            check_stmt(stmt, &mut fctx, true);
        }
    }

    out
}

pub fn validate_project(project: &SimulationProject) -> ValidationResult {
    let mut out = validate_program(&project.asl, &FixtureInject::default());
    if project.metadata.project_name.is_none() {
        out.diagnostics.push(Diagnostic {
            code: ErrorCode::ErrMetadataMissingField,
            message: "project metadata.project_name is required".into(),
        });
    }
    if project.board.profile.is_empty() {
        out.diagnostics.push(Diagnostic {
            code: ErrorCode::ErrIrDeserialize,
            message: "board.profile is required".into(),
        });
    }
    for w in &project.wiring {
        if w.from.is_empty() || w.to.is_empty() {
            out.diagnostics.push(Diagnostic {
                code: ErrorCode::ErrIrDeserialize,
                message: "wiring endpoints must be non-empty".into(),
            });
        }
    }
    out
}

#[derive(Clone, Copy)]
enum NameKind {
    Hardware,
    Data,
    Param,
}

#[derive(Clone, Copy)]
struct NameInfo {
    scope: Scope,
    #[allow(dead_code)]
    kind: NameKind,
}

struct CheckCtx<'a> {
    names: &'a mut HashMap<String, NameInfo>,
    functions: &'a HashMap<String, usize>,
    out: &'a mut ValidationResult,
    /// Function bodies may introduce TEMP locals on first assign (G08 format_time).
    allow_implicit_temp: bool,
}

fn check_stmt(stmt: &Stmt, ctx: &mut CheckCtx<'_>, allow_write: bool) {
    match stmt {
        Stmt::Assign { target, value } => {
            match ctx.names.get(target).copied() {
                None if ctx.allow_implicit_temp => {
                    ctx.names.insert(
                        target.clone(),
                        NameInfo {
                            scope: Scope::Temp,
                            kind: NameKind::Data,
                        },
                    );
                }
                None => {
                    ctx.out.diagnostics.push(Diagnostic {
                        code: ErrorCode::ErrUndeclaredVariable,
                        message: format!("assign target `{target}` is undeclared"),
                    });
                }
                Some(info) if info.scope == Scope::Const => {
                    ctx.out.diagnostics.push(Diagnostic {
                        code: ErrorCode::ErrConstWriteViolation,
                        message: format!("cannot assign to CONST `{target}`"),
                    });
                }
                Some(_) if !allow_write => {}
                Some(_) => {}
            }
            check_expr(value, ctx);
        }
        Stmt::Call { name, args } => {
            check_call(name, args, ctx);
        }
        Stmt::If {
            condition,
            then,
            elif,
            else_body,
        } => {
            check_expr(condition, ctx);
            for s in then {
                check_stmt(s, ctx, allow_write);
            }
            for arm in elif {
                check_expr(&arm.condition, ctx);
                for s in &arm.then {
                    check_stmt(s, ctx, allow_write);
                }
            }
            if let Some(body) = else_body {
                for s in body {
                    check_stmt(s, ctx, allow_write);
                }
            }
        }
        Stmt::While { condition, body } => {
            check_expr(condition, ctx);
            for s in body {
                check_stmt(s, ctx, allow_write);
            }
        }
        Stmt::For {
            counter,
            to,
            body,
        } => {
            if let Some(c) = counter {
                if !ctx.names.contains_key(c) {
                    // Implicit counters (_i0) allowed without prior declaration.
                    if !c.starts_with('_') {
                        ctx.out.diagnostics.push(Diagnostic {
                            code: ErrorCode::ErrUndeclaredVariable,
                            message: format!("for counter `{c}` is undeclared"),
                        });
                    }
                }
            }
            check_expr(to, ctx);
            for s in body {
                check_stmt(s, ctx, allow_write);
            }
        }
        Stmt::Return { value } => {
            if let Some(v) = value {
                check_expr(v, ctx);
            }
        }
        Stmt::Exit | Stmt::Continue | Stmt::VendorExtension { .. } => {}
    }
}

fn check_expr(expr: &Expr, ctx: &mut CheckCtx<'_>) {
    match expr {
        Expr::Var { name } => {
            // ISR / function refs are passed as `var` exprs in CFG_ATTACH_INTERRUPT.
            if !ctx.names.contains_key(name) && !ctx.functions.contains_key(name) {
                ctx.out.diagnostics.push(Diagnostic {
                    code: ErrorCode::ErrUndeclaredVariable,
                    message: format!("variable `{name}` is undeclared"),
                });
            }
        }
        Expr::Call { name, args } => check_call(name, args, ctx),
        Expr::Unary { expr, .. } => check_expr(expr, ctx),
        Expr::Binop {
            op,
            left,
            right,
        } => {
            if matches!(op, BinaryOp::Sub) {
                // Heuristic: raw time subtraction warning when both sides look like DWORD times.
                // Full typing is Phase 1.x; keep soft for now.
                let _ = (left, right);
            }
            check_expr(left, ctx);
            check_expr(right, ctx);
        }
        Expr::ExprText { text } => match parse_expr_text(text) {
            Ok(e) => check_expr(&e, ctx),
            Err(err) => ctx.out.diagnostics.push(Diagnostic {
                code: ErrorCode::ErrUnknownIrKind,
                message: format!("expr_text parse failed: {err}"),
            }),
        },
        Expr::Lit { .. }
        | Expr::Bool { .. }
        | Expr::Uint { .. }
        | Expr::Int { .. }
        | Expr::Real { .. }
        | Expr::String { .. }
        | Expr::Time { .. }
        | Expr::Hex { .. } => {}
    }
}

fn check_call(name: &str, args: &[Expr], ctx: &mut CheckCtx<'_>) {
    for a in args {
        check_expr(a, ctx);
    }

    if let Some(&arity) = ctx.functions.get(name) {
        if args.len() != arity {
            ctx.out.diagnostics.push(Diagnostic {
                code: ErrorCode::ErrFunctionArityMismatch,
                message: format!(
                    "function `{name}` expects {arity} args, got {}",
                    args.len()
                ),
            });
        }
        return;
    }

    if let Some(op) = dendri_stdlib::lookup(name) {
        // IEC FB timers accept 2 (IN, PT) or 3 (instance, IN, PT) in Phase 1 sim.
        if matches!(op.canonical.as_str(), "TON" | "TOF" | "TP") {
            if args.len() != 2 && args.len() != 3 {
                ctx.out.diagnostics.push(Diagnostic {
                    code: ErrorCode::ErrFunctionArityMismatch,
                    message: format!(
                        "stdlib `{}` expects 2 or 3 args, got {}",
                        op.canonical,
                        args.len()
                    ),
                });
            }
            return;
        }
        // Variadic / opaque signatures (CONCAT "STRING, ...", MAP_RANGE shorthand).
        if op.signature.contains("...") || op.signature.contains(',') && !op.signature.contains(':')
        {
            return;
        }
        let expected = count_signature_args(&op.signature);
        if let Some(n) = expected {
            if args.len() != n {
                ctx.out.diagnostics.push(Diagnostic {
                    code: ErrorCode::ErrFunctionArityMismatch,
                    message: format!(
                        "stdlib `{name}` expects {n} args, got {}",
                        args.len()
                    ),
                });
            }
        }
        return;
    }

    // Allow unresolved only if it looks like a future user fn referenced before define —
    // Phase 1 treats unknown calls as errors.
    ctx.out.diagnostics.push(Diagnostic {
        code: ErrorCode::ErrUnresolvedFunction,
        message: format!("unresolved function `{name}`"),
    });
}

fn count_signature_args(sig: &str) -> Option<usize> {
    let sig = sig.trim();
    if sig.is_empty() {
        return Some(0);
    }
    // FB signatures like "IN: BOOL, PT: TIME → Q: BOOL, ET: TIME" — count inputs before arrow.
    let inputs = sig.split('→').next().unwrap_or(sig).split("->").next().unwrap_or(sig);
    let inputs = inputs.trim();
    if inputs.is_empty() {
        return Some(0);
    }
    Some(inputs.split(',').filter(|s| !s.trim().is_empty()).count())
}

/// Validate path for error corpus: apply inject, then validate or return inject-driven code.
pub fn validate_error_fixture(text: &str) -> Result<ValidationResult, dendri_parse::ParseError> {
    let parsed = dendri_parse::parse_program_str(text)?;
    let expect = parsed
        .program
        .metadata
        .expect_error
        .clone()
        .unwrap_or_default();

    // E02 path: mutate type then expect unknown-type.
    if parsed.inject.unknown_type_token.is_some() {
        match dendri_parse::apply_inject_and_reparse(text, &parsed.inject)? {
            Err(code) => {
                return Ok(ValidationResult {
                    diagnostics: vec![Diagnostic {
                        code,
                        message: format!("inject produced {code}"),
                    }],
                });
            }
            Ok(prog) => {
                return Ok(validate_program(&prog, &parsed.inject));
            }
        }
    }

    let mut result = validate_program(&parsed.program, &parsed.inject);

    // If fixture declares an expected error and we found it, keep only matching-ish set.
    if !expect.is_empty() && result.ok() {
        result.diagnostics.push(Diagnostic {
            code: ErrorCode::ErrIrDeserialize,
            message: format!("expected {expect} but validation passed"),
        });
    }
    Ok(result)
}
