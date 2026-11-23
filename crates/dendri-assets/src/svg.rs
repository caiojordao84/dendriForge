//! Canonical SVG sanitizer, normalizer, `<g id="df-pins">` 48×48 px / 8 px parser,
//! `<metadata>` provenance reader, and deterministic `usvg`/`resvg`/`tiny-skia` render SHA-256 hasher
//! (`03-asset-and-profile-contract.md` §2, §4.2–§4.3, §7).

use dendri_theme::PinClass;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Normative 1 u grid pitch in SVG user units (`03` §2.1).
pub const CANONICAL_GRID_PX: u32 = 48;
/// Normative terminal marker size in SVG user units (`03` §2.2: `8x8 px` / `r="4"`).
pub const CANONICAL_PIN_PX: u32 = 8;

/// XML namespace used inside `<metadata>` for SVG-embedded provenance (`03` §4.1).
pub const DF_SVG_NS: &str = "https://dendriforge.dev/ns/svg/v2";

/// Editor namespace substrings stripped by canonicalization and rejected in live assets (`03` §4.2).
const FORBIDDEN_NS_MARKERS: &[&str] = &[
    "inkscape",
    "sodipodi",
    "adobe",
    "illustrator",
    "figma",
    "sketch",
    "serif.com",
];

const FORBIDDEN_ELEMENTS: &[&str] = &[
    "script",
    "foreignObject",
    "image",
    "animate",
    "set",
    "animateTransform",
    "animateMotion",
    "animateColor",
];

/// Typed error returned during SVG sanitization, parsing, or `usvg`/`resvg` rendering.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SvgError {
    #[error("XML parse error: {0}")]
    XmlParse(String),
    #[error("root element must be <svg>, found <{0}>")]
    NotSvgRoot(String),
    #[error("forbidden SVG construct: {0}")]
    ForbiddenConstruct(String),
    #[error("missing or malformed numeric viewBox (`viewBox=\"0 0 W H\"`): {0}")]
    InvalidViewBox(String),
    #[error("SVG must contain exactly one <g id=\"df-pins\"> group (found {0})")]
    InvalidPinsGroupCount(usize),
    #[error("invalid pin marker inside <g id=\"df-pins\">: {0}")]
    InvalidPinMarker(String),
    #[error(
        "pin collision detected between `{pin_a}` and `{pin_b}` (distance {dist_px} px < 8 px)"
    )]
    PinCollision {
        pin_a: String,
        pin_b: String,
        dist_px: String,
    },
    #[error("SVG <metadata> provenance error: {0}")]
    InvalidMetadata(String),
    #[error("usvg parse error: {0}")]
    UsvgParse(String),
    #[error("headless tiny-skia render error: {0}")]
    RenderError(String),
}

/// Parsed semantic pin marker from `<g id="df-pins">` (`03` §2.2).
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedPin {
    pub port_id: String,
    pub pin_class: PinClass,
    pub cx: f64,
    pub cy: f64,
}

/// Embedded `<metadata><df:provenance .../></metadata>` fields (`03` §4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SvgProvenanceMeta {
    pub source: String,
    pub symbol_ref: Option<String>,
    pub reviewed_by: String,
    pub reviewed_on: String,
    pub ml_restricted: bool,
}

/// Complete validated summary of a sanitized dual-view SVG asset.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedSvg {
    pub viewbox_w: f64,
    pub viewbox_h: f64,
    pub pins: BTreeMap<String, ParsedPin>,
    pub element_ids: BTreeSet<String>,
    pub provenance: Option<SvgProvenanceMeta>,
}

