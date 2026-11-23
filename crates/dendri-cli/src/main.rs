//! `dendri` command-line tool (governance, project validation, and verification gates).

use clap::{Parser, Subcommand};
use dendri_assets::{AssetCatalog, AssetCheckError, approve_asset_dir, validate_assets_dir};
use dendri_circuit::SpiceCircuitBackend;
use dendri_cli::{ledger, provenance, resolve_assets_dir, resolve_workspace_root};
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
