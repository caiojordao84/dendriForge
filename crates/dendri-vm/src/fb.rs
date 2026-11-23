//! Exact IEC 61131-3 Function Block state machines (`02-simulation-and-electrical-engine.md` §2.3).
//!
//! Edge cases explicitly verified:
//! - `TON`: `PT <= 0` activates immediately on `IN = true`; resets `ET = 0` when `IN = false`.
//! - `TOF`: `PT <= 0` deactivates immediately on `IN = false` (fixing v1's off-delay bug);
//!   starts with `Q = false` before `IN` has ever been `true`.
//! - `TP`: non-retriggerable while running; holds `ET = PT` while `IN` remains `true` post-pulse,
//!   and **resets `ET = 0` as soon as `IN = false` post-pulse** (exact IEC 61131-3!).
//! - `CTD`: starts with `CV = 0`, `Q = true` until `LD` loads `PV` (`Q = (CV <= 0)`).
//! - `CTUD`: `R` takes priority over `LD`; `QU = (CV >= PV)`, `QD = (CV <= 0)`.
//! - `R_TRIG` / `F_TRIG` / `SR` (set-dominant) / `RS` (reset-dominant).

use crate::RuntimeError;
use crate::io::BoardIoInterface;
use crate::value::Value;
use dendri_ast::FbKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Dedicated typed runtime state per Function Block instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FbState {
    Ton {
        in_val: bool,
        pt_us: i64,
        q: bool,
        et_us: i64,
    },
    Tof {
        in_val: bool,
        pt_us: i64,
        q: bool,
        et_us: i64,
        seen_true: bool,
    },
    Tp {
        in_val: bool,
        prev_in: bool,
        pt_us: i64,
        q: bool,
        et_us: i64,
        running: bool,
    },
    Ctu {
        prev_cu: bool,
        pv: i32,
        cv: i32,
        q: bool,
    },
    Ctd {
        prev_cd: bool,
        pv: i32,
        cv: i32,
        q: bool,
    },
    Ctud {
        prev_cu: bool,
        prev_cd: bool,
        pv: i32,
        cv: i32,
        qu: bool,
        qd: bool,
    },
    RTrig {
        prev_clk: bool,
        q: bool,
    },
    FTrig {
        prev_clk: bool,
        q: bool,
    },
    Sr {
        q1: bool,
    },
    Rs {
        q1: bool,
    },
    UartSend {
        done: bool,
        err: bool,
    },
    UartRecv {
        ndr: bool,
        data: String,
        err: bool,
    },
    CommStub {
        done: bool,
        ndr: bool,
        rx: u8,
        err: bool,
    },
    User {
        name: String,
        fields: BTreeMap<String, Value>,
    },
}

impl FbState {
    /// Creates an IEC 61131-3 compliant initial state for a built-in [`FbKind`].
    #[must_use]
    pub fn from_kind(kind: &FbKind) -> Self {
        match kind {
            FbKind::Ton => Self::Ton {
                in_val: false,
                pt_us: 0,
                q: false,
                et_us: 0,
            },
            FbKind::Tof => Self::Tof {
                in_val: false,
                pt_us: 0,
                q: false,
                et_us: 0,
                seen_true: false,
            },
            FbKind::Tp => Self::Tp {
                in_val: false,
                prev_in: false,
                pt_us: 0,
                q: false,
                et_us: 0,
                running: false,
            },
            FbKind::Ctu => Self::Ctu {
                prev_cu: false,
                pv: 0,
                cv: 0,
                q: false,
            },
            FbKind::Ctd => Self::Ctd {
                prev_cd: false,
                pv: 0,
                cv: 0,
                q: true, // Exact IEC 61131-3: CV = 0 <= 0 => Q = TRUE until LD loads PV > 0
            },
            FbKind::Ctud => Self::Ctud {
                prev_cu: false,
                prev_cd: false,
                pv: 0,
                cv: 0,
                qu: true,
                qd: true,
            },
            FbKind::RTrig => Self::RTrig {
                prev_clk: false,
                q: false,
            },
            FbKind::FTrig => Self::FTrig {
                prev_clk: false,
                q: false,
            },
            FbKind::Sr => Self::Sr { q1: false },
            FbKind::Rs => Self::Rs { q1: false },
            FbKind::UartSend => Self::UartSend {
                done: false,
                err: false,
            },
            FbKind::UartRecv => Self::UartRecv {
                ndr: false,
                data: String::new(),
                err: false,
            },
            FbKind::I2cXfer | FbKind::SpiXfer | FbKind::MbClient | FbKind::MbServer => {
                Self::CommStub {
                    done: false,
                    ndr: false,
                    rx: 0,
                    err: false,
                }
            }
            FbKind::User { fb_name } => Self::User {
                name: fb_name.clone(),
                fields: BTreeMap::new(),
            },
        }
    }

