//! Lightweight hardware profile abstraction consumed by `dendri-validate`.
//!
//! In Phase 4, `dendri-assets::AssetRegistry` implements [`ProfileCatalog`]
//! directly, keeping `dendri-validate` cleanly decoupled from SVG/GPU crates.

use dendri_ast::{ChannelBand, Codec, CodecKind, HardwareKind, PullMode};
use std::collections::BTreeMap;

/// Supported display character ROM sets (`03-asset-and-profile-contract.md` §6.1, `DEF-007`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DisplayCharset {
    /// Strict printable 7-bit ASCII (`0x20..=0x7E`).
    Ascii7Bit,
    /// Hitachi HD44780U ROM Code A00 (Datasheet Table 4: ASCII + `¥`, `→`, `←`, `°`, `α`, `β`, `ε`, `μ`, `Ω`, `Σ`, `π`, `÷`, `█`, half-width Katakana).
    Hd44780A00,
    /// Hitachi HD44780U ROM Code A02 (Datasheet Table 5: ASCII + European/Latin-1 accents `áéíóúãõçüöäñ°±µΩ`).
    Hd44780A02,
}

impl DisplayCharset {
    /// Parses a profile TOML `[display].charset` identifier.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "ascii_7bit" => Some(Self::Ascii7Bit),
            "hd44780_a00" => Some(Self::Hd44780A00),
            "hd44780_a02" => Some(Self::Hd44780A02),
            _ => None,
        }
    }

    /// Returns `true` iff `ch` can be rendered natively by this display ROM.
    #[must_use]
    pub const fn supports_char(self, ch: char) -> bool {
        match self {
            Self::Ascii7Bit => matches!(ch, ' '..='~' | '\n' | '\r'),
            Self::Hd44780A00 => matches!(
                ch,
                ' '..='['
                    | ']'..='}'
                    | '\n'
                    | '\r'
                    | '¥'
                    | '→'
                    | '←'
                    | '°'
                    | 'α'
                    | 'ä'
                    | 'β'
                    | 'ε'
                    | 'μ'
                    | 'µ'
                    | 'σ'
                    | 'ρ'
                    | '√'
                    | '¢'
                    | '£'
                    | 'ñ'
                    | 'ö'
                    | 'θ'
                    | '∞'
                    | 'Ω'
                    | 'ü'
                    | 'Σ'
                    | 'π'
                    | '÷'
                    | '█'
                    | '\u{FF61}'..='\u{FF9E}'
            ),
            Self::Hd44780A02 => matches!(
                ch,
                ' '..='~'
                    | '\n'
                    | '\r'
                    | '£'
                    | '¥'
                    | '°'
                    | '±'
                    | 'µ'
                    | 'μ'
                    | 'À'
                    | 'Á'
                    | 'Â'
                    | 'Ã'
                    | 'Ä'
                    | 'Ç'
                    | 'É'
                    | 'Ê'
                    | 'Í'
                    | 'Ñ'
                    | 'Ó'
                    | 'Ô'
                    | 'Õ'
                    | 'Ö'
                    | 'Ú'
                    | 'Ü'
                    | 'ß'
                    | 'à'
                    | 'á'
                    | 'â'
                    | 'ã'
                    | 'ä'
                    | 'ç'
                    | 'é'
                    | 'ê'
                    | 'í'
                    | 'ñ'
                    | 'ó'
                    | 'ô'
                    | 'õ'
                    | 'ö'
                    | '÷'
                    | 'ú'
                    | 'ü'
                    | 'Ω'
                    | '█'
            ),
        }
    }

    /// Canonical identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ascii7Bit => "ascii_7bit",
            Self::Hd44780A00 => "hd44780_a00",
            Self::Hd44780A02 => "hd44780_a02",
        }
    }
}

/// Electrical & capability summary of one board or component port.
#[derive(Debug, Clone, PartialEq)]
pub struct PortMeta {
    pub digital_in: bool,
    pub digital_out: bool,
    pub pullup: bool,
    pub pulldown: bool,
    pub analog_in: bool,
    pub analog_out: bool,
    pub pwm: bool,
    pub bus: bool,
    /// Nominal operating voltage domain of this port (`5.0`, `24.0`, `10.0`, etc.).
    pub nominal_v: f64,
    /// Absolute maximum tolerated voltage before hardware damage fault.
    pub v_abs_max_v: f64,
}