/// Validates and parses an SVG string against ALL structural and security rules of `03` §2.2 & §4.2.
///
/// - Rejects forbidden tags, event handlers, external references, CSS `var(`, SMIL, base64 blobs,
///   `<?xml-stylesheet?>`, and unstripped editor namespaces.
/// - Enforces explicit numeric `viewBox="0 0 W H"`, single `<g id="df-pins">`, `8x8 px` (`r=4`)
///   pin markers with semantic `df-pin-*` classes only, grid snap to `pin_pitch_px`, and zero
///   collisions (`dist >= 8 px`).
/// - Parses cleanly via the workspace's single `usvg` crate.
pub fn parse_and_validate_svg(svg_src: &str, pin_pitch_px: u32) -> Result<ParsedSvg, SvgError> {
    // 1. Raw pre-checks for processing instructions & CSS/base64 patterns
    let lower = svg_src.to_ascii_lowercase();
    if lower.contains("<?xml-stylesheet") {
        return Err(SvgError::ForbiddenConstruct(
            "<?xml-stylesheet?> processing instruction is forbidden".to_owned(),
        ));
    }
    if lower.contains("@import") {
        return Err(SvgError::ForbiddenConstruct(
            "CSS @import is forbidden".to_owned(),
        ));
    }
    if lower.contains("var(--") {
        return Err(SvgError::ForbiddenConstruct(
            "CSS custom properties `var(--...)` are forbidden".to_owned(),
        ));
    }
    if lower.contains("data:image/") {
        return Err(SvgError::ForbiddenConstruct(
            "inline base64 raster data URIs (`data:image/...`) are forbidden".to_owned(),
        ));
    }

    // 2. Strict XML DOM parse via `roxmltree`
    let doc = roxmltree::Document::parse(svg_src).map_err(|e| SvgError::XmlParse(e.to_string()))?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        return Err(SvgError::NotSvgRoot(root.tag_name().name().to_owned()));
    }

    // Check root & descendant namespaces + forbidden elements/attributes
    let mut pins_groups = Vec::new();
    let mut element_ids = BTreeSet::new();
    let mut provenance: Option<SvgProvenanceMeta> = None;

    for node in doc.descendants() {
        if node.is_pi()
            && let Some(pi) = node.pi()
            && pi.target.eq_ignore_ascii_case("xml-stylesheet")
        {
            return Err(SvgError::ForbiddenConstruct(
                "xml-stylesheet PI is forbidden".to_owned(),
            ));
        }

        if !node.is_element() {
            continue;
        }

        // Check declared XML namespaces for unstripped editor cruft
        for ns in node.namespaces() {
            let uri_l = ns.uri().to_ascii_lowercase();
            let pfx_l = ns.name().unwrap_or("").to_ascii_lowercase();
            for &marker in FORBIDDEN_NS_MARKERS {
                if uri_l.contains(marker) || pfx_l.contains(marker) {
                    return Err(SvgError::ForbiddenConstruct(format!(
                        "unstripped editor namespace `{}` (`{}`)",
                        ns.name().unwrap_or(""),
                        ns.uri()
                    )));
                }
            }
        }

        let tag = node.tag_name().name();
        for &bad in FORBIDDEN_ELEMENTS {
            if tag.eq_ignore_ascii_case(bad) {
                return Err(SvgError::ForbiddenConstruct(format!(
                    "element <{tag}> is forbidden"
                )));
            }
        }

        // Reject event handlers, external hrefs, forbidden style contents, editor attrs
        for attr in node.attributes() {
            let name = attr.name();
            let name_l = name.to_ascii_lowercase();
            let val = attr.value();
            let val_l = val.to_ascii_lowercase();

            if let Some(ns_uri) = attr.namespace() {
                let ns_l = ns_uri.to_ascii_lowercase();
                for &marker in FORBIDDEN_NS_MARKERS {
                    if ns_l.contains(marker) {
                        return Err(SvgError::ForbiddenConstruct(format!(
                            "unstripped editor attribute `{name}` in namespace `{ns_uri}`"
                        )));
                    }
                }
            }

            if name_l.starts_with("on") && name_l.len() > 2 {
                return Err(SvgError::ForbiddenConstruct(format!(
                    "event-handler attribute `{name}` is forbidden"
                )));
            }
            if name_l == "href" {
                let trimmed = val.trim();
                if !trimmed.starts_with('#') {
                    return Err(SvgError::ForbiddenConstruct(format!(
                        "external or non-fragment href `{trimmed}` is forbidden"
                    )));
                }
            }
            if name_l == "style"
                && (val_l.contains("var(")
                    || val_l.contains("url(http")
                    || val_l.contains("url(data:"))
            {
                return Err(SvgError::ForbiddenConstruct(format!(
                    "forbidden CSS expression in style attribute `{val}`"
                )));
            }
        }

        if let Some(id) = node.attribute("id") {
            element_ids.insert(id.to_owned());
            if tag == "g" && id == "df-pins" {
                pins_groups.push(node);
            }
        }

        // Read embedded `<df:provenance .../>` inside `<metadata>`
        if tag == "provenance"
            && node.tag_name().namespace() == Some(DF_SVG_NS)
            && node
                .parent()
                .is_some_and(|p| p.tag_name().name() == "metadata")
        {
            provenance = Some(parse_svg_provenance(node)?);
        }
    }

    // 3. Parse explicit numeric `viewBox="0 0 W H"`
    let vb_raw = root
        .attribute("viewBox")
        .ok_or_else(|| SvgError::InvalidViewBox("missing viewBox attribute".to_owned()))?;
    let (vx, vy, vw, vh) = parse_viewbox(vb_raw)?;
    if libm::fabs(vx) > 1e-6 || libm::fabs(vy) > 1e-6 || vw <= 0.0 || vh <= 0.0 {
        return Err(SvgError::InvalidViewBox(format!(
            "viewBox `{vb_raw}` must start at (0, 0) with positive dimensions"
        )));
    }

    // 4. Require exactly one `<g id="df-pins">` and validate every pin marker
    if pins_groups.len() != 1 {
        return Err(SvgError::InvalidPinsGroupCount(pins_groups.len()));
    }
    let Some(&pins_group) = pins_groups.first() else {
        return Err(SvgError::InvalidPinsGroupCount(0));
    };

    let pitch = f64::from(pin_pitch_px.max(1));
    let mut pins = BTreeMap::new();
    for child in pins_group.children().filter(|n| n.is_element()) {
        let pin = parse_pin_marker(child, pitch, vw, vh)?;
        if pins.contains_key(&pin.port_id) {
            return Err(SvgError::InvalidPinMarker(format!(
                "duplicate pin marker `pin:{}`",
                pin.port_id
            )));
        }
        pins.insert(pin.port_id.clone(), pin);
    }

    // 5. Zero terminal collisions: pairwise distance >= 8.0 px
    let pin_list: Vec<&ParsedPin> = pins.values().collect();
    for (i, a) in pin_list.iter().enumerate() {
        for b in pin_list.iter().skip(i + 1) {
            let dist = libm::hypot(a.cx - b.cx, a.cy - b.cy);
            if dist < f64::from(CANONICAL_PIN_PX) - 1e-3 {
                return Err(SvgError::PinCollision {
                    pin_a: a.port_id.clone(),
                    pin_b: b.port_id.clone(),
                    dist_px: format!("{dist:.2}"),
                });
            }
        }
    }

    // 6. Verify workspace's single `usvg` parses the SVG cleanly
    let _tree = usvg::Tree::from_str(svg_src, &usvg::Options::default())
        .map_err(|e| SvgError::UsvgParse(e.to_string()))?;

    Ok(ParsedSvg {
        viewbox_w: vw,
        viewbox_h: vh,
        pins,
        element_ids,
        provenance,
    })
}