    /// Creates a user-defined FB state initialized with typed fields.
    #[must_use]
    pub const fn user(name: String, fields: BTreeMap<String, Value>) -> Self {
        Self::User { name, fields }
    }

    /// Reads an output/state field (`.Q`, `.ET`, `.CV`, `.QU`, `.QD`, `.Q1`, `.DONE`, `.NDR`, `.DATA`, `.ERR`).
    pub fn get_field(&self, field: &str) -> Result<Value, RuntimeError> {
        match (self, field) {
            (Self::Ton { q, .. } | Self::Tof { q, .. } | Self::Tp { q, .. }, "Q")
            | (Self::Ctu { q, .. } | Self::Ctd { q, .. }, "Q")
            | (Self::RTrig { q, .. } | Self::FTrig { q, .. }, "Q") => Ok(Value::Bool(*q)),
            (Self::Ton { et_us, .. } | Self::Tof { et_us, .. } | Self::Tp { et_us, .. }, "ET") => {
                Ok(Value::TimeUs(*et_us))
            }
            (Self::Ton { pt_us, .. } | Self::Tof { pt_us, .. } | Self::Tp { pt_us, .. }, "PT") => {
                Ok(Value::TimeUs(*pt_us))
            }
            (
                Self::Ton { in_val, .. } | Self::Tof { in_val, .. } | Self::Tp { in_val, .. },
                "IN",
            ) => Ok(Value::Bool(*in_val)),
            (Self::Ctu { cv, .. } | Self::Ctd { cv, .. } | Self::Ctud { cv, .. }, "CV") => {
                Ok(Value::Dint(*cv))
            }
            (Self::Ctu { pv, .. } | Self::Ctd { pv, .. } | Self::Ctud { pv, .. }, "PV") => {
                Ok(Value::Dint(*pv))
            }
            (Self::Ctud { qu, .. }, "QU") => Ok(Value::Bool(*qu)),
            (Self::Ctud { qd, .. }, "QD") => Ok(Value::Bool(*qd)),
            (Self::Sr { q1 } | Self::Rs { q1 }, "Q1") => Ok(Value::Bool(*q1)),
            (Self::UartSend { done, .. } | Self::CommStub { done, .. }, "DONE") => {
                Ok(Value::Bool(*done))
            }
            (
                Self::UartSend { err, .. }
                | Self::UartRecv { err, .. }
                | Self::CommStub { err, .. },
                "ERR",
            ) => Ok(Value::Bool(*err)),
            (Self::UartRecv { ndr, .. } | Self::CommStub { ndr, .. }, "NDR") => {
                Ok(Value::Bool(*ndr))
            }
            (Self::UartRecv { data, .. }, "DATA") => Ok(Value::String {
                text: data.clone(),
                max_len: 255,
            }),
            (Self::CommStub { rx, .. }, "RX") => Ok(Value::Byte(*rx)),
            (Self::User { fields, .. }, f) => fields
                .get(f)
                .cloned()
                .ok_or_else(|| RuntimeError::UnknownField(f.to_owned())),
            _ => Err(RuntimeError::UnknownField(field.to_owned())),
        }
    }

