//! Deterministic scan-cycle simulator for ASL.
//!
//! MVP subset: `SET_BOOL`, `GET_BOOL`, `WAIT_TIME`, `TON`, plus assign / if / call
//! plumbing needed to run blink-style programs. Visual SVG binding is deferred;
//! this crate emits logical traces only.

use std::collections::HashMap;
use std::fmt::Write as _;

use dendri_ast::expr::{BinaryOp, Expr, UnaryOp};
use dendri_ast::lit::Literal;
use dendri_ast::stmt::Stmt;
use dendri_ast::{Program, SimulationProject};
use dendri_parse::parse_expr_text;

mod value;
pub use value::Value;

/// One deterministic trace event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEvent {
    pub time_ms: u64,
    pub kind: TraceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceKind {
    Boot,
    SetupDone,
    ScanStart { cycle: u64 },
    ScanEnd { cycle: u64 },
    PinWrite { pin: u32, value: bool },
    PinRead { pin: u32, value: bool },
    Wait { duration_ms: u64 },
    TonUpdate {
        instance: String,
        q: bool,
        et_ms: u64,
    },
    Assign { name: String, value: String },
    Halt { reason: String },
}

impl TraceEvent {
    /// Stable line format for snapshot tests.
    pub fn to_line(&self) -> String {
        match &self.kind {
            TraceKind::Boot => format!("t={:>6} BOOT", self.time_ms),
            TraceKind::SetupDone => format!("t={:>6} SETUP_DONE", self.time_ms),
            TraceKind::ScanStart { cycle } => {
                format!("t={:>6} SCAN_START cycle={cycle}", self.time_ms)
            }
            TraceKind::ScanEnd { cycle } => {
                format!("t={:>6} SCAN_END cycle={cycle}", self.time_ms)
            }
            TraceKind::PinWrite { pin, value } => {
                format!("t={:>6} PIN_WRITE pin={pin} value={value}", self.time_ms)
            }
            TraceKind::PinRead { pin, value } => {
                format!("t={:>6} PIN_READ pin={pin} value={value}", self.time_ms)
            }
            TraceKind::Wait { duration_ms } => {
                format!("t={:>6} WAIT ms={duration_ms}", self.time_ms)
            }
            TraceKind::TonUpdate {
                instance,
                q,
                et_ms,
            } => format!(
                "t={:>6} TON id={instance} Q={q} ET={et_ms}",
                self.time_ms
            ),
            TraceKind::Assign { name, value } => {
                format!("t={:>6} ASSIGN {name}={value}", self.time_ms)
            }
            TraceKind::Halt { reason } => {
                format!("t={:>6} HALT {reason}", self.time_ms)
            }
        }
    }
}

#[derive(Debug, Clone)]
struct TonState {
    et_ms: u64,
    pt_ms: u64,
    q: bool,
    prev_in: bool,
}

#[derive(Debug, Clone)]
pub struct Simulator {
    program: Program,
    vars: HashMap<String, Value>,
    pins: HashMap<u32, bool>,
    time_ms: u64,
    cycle: u64,
    setup_done: bool,
    tons: HashMap<String, TonState>,
    functions: HashMap<String, (Vec<String>, Vec<Stmt>)>,
    pub trace: Vec<TraceEvent>,
    /// Max statements per scan to avoid runaway while/for.
    stmt_budget: usize,
}

#[derive(Debug, Clone)]
pub struct SimConfig {
    pub max_cycles: u64,
    pub stmt_budget: usize,
    /// Optional initial pin levels (inputs).
    pub initial_pins: HashMap<u32, bool>,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            max_cycles: 8,
            stmt_budget: 10_000,
            initial_pins: HashMap::new(),
        }
    }
}

impl Simulator {
    pub fn from_program(program: Program, config: &SimConfig) -> Self {
        let mut vars = HashMap::new();
        for d in &program.data {
            let v = Value::parse_init(&d.init, d.data_type);
            vars.insert(d.name.clone(), v);
        }
        let mut functions = HashMap::new();
        for f in &program.functions {
            functions.insert(f.name.clone(), (f.params.clone(), f.body.clone()));
        }
        let mut pins = config.initial_pins.clone();
        for hw in &program.hardware {
            pins.entry(hw.pin).or_insert(false);
        }
        Self {
            program,
            vars,
            pins,
            time_ms: 0,
            cycle: 0,
            setup_done: false,
            tons: HashMap::new(),
            functions,
            trace: Vec::new(),
            stmt_budget: config.stmt_budget,
        }
    }

    pub fn from_project(project: &SimulationProject, config: &SimConfig) -> Self {
        Self::from_program(project.asl.clone(), config)
    }

    fn push(&mut self, kind: TraceKind) {
        self.trace.push(TraceEvent {
            time_ms: self.time_ms,
            kind,
        });
    }

