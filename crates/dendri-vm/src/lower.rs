//! Load-time lowering of ASL v2 POUs (`Statements`, `Network`, `Chart`) into in-memory
//! executable bytecode and SFC state machines (`02` §2.3, `04` §5, ADR-026).
//!
//! **Zero synthetic temporaries (`__rail_N`, `__sfc_*`) are ever persisted in ASL.**
//! All power-rail registers and edge-memory slots live strictly inside this ephemeral
//! load-time representation.

use crate::RuntimeError;
use dendri_ast::{
    ActionQualifier, Board, CallArg, CasePattern, DataDecl, Expr, FbKind, ForeignFidelity,
    HardwareBinding, LValue, LadderBoxKind, LadderCell, LadderCellKind, LadderCoilKind,
    LadderCompareOp, LadderContactKind, LadderNetwork, NodeId, ObKind, ParamDecl, Pou, PouBody,
    PouKind, SfcChart, Stmt, StmtKind, TypeDecl, TypeRef,
};
use std::collections::{BTreeMap, BTreeSet};

/// Ephemeral boolean power-rail register used solely during in-memory Ladder execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RailReg(pub u32);

/// Ephemeral previous-boolean edge-detection slot for Ladder `--|P|--`/`--|N|--`/`--(P)--`/`--(N)--`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct EdgeReg(pub u32);

/// Flat bytecode instruction lowered at load time for cooperative suspend/resume.
#[derive(Debug, Clone)]
pub(crate) enum Instr {
    Assign {
        target: LValue,
        value: Expr,
    },
    CallStmt {
        callee: String,
        args: Vec<CallArg>,
    },
    FbInvoke {
        instance: LValue,
        inputs: Vec<(String, Expr)>,
    },
    Jump(usize),
    JumpIfFalse {
        condition: Expr,
        target: usize,
    },
    Switch {
        scrutinee: Expr,
        arms: Vec<(Vec<CasePattern>, usize)>,
        default_target: usize,
    },
    ForInit {
        counter: String,
        from: Expr,
        to: Expr,
        step: Expr,
        inclusive: bool,
        end_target: usize,
    },
    ForStep {
        counter: String,
        to: Expr,
        step: Expr,
        inclusive: bool,
        body_start: usize,
    },
    Return {
        value: Option<Expr>,
    },
    Wait {
        duration: Expr,
    },
    Foreign {
        node_id: NodeId,
        language: String,
        fidelity: ForeignFidelity,
        text: String,
    },
    // --- Ephemeral Ladder power-flow instructions (zero ASL temporaries!) ---
    RailConst {
        dst: RailReg,
        val: bool,
    },
    RailCopy {
        dst: RailReg,
        src: RailReg,
    },
    RailOr {
        dst: RailReg,
        a: RailReg,
        b: RailReg,
    },
    LadderContact {
        in_rail: RailReg,
        out_rail: RailReg,
        kind: LadderContactKind,
        target: LValue,
        edge_slot: Option<EdgeReg>,
    },
    LadderCoil {
        in_rail: RailReg,
        out_rail: RailReg,
        kind: LadderCoilKind,
        target: LValue,
        edge_slot: Option<EdgeReg>,
    },
    LadderCompare {
        in_rail: RailReg,
        out_rail: RailReg,
        op: LadderCompareOp,
        left: Expr,
        right: Expr,
    },
    LadderFbBox {
        in_rail: RailReg,
        out_rail: RailReg,
        fb_kind: FbKind,
        instance: LValue,
        inputs: Vec<(String, Expr)>,
    },
    LadderCallBox {
        in_rail: RailReg,
        out_rail: RailReg,
        callee: String,
        args: Vec<(Option<String>, Expr)>,
        output: Option<LValue>,
    },
    /// Executes an in-memory compiled SFC chart (`PouBody::Chart`) for one scan tick.
    RunSfcChart {
        chart_idx: usize,
    },
}

/// Lowered in-memory SFC action.
#[derive(Debug, Clone)]
pub(crate) struct CompiledSfcAction {
    pub qualifier: ActionQualifier,
    pub duration: Option<Expr>,
    pub target: Option<LValue>,
    pub body_instrs: Vec<Instr>,
}

/// Lowered in-memory SFC step.
#[derive(Debug, Clone)]
pub(crate) struct CompiledSfcStep {
    pub name: String,
    pub initial: bool,
    pub actions: Vec<CompiledSfcAction>,
    pub macro_chart: Option<String>,
}

