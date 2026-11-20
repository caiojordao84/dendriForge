//! DendriForge A11Y Slate theme for iced.
//!
//! **Source of truth:** `core/styles/dendriForge-A11Y-Slate.css` (v0.6.0).
//! This module mirrors every `:root` design token from that file. CSS `var()` /
//! forward refs are resolved to concrete values here.
//!
//! Short aliases (`PANEL`, `DIGITAL`, …) keep existing iced chrome call sites;
//! `df_*` / `prism_*` names match the CSS custom properties 1:1.
//!
//! **Not Color tokens (documented, not silently skipped):**
//! - Font stacks (`Roboto`, `Roboto Condensed`, `Roboto Mono`) — iced uses
//!   `Font` separately; see [`FONT_UI`], [`FONT_HEADING`], [`FONT_MONO`].
//! - CSS-only rules (scrollbar, status badge `color-mix`, SVG class selectors,
//!   `@keyframes`) are behavioural CSS, not palette tokens.
//!
//! **Editor chrome extras** (used by iced layout; not in the CSS file):
//! [`LOWEST`], [`SURFACE`], [`LOW`], [`CARD`], [`SECONDARY`].

#![allow(dead_code)]
use iced::{
    widget::{button, container},
    Background, Border, Color, Theme,
};

pub const fn rgb(hex: u32) -> Color {
    Color::from_rgb(
        ((hex >> 16) & 255) as f32 / 255.0,
        ((hex >> 8) & 255) as f32 / 255.0,
        (hex & 255) as f32 / 255.0,
    )
}

/// 8-digit `#RRGGBBAA` (CSS glow tokens).
pub const fn rgba(hex: u32) -> Color {
    Color::from_rgba(
        ((hex >> 24) & 255) as f32 / 255.0,
        ((hex >> 16) & 255) as f32 / 255.0,
        ((hex >> 8) & 255) as f32 / 255.0,
        (hex & 255) as f32 / 255.0,
    )
}

// ---------------------------------------------------------------------------
// CSS SoT — Layout backgrounds (`--df-bg-*`)
// ---------------------------------------------------------------------------
/// `--df-bg-main` / `--df-bg-terminal` slate-900
pub const DF_BG_MAIN: Color = rgb(0x0f172a);
/// `--df-bg-panel` slate-950
pub const DF_BG_PANEL: Color = rgb(0x020617);
/// `--df-bg-sidebar`
pub const DF_BG_SIDEBAR: Color = rgb(0x1b1e2b);
/// `--df-bg-editor` / `--prism-bg`
pub const DF_BG_EDITOR: Color = rgb(0x2b2b2b);
/// `--df-bg-hover` / `--df-border-main` slate-800
pub const DF_BG_HOVER: Color = rgb(0x1e293b);
/// `--df-bg-terminal` → same as main
pub const DF_BG_TERMINAL: Color = DF_BG_MAIN;

// ---------------------------------------------------------------------------
// CSS SoT — Borders (`--df-border-*`)
// ---------------------------------------------------------------------------
/// `--df-border-main` → `--df-bg-hover`
pub const DF_BORDER_MAIN: Color = DF_BG_HOVER;
/// `--df-border-light` slate-700
pub const DF_BORDER_LIGHT: Color = rgb(0x334155);
/// `--df-border-subtle` slate-600
pub const DF_BORDER_SUBTLE: Color = rgb(0x475569);

// ---------------------------------------------------------------------------
// CSS SoT — Interface text (`--df-text-*`)
// ---------------------------------------------------------------------------
/// `--df-text-main` slate-200
pub const DF_TEXT_MAIN: Color = rgb(0xe2e8f0);
/// `--df-text-heading` slate-50
pub const DF_TEXT_HEADING: Color = rgb(0xf8fafc);
/// `--df-text-muted` slate-400
pub const DF_TEXT_MUTED: Color = rgb(0x94a3b8);
/// `--df-color-slate-muted` / `--df-text-faded` / `--df-status-disconnected` / `--df-sim-pin-low`
pub const DF_COLOR_SLATE_MUTED: Color = rgb(0x64748b);
pub const DF_TEXT_FADED: Color = DF_COLOR_SLATE_MUTED;
pub const DF_STATUS_DISCONNECTED: Color = DF_COLOR_SLATE_MUTED;
pub const DF_SIM_PIN_LOW: Color = DF_COLOR_SLATE_MUTED;

