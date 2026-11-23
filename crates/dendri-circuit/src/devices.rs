//! Complete Tier 1, Tier 2, and Tier 3 electrical device models and SPICE nonlinear companion
//! evaluators (`02-simulation-and-electrical-engine.md` §3.2).
//!
//! All transcendental math uses pure-Rust `libm` for cross-platform determinism (`02` §2.1).

use serde::{Deserialize, Serialize};

/// Thermal voltage $V_T = kT/q$ at $T = 300\text{ K}$ (in Volts).
pub const VT_300K: f64 = 0.025_851_99;

/// Minimum conductance bleed (`Gmin = 1e-12 S`) preventing singular matrices (`02` §3.1).
pub const DEFAULT_GMIN: f64 = 1e-12;

/// SPICE3 `pnjlim` junction voltage limiter preventing exponential overflow in Newton-Raphson
/// iterations (`02` §3.2).
#[must_use]
pub fn pnjlim(v_new: f64, v_old: f64, vt: f64, v_crit: f64) -> f64 {
    if v_new > v_crit && libm::fabs(v_new - v_old) > 2.0 * vt {
        if v_old > 0.0 {
            let arg = 1.0 + (v_new - v_old) / vt;
            if arg > 0.0 {
                v_old + vt * libm::log(arg)
            } else {
                v_crit
            }
        } else {
            vt * libm::log((v_new / vt).max(1e-12))
        }
    } else {
        v_new
    }
}

/// Evaluates junction critical voltage $V_{\text{crit}} = V_T \ln(V_T / (\sqrt{2} I_S))$.
#[must_use]
pub fn junction_vcrit(vt: f64, i_s: f64) -> f64 {
    let denom = core::f64::consts::SQRT_2 * i_s.max(1e-30);
    let ratio = (vt / denom).max(1.0);
    vt * libm::log(ratio)
}

/// Linearized 2-terminal nonlinear junction companion stamp:
/// $I(v) \approx g_{\text{eq}} \cdot v + i_{\text{eq}}$ where $i_{\text{eq}} = i(v_0) - g_{\text{eq}} \cdot v_0$.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JunctionStamp {
    pub current_a: f64,
    pub conductance_s: f64,
    pub i_eq_a: f64,
}

/// Evaluates Shockley exponential diode + optional reverse Zener breakdown companion model
/// at junction voltage `v_j` (anode minus cathode).
#[must_use]
pub fn eval_diode_junction(p: &DiodeParams, v_j: f64, gmin: f64) -> JunctionStamp {
    let n_vt = (p.n * VT_300K).max(1e-4);
    let i_s = p.i_s.max(1e-30);

    // Forward Shockley branch: I_f = I_s * (exp(v_j / (N * V_T)) - 1)
    let arg_f = (v_j / n_vt).clamp(-80.0, 80.0);
    let exp_f = libm::exp(arg_f);
    let i_fwd = i_s * (exp_f - 1.0);
    let g_fwd = (i_s / n_vt) * exp_f;

    // Reverse breakdown branch (Zener / LED reverse breakdown at -v_br)
    let (i_br, g_br) = if p.v_br > 0.0 && p.v_br.is_finite() {
        let vt_br = (p.n_br * VT_300K).max(1e-4);
        let arg_br = ((-v_j - p.v_br) / vt_br).clamp(-80.0, 80.0);
        let exp_br = libm::exp(arg_br);
        let i_bv = p.i_bv.max(1e-12);
        (-i_bv * exp_br, (i_bv / vt_br) * exp_br)
    } else {
        (0.0, 0.0)
    };

    let current_a = i_fwd + i_br + gmin * v_j;
    let conductance_s = g_fwd + g_br + gmin;
    let i_eq_a = current_a - conductance_s * v_j;

    JunctionStamp {
        current_a,
        conductance_s,
        i_eq_a,
    }
}

/// Linearized Ebers-Moll transport BJT stamp at operating point `(v_be, v_bc)` (signed for polarity).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BjtStamp {
    pub g_be: f64,
    pub i_be_eq: f64,
    pub g_bc: f64,
    pub i_bc_eq: f64,
    pub gm_f: f64,
    pub gm_r: f64,
    pub i_ct_eq: f64,
    pub i_c: f64,
    pub i_b: f64,
    pub i_e: f64,
}

