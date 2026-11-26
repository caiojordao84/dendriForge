//! Single source of truth for the `dendriForge` v2 **A11Y Slate** palettes
//! (**Dark** and **Light**, resolved via [`ThemeMode`] with `System` default),
//! semantic `<g id="df-pins">` pin classes (`03-asset-and-profile-contract.md` §2.2),
//! and deterministic WCAG 2.1 contrast calculation (`docs/ui/DESIGN.md` §5.1, `F11`).
//!
//! Zero UI-framework dependency (Layer 0): the single adapter in `dendri-editor`
//! converts [`ThemePalette`] tokens into `egui::Color32` cleanly without
//! duplicating hex constants.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Straight-alpha 8-bit sRGB color token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    /// Constructs an opaque sRGB color from a `0xRRGGBB` literal.
    #[must_use]
    pub const fn from_rgb_hex(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xFF) as u8,
            g: ((hex >> 8) & 0xFF) as u8,
            b: (hex & 0xFF) as u8,
            a: 0xFF,
        }
    }

    /// Constructs a translucent sRGB color from a `0xRRGGBBAA` literal.
    #[must_use]
    pub const fn from_rgba_hex(hex: u32) -> Self {
        Self {
            r: ((hex >> 24) & 0xFF) as u8,
            g: ((hex >> 16) & 0xFF) as u8,
            b: ((hex >> 8) & 0xFF) as u8,
            a: (hex & 0xFF) as u8,
        }
    }

    /// Returns `[r, g, b, a]` as raw bytes.
    #[must_use]
    pub const fn to_rgba_u8(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// Formats as `#rrggbb` (if opaque) or `#rrggbbaa`.
    #[must_use]
    pub fn to_hex_string(self) -> String {
        if self.a == 0xFF {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    /// Computes WCAG 2.1 relative luminance in `[0.0, 1.0]` using pure-Rust `libm`.
    #[must_use]
    pub fn relative_luminance(self) -> f64 {
        let r = linearize_channel(self.r);
        let g = linearize_channel(self.g);
        let b = linearize_channel(self.b);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    /// Computes the WCAG 2.1 contrast ratio (`1.0 ..= 21.0`) between two opaque colors.
    #[must_use]
    pub fn contrast_ratio(self, other: Self) -> f64 {
        let l1 = self.relative_luminance();
        let l2 = other.relative_luminance();
        let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
        (hi + 0.05) / (lo + 0.05)
    }
}

fn linearize_channel(channel: u8) -> f64 {
    let s = f64::from(channel) / 255.0;
    if s <= 0.03928 {
        s / 12.92
    } else {
        libm::pow((s + 0.055) / 1.055, 2.4)
    }
}

/// WCAG 2.1 minimum contrast ratio for AA non-text UI components & graphical indicators.
pub const WCAG_AA_NON_TEXT: f64 = 3.0;
/// WCAG 2.1 minimum contrast ratio for AA normal text.
pub const WCAG_AA_NORMAL_TEXT: f64 = 4.5;
/// WCAG 2.1 minimum contrast ratio for AAA normal text & critical pin/syntax tokens.
pub const WCAG_AAA_NORMAL_TEXT: f64 = 7.0;

// ---------------------------------------------------------------------------
// Layout Backgrounds (`--df-bg-*`)
// ---------------------------------------------------------------------------
pub const DF_BG_MAIN: Color = Color::from_rgb_hex(0x0f_17_2a); // slate-900
pub const DF_BG_PANEL: Color = Color::from_rgb_hex(0x02_06_17); // slate-950
pub const DF_BG_SIDEBAR: Color = Color::from_rgb_hex(0x1b_1e_2b);
pub const DF_BG_EDITOR: Color = Color::from_rgb_hex(0x2b_2b_2b);
pub const DF_BG_HOVER: Color = Color::from_rgb_hex(0x1e_29_3b); // slate-800
pub const DF_BG_LOWEST: Color = Color::from_rgb_hex(0x06_0e_20);
pub const DF_BG_SURFACE: Color = Color::from_rgb_hex(0x0b_13_26);
pub const DF_BG_LOW: Color = Color::from_rgb_hex(0x13_1b_2e);
pub const DF_BG_CARD: Color = Color::from_rgb_hex(0x17_1f_33);

// ---------------------------------------------------------------------------
// Borders, Dividers & Ladder Rails
// ---------------------------------------------------------------------------
pub const DF_BORDER_MAIN: Color = DF_BG_HOVER;
pub const DF_BORDER_LIGHT: Color = Color::from_rgb_hex(0x33_41_55); // slate-700
pub const DF_BORDER_SUBTLE: Color = Color::from_rgb_hex(0x47_55_69); // slate-600
/// Non-energized Ladder rail / wire neutral stroke (3.75:1 on BG_MAIN, fixing v1's 1.79:1).
pub const DF_LADDER_RAIL_INACTIVE: Color = Color::from_rgb_hex(0x64_74_8b); // slate-500
/// Energized Ladder rail / active power flow (10.17:1 on BG_MAIN).
pub const DF_LADDER_RAIL_ACTIVE: Color = DF_ACCENT_EMERALD;

// ---------------------------------------------------------------------------
// Interface Text (`--df-text-*`)
// ---------------------------------------------------------------------------
pub const DF_TEXT_HEADING: Color = Color::from_rgb_hex(0xf8_fa_fc); // slate-50
pub const DF_TEXT_MAIN: Color = Color::from_rgb_hex(0xe2_e8_f0); // slate-200
pub const DF_TEXT_MUTED: Color = Color::from_rgb_hex(0x94_a3_b8); // slate-400
pub const DF_TEXT_FADED: Color = Color::from_rgb_hex(0x64_74_8b); // slate-500 (non-text / icon only)

// ---------------------------------------------------------------------------
// Accents & Primary Actions
// ---------------------------------------------------------------------------
pub const DF_ACCENT_SKY: Color = Color::from_rgb_hex(0x38_bd_f8);
pub const DF_ACCENT_AMBER: Color = Color::from_rgb_hex(0xf5_9e_0b);
pub const DF_ACCENT_AMBER_LIGHT: Color = Color::from_rgb_hex(0xfb_bf_24);
pub const DF_ACCENT_ORANGE: Color = Color::from_rgb_hex(0xff_8c_00);
/// Hover surface for primary action buttons paired with `DF_BG_PANEL` text (10.40:1 AAA; fixes v1's 2.47:1).
pub const DF_ACCENT_ORANGE_HOVER: Color = Color::from_rgb_hex(0xff_a7_33);
pub const DF_ACCENT_ROSE: Color = Color::from_rgb_hex(0xf4_3f_5e);
pub const DF_ACCENT_EMERALD: Color = Color::from_rgb_hex(0x34_d3_99);
pub const DF_ACCENT_CYAN: Color = Color::from_rgb_hex(0x22_d3_ee);
pub const DF_ACCENT_PURPLE: Color = Color::from_rgb_hex(0xa7_8b_fa);
pub const DF_ACCENT_PURPLE_LIGHT: Color = Color::from_rgb_hex(0xe9_d5_ff);

// ---------------------------------------------------------------------------
// Runtime Status Tokens
// ---------------------------------------------------------------------------
pub const DF_STATUS_RUNNING: Color = DF_ACCENT_EMERALD;
pub const DF_STATUS_PAUSED: Color = DF_ACCENT_AMBER_LIGHT;
pub const DF_STATUS_FAULT: Color = DF_PIN_INTERRUPT;
/// Bright violet-300 (`#c4b5fd`, 9.6:1 AAA on `DF_BG_MAIN`; upgraded from `DF_ACCENT_PURPLE` 6.56:1 AA).
pub const DF_STATUS_MANUAL: Color = PRISM_MACRO;
pub const DF_STATUS_LIVE: Color = DF_ACCENT_SKY;
pub const DF_STATUS_VIRTUAL: Color = DF_ACCENT_CYAN;

// ---------------------------------------------------------------------------
// Prism A11Y Dark Syntax Tokens (all WCAG AAA >= 7.0:1 on DF_BG_EDITOR #2b2b2b)
// ---------------------------------------------------------------------------
pub const PRISM_BG: Color = DF_BG_EDITOR;
pub const PRISM_FG: Color = Color::from_rgb_hex(0xf8_f8_f2);
pub const PRISM_COMMENT: Color = Color::from_rgb_hex(0xd4_d0_ab);
pub const PRISM_KEYWORD: Color = Color::from_rgb_hex(0xff_a0_7a);
pub const PRISM_STRING: Color = Color::from_rgb_hex(0xab_e3_38);
pub const PRISM_NUMBER: Color = Color::from_rgb_hex(0xff_d7_00);
pub const PRISM_FUNCTION: Color = Color::from_rgb_hex(0x60_ca_ff);
pub const PRISM_OPERATOR: Color = Color::from_rgb_hex(0x00_e0_ff);
pub const PRISM_TYPE: Color = Color::from_rgb_hex(0xdc_c6_e0);
pub const PRISM_MACRO: Color = Color::from_rgb_hex(0xc4_b5_fd);

// ---------------------------------------------------------------------------
// Semantic Pin Tokens (all 12 WCAG AAA >= 7.0:1 on DF_BG_MAIN #0f172a)
// ---------------------------------------------------------------------------
pub const DF_PIN_POWER: Color = Color::from_rgb_hex(0xfc_81_81);
pub const DF_PIN_GND: Color = Color::from_rgb_hex(0xa0_ae_c0);
pub const DF_PIN_AC_PHASE: Color = DF_ACCENT_AMBER_LIGHT;
pub const DF_PIN_DIGITAL: Color = PRISM_OPERATOR;
pub const DF_PIN_ANALOG: Color = PRISM_STRING;
pub const DF_PIN_PWM: Color = PRISM_NUMBER;
pub const DF_PIN_COMM: Color = PRISM_MACRO;
/// Brightened rose-pink (7.46:1 AAA on BG_MAIN; replaces v1's 4.86:1 `#f43f5e`).
pub const DF_PIN_INTERRUPT: Color = Color::from_rgb_hex(0xff_80_95);
/// Brightened vibrant orange (7.89:1 AAA on BG_MAIN; replaces v1's 6.37:1 `#f97316`).
pub const DF_PIN_RESET: Color = Color::from_rgb_hex(0xfb_92_3c);
pub const DF_PIN_PASSIVE: Color = Color::from_rgb_hex(0xcb_d5_e1); // slate-300
pub const DF_PIN_PNEUMATIC: Color = PRISM_FUNCTION;
pub const DF_PIN_HYDRAULIC: Color = PRISM_KEYWORD;

/// Complete 12-variant semantic pin classification matching
/// `03-asset-and-profile-contract.md` §2.2 (`<g id="df-pins">`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinClass {
    Power,
    Gnd,
    AcPhase,
    Digital,
    Analog,
    Pwm,
    Comm,
    Interrupt,
    Reset,
    Passive,
    Pneumatic,
    Hydraulic,
}

impl PinClass {
    /// Complete ordered list of all 12 normative semantic pin classes.
    pub const ALL: &'static [Self] = &[
        Self::Power,
        Self::Gnd,
        Self::AcPhase,
        Self::Digital,
        Self::Analog,
        Self::Pwm,
        Self::Comm,
        Self::Interrupt,
        Self::Reset,
        Self::Passive,
        Self::Pneumatic,
        Self::Hydraulic,
    ];

    /// Required SVG `class` attribute inside `<g id="df-pins">`.
    #[must_use]
    pub const fn css_class(self) -> &'static str {
        match self {
            Self::Power => "df-pin-power",
            Self::Gnd => "df-pin-gnd",
            Self::AcPhase => "df-pin-ac-phase",
            Self::Digital => "df-pin-digital",
            Self::Analog => "df-pin-analog",
            Self::Pwm => "df-pin-pwm",
            Self::Comm => "df-pin-comm",
            Self::Interrupt => "df-pin-interrupt",
            Self::Reset => "df-pin-reset",
            Self::Passive => "df-pin-passive",
            Self::Pneumatic => "df-pin-pneumatic",
            Self::Hydraulic => "df-pin-hydraulic",
        }
    }

    /// Required `port.role` value in component/board `profile.toml`.
    #[must_use]
    pub const fn toml_role(self) -> &'static str {
        match self {
            Self::Power => "power",
            Self::Gnd => "gnd",
            Self::AcPhase => "ac_phase",
            Self::Digital => "digital",
            Self::Analog => "analog",
            Self::Pwm => "pwm",
            Self::Comm => "comm",
            Self::Interrupt => "interrupt",
            Self::Reset => "reset",
            Self::Passive => "passive",
            Self::Pneumatic => "pneumatic",
            Self::Hydraulic => "hydraulic",
        }
    }

    /// Resolves an SVG `df-pin-*` CSS class to its [`PinClass`].
    #[must_use]
    pub fn from_css_class(css: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.css_class() == css)
    }

    /// Resolves a TOML `port.role` value to its [`PinClass`].
    #[must_use]
    pub fn from_toml_role(role: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.toml_role() == role)
    }

    /// Authoritative A11Y Slate Dark paint color for this pin class.
    #[must_use]
    pub const fn color(self) -> Color {
        self.color_in(&ThemePalette::DARK)
    }

    /// Authoritative paint color for this pin class in the given [`ThemePalette`].
    #[must_use]
    pub const fn color_in(self, palette: &ThemePalette) -> Color {
        match self {
            Self::Power => palette.pin_power,
            Self::Gnd => palette.pin_gnd,
            Self::AcPhase => palette.pin_ac_phase,
            Self::Digital => palette.pin_digital,
            Self::Analog => palette.pin_analog,
            Self::Pwm => palette.pin_pwm,
            Self::Comm => palette.pin_comm,
            Self::Interrupt => palette.pin_interrupt,
            Self::Reset => palette.pin_reset,
            Self::Passive => palette.pin_passive,
            Self::Pneumatic => palette.pin_pneumatic,
            Self::Hydraulic => palette.pin_hydraulic,
        }
    }
}