/// Lowered in-memory SFC transition (pre-sorted by deterministic `(priority, original_idx)`).
#[derive(Debug, Clone)]
pub(crate) struct CompiledSfcTransition {
    pub from_steps: Vec<String>,
    pub to_steps: Vec<String>,
    pub receptivity: Expr,
}

/// Lowered in-memory GRAFCET/SFC chart (`04` §5.2).
#[derive(Debug, Clone)]
pub(crate) struct CompiledSfcChart {
    pub name: String,
    pub steps: Vec<CompiledSfcStep>,
    pub transitions: Vec<CompiledSfcTransition>,
    pub evolve_to_stability: bool,
}

/// Load-time compiled POU (either `Statements`, materialized `Network`, or `Chart` driver).
#[derive(Debug, Clone)]
pub(crate) struct CompiledPou {
    pub name: String,
    pub params: Vec<ParamDecl>,
    pub locals: Vec<DataDecl>,
    pub instrs: Vec<Instr>,
    pub rail_count: u32,
}

/// Complete load-time compiled representation of one board.
#[derive(Debug, Clone)]
pub(crate) struct CompiledBoard {
    pub board_id: String,
    pub hardware: BTreeMap<String, HardwareBinding>,
    pub struct_defs: BTreeMap<String, BTreeMap<String, TypeRef>>,
    pub user_fb_fields: BTreeMap<String, BTreeMap<String, TypeRef>>,
    /// Global data declarations ordered in module topological initialization order (`01` §6.1).
    pub ordered_data: Vec<DataDecl>,
    pub pous: Vec<CompiledPou>,
    pub pou_by_name: BTreeMap<String, usize>,
    pub startup_pou: Option<usize>,
    pub main_pou: Option<usize>,
    pub cyclic_ints: Vec<(usize, u64)>,
    pub hw_ints: Vec<(usize, String, dendri_ast::InterruptEdge, u8)>,
    pub comm_events: Vec<(usize, String)>,
    pub sfc_charts: Vec<CompiledSfcChart>,
    pub sfc_chart_by_name: BTreeMap<String, usize>,
    pub edge_slot_count: u32,
}