/// Evaluates Ebers-Moll transport BJT model (with Early voltage $V_{AF}$) at `(v_be, v_bc)`
/// where `v_be = sign * (V_B - V_E)` and `v_bc = sign * (V_B - V_C)`.
#[must_use]
pub fn eval_ebers_moll(p: &BjtParams, v_be: f64, v_bc: f64, gmin: f64) -> BjtStamp {
    let vt = VT_300K;
    let i_s = p.i_s.max(1e-24);
    let bf = p.beta_f.max(1.0);
    let br = p.beta_r.max(0.1);

    let exp_be = libm::exp((v_be / vt).clamp(-80.0, 80.0));
    let exp_bc = libm::exp((v_bc / vt).clamp(-80.0, 80.0));

    let i_f = i_s * (exp_be - 1.0);
    let g_f = (i_s / vt) * exp_be;
    let i_r = i_s * (exp_bc - 1.0);
    let g_r = (i_s / vt) * exp_bc;

    let (q_early, dq_dvbc) = if p.v_af > 0.0 && p.v_af.is_finite() {
        let raw = 1.0 - v_bc / p.v_af;
        if raw > 0.1 {
            (raw, -1.0 / p.v_af)
        } else {
            (0.1, 0.0)
        }
    } else {
        (1.0, 0.0)
    };

    let i_be = i_f / bf + gmin * v_be;
    let g_be = g_f / bf + gmin;
    let i_be_eq = i_be - g_be * v_be;

    let i_bc = i_r / br + gmin * v_bc;
    let g_bc = g_r / br + gmin;
    let i_bc_eq = i_bc - g_bc * v_bc;

    let i_ct = (i_f - i_r) * q_early;
    let gm_f = g_f * q_early;
    let gm_r = -g_r * q_early + (i_f - i_r) * dq_dvbc;
    let i_ct_eq = i_ct - gm_f * v_be - gm_r * v_bc;

    let sign = match p.polarity {
        BjtPolarity::Npn => 1.0,
        BjtPolarity::Pnp => -1.0,
    };
    let i_b = sign * (i_be + i_bc);
    let i_c = sign * (i_ct - i_bc);
    let i_e = -i_b - i_c;

    BjtStamp {
        g_be,
        i_be_eq,
        g_bc,
        i_bc_eq,
        gm_f,
        gm_r,
        i_ct_eq,
        i_c,
        i_b,
        i_e,
    }
}

/// Linearized Shichman-Hodges Level-1 MOSFET stamp (channel current from effective Drain to Source).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MosfetStamp {
    pub i_d: f64,
    pub g_m: f64,
    pub g_ds: f64,
    pub i_eq: f64,
}

/// Evaluates Shichman-Hodges Level-1 MOSFET model for non-negative `v_ds >= 0.0`
/// and polarity-normalized `v_gs` (`V_G - V_S` for NMOS, `V_S - V_G` for PMOS).
#[must_use]
pub fn eval_shichman_hodges(p: &MosfetParams, v_gs: f64, v_ds: f64, gmin: f64) -> MosfetStamp {
    let v_th = libm::fabs(p.v_th);
    let k_p = p.k_p.max(1e-9);
    let lambda = p.lambda.max(0.0);
    let v_ov = v_gs - v_th;

    let (i_d, g_m, g_ds) = if v_ov <= 0.0 {
        // Cutoff region
        (gmin * v_ds, 0.0, gmin)
    } else if v_ds < v_ov {
        // Linear / Triode region
        let early = 1.0 + lambda * v_ds;
        let base = k_p * (v_ov * v_ds - 0.5 * v_ds * v_ds);
        let id = base * early + gmin * v_ds;
        let gm = k_p * v_ds * early;
        let gds = k_p * (v_ov - v_ds) * early + base * lambda + gmin;
        (id, gm, gds)
    } else {
        // Saturation region
        let early = 1.0 + lambda * v_ds;
        let base = 0.5 * k_p * v_ov * v_ov;
        let id = base * early + gmin * v_ds;
        let gm = k_p * v_ov * early;
        let gds = base * lambda + gmin;
        (id, gm, gds)
    };

    let i_eq = i_d - g_m * v_gs - g_ds * v_ds;
    MosfetStamp {
        i_d,
        g_m,
        g_ds,
        i_eq,
    }
}