// ---------------------------------------------------------------------------
// CSS SoT — Accents (`--df-accent-*`)
// ---------------------------------------------------------------------------
pub const DF_ACCENT_SKY: Color = rgb(0x38bdf8);
pub const DF_ACCENT_SKY_LIGHT: Color = rgb(0x00b4d8);
pub const DF_ACCENT_AMBER: Color = rgb(0xf59e0b);
pub const DF_ACCENT_AMBER_LIGHT: Color = rgb(0xfbbf24);
pub const DF_ACCENT_ORANGE: Color = rgb(0xff8c00);
pub const DF_ACCENT_ORANGE_DIM: Color = rgb(0x7c3f00);
pub const DF_ACCENT_ROSE: Color = rgb(0xf43f5e);
pub const DF_ACCENT_EMERALD: Color = rgb(0x34d399);
pub const DF_ACCENT_CYAN: Color = rgb(0x22d3ee);
pub const DF_ACCENT_MAGENTA: Color = rgb(0xd946ef);
pub const DF_ACCENT_PURPLE: Color = rgb(0xa78bfa);
pub const DF_ACCENT_PURPLE_LIGHT: Color = rgb(0xe9d5ff);

// ---------------------------------------------------------------------------
// CSS SoT — Runtime status (`--df-status-*`)
// ---------------------------------------------------------------------------
pub const DF_STATUS_RUNNING: Color = DF_ACCENT_EMERALD;
pub const DF_STATUS_PAUSED: Color = DF_ACCENT_AMBER;
pub const DF_STATUS_FAULT: Color = DF_ACCENT_ROSE;
pub const DF_STATUS_MANUAL: Color = DF_ACCENT_PURPLE;
pub const DF_STATUS_LIVE: Color = DF_ACCENT_SKY;
pub const DF_STATUS_VIRTUAL: Color = DF_ACCENT_CYAN;

// ---------------------------------------------------------------------------
// CSS SoT — Simulation & canvas (`--df-sim-*`)
// ---------------------------------------------------------------------------
pub const DF_SIM_COMP_BG: Color = DF_BG_HOVER;
pub const DF_SIM_COMP_MCU: Color = DF_BG_MAIN;
pub const DF_SIM_COMP_STROKE: Color = DF_BORDER_SUBTLE;
pub const DF_SIM_WIRE_DEFAULT: Color = DF_ACCENT_SKY;
/// `--df-sim-wire-vcc` red-500
pub const DF_SIM_WIRE_VCC: Color = rgb(0xef4444);
/// `--df-sim-wire-gnd` → `--df-pin-gnd`
pub const DF_SIM_WIRE_GND: Color = rgb(0xa0aec0);
pub const DF_SIM_PIN_HIGH: Color = DF_ACCENT_EMERALD;
/// `--df-sim-multimeter` yellow-500
pub const DF_SIM_MULTIMETER: Color = rgb(0xeab308);

// ---------------------------------------------------------------------------
// CSS SoT — LED colors / glows (`--df-led-*`)
// ---------------------------------------------------------------------------
pub const DF_LED_OFF_BASE: Color = rgb(0x1a1a1a);
pub const DF_LED_OFF_IDLE: Color = rgb(0x2a2a2a);
pub const DF_LED_COLOR_ORANGE: Color = DF_ACCENT_ORANGE;
/// `--df-led-color-yellow` → `--prism-number`
pub const DF_LED_COLOR_YELLOW: Color = rgb(0xffd700);
pub const DF_LED_COLOR_GREEN: Color = DF_ACCENT_EMERALD;
pub const DF_LED_COLOR_RED: Color = DF_ACCENT_ROSE;
pub const DF_LED_GLOW_ORANGE: Color = rgba(0xff8c00aa);
pub const DF_LED_GLOW_YELLOW: Color = rgba(0xffd700aa);
pub const DF_LED_GLOW_GREEN: Color = rgba(0x34d399aa);
pub const DF_LED_GLOW_RED: Color = rgba(0xf43f5eaa);
pub const DF_LED_GLOW_GREEN_DIM: Color = rgba(0x34d39944);
pub const DF_LED_GLOW_YELLOW_OFF: Color = rgba(0xffd70000);

// ---------------------------------------------------------------------------
// CSS SoT — Prism A11Y Dark (`--prism-*`)
// ---------------------------------------------------------------------------
pub const PRISM_BG: Color = DF_BG_EDITOR;
pub const PRISM_FG: Color = rgb(0xf8f8f2);
pub const PRISM_COMMENT: Color = rgb(0xd4d0ab);
pub const PRISM_KEYWORD: Color = rgb(0xffa07a);
pub const PRISM_STRING: Color = rgb(0xabe338);
pub const PRISM_NUMBER: Color = rgb(0xffd700);
pub const PRISM_FUNCTION: Color = rgb(0x60caff);
pub const PRISM_OPERATOR: Color = rgb(0x00e0ff);
pub const PRISM_TYPE: Color = rgb(0xdcc6e0);
pub const PRISM_MACRO: Color = rgb(0xc4b5fd);