/// Compiles a validated [`Board`] into an executable in-memory [`CompiledBoard`].
pub(crate) fn compile_board(board: &Board) -> Result<CompiledBoard, RuntimeError> {
    let mut hardware = BTreeMap::new();
    for h in &board.program.hardware {
        hardware.insert(h.label.clone(), h.clone());
    }

    let mut struct_defs = BTreeMap::new();
    for td in board
        .program
        .types
        .iter()
        .chain(board.program.modules.iter().flat_map(|m| m.types.iter()))
    {
        if let TypeDecl::Struct { name, fields, .. } = td {
            let mut map = BTreeMap::new();
            for f in fields {
                map.insert(f.name.clone(), f.ty.clone());
            }
            struct_defs.insert(name.clone(), map);
        }
    }

    // Topological sort of embedded modules so module initializers run once in dependency order
    // before `OB_STARTUP` (`01` §6.1).
    let module_order = topo_sort_modules(board)?;
    let mut ordered_data = Vec::new();
    for idx in &module_order {
        if let Some(m) = board.program.modules.get(*idx) {
            ordered_data.extend(m.data.iter().cloned());
        }
    }
    ordered_data.extend(board.program.data.iter().cloned());

    let mut all_pous: Vec<&Pou> = Vec::new();
    for idx in &module_order {
        if let Some(m) = board.program.modules.get(*idx) {
            all_pous.extend(m.pous.iter());
        }
    }
    all_pous.extend(board.program.pous.iter());

    let mut user_fb_fields = BTreeMap::new();
    for pou in &all_pous {
        if matches!(pou.kind, PouKind::Fb) {
            let mut f = BTreeMap::new();
            for p in &pou.params {
                f.insert(p.name.clone(), p.ty.clone());
            }
            user_fb_fields.insert(pou.name.clone(), f);
        }
    }

    let mut compiled_pous = Vec::with_capacity(all_pous.len());
    let mut pou_by_name = BTreeMap::new();
    let mut startup_pou = None;
    let mut main_pou = None;
    let mut cyclic_ints = Vec::new();
    let mut hw_ints = Vec::new();
    let mut comm_events = Vec::new();
    let mut sfc_charts = Vec::new();
    let mut sfc_chart_by_name = BTreeMap::new();
    let mut edge_alloc = 0u32;

    for pou in all_pous {
        let pou_idx = compiled_pous.len();
        pou_by_name.insert(pou.name.clone(), pou_idx);

        match &pou.kind {
            PouKind::Ob {
                ob: ObKind::Startup,
            } => startup_pou = Some(pou_idx),
            PouKind::Ob { ob: ObKind::Main } => main_pou = Some(pou_idx),
            PouKind::Ob {
                ob: ObKind::CyclicInt { period_us },
            } => cyclic_ints.push((pou_idx, *period_us)),
            PouKind::Ob {
                ob:
                    ObKind::HwInt {
                        pin_label,
                        edge,
                        priority,
                    },
            } => hw_ints.push((pou_idx, pin_label.clone(), *edge, *priority)),
            PouKind::Ob {
                ob: ObKind::CommEvent { bus_id },
            } => comm_events.push((pou_idx, bus_id.clone())),
            PouKind::Fc { .. } | PouKind::Fb => {}
        }

        let (instrs, rail_count) = match &pou.body {
            PouBody::Statements(stmts) => {
                let mut b = StmtLowerer::default();
                b.lower_block(stmts)?;
                (b.instrs, 0)
            }
            PouBody::Network(net) => lower_ladder_network(net, &mut edge_alloc),
            PouBody::Chart(chart) => {
                let chart_idx = sfc_charts.len();
                let compiled_chart = lower_sfc_chart(&pou.name, chart)?;
                sfc_chart_by_name.insert(pou.name.clone(), chart_idx);
                sfc_charts.push(compiled_chart);
                (vec![Instr::RunSfcChart { chart_idx }], 0)
            }
        };

        compiled_pous.push(CompiledPou {
            name: pou.name.clone(),
            params: pou.params.clone(),
            locals: pou.locals.clone(),
            instrs,
            rail_count,
        });
    }

    // Sort HW interrupts deterministically by (priority ascending, pou_idx ascending)
    hw_ints.sort_by_key(|&(idx, _, _, prio)| (prio, idx));

    Ok(CompiledBoard {
        board_id: board.id.clone(),
        hardware,
        struct_defs,
        user_fb_fields,
        ordered_data,
        pous: compiled_pous,
        pou_by_name,
        startup_pou,
        main_pou,
        cyclic_ints,
        hw_ints,
        comm_events,
        sfc_charts,
        sfc_chart_by_name,
        edge_slot_count: edge_alloc,
    })
}

fn topo_sort_modules(board: &Board) -> Result<Vec<usize>, RuntimeError> {
    let modules = &board.program.modules;
    let mut name_to_idx: BTreeMap<&str, usize> = BTreeMap::new();
    for (idx, m) in modules.iter().enumerate() {
        name_to_idx.insert(m.name.as_str(), idx);
    }

    let mut visited = BTreeSet::new();
    let mut visiting = BTreeSet::new();
    let mut order = Vec::with_capacity(modules.len());

    fn visit(
        idx: usize,
        board: &Board,
        name_to_idx: &BTreeMap<&str, usize>,
        visited: &mut BTreeSet<usize>,
        visiting: &mut BTreeSet<usize>,
        order: &mut Vec<usize>,
    ) -> Result<(), RuntimeError> {
        if visited.contains(&idx) {
            return Ok(());
        }
        if !visiting.insert(idx) {
            return Err(RuntimeError::ImportCycle);
        }
        if let Some(m) = board.program.modules.get(idx) {
            for imp in &m.imports {
                if let Some(dep_name) = imp.strip_prefix("project.")
                    && let Some(&dep_idx) = name_to_idx.get(dep_name)
                {
                    visit(dep_idx, board, name_to_idx, visited, visiting, order)?;
                }
            }
        }
        visiting.remove(&idx);
        visited.insert(idx);
        order.push(idx);
        Ok(())
    }

    for idx in 0..modules.len() {
        visit(
            idx,
            board,
            &name_to_idx,
            &mut visited,
            &mut visiting,
            &mut order,
        )?;
    }
    Ok(order)
}

// ===========================================================================
// Statement Lowerer (`PouBody::Statements` -> flat CFG bytecode)
// ===========================================================================

#[derive(Default)]
struct StmtLowerer {
    instrs: Vec<Instr>,
    loop_stack: Vec<LoopPatch>,
}