// ===========================================================================
// Device Parameter Structs (Tiers 1, 2, 3)
// ===========================================================================

/// Linear resistor (`A`, `B`) with rated power dissipation monitor (`I^2 R > P_rated`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResistorParams {
    pub r_ohm: f64,
    pub p_rated_w: f64,
}

impl ResistorParams {
    #[must_use]
    pub const fn new(r_ohm: f64) -> Self {
        Self {
            r_ohm,
            p_rated_w: 0.25,
        }
    }

    #[must_use]
    pub const fn with_power(r_ohm: f64, p_rated_w: f64) -> Self {
        Self { r_ohm, p_rated_w }
    }
}

/// Ideal DC voltage source (`POS`, `NEG`, optional series resistance `r_ser_ohm`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoltageSourceDcParams {
    pub voltage_v: f64,
    /// Optional internal series resistance (0.0 for ideal MNA branch voltage source).
    pub r_ser_ohm: f64,
}

impl VoltageSourceDcParams {
    #[must_use]
    pub const fn ideal(voltage_v: f64) -> Self {
        Self {
            voltage_v,
            r_ser_ohm: 0.0,
        }
    }
}

/// Ideal DC current source (`POS`, `NEG` — drives `current_a` from `POS` into `NEG` internally,
/// or out of `POS` into the external circuit: positive `current_a` flows OUT of `OUT` and INTO `IN`).
/// Terminals: `"IN"`, `"OUT"` — current `current_a` flows from `"IN"` through source to `"OUT"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurrentSourceDcParams {
    pub current_a: f64,
}

/// 3-terminal linear potentiometer (`A`, `W`, `B`) with normalized wiper position `0.0..=1.0`.
/// `R(A, W) = wiper * r_total_ohm`, `R(W, B) = (1 - wiper) * r_total_ohm` (clamped with small endstop).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PotentiometerParams {
    pub r_total_ohm: f64,
    pub wiper: f64,
}

/// Electromechanical switch / pushbutton (`A`, `B`) with realistic finite `R_on` / `R_off` (`02` §3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SwitchParams {
    pub closed: bool,
    pub r_on_ohm: f64,
    pub r_off_ohm: f64,
}

impl SwitchParams {
    #[must_use]
    pub const fn normally_open(closed: bool) -> Self {
        Self {
            closed,
            r_on_ohm: 0.01,
            r_off_ohm: 100_000_000.0,
        }
    }

    #[must_use]
    pub const fn normally_closed(pressed: bool) -> Self {
        Self {
            closed: !pressed,
            r_on_ohm: 0.01,
            r_off_ohm: 100_000_000.0,
        }
    }
}

/// Electromechanical relay (`COIL_A`, `COIL_B`, `COM`, `NO`, `NC`) with inductive coil
/// (capable of generating real flyback voltage spikes when opened without a flyback diode!)
/// and galvanically isolated changeover contacts (`02` §3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayParams {
    pub r_coil_ohm: f64,
    pub l_coil_h: f64,
    pub v_pull_in_v: f64,
    pub v_drop_out_v: f64,
    pub r_on_ohm: f64,
    pub r_off_ohm: f64,
}

impl Default for RelayParams {
    fn default() -> Self {
        Self {
            r_coil_ohm: 100.0,
            l_coil_h: 0.050, // 50 mH typical 5V/12V relay coil
            v_pull_in_v: 3.75,
            v_drop_out_v: 1.0,
            r_on_ohm: 0.01,
            r_off_ohm: 100_000_000.0,
        }
    }
}

/// Shockley exponential Diode / LED / Schottky / Zener / TVS (`A`, `K`) (`02` §3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiodeParams {
    pub i_s: f64,
    pub n: f64,
    pub r_s: f64,
    /// Reverse breakdown voltage magnitude in Volts (`> 0` activates smooth reverse breakdown).
    pub v_br: f64,
    /// Reverse breakdown knee current at `-v_br` (in A).
    pub i_bv: f64,
    /// Reverse breakdown ideality factor.
    pub n_br: f64,
    /// Maximum rated continuous forward current before burn fault (in A).
    pub i_max_a: f64,
    /// True if this diode is an optical LED (enables luminosity calculation & reverse burn fault).
    pub is_led: bool,
    /// Nominal forward current for 100% LED optical luminosity (e.g. `0.020 A`).
    pub i_nom_a: f64,
    /// Optional LED color name.
    pub color: Option<String>,
}

