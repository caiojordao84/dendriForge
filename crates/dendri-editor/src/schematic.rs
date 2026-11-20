//! Faithful static schematic composition matching the HTML Simulation mockup.
//!
//! iced `svg` widgets panic on damage-sort under this stack, so we draw the
//! board / LED / button / jumper layout with `canvas` while keeping live
//! LED + button indicators from `dendri-canvas` / sim state.

use iced::mouse;
use iced::widget::canvas::{self, Canvas, Frame, Geometry, Path, Stroke, Text};
use iced::{Color, Element, Point, Rectangle, Renderer, Size, Theme};

use crate::theme::{ACCENT_EMERALD, ACCENT_ORANGE, ACCENT_SKY, BORDER, SURFACE, TEXT_MAIN, TEXT_MUTED};
use crate::Message;

/// PCB teal from mockup Arduino Uno fill `#017e7b`.
const PCB_TEAL: Color = Color::from_rgb(0.004, 0.494, 0.482);
const PCB_DARK: Color = Color::from_rgb(0.090, 0.302, 0.310);
const LED_OFF: Color = Color::from_rgb(0.753, 0.224, 0.169); // #c0392b
const LED_ON: Color = Color::from_rgb(0.937, 0.267, 0.267); // #ef4444
const WIRE_GREEN: Color = Color::from_rgb(0.133, 0.773, 0.369); // #22c55e
const WIRE_GND: Color = Color::from_rgb(0.278, 0.333, 0.412); // #475569
const WIRE_LED_GND: Color = Color::from_rgb(0.392, 0.455, 0.545); // #64748b
const BTN_HOUSING: Color = Color::from_rgb(0.173, 0.243, 0.314); // #2c3e50
const BTN_ACTUATOR: Color = Color::from_rgb(0.153, 0.682, 0.376); // #27ae60
const BTN_PRESSED: Color = Color::from_rgb(0.063, 0.725, 0.506); // #10b981
const PANEL: Color = Color::from_rgb(0.059, 0.090, 0.165); // #0f172a

#[derive(Debug, Clone, Copy)]
pub struct SchematicView {
    pub led_on: bool,
    pub button_pressed: bool,
    pub button_pin: u32,
    pub led_pin: u32,
}

impl SchematicView {
    pub fn view(self) -> Element<'static, Message> {
        Canvas::new(self)
            .width(iced::Fill)
            .height(iced::Fill)
            .into()
    }
}

impl canvas::Program<Message> for SchematicView {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        let Some(pos) = cursor.position_in(bounds) else {
            return (canvas::event::Status::Ignored, None);
        };
        let layout = Layout::fit(bounds.size());
        let inside = layout.button_hit(pos);
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if inside => {
                (canvas::event::Status::Captured, Some(Message::SimPress))
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if self.button_pressed {
                    (canvas::event::Status::Captured, Some(Message::SimRelease))
                } else {
                    (canvas::event::Status::Ignored, None)
                }
            }
            _ => (canvas::event::Status::Ignored, None),
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let layout = Layout::fit(bounds.size());

        draw_grid(&mut frame, bounds.size());
        draw_wires(&mut frame, &layout);
        draw_board(&mut frame, &layout);
        draw_led(&mut frame, &layout, self.led_on);
        draw_button(&mut frame, &layout, self.button_pressed);
        draw_pullup_callout(&mut frame, &layout, self.button_pin);
        draw_labels(&mut frame, &layout, self.led_pin, self.button_pin, self.led_on);

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if let Some(pos) = cursor.position_in(bounds) {
            let layout = Layout::fit(bounds.size());
            if layout.button_hit(pos) {
                return mouse::Interaction::Pointer;
            }
        }
        mouse::Interaction::default()
    }
}

struct Layout {
    board: Rectangle,
    led: Point,
    button: Point,
    d2: Point,
    d13: Point,
    gnd: Point,
    led_a: Point,
    led_k: Point,
    btn_no: Point,
    btn_com: Point,
    scale: f32,
}