    pub fn run(&mut self, max_cycles: u64) -> Result<(), String> {
        self.push(TraceKind::Boot);
        if !self.setup_done {
            self.exec_block(&self.program.setup.clone())?;
            self.setup_done = true;
            self.push(TraceKind::SetupDone);
        }
        while self.cycle < max_cycles {
            self.cycle += 1;
            self.push(TraceKind::ScanStart { cycle: self.cycle });
            self.exec_block(&self.program.loop_body.clone())?;
            self.push(TraceKind::ScanEnd { cycle: self.cycle });
        }
        Ok(())
    }

    pub fn trace_text(&self) -> String {
        let mut s = String::new();
        for ev in &self.trace {
            let _ = writeln!(s, "{}", ev.to_line());
        }
        s
    }

    fn exec_block(&mut self, body: &[Stmt]) -> Result<(), String> {
        for stmt in body {
            self.exec_stmt(stmt)?;
        }
        Ok(())
    }

    fn exec_stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        if self.stmt_budget == 0 {
            return Err("statement budget exceeded".into());
        }
        self.stmt_budget -= 1;

        match stmt {
            Stmt::Assign { target, value } => {
                let v = self.eval(value)?;
                self.push(TraceKind::Assign {
                    name: target.clone(),
                    value: v.to_trace(),
                });
                self.vars.insert(target.clone(), v);
            }
            Stmt::Call { name, args } => {
                self.eval_call(name, args)?;
            }
            Stmt::If {
                condition,
                then,
                elif,
                else_body,
            } => {
                if self.eval(condition)?.as_bool()? {
                    self.exec_block(then)?;
                } else {
                    let mut taken = false;
                    for arm in elif {
                        if self.eval(&arm.condition)?.as_bool()? {
                            self.exec_block(&arm.then)?;
                            taken = true;
                            break;
                        }
                    }
                    if !taken {
                        if let Some(body) = else_body {
                            self.exec_block(body)?;
                        }
                    }
                }
            }
            Stmt::While { condition, body } => {
                let mut guard = 0u32;
                while self.eval(condition)?.as_bool()? {
                    self.exec_block(body)?;
                    guard += 1;
                    if guard > 1000 {
                        return Err("while loop guard exceeded".into());
                    }
                }
            }
            Stmt::For { counter, to, body } => {
                let n = self.eval(to)?.as_u64()?;
                let cname = counter.clone().unwrap_or_else(|| "_i".into());
                for i in 0..n {
                    self.vars.insert(cname.clone(), Value::Uint(i));
                    self.exec_block(body)?;
                }
            }
            Stmt::Return { .. } => {
                return Err("return outside function".into());
            }
            Stmt::Exit => {
                self.push(TraceKind::Halt {
                    reason: "EXIT".into(),
                });
            }
            Stmt::Continue => {}
            Stmt::VendorExtension { reason } => {
                self.push(TraceKind::Halt {
                    reason: format!("VENDOR_EXTENSION:{reason}"),
                });
            }
        }
        Ok(())
    }

    fn eval(&mut self, expr: &Expr) -> Result<Value, String> {
        match expr {
            Expr::Bool { value } => Ok(Value::Bool(*value)),
            Expr::Uint { value } => Ok(Value::Uint(*value)),
            Expr::Int { value } => Ok(Value::Int(*value)),
            Expr::Real { value } => Ok(Value::Real(*value)),
            Expr::String { value } => Ok(Value::String(value.clone())),
            Expr::Time { value } => Ok(Value::TimeMs(parse_time_ms(value)?)),
            Expr::Hex { value } => Ok(Value::Uint(parse_hex(value)?)),
            Expr::Lit { value } => Ok(match value {
                Literal::Bool { value } => Value::Bool(*value),
                Literal::Int { value } => Value::Int(*value),
                Literal::Uint { value } => Value::Uint(*value),
                Literal::Real { value } => Value::Real(*value),
                Literal::String { value } => Value::String(value.clone()),
                Literal::Time { value } => Value::TimeMs(parse_time_ms(value)?),
                Literal::Hex { value } => Value::Uint(parse_hex(value)?),
            }),
            Expr::Var { name } => self
                .vars
                .get(name)
                .cloned()
                .ok_or_else(|| format!("undeclared var {name}")),
            Expr::Unary { op, expr } => {
                let v = self.eval(expr)?;
                match op {
                    UnaryOp::Not => Ok(Value::Bool(!v.as_bool()?)),
                    UnaryOp::Neg => Ok(Value::Int(-v.as_i64()?)),
                    UnaryOp::BitNot => Ok(Value::Uint(!v.as_u64()?)),
                }
            }
            Expr::Binop { op, left, right } => {
                let l = self.eval(left)?;
                let r = self.eval(right)?;
                eval_binop(*op, l, r)
            }
            Expr::Call { name, args } => self.eval_call(name, args),
            Expr::ExprText { text } => {
                let e = parse_expr_text(text)?;
                self.eval(&e)
            }
        }
    }

    fn eval_call(&mut self, name: &str, args: &[Expr]) -> Result<Value, String> {
        let canon = dendri_stdlib::resolve_name(name).unwrap_or(name);

        // User function
        if let Some((params, body)) = self.functions.get(canon).cloned() {
            if params.len() != args.len() {
                return Err(format!(
                    "arity mismatch calling {canon}: {} vs {}",
                    params.len(),
                    args.len()
                ));
            }
            let mut locals = HashMap::new();
            for (p, a) in params.iter().zip(args.iter()) {
                locals.insert(p.clone(), self.eval(a)?);
            }
            // Simple function interpreter: restore globals after.
            let saved = self.vars.clone();
            for (k, v) in locals {
                self.vars.insert(k, v);
            }
            let mut ret = Value::Bool(false);
            for stmt in &body {
                match stmt {
                    Stmt::Return { value } => {
                        ret = if let Some(v) = value {
                            self.eval(v)?
                        } else {
                            Value::Bool(false)
                        };
                        break;
                    }
                    other => self.exec_stmt(other)?,
                }
            }
            // Keep only original globals (drop params).
            let mut merged = saved;
            for (k, v) in self.vars.drain() {
                if merged.contains_key(&k) {
                    merged.insert(k, v);
                }
            }
            self.vars = merged;
            return Ok(ret);
        }

        match canon {
            "CFG_DIGITAL_OUTPUT" | "CFG_DIGITAL_INPUT" | "CFG_DIGITAL_INPUT_PULLUP"
            | "CFG_DIGITAL_INPUT_PULLDOWN" => {
                let _pin = self.eval(&args[0])?.as_u64()? as u32;
                Ok(Value::Void)
            }
            "SET_BOOL" => {
                let pin = self.eval(&args[0])?.as_u64()? as u32;
                let val = self.eval(&args[1])?.as_bool()?;
                self.pins.insert(pin, val);
                self.push(TraceKind::PinWrite { pin, value: val });
                Ok(Value::Void)
            }
            "GET_BOOL" => {
                let pin = self.eval(&args[0])?.as_u64()? as u32;
                let val = *self.pins.get(&pin).unwrap_or(&false);
                self.push(TraceKind::PinRead { pin, value: val });
                Ok(Value::Bool(val))
            }
            "WAIT_TIME" => {
                let ms = match self.eval(&args[0])? {
                    Value::TimeMs(ms) => ms,
                    Value::Uint(u) => u,
                    Value::Int(i) => i.max(0) as u64,
                    other => return Err(format!("WAIT_TIME expects TIME, got {other:?}")),
                };
                self.push(TraceKind::Wait { duration_ms: ms });
                self.time_ms = self.time_ms.saturating_add(ms);
                // Advance TON elapsed for active instances while waiting.
                self.advance_tons(ms);
                Ok(Value::Void)
            }
            "TON" => self.eval_ton(args),
            "CLAMP" => {
                let x = self.eval(&args[0])?.as_i64()?;
                let lo = self.eval(&args[1])?.as_i64()?;
                let hi = self.eval(&args[2])?.as_i64()?;
                Ok(Value::Int(x.clamp(lo, hi)))
            }
            "MOD" => {
                let a = self.eval(&args[0])?.as_u64()?;
                let b = self.eval(&args[1])?.as_u64()?;
                if b == 0 {
                    return Err("MOD by zero".into());
                }
                Ok(Value::Uint(a % b))
            }
            "GET_SYS_TIME" => Ok(Value::Uint(self.time_ms)),
            other => Err(format!("unsupported call in sim MVP: {other}")),
        }
    }

    /// TON(IN, PT) or TON(instance, IN, PT). Returns Q.
    fn eval_ton(&mut self, args: &[Expr]) -> Result<Value, String> {
        let (instance, in_expr, pt_expr) = match args.len() {
            2 => ("__ton0".to_string(), &args[0], &args[1]),
            3 => {
                let id = match self.eval(&args[0])? {
                    Value::String(s) => s,
                    other => return Err(format!("TON instance id must be string, got {other:?}")),
                };
                (id, &args[1], &args[2])
            }
            n => return Err(format!("TON expects 2 or 3 args, got {n}")),
        };
        let inn = self.eval(in_expr)?.as_bool()?;
        let pt = match self.eval(pt_expr)? {
            Value::TimeMs(ms) => ms,
            Value::Uint(u) => u,
            other => return Err(format!("TON PT expects TIME, got {other:?}")),
        };

        {
            let st = self.tons.entry(instance.clone()).or_insert(TonState {
                et_ms: 0,
                pt_ms: pt,
                q: false,
                prev_in: false,
            });
            st.pt_ms = pt;

            if !inn {
                st.et_ms = 0;
                st.q = false;
            } else if !st.prev_in {
                // Rising edge — restart ET; elapsed advances on WAIT_TIME.
                st.et_ms = 0;
                st.q = false;
            }
            // Scan-cycle model: time advances via WAIT_TIME; Q latches when ET >= PT.
            if inn && st.et_ms >= st.pt_ms {
                st.q = true;
            }
            st.prev_in = inn;
        }
        let (q, et) = self
            .tons
            .get(&instance)
            .map(|s| (s.q, s.et_ms))
            .unwrap_or((false, 0));
        self.push(TraceKind::TonUpdate {
            instance,
            q,
            et_ms: et,
        });
        Ok(Value::Bool(q))
    }

    fn advance_tons(&mut self, ms: u64) {
        for st in self.tons.values_mut() {
            if st.prev_in {
                st.et_ms = st.et_ms.saturating_add(ms);
                if st.et_ms >= st.pt_ms {
                    st.q = true;
                }
            }
        }
    }
}