impl DiodeParams {
    /// Standard fast silicon switching diode (`1N4148`).
    #[must_use]
    pub fn silicon_1n4148() -> Self {
        Self {
            i_s: 2.52e-9,
            n: 1.752,
            r_s: 0.568,
            v_br: 100.0,
            i_bv: 1e-4,
            n_br: 1.0,
            i_max_a: 0.300,
            is_led: false,
            i_nom_a: 0.020,
            color: None,
        }
    }

    /// Standard 5 mm Red LED (`V_F ≈ 2.0 V @ 20 mA`, `I_max = 30 mA`, `V_Rmax = 5.0 V`).
    #[must_use]
    pub fn led_red() -> Self {
        Self {
            i_s: 1.52e-17,
            n: 2.0,
            r_s: 10.0,
            v_br: 5.0,
            i_bv: 1e-4,
            n_br: 1.5,
            i_max_a: 0.030,
            is_led: true,
            i_nom_a: 0.020,
            color: Some("red".to_owned()),
        }
    }

    /// Zener voltage regulator diode with breakdown voltage `v_z` (e.g., `5.1 V` 1N4733A).
    #[must_use]
    pub fn zener(v_z: f64) -> Self {
        Self {
            i_s: 1e-12,
            n: 1.5,
            r_s: 1.5,
            v_br: libm::fabs(v_z),
            i_bv: 0.005,
            n_br: 1.0,
            i_max_a: 0.250,
            is_led: false,
            i_nom_a: 0.020,
            color: None,
        }
    }
}

/// Linear capacitor (`A`, `B`) with optional ESR and initial voltage `v_init_v`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapacitorParams {
    pub c_f: f64,
    pub esr_ohm: f64,
    pub v_init_v: f64,
}

impl CapacitorParams {
    #[must_use]
    pub const fn ideal(c_f: f64) -> Self {
        Self {
            c_f,
            esr_ohm: 0.0,
            v_init_v: 0.0,
        }
    }
}

/// Linear inductor (`A`, `B`) with optional winding series resistance and initial current `i_init_a`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InductorParams {
    pub l_h: f64,
    pub r_ser_ohm: f64,
    pub i_init_a: f64,
}

impl InductorParams {
    #[must_use]
    pub const fn new(l_h: f64, r_ser_ohm: f64) -> Self {
        Self {
            l_h,
            r_ser_ohm,
            i_init_a: 0.0,
        }
    }
}

/// Mutually coupled two-winding transformer (`P1`, `P2`, `S1`, `S2`) with $M = k\sqrt{L_1 L_2}$.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformerParams {
    pub l1_h: f64,
    pub l2_h: f64,
    /// Coupling coefficient `0.0 <= k < 1.0` (e.g., `0.999` for tight iron-core transformer).
    pub k_coupling: f64,
    pub r1_ohm: f64,
    pub r2_ohm: f64,
}

/// Continuous sinusoidal AC voltage source (`POS`, `NEG`):
/// $v(t) = V_{\text{dc}} + V_{\text{peak}} \sin(2\pi f t + \phi)$.
/// Supports 1φ AC and 3φ AC ($0^\circ, -120^\circ, +120^\circ$).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoltageSourceAcParams {
    pub v_peak: f64,
    pub freq_hz: f64,
    pub phase_rad: f64,
    pub dc_offset: f64,
}

impl VoltageSourceAcParams {
    /// Evaluates instantaneous voltage at `time_us` using pure-Rust `libm::sin`.
    #[must_use]
    pub fn voltage_at_us(&self, time_us: u64) -> f64 {
        let t_s = (time_us as f64) * 1e-6;
        let omega_t = 2.0 * core::f64::consts::PI * self.freq_hz * t_s + self.phase_rad;
        self.dc_offset + self.v_peak * libm::sin(omega_t)
    }
}

/// BJT polarity (`Npn` or `Pnp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BjtPolarity {
    Npn,
    Pnp,
}

