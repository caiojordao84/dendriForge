//! Legacy ASL TOON → `.asl.toml` migrator (DEN-20).
//!
//! Ports `tools/migrate_asl_toon.py` (DEN-15) to pure Rust. Converts
//! `core/asl/v010/{golden,examples}` programs into tagged-statement TOML.
//! Drops `version` / `ASLversion`. Nesting uses the same heuristics as the
//! Python one-shot — hand-protected fixtures must not be overwritten.
//!
//! Migrated programs stamp `asl_version = "0.0.1"` (live ASL tag).

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use thiserror::Error;

static CTRL_IF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?i)(IF(?:\s+NOT)?|ELSIF)\s+(.+):\s*(.*)$").unwrap());
static CTRL_ELSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?i)ELSE:\s*(.*)$").unwrap());
static CTRL_WHILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?i)WHILE\s+(.+):\s*(.*)$").unwrap());
static CTRL_FOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?i)FOR\s+([A-Za-z_][A-Za-z0-9_]*)\s*:=\s*0\s+TO\s+(.+):\s*(.*)$").unwrap()
});
static CTRL_REPEAT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?i)repeat\((\S+)\):\s*(.*)$").unwrap());
static ASSIGN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.+)$").unwrap());
static CALL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*)\((.*)\)\s*$").unwrap());
static VENDOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)^VENDOR_EXTENSION\((["'].*["'])\)\s*$"#).unwrap());
static RETURN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^RETURN(?:\s+(.+))?$").unwrap());
static HW_ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^(\d+)\|(\w+)\|(\w+)\|"([^"]*)"\|(.+)$"#).unwrap());
static DATA_ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*)\|(\w+)\|(\w+)\|(.+)$").unwrap());
static AND_CALL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^AND\((.+)\)$").unwrap());
static FN_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*)\(([^)]*)\)\[(\d+)\]:\s*$").unwrap());
static RE_BOOL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(TRUE|FALSE)$").unwrap());
static RE_UINT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]+$").unwrap());
static RE_INT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-?[0-9]+$").unwrap());
static RE_REAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-?[0-9]+\.[0-9]+$").unwrap());
static RE_TIME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^T#[0-9]+ms$").unwrap());
static RE_HEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^0x[0-9A-Fa-f]+$").unwrap());
static RE_STR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^"([^"\\]|\\.)*"$"#).unwrap());
static RE_VAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").unwrap());
static RE_SETUP_LOOP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(setup|loop)\[\d+\]:\s*$").unwrap());
static RE_EMPTY_STRING_DASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^""-"#).unwrap());
static RE_TRUE_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bTRUE\b").unwrap());
static RE_FALSE_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bFALSE\b").unwrap());

const EDGE_MODES: &[&str] = &["RISING", "FALLING", "CHANGE"];

/// Hand-migrated fixtures — must never be overwritten by the migrator.
pub const PROTECTED: &[&str] = &[
    "golden/G01-gpio-all-modes-probe.asl.toml",
    "golden/G02-input-pullup-pulldown-matrix.asl.toml",
    "golden/G03-traffic-light-sequencer.asl.toml",
    "golden/G05-pwm-fade-breathing-led.asl.toml",
    "golden/G08-stopwatch-lap-timer.asl.toml",
    "golden/G17-retain-power-cycle-counter.asl.toml",
    "examples/basic-blink-logic.asl.toml",
    "errors/E01-missing-metadata-field.asl.toml",
    "errors/E02-unknown-data-type.asl.toml",
    "errors/E03-undeclared-variable.asl.toml",
    "errors/E04-hardware-pin-conflict.asl.toml",
    "errors/E05-setup-count-mismatch.asl.toml",
    "errors/E06-invalid-scope-write-const.asl.toml",
    "errors/E07-function-arity-mismatch.asl.toml",
];

#[derive(Debug, Error)]
pub enum AslToonError {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("convert {path}: {message}")]
    Convert { path: String, message: String },
}

#[derive(Debug, Clone)]
pub struct MigrateOptions {
    pub dry_run: bool,
    /// Substring filters against the source path (same as Python `--only`).
    pub only: Vec<String>,
}

impl Default for MigrateOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            only: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MigrateItem {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub written: bool,
    pub protected: bool,
}

#[derive(Debug, Clone, Default)]
pub struct MigrateReport {
    pub converted: usize,
    pub skipped_protected: usize,
    pub items: Vec<MigrateItem>,
}

pub fn is_protected(rel: &str) -> bool {
    PROTECTED.contains(&rel)
}