fn eval_binop(op: BinaryOp, l: Value, r: Value) -> Result<Value, String> {
    match op {
        BinaryOp::Add => Ok(Value::Int(l.as_i64()?.saturating_add(r.as_i64()?))),
        BinaryOp::Sub => Ok(Value::Int(l.as_i64()?.saturating_sub(r.as_i64()?))),
        BinaryOp::Mul => Ok(Value::Int(l.as_i64()?.saturating_mul(r.as_i64()?))),
        BinaryOp::Div => {
            let d = r.as_i64()?;
            if d == 0 {
                return Err("div by zero".into());
            }
            Ok(Value::Int(l.as_i64()? / d))
        }
        BinaryOp::Mod => {
            let d = r.as_u64()?;
            if d == 0 {
                return Err("mod by zero".into());
            }
            Ok(Value::Uint(l.as_u64()? % d))
        }
        BinaryOp::Eq => Ok(Value::Bool(l == r)),
        BinaryOp::Ne => Ok(Value::Bool(l != r)),
        BinaryOp::Lt => Ok(Value::Bool(l.as_i64()? < r.as_i64()?)),
        BinaryOp::Le => Ok(Value::Bool(l.as_i64()? <= r.as_i64()?)),
        BinaryOp::Gt => Ok(Value::Bool(l.as_i64()? > r.as_i64()?)),
        BinaryOp::Ge => Ok(Value::Bool(l.as_i64()? >= r.as_i64()?)),
        BinaryOp::And => Ok(Value::Bool(l.as_bool()? && r.as_bool()?)),
        BinaryOp::Or => Ok(Value::Bool(l.as_bool()? || r.as_bool()?)),
        BinaryOp::Xor => Ok(Value::Bool(l.as_bool()? ^ r.as_bool()?)),
        BinaryOp::BitAnd => Ok(Value::Uint(l.as_u64()? & r.as_u64()?)),
        BinaryOp::BitOr => Ok(Value::Uint(l.as_u64()? | r.as_u64()?)),
        BinaryOp::BitXor => Ok(Value::Uint(l.as_u64()? ^ r.as_u64()?)),
        BinaryOp::Shl => Ok(Value::Uint(l.as_u64()? << r.as_u64()?)),
        BinaryOp::Shr => Ok(Value::Uint(l.as_u64()? >> r.as_u64()?)),
    }
}