/// Ebers-Moll BJT parameters (`B`, `C`, `E`) (`02` §3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BjtParams {
    pub polarity: BjtPolarity,
    pub beta_f: f64,
    pub beta_r: f64,
    pub i_s: f64,
    /// Forward Early voltage in Volts (`0.0` disables Early effect).
    pub v_af: f64,
}

impl BjtParams {
    /// Classic small-signal NPN transistor (`2N2222` / `BC547` class).
    #[must_use]
    pub const fn npn_2n2222() -> Self {
        Self {
            polarity: BjtPolarity::Npn,
            beta_f: 100.0,
            beta_r: 2.0,
            i_s: 1e-14,
            v_af: 100.0,
        }
    }

    /// Classic small-signal PNP transistor (`2N2907` / `BC557` class).
    #[must_use]
    pub const fn pnp_2n2907() -> Self {
        Self {
            polarity: BjtPolarity::Pnp,
            beta_f: 100.0,
            beta_r: 2.0,
            i_s: 1e-14,
            v_af: 100.0,
        }
    }
}

/// MOSFET channel polarity (`Nmos` or `Pmos`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MosfetPolarity {
    Nmos,
    Pmos,
}

/// Shichman-Hodges Level-1 MOSFET (`G`, `D`, `S`) with optional integral body diode (`02` §3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MosfetParams {
    pub polarity: MosfetPolarity,
    pub v_th: f64,
    /// Transconductance parameter $K_P = \mu C_{ox} W/L$ (in $\text{A/V}^2$).
    pub k_p: f64,
    /// Channel-length modulation parameter $\lambda$ (in $\text{V}^{-1}$).
    pub lambda: f64,
    /// Enable integral body diode (`S -> D` for NMOS, `D -> S` for PMOS).
    pub has_body_diode: bool,
}

impl MosfetParams {
    /// Logic-level N-channel power MOSFET (`IRLZ44N` / `2N7000` class).
    #[must_use]
    pub const fn nmos_logic_level() -> Self {
        Self {
            polarity: MosfetPolarity::Nmos,
            v_th: 2.0,
            k_p: 2.0,
            lambda: 0.01,
            has_body_diode: true,
        }
    }

    /// Logic-level P-channel MOSFET.
    #[must_use]
    pub const fn pmos_logic_level() -> Self {
        Self {
            polarity: MosfetPolarity::Pmos,
            v_th: 2.0,
            k_p: 2.0,
            lambda: 0.01,
            has_body_diode: true,
        }
    }
}

/// Galvanically isolated Optocoupler (`ANODE`, `CATHODE`, `COLLECTOR`, `EMITTER`) (`02` §3.2).
/// Input LED and output phototransistor may belong to completely separate electrical islands!
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptocouplerParams {
    pub led: DiodeParams,
    /// Current Transfer Ratio ($I_C / I_F$, e.g., `1.0` = 100% for PC817).
    pub ctr: f64,
    /// Collector-emitter saturation voltage knee (e.g., `0.2 V`).
    pub v_ce_sat: f64,
    /// Off-state dark resistance across collector-emitter.
    pub r_off_ohm: f64,
}

impl Default for OptocouplerParams {
    fn default() -> Self {
        Self {
            led: DiodeParams {
                i_s: 1e-12,
                n: 1.6,
                r_s: 5.0,
                v_br: 6.0,
                i_bv: 1e-4,
                n_br: 1.5,
                i_max_a: 0.050,
                is_led: false,
                i_nom_a: 0.010,
                color: None,
            },
            ctr: 1.0,
            v_ce_sat: 0.2,
            r_off_ohm: 100_000_000.0,
        }
    }
}

/// Insulated-Gate Bipolar Transistor macromodel (`G`, `C`, `E`) (`02` §3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IgbtParams {
    pub v_ge_th: f64,
    pub v_ce_sat: f64,
    pub r_on_ohm: f64,
    pub r_off_ohm: f64,
}

impl Default for IgbtParams {
    fn default() -> Self {
        Self {
            v_ge_th: 5.0,
            v_ce_sat: 1.5,
            r_on_ohm: 0.05,
            r_off_ohm: 100_000_000.0,
        }
    }
}

/// Silicon Controlled Rectifier / Thyristor (`A`, `K`, `G`) (`02` §3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScrParams {
    pub v_gt: f64,
    pub i_hold: f64,
    pub v_on: f64,
    pub r_on_ohm: f64,
    pub r_off_ohm: f64,
    pub r_gate_ohm: f64,
}

