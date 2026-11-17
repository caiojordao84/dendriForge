//! Deterministic scan-cycle simulator for ASL.
//!
//! Opcode set: GPIO, PWM, ADC, UART/serial, I2C/SPI config **and** transfers,
//! string ops (`CONCAT` / `TO_STRING*`), math (`SQRT` / `LOG` / `IN_RANGE`),
//! interrupts (`CFG_ATTACH_INTERRUPT` / `CFG_DETACH_INTERRUPT`, RISING/FALLING/CHANGE),
//! plant physics (`CFG_PLANT_TANK` / `CFG_PLANT_THERMAL` synced to scan),
//! display stubs (`LCD_*` / `OLED_*` / `TFT_*` / `PLAY_TONE`),
//! `SYS_*_RETAIN` + soft-restart persistence, MAP_RANGE, and IEC FBs
//! (TON/TOF/TP, CTU/CTD/CTUD, R_TRIG/F_TRIG).
//! Interactive `step` / pin APIs feed the live SVG canvas (`dendri-canvas`);
//! this crate still emits logical traces for CI snapshots.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;

use dendri_ast::expr::{BinaryOp, Expr, UnaryOp};
use dendri_ast::lit::Literal;
use dendri_ast::stmt::Stmt;
use dendri_ast::{Program, Scope, SimulationProject};
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
    BusXfer { bus: String, detail: String },
    RetainWrite { name: String, value: String },
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
    IsrAttach {
        pin: u32,
        mode: String,
        handler: String,
    },
    IsrDetach { pin: u32 },
    IsrFire {
        pin: u32,
        mode: String,
        handler: String,
        level: bool,
    },
    Display {
        device: String,
        detail: String,
    },
    Plant {
        kind: String,
        detail: String,
    },
    Tone {
        pin: u32,
        freq: u32,
        duration_ms: u64,
    },
    Halt { reason: String },
}

/// Edge trigger mode for `CFG_ATTACH_INTERRUPT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeMode {
    Rising,
    Falling,
    Change,
}

impl EdgeMode {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_uppercase().as_str() {
            "RISING" => Ok(Self::Rising),
            "FALLING" => Ok(Self::Falling),
            "CHANGE" => Ok(Self::Change),
            other => Err(format!("unknown EDGE mode {other}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rising => "RISING",
            Self::Falling => "FALLING",
            Self::Change => "CHANGE",
        }
    }

