//! Deterministic scan-cycle simulator for ASL.
//!
//! Priority-2 opcode set: GPIO, PWM, ADC, UART/serial, I2C/SPI config,
//! MAP_RANGE, and IEC FBs (TON deepened, TOF, TP, CTU/CTD/CTUD, R_TRIG/F_TRIG).
//! Visual SVG binding is deferred; this crate emits logical traces only.

use std::collections::{HashMap, VecDeque};
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
    PwmDuty { pin: u32, duty: u32 },
    PwmFreq { pin: u32, hz: u32 },
    AdcRead { pin: u32, value: u32 },
    SerialOut { line: String },
    BusCfg { bus: String, detail: String },
    Wait { duration_ms: u64 },
    TonUpdate {
        instance: String,
        q: bool,
        et_ms: u64,
    },
    TofUpdate {
        instance: String,
        q: bool,
        et_ms: u64,
    },
    TpUpdate {
        instance: String,
        q: bool,
        et_ms: u64,
    },
    EdgeTrig {
        kind: String,
        instance: String,
        q: bool,
    },
    CounterUpdate {
        kind: String,
        instance: String,
        q: bool,
        cv: i64,
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
            TraceKind::PwmDuty { pin, duty } => {
                format!("t={:>6} PWM_DUTY pin={pin} duty={duty}", self.time_ms)
            }
            TraceKind::PwmFreq { pin, hz } => {
                format!("t={:>6} PWM_FREQ pin={pin} hz={hz}", self.time_ms)
            }
            TraceKind::AdcRead { pin, value } => {
                format!("t={:>6} ADC_READ pin={pin} value={value}", self.time_ms)
            }
            TraceKind::SerialOut { line } => {
                format!("t={:>6} SERIAL_OUT {line}", self.time_ms)
            }
            TraceKind::BusCfg { bus, detail } => {
                format!("t={:>6} BUS_CFG {bus} {detail}", self.time_ms)
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
            TraceKind::TofUpdate {
                instance,
                q,
                et_ms,
            } => format!(
                "t={:>6} TOF id={instance} Q={q} ET={et_ms}",
                self.time_ms
            ),
            TraceKind::TpUpdate {
                instance,
                q,
                et_ms,
            } => format!(
                "t={:>6} TP id={instance} Q={q} ET={et_ms}",
                self.time_ms
            ),
            TraceKind::EdgeTrig { kind, instance, q } => {
                format!(
                    "t={:>6} {kind} id={instance} Q={q}",
                    self.time_ms
                )
            }
            TraceKind::CounterUpdate {
                kind,
                instance,
                q,
                cv,
            } => format!(
                "t={:>6} {kind} id={instance} Q={q} CV={cv}",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimerKind {
    Ton,
    Tof,
    Tp,
}

#[derive(Debug, Clone)]
struct TimerState {
    kind: TimerKind,
    et_ms: u64,
    pt_ms: u64,
    q: bool,
    prev_in: bool,
    /// TOF off-delay / TP pulse running.
    running: bool,
}

#[derive(Debug, Clone)]
struct EdgeState {
    prev_clk: bool,
}

#[derive(Debug, Clone)]
struct CounterState {
    cv: i64,
    prev_cu: bool,
    prev_cd: bool,
}

#[derive(Debug, Clone, Default)]
struct PwmState {
    freq_hz: u32,
    duty: u32,
}

#[derive(Debug, Clone)]
pub struct Simulator {
    program: Program,
    vars: HashMap<String, Value>,
    pins: HashMap<u32, bool>,
    adc: HashMap<u32, u32>,
    pwm: HashMap<u32, PwmState>,
    serial_rx: VecDeque<String>,
    time_ms: u64,
    cycle: u64,
    setup_done: bool,
    timers: HashMap<String, TimerState>,
    edges: HashMap<String, EdgeState>,
    counters: HashMap<String, CounterState>,
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
    /// Optional initial ADC raw values (0–65535).
    pub initial_adc: HashMap<u32, u32>,
    /// Lines available to SERIAL_READLN / SERIAL_READ (FIFO).
    pub serial_rx: Vec<String>,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            max_cycles: 8,
            stmt_budget: 10_000,
            initial_pins: HashMap::new(),
            initial_adc: HashMap::new(),
            serial_rx: Vec::new(),
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
            adc: config.initial_adc.clone(),
            pwm: HashMap::new(),
            serial_rx: VecDeque::from(config.serial_rx.clone()),
            time_ms: 0,
            cycle: 0,
            setup_done: false,
            timers: HashMap::new(),
            edges: HashMap::new(),
            counters: HashMap::new(),
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
            "CFG_PWM_OUTPUT" => {
                let pin = self.eval(&args[0])?.as_u64()? as u32;
                let freq = self.eval(&args[1])?.as_u64()? as u32;
                let st = self.pwm.entry(pin).or_default();
                st.freq_hz = freq;
                self.push(TraceKind::PwmFreq { pin, hz: freq });
                self.push(TraceKind::BusCfg {
                    bus: "PWM".into(),
                    detail: format!("pin={pin} freq={freq}"),
                });
                Ok(Value::Void)
            }
            "SET_PWM_FREQ" => {
                let pin = self.eval(&args[0])?.as_u64()? as u32;
                let hz = self.eval(&args[1])?.as_u64()? as u32;
                self.pwm.entry(pin).or_default().freq_hz = hz;
                self.push(TraceKind::PwmFreq { pin, hz });
                Ok(Value::Void)
            }
            "SET_PWM_DUTY" => {
                let pin = self.eval(&args[0])?.as_u64()? as u32;
                let duty = self.eval(&args[1])?.as_u64()? as u32;
                self.pwm.entry(pin).or_default().duty = duty;
                self.push(TraceKind::PwmDuty { pin, duty });
                Ok(Value::Void)
            }
            "GET_PWM_DUTY" => {
                let pin = self.eval(&args[0])?.as_u64()? as u32;
                let duty = self.pwm.get(&pin).map(|s| s.duty).unwrap_or(0);
                Ok(Value::Uint(duty as u64))
            }
            "CFG_ADC_INPUT" => {
                let pin = self.eval(&args[0])?.as_u64()? as u32;
                self.adc.entry(pin).or_insert(0);
                self.push(TraceKind::BusCfg {
                    bus: "ADC".into(),
                    detail: format!("pin={pin}"),
                });
                Ok(Value::Void)
            }
            "READ_ADC" => {
                let pin = self.eval(&args[0])?.as_u64()? as u32;
                let value = *self.adc.get(&pin).unwrap_or(&0);
                self.push(TraceKind::AdcRead { pin, value });
                Ok(Value::Uint(value as u64))
            }
            "CFG_SERIAL" => {
                let baud = self.eval(&args[0])?.as_i64()?;
                self.push(TraceKind::BusCfg {
                    bus: "UART".into(),
                    detail: format!("baud={baud}"),
                });
                Ok(Value::Void)
            }
            "SERIAL_PRINT" | "SERIAL_PRINTLN" => {
                let msg = self.eval(&args[0])?.as_string()?;
                let line = if canon == "SERIAL_PRINTLN" {
                    format!("{msg}\\n")
                } else {
                    msg
                };
                self.push(TraceKind::SerialOut { line });
                Ok(Value::Void)
            }
            "SERIAL_READLN" => {
                let line = self.serial_rx.pop_front().unwrap_or_default();
                Ok(Value::String(line))
            }
            "SERIAL_READ" => {
                let line = self.serial_rx.front().cloned().unwrap_or_default();
                let (ch, rest) = if line.is_empty() {
                    (String::new(), String::new())
                } else {
                    let mut chars = line.chars();
                    let c = chars.next().unwrap().to_string();
                    (c, chars.collect())
                };
                if rest.is_empty() {
                    self.serial_rx.pop_front();
                } else if let Some(front) = self.serial_rx.front_mut() {
                    *front = rest;
                }
                Ok(Value::String(ch))
            }
            "SERIAL_AVAILABLE" => Ok(Value::Uint(
                self.serial_rx
                    .iter()
                    .map(|s| s.len() as u64)
                    .sum::<u64>()
                    .max(if self.serial_rx.is_empty() { 0 } else { 1 }),
            )),
            "SERIAL_FLUSH" => {
                self.serial_rx.clear();
                Ok(Value::Void)
            }
            "CFG_I2C" => {
                let bus = self.eval(&args[0])?.as_u64()?;
                let sda = self.eval(&args[1])?.as_u64()?;
                let scl = self.eval(&args[2])?.as_u64()?;
                let freq = self.eval(&args[3])?.as_i64()?;
                self.push(TraceKind::BusCfg {
                    bus: "I2C".into(),
                    detail: format!("bus={bus} sda={sda} scl={scl} freq={freq}"),
                });
                Ok(Value::Void)
            }
            "CFG_SENSOR_I2C" => {
                self.push(TraceKind::BusCfg {
                    bus: "I2C".into(),
                    detail: "sensor".into(),
                });
                Ok(Value::Void)
            }
            "CFG_SPI" => {
                let bus = self.eval(&args[0])?.as_u64()?;
                let mosi = self.eval(&args[1])?.as_u64()?;
                let miso = self.eval(&args[2])?.as_i64()?;
                let sck = self.eval(&args[3])?.as_u64()?;
                let cs = self.eval(&args[4])?.as_u64()?;
                self.push(TraceKind::BusCfg {
                    bus: "SPI".into(),
                    detail: format!("bus={bus} mosi={mosi} miso={miso} sck={sck} cs={cs}"),
                });
                Ok(Value::Void)
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
                self.advance_timers(ms);
                Ok(Value::Void)
            }
            "WAIT_TIME_US" => {
                // Deterministic ms-granularity sim: round up µs → ms (min 0).
                let us = match self.eval(&args[0])? {
                    Value::TimeMs(ms) => ms, // already ms if TIME literal misused
                    Value::Uint(u) => u,
                    Value::Int(i) => i.max(0) as u64,
                    other => return Err(format!("WAIT_TIME_US expects TIME/UINT, got {other:?}")),
                };
                let ms = us.div_ceil(1000);
                if ms > 0 {
                    self.push(TraceKind::Wait { duration_ms: ms });
                    self.time_ms = self.time_ms.saturating_add(ms);
                    self.advance_timers(ms);
                }
                Ok(Value::Void)
            }
            "TON" => self.eval_timer(TimerKind::Ton, args),
            "TOF" => self.eval_timer(TimerKind::Tof, args),
            "TP" => self.eval_timer(TimerKind::Tp, args),
            "R_TRIG" => self.eval_edge(true, args),
            "F_TRIG" => self.eval_edge(false, args),
            "CTU" => self.eval_ctu(args),
            "CTD" => self.eval_ctd(args),
            "CTUD" => self.eval_ctud(args),
            "MAP_RANGE" => {
                if args.len() != 5 {
                    return Err(format!("MAP_RANGE expects 5 args, got {}", args.len()));
                }
                let x = self.eval(&args[0])?.as_f64()?;
                let in_min = self.eval(&args[1])?.as_f64()?;
                let in_max = self.eval(&args[2])?.as_f64()?;
                let out_min = self.eval(&args[3])?.as_f64()?;
                let out_max = self.eval(&args[4])?.as_f64()?;
                let den = in_max - in_min;
                if den == 0.0 {
                    return Err("MAP_RANGE zero input span".into());
                }
                let y = (x - in_min) * (out_max - out_min) / den + out_min;
                Ok(Value::Real(y))
            }
            "CLAMP" => {
                let x = self.eval(&args[0])?.as_i64()?;
                let lo = self.eval(&args[1])?.as_i64()?;
                let hi = self.eval(&args[2])?.as_i64()?;
                Ok(Value::Int(x.clamp(lo, hi)))
            }
            "REAL_TO_INT" | "TRUNC" => {
                Ok(Value::Int(self.eval(&args[0])?.as_f64()? as i64))
            }
            "INT_TO_REAL" => Ok(Value::Real(self.eval(&args[0])?.as_i64()? as f64)),
            "BOOL_TO_INT" => Ok(Value::Int(if self.eval(&args[0])?.as_bool()? {
                1
            } else {
                0
            })),
            "INT_TO_BOOL" => Ok(Value::Bool(self.eval(&args[0])?.as_i64()? != 0)),
            "MOD" => {
                let a = self.eval(&args[0])?.as_u64()?;
                let b = self.eval(&args[1])?.as_u64()?;
                if b == 0 {
                    return Err("MOD by zero".into());
                }
                Ok(Value::Uint(a % b))
            }
            "GET_SYS_TIME" => Ok(Value::Uint(self.time_ms)),
            "GET_SYS_TIME_US" => Ok(Value::Uint(self.time_ms.saturating_mul(1000))),
            other => Err(format!("unsupported call in sim MVP: {other}")),
        }
    }

    fn parse_time_arg(&mut self, expr: &Expr) -> Result<u64, String> {
        match self.eval(expr)? {
            Value::TimeMs(ms) => Ok(ms),
            Value::Uint(u) => Ok(u),
            Value::Int(i) => Ok(i.max(0) as u64),
            other => Err(format!("TIME arg expected, got {other:?}")),
        }
    }

    fn instance_and_rest<'a>(
        &mut self,
        args: &'a [Expr],
        min_inputs: usize,
        default_id: &str,
    ) -> Result<(String, &'a [Expr]), String> {
        if args.len() == min_inputs {
            Ok((default_id.to_string(), args))
        } else if args.len() == min_inputs + 1 {
            let id = match self.eval(&args[0])? {
                Value::String(s) => s,
                other => return Err(format!("FB instance id must be string, got {other:?}")),
            };
            Ok((id, &args[1..]))
        } else {
            Err(format!(
                "FB expects {min_inputs} or {} args, got {}",
                min_inputs + 1,
                args.len()
            ))
        }
    }

    /// TON / TOF / TP with IEC-aligned ET clamp and WAIT_TIME time base.
    fn eval_timer(&mut self, kind: TimerKind, args: &[Expr]) -> Result<Value, String> {
        let default = match kind {
            TimerKind::Ton => "__ton0",
            TimerKind::Tof => "__tof0",
            TimerKind::Tp => "__tp0",
        };
        let (instance, rest) = self.instance_and_rest(args, 2, default)?;
        let inn = self.eval(&rest[0])?.as_bool()?;
        let pt = self.parse_time_arg(&rest[1])?;

        {
            let st = self.timers.entry(instance.clone()).or_insert(TimerState {
                kind,
                et_ms: 0,
                pt_ms: pt,
                q: false,
                prev_in: false,
                running: false,
            });
            st.kind = kind;
            st.pt_ms = pt;

            match kind {
                TimerKind::Ton => {
                    if !inn {
                        st.et_ms = 0;
                        st.q = false;
                        st.running = false;
                    } else if !st.prev_in {
                        // Rising edge — restart ET; elapsed advances on WAIT_TIME.
                        st.et_ms = 0;
                        st.q = false;
                        st.running = true;
                    }
                    if inn && st.et_ms >= st.pt_ms {
                        st.q = true;
                        st.et_ms = st.pt_ms; // deepen: clamp ET at PT
                    }
                    st.prev_in = inn;
                }
                TimerKind::Tof => {
                    if inn {
                        st.q = true;
                        st.et_ms = 0;
                        st.running = false;
                    } else if st.prev_in {
                        // Falling edge — start off-delay.
                        st.running = true;
                        st.et_ms = 0;
                        st.q = true;
                    } else if st.running && st.et_ms >= st.pt_ms {
                        st.q = false;
                        st.running = false;
                        st.et_ms = st.pt_ms;
                    }
                    st.prev_in = inn;
                }
                TimerKind::Tp => {
                    if inn && !st.prev_in && !st.running {
                        st.running = true;
                        st.et_ms = 0;
                        st.q = true;
                    }
                    if st.running && st.et_ms >= st.pt_ms {
                        st.q = false;
                        st.running = false;
                        st.et_ms = st.pt_ms;
                    }
                    st.prev_in = inn;
                }
            }
        }

        let (q, et) = self
            .timers
            .get(&instance)
            .map(|s| (s.q, s.et_ms))
            .unwrap_or((false, 0));
        // Mirror ET into `{instance}_ET` when that VAR exists.
        let et_key = format!("{instance}_ET");
        if self.vars.contains_key(&et_key) {
            self.vars.insert(et_key, Value::TimeMs(et));
        }
        match kind {
            TimerKind::Ton => self.push(TraceKind::TonUpdate {
                instance,
                q,
                et_ms: et,
            }),
            TimerKind::Tof => self.push(TraceKind::TofUpdate {
                instance,
                q,
                et_ms: et,
            }),
            TimerKind::Tp => self.push(TraceKind::TpUpdate {
                instance,
                q,
                et_ms: et,
            }),
        }
        Ok(Value::Bool(q))
    }

    fn eval_edge(&mut self, rising: bool, args: &[Expr]) -> Result<Value, String> {
        let default = if rising { "__rtrig0" } else { "__ftrig0" };
        let (instance, rest) = self.instance_and_rest(args, 1, default)?;
        let clk = self.eval(&rest[0])?.as_bool()?;
        let st = self
            .edges
            .entry(instance.clone())
            .or_insert(EdgeState { prev_clk: false });
        let q = if rising {
            clk && !st.prev_clk
        } else {
            !clk && st.prev_clk
        };
        st.prev_clk = clk;
        let kind = if rising { "R_TRIG" } else { "F_TRIG" };
        self.push(TraceKind::EdgeTrig {
            kind: kind.into(),
            instance,
            q,
        });
        Ok(Value::Bool(q))
    }

    fn mirror_cv(&mut self, instance: &str, cv: i64) {
        let key = format!("{instance}_CV");
        if self.vars.contains_key(&key) {
            self.vars.insert(key, Value::Int(cv));
        }
    }

    fn eval_ctu(&mut self, args: &[Expr]) -> Result<Value, String> {
        let (instance, rest) = self.instance_and_rest(args, 3, "__ctu0")?;
        let cu = self.eval(&rest[0])?.as_bool()?;
        let r = self.eval(&rest[1])?.as_bool()?;
        let pv = self.eval(&rest[2])?.as_i64()?;
        let st = self.counters.entry(instance.clone()).or_insert(CounterState {
            cv: 0,
            prev_cu: false,
            prev_cd: false,
        });
        if r {
            st.cv = 0;
        } else if cu && !st.prev_cu {
            st.cv = st.cv.saturating_add(1);
        }
        st.prev_cu = cu;
        let q = st.cv >= pv;
        let cv = st.cv;
        self.mirror_cv(&instance, cv);
        self.push(TraceKind::CounterUpdate {
            kind: "CTU".into(),
            instance,
            q,
            cv,
        });
        Ok(Value::Bool(q))
    }

    fn eval_ctd(&mut self, args: &[Expr]) -> Result<Value, String> {
        let (instance, rest) = self.instance_and_rest(args, 3, "__ctd0")?;
        let cd = self.eval(&rest[0])?.as_bool()?;
        let ld = self.eval(&rest[1])?.as_bool()?;
        let pv = self.eval(&rest[2])?.as_i64()?;
        let st = self.counters.entry(instance.clone()).or_insert(CounterState {
            cv: pv,
            prev_cu: false,
            prev_cd: false,
        });
        if ld {
            st.cv = pv;
        } else if cd && !st.prev_cd {
            st.cv = st.cv.saturating_sub(1);
        }
        st.prev_cd = cd;
        let q = st.cv <= 0;
        let cv = st.cv;
        self.mirror_cv(&instance, cv);
        self.push(TraceKind::CounterUpdate {
            kind: "CTD".into(),
            instance,
            q,
            cv,
        });
        Ok(Value::Bool(q))
    }

    fn eval_ctud(&mut self, args: &[Expr]) -> Result<Value, String> {
        let (instance, rest) = self.instance_and_rest(args, 5, "__ctud0")?;
        let cu = self.eval(&rest[0])?.as_bool()?;
        let cd = self.eval(&rest[1])?.as_bool()?;
        let r = self.eval(&rest[2])?.as_bool()?;
        let ld = self.eval(&rest[3])?.as_bool()?;
        let pv = self.eval(&rest[4])?.as_i64()?;
        let st = self.counters.entry(instance.clone()).or_insert(CounterState {
            cv: 0,
            prev_cu: false,
            prev_cd: false,
        });
        if r {
            st.cv = 0;
        } else if ld {
            st.cv = pv;
        } else {
            if cu && !st.prev_cu && !(cd && !st.prev_cd) {
                st.cv = st.cv.saturating_add(1);
            } else if cd && !st.prev_cd && !(cu && !st.prev_cu) {
                st.cv = st.cv.saturating_sub(1);
            }
            // Simultaneous CU+CD edges: IEC leaves CV unchanged.
        }
        st.prev_cu = cu;
        st.prev_cd = cd;
        let qu = st.cv >= pv;
        let qd = st.cv <= 0;
        let cv = st.cv;
        self.mirror_cv(&instance, cv);
        let qd_key = format!("{instance}_QD");
        if self.vars.contains_key(&qd_key) {
            self.vars.insert(qd_key, Value::Bool(qd));
        }
        self.push(TraceKind::CounterUpdate {
            kind: "CTUD".into(),
            instance,
            q: qu,
            cv,
        });
        Ok(Value::Bool(qu))
    }

    fn advance_timers(&mut self, ms: u64) {
        for st in self.timers.values_mut() {
            match st.kind {
                TimerKind::Ton => {
                    if st.prev_in {
                        st.et_ms = st.et_ms.saturating_add(ms).min(st.pt_ms);
                        if st.et_ms >= st.pt_ms {
                            st.q = true;
                        }
                    }
                }
                TimerKind::Tof => {
                    if st.running && !st.prev_in {
                        st.et_ms = st.et_ms.saturating_add(ms).min(st.pt_ms);
                        if st.et_ms >= st.pt_ms {
                            st.q = false;
                            st.running = false;
                        }
                    }
                }
                TimerKind::Tp => {
                    if st.running {
                        st.et_ms = st.et_ms.saturating_add(ms).min(st.pt_ms);
                        if st.et_ms >= st.pt_ms {
                            st.q = false;
                            st.running = false;
                        }
                    }
                }
            }
        }
    }
}

