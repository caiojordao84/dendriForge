//! Phase B2/C1 Simulation world: parts + wires, trilayer cache, pin snap,
//! dynamic LED/button Paths (D-C3 / D-C4).
//!
//! Input: LMB select/drag parts, LMB pin→pin wire, click wire select / waypoint,
//! MMB pan, scroll zoom, R rotate. While Running: LMB button press / Shift+LMB latch.
//! Canvas stays full-bleed under overlays.
//!
//! Caches (plan §6 / C1):
//! - **background** — grid (invalidate on pan/zoom)
//! - **static** — SVG bodies + committed wires (invalidate on layout/wire commit)
//! - **dynamic** — LED glow/burned, button press/latch, rubber-band, pin glow,
//!   status (invalidate on present_gen / input / CursorMoved — not mouse-only)

use std::cell::Cell;
use std::f32::consts::PI;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use dendri_ast::Wire;
use dendri_canvas::{
    parse_svg_str, pick_wire, prepare_hook, route_wires, rubber_band, waypoint_from_click, Camera,
    MvpProfile, PreparedHook, RoutedWire, RuntimeAvailability, RuntimeSnapshot, Scene, ScenePart,
    SimRunState, TerminalRef, TerminalRole, WireColorKind, WorldPoint, PIN_SNAP_SCREEN_PX,
    WIRE_BASE_WIDTH_PX, WIRE_COLOR_WIDTH_PX, WIRE_FILLET_SCREEN_PX, WIRE_HIT_SCREEN_PX, ZOOM_MAX,
    ZOOM_MIN,
};
use iced::{
    mouse,
    widget::canvas::{self, Frame, Geometry, Path, Stroke, Text},
    Color, Point, Rectangle, Renderer, Size, Theme, Vector,
};

use crate::canvas_path::{hook_circle_path, hook_path_svg_units};
use crate::theme as ui;
use crate::Message;

/// Canvas selection: a part instance or a wiring index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanvasSelection {
    None,
    Part(String),
    Wire(usize),
}

impl Default for CanvasSelection {
    fn default() -> Self {
        Self::None
    }
}

impl CanvasSelection {
    pub fn part_id(&self) -> Option<&str> {
        match self {
            Self::Part(id) => Some(id.as_str()),
            _ => None,
        }
    }

    pub fn wire_index(&self) -> Option<usize> {
        match self {
            Self::Wire(i) => Some(*i),
            _ => None,
        }
    }
}

/// Drag / pan / wire state living in the iced canvas Program state.
#[derive(Debug, Default)]
pub struct WorldCanvasState {
    pan_last: Option<Point>,
    drag: Option<DragState>,
    wire_drag: Option<WireDragState>,
    last_click: Option<(Instant, Point)>,
    bg_cache: canvas::Cache,
    static_cache: canvas::Cache,
    dynamic_cache: canvas::Cache,
    cached_zoom: Cell<f64>,
    cached_pan_x: Cell<f64>,
    cached_pan_y: Cell<f64>,
    cached_static_gen: Cell<u64>,
    /// Last consumed runtime present_gen (invalidates **dynamic** layer only).
    cached_present_gen: Cell<u64>,
}

#[derive(Debug, Clone)]
struct DragState {
    id: String,
    grab_dx: f64,
    grab_dy: f64,
    started: bool,
}

#[derive(Debug, Clone)]
struct WireDragState {
    from: TerminalRef,
}

/// SVG handles + prepared hook geometry for MVP parts (loaded once at app boot).
#[derive(Debug, Clone)]
pub struct SceneSvgHandles {
    pub uno: iced::widget::svg::Handle,
    pub led: iced::widget::svg::Handle,
    pub button: iced::widget::svg::Handle,
    /// `led-body` hook — prepared once; reused every dynamic frame.
    pub led_body: Option<PreparedHook>,
    /// `actuator` hook — prepared once; reused every dynamic frame.
    pub button_actuator: Option<PreparedHook>,
}

