//! Legacy TOON → TOML profile converter (DEN-14).
//!
//! Parses the profile dialect under `core/{boards,components,screens}/*.toon`
//! into TOML suitable for `content/profiles/`. TOON remains read-only legacy;
//! this module exists so migration is automated and re-runnable.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToonError {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse {path}: {message}")]
    Parse { path: String, message: String },
    #[error("emit: {0}")]
    Emit(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileFamily {
    Boards,
    Components,
    Screens,
}

impl ProfileFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Boards => "boards",
            Self::Components => "components",
            Self::Screens => "screens",
        }
    }

    pub fn from_dir_name(name: &str) -> Option<Self> {
        match name {
            "boards" => Some(Self::Boards),
            "components" => Some(Self::Components),
            "screens" => Some(Self::Screens),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MigrateReport {
    pub family: ProfileFamily,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub profile_id: String,
    pub svg_path: Option<String>,
    pub svg_present: bool,
    pub svg_copied: bool,
}

/// Convert one TOON profile file into a TOML document string.
pub fn convert_toon_file(path: &Path, family: ProfileFamily) -> Result<(String, String), ToonError> {
    let text = fs::read_to_string(path)?;
    convert_toon_str(&text, family, path)
}

pub fn convert_toon_str(
    text: &str,
    family: ProfileFamily,
    path: &Path,
) -> Result<(String, String), ToonError> {
    let mut root = parse_toon_document(text).map_err(|message| ToonError::Parse {
        path: path.display().to_string(),
        message,
    })?;

    // Drop legacy TOON ASL version markers from profiles (language tag is on program IR).
    if let Some(Value::Map(meta)) = root.get_mut("metadata") {
        meta.remove("ASLversion");
        meta.remove("aslversion");
        meta.remove("version");
        meta.remove("asl_version");
    }

    restructure_identification(&mut root);
    normalize_keys(&mut root);
    let profile_id = extract_profile_file_stem(path, &root);
    apply_dendri_section(&mut root, family, &profile_id);

    let header = format!(
        "# {} profile — {} (migrated from legacy TOON)\n# Source: {}\n# DEN-14: do not assign ASL 0.0.1\n\n",
        family.as_str().trim_end_matches('s'),
        profile_id,
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown.toon")
    );
    let body = emit_toml(&root).map_err(ToonError::Emit)?;
    Ok((profile_id, format!("{header}{body}")))
}

/// Migrate all `*.toon` under `core/{family}` into `content/profiles/{family}`.
/// Copies known SVGs into `content/assets/svg/{family}/` when present.
pub fn migrate_family(
    repo_root: &Path,
    family: ProfileFamily,
) -> Result<Vec<MigrateReport>, ToonError> {
    let src_dir = repo_root.join("core").join(family.as_str());
    let dst_dir = repo_root
        .join("content")
        .join("profiles")
        .join(family.as_str());
    let asset_dir = repo_root
        .join("content")
        .join("assets")
        .join("svg")
        .join(family.as_str());
    fs::create_dir_all(&dst_dir)?;
    fs::create_dir_all(&asset_dir)?;

    let mut reports = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(&src_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("toon"))
        .collect();
    entries.sort();

    let mut keep_stems = std::collections::HashSet::new();
    for src in entries {
        let (profile_id, toml_text) = convert_toon_file(&src, family)?;
        keep_stems.insert(profile_id.clone());
        let dst = dst_dir.join(format!("{profile_id}.toml"));
        fs::write(&dst, toml_text)?;

        let image_name = read_image_field(&dst).unwrap_or_else(|| format!("{profile_id}.svg"));
        let (svg_path, svg_present, svg_copied) =
            stage_svg(repo_root, family, &profile_id, &image_name, &asset_dir)?;

        // Patch svg fields after staging so presence is accurate.
        patch_svg_fields(&dst, family, &svg_path, svg_present)?;

        reports.push(MigrateReport {
            family,
            source: src,
            destination: dst,
            profile_id,
            svg_path: Some(svg_path),
            svg_present,
            svg_copied,
        });
    }

    // Drop stale TOMLs left from earlier mis-keyed migrations.
    if dst_dir.exists() {
        for entry in fs::read_dir(&dst_dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if !keep_stems.contains(stem) {
                fs::remove_file(&path)?;
            }
        }
    }
    Ok(reports)
}

fn read_image_field(toml_path: &Path) -> Option<String> {
    let text = fs::read_to_string(toml_path).ok()?;
    let value: toml::Value = toml::from_str(&text).ok()?;
    value
        .get("identification")?
        .get("image")?
        .as_str()
        .map(|s| s.to_string())
}

fn stage_svg(
    repo_root: &Path,
    family: ProfileFamily,
    profile_id: &str,
    image_name: &str,
    asset_dir: &Path,
) -> Result<(String, bool, bool), ToonError> {
    let preferred = format!(
        "content/assets/svg/{}/{}",
        family.as_str(),
        image_name
    );
    let preferred_abs = repo_root.join(&preferred);
    if preferred_abs.exists() {
        return Ok((preferred, true, false));
    }

    // Known core locations + Siemens filename mismatch.
    let mut candidates = vec![
        repo_root
            .join("core")
            .join(family.as_str())
            .join(image_name),
    ];
    if family == ProfileFamily::Boards && profile_id == "siemens-s7-1200-cpu-1214c" {
        candidates.push(
            repo_root
                .join("core")
                .join("boards")
                .join("siemens-s7-1200-cpu-1214c-corrigido.svg"),
        );
    }

    for cand in candidates {
        if cand.exists() {
            let dest = asset_dir.join(image_name);
            fs::copy(&cand, &dest)?;
            return Ok((preferred, true, true));
        }
    }
    Ok((preferred, false, false))
}

fn patch_svg_fields(
    toml_path: &Path,
    family: ProfileFamily,
    svg_path: &str,
    present: bool,
) -> Result<(), ToonError> {
    let mut text = fs::read_to_string(toml_path)?;
    // Ensure [dendri] block carries canonical svg_path / status.
    if !text.contains("[dendri]") {
        text.push_str("\n[dendri]\n");
    }
    // Rewrite svg_path line if present, else append under [dendri].
    let status = if present { "authored" } else { "missing" };
    let svg_lines = format!(
        "svg_path = \"{svg_path}\"\nsvg_status = \"{status}\"\n"
    );

    if let Some(idx) = text.find("[dendri]") {
        let after = &text[idx + "[dendri]".len()..];
        let next_table = after.find("\n[").map(|i| idx + "[dendri]".len() + i);
        let end = next_table.unwrap_or(text.len());
        let mut block = text[idx..end].to_string();
        block = replace_or_insert_key(&block, "svg_path", &format!("svg_path = \"{svg_path}\""));
        block = replace_or_insert_key(&block, "svg_status", &format!("svg_status = \"{status}\""));
        // Preserve / set MVP overlays.
        if matches!(
            toml_path.file_stem().and_then(|s| s.to_str()),
            Some("arduino-uno-r3" | "led" | "cmd-pushbutton-no-start")
        ) {
            block = replace_or_insert_key(&block, "visual_mvp", "visual_mvp = true");
        }
        if toml_path.file_stem().and_then(|s| s.to_str()) == Some("arduino-uno-r3") {
            block = replace_or_insert_key(
                &block,
                "svg_required_ids",
                "svg_required_ids = [\"led-pin13\"]",
            );
            block = replace_or_insert_key(
                &block,
                "svg_note",
                "svg_note = \"Canvas binds #led-pin13 + D13 pad; prefers content/assets/svg/\"",
            );
        }
        if toml_path.file_stem().and_then(|s| s.to_str()) == Some("led") {
            block = replace_or_insert_key(
                &block,
                "svg_required_ids",
                "svg_required_ids = [\"led-body\"]",
            );
        }
        if toml_path.file_stem().and_then(|s| s.to_str()) == Some("cmd-pushbutton-no-start") {
            block = replace_or_insert_key(
                &block,
                "svg_required_ids",
                "svg_required_ids = [\"actuator\", \"contact\"]",
            );
        }
        let _ = family; // family used for future per-family overlays
        text.replace_range(idx..end, &block);
    } else {
        text.push_str(&format!("\n[dendri]\n{svg_lines}"));
    }
    fs::write(toml_path, text)?;
    Ok(())
}

fn replace_or_insert_key(block: &str, key: &str, line: &str) -> String {
    let mut out = String::new();
    let mut replaced = false;
    for l in block.lines() {
        let trimmed = l.trim_start();
        if trimmed.starts_with(&format!("{key} "))
            || trimmed.starts_with(&format!("{key}="))
            || trimmed.starts_with(&format!("{key} ="))
        {
            out.push_str(line);
            out.push('\n');
            replaced = true;
        } else {
            out.push_str(l);
            out.push('\n');
        }
    }
    if !replaced {
        // Insert before trailing blank.
        let trimmed = out.trim_end().to_string();
        format!("{trimmed}\n{line}\n")
    } else {
        out
    }
}

/// TOON identification fields are flat under the IDENTIFICATION section; fold them
/// into an `identification` map so the TOML matches `ProfileDocument`.
fn restructure_identification(root: &mut Value) {
    let Value::Map(map) = root else {
        return;
    };
    if map.contains_key("identification") {
        return;
    }
    const KEYS: &[&str] = &[
        "id",
        "name",
        "manufacturer",
        "mcu",
        "cpu",
        "category",
        "family",
        "subfamily",
        "standard",
        "form_factor",
        "image",
        "symbol",
        "symbol_standard",
        "url",
    ];
    let mut ident = BTreeMap::new();
    for key in KEYS {
        if let Some(val) = map.remove(*key) {
            if !matches!(val, Value::Null) {
                ident.insert((*key).to_string(), val);
            }
        }
    }
    if !ident.is_empty() {
        map.insert("identification".into(), Value::Map(ident));
    }
}

fn extract_profile_file_stem(path: &Path, _root: &Value) -> String {
    // Always key the TOML file by the legacy TOON filename stem so duplicate /
    // mis-labeled `boardProfileId` values in the corpus cannot collide
    // (e.g. arduino-nano-rp2040.toon incorrectly cites *-connect).
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}

fn apply_dendri_section(root: &mut Value, family: ProfileFamily, profile_id: &str) {
    let Value::Map(map) = root else {
        return;
    };
    // Rename dendriForge → dendri (already done by normalize for camelCase keys inside).
    if let Some(Value::Map(legacy)) = map.remove("dendriForge") {
        let mut dendri = BTreeMap::new();
        for (k, v) in legacy {
            dendri.insert(camel_to_snake(&k), v);
        }
        map.insert("dendri".into(), Value::Map(dendri));
    }
    let image = map
        .get("identification")
        .and_then(|v| match v {
            Value::Map(m) => m.get("image").and_then(|x| match x {
                Value::String(s) => Some(s.clone()),
                _ => None,
            }),
            _ => None,
        })
        .unwrap_or_else(|| format!("{profile_id}.svg"));
    let svg_path = format!(
        "content/assets/svg/{}/{}",
        family.as_str(),
        image
    );
    let dendri = match map.entry("dendri".into()) {
        std::collections::btree_map::Entry::Occupied(o) => o.into_mut(),
        std::collections::btree_map::Entry::Vacant(v) => {
            v.insert(Value::Map(BTreeMap::new()))
        }
    };
    let Value::Map(d) = dendri else {
        return;
    };
    match family {
        ProfileFamily::Boards => {
            // Force packer id to the file stem (canonical, collision-free).
            d.insert(
                "board_profile_id".into(),
                Value::String(profile_id.to_string()),
            );
        }
        ProfileFamily::Components | ProfileFamily::Screens => {
            d.insert(
                "component_profile_id".into(),
                Value::String(profile_id.to_string()),
            );
        }
    }
    d.insert("svg_path".into(), Value::String(svg_path));
}

fn normalize_keys(value: &mut Value) {
    match value {
        Value::Map(map) => {
            let old = std::mem::take(map);
            for (k, mut v) in old {
                normalize_keys(&mut v);
                let nk = match k.as_str() {
                    "powerPins" => "power_pins".into(),
                    "dendriForge" => "dendri".into(),
                    other => camel_to_snake(other),
                };
                // Special: roles string with ';' → array
                if nk == "roles" {
                    if let Value::String(s) = &v {
                        if s.contains(';') {
                            let parts: Vec<Value> = s
                                .split(';')
                                .map(|p| p.trim())
                                .filter(|p| !p.is_empty() && *p != "null")
                                .map(|p| Value::String(p.to_string()))
                                .collect();
                            v = Value::Array(parts);
                        } else if s == "null" {
                            v = Value::Array(vec![]);
                        } else {
                            v = Value::Array(vec![Value::String(s.clone())]);
                        }
                    }
                }
                map.insert(nk, v);
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize_keys(item);
            }
        }
        _ => {}
    }
}

fn camel_to_snake(s: &str) -> String {
    if s.contains('_') || s.chars().all(|c| !c.is_ascii_uppercase()) {
        return s.to_string();
    }
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

// --- TOON value model -------------------------------------------------------

#[derive(Debug, Clone)]
enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Array(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

impl Value {
    fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        match self {
            Value::Map(m) => m.get_mut(key),
            _ => None,
        }
    }
}

fn parse_toon_document(text: &str) -> Result<Value, String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    let mut root = BTreeMap::new();

    // Optional METADATA block headed by `# METADATA:`
    while i < lines.len() {
        let raw = lines[i];
        let trimmed = raw.trim();
        if trimmed.is_empty() || (trimmed.starts_with('#') && !trimmed.starts_with("# METADATA")) {
            i += 1;
            continue;
        }
        break;
    }
    if i < lines.len() && lines[i].trim().starts_with("# METADATA") {
        i += 1;
        let mut meta = BTreeMap::new();
        while i < lines.len() {
            let trimmed = lines[i].trim();
            if trimmed.is_empty() {
                i += 1;
                continue;
            }
            if trimmed.starts_with("##") {
                break;
            }
            if trimmed.starts_with('#') {
                i += 1;
                continue;
            }
            // Metadata keys are flat scalars at column 0.
            if leading_spaces(lines[i]) > 0 {
                break;
            }
            let (key, val, next) = parse_entry(&lines, i, 0)?;
            meta.insert(key, val);
            i = next;
        }
        root.insert("metadata".into(), Value::Map(meta));
    }

    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.is_empty() || (trimmed.starts_with('#') && !trimmed.starts_with("##")) {
            i += 1;
            continue;
        }
        if trimmed.starts_with("##") {
            i += 1;
            // Section body: parse key/values until next ## or EOF
            while i < lines.len() {
                let t = lines[i].trim();
                if t.starts_with("##") {
                    break;
                }
                if t.is_empty() || t.starts_with('#') {
                    i += 1;
                    continue;
                }
                let (key, val, next) = parse_entry(&lines, i, 0)?;
                // Merge maps for known section keys that appear once.
                insert_merge(&mut root, key, val);
                i = next;
            }
            continue;
        }
        // Top-level entry outside sections (rare)
        let (key, val, next) = parse_entry(&lines, i, 0)?;
        insert_merge(&mut root, key, val);
        i = next;
    }

    Ok(Value::Map(root))
}

fn insert_merge(map: &mut BTreeMap<String, Value>, key: String, val: Value) {
    match (map.get_mut(&key), val) {
        (Some(Value::Map(existing)), Value::Map(incoming)) => {
            for (k, v) in incoming {
                existing.insert(k, v);
            }
        }
        (_, v) => {
            map.insert(key, v);
        }
    }
}

fn parse_entry(lines: &[&str], i: usize, indent: usize) -> Result<(String, Value, usize), String> {
    let line = lines.get(i).ok_or_else(|| "unexpected EOF".to_string())?;
    let current_indent = leading_spaces(line);
    if current_indent < indent {
        return Err(format!("indent underflow at line {}", i + 1));
    }
    let trimmed = line.trim();

    // Table rows header: name[N|]{cols}:
    if let Some((name, cols)) = parse_table_header(trimmed) {
        let (rows, next) = parse_table_rows(lines, i + 1, cols, current_indent)?;
        return Ok((name, Value::Array(rows), next));
    }

    // List header: key[N]:
    if let Some(key) = parse_list_header(trimmed) {
        let (items, next) = parse_dash_list(lines, i + 1, current_indent)?;
        return Ok((key, Value::Array(items), next));
    }

    // key: value  OR  key:
    let (key, rest) = split_key_value(trimmed).ok_or_else(|| {
        format!("expected key: value at line {}: {trimmed}", i + 1)
    })?;

    if rest.is_empty() {
        // Nested map or following dash-list without [N]
        // Peek next non-empty
        let mut j = i + 1;
        while j < lines.len() && (lines[j].trim().is_empty() || lines[j].trim().starts_with('#')) {
            j += 1;
        }
        if j < lines.len() {
            let next_indent = leading_spaces(lines[j]);
            if next_indent > current_indent && lines[j].trim().starts_with('-') {
                let (items, next) = parse_dash_list(lines, i + 1, current_indent)?;
                return Ok((key, Value::Array(items), next));
            }
            if next_indent > current_indent {
                let (child, next) = parse_map_block(lines, i + 1, current_indent)?;
                return Ok((key, Value::Map(child), next));
            }
        }
        return Ok((key, Value::Map(BTreeMap::new()), i + 1));
    }

    // Inline map: {x: 0, y: 20}
    if rest.starts_with('{') && rest.ends_with('}') {
        return Ok((key, parse_inline_map(&rest)?, i + 1));
    }

    Ok((key, parse_scalar(&rest)?, i + 1))
}

fn parse_map_block(
    lines: &[&str],
    mut i: usize,
    parent_indent: usize,
) -> Result<(BTreeMap<String, Value>, usize), String> {
    let mut map = BTreeMap::new();
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }
        if trimmed.starts_with("##") {
            break;
        }
        if trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        let ind = leading_spaces(line);
        // Nested map children must be indented strictly deeper than the parent key.
        if ind <= parent_indent {
            break;
        }
        let (key, val, next) = parse_entry(lines, i, ind)?;
        if next == i {
            break;
        }
        map.insert(key, val);
        i = next;
    }
    Ok((map, i))
}