pub fn parse_time_ms(text: &str) -> Result<u64, String> {
    let t = text.trim();
    let body = if let Some(rest) = t.strip_prefix("T#").or_else(|| t.strip_prefix("t#")) {
        rest
    } else {
        t
    };
    // Support 500ms, 1s, 1m, 1000
    if let Some(num) = body.strip_suffix("ms") {
        return num
            .parse()
            .map_err(|e| format!("bad TIME ms {body}: {e}"));
    }
    if let Some(num) = body.strip_suffix('s') {
        let s: u64 = num.parse().map_err(|e| format!("bad TIME s: {e}"))?;
        return Ok(s.saturating_mul(1000));
    }
    if let Some(num) = body.strip_suffix('m') {
        let m: u64 = num.parse().map_err(|e| format!("bad TIME m: {e}"))?;
        return Ok(m.saturating_mul(60_000));
    }
    body.parse()
        .map_err(|e| format!("bad TIME literal {text}: {e}"))
}

fn parse_hex(text: &str) -> Result<u64, String> {
    let t = text.trim();
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return u64::from_str_radix(h, 16).map_err(|e| e.to_string());
    }
    if let Some(h) = t.strip_prefix("16#") {
        return u64::from_str_radix(h, 16).map_err(|e| e.to_string());
    }
    Err(format!("bad hex literal {text}"))
}
