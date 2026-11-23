//! Live Simulation scene on the logical grid.
//!
//! Layout is authored in **grid units**; zoom is applied only when emitting SVG
//! (`px = grid * 48 * zoom` via a root `scale` transform). Terminals sit on
//! exact full-module intersections; bodies use half-module centers.

use crate::grid::{
    grid_to_px, is_terminal_intersection, px_per_grid, snap_half_module, snap_terminal,
    DEFAULT_SCENE_HEIGHT_U, DEFAULT_SCENE_WIDTH_U, PX_PER_GRID_AT_100,
};

/// Live pin / visual state for the MVP C01 scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LiveSceneState {
    pub led_on: bool,
    pub button_pressed: bool,
}

/// Fixed C01 layout in grid units (terminals on integers).
#[derive(Debug, Clone, Copy)]
pub struct C01Layout {
    pub width_u: f64,
    pub height_u: f64,
    /// Board body origin (top-left), size in modules.
    pub board_origin: (f64, f64),
    pub board_size: (f64, f64),
    /// Board bornes (full-module).
    pub board_d13: (f64, f64),
    pub board_d2: (f64, f64),
    pub board_gnd: (f64, f64),
    /// LED body center (half-module) + A / K bornes.
    pub led_center: (f64, f64),
    pub led_a: (f64, f64),
    pub led_k: (f64, f64),
    /// Button body center + NO / COM bornes.
    pub btn_center: (f64, f64),
    pub btn_no: (f64, f64),
    pub btn_com: (f64, f64),
}

impl Default for C01Layout {
    fn default() -> Self {
        // All terminals are integers; body centers on *.5 half-modules.
        let layout = Self {
            width_u: DEFAULT_SCENE_WIDTH_U,
            height_u: DEFAULT_SCENE_HEIGHT_U,
            board_origin: (1.0, 2.0),
            board_size: (7.0, 8.0),
            board_d13: (8.0, 3.0),
            board_d2: (8.0, 5.0),
            board_gnd: (8.0, 7.0),
            led_center: (14.5, 3.5),
            led_a: (13.0, 3.0),
            led_k: (13.0, 4.0),
            btn_center: (14.5, 7.5),
            btn_no: (13.0, 7.0),
            btn_com: (13.0, 8.0),
        };
        layout.assert_terminal_contract();
        layout
    }
}

impl C01Layout {
    /// Panic in tests / debug if any borne left the full-module lattice.
    pub fn assert_terminal_contract(&self) {
        for (name, (x, y)) in [
            ("board_d13", self.board_d13),
            ("board_d2", self.board_d2),
            ("board_gnd", self.board_gnd),
            ("led_a", self.led_a),
            ("led_k", self.led_k),
            ("btn_no", self.btn_no),
            ("btn_com", self.btn_com),
        ] {
            assert!(
                is_terminal_intersection(x) && is_terminal_intersection(y),
                "{name} terminal ({x},{y}) not on full-module intersection"
            );
            let _ = (snap_terminal(x), snap_terminal(y));
        }
        for (name, (x, y)) in [
            ("led_center", self.led_center),
            ("btn_center", self.btn_center),
        ] {
            let sx = snap_half_module(x);
            let sy = snap_half_module(y);
            assert!(
                (sx - x).abs() < 1e-9 && (sy - y).abs() < 1e-9,
                "{name} ({x},{y}) not on half-module"
            );
        }
    }

    pub fn pixel_size(&self, zoom: f64) -> (f64, f64) {
        (
            grid_to_px(self.width_u, zoom),
            grid_to_px(self.height_u, zoom),
        )
    }
}

/// Render the live C01 scene as SVG. Geometry is in grid units; zoom is a
/// root scale so wire/terminal math stays on the logical lattice.
pub fn render_live_scene_svg(state: &LiveSceneState, zoom: f64) -> String {
    render_live_scene_svg_with_layout(state, zoom, &C01Layout::default())
}

