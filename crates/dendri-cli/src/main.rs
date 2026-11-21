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
    Rust,
    Ladder,
    Sfc,
}

impl From<Lang> for SourceLanguage {
    fn from(l: Lang) -> Self {
        match l {
            Lang::Cpp => SourceLanguage::Cpp,
            Lang::Micropython => SourceLanguage::Micropython,
            Lang::St => SourceLanguage::St,
            Lang::Rust => SourceLanguage::Rust,
            Lang::Ladder => SourceLanguage::Ladder,
            Lang::Sfc => SourceLanguage::Sfc,
        }
    }
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Semantic validate a program or project `.toml` file
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
    /// Emit Dendri HAL Rust (P2) from the same AST
    EmitRust { path: PathBuf },
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
    /// Export generated backends from ASL into `build/` (or `--out`)
    Export {
        path: PathBuf,
        /// Output directory (default: `<project-dir>/build` or `./build`)
        #[arg(long)]
        out: Option<PathBuf>,
        /// Also print project TOML to stdout after ensuring visual ASL blocks
        #[arg(long)]
        print_project: bool,
    },
    /// Deprecated alias for `export` (no longer embeds sources in the project file)
    SyncSources {
        path: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Bidirectional sync smoke: load project, project ASL↔ladder/sfc blocks
    SyncVisual { path: PathBuf },
    /// Show document / profile info
    Info { path: PathBuf },
    /// Load visual-MVP profiles (board + LED + button) and report SVG hooks
    Profiles {
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Validate every migrated profile under content/profiles/
        #[arg(long)]
        all: bool,
    },
    /// Migrate legacy core/**/*.toon profiles → content/profiles/**/*.toml (DEN-14)
    MigrateProfiles {
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Only migrate one family: boards | components | screens
        #[arg(long)]
        family: Option<String>,
    },
    /// Migrate legacy core/asl/v010/**/*.toon → content/fixtures/**/*.toml (DEN-20)
    MigrateAsl {
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Print destinations without writing (skips hand-protected fixtures)
        #[arg(long)]
        dry_run: bool,
        /// Substring filter against the source path (repeatable)
        #[arg(long)]
        only: Vec<String>,
    },
    /// Headless SVG canvas sim: button sequence → visual-frame snapshot (CI)
    CanvasSim {
        path: PathBuf,
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Comma-separated button levels applied before each scan (e.g. 0,1,0)
        #[arg(long, default_value = "0,1,0")]
        sequence: String,
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
        Cmd::EmitRust { path } => {
            let doc = parse_path(&path).map_err(|e| e.to_string())?;
            print!("{}", dendri_frontends::emit_rust(doc.program()));
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
        Cmd::SyncSources { path, out } => {
            eprintln!("note: sync-sources is deprecated; use `dendri export` (writes build/, no embedded sources)");
            run_export(&path, out.as_deref(), false)
        }
        Cmd::Export {
            path,
            out,
            print_project,
        } => run_export(&path, out.as_deref(), print_project),
        Cmd::SyncVisual { path } => {
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let mut session =
                dendri_sync::SyncSession::from_project_toml(&text).map_err(|e| e.to_string())?;
            session
                .sync_visual_sources_from_asl()
                .map_err(|e| e.to_string())?;
            // Ephemeral ladder/sfc texts (not stored in project TOML) round-trip via ASL blocks.
            let ladder = session
                .source_text(SourceLanguage::Ladder)
                .unwrap_or("")
                .to_string();
            if !ladder.trim().is_empty() {
                session
                    .apply_source_toml(dendri_sync::VisualKind::Ladder, &ladder)
                    .map_err(|e| e.to_string())?;
                session.assert_ladder_roundtrip().map_err(|e| e.to_string())?;
            }
            let sfc = session
                .source_text(SourceLanguage::Sfc)
                .unwrap_or("")
                .to_string();
            if !sfc.trim().is_empty() {
                session
                    .apply_source_toml(dendri_sync::VisualKind::Sfc, &sfc)
                    .map_err(|e| e.to_string())?;
                session.assert_sfc_roundtrip().map_err(|e| e.to_string())?;
            }
            println!("ok: visual sync ladder/sfc ↔ ASL (export-only texts)");
            Ok(())
        }
        Cmd::Info { path } => {
            let doc = parse_path(&path).map_err(|e| e.to_string())?;
            match doc {
                ParsedDocument::Program(p) => {
                    println!("kind: program (.toml)");
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
                    println!("kind: project (.toml)");
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
                    println!(
                        "asl.project_name: {}",
                        p.project
                            .asl
                            .metadata
                            .project_name
                            .as_deref()
                            .unwrap_or("<missing>")
                    );
                    println!("asl.hardware: {}", p.project.asl.hardware.len());
                    println!("asl.ladder: {}", p.project.asl.ladder.len());
                    println!("asl.grafcet: {}", p.project.asl.grafcet.len());
                }
            }
            Ok(())
        }
        Cmd::Profiles { repo, all } => {
            let root = repo.unwrap_or_else(|| discover_repo_root());
            let store = dendri_profiles::ProfileStore::from_repo_root(&root);
            if all {
                let report = store.validate_all().map_err(|e| e.to_string())?;
                println!("profiles_loaded: {}", report.loaded);
                println!("svg_present: {}", report.svg_present);
                println!("svg_missing: {}", report.svg_missing);
                if !report.errors.is_empty() {
                    for e in &report.errors {
                        eprintln!("error: {e}");
                    }
                    return Err(format!(
                        "{} profile(s) failed to load",
                        report.errors.len()
                    ));
                }
                println!(
                    "boards: {}",
                    store.list_ids("boards").map_err(|e| e.to_string())?.len()
                );
                println!(
                    "components: {}",
                    store
                        .list_ids("components")
                        .map_err(|e| e.to_string())?
                        .len()
                );
                println!(
                    "screens: {}",
                    store.list_ids("screens").map_err(|e| e.to_string())?.len()
                );
            }
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
            let report = dendri_canvas::report_mvp_assets(&root).map_err(|e| e.to_string())?;
            println!(
                "resolved_board: {}",
                report.resolved_board.display()
            );
            println!("resolved_led: {}", report.resolved_led.display());
            println!("resolved_button: {}", report.resolved_button.display());
            println!(
                "content_assets_preferred: {}",
                report.preferred_paths_present
            );
            Ok(())
        }
        Cmd::MigrateAsl {
            repo,
            dry_run,
            only,
        } => {
            let root = repo.unwrap_or_else(|| discover_repo_root());
            let opts = dendri_ast::MigrateOptions { dry_run, only };
            let report =
                dendri_ast::migrate_v010(&root, &opts).map_err(|e| e.to_string())?;
            for item in &report.items {
                if item.protected {
                    continue;
                }
                let rel = item
                    .destination
                    .strip_prefix(&root)
                    .unwrap_or(&item.destination);
                let tag = if dry_run { "DRY " } else { "WRITE" };
                println!("{tag} {}", rel.display());
            }
            println!("converted={}", report.converted);
            println!("skipped_protected={}", report.skipped_protected);
            Ok(())
        }
        Cmd::MigrateProfiles { repo, family } => {
            let root = repo.unwrap_or_else(|| discover_repo_root());
            let families: Vec<dendri_profiles::ProfileFamily> = match family.as_deref() {
                None => vec![
                    dendri_profiles::ProfileFamily::Boards,
                    dendri_profiles::ProfileFamily::Components,
                    dendri_profiles::ProfileFamily::Screens,
                ],
                Some("boards") => vec![dendri_profiles::ProfileFamily::Boards],
                Some("components") => vec![dendri_profiles::ProfileFamily::Components],
                Some("screens") => vec![dendri_profiles::ProfileFamily::Screens],
                Some(other) => {
                    return Err(format!(
                        "unknown family '{other}' (expected boards|components|screens)"
                    ));
                }
            };
            let mut total = 0usize;
            let mut svg_ok = 0usize;
            let mut svg_copied = 0usize;
            for fam in families {
                let reports =
                    dendri_profiles::migrate_family(&root, fam).map_err(|e| e.to_string())?;
                println!(
                    "migrated_{}: {}",
                    fam.as_str(),
                    reports.len()
                );
                for r in &reports {
                    total += 1;
                    if r.svg_present {
                        svg_ok += 1;
                    }
                    if r.svg_copied {
                        svg_copied += 1;
                    }
                }
            }
            println!("migrated_total: {total}");
            println!("svg_present: {svg_ok}");
            println!("svg_copied: {svg_copied}");
            println!("svg_missing: {}", total - svg_ok);
            Ok(())
        }
        Cmd::CanvasSim {
            path,
            repo,
            sequence,
        } => {
            let root = repo.unwrap_or_else(|| discover_repo_root());
            let doc = parse_path(&path).map_err(|e| e.to_string())?;
            let project = match doc {
                ParsedDocument::Project(p) => p.project,
                ParsedDocument::Program(p) => wrap_program_as_mvp_project(p.program),
            };
            let cfg = SimConfig::default();
            let mut canvas =
                dendri_canvas::SimCanvas::from_project(&project, &root, &cfg)
                    .map_err(|e| e.to_string())?;
            canvas.boot().map_err(|e| e.to_string())?;
            for token in sequence.split(',') {
                let pressed = match token.trim() {
                    "1" | "true" | "pressed" | "on" => true,
                    "0" | "false" | "released" | "off" => false,
                    other => return Err(format!("bad sequence token: {other}")),
                };
                canvas.set_button(pressed);
                canvas.step().map_err(|e| e.to_string())?;
            }
            print!("{}", canvas.frames_snapshot());
            Ok(())
        }
    }
}

fn run_export(path: &Path, out: Option<&Path>, print_project: bool) -> Result<(), String> {
    let doc = parse_path(path).map_err(|e| e.to_string())?;
    let program = doc.program().clone();
    let mut project = match doc {
        ParsedDocument::Project(p) => p.project,
        ParsedDocument::Program(p) => wrap_program_as_mvp_project(p.program),
    };
    dendri_sync::fill_visual_sources(&mut project);

    let out_dir = out
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            path.parent()
                .unwrap_or_else(|| Path::new("."))
                .join("build")
        });
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;