/// Strips editor namespaces/attributes and outputs canonical sorted-attribute XML (`03` §4.2).
///
/// Always validates the resulting canonical SVG via [`parse_and_validate_svg`].
pub fn canonicalize_and_sanitize_svg(raw_svg: &str, pin_pitch_px: u32) -> Result<String, SvgError> {
    let doc = roxmltree::Document::parse(raw_svg).map_err(|e| SvgError::XmlParse(e.to_string()))?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        return Err(SvgError::NotSvgRoot(root.tag_name().name().to_owned()));
    }

    let mut out = String::with_capacity(raw_svg.len());
    write_canonical_element(root, &mut out, 0, true)?;
    out.push('\n');

    // Enforce full contract on canonicalized output
    let _ = parse_and_validate_svg(&out, pin_pitch_px)?;
    Ok(out)
}

/// Computes the deterministic SHA-256 digest over `(profile.toml, canonical SVG, headless tiny-skia render)`
/// (`03` §4.3, `03` §7 #12).
///
/// Any change to `profile.toml`, SVG markup, or rendered pixels changes this hex SHA-256 immediately.
pub fn compute_render_sha256(profile_toml: &str, svg_src: &str) -> Result<String, SvgError> {
    let tree = usvg::Tree::from_str(svg_src, &usvg::Options::default())
        .map_err(|e| SvgError::UsvgParse(e.to_string()))?;
    let size = tree.size();
    let w = (libm::ceil(f64::from(size.width())) as u32).max(1);
    let h = (libm::ceil(f64::from(size.height())) as u32).max(1);
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or_else(|| {
        SvgError::RenderError(format!("failed to allocate {w}x{h} tiny-skia pixmap"))
    })?;
    resvg::render(
        &tree,
        tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );

    let mut hasher = Sha256::new();
    hasher.update(profile_toml.as_bytes());
    hasher.update(b"\0SVG\0");
    hasher.update(svg_src.as_bytes());
    hasher.update(b"\0PIXELS\0");
    hasher.update(pixmap.data());
    Ok(hex::encode(hasher.finalize()))
}