pub fn render_live_scene_svg_with_layout(
    state: &LiveSceneState,
    zoom: f64,
    layout: &C01Layout,
) -> String {
    let zoom = zoom.clamp(crate::camera::ZOOM_MIN, crate::camera::ZOOM_MAX);
    let scale = px_per_grid(zoom);
    let (pw, ph) = layout.pixel_size(zoom);
    let led_fill = if state.led_on { "#ef4444" } else { "#7f1d1d" };
    let led_opacity = if state.led_on { "1" } else { "0.4" };
    let led_class = if state.led_on { "on" } else { "off" };
    let btn_fill = if state.button_pressed {
        "#10b981"
    } else {
        "#27ae60"
    };
    let btn_class = if state.button_pressed {
        "pressed"
    } else {
        "released"
    };
    let contact_class = if state.button_pressed { "closed" } else { "open" };
    let wire_active = if state.led_on { "#f59e0b" } else { "#64748b" };
    let dig_active = if state.button_pressed {
        "#34d399"
    } else {
        "#00e0ff"
    };

    let mut out = String::with_capacity(8_192);
    out.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{pw:.4}" height="{ph:.4}" viewBox="0 0 {pw:.4} {ph:.4}" data-df-grid-unit-mm="4" data-df-px-per-grid-100="{base}" data-df-zoom="{zoom:.4}">
<defs>
  <style>
    .df-grid-full {{ stroke:#1e293b; stroke-width:0.02; }}
    .df-grid-half {{ stroke:#17223b; stroke-width:0.012; stroke-dasharray:0.06 0.06; }}
    .df-label {{ font-family: 'JetBrains Mono', 'Roboto Mono', monospace; fill:#94a3b8; font-size:0.28px; }}
    .df-title {{ font-family: 'Roboto Condensed', sans-serif; fill:#f8fafc; font-size:0.42px; font-weight:700; }}
    .df-borne {{ fill:#00e0ff; stroke:#0b1326; stroke-width:0.04; }}
    .df-borne-gnd {{ fill:#a0aec0; stroke:#0b1326; stroke-width:0.04; }}
    #led-body.on {{ filter: url(#df-glow); }}
  </style>
  <filter id="df-glow" x="-50%" y="-50%" width="200%" height="200%">
    <feGaussianBlur in="SourceGraphic" stdDeviation="0.08" result="b"/>
    <feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge>
  </filter>
</defs>
<rect width="{pw:.4}" height="{ph:.4}" fill="#0b1326"/>
<g id="df-logical-root" transform="scale({scale:.6})">
"##,
        pw = pw,
        ph = ph,
        base = PX_PER_GRID_AT_100,
        zoom = zoom,
        scale = scale,
    ));

    // Half-module then full-module grid (drawn in grid units).
    out.push_str(r#"<g id="df-grid" pointer-events="none">"#);
    let mut x = 0.0;
    while x <= layout.width_u + 1e-9 {
        let half = (x * 2.0).round() % 2.0 != 0.0;
        let cls = if half { "df-grid-half" } else { "df-grid-full" };
        out.push_str(&format!(
            r#"<line class="{cls}" x1="{x}" y1="0" x2="{x}" y2="{h}"/>"#,
            cls = cls,
            x = x,
            h = layout.height_u
        ));
        x += 0.5;
    }
    let mut y = 0.0;
    while y <= layout.height_u + 1e-9 {
        let half = (y * 2.0).round() % 2.0 != 0.0;
        let cls = if half { "df-grid-half" } else { "df-grid-full" };
        out.push_str(&format!(
            r#"<line class="{cls}" x1="0" y1="{y}" x2="{w}" y2="{y}"/>"#,
            cls = cls,
            y = y,
            w = layout.width_u
        ));
        y += 0.5;
    }
    out.push_str("</g>\n");

    // Caption
    out.push_str(
        r#"<text class="df-title" x="0.4" y="0.7">LOGICAL GRID · 1u = 4 mm = 48 px @ 100%</text>"#,
    );
    out.push_str(&format!(
        r#"<text class="df-label" x="0.4" y="1.15">zoom={:.0}% · terminals on full modules · bodies on half</text>"#,
        zoom * 100.0
    ));

    // Board block
    let (bx, by) = layout.board_origin;
    let (bw, bh) = layout.board_size;
    out.push_str(&format!(
        r##"<g id="arduino-uno-r3">
  <rect x="{bx}" y="{by}" width="{bw}" height="{bh}" rx="0.15" fill="#017e7b" stroke="#0f766e" stroke-width="0.06"/>
  <rect x="{inx}" y="{iny}" width="{inw}" height="{inh}" fill="#0b1326" opacity="0.25"/>
  <text class="df-title" x="{tx}" y="{ty}" font-size="0.55px" fill="#ecfdf5">UNO R3</text>
  <text class="df-label" x="{tx}" y="{ty2}" fill="#a7f3d0">ATmega328P</text>
</g>
"##,
        bx = bx,
        by = by,
        bw = bw,
        bh = bh,
        inx = bx + 0.4,
        iny = by + 1.2,
        inw = bw - 0.8,
        inh = bh - 2.0,
        tx = bx + 0.5,
        ty = by + 0.7,
        ty2 = by + 1.05,
    ));

    // Wires (orthogonal, on full-module lattice)
    let (d13x, d13y) = layout.board_d13;
    let (d2x, d2y) = layout.board_d2;
    let (gndx, gndy) = layout.board_gnd;
    let (ax, ay) = layout.led_a;
    let (kx, ky) = layout.led_k;
    let (nox, noy) = layout.btn_no;
    let (comx, comy) = layout.btn_com;
    let mid_x = 11.0; // full-module routing column

    out.push_str(&format!(
        r##"<g id="wires">
  <path id="wire-d13-led" fill="none" stroke="{wire}" stroke-width="0.08" stroke-linecap="round"
        d="M {d13x} {d13y} H {ax} V {ay}"/>
  <path id="wire-gnd-led" fill="none" stroke="#a0aec0" stroke-width="0.08" stroke-linecap="round"
        d="M {gndx} {gndy} H {mid_x} V {ky} H {kx}"/>
  <path id="wire-d2-btn" fill="none" stroke="{dig}" stroke-width="0.08" stroke-linecap="round"
        d="M {d2x} {d2y} H {mid_x} V {noy} H {nox}"/>
  <path id="wire-gnd-btn" fill="none" stroke="#a0aec0" stroke-width="0.08" stroke-linecap="round"
        d="M {gndx} {gndy} H {comx} V {comy}"/>
</g>
"##,
        wire = wire_active,
        dig = dig_active,
        d13x = d13x,
        d13y = d13y,
        ax = ax,
        ay = ay,
        gndx = gndx,
        gndy = gndy,
        mid_x = mid_x,
        ky = ky,
        kx = kx,
        d2x = d2x,
        d2y = d2y,
        noy = noy,
        nox = nox,
        comx = comx,
        comy = comy,
    ));

    // Board terminals
    for (id, (x, y), label, gnd) in [
        ("borne-d13", layout.board_d13, "D13", false),
        ("borne-d2", layout.board_d2, "D2", false),
        ("borne-gnd", layout.board_gnd, "GND", true),
    ] {
        let cls = if gnd { "df-borne-gnd" } else { "df-borne" };
        out.push_str(&format!(
            r##"<g id="{id}" data-grid-x="{x}" data-grid-y="{y}">
  <circle class="{cls}" cx="{x}" cy="{y}" r="0.12"/>
  <text class="df-label" x="{lx}" y="{ly}" text-anchor="end">{label}</text>
</g>
"##,
            id = id,
            cls = cls,
            x = x,
            y = y,
            lx = x - 0.25,
            ly = y + 0.08,
            label = label,
        ));
    }

    // LED module
    let (lcx, lcy) = layout.led_center;
    out.push_str(&format!(
        r##"<g id="led1" data-grid-cx="{lcx}" data-grid-cy="{lcy}">
  <circle cx="{lcx}" cy="{lcy}" r="1.35" fill="#1e293b" stroke="#334155" stroke-width="0.05"/>
  <circle id="led-body" class="df-component-die {led_class}" cx="{lcx}" cy="{lcy}" r="0.85"
          fill="{led_fill}" opacity="{led_opacity}" data-state="{led_class}"/>
  <circle class="df-borne" cx="{ax}" cy="{ay}" r="0.12" id="led-a"/>
  <circle class="df-borne-gnd" cx="{kx}" cy="{ky}" r="0.12" id="led-k"/>
  <text class="df-label" x="{lcx}" y="{ly}" text-anchor="middle">led1 · A/K</text>
  <text class="df-label" x="{ax}" y="{ayl}" text-anchor="end">A</text>
  <text class="df-label" x="{kx}" y="{kyl}" text-anchor="end">K</text>
</g>
"##,
        lcx = lcx,
        lcy = lcy,
        led_class = led_class,
        led_fill = led_fill,
        led_opacity = led_opacity,
        ax = ax,
        ay = ay,
        kx = kx,
        ky = ky,
        ly = lcy + 1.7,
        ayl = ay - 0.2,
        kyl = ky + 0.35,
    ));

    // Button module
    let (bcx, bcy) = layout.btn_center;
    out.push_str(&format!(
        r##"<g id="cmd-pushbutton-no-start" class="{btn_class}" data-grid-cx="{bcx}" data-grid-cy="{bcy}">
  <circle cx="{bcx}" cy="{bcy}" r="1.35" fill="#1e293b" stroke="#334155" stroke-width="0.05"/>
  <circle id="actuator" class="{btn_class}" cx="{bcx}" cy="{bcy}" r="0.75" fill="{btn_fill}"/>
  <circle class="df-borne" cx="{nox}" cy="{noy}" r="0.12" id="btn-no"/>
  <circle class="df-borne-gnd" cx="{comx}" cy="{comy}" r="0.12" id="btn-com"/>
  <path id="contact" class="{contact}" fill="none" stroke="#ecf0f1" stroke-width="0.06"
        d="M {nox} {noy} L {mx} {noy} L {mx} {comy} L {comx} {comy}"/>
  <text class="df-label" x="{bcx}" y="{bly}" text-anchor="middle">btn_start · NO</text>
  <text class="df-label" x="{nox}" y="{nol}" text-anchor="end">NO</text>
  <text class="df-label" x="{comx}" y="{coml}" text-anchor="end">COM</text>
</g>
"##,
        btn_class = btn_class,
        bcx = bcx,
        bcy = bcy,
        btn_fill = btn_fill,
        nox = nox,
        noy = noy,
        comx = comx,
        comy = comy,
        contact = contact_class,
        mx = nox + 0.55,
        bly = bcy + 1.7,
        nol = noy - 0.2,
        coml = comy + 0.35,
    ));

    out.push_str("</g>\n</svg>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_terminals_on_full_modules() {
        C01Layout::default().assert_terminal_contract();
    }

    #[test]
    fn scene_svg_carries_live_hooks_and_zoom_scale() {
        let off = render_live_scene_svg(
            &LiveSceneState {
                led_on: false,
                button_pressed: false,
            },
            1.0,
        );
        assert!(off.contains("data-df-px-per-grid-100=\"48\""));
        assert!(off.contains("transform=\"scale(48"));
        assert!(off.contains("id=\"led-body\""));
        assert!(off.contains("class=\"df-component-die off\"") || off.contains(" off\""));
        assert!(off.contains("id=\"actuator\""));
        assert!(off.contains("data-grid-x=\"8\"")); // D13/D2/GND column

        let on = render_live_scene_svg(
            &LiveSceneState {
                led_on: true,
                button_pressed: true,
            },
            1.5,
        );
        assert!(on.contains("data-df-zoom=\"1.5000\"") || on.contains("data-df-zoom=\"1.5\""));
        assert!(on.contains("transform=\"scale(72"));
        assert!(on.contains("pressed"));
        assert!(on.contains("class=\"df-component-die on\"") || on.contains(" on\""));
        // Pixel size at 150%: 20*72 × 13*72
        assert!(on.contains("width=\"1440") || on.contains("width=\"1440.0000\""));
    }

    #[test]
    fn zoom_preserves_terminal_grid_identity() {
        let layout = C01Layout::default();
        for z in [0.7_f64, 1.0, 1.5] {
            let svg = render_live_scene_svg_with_layout(
                &LiveSceneState::default(),
                z,
                &layout,
            );
            // Terminal data attributes stay in grid units (not zoomed px).
            assert!(svg.contains("data-grid-x=\"8\""));
            assert!(svg.contains("data-grid-y=\"3\"")); // D13
            let scale = px_per_grid(z);
            assert!(
                svg.contains(&format!("scale({scale:.6}")),
                "missing scale({scale:.6}) in svg"
            );
        }
    }
}