impl Layout {
    fn fit(size: Size) -> Self {
        let scale = (size.width / 960.0).min(size.height / 560.0).max(0.55);
        let ox = (size.width - 960.0 * scale) * 0.5;
        let oy = (size.height - 560.0 * scale) * 0.35;
        let map = |x: f32, y: f32| Point::new(ox + x * scale, oy + y * scale);
        let board = Rectangle {
            x: ox + 40.0 * scale,
            y: oy + 90.0 * scale,
            width: 420.0 * scale,
            height: 280.0 * scale,
        };
        Self {
            board,
            led: map(720.0, 140.0),
            button: map(720.0, 340.0),
            d2: map(390.0, 110.0),
            d13: map(210.0, 110.0),
            gnd: map(250.0, 350.0),
            led_a: map(690.0, 110.0),
            led_k: map(690.0, 185.0),
            btn_no: map(690.0, 380.0),
            btn_com: map(690.0, 300.0),
            scale,
        }
    }

    fn button_hit(&self, pos: Point) -> bool {
        let r = 38.0 * self.scale;
        let dx = pos.x - self.button.x;
        let dy = pos.y - self.button.y;
        dx * dx + dy * dy <= r * r
    }
}

fn draw_grid(frame: &mut Frame, size: Size) {
    frame.fill_rectangle(Point::ORIGIN, size, SURFACE);
    let step = 20.0;
    let mut y = 10.0;
    while y < size.height {
        let mut x = 10.0;
        while x < size.width {
            let dot = Path::circle(Point::new(x, y), 1.2);
            frame.fill(&dot, BORDER);
            x += step;
        }
        y += step;
    }
}

fn draw_board(frame: &mut Frame, layout: &Layout) {
    let r = layout.board;
    let body = Path::rounded_rectangle(
        Point::new(r.x, r.y),
        Size::new(r.width, r.height),
        8.0.into(),
    );
    frame.fill(&body, PCB_TEAL);
    frame.stroke(
        &body,
        Stroke::default()
            .with_width(1.5)
            .with_color(Color::from_rgb(0.0, 0.361, 0.518)),
    );

    // Infinity motif (two overlapping ovals approximated as circles)
    let cx = r.x + r.width * 0.55;
    let cy = r.y + r.height * 0.45;
    let loop_r = r.height * 0.14;
    frame.fill(
        &Path::circle(Point::new(cx - loop_r * 0.9, cy), loop_r),
        PCB_DARK,
    );
    frame.fill(
        &Path::circle(Point::new(cx + loop_r * 0.9, cy), loop_r),
        PCB_DARK,
    );

    // USB / jack hints
    let usb = Path::rectangle(
        Point::new(r.x - 18.0 * layout.scale, r.y + r.height * 0.18),
        Size::new(22.0 * layout.scale, 48.0 * layout.scale),
    );
    frame.fill(&usb, Color::from_rgb(0.345, 0.345, 0.345));

    let jack = Path::circle(
        Point::new(r.x + 18.0 * layout.scale, r.y + r.height * 0.78),
        10.0 * layout.scale,
    );
    frame.fill(&jack, Color::from_rgb(0.2, 0.2, 0.2));

    // MCU block
    let mcu = Path::rounded_rectangle(
        Point::new(r.x + r.width * 0.35, r.y + r.height * 0.55),
        Size::new(r.width * 0.45, r.height * 0.22),
        2.0.into(),
    );
    frame.fill(&mcu, Color::from_rgb(0.173, 0.188, 0.204));
    frame.fill_text(Text {
        content: "ATmega328P".into(),
        position: Point::new(r.x + r.width * 0.38, r.y + r.height * 0.68),
        color: Color::WHITE,
        size: iced::Pixels(11.0 * layout.scale.min(1.2)),
        font: iced::Font::MONOSPACE,
        horizontal_alignment: iced::alignment::Horizontal::Left,
        vertical_alignment: iced::alignment::Vertical::Center,
        ..Text::default()
    });

    // Digital header pads
    for i in 0..14 {
        let px = r.x + r.width * 0.28 + (i as f32) * (r.width * 0.045);
        let py = r.y + 14.0 * layout.scale;
        frame.fill(&Path::circle(Point::new(px, py), 3.0 * layout.scale), Color::from_rgb(0.85, 0.85, 0.85));
    }
    // Power / analog pads
    for i in 0..8 {
        let px = r.x + r.width * 0.35 + (i as f32) * (r.width * 0.055);
        let py = r.y + r.height - 14.0 * layout.scale;
        frame.fill(&Path::circle(Point::new(px, py), 3.0 * layout.scale), Color::from_rgb(0.85, 0.85, 0.85));
    }

    // Onboard pin-13 LED (amber when external LED on)
    let pin13 = Path::circle(
        Point::new(r.x + r.width * 0.48, r.y + r.height * 0.28),
        4.5 * layout.scale,
    );
    // lit when led_on — caller passes via separate draw; use accent when external on later
    frame.fill(&pin13, Color::from_rgb(0.16, 0.16, 0.16));

    frame.fill_text(Text {
        content: "Arduino Uno R3".into(),
        position: Point::new(r.x + 12.0 * layout.scale, r.y + 28.0 * layout.scale),
        color: Color::WHITE,
        size: iced::Pixels(13.0 * layout.scale.min(1.3)),
        font: iced::Font::DEFAULT,
        horizontal_alignment: iced::alignment::Horizontal::Left,
        vertical_alignment: iced::alignment::Vertical::Center,
        ..Text::default()
    });
}