fn parse_viewbox(raw: &str) -> Result<(f64, f64, f64, f64), SvgError> {
    let parts: Vec<&str> = raw
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() != 4 {
        return Err(SvgError::InvalidViewBox(format!(
            "expected 4 numbers in `{raw}`"
        )));
    }
    let parse_num = |idx: usize| -> Result<f64, SvgError> {
        parts
            .get(idx)
            .ok_or_else(|| SvgError::InvalidViewBox(raw.to_owned()))?
            .parse::<f64>()
            .map_err(|_| SvgError::InvalidViewBox(raw.to_owned()))
    };
    Ok((parse_num(0)?, parse_num(1)?, parse_num(2)?, parse_num(3)?))
}

fn parse_pin_marker(
    node: roxmltree::Node<'_, '_>,
    pitch: f64,
    vb_w: f64,
    vb_h: f64,
) -> Result<ParsedPin, SvgError> {
    let id = node.attribute("id").ok_or_else(|| {
        SvgError::InvalidPinMarker("pin marker missing `id=\"pin:<port_id>\"`".to_owned())
    })?;
    let Some(port_id) = id.strip_prefix("pin:") else {
        return Err(SvgError::InvalidPinMarker(format!(
            "pin marker id `{id}` must start with `pin:`"
        )));
    };
    if port_id.is_empty() {
        return Err(SvgError::InvalidPinMarker(
            "empty port id in `pin:` marker".to_owned(),
        ));
    }

    // Zero hardcoded colors inside `<g id="df-pins">` (`03` §2.2)
    for forbidden_color_attr in ["fill", "stroke", "color", "style"] {
        if node.attribute(forbidden_color_attr).is_some() {
            return Err(SvgError::InvalidPinMarker(format!(
                "pin `{port_id}` must not declare hardcoded `{forbidden_color_attr}` attribute; use semantic `df-pin-*` class only"
            )));
        }
    }

    let class_str = node.attribute("class").ok_or_else(|| {
        SvgError::InvalidPinMarker(format!(
            "pin `{port_id}` missing semantic `class=\"df-pin-*\"`"
        ))
    })?;
    let pin_class = PinClass::from_css_class(class_str.trim()).ok_or_else(|| {
        SvgError::InvalidPinMarker(format!(
            "pin `{port_id}` has unknown semantic class `{class_str}`"
        ))
    })?;

    let read_f64 = |attr: &str| -> Result<f64, SvgError> {
        node.attribute(attr)
            .ok_or_else(|| {
                SvgError::InvalidPinMarker(format!("pin `{port_id}` missing attribute `{attr}`"))
            })?
            .parse::<f64>()
            .map_err(|_| {
                SvgError::InvalidPinMarker(format!(
                    "pin `{port_id}` has non-numeric attribute `{attr}`"
                ))
            })
    };

    let (cx, cy) = match node.tag_name().name() {
        "circle" => {
            let cx = read_f64("cx")?;
            let cy = read_f64("cy")?;
            let r = read_f64("r")?;
            if libm::fabs(r - 4.0) > 1e-3 {
                return Err(SvgError::InvalidPinMarker(format!(
                    "circle pin `{port_id}` must have r=\"4\" (8px diameter), found r={r}"
                )));
            }
            (cx, cy)
        }
        "rect" => {
            let x = read_f64("x")?;
            let y = read_f64("y")?;
            let w = read_f64("width")?;
            let h = read_f64("height")?;
            if libm::fabs(w - 8.0) > 1e-3 || libm::fabs(h - 8.0) > 1e-3 {
                return Err(SvgError::InvalidPinMarker(format!(
                    "rect pin `{port_id}` must be 8x8 px, found {w}x{h}"
                )));
            }
            (x + 4.0, y + 4.0)
        }
        other => {
            return Err(SvgError::InvalidPinMarker(format!(
                "pin `{port_id}` must be <circle r=\"4\"> or <rect width=\"8\" height=\"8\">, found <{other}>"
            )));
        }
    };

    // Enforce grid snap to `pitch` and viewBox bounds
    let rem_x = libm::fabs(cx - libm::round(cx / pitch) * pitch);
    let rem_y = libm::fabs(cy - libm::round(cy / pitch) * pitch);
    if rem_x > 1e-3 || rem_y > 1e-3 {
        return Err(SvgError::InvalidPinMarker(format!(
            "pin `{port_id}` at ({cx}, {cy}) is not snapped to grid pitch {pitch} px"
        )));
    }
    if cx < -1e-3 || cy < -1e-3 || cx > vb_w + 1e-3 || cy > vb_h + 1e-3 {
        return Err(SvgError::InvalidPinMarker(format!(
            "pin `{port_id}` at ({cx}, {cy}) lies outside viewBox 0..={vb_w} x 0..={vb_h}"
        )));
    }

    Ok(ParsedPin {
        port_id: port_id.to_owned(),
        pin_class,
        cx,
        cy,
    })
}

