//! Project open / save / export helpers for the iced editor MVP.
//!
//! Wires existing `dendri-parse` / `dendri-emit` / `dendri-frontends` crates —
//! same paths as `dendri export`. No fake compile/upload success.

use std::path::{Path, PathBuf};

use dendri_ast::{SimulationProject, SourceLanguage};
use dendri_frontends::emit_source;
use dendri_parse::{parse_path, ParsedDocument};

/// Live Code-tab projections generated from the loaded project's ASL.
#[derive(Debug, Clone, Default)]
pub struct Projections {
    pub cpp: String,
    pub st: String,
    pub python: String,
    pub toml: String,
}

impl Projections {
    pub fn from_project(project: &SimulationProject) -> Self {
        let program = &project.asl;
        let cpp = emit_source(SourceLanguage::Cpp, program)
            .unwrap_or_else(|e| format!("// emit cpp failed: {e}\n"));
        let st = emit_source(SourceLanguage::St, program)
            .unwrap_or_else(|e| format!("(* emit st failed: {e} *)\n"));
        let python = emit_source(SourceLanguage::Micropython, program)
            .unwrap_or_else(|e| format!("# emit micropython failed: {e}\n"));
        let toml = dendri_emit::emit_project(project)
            .unwrap_or_else(|e| format!("# emit project toml failed: {e}\n"));
        Self {
            cpp,
            st,
            python,
            toml,
        }
    }
}

pub fn load_project(path: &Path) -> Result<SimulationProject, String> {
    match parse_path(path).map_err(|e| e.to_string())? {
        ParsedDocument::Project(p) => Ok(p.project),
        _ => Err("expected project document (.toml with board + asl)".into()),
    }
}

/// Persist the project TOML to `path` (sole source of truth on disk).
pub fn save_project(path: &Path, project: &SimulationProject) -> Result<(), String> {
    let text = dendri_emit::emit_project(project).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, text).map_err(|e| e.to_string())
}

/// Export generated backends into `out_dir` (same layout as `dendri export`).
/// Also writes `project.toml` into that directory.
pub fn export_project(project: &SimulationProject, out_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut project = project.clone();
    dendri_sync::fill_visual_sources(&mut project);
    let program = project.asl.clone();

    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let mut written = Vec::new();

    let langs = [
        SourceLanguage::Cpp,
        SourceLanguage::Micropython,
        SourceLanguage::St,
        SourceLanguage::Rust,
        SourceLanguage::Ladder,
        SourceLanguage::Sfc,
    ];
    for lang in langs {
        let text = emit_source(lang, &program).map_err(|e| e.to_string())?;
        let hint = PathBuf::from(lang.build_path_hint());
        let rel = hint.strip_prefix("build").unwrap_or(hint.as_path());
        let dest = out_dir.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&dest, text).map_err(|e| e.to_string())?;
        written.push(dest);
    }

    let project_toml = out_dir.join("project.toml");
    let text = dendri_emit::emit_project(&project).map_err(|e| e.to_string())?;
    std::fs::write(&project_toml, text).map_err(|e| e.to_string())?;
    written.push(project_toml);

    Ok(written)
}

pub fn default_export_dir(repo_root: &Path) -> PathBuf {
    repo_root.join("build")
}

pub fn project_display_name(project: &SimulationProject, path: &Path) -> String {
    project
        .metadata
        .project_name
        .clone()
        .or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "untitled".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c01() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/fixtures/canvas/C01-button-led.toml")
    }

    #[test]
    fn load_c01_and_project_projections() {
        let project = load_project(&c01()).expect("load C01");
        assert_eq!(
            project.metadata.project_name.as_deref(),
            Some("C01-button-led")
        );
        let proj = Projections::from_project(&project);
        assert!(proj.cpp.contains("pinMode") || proj.cpp.contains("btnPin"));
        assert!(proj.st.to_uppercase().contains("PRESSED") || proj.st.contains("pressed"));
        assert!(proj.toml.contains("C01-button-led"));
    }

    #[test]
    fn export_writes_build_layout() {
        let project = load_project(&c01()).expect("load C01");
        let dir = tempfile_dir();
        let written = export_project(&project, &dir).expect("export");
        assert!(written.iter().any(|p| p.ends_with("arduino/main.cpp")));
        assert!(written.iter().any(|p| p.ends_with("project.toml")));
        assert!(dir.join("project.toml").is_file());
        assert!(dir.join("arduino/main.cpp").is_file());
        assert!(dir.join("st/main.st").is_file());
    }

    #[test]
    fn save_roundtrips_project_name() {
        let project = load_project(&c01()).expect("load C01");
        let dir = tempfile_dir();
        let path = dir.join("saved.toml");
        save_project(&path, &project).expect("save");
        let again = load_project(&path).expect("reload");
        assert_eq!(again.metadata.project_name, project.metadata.project_name);
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dendri-editor-io-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