impl fmt::Display for PinClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.css_class())
    }
}

// ---------------------------------------------------------------------------
// Theme Modes (`System` default, `Dark`, `Light` — F11) & Typed Palettes
// ---------------------------------------------------------------------------

/// User-facing theme mode preference stored in `workspace.toml` (`F11`).
///
/// Defaults to [`ThemeMode::System`], following the host OS dark/light
/// preference with a safe fallback to [`ResolvedTheme::Dark`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    System,
    Dark,
    Light,
}

impl ThemeMode {
    /// Resolves this mode against an optional host OS preference
    /// (`Some(true)` = host dark, `Some(false)` = host light, `None` = unknown -> `Dark`).
    #[must_use]
    pub const fn resolve(self, system_prefers_dark: Option<bool>) -> ResolvedTheme {
        match self {
            Self::Dark => ResolvedTheme::Dark,
            Self::Light => ResolvedTheme::Light,
            Self::System => match system_prefers_dark {
                Some(false) => ResolvedTheme::Light,
                Some(true) | None => ResolvedTheme::Dark,
            },
        }
    }

    /// Cycles `System -> Light -> Dark -> System` for quick panel toggles.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }
}

/// Concrete resolved theme variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedTheme {
    Dark,
    Light,
}

impl ResolvedTheme {
    /// Returns the authoritative WCAG-verified [`ThemePalette`] for this theme.
    #[must_use]
    pub const fn palette(self) -> &'static ThemePalette {
        match self {
            Self::Dark => &ThemePalette::DARK,
            Self::Light => &ThemePalette::LIGHT,
        }
    }
}