// ---------------------------------------------------------------------------
// CSS SoT — MCU pin colors (`--df-pin-*`) — wire/pin palette for Phase B
// ---------------------------------------------------------------------------
/// `--df-pin-digital` → `--prism-operator`
pub const DF_PIN_DIGITAL: Color = PRISM_OPERATOR;
/// `--df-pin-pwm` Amarelo Ouro
pub const DF_PIN_PWM: Color = rgb(0xffd700);
/// `--df-pin-analog` → `--prism-string`
pub const DF_PIN_ANALOG: Color = PRISM_STRING;
pub const DF_PIN_POWER: Color = rgb(0xfc8181);
pub const DF_PIN_GND: Color = rgb(0xa0aec0);
/// `--df-pin-i2c` → `--prism-type`
pub const DF_PIN_I2C: Color = PRISM_TYPE;
/// `--df-pin-spi` → `--prism-function`
pub const DF_PIN_SPI: Color = PRISM_FUNCTION;
/// `--df-pin-uart` → `--prism-macro`
pub const DF_PIN_UART: Color = PRISM_MACRO;
pub const DF_PIN_NC: Color = rgb(0xff8080);
pub const DF_PIN_ICSP: Color = rgb(0x94d2bd);
/// `--df-pin-reset` Laranja Vivo
pub const DF_PIN_RESET: Color = rgb(0xf97316);
/// `--df-pin-aref` → `--df-pin-gnd`
pub const DF_PIN_AREF: Color = DF_PIN_GND;
/// `--df-pin-interrupt`
pub const DF_PIN_INTERRUPT: Color = rgb(0xf43f5e);

// ---------------------------------------------------------------------------
// Font stacks (CSS `@import` / `font-family` — not Color)
// ---------------------------------------------------------------------------
pub const FONT_UI: &str = "Roboto";
pub const FONT_HEADING: &str = "Roboto Condensed";
pub const FONT_MONO: &str = "Roboto Mono";

// ---------------------------------------------------------------------------
// Short aliases used by iced chrome (map onto CSS SoT)
// ---------------------------------------------------------------------------
pub const PANEL: Color = DF_BG_PANEL;
pub const MAIN: Color = DF_BG_MAIN;
pub const HOVER: Color = DF_BG_HOVER;
pub const BORDER: Color = DF_BORDER_LIGHT;
pub const MUTED: Color = DF_TEXT_MUTED;
pub const TEXT: Color = DF_TEXT_MAIN;
pub const HEADING: Color = DF_TEXT_HEADING;
pub const EDITOR: Color = DF_BG_EDITOR;
pub const SKY: Color = DF_ACCENT_SKY;
pub const CYAN: Color = DF_ACCENT_CYAN;
pub const ORANGE: Color = DF_ACCENT_ORANGE;
pub const EMERALD: Color = DF_ACCENT_EMERALD;
pub const ROSE: Color = DF_ACCENT_ROSE;
pub const PURPLE: Color = DF_ACCENT_PURPLE;
pub const DIGITAL: Color = DF_PIN_DIGITAL;
pub const ANALOG: Color = DF_PIN_ANALOG;
pub const POWER: Color = DF_PIN_POWER;
pub const GROUND: Color = DF_PIN_GND;
pub const ICSP: Color = DF_PIN_ICSP;

// Editor chrome extras (not in CSS SoT — keep for layout depth).
pub const LOWEST: Color = rgb(0x060e20);
pub const SURFACE: Color = rgb(0x0b1326);
pub const LOW: Color = rgb(0x131b2e);
pub const CARD: Color = rgb(0x171f33);
/// Legacy chrome accent (closest CSS: `--df-accent-purple-light` = #e9d5ff).
pub const SECONDARY: Color = rgb(0xcebdff);

pub fn slate() -> Theme {
    Theme::custom(
        "DendriForge Slate".into(),
        iced::theme::Palette {
            background: MAIN,
            text: TEXT,
            primary: SKY,
            success: EMERALD,
            danger: ROSE,
        },
    )
}

pub fn surface(color: Color) -> container::Style {
    container::Style {
        background: Some(Background::Color(color)),
        text_color: Some(TEXT),
        ..Default::default()
    }
}

pub fn panel(_: &Theme) -> container::Style {
    surface(PANEL)
}

pub fn card(_: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: 4.0.into(),
        },
        ..surface(LOW)
    }
}

pub fn editor(_: &Theme) -> container::Style {
    surface(EDITOR)
}