fn eval_binop(op: BinaryOp, l: Value, r: Value) -> Result<Value, String> {
    match op {
        BinaryOp::Add => {
            if matches!((&l, &r), (Value::Real(_), _) | (_, Value::Real(_))) {
                Ok(Value::Real(l.as_f64()? + r.as_f64()?))
            } else {
                Ok(Value::Int(l.as_i64()?.saturating_add(r.as_i64()?)))
            }
        }
        BinaryOp::Sub => {
            if matches!((&l, &r), (Value::Real(_), _) | (_, Value::Real(_))) {
                Ok(Value::Real(l.as_f64()? - r.as_f64()?))
            } else {
                Ok(Value::Int(l.as_i64()?.saturating_sub(r.as_i64()?)))
            }
        }
        BinaryOp::Mul => {
            if matches!((&l, &r), (Value::Real(_), _) | (_, Value::Real(_))) {
                Ok(Value::Real(l.as_f64()? * r.as_f64()?))
            } else {
                Ok(Value::Int(l.as_i64()?.saturating_mul(r.as_i64()?)))
            }
        }
        BinaryOp::Div => {
            if matches!((&l, &r), (Value::Real(_), _) | (_, Value::Real(_))) {
                let d = r.as_f64()?;
                if d == 0.0 {
                    return Err("div by zero".into());
                }
                Ok(Value::Real(l.as_f64()? / d))
            } else {
                let d = r.as_i64()?;
                if d == 0 {
                    return Err("div by zero".into());
                }
                Ok(Value::Int(l.as_i64()? / d))
            }
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