/// Complete opaque A11Y Slate palette consumed by the single UI theme adapter (`F11`).
///
/// Every token in both [`ThemePalette::DARK`] and [`ThemePalette::LIGHT`] is verified
/// by automated WCAG 2.1 contrast tests in this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemePalette {
    pub theme: ResolvedTheme,

    // Layout Backgrounds
    pub bg_main: Color,
    pub bg_panel: Color,
    pub bg_sidebar: Color,
    pub bg_editor: Color,
    pub bg_hover: Color,
    pub bg_lowest: Color,
    pub bg_surface: Color,
    pub bg_low: Color,
    pub bg_card: Color,

    // Borders, Dividers & Ladder Rails
    pub border_main: Color,
    pub border_light: Color,
    pub border_subtle: Color,
    pub ladder_rail_inactive: Color,
    pub ladder_rail_active: Color,

    // Interface Text
    pub text_heading: Color,
    pub text_main: Color,
    pub text_muted: Color,
    pub text_faded: Color,

    // Accents & Primary Actions
    pub accent_sky: Color,
    pub accent_amber: Color,
    pub accent_amber_light: Color,
    pub accent_orange: Color,
    pub accent_orange_hover: Color,
    /// Foreground text painted over `accent_orange` / `accent_orange_hover` primary buttons.
    pub primary_button_fg: Color,
    /// Decorative rose token — MUST NOT carry text or fault meaning.
    pub accent_rose: Color,
    pub accent_emerald: Color,
    pub accent_cyan: Color,
    pub accent_purple: Color,
    pub accent_purple_light: Color,

    // Runtime Status
    pub status_running: Color,
    pub status_paused: Color,
    pub status_fault: Color,
    pub status_manual: Color,
    pub status_live: Color,
    pub status_virtual: Color,

    // Syntax Tokens (all WCAG AAA >= 7.0:1 on `prism_bg`)
    pub prism_bg: Color,
    pub prism_fg: Color,
    pub prism_comment: Color,
    pub prism_keyword: Color,
    pub prism_string: Color,
    pub prism_number: Color,
    pub prism_function: Color,
    pub prism_operator: Color,
    pub prism_type: Color,
    pub prism_macro: Color,

    // 12 Semantic Pin Classes (all WCAG AAA >= 7.0:1 on `bg_main`)
    pub pin_power: Color,
    pub pin_gnd: Color,
    pub pin_ac_phase: Color,
    pub pin_digital: Color,
    pub pin_analog: Color,
    pub pin_pwm: Color,
    pub pin_comm: Color,
    pub pin_interrupt: Color,
    pub pin_reset: Color,
    pub pin_passive: Color,
    pub pin_pneumatic: Color,
    pub pin_hydraulic: Color,
}

