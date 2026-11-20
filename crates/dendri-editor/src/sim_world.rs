//! Phase A Simulation world viewport: infinite line grid + camera + Uno/LED.
//!
//! Input (D7): scroll wheel = zoom, middle mouse = pan. No world scrollbars (D1).

use dendri_canvas::{Camera, PhaseAScene, WorldPoint, ZOOM_MAX, ZOOM_MIN};
use iced::{
    mouse,
    widget::canvas::{self, path::Builder, Frame, Geometry, Path, Stroke, Text},
    Color, Point, Rectangle, Renderer, Theme,
};

use crate::theme as ui;
use crate::Message;

/// Drag state for middle-button pan (lives in the iced canvas Program state).
#[derive(Debug, Default, Clone)]
pub struct WorldCanvasState {
    pan_last: Option<Point>,
}

/// Full-bleed Simulation viewport (layer under chrome).
pub struct WorldCanvas {
    pub camera: Camera,
    pub scene: PhaseAScene,
    pub led_on: bool,
}

impl canvas::Program<Message> for WorldCanvas {
    type State = WorldCanvasState;

    fn update(
        &self,
        state: &mut WorldCanvasState,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        match event {
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let Some(pos) = cursor.position_in(bounds) else {
                    return (canvas::event::Status::Ignored, None);
                };
                let steps = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
                };
                if steps.abs() < f32::EPSILON {
                    return (canvas::event::Status::Captured, None);
                }
                // Wheel up (positive) → zoom in.
                let factor = if steps > 0.0 { 1.1_f32.powf(steps) } else { (1.0 / 1.1_f32).powf(-steps) };
                (
                    canvas::event::Status::Captured,
                    Some(Message::CameraZoomAt {
                        factor,
                        anchor_x: pos.x,
                        anchor_y: pos.y,
                    }),
                )
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle)) => {
                if let Some(pos) = cursor.position_in(bounds) {
                    state.pan_last = Some(pos);
                    return (canvas::event::Status::Captured, None);
                }
                (canvas::event::Status::Ignored, None)
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Middle)) => {
                state.pan_last = None;
                (canvas::event::Status::Captured, None)
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let Some(last) = state.pan_last else {
                    return (canvas::event::Status::Ignored, None);
                };
                let Some(pos) = cursor.position_in(bounds) else {
                    return (canvas::event::Status::Ignored, None);
                };
                let dx = pos.x - last.x;
                let dy = pos.y - last.y;
                state.pan_last = Some(pos);
                if dx.abs() < f32::EPSILON && dy.abs() < f32::EPSILON {
                    return (canvas::event::Status::Captured, None);
                }
                (
                    canvas::event::Status::Captured,
                    Some(Message::CameraPanBy { dx_px: dx, dy_px: dy }),
                )
            }
            canvas::Event::Mouse(mouse::Event::CursorLeft) => {
                state.pan_last = None;
                (canvas::event::Status::Ignored, None)
            }
            _ => (canvas::event::Status::Ignored, None),
        }
    }

    fn mouse_interaction(
        &self,
        state: &WorldCanvasState,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.pan_last.is_some() {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(bounds) {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::default()
        }
    }

    fn draw(
        &self,
        _state: &WorldCanvasState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), ui::SURFACE);

        draw_line_grid(&mut frame, &self.camera, bounds.size().width, bounds.size().height);
        draw_uno(&mut frame, &self.camera, &self.scene);
        draw_led(&mut frame, &self.camera, &self.scene, self.led_on);
        draw_overlay(&mut frame, &self.camera, bounds.size().width);

        vec![frame.into_geometry()]
    }
}

fn view_pt(camera: &Camera, world: WorldPoint) -> Point {
    let (x, y) = camera.world_to_view(world);
    Point::new(x as f32, y as f32)
}