struct LoopPatch {
    continue_jumps: Vec<usize>,
    break_jumps: Vec<usize>,
}

impl StmtLowerer {
    fn emit(&mut self, instr: Instr) -> usize {
        let idx = self.instrs.len();
        self.instrs.push(instr);
        idx
    }

    fn patch_jump(&mut self, instr_idx: usize, target: usize) {
        if let Some(
            Instr::Jump(t)
            | Instr::JumpIfFalse { target: t, .. }
            | Instr::ForInit { end_target: t, .. },
        ) = self.instrs.get_mut(instr_idx)
        {
            *t = target;
        }
    }

    fn lower_block(&mut self, stmts: &[Stmt]) -> Result<(), RuntimeError> {
        for s in stmts {
            self.lower_stmt(s)?;
        }
        Ok(())
    }

    fn lower_stmt(&mut self, stmt: &Stmt) -> Result<(), RuntimeError> {
        match &stmt.kind {
            StmtKind::Assign { target, value } => {
                self.emit(Instr::Assign {
                    target: target.clone(),
                    value: value.clone(),
                });
            }
            StmtKind::Call { callee, args } => {
                self.emit(Instr::CallStmt {
                    callee: callee.clone(),
                    args: args.clone(),
                });
            }
            StmtKind::FbInvoke { instance, inputs } => {
                self.emit(Instr::FbInvoke {
                    instance: instance.clone(),
                    inputs: inputs.clone(),
                });
            }
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => {
                let mut end_jumps = Vec::new();
                let j_false = self.emit(Instr::JumpIfFalse {
                    condition: condition.clone(),
                    target: 0,
                });
                self.lower_block(then_body)?;
                end_jumps.push(self.emit(Instr::Jump(0)));
                let next_start = self.instrs.len();
                self.patch_jump(j_false, next_start);

                for arm in elif_arms {
                    let j_arm_false = self.emit(Instr::JumpIfFalse {
                        condition: arm.condition.clone(),
                        target: 0,
                    });
                    self.lower_block(&arm.body)?;
                    end_jumps.push(self.emit(Instr::Jump(0)));
                    let after_arm = self.instrs.len();
                    self.patch_jump(j_arm_false, after_arm);
                }

                self.lower_block(else_body)?;
                let end_pc = self.instrs.len();
                for j in end_jumps {
                    self.patch_jump(j, end_pc);
                }
            }
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => {
                let switch_pc = self.emit(Instr::Switch {
                    scrutinee: scrutinee.clone(),
                    arms: Vec::new(),
                    default_target: 0,
                });
                let mut arms = Vec::with_capacity(cases.len());
                let mut end_jumps = Vec::new();
                for case in cases {
                    let arm_pc = self.instrs.len();
                    arms.push((case.patterns.clone(), arm_pc));
                    self.lower_block(&case.body)?;
                    end_jumps.push(self.emit(Instr::Jump(0)));
                }
                let default_pc = self.instrs.len();
                if let Some(def_stmts) = default {
                    self.lower_block(def_stmts)?;
                }
                let end_pc = self.instrs.len();
                for j in end_jumps {
                    self.patch_jump(j, end_pc);
                }
                if let Some(Instr::Switch {
                    arms: dst_arms,
                    default_target: dst_def,
                    ..
                }) = self.instrs.get_mut(switch_pc)
                {
                    *dst_arms = arms;
                    *dst_def = default_pc;
                }
            }
            StmtKind::While { condition, body } => {
                let cond_pc = self.instrs.len();
                let exit_j = self.emit(Instr::JumpIfFalse {
                    condition: condition.clone(),
                    target: 0,
                });
                self.loop_stack.push(LoopPatch {
                    continue_jumps: Vec::new(),
                    break_jumps: Vec::new(),
                });
                self.lower_block(body)?;
                self.emit(Instr::Jump(cond_pc));
                let end_pc = self.instrs.len();
                self.patch_jump(exit_j, end_pc);
                if let Some(patch) = self.loop_stack.pop() {
                    for cj in patch.continue_jumps {
                        self.patch_jump(cj, cond_pc);
                    }
                    for bj in patch.break_jumps {
                        self.patch_jump(bj, end_pc);
                    }
                }
            }
            StmtKind::For {
                counter,
                from,
                to,
                step,
                inclusive,
                body,
            } => {
                let init_pc = self.emit(Instr::ForInit {
                    counter: counter.clone(),
                    from: from.clone(),
                    to: to.clone(),
                    step: step.clone(),
                    inclusive: *inclusive,
                    end_target: 0,
                });
                let body_start = self.instrs.len();
                self.loop_stack.push(LoopPatch {
                    continue_jumps: Vec::new(),
                    break_jumps: Vec::new(),
                });
                self.lower_block(body)?;
                let step_pc = self.instrs.len();
                self.emit(Instr::ForStep {
                    counter: counter.clone(),
                    to: to.clone(),
                    step: step.clone(),
                    inclusive: *inclusive,
                    body_start,
                });
                let end_pc = self.instrs.len();
                self.patch_jump(init_pc, end_pc);
                if let Some(patch) = self.loop_stack.pop() {
                    for cj in patch.continue_jumps {
                        self.patch_jump(cj, step_pc);
                    }
                    for bj in patch.break_jumps {
                        self.patch_jump(bj, end_pc);
                    }
                }
            }
            StmtKind::Break => {
                let j = self.emit(Instr::Jump(0));
                if let Some(top) = self.loop_stack.last_mut() {
                    top.break_jumps.push(j);
                } else {
                    return Err(RuntimeError::BreakOutsideLoop);
                }
            }
            StmtKind::Continue => {
                let j = self.emit(Instr::Jump(0));
                if let Some(top) = self.loop_stack.last_mut() {
                    top.continue_jumps.push(j);
                } else {
                    return Err(RuntimeError::ContinueOutsideLoop);
                }
            }
            StmtKind::Return { value } => {
                self.emit(Instr::Return {
                    value: value.clone(),
                });
            }
            StmtKind::Wait { duration } => {
                self.emit(Instr::Wait {
                    duration: duration.clone(),
                });
            }
            StmtKind::Foreign {
                language,
                fidelity,
                text,
                ..
            } => {
                self.emit(Instr::Foreign {
                    node_id: stmt.id.clone(),
                    language: language.clone(),
                    fidelity: *fidelity,
                    text: text.clone(),
                });
            }
        }
        Ok(())
    }
}