impl ThemePalette {
    /// Baseline **A11Y Slate Dark** palette.
    pub const DARK: Self = Self {
        theme: ResolvedTheme::Dark,
        bg_main: DF_BG_MAIN,
        bg_panel: DF_BG_PANEL,
        bg_sidebar: DF_BG_SIDEBAR,
        bg_editor: DF_BG_EDITOR,
        bg_hover: DF_BG_HOVER,
        bg_lowest: DF_BG_LOWEST,
        bg_surface: DF_BG_SURFACE,
        bg_low: DF_BG_LOW,
        bg_card: DF_BG_CARD,
        border_main: DF_BORDER_MAIN,
        border_light: DF_BORDER_LIGHT,
        border_subtle: DF_BORDER_SUBTLE,
        ladder_rail_inactive: DF_LADDER_RAIL_INACTIVE,
        ladder_rail_active: DF_LADDER_RAIL_ACTIVE,
        text_heading: DF_TEXT_HEADING,
        text_main: DF_TEXT_MAIN,
        text_muted: DF_TEXT_MUTED,
        text_faded: DF_TEXT_FADED,
        accent_sky: DF_ACCENT_SKY,
        accent_amber: DF_ACCENT_AMBER,
        accent_amber_light: DF_ACCENT_AMBER_LIGHT,
        accent_orange: DF_ACCENT_ORANGE,
        accent_orange_hover: DF_ACCENT_ORANGE_HOVER,
        primary_button_fg: DF_BG_PANEL,
        accent_rose: DF_ACCENT_ROSE,
        accent_emerald: DF_ACCENT_EMERALD,
        accent_cyan: DF_ACCENT_CYAN,
        accent_purple: DF_ACCENT_PURPLE,
        accent_purple_light: DF_ACCENT_PURPLE_LIGHT,
        status_running: DF_STATUS_RUNNING,
        status_paused: DF_STATUS_PAUSED,
        status_fault: DF_STATUS_FAULT,
        status_manual: DF_STATUS_MANUAL,
        status_live: DF_STATUS_LIVE,
        status_virtual: DF_STATUS_VIRTUAL,
        prism_bg: PRISM_BG,
        prism_fg: PRISM_FG,
        prism_comment: PRISM_COMMENT,
        prism_keyword: PRISM_KEYWORD,
        prism_string: PRISM_STRING,
        prism_number: PRISM_NUMBER,
        prism_function: PRISM_FUNCTION,
        prism_operator: PRISM_OPERATOR,
        prism_type: PRISM_TYPE,
        prism_macro: PRISM_MACRO,
        pin_power: DF_PIN_POWER,
        pin_gnd: DF_PIN_GND,
        pin_ac_phase: DF_PIN_AC_PHASE,
        pin_digital: DF_PIN_DIGITAL,
        pin_analog: DF_PIN_ANALOG,
        pin_pwm: DF_PIN_PWM,
        pin_comm: DF_PIN_COMM,
        pin_interrupt: DF_PIN_INTERRUPT,
        pin_reset: DF_PIN_RESET,
        pin_passive: DF_PIN_PASSIVE,
        pin_pneumatic: DF_PIN_PNEUMATIC,
        pin_hydraulic: DF_PIN_HYDRAULIC,
    };

