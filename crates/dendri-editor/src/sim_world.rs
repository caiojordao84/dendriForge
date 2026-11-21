//! Phase B1 Simulation world: scene from project, SVG bodies, pick/drag/snap.
//!
//! Input: LMB select/drag (grid snap), MMB pan, scroll zoom, R rotate,
//! RMB context menus (via messages). Canvas stays full-bleed under overlays.

use std::f32::consts::PI;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use dendri_canvas::{
    Camera, MvpProfile, Scene, ScenePart, TerminalRole, WorldPoint, ZOOM_MAX, ZOOM_MIN,
};
use iced::{
    mouse,
    widget::canvas::{self, Frame, Geometry, Path, Stroke, Text},
    Color, Point, Rectangle, Renderer, Size, Theme, Vector,
};

use crate::theme as ui;
use crate::Message;

/// Drag / pan state living in the iced canvas Program state.
#[derive(Debug, Default, Clone)]
pub struct WorldCanvasState {
    pan_last: Option<Point>,
    /// LMB drag in progress.
    drag: Option<DragState>,
    last_click: Option<(Instant, Point)>,
}

#[derive(Debug, Clone)]
struct DragState {
    id: String,
    /// World-space offset from part origin to grab point.
    grab_dx: f64,
    grab_dy: f64,
    started: bool,
}

/// SVG handles for MVP parts (loaded once at app boot).
#[derive(Debug, Clone)]
pub struct SceneSvgHandles {
    pub uno: iced::widget::svg::Handle,
    pub led: iced::widget::svg::Handle,
    pub button: iced::widget::svg::Handle,
}

impl SceneSvgHandles {
    pub fn load(repo_root: impl Into<PathBuf>) -> Self {
        let root = repo_root.into();
        let uno = resolve(&root, dendri_canvas::ASSET_BOARD, dendri_canvas::CORE_BOARD);
        let led = resolve(&root, dendri_canvas::ASSET_LED, dendri_canvas::CORE_LED);
        let button = resolve(&root, dendri_canvas::ASSET_BUTTON, dendri_canvas::CORE_BUTTON);
        Self {
            uno: iced::widget::svg::Handle::from_path(uno),
            led: iced::widget::svg::Handle::from_path(led),
            button: iced::widget::svg::Handle::from_path(button),
        }
    }

    fn for_profile(&self, profile: MvpProfile) -> &iced::widget::svg::Handle {
        match profile {
            MvpProfile::UnoR3 => &self.uno,
            MvpProfile::Led => &self.led,
            MvpProfile::Pushbutton => &self.button,
        }
    }
}

fn resolve(root: &std::path::Path, preferred: &str, fallback: &str) -> PathBuf {
    let a = root.join(preferred);
    if a.exists() {
        a
    } else {
        root.join(fallback)
    }
}

/// Full-bleed Simulation viewport (layer under chrome).
pub struct WorldCanvas<'a> {
    pub camera: Camera,
    pub scene: &'a Scene,
    pub selection: Option<&'a str>,
    pub svgs: &'a SceneSvgHandles,
    pub led_on: bool,
}