// ===========================================================================
// Load-Time Materialized Power-Flow Ladder Lowerer (`PouBody::Network`)
// ===========================================================================

fn lower_ladder_network(net: &LadderNetwork, edge_alloc: &mut u32) -> (Vec<Instr>, u32) {
    let mut instrs = Vec::new();
    let mut next_rail = 0u32;

    let alloc_rail = |n: &mut u32| -> RailReg {
        let r = RailReg(*n);
        *n = n.saturating_add(1);
        r
    };

    // Sort rungs strictly by explicit `rung.order` (never trusting raw array order!)
    let mut sorted_rungs: Vec<&dendri_ast::LadderRung> = net.rungs.iter().collect();
    sorted_rungs.sort_by_key(|r| r.order);

    for rung in sorted_rungs {
        // Left power rail starts energized (`true`)
        let left_rail = alloc_rail(&mut next_rail);
        instrs.push(Instr::RailConst {
            dst: left_rail,
            val: true,
        });

        // Sort rung cells left-to-right by `(col, row)`
        let mut sorted_cells: Vec<&LadderCell> = rung.cells.iter().collect();
        sorted_cells.sort_by_key(|c| (c.col, c.row));

        let mut cur_rail = left_rail;
        for cell in sorted_cells {
            cur_rail = lower_ladder_cell(cell, cur_rail, &mut next_rail, edge_alloc, &mut instrs);
        }
    }

    (instrs, next_rail)
}

