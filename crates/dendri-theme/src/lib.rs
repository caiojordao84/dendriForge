//! Single source of truth for the `dendriForge` v2 **A11Y Slate** palette,
//! semantic `<g id="df-pins">` pin classes (`03-asset-and-profile-contract.md` §2.2),
//! and deterministic WCAG 2.1 contrast calculation.
//!
//! Zero UI-framework dependency (Layer 0): adapters in `dendri-canvas` and
//! `dendri-editor` convert [`Color`] into `egui::Color32` cleanly without
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
pub const DF_STATUS_MANUAL: Color = DF_ACCENT_PURPLE;
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

    /// Authoritative A11Y Slate paint color for this pin class.
    #[must_use]
    pub const fn color(self) -> Color {
        match self {
            Self::Power => DF_PIN_POWER,
            Self::Gnd => DF_PIN_GND,
            Self::AcPhase => DF_PIN_AC_PHASE,
            Self::Digital => DF_PIN_DIGITAL,
            Self::Analog => DF_PIN_ANALOG,
            Self::Pwm => DF_PIN_PWM,
            Self::Comm => DF_PIN_COMM,
            Self::Interrupt => DF_PIN_INTERRUPT,
            Self::Reset => DF_PIN_RESET,
            Self::Passive => DF_PIN_PASSIVE,
            Self::Pneumatic => DF_PIN_PNEUMATIC,
            Self::Hydraulic => DF_PIN_HYDRAULIC,
        }
    }
}

impl fmt::Display for PinClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.css_class())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wcag_aaa_primary_and_heading_text_across_surfaces() {
        for (surface_name, bg) in [
            ("DF_BG_MAIN", DF_BG_MAIN),
            ("DF_BG_PANEL", DF_BG_PANEL),
            ("DF_BG_CARD", DF_BG_CARD),
            ("DF_BG_HOVER", DF_BG_HOVER),
            ("DF_BG_SIDEBAR", DF_BG_SIDEBAR),
        ] {
            let main_cr = DF_TEXT_MAIN.contrast_ratio(bg);
            assert!(
                main_cr >= WCAG_AAA_NORMAL_TEXT,
                "DF_TEXT_MAIN on {surface_name} contrast {main_cr:.2} < {WCAG_AAA_NORMAL_TEXT}"
            );
            let head_cr = DF_TEXT_HEADING.contrast_ratio(bg);
            assert!(
                head_cr >= WCAG_AAA_NORMAL_TEXT,
                "DF_TEXT_HEADING on {surface_name} contrast {head_cr:.2} < {WCAG_AAA_NORMAL_TEXT}"
            );
        }
    }

    #[test]
    fn wcag_aa_muted_text_across_surfaces() {
        for (surface_name, bg) in [
            ("DF_BG_MAIN", DF_BG_MAIN),
            ("DF_BG_PANEL", DF_BG_PANEL),
            ("DF_BG_CARD", DF_BG_CARD),
            ("DF_BG_HOVER", DF_BG_HOVER),
        ] {
            let cr = DF_TEXT_MUTED.contrast_ratio(bg);
            assert!(
                cr >= WCAG_AA_NORMAL_TEXT,
                "DF_TEXT_MUTED on {surface_name} contrast {cr:.2} < {WCAG_AA_NORMAL_TEXT}"
            );
        }
    }

    #[test]
    fn wcag_aaa_primary_button_normal_and_hover() {
        let normal = DF_BG_PANEL.contrast_ratio(DF_ACCENT_ORANGE);
        assert!(
            normal >= WCAG_AAA_NORMAL_TEXT,
            "primary button normal contrast {normal:.2} < 7.0"
        );
        let hover = DF_BG_PANEL.contrast_ratio(DF_ACCENT_ORANGE_HOVER);
        assert!(
            hover >= WCAG_AAA_NORMAL_TEXT,
            "primary button hover contrast {hover:.2} < 7.0"
        );
    }

    #[test]
    fn wcag_aa_non_text_ladder_rails_and_focus() {
        let inactive = DF_LADDER_RAIL_INACTIVE.contrast_ratio(DF_BG_MAIN);
        assert!(
            inactive >= WCAG_AA_NON_TEXT,
            "inactive Ladder rail contrast {inactive:.2} < {WCAG_AA_NON_TEXT}"
        );
        let active = DF_LADDER_RAIL_ACTIVE.contrast_ratio(DF_BG_MAIN);
        assert!(
            active >= WCAG_AAA_NORMAL_TEXT,
            "active Ladder rail contrast {active:.2} < {WCAG_AAA_NORMAL_TEXT}"
        );
        let focus = DF_ACCENT_SKY.contrast_ratio(DF_BG_MAIN);
        assert!(
            focus >= WCAG_AAA_NORMAL_TEXT,
            "focus outline contrast {focus:.2} < {WCAG_AAA_NORMAL_TEXT}"
        );
    }

    #[test]
    fn wcag_aaa_all_prism_syntax_tokens_on_editor_bg() {
        for (name, color) in [
            ("PRISM_FG", PRISM_FG),
            ("PRISM_COMMENT", PRISM_COMMENT),
            ("PRISM_KEYWORD", PRISM_KEYWORD),
            ("PRISM_STRING", PRISM_STRING),
            ("PRISM_NUMBER", PRISM_NUMBER),
            ("PRISM_FUNCTION", PRISM_FUNCTION),
            ("PRISM_OPERATOR", PRISM_OPERATOR),
            ("PRISM_TYPE", PRISM_TYPE),
            ("PRISM_MACRO", PRISM_MACRO),
        ] {
            let cr = color.contrast_ratio(PRISM_BG);
            assert!(
                cr >= WCAG_AAA_NORMAL_TEXT,
                "{name} on PRISM_BG contrast {cr:.2} < {WCAG_AAA_NORMAL_TEXT}"
            );
        }
    }

    #[test]
    fn wcag_aaa_all_12_semantic_pin_classes_and_bijection() {
        assert_eq!(PinClass::ALL.len(), 12);
        for &pin in PinClass::ALL {
            assert_eq!(PinClass::from_css_class(pin.css_class()), Some(pin));
            assert_eq!(PinClass::from_toml_role(pin.toml_role()), Some(pin));
            let cr_main = pin.color().contrast_ratio(DF_BG_MAIN);
            assert!(
                cr_main >= WCAG_AAA_NORMAL_TEXT,
                "PinClass::{pin:?} ({}) on DF_BG_MAIN contrast {cr_main:.2} < {WCAG_AAA_NORMAL_TEXT}",
                pin.color().to_hex_string()
            );
            let cr_hover = pin.color().contrast_ratio(DF_BG_HOVER);
            assert!(
                cr_hover >= WCAG_AA_NORMAL_TEXT,
                "PinClass::{pin:?} on DF_BG_HOVER contrast {cr_hover:.2} < {WCAG_AA_NORMAL_TEXT}"
            );
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