    fn matches(self, prev: bool, next: bool) -> bool {
        if prev == next {
            return false;
        }
        match self {
            Self::Rising => !prev && next,
            Self::Falling => prev && !next,
            Self::Change => true,
        }
    }
}

#[derive(Debug, Clone)]
struct InterruptAttach {
    handler: String,
    mode: EdgeMode,
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
            TraceKind::BusXfer { bus, detail } => {
                format!("t={:>6} BUS_XFER {bus} {detail}", self.time_ms)
            }
            TraceKind::RetainWrite { name, value } => {
                format!("t={:>6} RETAIN_WRITE {name}={value}", self.time_ms)
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
            TraceKind::IsrAttach { pin, mode, handler } => {
                format!(
                    "t={:>6} ISR_ATTACH pin={pin} mode={mode} handler={handler}",
                    self.time_ms
                )
            }
            TraceKind::IsrDetach { pin } => {
                format!("t={:>6} ISR_DETACH pin={pin}", self.time_ms)
            }
            TraceKind::IsrFire {
                pin,
                mode,
                handler,
                level,
            } => format!(
                "t={:>6} ISR_FIRE pin={pin} mode={mode} handler={handler} level={level}",
                self.time_ms
            ),
            TraceKind::Display { device, detail } => {
                format!("t={:>6} DISP {device} {detail}", self.time_ms)
            }
            TraceKind::Plant { kind, detail } => {
                format!("t={:>6} PLANT {kind} {detail}", self.time_ms)
            }
            TraceKind::Tone {
                pin,
                freq,
                duration_ms,
            } => format!(
                "t={:>6} TONE pin={pin} freq={freq} ms={duration_ms}",
                self.time_ms
            ),
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

/// External reservoir: level (cm) advances each scan from pump PWM + valve DO.
#[derive(Debug, Clone)]
struct PlantTank {
    height_cm: f64,
    pump_pin: u32,
    valve_pin: u32,
    fill_cm_per_s: f64,
    drain_cm_per_s: f64,
    level_cm: f64,
}

/// First-order thermal plant driven by heater PWM duty.
#[derive(Debug, Clone)]
struct PlantThermal {
    ambient_c: f64,
    heater_pin: u32,
    heat_c_per_s: f64,
    cool_c_per_s: f64,
    temp_c: f64,
}

#[derive(Debug, Clone, Default)]
struct LcdState {
    #[allow(dead_code)]
    cols: u32,
    #[allow(dead_code)]
    rows: u32,
    cells: HashMap<(u32, u32), String>,
}

#[derive(Debug, Clone, Default)]
struct OledState {
    cursor_x: u32,
    cursor_y: u32,
    fg: u32,
    bg: u32,
}

#[derive(Debug, Clone, Default)]
struct TftState {
    text_size: u32,
    fg: u32,
    bg: u32,
}

#[derive(Debug, Clone)]
pub struct Simulator {
    program: Program,
    vars: HashMap<String, Value>,
    /// Data-var scope index (name → scope) for RETAIN/PERSISTENT persistence.
    scopes: HashMap<String, Scope>,
    pins: HashMap<u32, bool>,
    adc: HashMap<u32, u32>,
    pwm: HashMap<u32, PwmState>,
    serial_rx: VecDeque<String>,
    /// Non-volatile image for RETAIN / PERSISTENT (`SYS_*_RETAIN` + soft restart).
    nv_store: HashMap<String, Value>,
    /// Virtual I2C device registers: (bus, addr) → byte.
    i2c_mem: HashMap<(u64, u64), u64>,
    /// Virtual SPI per-bus last/device byte.
    spi_mem: HashMap<u64, u64>,
    time_ms: u64,
    cycle: u64,
    setup_done: bool,
    timers: HashMap<String, TimerState>,
    edges: HashMap<String, EdgeState>,
    counters: HashMap<String, CounterState>,
    functions: HashMap<String, (Vec<String>, Vec<Stmt>)>,
    /// Pin → attached ISR (external edge via `set_pin`).
    interrupts: HashMap<u32, InterruptAttach>,
    /// Vars written inside `isr_*` bodies (volatile for codegen / honesty).
    volatile_vars: HashSet<String>,
    /// True while an ISR body is executing (shared-state writes are volatile).
    in_isr: bool,
    plant_tank: Option<PlantTank>,
    plant_thermal: Option<PlantThermal>,
    /// Accumulated WAIT_TIME during current scan (drives plant dt).
    scan_dt_ms: u64,
    lcd: Option<LcdState>,
    oled: Option<OledState>,
    tft: Option<TftState>,
    pub trace: Vec<TraceEvent>,
    /// Max statements per scan to avoid runaway while/for.
    stmt_budget: usize,
    initial_pins: HashMap<u32, bool>,
    initial_adc: HashMap<u32, u32>,
    initial_serial_rx: Vec<String>,
    initial_stmt_budget: usize,
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
    /// Seed non-volatile RETAIN/PERSISTENT image (simulates prior power cycle).
    pub retain_seed: HashMap<String, Value>,
    /// Seed I2C device registers as `(bus, addr) → byte`.
    pub i2c_seed: HashMap<(u64, u64), u64>,
    /// Seed SPI device bytes as `bus → byte`.
    pub spi_seed: HashMap<u64, u64>,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            max_cycles: 8,
            stmt_budget: 10_000,
            initial_pins: HashMap::new(),
            initial_adc: HashMap::new(),
            serial_rx: Vec::new(),
            retain_seed: HashMap::new(),
            i2c_seed: HashMap::new(),
            spi_seed: HashMap::new(),
        }
    }
}

impl Simulator {
    pub fn from_program(program: Program, config: &SimConfig) -> Self {
        let mut vars = HashMap::new();
        let mut scopes = HashMap::new();
        for d in &program.data {
            let v = Value::parse_init(&d.init, d.data_type);
            vars.insert(d.name.clone(), v);
            scopes.insert(d.name.clone(), d.scope);
        }
        let mut functions = HashMap::new();
        for f in &program.functions {
            functions.insert(f.name.clone(), (f.params.clone(), f.body.clone()));
        }
        let mut pins = config.initial_pins.clone();
        for hw in &program.hardware {
            pins.entry(hw.pin).or_insert(false);
        }
        // Apply retain seed over init values (prior power-cycle image).
        let mut nv_store = config.retain_seed.clone();
        for (k, v) in &nv_store {
            if matches!(scopes.get(k), Some(Scope::Retain | Scope::Persistent)) {
                vars.insert(k.clone(), v.clone());
            }
        }
        // Also snapshot current retain/persistent into nv (init or seeded).
        for (name, scope) in &scopes {
            if matches!(scope, Scope::Retain | Scope::Persistent) {
                if let Some(v) = vars.get(name) {
                    nv_store.entry(name.clone()).or_insert_with(|| v.clone());
                }
            }
        }
        Self {
            program,
            vars,
            scopes,
            pins,
            adc: config.initial_adc.clone(),
            pwm: HashMap::new(),
            serial_rx: VecDeque::from(config.serial_rx.clone()),
            nv_store,
            i2c_mem: config.i2c_seed.clone(),
            spi_mem: config.spi_seed.clone(),
            time_ms: 0,
            cycle: 0,
            setup_done: false,
            timers: HashMap::new(),
            edges: HashMap::new(),
            counters: HashMap::new(),
            functions,
            interrupts: HashMap::new(),
            volatile_vars: HashSet::new(),
            in_isr: false,
            plant_tank: None,
            plant_thermal: None,
            scan_dt_ms: 0,
            lcd: None,
            oled: None,
            tft: None,
            trace: Vec::new(),
            stmt_budget: config.stmt_budget,
            initial_pins: config.initial_pins.clone(),
            initial_adc: config.initial_adc.clone(),
            initial_serial_rx: config.serial_rx.clone(),
            initial_stmt_budget: config.stmt_budget,
        }
    }

    pub fn from_project(project: &SimulationProject, config: &SimConfig) -> Self {
        Self::from_program(project.asl.clone(), config)
    }

    /// Snapshot of non-volatile RETAIN/PERSISTENT image.
    pub fn retain_image(&self) -> HashMap<String, Value> {
        self.nv_store.clone()
    }

    /// Load a non-volatile image (does not re-run setup).
    pub fn load_retain_image(&mut self, image: HashMap<String, Value>) {
        for (k, v) in image {
            if matches!(self.scopes.get(&k), Some(Scope::Retain | Scope::Persistent)) {
                self.vars.insert(k.clone(), v.clone());
            }
            self.nv_store.insert(k, v);
        }
    }

    pub fn var(&self, name: &str) -> Option<&Value> {
        self.vars.get(name)
    }

    /// Soft power-cycle: keep RETAIN/PERSISTENT (and bus device memory), reset
    /// volatile state, clear trace, and re-run `setup`.
    pub fn soft_restart(&mut self) -> Result<(), String> {
        self.sync_nv_from_vars();
        let nv = self.nv_store.clone();
        let i2c = self.i2c_mem.clone();
        let spi = self.spi_mem.clone();
        let program = self.program.clone();
        let functions = self.functions.clone();
        let scopes = self.scopes.clone();
        let initial_pins = self.initial_pins.clone();
        let initial_adc = self.initial_adc.clone();
        let initial_serial_rx = self.initial_serial_rx.clone();
        let budget = self.initial_stmt_budget;

        let mut vars = HashMap::new();
        for d in &program.data {
            let v = Value::parse_init(&d.init, d.data_type);
            vars.insert(d.name.clone(), v);
        }
        for (k, v) in &nv {
            if matches!(scopes.get(k), Some(Scope::Retain | Scope::Persistent)) {
                vars.insert(k.clone(), v.clone());
            }
        }
        let mut pins = initial_pins.clone();
        for hw in &program.hardware {
            pins.entry(hw.pin).or_insert(false);
        }

        self.vars = vars;
        self.pins = pins;
        self.adc = initial_adc;
        self.pwm.clear();
        self.serial_rx = VecDeque::from(initial_serial_rx);
        self.nv_store = nv;
        self.i2c_mem = i2c;
        self.spi_mem = spi;
        self.time_ms = 0;
        self.cycle = 0;
        self.setup_done = false;
        self.timers.clear();
        self.edges.clear();
        self.counters.clear();
        self.interrupts.clear();
        self.volatile_vars.clear();
        self.in_isr = false;
        self.plant_tank = None;
        self.plant_thermal = None;
        self.scan_dt_ms = 0;
        self.lcd = None;
        self.oled = None;
        self.tft = None;
        self.functions = functions;
        self.scopes = scopes;
        self.trace.clear();
        self.stmt_budget = budget;
        self.ensure_setup()
    }