fn parse_dash_list(
    lines: &[&str],
    mut i: usize,
    parent_indent: usize,
) -> Result<(Vec<Value>, usize), String> {
    let mut items = Vec::new();
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }
        if trimmed.starts_with('#') && !trimmed.starts_with("-") {
            i += 1;
            continue;
        }
        let ind = leading_spaces(line);
        if ind <= parent_indent {
            break;
        }
        if !trimmed.starts_with('-') {
            break;
        }
        let rest = trimmed[1..].trim();
        items.push(parse_scalar(rest)?);
        i += 1;
    }
    Ok((items, i))
}

fn parse_table_header(trimmed: &str) -> Option<(String, Vec<String>)> {
    // name[N|]{a|b|c}:
    let re_start = trimmed.find('[')?;
    let re_brace = trimmed.find('{')?;
    let re_end = trimmed.find("}:")?;
    if !trimmed[re_start..].contains("|]") {
        return None;
    }
    let name = trimmed[..re_start].trim().to_string();
    let cols: Vec<String> = trimmed[re_brace + 1..re_end]
        .split('|')
        .map(|s| s.trim().to_string())
        .collect();
    Some((name, cols))
}

fn parse_list_header(trimmed: &str) -> Option<String> {
    // key[N]:  but not key[N|]{
    if trimmed.contains("|]") {
        return None;
    }
    let open = trimmed.find('[')?;
    let close = trimmed.find("]:")?;
    if open > close {
        return None;
    }
    let inside = &trimmed[open + 1..close];
    if !inside.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(trimmed[..open].trim().to_string())
}