fn draw_line_grid(frame: &mut Frame, camera: &Camera, width_px: f32, height_px: f32) {
    let vis = camera.visible_world(width_px as f64, height_px as f64).expand(1.0);
    let scale = camera.scale();

    // Draw as filled 1–2 px strips in *view* space. Tiny-skia hairline strokes on
    // near-black backgrounds anti-alias into invisibility; fills stay readable.
    let half_color = Color::from_rgb(0.18, 0.22, 0.32);
    let full_color = Color::from_rgb(0.35, 0.42, 0.52);

    if scale >= 12.0 {
        let x0 = (vis.min.x * 2.0).floor() / 2.0;
        let mut x = x0;
        while x <= vis.max.x + 1e-9 {
            if (x * 2.0).round() as i64 % 2 != 0 {
                fill_view_vline(frame, camera, x, height_px, 1.0, half_color);
            }
            x += 0.5;
        }
        let y0 = (vis.min.y * 2.0).floor() / 2.0;
        let mut y = y0;
        while y <= vis.max.y + 1e-9 {
            if (y * 2.0).round() as i64 % 2 != 0 {
                fill_view_hline(frame, camera, y, width_px, 1.0, half_color);
            }
            y += 0.5;
        }
    }

    let x0 = vis.min.x.floor();
    let mut x = x0;
    while x <= vis.max.x + 1e-9 {
        fill_view_vline(frame, camera, x, height_px, 2.0, full_color);
        x += 1.0;
    }
    let y0 = vis.min.y.floor();
    let mut y = y0;
    while y <= vis.max.y + 1e-9 {
        fill_view_hline(frame, camera, y, width_px, 2.0, full_color);
        y += 1.0;
    }
}

fn fill_view_vline(
    frame: &mut Frame,
    camera: &Camera,
    world_x: f64,
    height_px: f32,
    thickness: f32,
    color: Color,
) {
    let (vx, _) = camera.world_to_view(WorldPoint::new(world_x, 0.0));
    let x = vx as f32 - thickness * 0.5;
    frame.fill_rectangle(
        Point::new(x, 0.0),
        iced::Size::new(thickness, height_px),
        color,
    );
}

fn fill_view_hline(
    frame: &mut Frame,
    camera: &Camera,
    world_y: f64,
    width_px: f32,
    thickness: f32,
    color: Color,
) {
    let (_, vy) = camera.world_to_view(WorldPoint::new(0.0, world_y));
    let y = vy as f32 - thickness * 0.5;
    frame.fill_rectangle(
        Point::new(0.0, y),
        iced::Size::new(width_px, thickness),
        color,
    );
}

fn draw_uno(frame: &mut Frame, camera: &Camera, scene: &PhaseAScene) {
    let uno = &scene.uno;
    let scale = camera.scale() as f32;
    let origin = view_pt(camera, uno.origin);
    let w = (uno.size.x * camera.scale()) as f32;
    let h = (uno.size.y * camera.scale()) as f32;

    let board = Path::new(|b: &mut Builder| {
        let r = (0.15 * camera.scale()) as f32;
        rounded_rect(b, origin, w, h, r);
    });
    frame.fill(&board, Color::from_rgb(0.004, 0.494, 0.482)); // #017e7b
    frame.stroke(
        &board,
        Stroke::default()
            .with_color(Color::from_rgb(0.059, 0.463, 0.431))
            .with_width(1.5),
    );

    // Inner chip well
    let inset = 0.4 * scale;
    frame.fill_rectangle(
        Point::new(origin.x + inset, origin.y + 1.2 * scale),
        iced::Size::new(w - 2.0 * inset, h - 2.0 * scale),
        Color::from_rgba(0.043, 0.075, 0.149, 0.35),
    );

    fill_label(
        frame,
        Point::new(origin.x + 0.5 * scale, origin.y + 0.55 * scale),
        "UNO R3",
        ui::HEADING,
        (0.45 * scale).clamp(10.0, 22.0),
    );
    fill_label(
        frame,
        Point::new(origin.x + 0.5 * scale, origin.y + 1.0 * scale),
        "ATmega328P",
        Color::from_rgb(0.655, 0.953, 0.816),
        (0.28 * scale).clamp(9.0, 14.0),
    );

    draw_terminal(frame, camera, uno.d13.pos, uno.d13.label, false);
    draw_terminal(frame, camera, uno.gnd.pos, uno.gnd.label, true);
}