fn parse_svg_provenance(node: roxmltree::Node<'_, '_>) -> Result<SvgProvenanceMeta, SvgError> {
    let req = |attr: &str| -> Result<String, SvgError> {
        let v = node.attribute(attr).ok_or_else(|| {
            SvgError::InvalidMetadata(format!("missing `{attr}` on <df:provenance>"))
        })?;
        if v.trim().is_empty() {
            return Err(SvgError::InvalidMetadata(format!(
                "empty `{attr}` on <df:provenance>"
            )));
        }
        Ok(v.to_owned())
    };
    let source = req("source")?;
    let reviewed_by = req("reviewed-by")?;
    let reviewed_on = req("reviewed-on")?;
    let ml_raw = req("ml-restricted")?;
    let ml_restricted = match ml_raw.as_str() {
        "true" => true,
        "false" => false,
        other => {
            return Err(SvgError::InvalidMetadata(format!(
                "invalid ml-restricted `{other}` (expected true|false)"
            )));
        }
    };
    let symbol_ref = node
        .attribute("symbol-ref")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Ok(SvgProvenanceMeta {
        source,
        symbol_ref,
        reviewed_by,
        reviewed_on,
        ml_restricted,
    })
}

fn write_canonical_element(
    node: roxmltree::Node<'_, '_>,
    out: &mut String,
    depth: usize,
    is_root: bool,
) -> Result<(), SvgError> {
    // Skip editor-namespaced elements completely during canonicalization (`03` §4.2)
    if let Some(ns) = node.tag_name().namespace() {
        let ns_l = ns.to_ascii_lowercase();
        if FORBIDDEN_NS_MARKERS.iter().any(|m| ns_l.contains(m)) {
            return Ok(());
        }
    }

    let tag = node.tag_name().name();
    let indent = "  ".repeat(depth);
    out.push_str(&indent);
    out.push('<');
    if node.tag_name().namespace() == Some(DF_SVG_NS) {
        out.push_str("df:");
    }
    out.push_str(tag);

    if is_root {
        out.push_str(" xmlns=\"http://www.w3.org/2000/svg\"");
        out.push_str(" xmlns:df=\"");
        out.push_str(DF_SVG_NS);
        out.push('"');
    }

    // Collect & sort non-editor attributes lexicographically for 100% deterministic XML (`03` §4.2)
    let mut attrs: BTreeMap<String, String> = BTreeMap::new();
    for attr in node.attributes() {
        if let Some(ns) = attr.namespace() {
            let ns_l = ns.to_ascii_lowercase();
            if FORBIDDEN_NS_MARKERS.iter().any(|m| ns_l.contains(m)) {
                continue;
            }
        }
        attrs.insert(attr.name().to_owned(), attr.value().to_owned());
    }
    for (k, v) in attrs {
        out.push(' ');
        out.push_str(&k);
        out.push_str("=\"");
        out.push_str(&escape_xml_attr(&v));
        out.push('"');
    }

    let elem_children: Vec<roxmltree::Node<'_, '_>> =
        node.children().filter(|c| c.is_element()).collect();
    let text_content: String = node
        .children()
        .filter(|c| c.is_text())
        .filter_map(|c| c.text())
        .collect();
    let trimmed_text = text_content.trim();

    if elem_children.is_empty() && trimmed_text.is_empty() {
        out.push_str("/>");
    } else if elem_children.is_empty() {
        out.push('>');
        out.push_str(&escape_xml_text(trimmed_text));
        out.push_str("</");
        if node.tag_name().namespace() == Some(DF_SVG_NS) {
            out.push_str("df:");
        }
        out.push_str(tag);
        out.push('>');
    } else {
        out.push_str(">\n");
        for child in elem_children {
            write_canonical_element(child, out, depth + 1, false)?;
            out.push('\n');
        }
        out.push_str(&indent);
        out.push_str("</");
        if node.tag_name().namespace() == Some(DF_SVG_NS) {
            out.push_str("df:");
        }
        out.push_str(tag);
        out.push('>');
    }

    Ok(())
}