fn lower_ladder_cell(
    cell: &LadderCell,
    in_rail: RailReg,
    next_rail: &mut u32,
    edge_alloc: &mut u32,
    instrs: &mut Vec<Instr>,
) -> RailReg {
    let out_rail = RailReg(*next_rail);
    *next_rail = next_rail.saturating_add(1);

    match &cell.kind {
        // Empty cell = OPEN CIRCUIT (`false`), NEVER conducts!
        LadderCellKind::Empty => {
            instrs.push(Instr::RailConst {
                dst: out_rail,
                val: false,
            });
        }
        LadderCellKind::Wire => {
            instrs.push(Instr::RailCopy {
                dst: out_rail,
                src: in_rail,
            });
        }
        LadderCellKind::Contact { kind, target } => {
            let edge_slot = if matches!(
                kind,
                LadderContactKind::PositiveTransition | LadderContactKind::NegativeTransition
            ) {
                let s = EdgeReg(*edge_alloc);
                *edge_alloc = edge_alloc.saturating_add(1);
                Some(s)
            } else {
                None
            };
            instrs.push(Instr::LadderContact {
                in_rail,
                out_rail,
                kind: *kind,
                target: target.clone(),
                edge_slot,
            });
        }
        LadderCellKind::Coil { kind, target } => {
            let edge_slot = if matches!(
                kind,
                LadderCoilKind::PositiveTransition | LadderCoilKind::NegativeTransition
            ) {
                let s = EdgeReg(*edge_alloc);
                *edge_alloc = edge_alloc.saturating_add(1);
                Some(s)
            } else {
                None
            };
            instrs.push(Instr::LadderCoil {
                in_rail,
                out_rail,
                kind: *kind,
                target: target.clone(),
                edge_slot,
            });
        }
        LadderCellKind::ParallelOr { branches } => {
            instrs.push(Instr::RailConst {
                dst: out_rail,
                val: false,
            });
            for branch in branches {
                let mut sorted: Vec<&LadderCell> = branch.iter().collect();
                sorted.sort_by_key(|c| (c.col, c.row));
                let mut br_rail = in_rail;
                for c in sorted {
                    br_rail = lower_ladder_cell(c, br_rail, next_rail, edge_alloc, instrs);
                }
                instrs.push(Instr::RailOr {
                    dst: out_rail,
                    a: out_rail,
                    b: br_rail,
                });
            }
        }
        LadderCellKind::Box(box_kind) => match box_kind {
            LadderBoxKind::Compare { op, left, right } => {
                instrs.push(Instr::LadderCompare {
                    in_rail,
                    out_rail,
                    op: *op,
                    left: left.clone(),
                    right: right.clone(),
                });
            }
            LadderBoxKind::Fb {
                kind,
                instance,
                inputs,
            } => {
                instrs.push(Instr::LadderFbBox {
                    in_rail,
                    out_rail,
                    fb_kind: kind.clone(),
                    instance: instance.clone(),
                    inputs: inputs.clone(),
                });
            }
            LadderBoxKind::Call {
                callee,
                args,
                output,
            } => {
                instrs.push(Instr::LadderCallBox {
                    in_rail,
                    out_rail,
                    callee: callee.clone(),
                    args: args.clone(),
                    output: output.clone(),
                });
            }
            LadderBoxKind::Foreign {
                language,
                fidelity,
                text,
                ..
            } => {
                instrs.push(Instr::Foreign {
                    node_id: cell.id.clone(),
                    language: language.clone(),
                    fidelity: *fidelity,
                    text: text.clone(),
                });
                instrs.push(Instr::RailCopy {
                    dst: out_rail,
                    src: in_rail,
                });
            }
        },
    }

    out_rail
}

// ===========================================================================
// Load-Time GRAFCET / SFC Lowerer (`PouBody::Chart`)
// ===========================================================================

fn lower_sfc_chart(name: &str, chart: &SfcChart) -> Result<CompiledSfcChart, RuntimeError> {
    let mut steps = Vec::with_capacity(chart.steps.len());
    for s in &chart.steps {
        let mut actions = Vec::with_capacity(s.actions.len());
        for a in &s.actions {
            let mut b = StmtLowerer::default();
            b.lower_block(&a.body)?;
            actions.push(CompiledSfcAction {
                qualifier: a.qualifier,
                duration: a.duration.clone(),
                target: a.target.clone(),
                body_instrs: b.instrs,
            });
        }
        steps.push(CompiledSfcStep {
            name: s.name.clone(),
            initial: s.initial,
            actions,
            macro_chart: s.macro_chart.clone(),
        });
    }

    // Sort transitions deterministically by `(priority ascending, original index)` (`04` §5.2)
    let mut indexed: Vec<(usize, &dendri_ast::SfcTransition)> =
        chart.transitions.iter().enumerate().collect();
    indexed.sort_by_key(|&(orig_idx, tr)| (tr.priority, orig_idx));

    let transitions = indexed
        .into_iter()
        .map(|(_, tr)| CompiledSfcTransition {
            from_steps: tr.from_steps.clone(),
            to_steps: tr.to_steps.clone(),
            receptivity: tr.receptivity.clone(),
        })
        .collect();

    Ok(CompiledSfcChart {
        name: name.to_owned(),
        steps,
        transitions,
        evolve_to_stability: chart.evolve_to_stability,
    })
}
