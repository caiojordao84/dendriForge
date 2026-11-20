//! A11Y Slate tokens from Caio's canvas mockups (`dendriForge-A11Y-Slate`).

use iced::widget::button;
use iced::{Background, Border, Color, Theme};

pub const BG_PANEL: Color = Color::from_rgb(0.008, 0.024, 0.090); // #020617
pub const BG_MAIN: Color = Color::from_rgb(0.059, 0.090, 0.165); // #0f172a
pub const SURFACE: Color = Color::from_rgb(0.043, 0.075, 0.149); // #0b1326
pub const SURFACE_LOW: Color = Color::from_rgb(0.075, 0.106, 0.180); // #131b2e
pub const SURFACE_LOWEST: Color = Color::from_rgb(0.024, 0.055, 0.125); // #060e20
pub const BORDER: Color = Color::from_rgb(0.118, 0.161, 0.231); // #1e293b
pub const BORDER_LIGHT: Color = Color::from_rgb(0.200, 0.255, 0.333); // #334155
pub const TEXT_MAIN: Color = Color::from_rgb(0.886, 0.910, 0.941); // #e2e8f0
pub const TEXT_MUTED: Color = Color::from_rgb(0.580, 0.639, 0.722); // #94a3b8
pub const TEXT_HEADING: Color = Color::from_rgb(0.973, 0.980, 0.988); // #f8fafc
pub const ACCENT_ORANGE: Color = Color::from_rgb(1.0, 0.549, 0.0); // #ff8c00
pub const ACCENT_EMERALD: Color = Color::from_rgb(0.204, 0.827, 0.600); // #34d399
pub const ACCENT_SKY: Color = Color::from_rgb(0.220, 0.741, 0.973); // #38bdf8
pub const ACCENT_CYAN: Color = Color::from_rgb(0.133, 0.827, 0.933); // #22d3ee
pub const EXPORT_BLUE: Color = Color::from_rgb(0.145, 0.388, 0.922); // #2563eb
pub const ON_AMBER: Color = Color::from_rgb(0.008, 0.024, 0.090); // slate-950
pub const ACCENT_AMBER: Color = Color::from_rgb(0.961, 0.620, 0.043); // #f59e0b
pub const ACCENT_PURPLE: Color = Color::from_rgb(0.655, 0.545, 0.980); // #a78bfa
pub const LED_RED: Color = Color::from_rgb(0.937, 0.267, 0.267); // #ef4444
pub const BG_EDITOR: Color = Color::from_rgb(0.169, 0.169, 0.169); // #2b2b2b

pub fn panel_bg() -> Background {
    Background::Color(BG_PANEL)
}

pub fn main_bg() -> Background {
    Background::Color(BG_MAIN)
}

pub fn surface_bg() -> Background {
    Background::Color(SURFACE)
}

pub fn surface_low_bg() -> Background {
    Background::Color(SURFACE_LOW)
}

pub fn surface_lowest_bg() -> Background {
    Background::Color(SURFACE_LOWEST)
}

fn rounded(radius: f32) -> Border {
    Border {
        color: BORDER,
        width: 1.0,
        radius: radius.into(),
    }
}

pub fn tab_active(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(Color::from_rgb(0.090, 0.133, 0.231))), // slate-750
        text_color: TEXT_HEADING,
        border: Border {
            color: BORDER,
            width: 0.0,
            radius: 4.0.into(),
        },
        ..button::Style::default()
    }
}

pub fn tab_idle(_theme: &Theme, status: button::Status) -> button::Style {
    let text = match status {
        button::Status::Hovered => TEXT_HEADING,
        _ => TEXT_MUTED,
    };
    button::Style {
        background: Some(Background::Color(Color::TRANSPARENT)),
        text_color: text,
        border: Border {
            radius: 4.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

pub fn pause_btn(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => Color::from_rgb(0.878, 0.482, 0.0), // amberHover
        _ => ACCENT_ORANGE,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: ON_AMBER,
        border: Border {
            radius: 4.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

pub fn transport_btn(_theme: &Theme, status: button::Status) -> button::Style {
    let (bg, text) = match status {
        button::Status::Hovered => (SURFACE_LOW, TEXT_HEADING),
        _ => (Color::TRANSPARENT, TEXT_MUTED),
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: text,
        border: Border {
            radius: 4.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

pub fn export_btn(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => Color::from_rgb(0.231, 0.510, 0.965),
        _ => EXPORT_BLUE,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: Color::WHITE,
        border: Border {
            radius: 4.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

pub fn chrome_btn(_theme: &Theme, status: button::Status) -> button::Style {
    let (bg, text) = match status {
        button::Status::Hovered => (BORDER, TEXT_HEADING),
        button::Status::Pressed => (BORDER_LIGHT, TEXT_HEADING),
        _ => (SURFACE_LOW, TEXT_MAIN),
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: text,
        border: rounded(4.0),
        ..button::Style::default()
    }
}

pub fn palette_card(_theme: &Theme, status: button::Status) -> button::Style {
    let border = match status {
        button::Status::Hovered | button::Status::Pressed => BORDER_LIGHT,
        _ => BORDER,
    };
    button::Style {
        background: Some(Background::Color(SURFACE_LOWEST)),
        text_color: TEXT_MAIN,
        border: Border {
            color: border,
            width: 1.0,
            radius: 4.0.into(),
        },
        ..button::Style::default()
    }
}

pub fn palette_card_active(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(SURFACE_LOWEST)),
        text_color: TEXT_MAIN,
        border: Border {
            color: ACCENT_ORANGE,
            width: 2.0,
            radius: 4.0.into(),
        },
        ..button::Style::default()
    }
}