impl SceneSvgHandles {
    pub fn load(repo_root: impl Into<PathBuf>) -> Self {
        let root = repo_root.into();
        let uno = resolve(&root, dendri_canvas::ASSET_BOARD, dendri_canvas::CORE_BOARD);
        let led = resolve(&root, dendri_canvas::ASSET_LED, dendri_canvas::CORE_LED);
        let button = resolve(&root, dendri_canvas::ASSET_BUTTON, dendri_canvas::CORE_BUTTON);
        let led_body = std::fs::read_to_string(&led)
            .ok()
            .and_then(|s| parse_svg_str(&s).ok())
            .and_then(|t| prepare_hook(&t, "led-body"));
        let button_actuator = std::fs::read_to_string(&button)
            .ok()
            .and_then(|s| parse_svg_str(&s).ok())
            .and_then(|t| prepare_hook(&t, "actuator"));
        Self {
            uno: iced::widget::svg::Handle::from_path(uno),
            led: iced::widget::svg::Handle::from_path(led),
            button: iced::widget::svg::Handle::from_path(button),
            led_body,
            button_actuator,
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
    pub wiring: &'a [Wire],
    pub selection: &'a CanvasSelection,
    pub svgs: &'a SceneSvgHandles,
    /// C0/C1 runtime snapshot (per-instance bindings + LED current/burned).
    pub snapshot: &'a RuntimeSnapshot,
    /// D-C1: when false, block layout/wire mutations (pan/zoom/select still ok).
    pub edits_allowed: bool,
    /// Shift modifier held (Running → button latch; else → multi-select stub).
    pub shift_held: bool,
    /// Bumped whenever static geometry must redraw (layout / wire commit).
    pub static_gen: u64,
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

                // 1) Terminal hit → start wire (or complete if already dragging).
                if let Some(term) = self.scene.nearest_terminal_screen(
                    &self.camera,
                    pos.x as f64,
                    pos.y as f64,
                    PIN_SNAP_SCREEN_PX,
                ) {
                    if !self.edits_allowed {
                        // Still allow selection/inspection while Running.
                        state.wire_drag = None;
                        state.drag = None;
                        return (canvas::event::Status::Captured, None);
                    }
                    if let Some(drag) = state.wire_drag.take() {
                        if drag.from.endpoint != term.endpoint {
                            state.dynamic_cache.clear();
                            return (
                                canvas::event::Status::Captured,
                                Some(Message::CreateWire {
                                    from: drag.from.endpoint,
                                    to: term.endpoint,
                                }),
                            );
                        }
                        state.dynamic_cache.clear();
                        return (canvas::event::Status::Captured, None);
                    }
                    state.wire_drag = Some(WireDragState { from: term });
                    state.drag = None;
                    state.dynamic_cache.clear();
                    return (canvas::event::Status::Captured, None);
                }

                // 2) Wire hit → select or insert waypoint when already selected.
                let routed = route_wires(self.scene, self.wiring);
                if let Some((idx, closest, seg)) = pick_wire(
                    &self.camera,
                    &routed,
                    pos.x as f64,
                    pos.y as f64,
                    WIRE_HIT_SCREEN_PX,
                ) {
                    if matches!(self.selection, CanvasSelection::Wire(i) if *i == idx) {
                        if !self.edits_allowed {
                            return (canvas::event::Status::Captured, None);
                        }
                        let wp = waypoint_from_click(closest);
                        // after_segment maps to waypoint insert index
                        // (segment 0 = before first waypoint).
                        let after = seg.saturating_sub(0);
                        // Insert after the first point of the segment relative to
                        // auto-route corners: use segment index among polyline
                        // minus endpoints — clamp into waypoints list.
                        let insert_after = {
                            let n_wp = self.wiring.get(idx).map(|w| w.waypoints.len()).unwrap_or(0);
                            // Prefer inserting after the waypoint that precedes this
                            // segment; for auto routes (0 waypoints) insert at 0.
                            if n_wp == 0 {
                                0
                            } else {
                                after.min(n_wp)
                            }
                        };
                        return (
                            canvas::event::Status::Captured,
                            Some(Message::InsertWireWaypoint {
                                index: idx,
                                at: wp,
                                after_segment: insert_after,
                            }),
                        );
                    }
                    state.drag = None;
                    state.wire_drag = None;
                    return (
                        canvas::event::Status::Captured,
                        Some(Message::SelectWire { index: Some(idx) }),
                    );
                }

                // 3) Part hit → Running button actuation (D-C4) or select / drag.
                if let Some(part) = self.scene.pick(world) {
                    if is_double && self.edits_allowed {
                        return (
                            canvas::event::Status::Captured,
                            Some(Message::OpenProperties {
                                id: part.id.clone(),
                            }),
                        );
                    }
                    state.wire_drag = None;

                    // While Running: actionable button body → press / Shift latch.
                    if self.snapshot.run_state == SimRunState::Running
                        && part.profile == MvpProfile::Pushbutton
                    {
                        state.drag = None;
                        state.dynamic_cache.clear();
                        if self.shift_held {
                            return (
                                canvas::event::Status::Captured,
                                Some(Message::CanvasButtonLatchToggle {
                                    id: part.id.clone(),
                                }),
                            );
                        }
                        return (
                            canvas::event::Status::Captured,
                            Some(Message::CanvasButtonPress {
                                id: part.id.clone(),
                            }),
                        );
                    }

                    // Outside Running: Shift+LMB is multi-select (stub: select part,
                    // never treat as latch).
                    if self.edits_allowed {
                        state.drag = Some(DragState {
                            id: part.id.clone(),
                            grab_dx: world.x - part.pose.x,
                            grab_dy: world.y - part.pose.y,
                            started: false,
                        });
                    } else {
                        state.drag = None;
                    }
                    // Shift outside Running: multi-select stub (select only; never latch).
                    let _shift_multiselect_stub = self.shift_held && self.edits_allowed;
                    let _ = _shift_multiselect_stub;
                    (
                        canvas::event::Status::Captured,
                        Some(Message::SelectPart {
                            id: Some(part.id.clone()),
                        }),
                    )
                } else {
                    state.drag = None;
                    state.wire_drag = None;
                    state.dynamic_cache.clear();
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
                // Running: LMB up releases momentary button hold (latches stay).
                if self.snapshot.run_state == SimRunState::Running {
                    state.dynamic_cache.clear();
                    return (
                        canvas::event::Status::Captured,
                        Some(Message::CanvasButtonRelease),
                    );
                }
                // Wire drag completes on press of target terminal; release alone
                // cancels if cursor is not on a pin (handled on next press).
                if state.wire_drag.is_some() {
                    if let Some(pos) = cursor.position_in(bounds) {
                        if self
                            .scene
                            .nearest_terminal_screen(
                                &self.camera,
                                pos.x as f64,
                                pos.y as f64,
                                PIN_SNAP_SCREEN_PX,
                            )
                            .is_none()
                        {
                            // Keep drag active until second press — allow MMB pan.
                        }
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
                    let routed = route_wires(self.scene, self.wiring);
                    if let Some((idx, _, _)) = pick_wire(
                        &self.camera,
                        &routed,
                        pos.x as f64,
                        pos.y as f64,
                        WIRE_HIT_SCREEN_PX,
                    ) {
                        (
                            canvas::event::Status::Captured,
                            Some(Message::ShowWireContextMenu {
                                index: idx,
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
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                state.dynamic_cache.clear();
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
                    if !self.edits_allowed {
                        state.drag = None;
                        return (canvas::event::Status::Captured, None);
                    }
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
                if state.wire_drag.is_some() {
                    return (canvas::event::Status::Captured, None);
                }
                // Hover near pins still needs a redraw (dynamic cache cleared).
                if cursor.position_in(bounds).is_some() {
                    return (canvas::event::Status::Captured, None);
                }
                (canvas::event::Status::Ignored, None)
            }
            canvas::Event::Mouse(mouse::Event::CursorLeft) => {
                state.pan_last = None;
                state.dynamic_cache.clear();
                // Release momentary on focus/cursor loss; latches independent (D-C4).
                if self.snapshot.run_state == SimRunState::Running {
                    return (
                        canvas::event::Status::Ignored,
                        Some(Message::CanvasButtonCancelMomentary),
                    );
                }
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
        } else if state.wire_drag.is_some() {
            mouse::Interaction::Crosshair
        } else if cursor.is_over(bounds) {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::default()
        }
    }

    fn draw(
        &self,
        state: &WorldCanvasState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        if (state.cached_zoom.get() - self.camera.zoom).abs() > 1e-12
            || (state.cached_pan_x.get() - self.camera.pan.x).abs() > 1e-12
            || (state.cached_pan_y.get() - self.camera.pan.y).abs() > 1e-12
        {
            state.bg_cache.clear();
            state.static_cache.clear();
            state.cached_zoom.set(self.camera.zoom);
            state.cached_pan_x.set(self.camera.pan.x);
            state.cached_pan_y.set(self.camera.pan.y);
        }
        if state.cached_static_gen.get() != self.static_gen {
            state.static_cache.clear();
            state.cached_static_gen.set(self.static_gen);
        }
        if state.cached_present_gen.get() != self.snapshot.present_gen {
            // C1: LED/button live on the dynamic layer — do not clear static.
            state.dynamic_cache.clear();
            state.cached_present_gen.set(self.snapshot.present_gen);
        }

        let size = bounds.size();
        let routed = route_wires(self.scene, self.wiring);

        let bg = state.bg_cache.draw(renderer, size, |frame| {
            frame.fill_rectangle(Point::ORIGIN, size, ui::SURFACE);
            draw_line_grid(
                frame,
                &self.camera,
                size.width,
                size.height,
            );
        });

        let static_geo = state.static_cache.draw(renderer, size, |frame| {
            for part in &self.scene.parts {
                let selected = self.selection.part_id() == Some(part.id.as_str());
                draw_part(frame, &self.camera, part, self.svgs, selected);
            }
            for w in &routed {
                let selected = self.selection.wire_index() == Some(w.index);
                draw_routed_wire(frame, &self.camera, w, selected);
            }
        });

        let dynamic = state.dynamic_cache.draw(renderer, size, |frame| {
            // Dynamic component overlays (LED current/burned, button press/latch).
            for part in &self.scene.parts {
                draw_dynamic_part(frame, &self.camera, part, self.svgs, self.snapshot);
            }

            draw_runtime_status(frame, self.snapshot, size);

            let cursor_pos = cursor.position_in(bounds);
            let snap = cursor_pos.and_then(|pos| {
                self.scene.nearest_terminal_screen(
                    &self.camera,
                    pos.x as f64,
                    pos.y as f64,
                    PIN_SNAP_SCREEN_PX,
                )
            });

            if let Some(term) = snap.as_ref() {
                draw_pin_glow(frame, &self.camera, term.world);
            }

            if self.edits_allowed {
                if let Some(drag) = state.wire_drag.as_ref() {
                    let end = snap
                        .as_ref()
                        .map(|t| t.world)
                        .or_else(|| {
                            cursor_pos.map(|pos| {
                                self.camera.view_to_world(pos.x as f64, pos.y as f64)
                            })
                        })
                        .unwrap_or(drag.from.world);
                    let pts = rubber_band(drag.from.world, end);
                    draw_polyline_dual(
                        frame,
                        &self.camera,
                        &pts,
                        ui::wire_color(ui::WireRole::Default),
                        false,
                    );
                    draw_pin_glow(frame, &self.camera, drag.from.world);
                }
            }

            draw_overlay(frame, &self.camera, size.width, state.wire_drag.is_some());
        });

        vec![bg, static_geo, dynamic]
    }
}

fn draw_routed_wire(frame: &mut Frame, camera: &Camera, wire: &RoutedWire, selected: bool) {
    if wire.points.len() < 2 {
        return;
    }
    let color = wire_draw_color(wire);
    draw_polyline_dual(frame, camera, &wire.points, color, selected);
}

fn wire_draw_color(wire: &RoutedWire) -> Color {
    match dendri_canvas::wire_color_kind(
        wire.signal.as_deref(),
        wire.from_role,
        wire.to_role,
        wire.sequence,
    ) {
        WireColorKind::Gnd => ui::wire_color(ui::WireRole::Gnd),
        WireColorKind::Vcc => ui::wire_color(ui::WireRole::Vcc),
        WireColorKind::Signal { sequence, from } => {
            let base = ui::wire_color(ui::WireRole::InheritPin(terminal_to_pin(from)));
            sequential_tint(base, sequence)
        }
    }
}

fn terminal_to_pin(role: TerminalRole) -> ui::PinRole {
    match role {
        TerminalRole::Digital => ui::PinRole::Digital,
        TerminalRole::Analog => ui::PinRole::Analog,
        TerminalRole::Power => ui::PinRole::Power,
        TerminalRole::Gnd => ui::PinRole::Gnd,
        TerminalRole::Reset => ui::PinRole::Reset,
        TerminalRole::Nc => ui::PinRole::Nc,
    }
}

fn sequential_tint(base: Color, sequence: usize) -> Color {
    // Cycle lightness slightly so parallel digital wires remain distinct.
    let f = ((sequence % 5) as f32) * 0.04;
    Color {
        r: (base.r * (1.0 - f) + f * 0.9).clamp(0.0, 1.0),
        g: (base.g * (1.0 - f * 0.5)).clamp(0.0, 1.0),
        b: (base.b * (1.0 + f * 0.2)).clamp(0.0, 1.0),
        a: base.a,
    }
}

fn draw_polyline_dual(
    frame: &mut Frame,
    camera: &Camera,
    points: &[WorldPoint],
    color: Color,
    selected: bool,
) {
    let path = filleted_path(camera, points, WIRE_FILLET_SCREEN_PX);
    let base = Color {
        r: 0.05,
        g: 0.07,
        b: 0.12,
        a: 0.55,
    };
    frame.stroke(
        &path,
        Stroke::default()
            .with_color(base)
            .with_width(if selected {
                WIRE_BASE_WIDTH_PX + 2.0
            } else {
                WIRE_BASE_WIDTH_PX
            }),
    );
    frame.stroke(
        &path,
        Stroke::default()
            .with_color(if selected { ui::SKY } else { color })
            .with_width(WIRE_COLOR_WIDTH_PX),
    );
}

fn filleted_path(camera: &Camera, points: &[WorldPoint], radius_px: f32) -> Path {
    let view: Vec<Point> = points
        .iter()
        .map(|p| {
            let (x, y) = camera.world_to_view(*p);
            Point::new(x as f32, y as f32)
        })
        .collect();
    if view.len() < 2 {
        return Path::new(|_| {});
    }
    Path::new(|b| {
        b.move_to(view[0]);
        if view.len() == 2 {
            b.line_to(view[1]);
            return;
        }
        for i in 1..view.len() - 1 {
            let prev = view[i - 1];
            let curr = view[i];
            let next = view[i + 1];
            let (p1, p2) = fillet_cut(prev, curr, next, radius_px);
            b.line_to(p1);
            b.arc_to(curr, p2, radius_px);
        }
        b.line_to(*view.last().unwrap());
    })
}

fn fillet_cut(prev: Point, curr: Point, next: Point, radius: f32) -> (Point, Point) {
    let d1x = curr.x - prev.x;
    let d1y = curr.y - prev.y;
    let d2x = next.x - curr.x;
    let d2y = next.y - curr.y;
    let len1 = (d1x * d1x + d1y * d1y).sqrt().max(1e-3);
    let len2 = (d2x * d2x + d2y * d2y).sqrt().max(1e-3);
    let r = radius.min(len1 * 0.45).min(len2 * 0.45);
    let p1 = Point::new(curr.x - d1x / len1 * r, curr.y - d1y / len1 * r);
    let p2 = Point::new(curr.x + d2x / len2 * r, curr.y + d2y / len2 * r);
    (p1, p2)
}

fn draw_pin_glow(frame: &mut Frame, camera: &Camera, world: WorldPoint) {
    let p = view_pt(camera, world);
    let r = (0.18 * camera.scale()).clamp(6.0, 14.0) as f32;
    frame.fill(
        &Path::circle(p, r + 4.0),
        Color::from_rgba(0.22, 0.75, 1.0, 0.22),
    );
    frame.stroke(
        &Path::circle(p, r),
        Stroke::default()
            .with_color(Color::from_rgba(0.45, 0.9, 1.0, 0.95))
            .with_width(2.0),
    );
}

fn draw_part(
    frame: &mut Frame,
    camera: &Camera,
    part: &ScenePart,
    svgs: &SceneSvgHandles,
    selected: bool,
) {
    let scale = camera.scale() as f32;
    let (vx, vy) = camera.world_to_view(WorldPoint::new(part.pose.x, part.pose.y));
    let w = (part.size.w * camera.scale()) as f32;
    let h = (part.size.h * camera.scale()) as f32;
    let cx = vx as f32 + w * 0.5;
    let cy = vy as f32 + h * 0.5;

    let handle = svgs.for_profile(part.profile);

    frame.with_save(|frame| {
        apply_part_pose(frame, part, cx, cy);
        frame.draw_svg(
            Rectangle::new(Point::new(-w * 0.5, -h * 0.5), Size::new(w, h)),
            handle,
        );
    });

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

fn apply_part_pose(frame: &mut Frame, part: &ScenePart, cx: f32, cy: f32) {
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
}

/// Map SVG user-unit hook geometry into the same local space as `draw_svg`.
fn with_hook_space(
    frame: &mut Frame,
    camera: &Camera,
    part: &ScenePart,
    view_w: f32,
    view_h: f32,
    f: impl FnOnce(&mut Frame),
) {
    let (vx, vy) = camera.world_to_view(WorldPoint::new(part.pose.x, part.pose.y));
    let w = (part.size.w * camera.scale()) as f32;
    let h = (part.size.h * camera.scale()) as f32;
    let cx = vx as f32 + w * 0.5;
    let cy = vy as f32 + h * 0.5;
    frame.with_save(|frame| {
        apply_part_pose(frame, part, cx, cy);
        // SVG draw maps viewBox → (-w/2,-h/2)-(w,h); scale SVG units the same way.
        let sx = if view_w > 0.0 { w / view_w } else { 1.0 };
        let sy = if view_h > 0.0 { h / view_h } else { 1.0 };
        frame.translate(Vector::new(-w * 0.5, -h * 0.5));
        frame.scale_nonuniform(Vector::new(sx, sy));
        f(frame);
    });
}

fn draw_dynamic_part(
    frame: &mut Frame,
    camera: &Camera,
    part: &ScenePart,
    svgs: &SceneSvgHandles,
    snapshot: &RuntimeSnapshot,
) {
    let Some(st) = snapshot.components.get(&part.id) else {
        return;
    };
    match part.profile {
        MvpProfile::Led => {
            let Some(hook) = svgs.led_body.as_ref() else {
                return;
            };
            let burned = st.led_burned == Some(true);
            let on = st.led_on == Some(true);
            if !burned && !on {
                return;
            }
            with_hook_space(frame, camera, part, hook.view_w, hook.view_h, |frame| {
                let path = hook_circle_path(hook).unwrap_or_else(|| hook_path_svg_units(hook));
                if burned {
                    // Dark centre + orange glow from led-body geometry (no new SVG tags).
                    frame.fill(&path, ui::DF_LED_OFF_BASE);
                    if let Some(c) = hook.circle {
                        frame.fill(
                            &Path::circle(Point::new(c.cx, c.cy), c.r * 1.35),
                            Color {
                                r: ui::DF_LED_GLOW_ORANGE.r,
                                g: ui::DF_LED_GLOW_ORANGE.g,
                                b: ui::DF_LED_GLOW_ORANGE.b,
                                a: 0.45,
                            },
                        );
                        frame.fill(&path, ui::DF_LED_OFF_BASE);
                    }
                } else {
                    // Digital indication: apply directly (no ramp / spring).
                    let alpha = brightness_from_current(st.led_current_a().unwrap_or(0.0));
                    frame.fill(
                        &path,
                        Color {
                            r: ui::DF_LED_GLOW_RED.r,
                            g: ui::DF_LED_GLOW_RED.g,
                            b: ui::DF_LED_GLOW_RED.b,
                            a: alpha,
                        },
                    );
                }
            });
        }
        MvpProfile::Pushbutton => {
            let pressed = st.button_pressed == Some(true);
            let latched = st.button_latched == Some(true);
            if !pressed && !latched {
                return;
            }
            let Some(hook) = svgs.button_actuator.as_ref() else {
                return;
            };
            with_hook_space(frame, camera, part, hook.view_w, hook.view_h, |frame| {
                let path = hook_circle_path(hook).unwrap_or_else(|| hook_path_svg_units(hook));
                // Pressed fill; unambiguous user-held ring when latched.
                frame.fill(
                    &path,
                    Color::from_rgba(0.12, 0.55, 0.35, if pressed { 0.55 } else { 0.25 }),
                );
                if latched {
                    if let Some(c) = hook.circle {
                        frame.stroke(
                            &Path::circle(Point::new(c.cx, c.cy), c.r * 1.15),
                            Stroke::default()
                                .with_color(Color::from_rgba(1.0, 0.85, 0.2, 0.95))
                                .with_width(3.0),
                        );
                    }
                }
            });
        }
        MvpProfile::UnoR3 => {}
    }
}

/// Map forward current to fill alpha. Digital on/off uses direct presentation;
/// continuous targets (PWM) would use lilt later — not in C1.
fn brightness_from_current(current_a: f64) -> f32 {
    if current_a <= 0.0 {
        return 0.0;
    }
    // Soft clip around typical ~20 mA rated current without hardcoding burn.
    let norm = (current_a / 0.02).clamp(0.0, 1.5);
    (0.35 + 0.55 * norm as f32).clamp(0.35, 0.95)
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

/// C0 proof that WorldCanvas consumes the runtime snapshot (tick / status).
fn draw_runtime_status(frame: &mut Frame, snapshot: &RuntimeSnapshot, size: Size) {
    let state = match snapshot.run_state {
        dendri_canvas::SimRunState::Idle => "Idle",
        dendri_canvas::SimRunState::Running => "Running",
        dendri_canvas::SimRunState::Paused => "Paused",
    };
    let avail = match &snapshot.availability {
        RuntimeAvailability::Available => "ok",
        RuntimeAvailability::Unavailable => "unavailable",
        RuntimeAvailability::Error(_) => "error",
    };
    let burned = if snapshot.any_led_burned() { "BURN" } else { "ok" };
    let line = format!(
        "SIM {state} · scan {} · led={} · {burned} · {avail} · gen={}",
        snapshot.cycle,
        if snapshot.any_led_on() { 1 } else { 0 },
        snapshot.present_gen
    );
    let color = match &snapshot.availability {
        RuntimeAvailability::Available => ui::EMERALD,
        RuntimeAvailability::Unavailable => ui::ORANGE,
        RuntimeAvailability::Error(_) => ui::ROSE,
    };
    fill_label(
        frame,
        Point::new(12.0, size.height - 28.0),
        &line,
        color,
        12.0,
    );
}

fn draw_overlay(frame: &mut Frame, camera: &Camera, width_px: f32, wiring: bool) {
    let zoom_pct = (camera.zoom * 100.0).round();
    let _ = camera.zoom >= ZOOM_MIN - 1e-9 && camera.zoom <= ZOOM_MAX + 1e-9;
    let mode = if wiring {
        "WIRING · release on pin to connect"
    } else {
        "WORLD · C1 live · Shift+LMB latch while Running · Pause to edit"
    };
    fill_label(
        frame,
        Point::new(12.0, 14.0),
        &format!("{mode} {zoom_pct:.0}% · scroll=zoom · middle=pan"),
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