fn parse_table_rows(
    lines: &[&str],
    mut i: usize,
    cols: Vec<String>,
    parent_indent: usize,
) -> Result<(Vec<Value>, usize), String> {
    let mut rows = Vec::new();
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }
        if trimmed.starts_with("##") {
            break;
        }
        if trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        let ind = leading_spaces(line);
        if ind <= parent_indent && !trimmed.contains('|') {
            break;
        }
        // Row lines are indented and contain |
        if !trimmed.contains('|') {
            break;
        }
        let parts = split_table_row(trimmed);
        let mut map = BTreeMap::new();
        for (idx, col) in cols.iter().enumerate() {
            let raw = parts.get(idx).map(|s| s.as_str()).unwrap_or("null");
            let mut val = parse_scalar(raw)?;
            if col == "roles" {
                if let Value::String(s) = &val {
                    if s.contains(';') {
                        val = Value::Array(
                            s.split(';')
                                .map(|p| p.trim())
                                .filter(|p| !p.is_empty() && *p != "null")
                                .map(|p| Value::String(p.to_string()))
                                .collect(),
                        );
                    } else if s == "null" {
                        val = Value::Array(vec![]);
                    } else {
                        val = Value::Array(vec![Value::String(s.clone())]);
                    }
                } else if matches!(val, Value::Null) {
                    val = Value::Array(vec![]);
                }
            }
            // Skip null fields to keep TOML tidy
            if !matches!(val, Value::Null) {
                map.insert(camel_to_snake(col), val);
            }
        }
        rows.push(Value::Map(map));
        i += 1;
    }
    Ok((rows, i))
}