    /// Sister **A11Y Slate Light** palette (`F11`) preserving Slate neutrals,
    /// semantic pin hues, and identical WCAG 2.1 AAA/AA contrast floors.
    pub const LIGHT: Self = Self {
        theme: ResolvedTheme::Light,
        // Cool Slate light surfaces (opaque, clear elevation steps)
        bg_main: Color::from_rgb_hex(0xf8_fa_fc), // slate-50
        bg_panel: Color::from_rgb_hex(0xff_ff_ff), // white
        bg_sidebar: Color::from_rgb_hex(0xf1_f5_f9), // slate-100
        bg_editor: Color::from_rgb_hex(0xf8_fa_fc), // slate-50
        bg_hover: Color::from_rgb_hex(0xe2_e8_f0), // slate-200
        bg_lowest: Color::from_rgb_hex(0xe2_e8_f0), // slate-200
        bg_surface: Color::from_rgb_hex(0xf1_f5_f9), // slate-100
        bg_low: Color::from_rgb_hex(0xe2_e8_f0),  // slate-200
        bg_card: Color::from_rgb_hex(0xff_ff_ff), // white
        // Borders & Ladder Rails
        border_main: Color::from_rgb_hex(0xcb_d5_e1), // slate-300
        border_light: Color::from_rgb_hex(0x94_a3_b8), // slate-400
        border_subtle: Color::from_rgb_hex(0x64_74_8b), // slate-500
        ladder_rail_inactive: Color::from_rgb_hex(0x47_55_69), // slate-600
        ladder_rail_active: Color::from_rgb_hex(0x05_58_40), // deep emerald (AAA)
        // Interface Text
        text_heading: Color::from_rgb_hex(0x02_06_17), // slate-950
        text_main: Color::from_rgb_hex(0x0f_17_2a),    // slate-900
        text_muted: Color::from_rgb_hex(0x33_41_55),   // slate-700
        text_faded: Color::from_rgb_hex(0x47_55_69),   // slate-600 (non-text/icon)
        // Accents & Primary Actions (darkened for AAA on light slate)
        accent_sky: Color::from_rgb_hex(0x03_53_88), // deep sky
        accent_amber: Color::from_rgb_hex(0x85_3a_0c), // deep amber
        accent_amber_light: Color::from_rgb_hex(0x78_35_0f), // amber-900
        accent_orange: Color::from_rgb_hex(0x8c_2d_0c), // deep burnt orange (AAA >= 7.8:1)
        accent_orange_hover: Color::from_rgb_hex(0x6e_22_08), // darker burnt orange (AAA >= 10.5:1)
        primary_button_fg: Color::from_rgb_hex(0xff_ff_ff), // white on deep orange
        accent_rose: Color::from_rgb_hex(0xe1_1d_48), // decorative only
        accent_emerald: Color::from_rgb_hex(0x05_58_40), // deep emerald
        accent_cyan: Color::from_rgb_hex(0x0e_52_66), // deep cyan
        accent_purple: Color::from_rgb_hex(0x5b_21_b6), // violet-800
        accent_purple_light: Color::from_rgb_hex(0x6d_28_d9), // violet-700
        // Runtime Status
        status_running: Color::from_rgb_hex(0x05_58_40),
        status_paused: Color::from_rgb_hex(0x78_35_0f),
        status_fault: Color::from_rgb_hex(0x9d_17_4d),
        status_manual: Color::from_rgb_hex(0x5b_21_b6),
        status_live: Color::from_rgb_hex(0x03_53_88),
        status_virtual: Color::from_rgb_hex(0x0e_52_66),
        // Prism A11Y Light Syntax Tokens (all AAA >= 7.0:1 on #f8fafc)
        prism_bg: Color::from_rgb_hex(0xf8_fa_fc),
        prism_fg: Color::from_rgb_hex(0x0f_17_2a),
        prism_comment: Color::from_rgb_hex(0x425064),
        prism_keyword: Color::from_rgb_hex(0x8c_2d_0c),
        prism_string: Color::from_rgb_hex(0x14_53_2d),
        prism_number: Color::from_rgb_hex(0x71_3f_12),
        prism_function: Color::from_rgb_hex(0x03_53_88),
        prism_operator: Color::from_rgb_hex(0x0e_52_66),
        prism_type: Color::from_rgb_hex(0x6b_21_a8),
        prism_macro: Color::from_rgb_hex(0x4c_1d_95),
        // 12 Semantic Pin Classes (all AAA >= 7.0:1 on #f8fafc, AA >= 4.5:1 on #e2e8f0)
        pin_power: Color::from_rgb_hex(0x99_1b_1b),
        pin_gnd: Color::from_rgb_hex(0x33_41_55),
        pin_ac_phase: Color::from_rgb_hex(0x78_35_0f),
        pin_digital: Color::from_rgb_hex(0x0e_52_66),
        pin_analog: Color::from_rgb_hex(0x14_53_2d),
        pin_pwm: Color::from_rgb_hex(0x71_3f_12),
        pin_comm: Color::from_rgb_hex(0x5b_21_b6),
        pin_interrupt: Color::from_rgb_hex(0x9d_17_4d),
        pin_reset: Color::from_rgb_hex(0x8c_2d_0c),
        pin_passive: Color::from_rgb_hex(0x3f_4d_63),
        pin_pneumatic: Color::from_rgb_hex(0x03_53_88),
        pin_hydraulic: Color::from_rgb_hex(0x7c_2d_12),
    };