pub fn control(_: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered => BORDER,
        button::Status::Pressed => LOWEST,
        _ => HOVER,
    };
    button::Style {
        background: Some(background.into()),
        text_color: if status == button::Status::Disabled {
            MUTED
        } else {
            TEXT
        },
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: 3.0.into(),
        },
        ..Default::default()
    }
}

pub fn selected(_: &Theme, _: button::Status) -> button::Style {
    button::Style {
        background: Some(SKY.into()),
        text_color: rgb(0x00354a),
        border: Border {
            radius: 3.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn primary(_: &Theme, _: button::Status) -> button::Style {
    button::Style {
        background: Some(ORANGE.into()),
        text_color: PANEL,
        border: Border {
            radius: 3.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Pin-role → fill color (CSS SVG board pin layer).
pub fn pin_fill(role: PinRole) -> Color {
    match role {
        PinRole::Digital => DF_PIN_DIGITAL,
        PinRole::Analog => DF_PIN_ANALOG,
        PinRole::Power => DF_PIN_POWER,
        PinRole::Gnd | PinRole::Aref => DF_PIN_GND,
        PinRole::Nc => DF_PIN_NC,
        PinRole::Reset => DF_PIN_RESET,
        PinRole::Icsp => DF_PIN_ICSP,
    }
}

/// Pin-role → stroke accent (PWM / protocols / interrupt).
pub fn pin_stroke(accent: PinStroke) -> Color {
    match accent {
        PinStroke::Pwm => DF_PIN_PWM,
        PinStroke::Interrupt => DF_PIN_INTERRUPT,
        PinStroke::Uart => DF_PIN_UART,
        PinStroke::I2c => DF_PIN_I2C,
        PinStroke::Spi => DF_PIN_SPI,
        PinStroke::Aref => DF_PIN_AREF,
        PinStroke::Icsp => DF_PIN_ICSP,
    }
}

/// Wire color from electrical role (Phase B2 will sequence conductors on top).
pub fn wire_color(role: WireRole) -> Color {
    match role {
        WireRole::Default => DF_SIM_WIRE_DEFAULT,
        WireRole::Vcc => DF_SIM_WIRE_VCC,
        WireRole::Gnd => DF_SIM_WIRE_GND,
        WireRole::InheritPin(p) => pin_fill(p),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinRole {
    Digital,
    Analog,
    Power,
    Gnd,
    Nc,
    Reset,
    Icsp,
    Aref,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinStroke {
    Pwm,
    Interrupt,
    Uart,
    I2c,
    Spi,
    Aref,
    Icsp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireRole {
    Default,
    Vcc,
    Gnd,
    InheritPin(PinRole),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_pin_and_wire_tokens_match_slate_hex() {
        assert_eq!(DF_PIN_DIGITAL, rgb(0x00e0ff));
        assert_eq!(DF_PIN_ANALOG, rgb(0xabe338));
        assert_eq!(DF_PIN_POWER, rgb(0xfc8181));
        assert_eq!(DF_PIN_GND, rgb(0xa0aec0));
        assert_eq!(DF_PIN_PWM, rgb(0xffd700));
        assert_eq!(DF_PIN_I2C, rgb(0xdcc6e0));
        assert_eq!(DF_PIN_SPI, rgb(0x60caff));
        assert_eq!(DF_PIN_UART, rgb(0xc4b5fd));
        assert_eq!(DF_PIN_NC, rgb(0xff8080));
        assert_eq!(DF_PIN_ICSP, rgb(0x94d2bd));
        assert_eq!(DF_PIN_RESET, rgb(0xf97316));
        assert_eq!(DF_PIN_INTERRUPT, rgb(0xf43f5e));
        assert_eq!(DF_SIM_WIRE_DEFAULT, rgb(0x38bdf8));
        assert_eq!(DF_SIM_WIRE_VCC, rgb(0xef4444));
        assert_eq!(DF_SIM_WIRE_GND, DF_PIN_GND);
        assert_eq!(wire_color(WireRole::Vcc), DF_SIM_WIRE_VCC);
        assert_eq!(
            wire_color(WireRole::InheritPin(PinRole::Digital)),
            DF_PIN_DIGITAL
        );
    }

    #[test]
    fn short_aliases_track_css_sot() {
        assert_eq!(PANEL, DF_BG_PANEL);
        assert_eq!(MAIN, DF_BG_MAIN);
        assert_eq!(DIGITAL, DF_PIN_DIGITAL);
        assert_eq!(ANALOG, DF_PIN_ANALOG);
        assert_eq!(POWER, DF_PIN_POWER);
        assert_eq!(GROUND, DF_PIN_GND);
        assert_eq!(ICSP, DF_PIN_ICSP);
    }
}