fn draw_led(frame: &mut Frame, layout: &Layout, on: bool) {
    let s = layout.scale;
    let housing = Path::rounded_rectangle(
        Point::new(layout.led.x - 48.0 * s, layout.led.y - 48.0 * s),
        Size::new(96.0 * s, 96.0 * s),
        10.0.into(),
    );
    frame.fill(&housing, BTN_HOUSING);
    frame.stroke(
        &housing,
        Stroke::default()
            .with_width(1.0)
            .with_color(Color::from_rgb(0.627, 0.667, 0.686)),
    );

    let outer = Path::circle(layout.led, 34.0 * s);
    frame.fill(&outer, BTN_HOUSING);

    let body = Path::circle(layout.led, 30.0 * s);
    if on {
        // Soft glow rings
        for (r, a) in [(42.0, 0.18), (36.0, 0.28)] {
            let glow = Path::circle(layout.led, r * s);
            frame.fill(
                &glow,
                Color {
                    a,
                    ..LED_ON
                },
            );
        }
        frame.fill(&body, LED_ON);
    } else {
        frame.fill(
            &body,
            Color {
                a: 0.35,
                ..LED_OFF
            },
        );
    }

    // Lens highlight
    let lens = Path::circle(
        Point::new(layout.led.x - 6.0 * s, layout.led.y - 8.0 * s),
        10.0 * s,
    );
    frame.fill(
        &lens,
        Color {
            a: 0.35,
            ..Color::WHITE
        },
    );

    // RSMDΩ badge
    let badge = Path::rounded_rectangle(
        Point::new(layout.led.x - 16.0 * s, layout.led.y - 42.0 * s),
        Size::new(32.0 * s, 10.0 * s),
        2.0.into(),
    );
    frame.fill(&badge, Color::from_rgb(0.204, 0.286, 0.369));
    frame.fill_text(Text {
        content: "RSMDΩ".into(),
        position: Point::new(layout.led.x, layout.led.y - 37.0 * s),
        color: Color::from_rgb(0.922, 0.922, 0.922),
        size: iced::Pixels(8.0 * s.min(1.2)),
        font: iced::Font::MONOSPACE,
        horizontal_alignment: iced::alignment::Horizontal::Center,
        vertical_alignment: iced::alignment::Vertical::Center,
        ..Text::default()
    });

    // Anode / cathode pads
    frame.fill(&Path::circle(layout.led_a, 4.0 * s), LED_OFF);
    frame.fill(&Path::circle(layout.led_k, 4.0 * s), LED_OFF);
}