    fn sync_nv_from_vars(&mut self) {
        for (name, scope) in &self.scopes {
            if matches!(scope, Scope::Retain | Scope::Persistent) {
                if let Some(v) = self.vars.get(name) {
                    self.nv_store.insert(name.clone(), v.clone());
                }
            }
        }
    }

    fn is_nv_scope(&self, name: &str) -> bool {
        matches!(
            self.scopes.get(name),
            Some(Scope::Retain | Scope::Persistent)
        )
    }

    fn resolve_var_ref(&mut self, expr: &Expr) -> Result<String, String> {
        match expr {
            Expr::String { value } => Ok(value.clone()),
            Expr::Var { name } => Ok(name.clone()),
            Expr::Lit {
                value: Literal::String { value },
            } => Ok(value.clone()),
            other => {
                // Allow expr_text / calls that evaluate to a string name.
                match self.eval(other)? {
                    Value::String(s) => Ok(s),
                    v => Err(format!("VAR_REF expected string/name, got {v:?}")),
                }
            }
        }
    }

    fn push(&mut self, kind: TraceKind) {
        self.trace.push(TraceEvent {
            time_ms: self.time_ms,
            kind,
        });
    }

    pub fn run(&mut self, max_cycles: u64) -> Result<(), String> {
        self.ensure_setup()?;
        while self.cycle < max_cycles {
            self.step()?;
        }
        Ok(())
    }

    /// Run setup once (emits Boot + SetupDone on first call).
    pub fn ensure_setup(&mut self) -> Result<(), String> {
        if self.setup_done {
            return Ok(());
        }
        self.push(TraceKind::Boot);
        self.exec_block_top(&self.program.setup.clone())?;
        self.setup_done = true;
        self.push(TraceKind::SetupDone);
        Ok(())
    }

    /// Execute one scan cycle (after setup). Deterministic for a fixed pin map.
    pub fn step(&mut self) -> Result<(), String> {
        self.ensure_setup()?;
        self.cycle += 1;
        self.scan_dt_ms = 0;
        self.push(TraceKind::ScanStart { cycle: self.cycle });
        self.exec_block_top(&self.program.loop_body.clone())?;
        self.advance_plants()?;
        self.push(TraceKind::ScanEnd { cycle: self.cycle });
        Ok(())
    }

    /// Current tank level (cm) when `CFG_PLANT_TANK` is active.
    pub fn plant_level(&self) -> Option<f64> {
        self.plant_tank.as_ref().map(|t| t.level_cm)
    }

    /// Current thermal plant temperature (°C) when configured.
    pub fn plant_temp(&self) -> Option<f64> {
        self.plant_thermal.as_ref().map(|t| t.temp_c)
    }

    /// Integrate external plant models using scan elapsed time (WAIT_TIME sum).
    /// Default dt = 100 ms when the loop did not call WAIT_TIME.
    fn advance_plants(&mut self) -> Result<(), String> {
        let dt_ms = if self.scan_dt_ms == 0 {
            100
        } else {
            self.scan_dt_ms
        };
        let dt_s = dt_ms as f64 / 1000.0;

        let tank_trace = if let Some(tank) = self.plant_tank.as_mut() {
            let duty = self
                .pwm
                .get(&tank.pump_pin)
                .map(|p| p.duty)
                .unwrap_or(0) as f64;
            let valve_open = *self.pins.get(&tank.valve_pin).unwrap_or(&false);
            let inflow = tank.fill_cm_per_s * (duty / 65535.0) * dt_s;
            let outflow = if valve_open {
                tank.drain_cm_per_s * dt_s
            } else {
                0.0
            };
            tank.level_cm = (tank.level_cm + inflow - outflow).clamp(0.0, tank.height_cm);
            Some(format!(
                "level={:.2} height={:.2} dt_ms={dt_ms}",
                tank.level_cm, tank.height_cm
            ))
        } else {
            None
        };
        if let Some(detail) = tank_trace {
            self.push(TraceKind::Plant {
                kind: "TANK".into(),
                detail,
            });
        }

        let thermal_trace = if let Some(th) = self.plant_thermal.as_mut() {
            let duty = self
                .pwm
                .get(&th.heater_pin)
                .map(|p| p.duty)
                .unwrap_or(0) as f64;
            let heat = th.heat_c_per_s * (duty / 65535.0) * dt_s;
            let cool = th.cool_c_per_s * (th.temp_c - th.ambient_c) * dt_s;
            th.temp_c += heat - cool;
            Some(format!("temp={:.3} dt_ms={dt_ms}", th.temp_c))
        } else {
            None
        };
        if let Some(detail) = thermal_trace {
            self.push(TraceKind::Plant {
                kind: "THERMAL".into(),
                detail,
            });
        }
        Ok(())
    }

    pub fn pin(&self, pin: u32) -> bool {
        *self.pins.get(&pin).unwrap_or(&false)
    }

    /// Vars written from ISR bodies (sim-visible volatile set).
    pub fn volatile_vars(&self) -> &HashSet<String> {
        &self.volatile_vars
    }

    /// Attached interrupt map: pin → (handler, mode string).
    pub fn interrupts(&self) -> Vec<(u32, String, String)> {
        let mut out: Vec<_> = self
            .interrupts
            .iter()
            .map(|(pin, att)| (*pin, att.handler.clone(), att.mode.as_str().to_string()))
            .collect();
        out.sort_by_key(|(p, _, _)| *p);
        out
    }

    /// Drive an external input pin. Fires attached ISRs on matching edges.
    pub fn set_pin(&mut self, pin: u32, value: bool) -> Result<(), String> {
        let prev = *self.pins.get(&pin).unwrap_or(&false);
        self.pins.insert(pin, value);
        if prev != value {
            self.maybe_fire_isr(pin, prev, value)?;
        }
        Ok(())
    }

