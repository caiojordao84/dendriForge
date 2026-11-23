//! Strict `#[serde(deny_unknown_fields)]` schemas for `profile.toml` (components & boards),
//! composite sub-circuits, `[provenance]`, and `review.toml` (`03-asset-and-profile-contract.md` §1.3, §4, §6).

use dendri_ast::{ChannelBand, ChannelRange, CodecKind, ExecutionKind};
use dendri_theme::PinClass;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Registered analytical primitive physics models implemented in `dendri-circuit` (`03` §7 #9).
pub const REGISTERED_PRIMITIVE_MODELS: &[&str] = &[
    "spice_resistor",
    "spice_led",
    "spice_diode",
    "spice_zener",
    "spice_switch_no",
    "spice_switch_nc",
    "spice_potentiometer",
    "spice_current_source_4_20ma",
    "spice_capacitor",
    "spice_inductor",
    "spice_relay",
    "spice_bjt_npn",
    "spice_bjt_pnp",
    "spice_mosfet_n",
    "spice_mosfet_p",
    "spice_optocoupler",
    "spice_igbt",
    "spice_scr",
    "spice_triac",
    "spice_ssr",
    "spice_hbridge",
    "spice_dc_motor",
    "spice_ground",
];

/// Returns `true` iff `model_name` is a registered, analytically tested primitive model.
#[must_use]
pub fn is_registered_primitive_model(model_name: &str) -> bool {
    REGISTERED_PRIMITIVE_MODELS.contains(&model_name)
}

/// Asset artwork origin (`03` §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProvenanceSource {
    Original,
    ThirdParty,
}

/// Source-neutral `[provenance]` block mandatory on every `profile.toml` (`03` §4.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub source: ProvenanceSource,
    /// Required on schematic-bearing assets: closest IEC 60617 / ISO 1219 symbol ID for reference only.
    #[serde(default)]
    pub symbol_ref: Option<String>,
    pub reviewed_by: String,
    pub reviewed_on: String,
    pub ml_restricted: bool,
    // Third-party only (forbidden when `source = "original"`):
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub attribution: Option<String>,
    #[serde(default)]
    pub modified: Option<bool>,
}

/// Grid geometry specification (`03` §2.1 & §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeometrySpec {
    pub grid_px: u32,
    pub pin_px: u32,
    /// Snap pitch for `<g id="df-pins">` markers (`48`, `24`, or `12` px for dense MCU headers).
    #[serde(default = "default_pin_pitch_px")]
    pub pin_pitch_px: u32,
    pub physical_grid_w: f64,
    pub physical_grid_h: f64,
    #[serde(default)]
    pub schematic_grid_w: Option<f64>,
    #[serde(default)]
    pub schematic_grid_h: Option<f64>,
}

const fn default_pin_pitch_px() -> u32 {
    24
}

/// Dynamic SVG layer binding mode (`03` §5, ADR-013).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingKind {
    Visibility,
    Paint,
    Translate,
    Rotate,
    OverlayRect,
}

/// Declarative TOML `[svg.bindings]` entry driving a named SVG layer without XML mutation (`03` §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SvgBinding {
    pub target_id: String,
    pub kind: BindingKind,
    pub source_signal: String,
    #[serde(default)]
    pub color_from_param: Option<String>,
    #[serde(default)]
    pub axis: Option<[f64; 2]>,
    #[serde(default)]
    pub pivot: Option<[f64; 2]>,
    #[serde(default)]
    pub angle_range_deg: Option<[f64; 2]>,
}

/// Physical domain of a component or board port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortDomain {
    Electrical,
    Pneumatic,
    Hydraulic,
    Thermal,
    Mechanical,
}

/// Component parameter kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamKind {
    Enum,
    Real,
    Integer,
    Boolean,
    String,
}

/// Typed component parameter schema (`[params.<name>]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamSpec {
    #[serde(rename = "type")]
    pub kind: ParamKind,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub options: Option<Vec<String>>,
    pub default: toml::Value,
}

/// Single internal primitive part inside a composite component sub-circuit (`03` §1.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompositePart {
    pub id: String,
    /// Registered primitive physics model name (e.g. `"spice_led"`, `"spice_resistor"`).
    pub model: String,
    #[serde(default)]
    pub params: BTreeMap<String, toml::Value>,
}

/// Internal connection inside a composite sub-circuit (`<part_or_port>.<pin>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompositeWire {
    pub from: String,
    pub to: String,
}

/// Declarative composite electrical sub-circuit (`03` §1.3), e.g., `signal-pilot-lamp` = LED + resistor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompositeSubcircuit {
    pub parts: Vec<CompositePart>,
    pub wires: Vec<CompositeWire>,
    /// Maps external component port ID (e.g. `"X1"`) -> internal part endpoint (e.g. `"r_limit.A"`).
    pub port_map: BTreeMap<String, String>,
}

/// Physics binding on a component profile: EITHER a primitive `model` OR a `composite` sub-circuit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicsSpec {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub composite: Option<CompositeSubcircuit>,
}

/// Optional display character-set descriptor (`03` §6.1, `DEF-007`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplaySpec {
    pub charset: String,
}