fn draw_led(frame: &mut Frame, camera: &Camera, scene: &PhaseAScene, led_on: bool) {
    let led = &scene.led;
    let scale = camera.scale() as f32;
    let c = view_pt(camera, led.center);
    let r_outer = (led.radius_u * camera.scale()) as f32;
    let r_body = (0.85 * camera.scale()) as f32;

    frame.fill(
        &Path::circle(c, r_outer),
        Color::from_rgb(0.118, 0.161, 0.231),
    );
    frame.stroke(
        &Path::circle(c, r_outer),
        Stroke::default().with_color(ui::BORDER).with_width(1.0),
    );

    let fill = if led_on {
        Color::from_rgb(0.937, 0.267, 0.267)
    } else {
        Color::from_rgba(0.498, 0.114, 0.114, 0.45)
    };
    frame.fill(&Path::circle(c, r_body), fill);

    fill_label(
        frame,
        Point::new(c.x, c.y + r_outer + 0.35 * scale),
        "led1 · A/K",
        ui::MUTED,
        (0.28 * scale).clamp(9.0, 14.0),
    );

    draw_terminal(frame, camera, led.anode.pos, led.anode.label, false);
    draw_terminal(frame, camera, led.cathode.pos, led.cathode.label, true);
}

fn draw_terminal(frame: &mut Frame, camera: &Camera, pos: WorldPoint, label: &str, ground: bool) {
    let p = view_pt(camera, pos);
    let r = (0.12 * camera.scale()).clamp(3.0, 8.0) as f32;
    let fill = if ground { ui::GROUND } else { ui::DIGITAL };
    frame.fill(&Path::circle(p, r), fill);
    frame.stroke(
        &Path::circle(p, r),
        Stroke::default().with_color(ui::LOWEST).with_width(1.0),
    );
    let scale = camera.scale() as f32;
    fill_label(
        frame,
        Point::new(p.x - 0.28 * scale, p.y - 0.08 * scale),
        label,
        ui::MUTED,
        (0.26 * scale).clamp(9.0, 13.0),
    );
}

fn draw_overlay(frame: &mut Frame, camera: &Camera, width_px: f32) {
    let zoom_pct = (camera.zoom * 100.0).round();
    let clamped = camera.zoom >= ZOOM_MIN - 1e-9 && camera.zoom <= ZOOM_MAX + 1e-9;
    let _ = clamped;
    fill_label(
        frame,
        Point::new(12.0, 14.0),
        &format!(
            "WORLD · line-grid · 1u=4mm=48px@100% · zoom {zoom_pct:.0}% · scroll=zoom · middle=pan"
        ),
        ui::MUTED,
        12.0,
    );
    // Right-side zoom readout (mirrors toolbar; useful when left dock hidden).
    let zoom_label = format!("{zoom_pct:.0}%");
    fill_label(
        frame,
        Point::new((width_px - 56.0).max(12.0), 14.0),
        &zoom_label,
        ui::SKY,
        13.0,
    );
}

fn fill_label(frame: &mut Frame, position: Point, content: &str, color: Color, size: f32) {
    frame.fill_text(Text {
        content: content.to_owned(),
        position,
        color,
        size: size.into(),
        font: iced::Font::MONOSPACE,
        ..Default::default()
    });
}

fn rounded_rect(b: &mut Builder, origin: Point, w: f32, h: f32, r: f32) {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let x = origin.x;
    let y = origin.y;
    b.move_to(Point::new(x + r, y));
    b.line_to(Point::new(x + w - r, y));
    b.quadratic_curve_to(Point::new(x + w, y), Point::new(x + w, y + r));
    b.line_to(Point::new(x + w, y + h - r));
    b.quadratic_curve_to(Point::new(x + w, y + h), Point::new(x + w - r, y + h));
    b.line_to(Point::new(x + r, y + h));
    b.quadratic_curve_to(Point::new(x, y + h), Point::new(x, y + h - r));
    b.line_to(Point::new(x, y + r));
    b.quadratic_curve_to(Point::new(x, y), Point::new(x + r, y));
    b.close();
}