    fn maybe_fire_isr(&mut self, pin: u32, prev: bool, next: bool) -> Result<(), String> {
        let Some(att) = self.interrupts.get(&pin).cloned() else {
            return Ok(());
        };
        if !att.mode.matches(prev, next) {
            return Ok(());
        }
        self.push(TraceKind::IsrFire {
            pin,
            mode: att.mode.as_str().to_string(),
            handler: att.handler.clone(),
            level: next,
        });
        self.invoke_isr(&att.handler)
    }

    fn invoke_isr(&mut self, handler: &str) -> Result<(), String> {
        let Some((params, body)) = self.functions.get(handler).cloned() else {
            return Err(format!("ISR handler {handler} not found"));
        };
        if !params.is_empty() {
            return Err(format!("ISR {handler} must take zero params"));
        }
        let was = self.in_isr;
        self.in_isr = true;
        let result = self.exec_function_body(handler, &[], &body);
        self.in_isr = was;
        result.map(|_| ())
    }

    /// Execute a user function body; persists mutations to pre-existing globals.
    fn exec_function_body(
        &mut self,
        name: &str,
        args: &[Expr],
        body: &[Stmt],
    ) -> Result<Value, String> {
        let Some((params, _)) = self.functions.get(name).cloned() else {
            return Err(format!("function {name} not found"));
        };
        if params.len() != args.len() {
            return Err(format!(
                "arity mismatch calling {name}: {} vs {}",
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
        let ret = match self.exec_block(body)? {
            Some(v) => v,
            None => Value::Bool(false),
        };
        let mut merged = saved;
        for (k, v) in self.vars.drain() {
            if merged.contains_key(&k) {
                merged.insert(k, v);
            }
        }
        self.vars = merged;
        Ok(ret)
    }

    fn resolve_isr_ref(expr: &Expr) -> Result<String, String> {
        match expr {
            Expr::Var { name } => Ok(name.clone()),
            Expr::String { value } => Ok(value.clone()),
            Expr::Lit {
                value: Literal::String { value },
            } => Ok(value.clone()),
            other => Err(format!("ISR_REF expected var/string, got {other:?}")),
        }
    }

    fn collect_assigns(stmts: &[Stmt], out: &mut HashSet<String>) {
        for stmt in stmts {
            match stmt {
                Stmt::Assign { target, .. } => {
                    out.insert(target.clone());
                }
                Stmt::If {
                    then,
                    elif,
                    else_body,
                    ..
                } => {
                    Self::collect_assigns(then, out);
                    for arm in elif {
                        Self::collect_assigns(&arm.then, out);
                    }
                    if let Some(body) = else_body {
                        Self::collect_assigns(body, out);
                    }
                }
                Stmt::While { body, .. } | Stmt::For { body, .. } => {
                    Self::collect_assigns(body, out);
                }
                Stmt::Call { .. }
                | Stmt::Return { .. }
                | Stmt::Exit
                | Stmt::Continue
                | Stmt::VendorExtension { .. } => {}
            }
        }
    }

    pub fn pins(&self) -> &HashMap<u32, bool> {
        &self.pins
    }

    pub fn time_ms(&self) -> u64 {
        self.time_ms
    }

    pub fn cycle(&self) -> u64 {
        self.cycle
    }

    pub fn setup_done(&self) -> bool {
        self.setup_done
    }

    pub fn trace_text(&self) -> String {
        let mut s = String::new();
        for ev in &self.trace {
            let _ = writeln!(s, "{}", ev.to_line());
        }
        s
    }

    /// `Ok(Some(v))` = early `return` (possibly nested); `Ok(None)` = fall through.
    fn exec_block(&mut self, body: &[Stmt]) -> Result<Option<Value>, String> {
        for stmt in body {
            if let Some(v) = self.exec_stmt(stmt)? {
                return Ok(Some(v));
            }
        }
        Ok(None)
    }

    fn exec_stmt(&mut self, stmt: &Stmt) -> Result<Option<Value>, String> {
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
                if self.in_isr {
                    self.volatile_vars.insert(target.clone());
                }
                if self.is_nv_scope(target) {
                    self.nv_store.insert(target.clone(), v.clone());
                }
                self.vars.insert(target.clone(), v);
                Ok(None)
            }
            Stmt::Call { name, args } => {
                self.eval_call(name, args)?;
                Ok(None)
            }
            Stmt::If {
                condition,
                then,
                elif,
                else_body,
            } => {
                if self.eval(condition)?.as_bool()? {
                    self.exec_block(then)
                } else {
                    for arm in elif {
                        if self.eval(&arm.condition)?.as_bool()? {
                            return self.exec_block(&arm.then);
                        }
                    }
                    if let Some(body) = else_body {
                        self.exec_block(body)
                    } else {
                        Ok(None)
                    }
                }
            }
            Stmt::While { condition, body } => {
                let mut guard = 0u32;
                while self.eval(condition)?.as_bool()? {
                    if let Some(v) = self.exec_block(body)? {
                        return Ok(Some(v));
                    }
                    guard += 1;
                    if guard > 1000 {
                        return Err("while loop guard exceeded".into());
                    }
                }
                Ok(None)
            }
            Stmt::For { counter, to, body } => {
                let n = self.eval(to)?.as_u64()?;
                let cname = counter.clone().unwrap_or_else(|| "_i".into());
                for i in 0..n {
                    self.vars.insert(cname.clone(), Value::Uint(i));
                    if let Some(v) = self.exec_block(body)? {
                        return Ok(Some(v));
                    }
                }
                Ok(None)
            }
            Stmt::Return { value } => {
                let v = if let Some(expr) = value {
                    self.eval(expr)?
                } else {
                    Value::Bool(false)
                };
                Ok(Some(v))
            }
            Stmt::Exit => {
                self.push(TraceKind::Halt {
                    reason: "EXIT".into(),
                });
                Ok(None)
            }
            Stmt::Continue => Ok(None),
            Stmt::VendorExtension { reason } => {
                // Vendor hooks are skipped (not abortive) so SD/RFID/DMA stubs
                // can leave a graceful alternate path in the scan.
                self.push(TraceKind::BusCfg {
                    bus: "VENDOR".into(),
                    detail: format!("skip:{reason}"),
                });
                Ok(None)
            }
        }
    }

    fn exec_block_top(&mut self, body: &[Stmt]) -> Result<(), String> {
        if let Some(_) = self.exec_block(body)? {
            return Err("return outside function".into());
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
                    UnaryOp::Neg => match v {
                        Value::Real(r) => Ok(Value::Real(-r)),
                        other => Ok(Value::Int(-other.as_i64()?)),
                    },
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
        if let Some((_params, body)) = self.functions.get(canon).cloned() {
            return self.exec_function_body(canon, args, &body);
        }

        match canon {
            "CFG_DIGITAL_OUTPUT" | "CFG_DIGITAL_INPUT" | "CFG_DIGITAL_INPUT_PULLUP"
            | "CFG_DIGITAL_INPUT_PULLDOWN" => {
                let _pin = self.eval(&args[0])?.as_u64()? as u32;
                Ok(Value::Void)
            }
            "CFG_ATTACH_INTERRUPT" => {
                if args.len() != 3 {
                    return Err(format!(
                        "CFG_ATTACH_INTERRUPT expects 3 args, got {}",
                        args.len()
                    ));
                }
                let pin = self.eval(&args[0])?.as_u64()? as u32;
                let handler = Self::resolve_isr_ref(&args[1])?;
                if !self.functions.contains_key(&handler) {
                    return Err(format!("unresolved ISR reference: {handler}"));
                }
                let mode_s = self.eval(&args[2])?.as_string()?;
                let mode = EdgeMode::parse(&mode_s)?;
                // Static volatile set: assignment targets inside the ISR body.
                if let Some((_, body)) = self.functions.get(&handler).cloned() {
                    let mut written = HashSet::new();
                    Self::collect_assigns(&body, &mut written);
                    self.volatile_vars.extend(written);
                }
                self.interrupts.insert(
                    pin,
                    InterruptAttach {
                        handler: handler.clone(),
                        mode,
                    },
                );
                self.push(TraceKind::IsrAttach {
                    pin,
                    mode: mode.as_str().to_string(),
                    handler,
                });
                Ok(Value::Void)
            }
            "CFG_DETACH_INTERRUPT" => {
                if args.len() != 1 {
                    return Err(format!(
                        "CFG_DETACH_INTERRUPT expects 1 arg, got {}",
                        args.len()
                    ));
                }
                let pin = self.eval(&args[0])?.as_u64()? as u32;
                self.interrupts.remove(&pin);
                self.push(TraceKind::IsrDetach { pin });
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
            "I2C_WRITE" => {
                if args.len() != 3 {
                    return Err(format!("I2C_WRITE expects 3 args, got {}", args.len()));
                }
                let bus = self.eval(&args[0])?.as_u64()?;
                let addr = self.eval(&args[1])?.as_u64()? & 0xff;
                let data = self.eval(&args[2])?.as_u64()? & 0xff;
                self.i2c_mem.insert((bus, addr), data);
                self.push(TraceKind::BusXfer {
                    bus: "I2C".into(),
                    detail: format!("WRITE bus={bus} addr=0x{addr:02X} data=0x{data:02X}"),
                });
                Ok(Value::Void)
            }
            "I2C_READ" => {
                if args.len() != 2 {
                    return Err(format!("I2C_READ expects 2 args, got {}", args.len()));
                }
                let bus = self.eval(&args[0])?.as_u64()?;
                let addr = self.eval(&args[1])?.as_u64()? & 0xff;
                let data = *self.i2c_mem.get(&(bus, addr)).unwrap_or(&0);
                self.push(TraceKind::BusXfer {
                    bus: "I2C".into(),
                    detail: format!("READ bus={bus} addr=0x{addr:02X} data=0x{data:02X}"),
                });
                Ok(Value::Uint(data))
            }
            "SPI_TRANSFER" => {
                if args.len() != 2 {
                    return Err(format!("SPI_TRANSFER expects 2 args, got {}", args.len()));
                }
                let bus = self.eval(&args[0])?.as_u64()?;
                let data = self.eval(&args[1])?.as_u64()? & 0xff;
                let prev = *self.spi_mem.get(&bus).unwrap_or(&0);
                self.spi_mem.insert(bus, data);
                self.push(TraceKind::BusXfer {
                    bus: "SPI".into(),
                    detail: format!("XFER bus={bus} tx=0x{data:02X} rx=0x{prev:02X}"),
                });
                Ok(Value::Uint(prev))
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
                self.scan_dt_ms = self.scan_dt_ms.saturating_add(ms);
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
                    self.scan_dt_ms = self.scan_dt_ms.saturating_add(ms);
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
            "IN_RANGE" => {
                // IEC 61131-3: IN_RANGE(MN, IN, MX) → BOOL
                if args.len() != 3 {
                    return Err(format!("IN_RANGE expects 3 args, got {}", args.len()));
                }
                let mn = self.eval(&args[0])?.as_f64()?;
                let inn = self.eval(&args[1])?.as_f64()?;
                let mx = self.eval(&args[2])?.as_f64()?;
                Ok(Value::Bool(mn <= inn && inn <= mx))
            }
            "SQRT" => {
                let x = self.eval(&args[0])?.as_f64()?;
                if x < 0.0 {
                    return Err("SQRT of negative".into());
                }
                Ok(Value::Real(x.sqrt()))
            }
            "LOG" => {
                let x = self.eval(&args[0])?.as_f64()?;
                if x <= 0.0 {
                    return Err("LOG domain error".into());
                }
                Ok(Value::Real(x.log10()))
            }
            "LN" => {
                let x = self.eval(&args[0])?.as_f64()?;
                if x <= 0.0 {
                    return Err("LN domain error".into());
                }
                Ok(Value::Real(x.ln()))
            }
            "ABS" => {
                let v = self.eval(&args[0])?;
                match v {
                    Value::Real(r) => Ok(Value::Real(r.abs())),
                    Value::Int(i) => Ok(Value::Int(i.abs())),
                    Value::Uint(u) => Ok(Value::Uint(u)),
                    other => Err(format!("ABS expects numeric, got {other:?}")),
                }
            }
            "TO_STRING" | "ANY_TO_STRING" => {
                let v = self.eval(&args[0])?;
                Ok(Value::String(v.as_string()?))
            }
            "TO_STRING_FMT" => {
                if args.len() != 2 {
                    return Err(format!("TO_STRING_FMT expects 2 args, got {}", args.len()));
                }
                let x = self.eval(&args[0])?.as_f64()?;
                let d = self.eval(&args[1])?.as_u64()? as usize;
                Ok(Value::String(format!("{x:.d$}")))
            }
            "CONCAT" => {
                if args.is_empty() {
                    return Ok(Value::String(String::new()));
                }
                let mut out = String::new();
                for a in args {
                    out.push_str(&self.eval(a)?.as_string()?);
                }
                Ok(Value::String(out))
            }
            "SYS_LOAD_MEMORY_SECTOR" => {
                if args.is_empty() {
                    return Err("SYS_LOAD_MEMORY_SECTOR expects sector arg".into());
                }
                let sector = self.eval(&args[0])?.as_string()?;
                let want = match sector.as_str() {
                    "RETAIN_SECTOR" | "RETAIN" => Scope::Retain,
                    "PERSISTENT_SECTOR" | "PERSISTENT" => Scope::Persistent,
                    other => {
                        return Err(format!("unknown memory sector {other}"));
                    }
                };
                for (name, scope) in self.scopes.clone() {
                    if scope == want {
                        if let Some(v) = self.nv_store.get(&name).cloned() {
                            self.vars.insert(name, v);
                        }
                    }
                }
                self.push(TraceKind::BusCfg {
                    bus: "NV".into(),
                    detail: format!("LOAD {sector}"),
                });
                Ok(Value::Void)
            }
            "SYS_WRITE_RETAIN" => {
                if args.len() != 2 {
                    return Err(format!("SYS_WRITE_RETAIN expects 2 args, got {}", args.len()));
                }
                let name = self.resolve_var_ref(&args[0])?;
                let v = self.eval(&args[1])?;
                self.nv_store.insert(name.clone(), v.clone());
                self.vars.insert(name.clone(), v.clone());
                self.push(TraceKind::RetainWrite {
                    name,
                    value: v.to_trace(),
                });
                Ok(Value::Void)
            }
            "SYS_READ_RETAIN" => {
                if args.len() != 1 {
                    return Err(format!("SYS_READ_RETAIN expects 1 arg, got {}", args.len()));
                }
                let name = self.resolve_var_ref(&args[0])?;
                if let Some(v) = self.nv_store.get(&name).cloned() {
                    self.vars.insert(name, v.clone());
                    return Ok(v);
                }
                self.vars
                    .get(&name)
                    .cloned()
                    .ok_or_else(|| format!("SYS_READ_RETAIN unknown var {name}"))
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
            "TIME_DIFF" => {
                if args.len() != 2 {
                    return Err(format!("TIME_DIFF expects 2 args, got {}", args.len()));
                }
                let a = self.eval(&args[0])?.as_u64()?;
                let b = self.eval(&args[1])?.as_u64()?;
                Ok(Value::Uint(a.saturating_sub(b)))
            }
            "GET_SYS_TIME" => Ok(Value::Uint(self.time_ms)),
            "GET_SYS_TIME_US" => Ok(Value::Uint(self.time_ms.saturating_mul(1000))),
            "CFG_PLANT_TANK" => {
                if args.len() != 5 {
                    return Err(format!("CFG_PLANT_TANK expects 5 args, got {}", args.len()));
                }
                let height_cm = self.eval(&args[0])?.as_f64()?;
                let pump_pin = self.eval(&args[1])?.as_u64()? as u32;
                let valve_pin = self.eval(&args[2])?.as_u64()? as u32;
                let fill_cm_per_s = self.eval(&args[3])?.as_f64()?;
                let drain_cm_per_s = self.eval(&args[4])?.as_f64()?;
                self.plant_tank = Some(PlantTank {
                    height_cm,
                    pump_pin,
                    valve_pin,
                    fill_cm_per_s,
                    drain_cm_per_s,
                    level_cm: 0.0,
                });
                self.push(TraceKind::Plant {
                    kind: "CFG_TANK".into(),
                    detail: format!(
                        "height={height_cm} pump={pump_pin} valve={valve_pin} fill={fill_cm_per_s}/s drain={drain_cm_per_s}/s"
                    ),
                });
                Ok(Value::Void)
            }
            "PLANT_LEVEL" => {
                let level = self
                    .plant_tank
                    .as_ref()
                    .map(|t| t.level_cm)
                    .ok_or_else(|| "PLANT_LEVEL: tank not configured".to_string())?;
                Ok(Value::Real(level))
            }
            "READ_DISTANCE_CM" => {
                if args.len() != 2 {
                    return Err(format!("READ_DISTANCE_CM expects 2 args, got {}", args.len()));
                }
                let _trig = self.eval(&args[0])?.as_u64()?;
                let _echo = self.eval(&args[1])?.as_u64()?;
                let Some(tank) = self.plant_tank.as_ref() else {
                    return Ok(Value::Real(0.0));
                };
                Ok(Value::Real((tank.height_cm - tank.level_cm).max(0.0)))
            }
            "CFG_PLANT_THERMAL" => {
                if args.len() != 4 {
                    return Err(format!(
                        "CFG_PLANT_THERMAL expects 4 args, got {}",
                        args.len()
                    ));
                }
                let ambient_c = self.eval(&args[0])?.as_f64()?;
                let heater_pin = self.eval(&args[1])?.as_u64()? as u32;
                let heat_c_per_s = self.eval(&args[2])?.as_f64()?;
                let cool_c_per_s = self.eval(&args[3])?.as_f64()?;
                self.plant_thermal = Some(PlantThermal {
                    ambient_c,
                    heater_pin,
                    heat_c_per_s,
                    cool_c_per_s,
                    temp_c: ambient_c,
                });
                self.push(TraceKind::Plant {
                    kind: "CFG_THERMAL".into(),
                    detail: format!(
                        "ambient={ambient_c} heater={heater_pin} heat={heat_c_per_s}/s cool={cool_c_per_s}/s"
                    ),
                });
                Ok(Value::Void)
            }
            "PLANT_TEMP" | "READ_TEMPERATURE" => {
                if let Some(th) = self.plant_thermal.as_ref() {
                    Ok(Value::Real(th.temp_c))
                } else if canon == "READ_TEMPERATURE" {
                    Ok(Value::Real(25.0))
                } else {
                    Err("PLANT_TEMP: thermal plant not configured".into())
                }
            }
            "READ_HUMIDITY" => Ok(Value::Real(50.0)),
            "READ_PRESSURE" => Ok(Value::Real(1013.25)),
            "CFG_LCD" => {
                if args.len() != 3 {
                    return Err(format!("CFG_LCD expects 3 args, got {}", args.len()));
                }
                let addr = self.eval(&args[0])?.as_u64()?;
                let cols = self.eval(&args[1])?.as_u64()? as u32;
                let rows = self.eval(&args[2])?.as_u64()? as u32;
                self.lcd = Some(LcdState {
                    cols,
                    rows,
                    cells: HashMap::new(),
                });
                self.push(TraceKind::Display {
                    device: "LCD".into(),
                    detail: format!("CFG addr=0x{addr:02X} cols={cols} rows={rows}"),
                });
                Ok(Value::Void)
            }
            "LCD_CLEAR" => {
                if let Some(lcd) = self.lcd.as_mut() {
                    lcd.cells.clear();
                }
                self.push(TraceKind::Display {
                    device: "LCD".into(),
                    detail: "CLEAR".into(),
                });
                Ok(Value::Void)
            }
            "LCD_PRINT" => {
                if args.len() != 3 {
                    return Err(format!("LCD_PRINT expects 3 args, got {}", args.len()));
                }
                let col = self.eval(&args[0])?.as_u64()? as u32;
                let row = self.eval(&args[1])?.as_u64()? as u32;
                let msg = self.eval(&args[2])?.as_string()?;
                if let Some(lcd) = self.lcd.as_mut() {
                    lcd.cells.insert((col, row), msg.clone());
                }
                self.push(TraceKind::Display {
                    device: "LCD".into(),
                    detail: format!("PRINT col={col} row={row} \"{msg}\""),
                });
                Ok(Value::Void)
            }
            "CFG_OLED" => {
                let addr = if args.is_empty() {
                    0x3C
                } else {
                    self.eval(&args[0])?.as_u64()?
                };
                self.oled = Some(OledState::default());
                self.push(TraceKind::Display {
                    device: "OLED".into(),
                    detail: format!("CFG addr=0x{addr:02X}"),
                });
                Ok(Value::Void)
            }
            "OLED_CLEAR" => {
                if let Some(o) = self.oled.as_mut() {
                    o.cursor_x = 0;
                    o.cursor_y = 0;
                }
                self.push(TraceKind::Display {
                    device: "OLED".into(),
                    detail: "CLEAR".into(),
                });
                Ok(Value::Void)
            }
            "OLED_DISPLAY" => {
                self.push(TraceKind::Display {
                    device: "OLED".into(),
                    detail: "FLUSH".into(),
                });
                Ok(Value::Void)
            }
            "OLED_SET_CURSOR" => {
                if args.len() != 2 {
                    return Err(format!("OLED_SET_CURSOR expects 2 args, got {}", args.len()));
                }
                let x = self.eval(&args[0])?.as_u64()? as u32;
                let y = self.eval(&args[1])?.as_u64()? as u32;
                if let Some(o) = self.oled.as_mut() {
                    o.cursor_x = x;
                    o.cursor_y = y;
                }
                self.push(TraceKind::Display {
                    device: "OLED".into(),
                    detail: format!("CURSOR x={x} y={y}"),
                });
                Ok(Value::Void)
            }
            "OLED_SET_TEXT_COLOR" => {
                if args.len() != 2 {
                    return Err(format!(
                        "OLED_SET_TEXT_COLOR expects 2 args, got {}",
                        args.len()
                    ));
                }
                let fg = self.eval(&args[0])?.as_u64()? as u32;
                let bg = self.eval(&args[1])?.as_u64()? as u32;
                if let Some(o) = self.oled.as_mut() {
                    o.fg = fg;
                    o.bg = bg;
                }
                self.push(TraceKind::Display {
                    device: "OLED".into(),
                    detail: format!("TEXT_COLOR fg={fg} bg={bg}"),
                });
                Ok(Value::Void)
            }
            "OLED_PRINT" => {
                if args.len() != 1 {
                    return Err(format!("OLED_PRINT expects 1 arg, got {}", args.len()));
                }
                let msg = self.eval(&args[0])?.as_string()?;
                let (x, y) = self
                    .oled
                    .as_ref()
                    .map(|o| (o.cursor_x, o.cursor_y))
                    .unwrap_or((0, 0));
                self.push(TraceKind::Display {
                    device: "OLED".into(),
                    detail: format!("PRINT x={x} y={y} \"{msg}\""),
                });
                Ok(Value::Void)
            }
            "OLED_DRAW_PIXEL" => {
                if args.len() != 3 {
                    return Err(format!("OLED_DRAW_PIXEL expects 3 args, got {}", args.len()));
                }
                let x = self.eval(&args[0])?.as_u64()?;
                let y = self.eval(&args[1])?.as_u64()?;
                let color = self.eval(&args[2])?.as_u64()?;
                self.push(TraceKind::Display {
                    device: "OLED".into(),
                    detail: format!("PIXEL x={x} y={y} c={color}"),
                });
                Ok(Value::Void)
            }
            "OLED_DRAW_RECT" => {
                if args.len() != 5 {
                    return Err(format!("OLED_DRAW_RECT expects 5 args, got {}", args.len()));
                }
                let x = self.eval(&args[0])?.as_u64()?;
                let y = self.eval(&args[1])?.as_u64()?;
                let w = self.eval(&args[2])?.as_u64()?;
                let h = self.eval(&args[3])?.as_u64()?;
                let color = self.eval(&args[4])?.as_u64()?;
                self.push(TraceKind::Display {
                    device: "OLED".into(),
                    detail: format!("RECT x={x} y={y} w={w} h={h} c={color}"),
                });
                Ok(Value::Void)
            }
            "OLED_FILL_RECT" => {
                if args.len() != 5 {
                    return Err(format!("OLED_FILL_RECT expects 5 args, got {}", args.len()));
                }
                let x = self.eval(&args[0])?.as_u64()?;
                let y = self.eval(&args[1])?.as_u64()?;
                let w = self.eval(&args[2])?.as_u64()?;
                let h = self.eval(&args[3])?.as_u64()?;
                let color = self.eval(&args[4])?.as_u64()?;
                self.push(TraceKind::Display {
                    device: "OLED".into(),
                    detail: format!("FILL_RECT x={x} y={y} w={w} h={h} c={color}"),
                });
                Ok(Value::Void)
            }
            "OLED_DRAW_VLINE" => {
                if args.len() != 4 {
                    return Err(format!("OLED_DRAW_VLINE expects 4 args, got {}", args.len()));
                }
                let x = self.eval(&args[0])?.as_u64()?;
                let y = self.eval(&args[1])?.as_u64()?;
                let h = self.eval(&args[2])?.as_u64()?;
                let color = self.eval(&args[3])?.as_u64()?;
                self.push(TraceKind::Display {
                    device: "OLED".into(),
                    detail: format!("VLINE x={x} y={y} h={h} c={color}"),
                });
                Ok(Value::Void)
            }
            "CFG_TFT" => {
                if args.len() != 3 {
                    return Err(format!("CFG_TFT expects 3 args, got {}", args.len()));
                }
                let dc = self.eval(&args[0])?.as_u64()?;
                let rst = self.eval(&args[1])?.as_i64()?;
                let cs = self.eval(&args[2])?.as_i64()?;
                self.tft = Some(TftState {
                    text_size: 1,
                    fg: 0xFFFF,
                    bg: 0,
                });
                self.push(TraceKind::Display {
                    device: "TFT".into(),
                    detail: format!("CFG dc={dc} rst={rst} cs={cs}"),
                });
                Ok(Value::Void)
            }
            "TFT_FILL" => {
                if args.len() != 1 {
                    return Err(format!("TFT_FILL expects 1 arg, got {}", args.len()));
                }
                let color = self.eval(&args[0])?.as_u64()?;
                self.push(TraceKind::Display {
                    device: "TFT".into(),
                    detail: format!("FILL color=0x{color:04X}"),
                });
                Ok(Value::Void)
            }
            "TFT_SET_TEXT_COLOR" => {
                if args.len() != 2 {
                    return Err(format!(
                        "TFT_SET_TEXT_COLOR expects 2 args, got {}",
                        args.len()
                    ));
                }
                let fg = self.eval(&args[0])?.as_u64()? as u32;
                let bg = self.eval(&args[1])?.as_u64()? as u32;
                if let Some(t) = self.tft.as_mut() {
                    t.fg = fg;
                    t.bg = bg;
                }
                self.push(TraceKind::Display {
                    device: "TFT".into(),
                    detail: format!("TEXT_COLOR fg=0x{fg:04X} bg=0x{bg:04X}"),
                });
                Ok(Value::Void)
            }
            "TFT_SET_TEXT_SIZE" => {
                if args.len() != 1 {
                    return Err(format!("TFT_SET_TEXT_SIZE expects 1 arg, got {}", args.len()));
                }
                let n = self.eval(&args[0])?.as_u64()? as u32;
                if let Some(t) = self.tft.as_mut() {
                    t.text_size = n;
                }
                self.push(TraceKind::Display {
                    device: "TFT".into(),
                    detail: format!("TEXT_SIZE n={n}"),
                });
                Ok(Value::Void)
            }
            "TFT_PRINT" => {
                if args.len() != 3 {
                    return Err(format!("TFT_PRINT expects 3 args, got {}", args.len()));
                }
                let x = self.eval(&args[0])?.as_u64()?;
                let y = self.eval(&args[1])?.as_u64()?;
                let msg = self.eval(&args[2])?.as_string()?;
                self.push(TraceKind::Display {
                    device: "TFT".into(),
                    detail: format!("PRINT x={x} y={y} \"{msg}\""),
                });
                Ok(Value::Void)
            }
            "TFT_DRAW_ROUND_RECT" => {
                if args.len() != 6 {
                    return Err(format!(
                        "TFT_DRAW_ROUND_RECT expects 6 args, got {}",
                        args.len()
                    ));
                }
                let x = self.eval(&args[0])?.as_u64()?;
                let y = self.eval(&args[1])?.as_u64()?;
                let w = self.eval(&args[2])?.as_u64()?;
                let h = self.eval(&args[3])?.as_u64()?;
                let r = self.eval(&args[4])?.as_u64()?;
                let color = self.eval(&args[5])?.as_u64()?;
                self.push(TraceKind::Display {
                    device: "TFT".into(),
                    detail: format!("ROUND_RECT x={x} y={y} w={w} h={h} r={r} c=0x{color:04X}"),
                });
                Ok(Value::Void)
            }
            "PLAY_TONE" => {
                if args.len() != 3 {
                    return Err(format!("PLAY_TONE expects 3 args, got {}", args.len()));
                }
                let pin = self.eval(&args[0])?.as_u64()? as u32;
                let freq = self.eval(&args[1])?.as_u64()? as u32;
                let duration_ms = match self.eval(&args[2])? {
                    Value::TimeMs(ms) => ms,
                    Value::Uint(u) => u,
                    Value::Int(i) => i.max(0) as u64,
                    other => {
                        return Err(format!("PLAY_TONE duration expects TIME, got {other:?}"))
                    }
                };
                self.push(TraceKind::Tone {
                    pin,
                    freq,
                    duration_ms,
                });
                if duration_ms > 0 {
                    self.time_ms = self.time_ms.saturating_add(duration_ms);
                    self.scan_dt_ms = self.scan_dt_ms.saturating_add(duration_ms);
                    self.advance_timers(duration_ms);
                }
                Ok(Value::Void)
            }
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
        BinaryOp::Eq => Ok(Value::Bool(values_equal(&l, &r)?)),
        BinaryOp::Ne => Ok(Value::Bool(!values_equal(&l, &r)?)),
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            // REAL-aware ordering (plant/PID paths); integers stay exact via f64 for MVP range.
            let a = l.as_f64()?;
            let b = r.as_f64()?;
            let q = match op {
                BinaryOp::Lt => a < b,
                BinaryOp::Le => a <= b,
                BinaryOp::Gt => a > b,
                BinaryOp::Ge => a >= b,
                _ => unreachable!(),
            };
            Ok(Value::Bool(q))
        }
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

fn values_equal(l: &Value, r: &Value) -> Result<bool, String> {
    if matches!((l, r), (Value::Real(_), _) | (_, Value::Real(_))) {
        Ok((l.as_f64()? - r.as_f64()?).abs() < 1e-12)
    } else {
        Ok(l == r)
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