    let langs = [
        SourceLanguage::Cpp,
        SourceLanguage::Micropython,
        SourceLanguage::St,
        SourceLanguage::Rust,
        SourceLanguage::Ladder,
        SourceLanguage::Sfc,
    ];
    for lang in langs {
        let text = dendri_frontends::emit_source(lang, &program).map_err(|e| e.to_string())?;
        let hint = PathBuf::from(lang.build_path_hint());
        // build_path_hint is `build/<backend>/file` — strip leading `build/`
        let rel = hint
            .strip_prefix("build")
            .unwrap_or(hint.as_path());
        let dest = out_dir.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&dest, text).map_err(|e| e.to_string())?;
        println!("wrote {}", dest.display());
    }

    if print_project {
        let toml = dendri_emit::emit_project(&project).map_err(|e| e.to_string())?;
        print!("{toml}");
    }
    Ok(())
}

fn wrap_program_as_mvp_project(program: dendri_ast::Program) -> dendri_ast::SimulationProject {
    dendri_ast::SimulationProject {
        metadata: program.metadata.clone(),
        board: dendri_ast::BoardInstance {
            profile: "arduino-uno-r3".into(),
            instance_id: "board".into(),
            layout: Default::default(),
        },
        components: vec![
            dendri_ast::ComponentInstance {
                instance_id: "led1".into(),
                profile: "led".into(),
                label: None,
                params: Default::default(),
                layout: Default::default(),
            },
            dendri_ast::ComponentInstance {
                instance_id: "btn_start".into(),
                profile: "cmd-pushbutton-no-start".into(),
                label: None,
                params: Default::default(),
                layout: Default::default(),
            },
        ],
        wiring: vec![
            dendri_ast::Wire {
                from: "board.D13".into(),
                to: "led1.A".into(),
                signal: Some("digital".into()),
            },
            dendri_ast::Wire {
                from: "board.GND".into(),
                to: "led1.K".into(),
                signal: Some("gnd".into()),
            },
            dendri_ast::Wire {
                from: "board.D2".into(),
                to: "btn_start.NO".into(),
                signal: Some("digital".into()),
            },
            dendri_ast::Wire {
                from: "board.GND".into(),
                to: "btn_start.COM".into(),
                signal: Some("gnd".into()),
            },
        ],
        sources: vec![],
        asl: program,
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