fn split_table_row(trimmed: &str) -> Vec<String> {
    // Split on | but respect double-quoted segments.
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for ch in trimmed.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                cur.push(ch);
            }
            '|' if !in_quotes => {
                parts.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    parts.push(cur.trim().to_string());
    parts
}

fn split_key_value(trimmed: &str) -> Option<(String, String)> {
    let mut in_quotes = false;
    for (i, ch) in trimmed.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ':' if !in_quotes => {
                let key = trimmed[..i].trim().to_string();
                let rest = trimmed[i + 1..].trim().to_string();
                if key.is_empty() {
                    return None;
                }
                return Some((key, rest));
            }
            _ => {}
        }
    }
    None
}

fn parse_inline_map(s: &str) -> Result<Value, String> {
    let inner = s.trim();
    let inner = inner
        .strip_prefix('{')
        .and_then(|x| x.strip_suffix('}'))
        .ok_or_else(|| format!("bad inline map: {s}"))?;
    let mut map = BTreeMap::new();
    if inner.trim().is_empty() {
        return Ok(Value::Map(map));
    }
    // split on commas at top level
    for part in split_top_level(inner, ',') {
        let (k, v) = split_key_value(part.trim())
            .ok_or_else(|| format!("bad inline entry: {part}"))?;
        map.insert(camel_to_snake(&k), parse_scalar(v.trim())?);
    }
    Ok(Value::Map(map))
}

fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut depth = 0;
    let mut in_quotes = false;
    for (i, ch) in s.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '{' if !in_quotes => depth += 1,
            '}' if !in_quotes => depth -= 1,
            c if c == sep && !in_quotes && depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn parse_scalar(raw: &str) -> Result<Value, String> {
    let s = raw.trim();
    if s.is_empty() || s == "null" {
        return Ok(Value::Null);
    }
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        let inner = &s[1..s.len() - 1];
        return Ok(Value::String(inner.to_string()));
    }
    match s {
        "true" | "t" => return Ok(Value::Bool(true)),
        "false" | "f" => return Ok(Value::Bool(false)),
        _ => {}
    }
    if let Ok(i) = s.parse::<i64>() {
        return Ok(Value::Integer(i));
    }
    if let Ok(f) = s.parse::<f64>() {
        if s.contains('.') || s.contains('e') || s.contains('E') {
            return Ok(Value::Float(f));
        }
    }
    // Bareword / unquoted string (may be array literal like [1, 2])
    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len() - 1];
        if inner.trim().is_empty() {
            return Ok(Value::Array(vec![]));
        }
        let mut items = Vec::new();
        for part in split_top_level(inner, ',') {
            items.push(parse_scalar(part.trim())?);
        }
        return Ok(Value::Array(items));
    }
    Ok(Value::String(s.to_string()))
}