impl canvas::Program<Message> for WorldCanvas<'_> {
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
                let factor = if steps > 0.0 {
                    1.1_f32.powf(steps)
                } else {
                    (1.0 / 1.1_f32).powf(-steps)
                };
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
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(pos) = cursor.position_in(bounds) else {
                    return (canvas::event::Status::Ignored, None);
                };
                // Double-click → open properties hook.
                let now = Instant::now();
                let is_double = state
                    .last_click
                    .as_ref()
                    .map(|(t, p)| {
                        now.duration_since(*t) < Duration::from_millis(350)
                            && (p.x - pos.x).abs() < 6.0
                            && (p.y - pos.y).abs() < 6.0
                    })
                    .unwrap_or(false);
                state.last_click = Some((now, pos));

                let world = self.camera.view_to_world(pos.x as f64, pos.y as f64);
                if let Some(part) = self.scene.pick(world) {
                    if is_double {
                        return (
                            canvas::event::Status::Captured,
                            Some(Message::OpenProperties {
                                id: part.id.clone(),
                            }),
                        );
                    }
                    state.drag = Some(DragState {
                        id: part.id.clone(),
                        grab_dx: world.x - part.pose.x,
                        grab_dy: world.y - part.pose.y,
                        started: false,
                    });
                    (
                        canvas::event::Status::Captured,
                        Some(Message::SelectPart {
                            id: Some(part.id.clone()),
                        }),
                    )
                } else {
                    state.drag = None;
                    (
                        canvas::event::Status::Captured,
                        Some(Message::SelectPart { id: None }),
                    )
                }
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if let Some(drag) = state.drag.take() {
                    if drag.started {
                        return (
                            canvas::event::Status::Captured,
                            Some(Message::DragPartEnd { id: drag.id }),
                        );
                    }
                }
                (canvas::event::Status::Captured, None)
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                let Some(pos) = cursor.position_in(bounds) else {
                    return (canvas::event::Status::Ignored, None);
                };
                let world = self.camera.view_to_world(pos.x as f64, pos.y as f64);
                if let Some(part) = self.scene.pick(world) {
                    (
                        canvas::event::Status::Captured,
                        Some(Message::ShowObjectContextMenu {
                            id: part.id.clone(),
                            screen_x: bounds.x + pos.x,
                            screen_y: bounds.y + pos.y,
                        }),
                    )
                } else {
                    (
                        canvas::event::Status::Captured,
                        Some(Message::ShowEmptyContextMenu {
                            screen_x: bounds.x + pos.x,
                            screen_y: bounds.y + pos.y,
                        }),
                    )
                }
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let Some(last) = state.pan_last {
                    let Some(pos) = cursor.position_in(bounds) else {
                        return (canvas::event::Status::Ignored, None);
                    };
                    let dx = pos.x - last.x;
                    let dy = pos.y - last.y;
                    state.pan_last = Some(pos);
                    if dx.abs() < f32::EPSILON && dy.abs() < f32::EPSILON {
                        return (canvas::event::Status::Captured, None);
                    }
                    return (
                        canvas::event::Status::Captured,
                        Some(Message::CameraPanBy {
                            dx_px: dx,
                            dy_px: dy,
                        }),
                    );
                }
                if let Some(drag) = state.drag.as_mut() {
                    let Some(pos) = cursor.position_in(bounds) else {
                        return (canvas::event::Status::Ignored, None);
                    };
                    let world = self.camera.view_to_world(pos.x as f64, pos.y as f64);
                    let raw_x = world.x - drag.grab_dx;
                    let raw_y = world.y - drag.grab_dy;
                    let (sx, sy) = Scene::snap_origin(raw_x, raw_y);
                    let first = !drag.started;
                    drag.started = true;
                    return (
                        canvas::event::Status::Captured,
                        Some(Message::DragPartTo {
                            id: drag.id.clone(),
                            x: sx,
                            y: sy,
                            begin_undo: first,
                        }),
                    );
                }
                (canvas::event::Status::Ignored, None)
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
        } else if state.drag.is_some() {
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

        draw_line_grid(
            &mut frame,
            &self.camera,
            bounds.size().width,
            bounds.size().height,
        );

        for part in &self.scene.parts {
            let selected = self.selection == Some(part.id.as_str());
            draw_part(&mut frame, &self.camera, part, self.svgs, selected, self.led_on);
        }

        draw_overlay(&mut frame, &self.camera, bounds.size().width);

        vec![frame.into_geometry()]
    }
}