/// Convert one ASL TOON program text into tagged `.asl.toml`.
pub fn convert_program(text: &str, source_rel: &str) -> String {
    let meta = parse_metadata(text);
    let mut hw = parse_hardware(text);
    let mut data = parse_data(text);
    let setup_lines = parse_list_section(text, "INITIALIZATION SYSTEM");
    let loop_lines = parse_list_section(text, "BEHAVIORAL LOGIC");
    let (setup_stmts, _) = parse_block(&setup_lines, 0, &HashSet::new());
    let (loop_stmts, _) = parse_block(&loop_lines, 0, &HashSet::new());
    let fns = parse_functions(text);

    let mut declared: HashSet<String> = data.iter().map(|d| d.name.clone()).collect();
    let mut counters = BTreeSet::new();
    collect_for_counters(&setup_stmts, &mut counters);
    collect_for_counters(&loop_stmts, &mut counters);
    for fn_def in &fns {
        collect_for_counters(&fn_def.body, &mut counters);
    }
    for c in counters {
        if !declared.contains(&c) {
            data.push(DataRow {
                name: c.clone(),
                ty: "UINT".into(),
                scope: "VAR".into(),
                init: "0".into(),
                unit: "-".into(),
            });
            declared.insert(c);
        }
    }

    let project = meta
        .get("project_name")
        .map(String::as_str)
        .unwrap_or("unnamed");
    let editor = meta
        .get("editor")
        .map(String::as_str)
        .unwrap_or("dendriForge");
    let author = meta
        .get("author")
        .map(String::as_str)
        .unwrap_or("dendriForge");
    let notes = meta.get("notes").map(String::as_str).unwrap_or("");

    let mut out: Vec<String> = Vec::new();
    out.push(format!("# Migrated from {source_rel}"));
    out.push("# Legacy TOON retained under core/asl/v010/ (read-only).".into());
    out.push(String::new());
    out.push("[metadata]".into());
    out.push(format!("asl_version = \"{}\"", crate::ASL_VERSION));
    out.push(format!("project_name = \"{}\"", esc(project)));
    out.push(format!("editor = \"{}\"", esc(editor)));
    out.push(format!("author = \"{}\"", esc(author)));
    if !notes.is_empty() {
        out.push(format!("notes = \"{}\"", esc(notes)));
    } else {
        out.push("notes = \"Migrated from legacy v010 TOON — ASL 0.0.1\"".into());
    }
    out.push(String::new());

    for h in &mut hw {
        out.push("[[hardware]]".into());
        out.push(format!("pin = {}", h.pin));
        out.push(format!("hw_type = \"{}\"", h.hw_type));
        out.push(format!("io_mode = \"{}\"", h.io_mode));
        out.push(format!("label = \"{}\"", esc(&h.label)));
        out.push(format!("target_role = \"{}\"", esc(&h.target_role)));
        out.push(String::new());
    }

    for d in &data {
        let mut init = d.init.clone();
        if init.eq_ignore_ascii_case("TRUE") || init.eq_ignore_ascii_case("FALSE") {
            init = init.to_ascii_uppercase();
        }
        out.push("[[data]]".into());
        out.push(format!("name = \"{}\"", d.name));
        out.push(format!("type = \"{}\"", d.ty));
        out.push(format!("scope = \"{}\"", d.scope));
        out.push(format!("init = \"{}\"", esc(&init)));
        out.push(format!("unit = \"{}\"", esc(&d.unit)));
        out.push(String::new());
    }

    for st in &setup_stmts {
        out.push(emit_top_stmt(st, "setup"));
    }
    for st in &loop_stmts {
        out.push(emit_top_stmt(st, "loop"));
    }

    for fn_def in &fns {
        let body = fn_def
            .body
            .iter()
            .map(emit_stmt)
            .collect::<Vec<_>>()
            .join(", ");
        let params = fn_def
            .params
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(", ");
        out.push("[[functions]]".into());
        out.push(format!("name = \"{}\"", fn_def.name));
        out.push(format!("params = [{params}]"));
        out.push(format!("body = [{body}]"));
        out.push(String::new());
    }

    let mut s = out.join("\n");
    while s.ends_with('\n') {
        s.pop();
    }
    s.push('\n');
    s
}

