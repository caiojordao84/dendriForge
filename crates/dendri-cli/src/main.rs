//! `dendri` command-line tool (Phase 0 governance & verification gates).

use clap::{Parser, Subcommand};
use dendri_cli::{ledger, provenance, resolve_assets_dir, resolve_workspace_root};
use dendri_stdlib::StandardsRegistry;
use std::path::PathBuf;
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
    }
}
