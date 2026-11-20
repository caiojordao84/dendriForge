#![allow(dead_code)]
use iced::{widget::{button, container}, Background, Border, Color, Theme};

pub const fn rgb(hex: u32) -> Color {
    Color::from_rgb(((hex >> 16) & 255) as f32 / 255.0,
        ((hex >> 8) & 255) as f32 / 255.0, (hex & 255) as f32 / 255.0)
}
pub const PANEL: Color = rgb(0x020617);
pub const LOWEST: Color = rgb(0x060e20);
pub const SURFACE: Color = rgb(0x0b1326);
pub const MAIN: Color = rgb(0x0f172a);
pub const LOW: Color = rgb(0x131b2e);
pub const CARD: Color = rgb(0x171f33);
pub const HOVER: Color = rgb(0x1e293b);
pub const BORDER: Color = rgb(0x334155);
pub const MUTED: Color = rgb(0x94a3b8);
pub const TEXT: Color = rgb(0xe2e8f0);
pub const HEADING: Color = rgb(0xf8fafc);
pub const EDITOR: Color = rgb(0x2b2b2b);
pub const SKY: Color = rgb(0x38bdf8);
pub const CYAN: Color = rgb(0x22d3ee);
pub const ORANGE: Color = rgb(0xff8c00);
pub const EMERALD: Color = rgb(0x34d399);
pub const ROSE: Color = rgb(0xf43f5e);
pub const PURPLE: Color = rgb(0xa78bfa);
pub const SECONDARY: Color = rgb(0xcebdff);
pub const DIGITAL: Color = rgb(0x00e0ff);
pub const ANALOG: Color = rgb(0xabe338);
pub const POWER: Color = rgb(0xfc8181);
pub const GROUND: Color = rgb(0xa0aec0);
pub const ICSP: Color = rgb(0x94d2bd);

pub fn slate() -> Theme {
    Theme::custom("DendriForge Slate".into(), iced::theme::Palette {
        background: MAIN, text: TEXT, primary: SKY, success: EMERALD, danger: ROSE,
    })
}
pub fn surface(color: Color) -> container::Style {
    container::Style { background: Some(Background::Color(color)), text_color: Some(TEXT),
        ..Default::default() }
}
pub fn panel(_: &Theme) -> container::Style { surface(PANEL) }
pub fn card(_: &Theme) -> container::Style {
    container::Style { border: Border { color: BORDER, width: 1.0, radius: 4.0.into() },
        ..surface(LOW) }
}
pub fn editor(_: &Theme) -> container::Style { surface(EDITOR) }
pub fn control(_: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered => BORDER,
        button::Status::Pressed => LOWEST,
        _ => HOVER,
    };
    button::Style { background: Some(background.into()),
        text_color: if status == button::Status::Disabled { MUTED } else { TEXT },
        border: Border { color: BORDER, width: 1.0, radius: 3.0.into() },
        ..Default::default() }
}
pub fn selected(_: &Theme, _: button::Status) -> button::Style {
    button::Style { background: Some(SKY.into()), text_color: rgb(0x00354a),
        border: Border { radius: 3.0.into(), ..Default::default() }, ..Default::default() }
}
pub fn primary(_: &Theme, _: button::Status) -> button::Style {
    button::Style { background: Some(ORANGE.into()), text_color: PANEL,
        border: Border { radius: 3.0.into(), ..Default::default() }, ..Default::default() }
}