/// Migrate `core/asl/v010/{golden,examples}/*.toon` → `content/fixtures/.../*.asl.toml`.
pub fn migrate_v010(repo_root: &Path, opts: &MigrateOptions) -> Result<MigrateReport, AslToonError> {
    let v010 = repo_root.join("core/asl/v010");
    let out_root = repo_root.join("content/fixtures");
    let mut report = MigrateReport::default();

    for sub in ["golden", "examples"] {
        let src_dir = v010.join(sub);
        if !src_dir.exists() {
            continue;
        }
        let mut entries: Vec<_> = fs::read_dir(&src_dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("toon"))
            .collect();
        entries.sort();

        for src in entries {
            let dst_name = src
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown.toon")
                .replace(".toon", ".asl.toml");
            let rel = format!("{sub}/{dst_name}");
            let dst = out_root.join(sub).join(&dst_name);

            if !opts.only.is_empty() {
                let src_s = src.to_string_lossy();
                if !opts.only.iter().any(|o| src_s.contains(o)) {
                    continue;
                }
            }
            if is_protected(&rel) {
                report.skipped_protected += 1;
                report.items.push(MigrateItem {
                    source: src,
                    destination: dst,
                    written: false,
                    protected: true,
                });
                continue;
            }

            let text = fs::read_to_string(&src)?;
            let source_rel = src
                .strip_prefix(repo_root)
                .unwrap_or(&src)
                .to_string_lossy()
                .replace('\\', "/");
            let toml = convert_program(&text, &source_rel);
            if !opts.dry_run {
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&dst, &toml)?;
            }
            report.converted += 1;
            report.items.push(MigrateItem {
                source: src,
                destination: dst,
                written: !opts.dry_run,
                protected: false,
            });
        }
    }
    Ok(report)
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn split_args(argstr: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut quote = '"';
    for ch in argstr.chars() {
        if in_str {
            cur.push(ch);
            if ch == quote {
                in_str = false;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_str = true;
            quote = ch;
            cur.push(ch);
            continue;
        }
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth == 0 => {
                let t = cur.trim().to_string();
                if !t.is_empty() {
                    args.push(t);
                }
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    let t = cur.trim().to_string();
    if !t.is_empty() {
        args.push(t);
    }
    args
}

fn norm_expr(text: &str) -> String {
    let mut t = text.trim().to_string();
    t = t.replace("<>", "!=");
    if let Some(caps) = AND_CALL.captures(&t) {
        let args = split_args(caps.get(1).unwrap().as_str());
        if args.len() == 2 {
            t = format!("{} AND {}", args[0], args[1]);
        }
    }
    t = RE_TRUE_WORD.replace_all(&t, "TRUE").into_owned();
    t = RE_FALSE_WORD.replace_all(&t, "FALSE").into_owned();
    t
}

fn expr_toml(text: &str) -> String {
    let t = norm_expr(text);
    if RE_BOOL.is_match(&t) {
        let v = if t.eq_ignore_ascii_case("TRUE") {
            "true"
        } else {
            "false"
        };
        return format!(r#"{{ kind = "bool", value = {v} }}"#);
    }
    if EDGE_MODES.iter().any(|m| *m == t) {
        return format!(r#"{{ kind = "string", value = "{t}" }}"#);
    }
    if RE_UINT.is_match(&t) {
        return format!(r#"{{ kind = "uint", value = {t} }}"#);
    }
    if RE_INT.is_match(&t) {
        return format!(r#"{{ kind = "int", value = {t} }}"#);
    }
    if RE_REAL.is_match(&t) {
        return format!(r#"{{ kind = "real", value = {t} }}"#);
    }
    if RE_TIME.is_match(&t) {
        return format!(r#"{{ kind = "time", value = "{t}" }}"#);
    }
    if RE_HEX.is_match(&t) {
        return format!(r#"{{ kind = "hex", value = "{t}" }}"#);
    }
    if RE_STR.is_match(&t) {
        let inner = &t[1..t.len() - 1];
        return format!(r#"{{ kind = "string", value = "{}" }}"#, esc(inner));
    }
    if RE_VAR.is_match(&t) {
        return format!(r#"{{ kind = "var", name = "{t}" }}"#);
    }
    format!(r#"{{ kind = "expr_text", text = "{}" }}"#, esc(&t))
}

#[derive(Debug, Clone)]
enum Kind {
    Empty,
    Exit,
    Continue,
    Return { value: Option<String> },
    Vendor { reason: String },
    Else { inline: String },
    If { cond: String, inline: String },
    Elsif { cond: String, inline: String },
    While { cond: String, inline: String },
    For {
        counter: String,
        to: String,
        inline: String,
    },
    Assign { target: String, value: String },
    Call { name: String, args: Vec<String> },
    Raw { text: String },
}

fn classify(line: &str) -> Kind {
    let s = line.trim();
    if s.is_empty() {
        return Kind::Empty;
    }
    let upper = s.to_ascii_uppercase();
    if upper == "EXIT" || upper == "BREAK" {
        return Kind::Exit;
    }
    if upper == "CONTINUE" {
        return Kind::Continue;
    }
    if let Some(caps) = RETURN.captures(s) {
        return Kind::Return {
            value: caps.get(1).map(|m| m.as_str().to_string()),
        };
    }
    if let Some(caps) = VENDOR.captures(s) {
        let mut raw = caps.get(1).unwrap().as_str().trim().to_string();
        if (raw.starts_with('"') && raw.ends_with('"'))
            || (raw.starts_with('\'') && raw.ends_with('\''))
        {
            raw = raw[1..raw.len() - 1].to_string();
        }
        return Kind::Vendor { reason: raw };
    }
    if let Some(caps) = CTRL_ELSE.captures(s) {
        return Kind::Else {
            inline: caps.get(1).unwrap().as_str().trim().to_string(),
        };
    }
    if let Some(caps) = CTRL_IF.captures(s) {
        let mut kind = caps.get(1).unwrap().as_str().to_ascii_uppercase();
        let mut cond = caps.get(2).unwrap().as_str().trim().to_string();
        let inline = caps.get(3).unwrap().as_str().trim().to_string();
        if kind.starts_with("IF") {
            let parts: Vec<_> = kind.split_whitespace().collect();
            if parts.len() >= 2 && parts[1] == "NOT" {
                if !cond.to_ascii_uppercase().starts_with("NOT ") {
                    cond = format!("NOT {cond}");
                }
                kind = "IF".into();
            }
        }
        if kind == "ELSIF" {
            return Kind::Elsif { cond, inline };
        }
        return Kind::If { cond, inline };
    }
    if let Some(caps) = CTRL_WHILE.captures(s) {
        return Kind::While {
            cond: caps.get(1).unwrap().as_str().trim().to_string(),
            inline: caps.get(2).unwrap().as_str().trim().to_string(),
        };
    }
    if let Some(caps) = CTRL_FOR.captures(s) {
        return Kind::For {
            counter: caps.get(1).unwrap().as_str().to_string(),
            to: caps.get(2).unwrap().as_str().trim().to_string(),
            inline: caps.get(3).unwrap().as_str().trim().to_string(),
        };
    }
    if let Some(caps) = CTRL_REPEAT.captures(s) {
        return Kind::For {
            counter: "_i".into(),
            to: caps.get(1).unwrap().as_str().trim().to_string(),
            inline: caps.get(2).unwrap().as_str().trim().to_string(),
        };
    }
    if let Some(caps) = ASSIGN.captures(s) {
        if !s.starts_with("IF ") {
            let left = caps.get(1).unwrap().as_str();
            let right = caps.get(2).unwrap().as_str();
            if !left.contains('(') {
                return Kind::Assign {
                    target: left.to_string(),
                    value: right.to_string(),
                };
            }
        }
    }
    if let Some(caps) = CALL.captures(s) {
        return Kind::Call {
            name: caps.get(1).unwrap().as_str().to_string(),
            args: split_args(caps.get(2).unwrap().as_str()),
        };
    }
    Kind::Raw {
        text: s.to_string(),
    }
}

fn kind_name(k: &Kind) -> &'static str {
    match k {
        Kind::Empty => "empty",
        Kind::Exit => "exit",
        Kind::Continue => "continue",
        Kind::Return { .. } => "return",
        Kind::Vendor { .. } => "vendor",
        Kind::Else { .. } => "else",
        Kind::If { .. } => "if",
        Kind::Elsif { .. } => "elsif",
        Kind::While { .. } => "while",
        Kind::For { .. } => "for",
        Kind::Assign { .. } => "assign",
        Kind::Call { .. } => "call",
        Kind::Raw { .. } => "raw",
    }
}

#[derive(Debug, Clone)]
struct Stmt {
    kind: String,
    // shared fields via Option
    target: Option<String>,
    value: Option<String>,
    name: Option<String>,
    args: Vec<String>,
    reason: Option<String>,
    condition: Option<String>,
    counter: Option<String>,
    to: Option<String>,
    then: Vec<Stmt>,
    elif: Vec<ElifArm>,
    else_body: Option<Vec<Stmt>>,
    body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
struct ElifArm {
    cond: String,
    body: Vec<Stmt>,
}

fn parse_block(items: &[String], mut i: usize, stop: &HashSet<&str>) -> (Vec<Stmt>, usize) {
    let mut out = Vec::new();
    while i < items.len() {
        let k = classify(&items[i]);
        if stop.contains(kind_name(&k)) {
            break;
        }
        let (stmt, next) = parse_one(items, i);
        out.push(stmt);
        i = next;
    }
    (out, i)
}

fn parse_one(items: &[String], mut i: usize) -> (Stmt, usize) {
    let kind = classify(&items[i]);
    i += 1;
    match kind {
        Kind::Assign { target, value } => (
            Stmt {
                kind: "assign".into(),
                target: Some(target),
                value: Some(value),
                ..Stmt::empty()
            },
            i,
        ),
        Kind::Call { name, args } => (
            Stmt {
                kind: "call".into(),
                name: Some(name),
                args,
                ..Stmt::empty()
            },
            i,
        ),
        Kind::Vendor { reason } => (
            Stmt {
                kind: "vendor_extension".into(),
                reason: Some(reason),
                ..Stmt::empty()
            },
            i,
        ),
        Kind::Return { value } => (
            Stmt {
                kind: "return".into(),
                value,
                ..Stmt::empty()
            },
            i,
        ),
        Kind::Exit => (
            Stmt {
                kind: "exit".into(),
                ..Stmt::empty()
            },
            i,
        ),
        Kind::Continue => (
            Stmt {
                kind: "continue".into(),
                ..Stmt::empty()
            },
            i,
        ),
        Kind::Raw { text } => (
            Stmt {
                kind: "vendor_extension".into(),
                reason: Some(format!("UNPARSED_STMT:{text}")),
                ..Stmt::empty()
            },
            i,
        ),
        Kind::If { .. } => {
            let (then, elif_arms, else_body, next) = parse_if_chain(items, i - 1);
            (
                Stmt {
                    kind: "if".into(),
                    condition: Some(then.cond),
                    then: then.body,
                    elif: elif_arms,
                    else_body,
                    ..Stmt::empty()
                },
                next,
            )
        }
        Kind::While { cond, inline } => {
            let body = if !inline.is_empty() {
                vec![inline_stmt(&inline)]
            } else {
                let (b, next) = parse_loop_body(items, i);
                i = next;
                b
            };
            (
                Stmt {
                    kind: "while".into(),
                    condition: Some(cond),
                    body,
                    ..Stmt::empty()
                },
                i,
            )
        }
        Kind::For {
            counter,
            to,
            inline,
        } => {
            let body = if !inline.is_empty() {
                vec![inline_stmt(&inline)]
            } else {
                let (b, next) = parse_loop_body(items, i);
                i = next;
                b
            };
            (
                Stmt {
                    kind: "for".into(),
                    counter: Some(counter),
                    to: Some(to),
                    body,
                    ..Stmt::empty()
                },
                i,
            )
        }
        Kind::Elsif { .. } => (
            Stmt {
                kind: "vendor_extension".into(),
                reason: Some("ORPHAN_ELSIF".into()),
                ..Stmt::empty()
            },
            i,
        ),
        Kind::Else { .. } => (
            Stmt {
                kind: "vendor_extension".into(),
                reason: Some("ORPHAN_ELSE".into()),
                ..Stmt::empty()
            },
            i,
        ),
        Kind::Empty => (
            Stmt {
                kind: "vendor_extension".into(),
                reason: Some(format!("UNKNOWN:{}", items[i - 1])),
                ..Stmt::empty()
            },
            i,
        ),
    }
}

impl Stmt {
    fn empty() -> Self {
        Self {
            kind: String::new(),
            target: None,
            value: None,
            name: None,
            args: Vec::new(),
            reason: None,
            condition: None,
            counter: None,
            to: None,
            then: Vec::new(),
            elif: Vec::new(),
            else_body: None,
            body: Vec::new(),
        }
    }
}

fn inline_stmt(text: &str) -> Stmt {
    match classify(text) {
        Kind::Assign { target, value } => Stmt {
            kind: "assign".into(),
            target: Some(target),
            value: Some(value),
            ..Stmt::empty()
        },
        Kind::Call { name, args } => Stmt {
            kind: "call".into(),
            name: Some(name),
            args,
            ..Stmt::empty()
        },
        Kind::Return { value } => Stmt {
            kind: "return".into(),
            value,
            ..Stmt::empty()
        },
        Kind::Vendor { reason } => Stmt {
            kind: "vendor_extension".into(),
            reason: Some(reason),
            ..Stmt::empty()
        },
        Kind::Exit => Stmt {
            kind: "exit".into(),
            ..Stmt::empty()
        },
        Kind::Continue => Stmt {
            kind: "continue".into(),
            ..Stmt::empty()
        },
        _ => Stmt {
            kind: "vendor_extension".into(),
            reason: Some(format!("UNPARSED_INLINE:{text}")),
            ..Stmt::empty()
        },
    }
}

struct ThenArm {
    cond: String,
    body: Vec<Stmt>,
}

fn parse_if_chain(
    items: &[String],
    mut i: usize,
) -> (ThenArm, Vec<ElifArm>, Option<Vec<Stmt>>, usize) {
    let Kind::If { cond, inline } = classify(&items[i]) else {
        panic!("parse_if_chain expected IF");
    };
    i += 1;
    let then_body = if !inline.is_empty() {
        vec![inline_stmt(&inline)]
    } else {
        let stop: HashSet<&str> = ["if", "elsif", "else"].into_iter().collect();
        let (b, next) = parse_block(items, i, &stop);
        i = next;
        b
    };
    let then = ThenArm {
        cond,
        body: then_body,
    };
    let mut elif_arms = Vec::new();
    let mut else_body = None;
    while i < items.len() {
        match classify(&items[i]) {
            Kind::Elsif { cond, inline } => {
                i += 1;
                let body = if !inline.is_empty() {
                    vec![inline_stmt(&inline)]
                } else {
                    let stop: HashSet<&str> = ["if", "elsif", "else"].into_iter().collect();
                    let (b, next) = parse_block(items, i, &stop);
                    i = next;
                    b
                };
                elif_arms.push(ElifArm { cond, body });
            }
            Kind::Else { inline } => {
                i += 1;
                let body = if !inline.is_empty() {
                    vec![inline_stmt(&inline)]
                } else {
                    let stop: HashSet<&str> = ["if", "elsif", "else"].into_iter().collect();
                    let (b, next) = parse_block(items, i, &stop);
                    i = next;
                    b
                };
                else_body = Some(body);
                break;
            }
            _ => break,
        }
    }
    (then, elif_arms, else_body, i)
}

fn parse_loop_body(items: &[String], mut i: usize) -> (Vec<Stmt>, usize) {
    if i >= items.len() {
        return (Vec::new(), i);
    }
    let peek = classify(&items[i]);
    if matches!(peek, Kind::While { .. } | Kind::For { .. }) {
        let (stmt, i2) = parse_one(items, i);
        return (vec![stmt], i2);
    }
    let mut body = Vec::new();
    while i < items.len() {
        let k = classify(&items[i]);
        match &k {
            Kind::Elsif { .. } | Kind::Else { .. } => break,
            Kind::While { .. } | Kind::For { .. } if !body.is_empty() => {
                let (stmt, next) = parse_one(items, i);
                body.push(stmt);
                i = next;
            }
            Kind::If { .. }
            | Kind::Assign { .. }
            | Kind::Call { .. }
            | Kind::Return { .. }
            | Kind::Exit
            | Kind::Continue
            | Kind::Vendor { .. }
            | Kind::Raw { .. }
            | Kind::While { .. }
            | Kind::For { .. } => {
                let (stmt, next) = parse_one(items, i);
                body.push(stmt);
                i = next;
            }
            _ => break,
        }
    }
    (body, i)
}

fn emit_stmt(stmt: &Stmt) -> String {
    match stmt.kind.as_str() {
        "assign" => format!(
            r#"{{ kind = "assign", target = "{}", value = {} }}"#,
            stmt.target.as_deref().unwrap_or(""),
            expr_toml(stmt.value.as_deref().unwrap_or(""))
        ),
        "call" => {
            let args = stmt
                .args
                .iter()
                .map(|a| expr_toml(a))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                r#"{{ kind = "call", name = "{}", args = [{args}] }}"#,
                stmt.name.as_deref().unwrap_or("")
            )
        }
        "vendor_extension" => format!(
            r#"{{ kind = "vendor_extension", reason = "{}" }}"#,
            esc(stmt.reason.as_deref().unwrap_or(""))
        ),
        "return" => {
            if let Some(v) = &stmt.value {
                if !v.is_empty() {
                    return format!(r#"{{ kind = "return", value = {} }}"#, expr_toml(v));
                }
            }
            r#"{ kind = "return" }"#.into()
        }
        "exit" => r#"{ kind = "exit" }"#.into(),
        "continue" => r#"{ kind = "continue" }"#.into(),
        "while" => {
            let body = stmt
                .body
                .iter()
                .map(emit_stmt)
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                r#"{{ kind = "while", condition = {}, body = [{body}] }}"#,
                expr_toml(stmt.condition.as_deref().unwrap_or(""))
            )
        }
        "for" => {
            let body = stmt
                .body
                .iter()
                .map(emit_stmt)
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                r#"{{ kind = "for", counter = "{}", to = {}, body = [{body}] }}"#,
                stmt.counter.as_deref().unwrap_or(""),
                expr_toml(stmt.to.as_deref().unwrap_or(""))
            )
        }
        "if" => {
            let then = stmt
                .then
                .iter()
                .map(emit_stmt)
                .collect::<Vec<_>>()
                .join(", ");
            let mut parts = vec![
                r#"kind = "if""#.to_string(),
                format!(
                    "condition = {}",
                    expr_toml(stmt.condition.as_deref().unwrap_or(""))
                ),
                format!("then = [{then}]"),
            ];
            if !stmt.elif.is_empty() {
                let arms = stmt
                    .elif
                    .iter()
                    .map(|arm| {
                        let b = arm
                            .body
                            .iter()
                            .map(emit_stmt)
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!(
                            "{{ condition = {}, then = [{b}] }}",
                            expr_toml(&arm.cond)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                parts.push(format!("elif = [{arms}]"));
            }
            if let Some(eb) = &stmt.else_body {
                let eb_s = eb.iter().map(emit_stmt).collect::<Vec<_>>().join(", ");
                parts.push(format!("else = [{eb_s}]"));
            }
            format!("{{ {} }}", parts.join(", "))
        }
        _ => r#"{ kind = "vendor_extension", reason = "BAD_STMT" }"#.into(),
    }
}

fn emit_top_stmt(stmt: &Stmt, section: &str) -> String {
    match stmt.kind.as_str() {
        "assign" => format!(
            "[[{section}]]\nkind = \"assign\"\ntarget = \"{}\"\nvalue = {}\n",
            stmt.target.as_deref().unwrap_or(""),
            expr_toml(stmt.value.as_deref().unwrap_or(""))
        ),
        "call" => {
            let args = stmt
                .args
                .iter()
                .map(|a| expr_toml(a))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "[[{section}]]\nkind = \"call\"\nname = \"{}\"\nargs = [{args}]\n",
                stmt.name.as_deref().unwrap_or("")
            )
        }
        "vendor_extension" => format!(
            "[[{section}]]\nkind = \"vendor_extension\"\nreason = \"{}\"\n",
            esc(stmt.reason.as_deref().unwrap_or(""))
        ),
        "return" => {
            if let Some(v) = &stmt.value {
                if !v.is_empty() {
                    return format!(
                        "[[{section}]]\nkind = \"return\"\nvalue = {}\n",
                        expr_toml(v)
                    );
                }
            }
            format!("[[{section}]]\nkind = \"return\"\n")
        }
        "exit" | "continue" => format!("[[{section}]]\nkind = \"{}\"\n", stmt.kind),
        _ => format!("[[{section}]]\n{}", emit_complex_table(stmt)),
    }
}

fn emit_complex_table(stmt: &Stmt) -> String {
    let mut lines = vec![format!("kind = \"{}\"", stmt.kind)];
    match stmt.kind.as_str() {
        "if" => {
            lines.push(format!(
                "condition = {}",
                expr_toml(stmt.condition.as_deref().unwrap_or(""))
            ));
            let then = stmt
                .then
                .iter()
                .map(emit_stmt)
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("then = [{then}]"));
            if !stmt.elif.is_empty() {
                let arms = stmt
                    .elif
                    .iter()
                    .map(|arm| {
                        let b = arm
                            .body
                            .iter()
                            .map(emit_stmt)
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!(
                            "{{ condition = {}, then = [{b}] }}",
                            expr_toml(&arm.cond)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(format!("elif = [{arms}]"));
            }
            if let Some(eb) = &stmt.else_body {
                let eb_s = eb.iter().map(emit_stmt).collect::<Vec<_>>().join(", ");
                lines.push(format!("else = [{eb_s}]"));
            }
        }
        "while" => {
            lines.push(format!(
                "condition = {}",
                expr_toml(stmt.condition.as_deref().unwrap_or(""))
            ));
            let body = stmt
                .body
                .iter()
                .map(emit_stmt)
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("body = [{body}]"));
        }
        "for" => {
            lines.push(format!(
                "counter = \"{}\"",
                stmt.counter.as_deref().unwrap_or("")
            ));
            lines.push(format!(
                "to = {}",
                expr_toml(stmt.to.as_deref().unwrap_or(""))
            ));
            let body = stmt
                .body
                .iter()
                .map(emit_stmt)
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("body = [{body}]"));
        }
        _ => lines.push(r#"reason = "UNSUPPORTED_COMPLEX""#.into()),
    }
    lines.join("\n") + "\n"
}

fn parse_metadata(text: &str) -> std::collections::BTreeMap<String, String> {
    let mut meta = std::collections::BTreeMap::new();
    let mut in_meta = false;
    for line in text.lines() {
        if line.starts_with("# METADATA:") {
            in_meta = true;
            continue;
        }
        if in_meta {
            if line.starts_with("## ") || (line.starts_with("# ") && line.contains("HARDWARE")) {
                break;
            }
            if line.starts_with("##") {
                break;
            }
            if line.contains(':') && !line.starts_with('#') {
                if let Some((k, v)) = line.split_once(':') {
                    // Drop legacy TOON ASLversion / version; stamp ASL_VERSION on emit.
                    let key = k.trim();
                    if key.eq_ignore_ascii_case("ASLversion")
                        || key.eq_ignore_ascii_case("version")
                    {
                        continue;
                    }
                    meta.insert(key.to_string(), v.trim().to_string());
                }
            }
        }
    }
    meta
}

#[derive(Debug, Clone)]
struct HwRow {
    pin: i64,
    hw_type: String,
    io_mode: String,
    label: String,
    target_role: String,
}

fn parse_hardware(text: &str) -> Vec<HwRow> {
    let mut rows = Vec::new();
    let mut in_hw = false;
    for line in text.lines() {
        if line.contains("HARDWARE INTERFACE") {
            in_hw = true;
            continue;
        }
        if in_hw && line.starts_with("## ") {
            break;
        }
        if !in_hw {
            continue;
        }
        let s = line.trim();
        if s.is_empty() || s.starts_with("hardware[") || s.starts_with('#') {
            continue;
        }
        if let Some(caps) = HW_ROW.captures(s) {
            let io = caps.get(3).unwrap().as_str();
            let io_mode = match io {
                "input" | "input_pullup" | "DI" => "DI".to_string(),
                "output" | "DO" => "DO".to_string(),
                "bi" | "BI" => "BI".to_string(),
                other if other.len() <= 3 => other.to_ascii_uppercase(),
                _ => "DI".to_string(),
            };
            rows.push(HwRow {
                pin: caps.get(1).unwrap().as_str().parse().unwrap_or(0),
                hw_type: caps.get(2).unwrap().as_str().to_ascii_lowercase(),
                io_mode,
                label: caps.get(4).unwrap().as_str().to_string(),
                target_role: caps
                    .get(5)
                    .unwrap()
                    .as_str()
                    .trim()
                    .trim_matches('"')
                    .to_string(),
            });
        }
    }
    rows
}

#[derive(Debug, Clone)]
struct DataRow {
    name: String,
    ty: String,
    scope: String,
    init: String,
    unit: String,
}

fn parse_data(text: &str) -> Vec<DataRow> {
    let mut rows = Vec::new();
    let mut in_data = false;
    for line in text.lines() {
        if line.contains("SYSTEM DATA") {
            in_data = true;
            continue;
        }
        if in_data && line.starts_with("## ") {
            break;
        }
        if !in_data {
            continue;
        }
        let s = line.trim();
        if s.is_empty() || s.starts_with("data[") || s.starts_with('#') {
            continue;
        }
        if let Some(caps) = DATA_ROW.captures(s) {
            let mut ty = caps.get(2).unwrap().as_str().to_ascii_uppercase();
            ty = match ty.as_str() {
                "FLOAT" => "REAL".into(),
                "DOUBLE" => "LREAL".into(),
                "BOOLEAN" => "BOOL".into(),
                other => other.to_string(),
            };
            let mut scope = caps.get(3).unwrap().as_str().to_ascii_uppercase();
            scope = match scope.as_str() {
                "CONSTANT" => "CONST".into(),
                "VAR_INPUT" | "VAR_OUTPUT" => "VAR".into(),
                other => other.to_string(),
            };
            let rest = caps.get(4).unwrap().as_str().trim();
            let (init_raw, unit) = parse_init_unit(rest);
            let mut init = init_raw.trim().to_string();
            if init == "\"\"" {
                init.clear();
            } else if init.len() >= 2 && init.starts_with('"') && init.ends_with('"') {
                init = init[1..init.len() - 1].to_string();
            }
            rows.push(DataRow {
                name: caps.get(1).unwrap().as_str().to_string(),
                ty,
                scope,
                init,
                unit: if unit.is_empty() {
                    "-".into()
                } else {
                    unit
                },
            });
        }
    }
    rows
}

fn parse_init_unit(rest: &str) -> (String, String) {
    if rest.ends_with("| -") || rest.ends_with("|-") {
        if let Some((init, unit)) = rest.rsplit_once('|') {
            return (init.to_string(), {
                let u = unit.trim();
                if u.is_empty() {
                    "-".into()
                } else {
                    u.to_string()
                }
            });
        }
    }
    if rest.contains("| ") || (rest.matches('|').count() >= 1 && !rest.starts_with('"')) {
        if let Some((init, unit)) = rest.rsplit_once('|') {
            let u = unit.trim();
            return (
                init.to_string(),
                if u.is_empty() {
                    "-".into()
                } else {
                    u.to_string()
                },
            );
        }
        return (rest.to_string(), "-".into());
    }
    if RE_EMPTY_STRING_DASH.is_match(rest) {
        return ("\"\"".into(), "-".into());
    }
    if rest.ends_with('-') && rest.starts_with('"') && rest.matches('"').count() >= 2 {
        return (rest[..rest.len() - 1].to_string(), "-".into());
    }
    if rest.contains('|') {
        if let Some((init, unit)) = rest.rsplit_once('|') {
            let u = unit.trim();
            return (
                init.to_string(),
                if u.is_empty() {
                    "-".into()
                } else {
                    u.to_string()
                },
            );
        }
    }
    (rest.to_string(), "-".into())
}

fn parse_list_section(text: &str, header_substr: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_sec = false;
    for line in text.lines() {
        if line.contains(header_substr) && line.starts_with("## ") {
            in_sec = true;
            continue;
        }
        if in_sec && line.starts_with("## ") {
            break;
        }
        if !in_sec {
            continue;
        }
        let s = line.trim();
        if s.is_empty() || s.starts_with('#') {
            continue;
        }
        if RE_SETUP_LOOP.is_match(s) {
            continue;
        }
        if let Some(rest) = s.strip_prefix("- ") {
            lines.push(rest.trim().to_string());
        }
    }
    lines
}

#[derive(Debug, Clone)]
struct FnDef {
    name: String,
    params: Vec<String>,
    body: Vec<Stmt>,
}

fn parse_functions(text: &str) -> Vec<FnDef> {
    let mut fns = Vec::new();
    let mut in_fn = false;
    let mut cur: Option<(String, Vec<String>)> = None;
    let mut body_lines: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.contains("PROCEDURAL FUNCTIONS") {
            in_fn = true;
            continue;
        }
        if !in_fn {
            continue;
        }
        if line.starts_with("## ") {
            break;
        }
        let s = line.trim();
        if s.is_empty() || s.starts_with('#') {
            continue;
        }
        if s == "functions:" {
            continue;
        }
        if let Some(caps) = FN_HEADER.captures(s) {
            if let Some((name, params)) = cur.take() {
                let (stmts, _) = parse_block(&body_lines, 0, &HashSet::new());
                fns.push(FnDef {
                    name,
                    params,
                    body: stmts,
                });
            }
            let params: Vec<String> = caps
                .get(2)
                .unwrap()
                .as_str()
                .split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect();
            cur = Some((caps.get(1).unwrap().as_str().to_string(), params));
            body_lines.clear();
            continue;
        }
        if let Some(rest) = s.strip_prefix("- ") {
            if cur.is_some() {
                body_lines.push(rest.trim().to_string());
            }
        }
    }
    if let Some((name, params)) = cur {
        let (stmts, _) = parse_block(&body_lines, 0, &HashSet::new());
        fns.push(FnDef {
            name,
            params,
            body: stmts,
        });
    }
    fns
}

fn collect_for_counters(stmts: &[Stmt], into: &mut BTreeSet<String>) {
    for st in stmts {
        match st.kind.as_str() {
            "for" => {
                if let Some(c) = &st.counter {
                    into.insert(c.clone());
                }
                collect_for_counters(&st.body, into);
            }
            "while" => collect_for_counters(&st.body, into),
            "if" => {
                collect_for_counters(&st.then, into);
                for arm in &st.elif {
                    collect_for_counters(&arm.body, into);
                }
                if let Some(eb) = &st.else_body {
                    collect_for_counters(eb, into);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn dry_run_skips_protected_and_counts() {
        let report = migrate_v010(
            &repo(),
            &MigrateOptions {
                dry_run: true,
                only: vec![],
            },
        )
        .unwrap();
        assert!(report.converted >= 20, "converted={}", report.converted);
        assert!(
            report.skipped_protected >= 7,
            "skipped={}",
            report.skipped_protected
        );
        assert!(report.items.iter().any(|i| i.protected));
        assert!(report.items.iter().all(|i| !i.written));
    }

    #[test]
    fn convert_g04_matches_checked_in_toml() {
        let root = repo();
        let src = root.join("core/asl/v010/golden/G04-rotary-encoder-counter.toon");
        let exp = root.join("content/fixtures/golden/G04-rotary-encoder-counter.asl.toml");
        let text = fs::read_to_string(&src).unwrap();
        let got = convert_program(&text, "core/asl/v010/golden/G04-rotary-encoder-counter.toon");
        let expected = fs::read_to_string(&exp).unwrap();
        assert_eq!(got, expected);
    }

    #[test]
    fn convert_golden_subset_matches_or_documents() {
        let root = repo();
        // Non-protected goldens that must match the checked-in TOML exactly.
        let subset = [
            "G04-rotary-encoder-counter",
            "G06-adc-multi-channel-logger",
            "G07-servo-sweep-sine",
            "G20-nested-loop-matrix-scan",
            "G21-deep-call-chain-state-machine",
        ];
        for stem in subset {
            let src = root.join(format!("core/asl/v010/golden/{stem}.toon"));
            let exp = root.join(format!("content/fixtures/golden/{stem}.asl.toml"));
            let rel = format!("golden/{stem}.asl.toml");
            assert!(!is_protected(&rel), "{rel} should not be protected");
            let text = fs::read_to_string(&src).unwrap();
            let source_rel = format!("core/asl/v010/golden/{stem}.toon");
            let got = convert_program(&text, &source_rel);
            let expected = fs::read_to_string(&exp).unwrap();
            assert_eq!(got, expected, "mismatch for {stem}");
        }
    }

    #[test]
    fn protected_fixtures_not_overwritten_on_migrate() {
        let root = repo();
        let g01 = root.join("content/fixtures/golden/G01-gpio-all-modes-probe.asl.toml");
        let before = fs::read_to_string(&g01).unwrap();
        let _ = migrate_v010(
            &root,
            &MigrateOptions {
                dry_run: true,
                only: vec!["G01".into()],
            },
        )
        .unwrap();
        // dry-run never writes; also G01 is protected so even a write pass would skip.
        let after = fs::read_to_string(&g01).unwrap();
        assert_eq!(before, after);
        assert!(is_protected("golden/G01-gpio-all-modes-probe.asl.toml"));
    }

    #[test]
    fn drops_aslversion_and_maps_edge_modes() {
        let sample = r#"# METADATA:
project_name: sample
version: 1
editor: dendriForge
author: test
ASLversion: 0.1.0

## 1. HARDWARE INTERFACE:
hardware[0|]{pin|type|io_mode|label|target_role}:

## 2. SYSTEM DATA:
data[0|]{name|type|scope|init_value|unit}:

## 3. INITIALIZATION SYSTEM:
setup[1]:
  - CFG_ATTACH_INTERRUPT(2, isr, RISING)

## 4. BEHAVIORAL LOGIC (LOOP):
loop[1]:
  - x = AND(a, b)

## 5. PROCEDURAL FUNCTIONS:
"#;
        let out = convert_program(sample, "core/asl/v010/golden/sample.toon");
        assert!(!out.contains("ASLversion"));
        assert!(
            !out.lines().any(|l| l.trim_start().starts_with("version =")),
            "legacy document version= must not appear"
        );
        assert!(out.contains(r#"asl_version = "0.0.1""#));
        assert!(out.contains(r#"{ kind = "string", value = "RISING" }"#));
        assert!(out.contains("a AND b") || out.contains(r#"text = "a AND b""#));
    }
}