fn escape_xml_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

fn escape_xml_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLEAN_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:df="https://dendriforge.dev/ns/svg/v2" viewBox="0 0 96 48">
  <metadata>
    <df:provenance source="original" symbol-ref="IEC 60617-S00555" reviewed-by="caio" reviewed-on="2026-11-23" ml-restricted="false"/>
  </metadata>
  <g id="body">
    <rect id="resistor-body" x="24" y="16" width="48" height="16" fill="#334155" stroke="#cbd5e1" stroke-width="2"/>
  </g>
  <g id="df-pins">
    <circle id="pin:1" class="df-pin-passive" cx="0" cy="24" r="4"/>
    <circle id="pin:2" class="df-pin-passive" cx="96" cy="24" r="4"/>
  </g>
</svg>"##;

    #[test]
    fn validates_clean_svg_and_produces_deterministic_render_sha256() {
        let parsed = parse_and_validate_svg(CLEAN_SVG, 24).unwrap();
        assert_eq!(parsed.viewbox_w, 96.0);
        assert_eq!(parsed.viewbox_h, 48.0);
        assert_eq!(parsed.pins.len(), 2);
        assert!(parsed.element_ids.contains("resistor-body"));
        assert_eq!(
            parsed
                .provenance
                .as_ref()
                .and_then(|p| p.symbol_ref.as_deref()),
            Some("IEC 60617-S00555")
        );

        let h1 = compute_render_sha256("profile = 1", CLEAN_SVG).unwrap();
        let h2 = compute_render_sha256("profile = 1", CLEAN_SVG).unwrap();
        let h3 = compute_render_sha256("profile = 2", CLEAN_SVG).unwrap();
        assert_eq!(h1.len(), 64);
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn sanitizer_rejects_script_var_smil_hardcoded_pin_color_and_collisions() {
        // Script tag
        let bad_script = CLEAN_SVG.replace("</svg>", "<script>alert(1)</script></svg>");
        assert!(matches!(
            parse_and_validate_svg(&bad_script, 24),
            Err(SvgError::ForbiddenConstruct(_))
        ));

        // CSS var(--...)
        let bad_var = CLEAN_SVG.replace("#334155", "var(--df-accent)");
        assert!(matches!(
            parse_and_validate_svg(&bad_var, 24),
            Err(SvgError::ForbiddenConstruct(_))
        ));

        // SMIL animate
        let bad_smil = CLEAN_SVG.replace(
            "</g>\n  <g id=\"df-pins\">",
            "<animate attributeName=\"x\"/></g>\n  <g id=\"df-pins\">",
        );
        assert!(matches!(
            parse_and_validate_svg(&bad_smil, 24),
            Err(SvgError::ForbiddenConstruct(_))
        ));

        // Hardcoded fill on df-pin marker
        let bad_pin_color = CLEAN_SVG.replace("r=\"4\"/>", "r=\"4\" fill=\"#ff0000\"/>");
        assert!(matches!(
            parse_and_validate_svg(&bad_pin_color, 24),
            Err(SvgError::InvalidPinMarker(_))
        ));

        // Pin collision (distance 0 < 8 px)
        let bad_collide = CLEAN_SVG.replace("cx=\"96\"", "cx=\"0\"");
        assert!(matches!(
            parse_and_validate_svg(&bad_collide, 24),
            Err(SvgError::PinCollision { .. })
        ));
    }

    #[test]
    fn canonicalizer_strips_inkscape_editor_cruft() {
        let dirty = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape" xmlns:df="https://dendriforge.dev/ns/svg/v2" viewBox="0 0 96 48">
  <metadata>
    <df:provenance source="original" symbol-ref="IEC 60617-S00555" reviewed-by="caio" reviewed-on="2026-11-23" ml-restricted="false"/>
  </metadata>
  <g id="body" inkscape:label="Layer 1">
    <rect id="resistor-body" x="24" y="16" width="48" height="16" fill="#334155"/>
  </g>
  <g id="df-pins">
    <circle id="pin:1" class="df-pin-passive" cx="0" cy="24" r="4"/>
    <circle id="pin:2" class="df-pin-passive" cx="96" cy="24" r="4"/>
  </g>
</svg>"##;
        // Raw dirty SVG fails validation
        assert!(matches!(
            parse_and_validate_svg(dirty, 24),
            Err(SvgError::ForbiddenConstruct(_))
        ));
        // Canonicalized output passes validation cleanly
        let clean = canonicalize_and_sanitize_svg(dirty, 24).unwrap();
        assert!(!clean.contains("inkscape"));
        assert!(parse_and_validate_svg(&clean, 24).is_ok());
    }
}