fn leading_spaces(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
}

// --- TOML emit --------------------------------------------------------------

fn emit_toml(root: &Value) -> Result<String, String> {
    let Value::Map(map) = root else {
        return Err("root must be a map".into());
    };
    let mut out = String::new();
    // Emit in a stable, readable order.
    let order = [
        "metadata",
        "identification",
        "specs",
        "dims",
        "params",
        "power_pins",
        "gpio",
        "peripherals",
        "usb",
        "ports",
        "pneumatic_ports",
        "hydraulic_ports",
        "sensor_slots",
        "protocol_ports",
        "simulation",
        "limits",
        "warnings",
        "restrictions",
        "compatibility",
        "connections",
        "canvas",
        "dendri",
    ];
    let mut emitted = std::collections::HashSet::new();
    for key in order {
        if let Some(val) = map.get(key) {
            emit_key(&mut out, key, val, "")?;
            emitted.insert(key.to_string());
        }
    }
    for (key, val) in map {
        if emitted.contains(key) {
            continue;
        }
        emit_key(&mut out, key, val, "")?;
    }
    Ok(out)
}

fn emit_key(out: &mut String, key: &str, val: &Value, prefix: &str) -> Result<(), String> {
    match val {
        Value::Map(m) => {
            // Detect array-of-tables keys that should use [[key]] — handled by Array arm.
            // Nested param maps etc.: use [prefix.key] dotted tables.
            let table_name = if prefix.is_empty() {
                key.to_string()
            } else {
                format!("{prefix}.{key}")
            };
            // Special-case canvas.port_* nested as [canvas.port_X]
            // Flatten one level for simple leaf maps under known parents.
            if is_leaf_scalar_map(m) && !prefix.is_empty() {
                out.push_str(&format!("\n[{table_name}]\n"));
                for (k, v) in m {
                    emit_assignment(out, k, v)?;
                }
            } else if prefix.is_empty()
                && matches!(
                    key,
                    "metadata"
                        | "identification"
                        | "specs"
                        | "dims"
                        | "usb"
                        | "simulation"
                        | "limits"
                        | "restrictions"
                        | "compatibility"
                        | "connections"
                        | "dendri"
                )
            {
                out.push_str(&format!("\n[{key}]\n"));
                for (k, v) in m {
                    match v {
                        Value::Map(_) | Value::Array(_) => emit_key(out, k, v, key)?,
                        _ => emit_assignment(out, k, v)?,
                    }
                }
            } else if key == "params" && prefix.is_empty() {
                for (pk, pv) in m {
                    match pv {
                        Value::Map(pm) => {
                            out.push_str(&format!("\n[params.{pk}]\n"));
                            for (k, v) in pm {
                                emit_assignment(out, k, v)?;
                            }
                        }
                        _ => {
                            out.push_str("\n[params]\n");
                            emit_assignment(out, pk, pv)?;
                        }
                    }
                }
            } else if key == "canvas" && prefix.is_empty() {
                out.push_str("\n[canvas]\n");
                for (k, v) in m {
                    match v {
                        Value::Map(pm) => {
                            // port anchors → [canvas.port_X]
                            out.push_str(&format!("\n[canvas.{k}]\n"));
                            for (ck, cv) in pm {
                                emit_assignment(out, ck, cv)?;
                            }
                        }
                        _ => emit_assignment(out, k, v)?,
                    }
                }
            } else {
                out.push_str(&format!("\n[{table_name}]\n"));
                for (k, v) in m {
                    match v {
                        Value::Map(_) => emit_key(out, k, v, &table_name)?,
                        Value::Array(arr) if is_array_of_maps(arr) => {
                            emit_key(out, k, v, &table_name)?;
                        }
                        Value::Array(_) => emit_assignment(out, k, v)?,
                        _ => emit_assignment(out, k, v)?,
                    }
                }
            }
        }
        Value::Array(items) if is_array_of_maps(items) => {
            let table = if prefix.is_empty() {
                key.to_string()
            } else {
                format!("{prefix}.{key}")
            };
            for item in items {
                let Value::Map(m) = item else { continue };
                out.push_str(&format!("\n[[{table}]]\n"));
                for (k, v) in m {
                    match v {
                        Value::Map(_) => emit_key(out, k, v, &table)?,
                        _ => emit_assignment(out, k, v)?,
                    }
                }
            }
        }
        _ => {
            if prefix.is_empty() {
                // Top-level scalar/array (e.g. warnings)
                // Put under a synthetic section if needed — warnings often under restrictions in boards.
                emit_assignment(out, key, val)?;
            } else {
                emit_assignment(out, key, val)?;
            }
        }
    }
    Ok(())
}