impl Default for ScrParams {
    fn default() -> Self {
        Self {
            v_gt: 0.8,
            i_hold: 0.010,
            v_on: 1.0,
            r_on_ohm: 0.05,
            r_off_ohm: 100_000_000.0,
            r_gate_ohm: 100.0,
        }
    }
}

/// Bidirectional AC TRIAC (`MT1`, `MT2`, `G`) (`02` §3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriacParams {
    pub v_gt: f64,
    pub i_hold: f64,
    pub v_on: f64,
    pub r_on_ohm: f64,
    pub r_off_ohm: f64,
    pub r_gate_ohm: f64,
}

impl Default for TriacParams {
    fn default() -> Self {
        Self {
            v_gt: 1.0,
            i_hold: 0.015,
            v_on: 1.1,
            r_on_ohm: 0.05,
            r_off_ohm: 100_000_000.0,
            r_gate_ohm: 100.0,
        }
    }
}

/// Galvanically isolated Solid-State Relay (`CTRL_P`, `CTRL_N`, `LOAD_A`, `LOAD_B`) (`02` §3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SsrParams {
    pub v_ctrl_on: f64,
    pub v_ctrl_off: f64,
    pub r_ctrl_ohm: f64,
    pub r_on_ohm: f64,
    pub r_off_ohm: f64,
}

impl Default for SsrParams {
    fn default() -> Self {
        Self {
            v_ctrl_on: 3.0,
            v_ctrl_off: 1.0,
            r_ctrl_ohm: 1_000.0,
            r_on_ohm: 0.05,
            r_off_ohm: 100_000_000.0,
        }
    }
}

/// Full 4-quadrant H-Bridge motor driver (`VCC`, `GND`, `IN1`, `IN2`, `EN`, `OUTA`, `OUTB`) (`02` §3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HBridgeParams {
    pub r_ds_on_ohm: f64,
    pub r_off_ohm: f64,
    pub v_logic_th: f64,
}

impl Default for HBridgeParams {
    fn default() -> Self {
        Self {
            r_ds_on_ohm: 0.2,
            r_off_ohm: 10_000_000.0,
            v_logic_th: 2.0,
        }
    }
}

/// Permanent-magnet DC motor (`A`, `B`) with true armature inductance & rotational back-EMF
/// ($v = R_a i_a + L_a \frac{di_a}{dt} + K_e \omega$, $J \frac{d\omega}{dt} + B \omega = K_t i_a$) (`02` §3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DcMotorParams {
    pub r_arm_ohm: f64,
    pub l_arm_h: f64,
    pub k_e: f64,
    pub k_t: f64,
    pub inertia_j: f64,
    pub friction_b: f64,
}

impl Default for DcMotorParams {
    fn default() -> Self {
        Self {
            r_arm_ohm: 2.0,
            l_arm_h: 0.002,
            k_e: 0.05,
            k_t: 0.05,
            inertia_j: 1e-4,
            friction_b: 1e-3,
        }
    }
}

/// Unified typed circuit component specification registered in [`crate::Circuit`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ComponentSpec {
    Resistor(ResistorParams),
    VoltageSourceDc(VoltageSourceDcParams),
    CurrentSourceDc(CurrentSourceDcParams),
    Potentiometer(PotentiometerParams),
    Switch(SwitchParams),
    Relay(RelayParams),
    Diode(DiodeParams),
    Capacitor(CapacitorParams),
    Inductor(InductorParams),
    Transformer(TransformerParams),
    VoltageSourceAc(VoltageSourceAcParams),
    Bjt(BjtParams),
    Mosfet(MosfetParams),
    Optocoupler(OptocouplerParams),
    Igbt(IgbtParams),
    Scr(ScrParams),
    Triac(TriacParams),
    Ssr(SsrParams),
    HBridge(HBridgeParams),
    DcMotor(DcMotorParams),
    /// Zero-ohm breadboard strip internally tying all named ports into one canonical electrical net.
    BreadboardStrip {
        ports: Vec<String>,
    },
    /// Explicit ideal reference ground (`0 V`) tied to port `"GND"`.
    Ground,
}