    /// Primary boolean power-flow output propagated out of a Ladder FB box (`04` §5.1).
    #[must_use]
    pub const fn ladder_out_rail(&self) -> bool {
        match self {
            Self::Ton { q, .. }
            | Self::Tof { q, .. }
            | Self::Tp { q, .. }
            | Self::Ctu { q, .. }
            | Self::Ctd { q, .. }
            | Self::RTrig { q, .. }
            | Self::FTrig { q, .. } => *q,
            Self::Ctud { qu, .. } => *qu,
            Self::Sr { q1 } | Self::Rs { q1 } => *q1,
            Self::UartSend { done, .. } | Self::CommStub { done, .. } => *done,
            Self::UartRecv { ndr, .. } => *ndr,
            Self::User { .. } => true,
        }
    }

    /// Advances active IEC timer elapsed time by `dt_us` (used at PLC scan start & timer ticks).
    pub fn advance_time_us(&mut self, dt_us: i64) {
        if dt_us <= 0 {
            return;
        }
        match self {
            Self::Ton {
                in_val,
                pt_us,
                q,
                et_us,
            } => {
                if *in_val {
                    if *pt_us <= 0 {
                        *et_us = 0;
                        *q = true;
                    } else {
                        *et_us = et_us.saturating_add(dt_us).min(*pt_us);
                        *q = *et_us >= *pt_us;
                    }
                }
            }
            Self::Tof {
                in_val,
                pt_us,
                q,
                et_us,
                seen_true,
            } => {
                if !*in_val && *seen_true {
                    if *pt_us <= 0 {
                        *et_us = 0;
                        *q = false;
                    } else {
                        *et_us = et_us.saturating_add(dt_us).min(*pt_us);
                        *q = *et_us < *pt_us;
                    }
                }
            }
            Self::Tp {
                in_val,
                pt_us,
                q,
                et_us,
                running,
                ..
            } => {
                if *running {
                    *et_us = et_us.saturating_add(dt_us).min(*pt_us);
                    if *et_us >= *pt_us {
                        *running = false;
                        *q = false;
                        if !*in_val {
                            *et_us = 0;
                        }
                    }
                }
            }
            Self::Ctu { .. }
            | Self::Ctd { .. }
            | Self::Ctud { .. }
            | Self::RTrig { .. }
            | Self::FTrig { .. }
            | Self::Sr { .. }
            | Self::Rs { .. }
            | Self::UartSend { .. }
            | Self::UartRecv { .. }
            | Self::CommStub { .. }
            | Self::User { .. } => {}
        }
    }