fn draw_button(frame: &mut Frame, layout: &Layout, pressed: bool) {
    let s = layout.scale;
    let housing = Path::rounded_rectangle(
        Point::new(layout.button.x - 48.0 * s, layout.button.y - 48.0 * s),
        Size::new(96.0 * s, 96.0 * s),
        10.0.into(),
    );
    frame.fill(&housing, BTN_HOUSING);

    let bezel = Path::circle(layout.button, 34.0 * s);
    frame.fill(&bezel, Color::from_rgb(0.498, 0.549, 0.553));
    frame.stroke(
        &bezel,
        Stroke::default()
            .with_width(1.2)
            .with_color(Color::from_rgb(0.337, 0.396, 0.451)),
    );

    let actuator_c = if pressed {
        Point::new(layout.button.x, layout.button.y + 2.5 * s)
    } else {
        layout.button
    };
    let actuator = Path::circle(actuator_c, 28.0 * s);
    frame.fill(&actuator, if pressed { BTN_PRESSED } else { BTN_ACTUATOR });
    if pressed {
        frame.fill(
            &Path::circle(actuator_c, 34.0 * s),
            Color {
                a: 0.25,
                ..BTN_PRESSED
            },
        );
    }

    frame.fill_text(Text {
        content: "NO".into(),
        position: actuator_c,
        color: Color::from_rgb(0.925, 0.941, 0.945),
        size: iced::Pixels(12.0 * s.min(1.2)),
        font: iced::Font::DEFAULT,
        horizontal_alignment: iced::alignment::Horizontal::Center,
        vertical_alignment: iced::alignment::Vertical::Center,
        ..Text::default()
    });

    // Connector pads
    for p in [
        Point::new(layout.button.x - 40.0 * s, layout.button.y - 40.0 * s),
        Point::new(layout.button.x + 40.0 * s, layout.button.y - 40.0 * s),
        Point::new(layout.button.x - 40.0 * s, layout.button.y + 40.0 * s),
        Point::new(layout.button.x + 40.0 * s, layout.button.y + 40.0 * s),
    ] {
        frame.fill(&Path::circle(p, 3.5 * s), Color::from_rgb(0.102, 0.145, 0.184));
    }
}

fn draw_wires(frame: &mut Frame, layout: &Layout) {
    // Green: D2 → button NO
    stroke_curve(
        frame,
        layout.d2,
        layout.btn_no,
        Point::new(layout.d2.x + 80.0 * layout.scale, layout.d2.y - 40.0 * layout.scale),
        Point::new(layout.btn_no.x - 40.0 * layout.scale, layout.btn_no.y - 20.0 * layout.scale),
        WIRE_GREEN,
        4.0,
    );
    // Black: button COM → GND
    stroke_curve(
        frame,
        layout.btn_com,
        layout.gnd,
        Point::new(layout.btn_com.x - 60.0 * layout.scale, layout.btn_com.y + 40.0 * layout.scale),
        Point::new(layout.gnd.x + 80.0 * layout.scale, layout.gnd.y + 20.0 * layout.scale),
        WIRE_GND,
        4.0,
    );
    // Sky: D13 → LED anode
    stroke_curve(
        frame,
        layout.d13,
        layout.led_a,
        Point::new(layout.d13.x + 40.0 * layout.scale, layout.d13.y - 50.0 * layout.scale),
        Point::new(layout.led_a.x - 80.0 * layout.scale, layout.led_a.y - 10.0 * layout.scale),
        ACCENT_SKY,
        3.5,
    );
    // Dashed slate: LED cathode → GND header
    stroke_curve(
        frame,
        layout.led_k,
        Point::new(layout.d13.x - 12.0 * layout.scale, layout.d13.y),
        Point::new(layout.led_k.x - 100.0 * layout.scale, layout.led_k.y + 10.0 * layout.scale),
        Point::new(layout.d13.x + 40.0 * layout.scale, layout.d13.y + 30.0 * layout.scale),
        WIRE_LED_GND,
        3.0,
    );

    for (p, c) in [
        (layout.d2, WIRE_GREEN),
        (layout.btn_no, WIRE_GREEN),
        (layout.btn_com, WIRE_GND),
        (layout.gnd, WIRE_GND),
        (layout.d13, ACCENT_SKY),
        (layout.led_a, ACCENT_SKY),
        (layout.led_k, WIRE_LED_GND),
    ] {
        frame.fill(&Path::circle(p, 3.5 * layout.scale), c);
        frame.stroke(
            &Path::circle(p, 3.5 * layout.scale),
            Stroke::default()
                .with_width(1.2)
                .with_color(Color::WHITE),
        );
    }
}