impl PortMeta {
    /// Checks whether this port supports the requested [`HardwareKind`].
    #[must_use]
    pub const fn supports_hardware_kind(&self, kind: HardwareKind) -> bool {
        match kind {
            HardwareKind::DigitalIn => self.digital_in,
            HardwareKind::DigitalOut => self.digital_out,
            HardwareKind::DigitalBidirectional => self.digital_in && self.digital_out,
            HardwareKind::AnalogIn => self.analog_in,
            HardwareKind::AnalogOut => self.analog_out,
            HardwareKind::PwmOut => self.pwm,
            HardwareKind::BusPin => self.bus,
        }
    }

    /// Checks whether this port supports the requested internal [`PullMode`].
    #[must_use]
    pub const fn supports_pull(&self, pull: PullMode) -> bool {
        match pull {
            PullMode::None => true,
            PullMode::PullUp => self.pullup,
            PullMode::PullDown => self.pulldown,
        }
    }
}

/// Board profile metadata required for static validation.
#[derive(Debug, Clone)]
pub struct BoardProfileMeta {
    pub id: String,
    pub ports: BTreeMap<String, PortMeta>,
    pub codecs: BTreeMap<String, Codec>,
    /// Optional per-port extended band tables (`DEF-002`).
    pub channel_bands: BTreeMap<String, ChannelBand>,
}

/// Component profile metadata required for static validation.
#[derive(Debug, Clone)]
pub struct ComponentProfileMeta {
    pub id: String,
    pub ports: BTreeMap<String, PortMeta>,
    pub display_charset: Option<DisplayCharset>,
}

/// Read-only hardware profile lookup trait consumed by `dendri-validate`.
pub trait ProfileCatalog {
    /// Looks up a board profile by kebab-case profile ID.
    fn board_profile(&self, id: &str) -> Option<&BoardProfileMeta>;
    /// Looks up a component profile by kebab-case profile ID.
    fn component_profile(&self, id: &str) -> Option<&ComponentProfileMeta>;
}

/// In-memory profile catalog used in Phase 1 validation and unit/integration tests.
#[derive(Debug, Clone, Default)]
pub struct MemoryProfileCatalog {
    boards: BTreeMap<String, BoardProfileMeta>,
    components: BTreeMap<String, ComponentProfileMeta>,
}

impl MemoryProfileCatalog {
    /// Creates an empty catalog.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts or replaces a board profile summary.
    pub fn insert_board(&mut self, board: BoardProfileMeta) {
        self.boards.insert(board.id.clone(), board);
    }

    /// Inserts or replaces a component profile summary.
    pub fn insert_component(&mut self, comp: ComponentProfileMeta) {
        self.components.insert(comp.id.clone(), comp);
    }

    /// Builds the reference catalog matching the Phase 4 Core Kit (`arduino-uno-r3`,
    /// `plc-1200-open-1214`, core components, plus `lcd-16x2` display charset).
    #[must_use]
    pub fn builtin_reference() -> Self {
        let mut cat = Self::new();
        cat.insert_board(arduino_uno_r3_meta());
        cat.insert_board(plc_1200_open_1214_meta());
        cat.insert_component(passive_2pin("led", "A", "K", 0.0, 30.0));
        cat.insert_component(passive_2pin("resistor", "1", "2", 0.0, 250.0));
        cat.insert_component(passive_2pin("maker-tactile-4pin", "1", "2", 0.0, 30.0));
        cat.insert_component(passive_2pin(
            "cmd-pushbutton-no-start",
            "NO",
            "COM",
            0.0,
            250.0,
        ));
        cat.insert_component(passive_2pin(
            "cmd-pushbutton-nc-stop",
            "NC",
            "COM",
            0.0,
            250.0,
        ));
        cat.insert_component(passive_2pin("signal-pilot-lamp", "X1", "X2", 0.0, 30.0));
        cat.insert_component(passive_2pin(
            "sensor-pressure-4-20ma",
            "OUT",
            "GND",
            0.0,
            30.0,
        ));
        cat.insert_component(potentiometer_meta());
        cat.insert_component(lcd_16x2_meta());
        cat
    }
}

