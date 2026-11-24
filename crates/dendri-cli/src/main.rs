//! `dendri` command-line tool (governance, project validation, and verification gates).

use clap::{Parser, Subcommand};
use dendri_assets::{AssetCatalog, AssetCheckError, approve_asset_dir, validate_assets_dir};
use dendri_circuit::SpiceCircuitBackend;
use dendri_cli::frontend::{
    CliBoardProfile, CliLanguage, CliStDialect, FrontendCliError, build_board_context,
    run_roundtrip, run_translate,
};
use dendri_cli::{
    bench_spice, ledger, packs, provenance, resolve_assets_dir, resolve_workspace_root,
};
use dendri_project::load_project_path;
use dendri_sim_kernel::{InputTimeline, KernelConfig, SimKernel};
use dendri_span::{Severity, SourceMap};
use dendri_stdlib::StandardsRegistry;
use dendri_validate::{MemoryProfileCatalog, ProfileCatalog, ValidateOptions, validate_project};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(
    name = "dendri",
    version,
    about = "dendriForge v2 engineering CLI — verification, governance, and simulation"
)]
struct Cli {
    /// Override workspace root directory (otherwise DENDRIFORGE_ROOT or CWD ancestry).
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    /// Override assets directory (otherwise DENDRIFORGE_ASSETS, <exe_dir>/assets, or <root>/assets).
    #[arg(long, global = true)]
    assets_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Enforce docs/deferrals.toml against docs/milestone.toml and scan for stubs or untracked markers (CI Gate 12).
    #[command(name = "ledger-check")]
    Ledger,
    /// Enforce [provenance] metadata, THIRD_PARTY_NOTICES, and ML-restriction policy (CI Gate 5).
    #[command(name = "provenance-check")]
    Provenance,
    /// Validate assets/standards.toml via dendri-stdlib (CI Gate 5).
    #[command(name = "standards-check")]
    Standards,
    /// Validate hash-locked Wave 1 packs (`assets/packs/*.pack.toml` vs `checksums.toml`, DEF-008).
    #[command(name = "packs-check")]
    PacksCheck,
    /// Run headless SPICE-class analytical physics benchmarks (`< 0.1%` error, CI Gate 6).
    #[command(name = "bench-spice")]
    BenchSpice,
    /// Translate a Wave 1 source file across C++, IEC ST, Ladder, and GRAFCET/SFC via ASL IR.
    Translate {
        /// Path to the input source file.
        file: PathBuf,
        /// Source language (`cpp | st | ladder | sfc`).
        #[arg(long)]
        from: CliLanguage,
        /// Target language (`cpp | st | ladder | sfc`).
        #[arg(long)]
        to: CliLanguage,
        /// Target board profile (defaults to `arduino-uno-r3` for C++, `plc-1200-open-1214` for IEC).
        #[arg(long)]
        board: Option<CliBoardProfile>,
        /// IEC 61131-3 ST comment dialect (`DEF-006`).
        #[arg(long, default_value = "iec-ed3")]
        st_dialect: CliStDialect,
        /// Optional output file path (prints to stdout when omitted).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Permit cross-language export of `Foreign` constructs with `NOT TRANSLATED` markers.
        #[arg(long)]
        allow_foreign: bool,
    },
    /// Verify `LOSSLESS_ASL`, `C1`/`C5` comment conservation, and zero magic comments for a source file.
    Roundtrip {
        /// Path to the input source file.
        file: PathBuf,
        /// Source language (`cpp | st | ladder | sfc`).
        #[arg(long)]
        lang: CliLanguage,
        /// Target board profile (defaults to `arduino-uno-r3` for C++, `plc-1200-open-1214` for IEC).
        #[arg(long)]
        board: Option<CliBoardProfile>,
        /// IEC 61131-3 ST comment dialect (`DEF-006`).
        #[arg(long, default_value = "iec-ed3")]
        st_dialect: CliStDialect,
    },
    /// Run all 13 asset contract rules across assets/{boards,components} (CI Gate 5).
    #[command(name = "asset-check")]
    AssetCheck,
    /// Explicitly sign off an inspected asset directory, writing canonical render SHA-256 hashes to review.toml.
    #[command(name = "asset-approve")]
    AssetApprove {
        /// Path to the asset directory (e.g. `assets/components/led`).
        dir: PathBuf,
        /// Human reviewer identifier recorded in `review.toml`.
        #[arg(long, default_value = "caio")]
        reviewer: String,
        /// Review date in ISO YYYY-MM-DD format.
        #[arg(long, default_value = "2026-11-23")]
        date: String,
        /// Claim formal standards conformity (must remain false unless backed by a licensed audit).
        #[arg(long, default_value_t = false)]
        conformity_claimed: bool,
    },
    /// Parse and statically validate a v2 single-file `.toml` project (`dendri-project` + `dendri-validate`).
    Validate {
        /// Path to the `.toml` project file.
        file: PathBuf,
        /// Print summary of boards, comments, and active assumptions (blocks release mode if assumptions exist).
        #[arg(long)]
        report: bool,
        /// Permit cross-language export of Foreign constructs.
        #[arg(long)]
        allow_foreign: bool,
    },
    /// Run a headless deterministic multi-board co-simulation (`dendri-sim-kernel` + `dendri-vm` + optional SPICE MNA).
    Sim {
        /// Path to the `.toml` project file.
        file: PathBuf,
        /// Virtual simulation duration in milliseconds.
        #[arg(long, default_value_t = 100)]
        duration_ms: u64,
        /// Deterministic replay seed.
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// Use the full SPICE-class MNA electrical solver (`dendri-circuit`) instead of digital wire fabric.
        #[arg(long)]
        spice: bool,
        /// Permit cross-language export of Foreign constructs during pre-run validation.
        #[arg(long)]
        allow_foreign: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let root = resolve_workspace_root(cli.root.as_deref());
    let assets_dir = resolve_assets_dir(cli.assets_dir.as_deref(), &root);

    match cli.command {
        Commands::Ledger => match ledger::run_ledger_check(&root) {
            Ok(report) => {
                println!(
                    "ledger-check OK | milestone={} | open_deferrals={} | rust_files={} | foreign_nodes={} | active_assumptions={}",
                    report.current_milestone,
                    report.open_deferrals,
                    report.scanned_rust_files,
                    report.foreign_node_count,
                    report.active_assumption_count,
                );
                ExitCode::SUCCESS
            }
            Err(ledger::LedgerError::Violations(vs)) => {
                for v in &vs {
                    eprintln!("ledger-check ERROR: {v}");
                }
                eprintln!("ledger-check FAILED with {} violation(s).", vs.len());
                ExitCode::FAILURE
            }
            Err(err) => {
                eprintln!("ledger-check ERROR: {err}");
                ExitCode::FAILURE
            }
        },
        Commands::Provenance => match provenance::run_provenance_check(&root, &assets_dir) {
            Ok(report) => {
                println!(
                    "provenance-check OK | checked_profiles={} | third_party={} | ml_restricted={}",
                    report.checked_profiles,
                    report.third_party_profiles,
                    report.ml_restricted_profiles,
                );
                ExitCode::SUCCESS
            }
            Err(provenance::ProvenanceError::Violations(vs)) => {
                for v in &vs {
                    eprintln!("provenance-check ERROR: {v}");
                }
                eprintln!("provenance-check FAILED with {} violation(s).", vs.len());
                ExitCode::FAILURE
            }
            Err(err) => {
                eprintln!("provenance-check ERROR: {err}");
                ExitCode::FAILURE
            }
        },
        Commands::Standards => {
            let standards_path = assets_dir.join("standards.toml");
            match StandardsRegistry::load_from_path(&standards_path) {
                Ok(reg) => {
                    println!(
                        "standards-check OK | file={} | standards={}",
                        standards_path.display(),
                        reg.len()
                    );
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("standards-check ERROR: {err}");
                    ExitCode::FAILURE
                }
            }
        }
        Commands::PacksCheck => match packs::run_packs_check(&assets_dir) {
            Ok(report) => {
                println!(
                    "packs-check OK | packs={} | symbols={} | ids={}",
                    report.packs_verified,
                    report.total_symbols,
                    report.pack_ids.join(","),
                );
                ExitCode::SUCCESS
            }
            Err(packs::PacksCheckError::Violations(vs)) => {
                for v in &vs {
                    eprintln!("packs-check ERROR: {v}");
                }
                eprintln!("packs-check FAILED with {} violation(s).", vs.len());
                ExitCode::FAILURE
            }
            Err(err) => {
                eprintln!("packs-check ERROR: {err}");
                ExitCode::FAILURE
            }
        },
        Commands::BenchSpice => {
            let report = bench_spice::run_bench_spice();
            for r in &report.results {
                let status = if r.passed { "PASS" } else { "FAIL" };
                println!(
                    "  [{status}] {:<36} sim={:.6} {} | exact={:.6} {} | rel_err={:.6}%",
                    r.name,
                    r.simulated,
                    r.unit,
                    r.analytical,
                    r.unit,
                    r.rel_error * 100.0,
                );
            }
            if report.all_passed {
                println!(
                    "bench-spice OK | benchmarks={} | max_rel_error={:.6}% (< 0.1%)",
                    report.results.len(),
                    report.max_rel_error * 100.0,
                );
                ExitCode::SUCCESS
            } else {
                eprintln!(
                    "bench-spice FAILED | max_rel_error={:.6}% (limit 0.1%)",
                    report.max_rel_error * 100.0,
                );
                ExitCode::FAILURE
            }
        }
        Commands::Translate {
            file,
            from,
            to,
            board,
            st_dialect,
            out,
            allow_foreign,
        } => {
            let src = match std::fs::read_to_string(&file) {
                Ok(s) => s,
                Err(err) => {
                    eprintln!("translate ERROR reading `{}`: {err}", file.display());
                    return ExitCode::FAILURE;
                }
            };
            let mut sources = SourceMap::new();
            let fid = sources.add_file(file.display().to_string(), src.clone());
            let profile = board.unwrap_or_else(|| from.default_board_profile());
            let ctx = build_board_context(profile, st_dialect, allow_foreign, Some(&assets_dir));
            match run_translate(&src, from, to, &ctx) {
                Ok(res) => {
                    for mut diag in res.diagnostics {
                        if diag.file_id.is_none() {
                            diag.file_id = Some(fid);
                        }
                        eprintln!("{}", diag.render(Some(&sources)));
                    }
                    if let Some(out_path) = out {
                        if let Err(err) = std::fs::write(&out_path, &res.emitted) {
                            eprintln!("translate ERROR writing `{}`: {err}", out_path.display());
                            return ExitCode::FAILURE;
                        }
                        println!(
                            "translate OK | from={} | to={} | out={} | pous={} | comments={}",
                            from.to_language_id(),
                            to.to_language_id(),
                            out_path.display(),
                            res.pou_count,
                            res.comment_count,
                        );
                    } else {
                        print!("{}", res.emitted);
                    }
                    ExitCode::SUCCESS
                }
                Err(FrontendCliError::Diagnostics(diags)) => {
                    for mut diag in diags {
                        if diag.file_id.is_none() {
                            diag.file_id = Some(fid);
                        }
                        eprintln!("{}", diag.render(Some(&sources)));
                    }
                    ExitCode::FAILURE
                }
                Err(FrontendCliError::Conformance(msg)) => {
                    eprintln!("translate ERROR: {msg}");
                    ExitCode::FAILURE
                }
            }
        }
        Commands::Roundtrip {
            file,
            lang,
            board,
            st_dialect,
        } => {
            let src = match std::fs::read_to_string(&file) {
                Ok(s) => s,
                Err(err) => {
                    eprintln!("roundtrip ERROR reading `{}`: {err}", file.display());
                    return ExitCode::FAILURE;
                }
            };
            let mut sources = SourceMap::new();
            let fid = sources.add_file(file.display().to_string(), src.clone());
            let profile = board.unwrap_or_else(|| lang.default_board_profile());
            let ctx = build_board_context(profile, st_dialect, false, Some(&assets_dir));
            match run_roundtrip(&src, lang, &ctx) {
                Ok(report) => {
                    println!(
                        "roundtrip OK | lang={} | board={} | pous={} | comments={} | foreign={} | bytes={}",
                        report.language,
                        report.board_profile,
                        report.pou_count,
                        report.comment_count,
                        report.foreign_count,
                        report.emitted_bytes,
                    );
                    ExitCode::SUCCESS
                }
                Err(FrontendCliError::Diagnostics(diags)) => {
                    for mut diag in diags {
                        if diag.file_id.is_none() {
                            diag.file_id = Some(fid);
                        }
                        eprintln!("{}", diag.render(Some(&sources)));
                    }
                    ExitCode::FAILURE
                }
                Err(FrontendCliError::Conformance(msg)) => {
                    eprintln!("roundtrip ERROR: {msg}");
                    ExitCode::FAILURE
                }
            }
        }
        Commands::AssetCheck => match validate_assets_dir(&root, &assets_dir) {
            Ok(report) => {
                println!(
                    "asset-check OK | boards={} | components={} | standards={} | third_party={} | ml_restricted={}",
                    report.boards_checked,
                    report.components_checked,
                    report.standards_loaded,
                    report.third_party_assets.len(),
                    report.ml_restricted_assets.len(),
                );
                ExitCode::SUCCESS
            }
            Err(AssetCheckError::Violations(vs)) => {
                for v in &vs {
                    eprintln!("asset-check ERROR: {v}");
                }
                eprintln!("asset-check FAILED with {} violation(s).", vs.len());
                ExitCode::FAILURE
            }
            Err(err) => {
                eprintln!("asset-check ERROR: {err}");
                ExitCode::FAILURE
            }
        },
        Commands::AssetApprove {
            dir,
            reviewer,
            date,
            conformity_claimed,
        } => match approve_asset_dir(&dir, &reviewer, &date, conformity_claimed) {
            Ok(review) => {
                println!(
                    "asset-approve OK | dir={} | reviewer={} | date={} | physical_sha256={} | schematic_sha256={}",
                    dir.display(),
                    review.reviewer,
                    review.reviewed_on,
                    review.physical_render_sha256,
                    review
                        .schematic_render_sha256
                        .as_deref()
                        .unwrap_or("<none>"),
                );
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("asset-approve ERROR: {err}");
                ExitCode::FAILURE
            }
        },
        Commands::Validate {
            file,
            report,
            allow_foreign,
        } => {
            let mut sources = SourceMap::new();
            let (fid, proj) = match load_project_path(&file, &mut sources) {
                Ok(res) => res,
                Err(err) => {
                    if let Some(diag) = err.as_diagnostic() {
                        eprintln!("{}", diag.render(Some(&sources)));
                    } else {
                        eprintln!("validate ERROR: {err}");
                    }
                    return ExitCode::FAILURE;
                }
            };
            let catalog = load_live_or_reference_catalog(&root, &assets_dir);
            let opts = ValidateOptions {
                file_id: Some(fid),
                allow_foreign_export: allow_foreign,
                ..ValidateOptions::default()
            };
            let bag = validate_project(&proj, catalog.as_ref(), &opts);
            for diag in bag.all() {
                eprintln!("{}", diag.render(Some(&sources)));
            }
            if report {
                let total_comments: usize = proj.comments.len()
                    + proj.boards.iter().map(|b| b.comments.len()).sum::<usize>();
                let total_assumptions: usize =
                    proj.boards.iter().map(|b| b.assumptions.len()).sum();
                println!(
                    "validate report | project={} | boards={} | components={} | wires={} | comments={} | assumptions={} | errors={} | warnings={}",
                    proj.meta.name,
                    proj.boards.len(),
                    proj.components.len(),
                    proj.wires.len(),
                    total_comments,
                    total_assumptions,
                    bag.count_by_severity(Severity::Error),
                    bag.count_by_severity(Severity::Warning),
                );
            }
            if bag.has_errors() {
                ExitCode::FAILURE
            } else {
                println!("validate OK | file={}", file.display());
                ExitCode::SUCCESS
            }
        }
        Commands::Sim {
            file,
            duration_ms,
            seed,
            spice,
            allow_foreign,
        } => {
            let mut sources = SourceMap::new();
            let (fid, proj) = match load_project_path(&file, &mut sources) {
                Ok(res) => res,
                Err(err) => {
                    if let Some(diag) = err.as_diagnostic() {
                        eprintln!("{}", diag.render(Some(&sources)));
                    } else {
                        eprintln!("sim ERROR: {err}");
                    }
                    return ExitCode::FAILURE;
                }
            };
            let catalog = load_live_or_reference_catalog(&root, &assets_dir);
            let opts = ValidateOptions {
                file_id: Some(fid),
                allow_foreign_export: allow_foreign,
                ..ValidateOptions::default()
            };
            let bag = validate_project(&proj, catalog.as_ref(), &opts);
            for diag in bag.all() {
                eprintln!("{}", diag.render(Some(&sources)));
            }
            if bag.has_errors() {
                return ExitCode::FAILURE;
            }

            let cfg = KernelConfig {
                seed,
                ..KernelConfig::default()
            };
            let mut kernel = if spice {
                let backend = match SpiceCircuitBackend::from_project_headless(&proj) {
                    Ok(b) => b,
                    Err(err) => {
                        eprintln!("sim SPICE ERROR: {err}");
                        return ExitCode::FAILURE;
                    }
                };
                match SimKernel::with_backend(&proj, cfg, InputTimeline::new(), Box::new(backend)) {
                    Ok(k) => k,
                    Err(err) => {
                        eprintln!("sim ERROR: {err}");
                        return ExitCode::FAILURE;
                    }
                }
            } else {
                match SimKernel::new(&proj, cfg, InputTimeline::new()) {
                    Ok(k) => k,
                    Err(err) => {
                        eprintln!("sim ERROR: {err}");
                        return ExitCode::FAILURE;
                    }
                }
            };
            let target_us = duration_ms.saturating_mul(1_000);
            if let Err(err) = kernel.run_until(target_us) {
                eprintln!("sim RUNTIME ERROR: {err}");
                return ExitCode::FAILURE;
            }
            let snap = kernel.snapshot();
            println!(
                "sim OK | mode={} | project={} | time_us={} | boards={} | trace_events={}",
                if spice { "spice-mna" } else { "digital-wire" },
                proj.meta.name,
                snap.time_us,
                snap.boards.len(),
                snap.global_trace_len,
            );
            for (bid, bsnap) in &snap.boards {
                println!(
                    "  board={} | time_us={} | cycles={} | digital_out={:?} | analog_out={:?} | pwm_out={:?}",
                    bid,
                    bsnap.time_us,
                    bsnap.fiber_cycles,
                    bsnap.digital_outputs,
                    bsnap.analog_outputs,
                    bsnap.pwm_outputs,
                );
            }
            ExitCode::SUCCESS
        }
    }
}

fn load_live_or_reference_catalog(root: &Path, assets_dir: &Path) -> Box<dyn ProfileCatalog> {
    if let Ok(live) = AssetCatalog::load_validated(root, assets_dir)
        && !live.boards.is_empty()
    {
        return Box::new(live);
    }
    Box::new(MemoryProfileCatalog::builtin_reference())
}