    /// Returns the authoritative palette for a [`ThemeMode`] and optional OS dark preference.
    #[must_use]
    pub const fn for_mode(mode: ThemeMode, system_prefers_dark: Option<bool>) -> &'static Self {
        mode.resolve(system_prefers_dark).palette()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_palette_wcag(name: &str, p: &ThemePalette) {
        // Primary & heading text: WCAG AAA >= 7.0:1 across all layout surfaces
        for (surface_name, bg) in [
            ("bg_main", p.bg_main),
            ("bg_panel", p.bg_panel),
            ("bg_card", p.bg_card),
            ("bg_hover", p.bg_hover),
            ("bg_sidebar", p.bg_sidebar),
            ("bg_surface", p.bg_surface),
        ] {
            let main_cr = p.text_main.contrast_ratio(bg);
            assert!(
                main_cr >= WCAG_AAA_NORMAL_TEXT,
                "{name}: text_main on {surface_name} contrast {main_cr:.2} < {WCAG_AAA_NORMAL_TEXT}"
            );
            let head_cr = p.text_heading.contrast_ratio(bg);
            assert!(
                head_cr >= WCAG_AAA_NORMAL_TEXT,
                "{name}: text_heading on {surface_name} contrast {head_cr:.2} < {WCAG_AAA_NORMAL_TEXT}"
            );
        }

        // Muted text: WCAG AA >= 4.5:1 across surfaces
        for (surface_name, bg) in [
            ("bg_main", p.bg_main),
            ("bg_panel", p.bg_panel),
            ("bg_card", p.bg_card),
            ("bg_hover", p.bg_hover),
            ("bg_sidebar", p.bg_sidebar),
        ] {
            let cr = p.text_muted.contrast_ratio(bg);
            assert!(
                cr >= WCAG_AA_NORMAL_TEXT,
                "{name}: text_muted on {surface_name} contrast {cr:.2} < {WCAG_AA_NORMAL_TEXT}"
            );
        }

        // Primary action button normal & hover: WCAG AAA >= 7.0:1
        let normal = p.primary_button_fg.contrast_ratio(p.accent_orange);
        assert!(
            normal >= WCAG_AAA_NORMAL_TEXT,
            "{name}: primary button normal contrast {normal:.2} < {WCAG_AAA_NORMAL_TEXT}"
        );
        let hover = p.primary_button_fg.contrast_ratio(p.accent_orange_hover);
        assert!(
            hover >= WCAG_AAA_NORMAL_TEXT,
            "{name}: primary button hover contrast {hover:.2} < {WCAG_AAA_NORMAL_TEXT}"
        );

        // Ladder rails & focus ring
        let inactive = p.ladder_rail_inactive.contrast_ratio(p.bg_main);
        assert!(
            inactive >= WCAG_AA_NON_TEXT,
            "{name}: inactive Ladder rail contrast {inactive:.2} < {WCAG_AA_NON_TEXT}"
        );
        let active = p.ladder_rail_active.contrast_ratio(p.bg_main);
        assert!(
            active >= WCAG_AAA_NORMAL_TEXT,
            "{name}: active Ladder rail contrast {active:.2} < {WCAG_AAA_NORMAL_TEXT}"
        );
        let focus = p.accent_sky.contrast_ratio(p.bg_main);
        assert!(
            focus >= WCAG_AAA_NORMAL_TEXT,
            "{name}: focus outline contrast {focus:.2} < {WCAG_AAA_NORMAL_TEXT}"
        );

        // Status tokens: fault/interrupt MUST NOT equal decorative accent_rose
        assert_ne!(
            p.status_fault, p.accent_rose,
            "{name}: status_fault must never reuse decorative accent_rose"
        );
        for (status_name, color) in [
            ("status_running", p.status_running),
            ("status_paused", p.status_paused),
            ("status_fault", p.status_fault),
            ("status_manual", p.status_manual),
            ("status_live", p.status_live),
            ("status_virtual", p.status_virtual),
        ] {
            let cr = color.contrast_ratio(p.bg_main);
            assert!(
                cr >= WCAG_AAA_NORMAL_TEXT,
                "{name}: {status_name} on bg_main contrast {cr:.2} < {WCAG_AAA_NORMAL_TEXT}"
            );
        }

        // All Prism syntax tokens: WCAG AAA >= 7.0:1 on prism_bg
        for (tok_name, color) in [
            ("prism_fg", p.prism_fg),
            ("prism_comment", p.prism_comment),
            ("prism_keyword", p.prism_keyword),
            ("prism_string", p.prism_string),
            ("prism_number", p.prism_number),
            ("prism_function", p.prism_function),
            ("prism_operator", p.prism_operator),
            ("prism_type", p.prism_type),
            ("prism_macro", p.prism_macro),
        ] {
            let cr = color.contrast_ratio(p.prism_bg);
            assert!(
                cr >= WCAG_AAA_NORMAL_TEXT,
                "{name}: {tok_name} on prism_bg contrast {cr:.2} < {WCAG_AAA_NORMAL_TEXT}"
            );
        }

        // All 12 semantic pin classes: AAA on bg_main, AA on bg_hover
        for &pin in PinClass::ALL {
            let c = pin.color_in(p);
            let cr_main = c.contrast_ratio(p.bg_main);
            assert!(
                cr_main >= WCAG_AAA_NORMAL_TEXT,
                "{name}: PinClass::{pin:?} ({}) on bg_main contrast {cr_main:.2} < {WCAG_AAA_NORMAL_TEXT}",
                c.to_hex_string()
            );
            let cr_hover = c.contrast_ratio(p.bg_hover);
            assert!(
                cr_hover >= WCAG_AA_NORMAL_TEXT,
                "{name}: PinClass::{pin:?} on bg_hover contrast {cr_hover:.2} < {WCAG_AA_NORMAL_TEXT}"
            );
        }
    }