impl ProfileCatalog for MemoryProfileCatalog {
    fn board_profile(&self, id: &str) -> Option<&BoardProfileMeta> {
        self.boards.get(id)
    }

    fn component_profile(&self, id: &str) -> Option<&ComponentProfileMeta> {
        self.components.get(id)
    }
}

fn arduino_uno_r3_meta() -> BoardProfileMeta {
    let mut ports = BTreeMap::new();
    for d in 0..=13u8 {
        let pwm = matches!(d, 3 | 5 | 6 | 9 | 10 | 11);
        let bus = matches!(d, 0 | 1 | 10 | 11 | 12 | 13);
        ports.insert(
            format!("D{d}"),
            PortMeta {
                digital_in: true,
                digital_out: true,
                pullup: true,
                pulldown: false, // ATmega328P has internal pull-up ONLY (no pull-down)
                analog_in: false,
                analog_out: false,
                pwm,
                bus,
                nominal_v: 5.0,
                v_abs_max_v: 5.5,
            },
        );
    }
    for a in 0..=5u8 {
        ports.insert(
            format!("A{a}"),
            PortMeta {
                digital_in: true,
                digital_out: true,
                pullup: true,
                pulldown: false,
                analog_in: true,
                analog_out: false,
                pwm: false,
                bus: matches!(a, 4 | 5),
                nominal_v: 5.0,
                v_abs_max_v: 5.5,
            },
        );
    }
    for (pwr, nom) in [("5V", 5.0), ("VCC", 5.0), ("GND", 0.0)] {
        ports.insert(
            pwr.to_owned(),
            PortMeta {
                digital_in: false,
                digital_out: false,
                pullup: false,
                pulldown: false,
                analog_in: false,
                analog_out: false,
                pwm: false,
                bus: false,
                nominal_v: nom,
                v_abs_max_v: 5.5,
            },
        );
    }
    let mut codecs = BTreeMap::new();
    codecs.insert(
        "uno_10bit".to_owned(),
        Codec {
            id: "uno_10bit".to_owned(),
            kind: CodecKind::Binned,
            d: 1024,
            n_max: 1023,
            source_ref: Some("ATmega328P Datasheet §24".to_owned()),
        },
    );
    codecs.insert(
        "pwm_8bit".to_owned(),
        Codec {
            id: "pwm_8bit".to_owned(),
            kind: CodecKind::Binned,
            d: 256,
            n_max: 255,
            source_ref: Some("ATmega328P Datasheet §16".to_owned()),
        },
    );
    BoardProfileMeta {
        id: "arduino-uno-r3".to_owned(),
        ports,
        codecs,
        channel_bands: BTreeMap::new(),
    }
}

