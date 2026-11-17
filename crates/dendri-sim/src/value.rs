//! Runtime values for the scan-cycle simulator.

use dendri_ast::DataType;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Uint(u64),
    Real(f64),
    String(String),
    TimeMs(u64),
    Void,
}

impl Value {
    pub fn parse_init(init: &str, ty: DataType) -> Self {
        let s = init.trim();
        match ty {
            DataType::Bool => {
                let v = s.eq_ignore_ascii_case("TRUE") || s == "1";
                Value::Bool(v)
            }
            DataType::Time => {
                Value::TimeMs(crate::parse_time_ms(s).unwrap_or(0))
            }
            DataType::String => {
                let t = s.trim_matches('"');
                Value::String(t.to_string())
            }
            DataType::Real | DataType::Lreal => {
                Value::Real(s.parse().unwrap_or(0.0))
            }
            DataType::Int | DataType::Dint => {
                Value::Int(s.parse().unwrap_or(0))
            }
            DataType::Uint
            | DataType::Byte
            | DataType::Word
            | DataType::Dword => {
                if let Ok(u) = s.parse::<u64>() {
                    Value::Uint(u)
                } else {
                    Value::Uint(0)
                }
            }
        }
    }

    pub fn as_bool(&self) -> Result<bool, String> {
        match self {
            Value::Bool(b) => Ok(*b),
            Value::Int(i) => Ok(*i != 0),
            Value::Uint(u) => Ok(*u != 0),
            other => Err(format!("expected BOOL, got {other:?}")),
        }
    }

    pub fn as_u64(&self) -> Result<u64, String> {
        match self {
            Value::Uint(u) => Ok(*u),
            Value::Int(i) if *i >= 0 => Ok(*i as u64),
            Value::Real(r) if *r >= 0.0 => Ok(*r as u64),
            Value::TimeMs(ms) => Ok(*ms),
            Value::Bool(b) => Ok(if *b { 1 } else { 0 }),
            other => Err(format!("expected UINT, got {other:?}")),
        }
    }

    pub fn as_i64(&self) -> Result<i64, String> {
        match self {
            Value::Int(i) => Ok(*i),
            Value::Uint(u) => Ok(*u as i64),
            Value::TimeMs(ms) => Ok(*ms as i64),
            Value::Bool(b) => Ok(if *b { 1 } else { 0 }),
            other => Err(format!("expected INT, got {other:?}")),
        }
    }

    pub fn as_f64(&self) -> Result<f64, String> {
        match self {
            Value::Real(r) => Ok(*r),
            Value::Int(i) => Ok(*i as f64),
            Value::Uint(u) => Ok(*u as f64),
            Value::TimeMs(ms) => Ok(*ms as f64),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            other => Err(format!("expected REAL, got {other:?}")),
        }
    }

    pub fn as_string(&self) -> Result<String, String> {
        match self {
            Value::String(s) => Ok(s.clone()),
            Value::Bool(b) => Ok(b.to_string()),
            Value::Int(i) => Ok(i.to_string()),
            Value::Uint(u) => Ok(u.to_string()),
            Value::Real(r) => Ok(format!("{r}")),
            Value::TimeMs(ms) => Ok(format!("{ms}ms")),
            Value::Void => Err("expected STRING, got void".into()),
        }
    }

    pub fn to_trace(&self) -> String {
        match self {
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Uint(u) => u.to_string(),
            Value::Real(r) => format!("{r}"),
            Value::String(s) => format!("\"{s}\""),
            Value::TimeMs(ms) => format!("{ms}ms"),
            Value::Void => "void".into(),
        }
    }
}