fn is_leaf_scalar_map(m: &BTreeMap<String, Value>) -> bool {
    m.values()
        .all(|v| matches!(v, Value::Null | Value::Bool(_) | Value::Integer(_) | Value::Float(_) | Value::String(_)))
}

fn is_array_of_maps(items: &[Value]) -> bool {
    !items.is_empty() && items.iter().all(|v| matches!(v, Value::Map(_)))
}

fn emit_assignment(out: &mut String, key: &str, val: &Value) -> Result<(), String> {
    match val {
        Value::Null => {} // omit
        Value::Bool(b) => out.push_str(&format!("{key} = {b}\n")),
        Value::Integer(i) => out.push_str(&format!("{key} = {i}\n")),
        Value::Float(f) => {
            let s = if f.fract() == 0.0 {
                format!("{f:.1}")
            } else {
                f.to_string()
            };
            out.push_str(&format!("{key} = {s}\n"));
        }
        Value::String(s) => out.push_str(&format!("{key} = {}\n", toml_quote(s))),
        Value::Array(items) => {
            out.push_str(&format!("{key} = "));
            emit_inline_array(out, items)?;
            out.push('\n');
        }
        Value::Map(_) => return Err(format!("cannot assign map inline for key {key}")),
    }
    Ok(())
}

fn emit_inline_array(out: &mut String, items: &[Value]) -> Result<(), String> {
    out.push('[');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        match item {
            Value::Null => out.push_str("null"), // toml 0.8 may not like this; skip ideally
            Value::Bool(b) => out.push_str(&b.to_string()),
            Value::Integer(n) => out.push_str(&n.to_string()),
            Value::Float(f) => out.push_str(&f.to_string()),
            Value::String(s) => out.push_str(&toml_quote(s)),
            Value::Array(inner) => emit_inline_array(out, inner)?,
            Value::Map(_) => return Err("nested map in inline array".into()),
        }
    }
    out.push(']');
    Ok(())
}