fn draw_part(
    frame: &mut Frame,
    camera: &Camera,
    part: &ScenePart,
    svgs: &SceneSvgHandles,
    selected: bool,
    led_on: bool,
) {
    let scale = camera.scale() as f32;
    let (vx, vy) = camera.world_to_view(WorldPoint::new(part.pose.x, part.pose.y));
    let w = (part.size.w * camera.scale()) as f32;
    let h = (part.size.h * camera.scale()) as f32;
    let cx = vx as f32 + w * 0.5;
    let cy = vy as f32 + h * 0.5;

    // Camada A: SVG body with bounds ∝ zoom (anti-pixelization).
    let handle = svgs.for_profile(part.profile);

    frame.with_save(|frame| {
        frame.translate(Vector::new(cx, cy));
        let sx = if part.pose.mirror_x { -1.0 } else { 1.0 };
        let sy = if part.pose.mirror_y { -1.0 } else { 1.0 };
        if sx != 1.0 || sy != 1.0 {
            frame.scale_nonuniform(Vector::new(sx, sy));
        }
        if part.pose.rotation % 360 != 0 {
            let rad = (part.pose.rotation as f32) * PI / 180.0;
            frame.rotate(rad);
        }
        // Draw in local space centered; bounds size = world size × scale (∝ zoom).
        frame.draw_svg(
            Rectangle::new(Point::new(-w * 0.5, -h * 0.5), Size::new(w, h)),
            handle,
        );
    });

    // Phase C will animate LED body; B1 uses a simple on-glow overlay.
    if part.profile == MvpProfile::Led && led_on {
        frame.fill(
            &Path::circle(Point::new(cx, cy), (0.7 * scale).max(6.0)),
            Color::from_rgba(0.94, 0.27, 0.27, 0.55),
        );
    }

    if selected {
        let aabb = part.world_aabb();
        let (x0, y0) = camera.world_to_view(aabb.min);
        let (x1, y1) = camera.world_to_view(aabb.max);
        frame.stroke_rectangle(
            Point::new(x0 as f32, y0 as f32),
            Size::new((x1 - x0) as f32, (y1 - y0) as f32),
            Stroke::default().with_color(ui::SKY).with_width(2.0),
        );
    }

    for t in &part.terminals {
        let world = part.terminal_world(t);
        draw_terminal(frame, camera, world, &t.label, t.role);
    }

    // Part id label under body.
    let aabb = part.world_aabb();
    let (lx, ly) = camera.world_to_view(WorldPoint::new(
        (aabb.min.x + aabb.max.x) * 0.5,
        aabb.max.y + 0.35,
    ));
    fill_label(
        frame,
        Point::new(lx as f32, ly as f32),
        &part.id,
        ui::MUTED,
        (0.28 * scale).clamp(9.0, 14.0),
    );
}

fn draw_terminal(
    frame: &mut Frame,
    camera: &Camera,
    pos: WorldPoint,
    label: &str,
    role: TerminalRole,
) {
    let p = view_pt(camera, pos);
    let r = (0.12 * camera.scale()).clamp(3.0, 8.0) as f32;
    let fill = pin_color(role);
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

fn pin_color(role: TerminalRole) -> Color {
    match role {
        TerminalRole::Digital => ui::pin_fill(ui::PinRole::Digital),
        TerminalRole::Analog => ui::pin_fill(ui::PinRole::Analog),
        TerminalRole::Power => ui::pin_fill(ui::PinRole::Power),
        TerminalRole::Gnd => ui::pin_fill(ui::PinRole::Gnd),
        TerminalRole::Reset => ui::pin_fill(ui::PinRole::Reset),
        TerminalRole::Nc => ui::pin_fill(ui::PinRole::Nc),
    }
}

fn view_pt(camera: &Camera, world: WorldPoint) -> Point {
    let (x, y) = camera.world_to_view(world);
    Point::new(x as f32, y as f32)
}

fn draw_line_grid(frame: &mut Frame, camera: &Camera, width_px: f32, height_px: f32) {
    let vis = camera
        .visible_world(width_px as f64, height_px as f64)
        .expand(1.0);
    let scale = camera.scale();

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
        Size::new(thickness, height_px),
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
    frame.fill_rectangle(Point::new(0.0, y), Size::new(width_px, thickness), color);
}

fn draw_overlay(frame: &mut Frame, camera: &Camera, width_px: f32) {
    let zoom_pct = (camera.zoom * 100.0).round();
    let _ = camera.zoom >= ZOOM_MIN - 1e-9 && camera.zoom <= ZOOM_MAX + 1e-9;
    fill_label(
        frame,
        Point::new(12.0, 14.0),
        &format!(
            "WORLD · B1 scene · snap · R=rotate · zoom {zoom_pct:.0}% · scroll=zoom · middle=pan"
        ),
        ui::MUTED,
        12.0,
    );
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