// ===========================================================================
// Component Profile (`assets/components/<id>/profile.toml`)
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentMeta {
    pub id: String,
    pub name: String,
    pub category: String,
    /// IEC 81346-2 schematic reference-designation prefix (`"-S"`, `"-H"`, `"-R"`, `"-B"`, `DEF-009`).
    pub ref_designator: String,
    pub standards: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentSvgSpec {
    pub physical: String,
    pub schematic: String,
    #[serde(default)]
    pub bindings: Vec<SvgBinding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentPort {
    pub id: String,
    pub label: String,
    pub domain: PortDomain,
    pub role: PinClass,
    #[serde(default)]
    pub nominal_v: f64,
    #[serde(default = "default_passive_v_abs_max")]
    pub v_abs_max_v: f64,
}

const fn default_passive_v_abs_max() -> f64 {
    30.0
}

/// Strict v2 Component Profile (`assets/components/<id>/profile.toml`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentProfile {
    pub schema_version: String,
    pub meta: ComponentMeta,
    pub provenance: Provenance,
    pub geometry: GeometrySpec,
    pub svg: ComponentSvgSpec,
    pub ports: Vec<ComponentPort>,
    #[serde(default)]
    pub params: BTreeMap<String, ParamSpec>,
    pub physics: PhysicsSpec,
    #[serde(default)]
    pub display: Option<DisplaySpec>,
}

// ===========================================================================
// Board Profile (`assets/boards/<id>/profile.toml`)
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardMeta {
    pub id: String,
    pub name: String,
    pub family: String,
    /// IEC 81346-2 schematic reference-designation prefix (typically `"-A"`, `DEF-009`).
    pub ref_designator: String,
    pub standards: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuSpec {
    pub int_bits: u8,
    pub clock_hz: u64,
    pub source_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionCostSpec {
    pub stmt_cycles: u64,
    pub io_read_cycles: u64,
    pub io_write_cycles: u64,
    pub adc_read_cycles: u64,
    pub branch_cycles: u64,
    pub call_cycles: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardExecutionSpec {
    pub default_kind: ExecutionKind,
    #[serde(default)]
    pub default_scan_period_us: Option<u64>,
    pub indeterminate_reads_as: String,
    pub cost: ExecutionCostSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardElectricalSpec {
    pub logic_high_v: f64,
    pub logic_low_v: f64,
    pub v_ih_v: f64,
    pub v_il_v: f64,
    pub pin_r_out_ohm: f64,
    pub pin_r_in_ohm: f64,
    pub pin_i_max_a: f64,
    pub v_abs_max_v: f64,
    pub v_abs_min_v: f64,
    pub pullup_ohm: f64,
    pub pulldown_ohm: f64,
    pub source_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardCodecSpec {
    pub id: String,
    pub kind: CodecKind,
    pub d: u64,
    pub n_max: u64,
    pub source_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardPwmSpec {
    pub mode: String,
    pub period_us: u64,
    pub source_ref: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardChannelSpec {
    pub port_id: String,
    pub codec_id: String,
    pub range: ChannelRange,
    #[serde(default)]
    pub band: Option<ChannelBand>,
    #[serde(default)]
    pub bipolar_truncation: Option<String>,
    #[serde(default)]
    pub pwm: Option<BoardPwmSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortCapability {
    DigitalIn,
    DigitalOut,
    Pullup,
    Pulldown,
    AnalogIn,
    AnalogOut,
    Pwm,
    Interrupt,
    UartTx,
    UartRx,
    I2cSda,
    I2cScl,
    SpiMosi,
    SpiMiso,
    SpiSck,
    SpiCs,
    Ethernet,
    Power,
    Ground,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardPort {
    pub id: String,
    pub label: String,
    pub domain: PortDomain,
    pub role: PinClass,
    #[serde(default)]
    pub iec_address: Option<String>,
    pub capabilities: Vec<PortCapability>,
    #[serde(default)]
    pub nominal_v: Option<f64>,
    #[serde(default)]
    pub v_abs_max_v: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardSvgSpec {
    pub physical: String,
    #[serde(default)]
    pub schematic: Option<String>,
    #[serde(default)]
    pub bindings: Vec<SvgBinding>,
}

/// Strict v2 Board Profile (`assets/boards/<id>/profile.toml`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardProfile {
    pub schema_version: String,
    pub meta: BoardMeta,
    pub provenance: Provenance,
    pub cpu: CpuSpec,
    pub execution: BoardExecutionSpec,
    pub electrical: BoardElectricalSpec,
    #[serde(default)]
    pub codecs: Vec<BoardCodecSpec>,
    #[serde(default)]
    pub channels: Vec<BoardChannelSpec>,
    pub geometry: GeometrySpec,
    pub svg: BoardSvgSpec,
    pub ports: Vec<BoardPort>,
}

// ===========================================================================
// Human Review Sign-Off (`review.toml`, `03` §4.3)
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewChecklist {
    pub grid_and_pins_verified: bool,
    pub symbology_style_checked: bool,
    pub datasheet_params_cited: bool,
    pub provenance_verified: bool,
}

/// Strict human review sign-off bound to canonical SVG + headless render SHA-256 hashes (`03` §4.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewSignOff {
    pub schema_version: String,
    pub reviewer: String,
    pub reviewed_on: String,
    pub physical_render_sha256: String,
    #[serde(default)]
    pub schematic_render_sha256: Option<String>,
    pub conformity_claimed: bool,
    pub checklist: ReviewChecklist,
}