fn plc_1200_open_1214_meta() -> BoardProfileMeta {
    let mut ports = BTreeMap::new();
    for i in 0..=7u8 {
        ports.insert(
            format!("Ia.{i}"),
            PortMeta {
                digital_in: true,
                digital_out: false,
                pullup: false,
                pulldown: true,
                analog_in: false,
                analog_out: false,
                pwm: false,
                bus: false,
                nominal_v: 24.0,
                v_abs_max_v: 30.0,
            },
        );
        ports.insert(
            format!("Qa.{i}"),
            PortMeta {
                digital_in: false,
                digital_out: true,
                pullup: false,
                pulldown: false,
                analog_in: false,
                analog_out: false,
                pwm: matches!(i, 0 | 1),
                bus: false,
                nominal_v: 24.0,
                v_abs_max_v: 30.0,
            },
        );
    }
    for ai in ["IW64", "IW66"] {
        ports.insert(
            ai.to_owned(),
            PortMeta {
                digital_in: false,
                digital_out: false,
                pullup: false,
                pulldown: false,
                analog_in: true,
                analog_out: false,
                pwm: false,
                bus: false,
                nominal_v: 10.0,
                v_abs_max_v: 12.0,
            },
        );
    }
    for (pwr, nom) in [("24V", 24.0), ("L+", 24.0), ("M", 0.0), ("GND", 0.0)] {
        ports.insert(
            pwr.to_owned(),
            PortMeta {
                digital_in: false,
                digital_out: false,
                pullup: false,
                pulldown: false,
                analog_in: false,
                analog_out: false,
                pwm: false,
                bus: false,
                nominal_v: nom,
                v_abs_max_v: 30.0,
            },
        );
    }
    ports.insert(
        "ETH0".to_owned(),
        PortMeta {
            digital_in: false,
            digital_out: false,
            pullup: false,
            pulldown: false,
            analog_in: false,
            analog_out: false,
            pwm: false,
            bus: true,
            nominal_v: 0.0,
            v_abs_max_v: 30.0,
        },
    );
    let mut codecs = BTreeMap::new();
    codecs.insert(
        "plc_span_27648".to_owned(),
        Codec {
            id: "plc_span_27648".to_owned(),
            kind: CodecKind::Span,
            d: 27_648,
            n_max: 27_648,
            source_ref: Some(
                "IEC 61131-2 / Industrial 27648-count unipolar analog scale".to_owned(),
            ),
        },
    );
    codecs.insert(
        "plc_bipolar_27648".to_owned(),
        Codec {
            id: "plc_bipolar_27648".to_owned(),
            kind: CodecKind::SpanBipolar,
            d: 27_648,
            n_max: 27_648,
            source_ref: Some(
                "IEC 61131-2 / Industrial 27648-count bipolar analog scale".to_owned(),
            ),
        },
    );
    let mut channel_bands = BTreeMap::new();
    channel_bands.insert(
        "IW64".to_owned(),
        ChannelBand {
            underrange_min: -4_864,
            overrange_max: 32_511,
            overflow_code: Some(32_767),
            wire_break_code: Some(-32_768),
            source_ref: "IEC 61131-2 / Industrial 27648-count AI band table".to_owned(),
        },
    );
    BoardProfileMeta {
        id: "plc-1200-open-1214".to_owned(),
        ports,
        codecs,
        channel_bands,
    }
}

fn passive_2pin(
    id: &str,
    p1: &str,
    p2: &str,
    nominal_v: f64,
    v_abs_max_v: f64,
) -> ComponentProfileMeta {
    let mut ports = BTreeMap::new();
    let pm = PortMeta {
        digital_in: true,
        digital_out: true,
        pullup: false,
        pulldown: false,
        analog_in: true,
        analog_out: true,
        pwm: false,
        bus: false,
        nominal_v,
        v_abs_max_v,
    };
    ports.insert(p1.to_owned(), pm.clone());
    ports.insert(p2.to_owned(), pm);
    ComponentProfileMeta {
        id: id.to_owned(),
        ports,
        display_charset: None,
    }
}

fn potentiometer_meta() -> ComponentProfileMeta {
    let mut ports = BTreeMap::new();
    let pm = PortMeta {
        digital_in: false,
        digital_out: false,
        pullup: false,
        pulldown: false,
        analog_in: true,
        analog_out: true,
        pwm: false,
        bus: false,
        nominal_v: 0.0,
        v_abs_max_v: 30.0,
    };
    for p in ["A", "W", "B"] {
        ports.insert(p.to_owned(), pm.clone());
    }
    ComponentProfileMeta {
        id: "potentiometer".to_owned(),
        ports,
        display_charset: None,
    }
}

fn lcd_16x2_meta() -> ComponentProfileMeta {
    let mut ports = BTreeMap::new();
    for p in ["SDA", "SCL", "VCC", "GND"] {
        ports.insert(
            p.to_owned(),
            PortMeta {
                digital_in: false,
                digital_out: false,
                pullup: false,
                pulldown: false,
                analog_in: false,
                analog_out: false,
                pwm: false,
                bus: matches!(p, "SDA" | "SCL"),
                nominal_v: 5.0,
                v_abs_max_v: 5.5,
            },
        );
    }
    ComponentProfileMeta {
        id: "lcd-16x2".to_owned(),
        ports,
        display_charset: Some(DisplayCharset::Hd44780A00),
    }
}