fn stroke_curve(
    frame: &mut Frame,
    from: Point,
    to: Point,
    c1: Point,
    c2: Point,
    color: Color,
    width: f32,
) {
    let path = Path::new(|b| {
        b.move_to(from);
        b.bezier_curve_to(c1, c2, to);
    });
    frame.stroke(
        &path,
        Stroke::default().with_width(width).with_color(color),
    );
}

fn draw_pullup_callout(frame: &mut Frame, layout: &Layout, button_pin: u32) {
    let s = layout.scale;
    let origin = Point::new(
        layout.board.x + layout.board.width * 0.55,
        layout.board.y - 8.0 * s,
    );
    let box_path = Path::rounded_rectangle(origin, Size::new(168.0 * s, 36.0 * s), 4.0.into());
    frame.fill(
        &box_path,
        Color {
            a: 0.92,
            ..PANEL
        },
    );
    frame.stroke(
        &box_path,
        Stroke::default()
            .with_width(1.0)
            .with_color(ACCENT_EMERALD),
    );
    frame.fill_text(Text {
        content: format!("D{button_pin} · MCU pull-up"),
        position: Point::new(origin.x + 8.0 * s, origin.y + 12.0 * s),
        color: Color::from_rgb(0.431, 0.906, 0.718),
        size: iced::Pixels(10.0 * s.min(1.2)),
        font: iced::Font::MONOSPACE,
        horizontal_alignment: iced::alignment::Horizontal::Left,
        vertical_alignment: iced::alignment::Vertical::Center,
        ..Text::default()
    });
    frame.fill_text(Text {
        content: "pressed → active-LOW".into(),
        position: Point::new(origin.x + 8.0 * s, origin.y + 24.0 * s),
        color: TEXT_MUTED,
        size: iced::Pixels(9.0 * s.min(1.2)),
        font: iced::Font::MONOSPACE,
        horizontal_alignment: iced::alignment::Horizontal::Left,
        vertical_alignment: iced::alignment::Vertical::Center,
        ..Text::default()
    });
}

fn draw_labels(
    frame: &mut Frame,
    layout: &Layout,
    led_pin: u32,
    button_pin: u32,
    led_on: bool,
) {
    let s = layout.scale;
    frame.fill_text(Text {
        content: format!("led1 · D{led_pin} · {}", if led_on { "ON" } else { "off" }),
        position: Point::new(layout.led.x, layout.led.y + 58.0 * s),
        color: if led_on { LED_ON } else { TEXT_MUTED },
        size: iced::Pixels(11.0 * s.min(1.2)),
        font: iced::Font::MONOSPACE,
        horizontal_alignment: iced::alignment::Horizontal::Center,
        vertical_alignment: iced::alignment::Vertical::Center,
        ..Text::default()
    });
    frame.fill_text(Text {
        content: format!("btn_start · D{button_pin}–GND"),
        position: Point::new(layout.button.x, layout.button.y + 58.0 * s),
        color: TEXT_MAIN,
        size: iced::Pixels(11.0 * s.min(1.2)),
        font: iced::Font::MONOSPACE,
        horizontal_alignment: iced::alignment::Horizontal::Center,
        vertical_alignment: iced::alignment::Vertical::Center,
        ..Text::default()
    });
    frame.fill_text(Text {
        content: "integrated 220 ohm".into(),
        position: Point::new(layout.led.x, layout.led.y + 72.0 * s),
        color: ACCENT_ORANGE,
        size: iced::Pixels(9.0 * s.min(1.2)),
        font: iced::Font::MONOSPACE,
        horizontal_alignment: iced::alignment::Horizontal::Center,
        vertical_alignment: iced::alignment::Vertical::Center,
        ..Text::default()
    });
}