    #[test]
    fn wcag_dark_and_light_palettes_both_pass_aaa_and_aa() {
        assert_palette_wcag("DARK", &ThemePalette::DARK);
        assert_palette_wcag("LIGHT", &ThemePalette::LIGHT);
    }

    #[test]
    fn theme_mode_defaults_to_system_and_resolves_deterministically() {
        assert_eq!(ThemeMode::default(), ThemeMode::System);
        assert_eq!(ThemeMode::System.resolve(Some(true)), ResolvedTheme::Dark);
        assert_eq!(ThemeMode::System.resolve(Some(false)), ResolvedTheme::Light);
        assert_eq!(ThemeMode::System.resolve(None), ResolvedTheme::Dark);
        assert_eq!(ThemeMode::Dark.resolve(Some(false)), ResolvedTheme::Dark);
        assert_eq!(ThemeMode::Light.resolve(Some(true)), ResolvedTheme::Light);
        assert_eq!(ThemeMode::System.next(), ThemeMode::Light);
        assert_eq!(ThemeMode::Light.next(), ThemeMode::Dark);
        assert_eq!(ThemeMode::Dark.next(), ThemeMode::System);
        assert_eq!(
            ThemePalette::for_mode(ThemeMode::System, Some(false)).theme,
            ResolvedTheme::Light
        );
    }

    #[test]
    fn pin_class_bijection_covers_all_12_variants() {
        assert_eq!(PinClass::ALL.len(), 12);
        for &pin in PinClass::ALL {
            assert_eq!(PinClass::from_css_class(pin.css_class()), Some(pin));
            assert_eq!(PinClass::from_toml_role(pin.toml_role()), Some(pin));
            assert_eq!(pin.color(), pin.color_in(&ThemePalette::DARK));
        }
        assert_eq!(PinClass::from_css_class("df-pin-unknown"), None);
        assert_eq!(PinClass::from_toml_role("unknown"), None);
    }

    #[test]
    fn hex_formatting_handles_opaque_and_translucent() {
        assert_eq!(DF_BG_MAIN.to_hex_string(), "#0f172a");
        assert_eq!(
            Color::from_rgba_hex(0x34_d3_99_aa).to_hex_string(),
            "#34d399aa"
        );
        assert_eq!(Color::from_rgb_hex(0x01_02_03).to_rgba_u8(), [1, 2, 3, 255]);
    }
}