fn toml_quote(s: &str) -> String {
    // Use basic strings with escaping.
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_led_smoke() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let path = repo.join("core/components/led.toon");
        let (id, toml_text) = convert_toon_file(&path, ProfileFamily::Components).unwrap();
        assert_eq!(id, "led");
        assert!(toml_text.contains("[identification]"), "got:\n{toml_text}");
        assert!(toml_text.contains("id = \"led\""));
        assert!(toml_text.contains("svg_path = \"content/assets/svg/components/led.svg\""));
        let _: toml::Value = toml::from_str(&toml_text).expect("valid toml");
    }

    #[test]
    fn converts_uno_board_smoke() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let path = repo.join("core/boards/arduino-uno-r3.toon");
        let (id, toml_text) = convert_toon_file(&path, ProfileFamily::Boards).unwrap();
        assert_eq!(id, "arduino-uno-r3");
        assert!(toml_text.contains("[identification]"), "got:\n{toml_text}");
        assert!(toml_text.contains("[[gpio]]"));
        assert!(toml_text.contains("[[power_pins]]"));
        let parsed: toml::Value = toml::from_str(&toml_text).expect("valid toml");
        let gpio = parsed.get("gpio").unwrap().as_array().unwrap();
        assert!(gpio.len() >= 20);
    }

    #[test]
    fn converts_screen_smoke() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let path = repo.join("core/screens/ind-hmi-numeric-indicator.toon");
        let (id, toml_text) = convert_toon_file(&path, ProfileFamily::Screens).unwrap();
        assert_eq!(id, "ind-hmi-numeric-indicator");
        let _: toml::Value = toml::from_str(&toml_text).expect("valid toml");
    }
}
