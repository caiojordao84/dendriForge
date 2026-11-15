//! `dendri` — Phase 1 CLI for validate / roundtrip / sim / text + visual frontends.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use dendri_ast::SourceLanguage;
use dendri_parse::{parse_path, ParsedDocument};
use dendri_sim::{SimConfig, Simulator};
use dendri_validate::{validate_error_fixture, validate_program, validate_project};

#[derive(Parser, Debug)]
#[command(name = "dendri", about = "DendriForge ASL tooling (Phase 1)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Lang {
    Cpp,
    Micropython,
    St,
    Ladder,
    Sfc,
}

impl From<Lang> for SourceLanguage {
    fn from(l: Lang) -> Self {
        match l {
            Lang::Cpp => SourceLanguage::Cpp,
            Lang::Micropython => SourceLanguage::Micropython,
            Lang::St => SourceLanguage::St,
            Lang::Ladder => SourceLanguage::Ladder,
            Lang::Sfc => SourceLanguage::Sfc,
        }
    }
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Semantic validate an `.asl.toml` or `.dendri.toml` file
    Validate {
        path: PathBuf,
        /// Treat as error-corpus fixture (apply fixture_inject)
        #[arg(long)]
        error_fixture: bool,
    },
    /// Parse → emit TOML → parse and assert AST equality
    Roundtrip { path: PathBuf },
    /// Run deterministic scan-cycle simulation and print trace
    Sim {
        path: PathBuf,
        #[arg(long, default_value_t = 4)]
        cycles: u64,
    },
    /// Emit IEC ST from the same AST
    EmitSt { path: PathBuf },
    /// Emit Arduino-style C++ from the same AST
    EmitCpp { path: PathBuf },
    /// Emit MicroPython from the same AST
    EmitMicropython { path: PathBuf },
    /// Emit Ladder diagram TOML from ASL
    EmitLadder { path: PathBuf },
    /// Emit SFC/GRAFCET diagram TOML from ASL
    EmitSfc { path: PathBuf },
    /// Parse a P1 source file into ASL TOML on stdout
    ParseSource {
        path: PathBuf,
        #[arg(long, value_enum)]
        lang: Lang,
    },
    /// Emit → parse for a P1 language and assert equality
    FrontendRoundtrip {
        path: PathBuf,
        #[arg(long, value_enum)]
        lang: Lang,
    },
    /// Fill empty `[[sources]].text` in a `.dendri.toml` from embedded ASL (stdout)
    SyncSources { path: PathBuf },
    /// Bidirectional sync smoke: load project, project ASL↔ladder/sfc, re-emit
    SyncVisual { path: PathBuf },
    /// Show document / profile info
    Info { path: PathBuf },
    /// Load visual-MVP profiles (board + LED + button) and report SVG hooks
    Profiles {
        #[arg(long)]
        repo: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.cmd {
        Cmd::Validate { path, error_fixture } => {
            if error_fixture {
                let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
                let result = validate_error_fixture(&text).map_err(|e| e.to_string())?;
                print_validation(&path, &result);
                if result.ok() {
                    return Err("expected error fixture to fail validation".into());
                }
                let expect = dendri_parse::parse_program_str(&text)
                    .map_err(|e| e.to_string())?
                    .program
                    .metadata
                    .expect_error;
                if let Some(exp) = expect {
                    let codes: Vec<_> = result
                        .diagnostics
                        .iter()
                        .map(|d| d.code.as_str().to_string())
                        .collect();
                    if !codes.iter().any(|c| c == &exp) {
                        return Err(format!("expected {exp}, got {codes:?}"));
                    }
                    println!("ok: matched {exp}");
                }
                return Ok(());
            }

            let doc = parse_path(&path).map_err(|e| e.to_string())?;
            let result = match &doc {
                ParsedDocument::Program(p) => validate_program(&p.program, &p.inject),
                ParsedDocument::Project(p) => validate_project(&p.project),
            };
            print_validation(&path, &result);
            if !result.ok() {
                return Err("validation failed".into());
            }
            println!("ok");
            Ok(())
        }
        Cmd::Roundtrip { path } => {
            let doc = parse_path(&path).map_err(|e| e.to_string())?;
            match doc {
                ParsedDocument::Program(p) => {
                    dendri_emit::roundtrip_program(&p.program)?;
                }
                ParsedDocument::Project(p) => {
                    dendri_emit::roundtrip_project(&p.project)?;
                    dendri_emit::roundtrip_program(&p.project.asl)?;
                }
            }
            println!("ok: round-trip AST equality");
            Ok(())
        }
        Cmd::Sim { path, cycles } => {
            let doc = parse_path(&path).map_err(|e| e.to_string())?;
            let cfg = SimConfig {
                max_cycles: cycles,
                ..SimConfig::default()
            };
            let mut sim = match doc {
                ParsedDocument::Program(p) => Simulator::from_program(p.program, &cfg),
                ParsedDocument::Project(p) => Simulator::from_project(&p.project, &cfg),
            };
            sim.run(cycles).map_err(|e| e.to_string())?;
            print!("{}", sim.trace_text());
            Ok(())
        }
        Cmd::EmitSt { path } => {
            let doc = parse_path(&path).map_err(|e| e.to_string())?;
            print!("{}", dendri_frontends::emit_st(doc.program()));
            Ok(())
        }
        Cmd::EmitCpp { path } => {
            let doc = parse_path(&path).map_err(|e| e.to_string())?;
            print!("{}", dendri_frontends::emit_cpp(doc.program()));
            Ok(())
        }
        Cmd::EmitMicropython { path } => {
            let doc = parse_path(&path).map_err(|e| e.to_string())?;
            print!("{}", dendri_frontends::emit_micropython(doc.program()));
            Ok(())
        }
        Cmd::EmitLadder { path } => {
            let doc = parse_path(&path).map_err(|e| e.to_string())?;
            print!("{}", dendri_frontends::emit_ladder(doc.program()));
            Ok(())
        }
        Cmd::EmitSfc { path } => {
            let doc = parse_path(&path).map_err(|e| e.to_string())?;
            print!("{}", dendri_frontends::emit_sfc(doc.program()));
            Ok(())
        }
        Cmd::ParseSource { path, lang } => {
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let program =
                dendri_frontends::parse_source(lang.into(), &text).map_err(|e| e.to_string())?;
            let toml = dendri_emit::emit_program(&program).map_err(|e| e.to_string())?;
            print!("{toml}");
            Ok(())
        }
        Cmd::FrontendRoundtrip { path, lang } => {
            let doc = parse_path(&path).map_err(|e| e.to_string())?;
            dendri_frontends::roundtrip_language(lang.into(), doc.program())
                .map_err(|e| e.to_string())?;
            println!(
                "ok: {} frontend round-trip",
                SourceLanguage::from(lang).as_str()
            );
            Ok(())
        }
        Cmd::SyncSources { path } => {
            let doc = parse_path(&path).map_err(|e| e.to_string())?;
            let ParsedDocument::Project(mut p) = doc else {
                return Err("sync-sources requires a .dendri.toml project".into());
            };
            for src in &mut p.project.sources {
                if !src.text.trim().is_empty() {
                    continue;
                }
                match src.language {
                    SourceLanguage::Cpp
                    | SourceLanguage::Micropython
                    | SourceLanguage::St
                    | SourceLanguage::Ladder
                    | SourceLanguage::Sfc => {
                        src.text = dendri_frontends::emit_source(src.language, &p.project.asl)
                            .map_err(|e| e.to_string())?;
                    }
                    _ => {}
                }
            }
            dendri_sync::fill_visual_sources(&mut p.project);
            let out = dendri_emit::emit_project(&p.project).map_err(|e| e.to_string())?;
            print!("{out}");
            Ok(())
        }
        Cmd::SyncVisual { path } => {
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let mut session =
                dendri_sync::SyncSession::from_project_toml(&text).map_err(|e| e.to_string())?;
            session
                .sync_visual_sources_from_asl()
                .map_err(|e| e.to_string())?;
            // Round-trip ladder source if present
            if let Some(t) = session.source_text(SourceLanguage::Ladder) {
                let t = t.to_string();
                if !t.trim().is_empty() {
                    session
                        .apply_source_toml(dendri_sync::VisualKind::Ladder, &t)
                        .map_err(|e| e.to_string())?;
                    session.assert_ladder_roundtrip().map_err(|e| e.to_string())?;
                }
            }
            if let Some(t) = session.source_text(SourceLanguage::Sfc) {
                let t = t.to_string();
                if !t.trim().is_empty() {
                    session
                        .apply_source_toml(dendri_sync::VisualKind::Sfc, &t)
                        .map_err(|e| e.to_string())?;
                    session.assert_sfc_roundtrip().map_err(|e| e.to_string())?;
                }
            }
            println!("ok: visual sync ladder/sfc ↔ TOML ↔ ASL");
            Ok(())
        }
        Cmd::Info { path } => {
            let doc = parse_path(&path).map_err(|e| e.to_string())?;
            match doc {
                ParsedDocument::Program(p) => {
                    println!("kind: program (.asl.toml)");
                    println!(
                        "project_name: {}",
                        p.program
                            .metadata
                            .project_name
                            .as_deref()
                            .unwrap_or("<missing>")
                    );
                    println!("hardware: {}", p.program.hardware.len());
                    println!("data: {}", p.program.data.len());
                    println!("setup: {}", p.program.setup.len());
                    println!("loop: {}", p.program.loop_body.len());
                    println!("functions: {}", p.program.functions.len());
                    println!("ladder: {}", p.program.ladder.len());
                    println!("grafcet: {}", p.program.grafcet.len());
                }
                ParsedDocument::Project(p) => {
                    println!("kind: project (.dendri.toml)");
                    println!(
                        "project_name: {}",
                        p.project
                            .metadata
                            .project_name
                            .as_deref()
                            .unwrap_or("<missing>")
                    );
                    println!("board: {}", p.project.board.profile);
                    println!("components: {}", p.project.components.len());
                    println!("wiring: {}", p.project.wiring.len());
                    println!("sources: {}", p.project.sources.len());
                    for s in &p.project.sources {
                        println!(
                            "  source language={} name={:?} bytes={}",
                            s.language.as_str(),
                            s.name,
                            s.text.len()
                        );
                    }
                    println!(
                        "asl.project_name: {}",
                        p.project
                            .asl
                            .metadata
                            .project_name
                            .as_deref()
                            .unwrap_or("<missing>")
                    );
                    println!("asl.ladder: {}", p.project.asl.ladder.len());
                    println!("asl.grafcet: {}", p.project.asl.grafcet.len());
                }
            }
            Ok(())
        }
        Cmd::Profiles { repo } => {
            let root = repo.unwrap_or_else(|| discover_repo_root());
            let store = dendri_profiles::ProfileStore::from_repo_root(&root);
            let mvp = store.load_visual_mvp().map_err(|e| e.to_string())?;
            println!("board: {}", mvp.board.identification.id);
            println!("led: {}", mvp.led.identification.id);
            println!("button: {}", mvp.button.identification.id);
            for (label, hook) in [
                ("board_svg", &mvp.board_hook),
                ("led_svg", &mvp.led_hook),
                ("button_svg", &mvp.button_hook),
            ] {
                if let Some(h) = hook {
                    println!(
                        "{label}: path={} present={} ids={:?}",
                        h.relative_path, h.present, h.required_ids
                    );
                }
            }
            Ok(())
        }
    }
}

fn print_validation(path: &Path, result: &dendri_validate::ValidationResult) {
    println!("file: {}", path.display());
    for d in &result.diagnostics {
        println!("  {} — {}", d.code.as_str(), d.message);
    }
}

fn discover_repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