    /// Invokes a built-in Function Block with its evaluated named inputs and elapsed `dt_us`.
    pub fn invoke(
        &mut self,
        inputs: &BTreeMap<String, Value>,
        dt_us: i64,
        io: &mut dyn BoardIoInterface,
    ) -> Result<(), RuntimeError> {
        let get_bool = |name: &str| -> Result<bool, RuntimeError> {
            match inputs.get(name) {
                Some(v) => v.as_bool(),
                None => Ok(false),
            }
        };
        let get_time = |name: &str, default: i64| -> Result<i64, RuntimeError> {
            match inputs.get(name) {
                Some(v) => v.as_time_us(),
                None => Ok(default),
            }
        };
        let get_dint = |name: &str, default: i32| -> Result<i32, RuntimeError> {
            match inputs.get(name) {
                Some(v) => Ok(v.as_i128()? as i32),
                None => Ok(default),
            }
        };

        match self {
            Self::Ton {
                in_val,
                pt_us,
                q,
                et_us,
            } => {
                let inp = get_bool("IN")?;
                *pt_us = get_time("PT", *pt_us)?;
                *in_val = inp;
                if !inp {
                    *q = false;
                    *et_us = 0;
                } else if *pt_us <= 0 {
                    *et_us = 0;
                    *q = true;
                } else {
                    *et_us = et_us.saturating_add(dt_us.max(0)).min(*pt_us);
                    *q = *et_us >= *pt_us;
                }
            }
            Self::Tof {
                in_val,
                pt_us,
                q,
                et_us,
                seen_true,
            } => {
                let inp = get_bool("IN")?;
                *pt_us = get_time("PT", *pt_us)?;
                *in_val = inp;
                if inp {
                    *seen_true = true;
                    *q = true;
                    *et_us = 0;
                } else if !*seen_true || *pt_us <= 0 {
                    *q = false;
                    *et_us = 0;
                } else {
                    *et_us = et_us.saturating_add(dt_us.max(0)).min(*pt_us);
                    *q = *et_us < *pt_us;
                }
            }
            Self::Tp {
                in_val,
                prev_in,
                pt_us,
                q,
                et_us,
                running,
            } => {
                let inp = get_bool("IN")?;
                *pt_us = get_time("PT", *pt_us)?;
                let rising = inp && !*prev_in;
                *prev_in = inp;
                *in_val = inp;

                if *running {
                    *et_us = et_us.saturating_add(dt_us.max(0)).min(*pt_us);
                    if *et_us >= *pt_us {
                        *running = false;
                        *q = false;
                        if !inp {
                            *et_us = 0;
                        }
                    } else {
                        *q = true;
                    }
                } else if rising && *pt_us > 0 {
                    *running = true;
                    *q = true;
                    *et_us = 0;
                } else if !inp {
                    // Exact IEC 61131-3 TP rule: ET resets to 0 once IN = FALSE post-pulse!
                    *q = false;
                    *et_us = 0;
                } else {
                    *q = false;
                }
            }
            Self::Ctu { prev_cu, pv, cv, q } => {
                let cu = get_bool("CU")?;
                let r = get_bool("R")?;
                *pv = get_dint("PV", *pv)?;
                if r {
                    *cv = 0;
                } else if cu && !*prev_cu {
                    *cv = cv.wrapping_add(1);
                }
                *prev_cu = cu;
                *q = *cv >= *pv;
            }
            Self::Ctd { prev_cd, pv, cv, q } => {
                let cd = get_bool("CD")?;
                let ld = get_bool("LD")?;
                *pv = get_dint("PV", *pv)?;
                if ld {
                    *cv = *pv;
                } else if cd && !*prev_cd {
                    *cv = cv.wrapping_sub(1);
                }
                *prev_cd = cd;
                *q = *cv <= 0;
            }
            Self::Ctud {
                prev_cu,
                prev_cd,
                pv,
                cv,
                qu,
                qd,
            } => {
                let cu = get_bool("CU")?;
                let cd = get_bool("CD")?;
                let r = get_bool("R")?;
                let ld = get_bool("LD")?;
                *pv = get_dint("PV", *pv)?;
                if r {
                    *cv = 0;
                } else if ld {
                    *cv = *pv;
                } else {
                    if cu && !*prev_cu {
                        *cv = cv.wrapping_add(1);
                    }
                    if cd && !*prev_cd {
                        *cv = cv.wrapping_sub(1);
                    }
                }
                *prev_cu = cu;
                *prev_cd = cd;
                *qu = *cv >= *pv;
                *qd = *cv <= 0;
            }
            Self::RTrig { prev_clk, q } => {
                let clk = get_bool("CLK")?;
                *q = clk && !*prev_clk;
                *prev_clk = clk;
            }
            Self::FTrig { prev_clk, q } => {
                let clk = get_bool("CLK")?;
                *q = !clk && *prev_clk;
                *prev_clk = clk;
            }
            Self::Sr { q1 } => {
                let s1 = get_bool("S1")?;
                let r = get_bool("R")?;
                *q1 = s1 || (!r && *q1);
            }
            Self::Rs { q1 } => {
                let s = get_bool("S")?;
                let r1 = get_bool("R1")?;
                *q1 = !r1 && (s || *q1);
            }
            Self::UartSend { done, err } => {
                let req = get_bool("REQ")?;
                if req {
                    let payload = inputs
                        .get("DATA")
                        .map(Value::as_str)
                        .transpose()?
                        .unwrap_or("");
                    let ok = io.uart_send("uart0", payload);
                    *done = ok;
                    *err = !ok;
                } else {
                    *done = false;
                    *err = false;
                }
            }
            Self::UartRecv { ndr, data, err } => {
                let en_r = get_bool("EN_R")?;
                if en_r {
                    if let Some(msg) = io.uart_recv("uart0") {
                        *ndr = true;
                        *data = msg;
                        *err = false;
                    } else {
                        *ndr = false;
                    }
                } else {
                    *ndr = false;
                }
            }
            Self::CommStub { done, .. } => {
                let req = get_bool("REQ").or_else(|_| get_bool("ENABLE"))?;
                *done = req;
            }
            Self::User { fields, .. } => {
                for (k, v) in inputs {
                    fields.insert(k.clone(), v.clone());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::MemoryBoardIo;

    fn map(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    #[test]
    fn ton_exact_delay_and_zero_pt_edge_case() {
        let mut io = MemoryBoardIo::default();
        let mut ton = FbState::from_kind(&FbKind::Ton);

        ton.invoke(
            &map(&[("IN", Value::Bool(true)), ("PT", Value::TimeUs(10_000))]),
            0,
            &mut io,
        )
        .unwrap();
        assert_eq!(ton.get_field("Q").unwrap(), Value::Bool(false));
        assert_eq!(ton.get_field("ET").unwrap(), Value::TimeUs(0));

        ton.invoke(
            &map(&[("IN", Value::Bool(true)), ("PT", Value::TimeUs(10_000))]),
            4_000,
            &mut io,
        )
        .unwrap();
        assert_eq!(ton.get_field("Q").unwrap(), Value::Bool(false));
        assert_eq!(ton.get_field("ET").unwrap(), Value::TimeUs(4_000));

        ton.invoke(
            &map(&[("IN", Value::Bool(true)), ("PT", Value::TimeUs(10_000))]),
            8_000,
            &mut io,
        )
        .unwrap();
        assert_eq!(ton.get_field("Q").unwrap(), Value::Bool(true));
        assert_eq!(ton.get_field("ET").unwrap(), Value::TimeUs(10_000));

        // Dropping IN resets Q and ET immediately
        ton.invoke(
            &map(&[("IN", Value::Bool(false)), ("PT", Value::TimeUs(10_000))]),
            1_000,
            &mut io,
        )
        .unwrap();
        assert_eq!(ton.get_field("Q").unwrap(), Value::Bool(false));
        assert_eq!(ton.get_field("ET").unwrap(), Value::TimeUs(0));

        // PT = 0 activates immediately
        ton.invoke(
            &map(&[("IN", Value::Bool(true)), ("PT", Value::TimeUs(0))]),
            0,
            &mut io,
        )
        .unwrap();
        assert_eq!(ton.get_field("Q").unwrap(), Value::Bool(true));
    }

    #[test]
    fn tof_exact_pt_zero_and_initial_false() {
        let mut io = MemoryBoardIo::default();
        let mut tof = FbState::from_kind(&FbKind::Tof);

        // Before IN ever went true, TOF stays Q = false
        tof.invoke(
            &map(&[("IN", Value::Bool(false)), ("PT", Value::TimeUs(5_000))]),
            1_000,
            &mut io,
        )
        .unwrap();
        assert_eq!(tof.get_field("Q").unwrap(), Value::Bool(false));

        // IN true arms TOF with Q = true, ET = 0
        tof.invoke(
            &map(&[("IN", Value::Bool(true)), ("PT", Value::TimeUs(5_000))]),
            1_000,
            &mut io,
        )
        .unwrap();
        assert_eq!(tof.get_field("Q").unwrap(), Value::Bool(true));

        // IN false starts off-delay countdown
        tof.invoke(
            &map(&[("IN", Value::Bool(false)), ("PT", Value::TimeUs(5_000))]),
            3_000,
            &mut io,
        )
        .unwrap();
        assert_eq!(tof.get_field("Q").unwrap(), Value::Bool(true));
        assert_eq!(tof.get_field("ET").unwrap(), Value::TimeUs(3_000));

        tof.invoke(
            &map(&[("IN", Value::Bool(false)), ("PT", Value::TimeUs(5_000))]),
            2_000,
            &mut io,
        )
        .unwrap();
        assert_eq!(tof.get_field("Q").unwrap(), Value::Bool(false));
        assert_eq!(tof.get_field("ET").unwrap(), Value::TimeUs(5_000));

        // PT = 0 drops Q immediately on falling edge
        tof.invoke(
            &map(&[("IN", Value::Bool(true)), ("PT", Value::TimeUs(0))]),
            0,
            &mut io,
        )
        .unwrap();
        assert_eq!(tof.get_field("Q").unwrap(), Value::Bool(true));
        tof.invoke(
            &map(&[("IN", Value::Bool(false)), ("PT", Value::TimeUs(0))]),
            0,
            &mut io,
        )
        .unwrap();
        assert_eq!(tof.get_field("Q").unwrap(), Value::Bool(false));
    }

    #[test]
    fn tp_pulse_resets_et_when_in_false_post_pulse() {
        let mut io = MemoryBoardIo::default();
        let mut tp = FbState::from_kind(&FbKind::Tp);

        // Rising edge starts pulse
        tp.invoke(
            &map(&[("IN", Value::Bool(true)), ("PT", Value::TimeUs(10_000))]),
            0,
            &mut io,
        )
        .unwrap();
        assert_eq!(tp.get_field("Q").unwrap(), Value::Bool(true));
        assert_eq!(tp.get_field("ET").unwrap(), Value::TimeUs(0));

        // Even if IN goes false mid-pulse, pulse continues until PT
        tp.invoke(
            &map(&[("IN", Value::Bool(false)), ("PT", Value::TimeUs(10_000))]),
            6_000,
            &mut io,
        )
        .unwrap();
        assert_eq!(tp.get_field("Q").unwrap(), Value::Bool(true));
        assert_eq!(tp.get_field("ET").unwrap(), Value::TimeUs(6_000));

        // Reaches PT while IN is false -> Q = false AND ET resets to 0 immediately!
        tp.invoke(
            &map(&[("IN", Value::Bool(false)), ("PT", Value::TimeUs(10_000))]),
            4_000,
            &mut io,
        )
        .unwrap();
        assert_eq!(tp.get_field("Q").unwrap(), Value::Bool(false));
        assert_eq!(tp.get_field("ET").unwrap(), Value::TimeUs(0));

        // If IN stays true past PT, ET holds at PT until IN drops to false!
        tp.invoke(
            &map(&[("IN", Value::Bool(true)), ("PT", Value::TimeUs(5_000))]),
            0,
            &mut io,
        )
        .unwrap();
        tp.invoke(
            &map(&[("IN", Value::Bool(true)), ("PT", Value::TimeUs(5_000))]),
            5_000,
            &mut io,
        )
        .unwrap();
        assert_eq!(tp.get_field("Q").unwrap(), Value::Bool(false));
        assert_eq!(tp.get_field("ET").unwrap(), Value::TimeUs(5_000));
        // Now drop IN to false -> ET MUST reset to 0!
        tp.invoke(
            &map(&[("IN", Value::Bool(false)), ("PT", Value::TimeUs(5_000))]),
            1_000,
            &mut io,
        )
        .unwrap();
        assert_eq!(tp.get_field("ET").unwrap(), Value::TimeUs(0));
    }

    #[test]
    fn ctd_starts_at_cv_zero_q_true_until_ld_and_ctud_r_dominates_ld() {
        let mut io = MemoryBoardIo::default();
        let mut ctd = FbState::from_kind(&FbKind::Ctd);
        assert_eq!(ctd.get_field("CV").unwrap(), Value::Dint(0));
        assert_eq!(ctd.get_field("Q").unwrap(), Value::Bool(true));

        ctd.invoke(
            &map(&[("LD", Value::Bool(true)), ("PV", Value::Dint(2))]),
            0,
            &mut io,
        )
        .unwrap();
        assert_eq!(ctd.get_field("CV").unwrap(), Value::Dint(2));
        assert_eq!(ctd.get_field("Q").unwrap(), Value::Bool(false));

        for expected in [1, 0] {
            ctd.invoke(
                &map(&[("CD", Value::Bool(true)), ("PV", Value::Dint(2))]),
                0,
                &mut io,
            )
            .unwrap();
            ctd.invoke(
                &map(&[("CD", Value::Bool(false)), ("PV", Value::Dint(2))]),
                0,
                &mut io,
            )
            .unwrap();
            assert_eq!(ctd.get_field("CV").unwrap(), Value::Dint(expected));
        }
        assert_eq!(ctd.get_field("Q").unwrap(), Value::Bool(true));

        let mut ctud = FbState::from_kind(&FbKind::Ctud);
        // R dominates LD
        ctud.invoke(
            &map(&[
                ("R", Value::Bool(true)),
                ("LD", Value::Bool(true)),
                ("PV", Value::Dint(5)),
            ]),
            0,
            &mut io,
        )
        .unwrap();
        assert_eq!(ctud.get_field("CV").unwrap(), Value::Dint(0));
        assert_eq!(ctud.get_field("QD").unwrap(), Value::Bool(true));
        assert_eq!(ctud.get_field("QU").unwrap(), Value::Bool(false));
    }

    #[test]
    fn triggers_and_dominant_bistables() {
        let mut io = MemoryBoardIo::default();
        let mut r = FbState::from_kind(&FbKind::RTrig);
        r.invoke(&map(&[("CLK", Value::Bool(true))]), 0, &mut io)
            .unwrap();
        assert_eq!(r.get_field("Q").unwrap(), Value::Bool(true));
        r.invoke(&map(&[("CLK", Value::Bool(true))]), 0, &mut io)
            .unwrap();
        assert_eq!(r.get_field("Q").unwrap(), Value::Bool(false));

        let mut f = FbState::from_kind(&FbKind::FTrig);
        f.invoke(&map(&[("CLK", Value::Bool(true))]), 0, &mut io)
            .unwrap();
        assert_eq!(f.get_field("Q").unwrap(), Value::Bool(false));
        f.invoke(&map(&[("CLK", Value::Bool(false))]), 0, &mut io)
            .unwrap();
        assert_eq!(f.get_field("Q").unwrap(), Value::Bool(true));

        // SR is Set-dominant; RS is Reset-dominant
        let mut sr = FbState::from_kind(&FbKind::Sr);
        sr.invoke(
            &map(&[("S1", Value::Bool(true)), ("R", Value::Bool(true))]),
            0,
            &mut io,
        )
        .unwrap();
        assert_eq!(sr.get_field("Q1").unwrap(), Value::Bool(true));

        let mut rs = FbState::from_kind(&FbKind::Rs);
        rs.invoke(
            &map(&[("S", Value::Bool(true)), ("R1", Value::Bool(true))]),
            0,
            &mut io,
        )
        .unwrap();
        assert_eq!(rs.get_field("Q1").unwrap(), Value::Bool(false));
    }
}
