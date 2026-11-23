//! Deferral ledger & stub enforcement gate (`dendri ledger-check`, CI Gate 12).
//!
//! Enforces `05-ide-and-roadmap.md` §5.2:
//! - Closed, totally ordered [`Milestone`] enum (`wave-2-gate < phase-9`).
//! - Strict `#[serde(deny_unknown_fields)]` parsing of `docs/milestone.toml`
//!   and `docs/deferrals.toml`.
//! - Fails on expired deadlines (`deadline_milestone < current_milestone`),
//!   empty or broken `refs`, duplicate/malformed `DEF-nnn` IDs, or any
//!   `todo!()`, `unimplemented!()`, `#[ignore]`, or untracked deferred-work
//!   comment marker in `crates/` without a registered `DEF-nnn` ID.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Closed, totally ordered development milestone enum (`05-ide-and-roadmap.md` §5.2).
///
/// Variant declaration order defines the exact total order (`#[derive(PartialOrd, Ord)]`).
/// Critically, `Wave2Gate < Phase9` so Wave 2 prerequisites (`DEF-004`) close
/// strictly before Phase 9 begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Milestone {
    #[serde(rename = "phase-0")]
    Phase0,
    #[serde(rename = "phase-1")]
    Phase1,
    #[serde(rename = "phase-2")]
    Phase2,
    #[serde(rename = "phase-3")]
    Phase3,
    #[serde(rename = "phase-4")]
    Phase4,
    #[serde(rename = "phase-5")]
    Phase5,
    #[serde(rename = "phase-6")]
    Phase6,
    #[serde(rename = "phase-6b")]
    Phase6b,
    #[serde(rename = "wave-1-exit")]
    Wave1Exit,
    #[serde(rename = "phase-7")]
    Phase7,
    #[serde(rename = "phase-8")]
    Phase8,
    #[serde(rename = "wave-2-gate")]
    Wave2Gate,
    #[serde(rename = "phase-9")]
    Phase9,
    #[serde(rename = "wave-3-exit")]
    Wave3Exit,
    #[serde(rename = "post-wave-3")]
    PostWave3,
}

impl Milestone {
    /// Canonical kebab-case identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Phase0 => "phase-0",
            Self::Phase1 => "phase-1",
            Self::Phase2 => "phase-2",
            Self::Phase3 => "phase-3",
            Self::Phase4 => "phase-4",
            Self::Phase5 => "phase-5",
            Self::Phase6 => "phase-6",
            Self::Phase6b => "phase-6b",
            Self::Wave1Exit => "wave-1-exit",
            Self::Phase7 => "phase-7",
            Self::Phase8 => "phase-8",
            Self::Wave2Gate => "wave-2-gate",
            Self::Phase9 => "phase-9",
            Self::Wave3Exit => "wave-3-exit",
            Self::PostWave3 => "post-wave-3",
        }
    }
}

impl fmt::Display for Milestone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Strict schema of `docs/milestone.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MilestoneFile {
    pub current: Milestone,
}

/// Single entry inside `docs/deferrals.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeferralEntry {
    pub id: String,
    pub title: String,
    pub owner_milestone: Milestone,
    pub deadline_milestone: Milestone,
    pub reason: String,
    pub refs: Vec<String>,
}

/// Strict schema of `docs/deferrals.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeferralLedger {
    pub schema_version: String,
    #[serde(default)]
    pub deferral: Vec<DeferralEntry>,
}

/// Summary report emitted on a clean `dendri ledger-check` run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerReport {
    pub current_milestone: Milestone,
    pub open_deferrals: usize,
    pub scanned_rust_files: usize,
    pub foreign_node_count: usize,
    pub active_assumption_count: usize,
}

/// Single ledger or source-scan violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerViolation {
    pub location: String,
    pub message: String,
}

impl fmt::Display for LedgerViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.location, self.message)
    }
}

/// Failure returned by [`run_ledger_check`].
#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("failed to read `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse milestone TOML: {0}")]
    MilestoneToml(#[source] toml::de::Error),
    #[error("failed to parse deferrals TOML: {0}")]
    DeferralsToml(#[source] toml::de::Error),
    #[error("unsupported deferrals schema_version `{0}` (expected \"2.0\")")]
    UnsupportedVersion(String),
    #[error("ledger check failed with {} violation(s)", .0.len())]
    Violations(Vec<LedgerViolation>),
}

/// Parses `docs/milestone.toml` text strictly.
pub fn parse_milestone_str(input: &str) -> Result<MilestoneFile, LedgerError> {
    toml::from_str(input).map_err(LedgerError::MilestoneToml)
}

/// Parses `docs/deferrals.toml` text strictly.
pub fn parse_deferrals_str(input: &str) -> Result<DeferralLedger, LedgerError> {
    let ledger: DeferralLedger = toml::from_str(input).map_err(LedgerError::DeferralsToml)?;
    if ledger.schema_version != "2.0" {
        return Err(LedgerError::UnsupportedVersion(ledger.schema_version));
    }
    Ok(ledger)
}

/// Validates `docs/milestone.toml`, `docs/deferrals.toml`, and scans all Rust
/// sources under `<root>/crates/` for banned stubs or untracked deferral markers.
pub fn run_ledger_check(root: &Path) -> Result<LedgerReport, LedgerError> {
    let milestone_path = root.join("docs").join("milestone.toml");
    let deferrals_path = root.join("docs").join("deferrals.toml");

    let milestone_text =
        std::fs::read_to_string(&milestone_path).map_err(|source| LedgerError::Io {
            path: milestone_path.display().to_string(),
            source,
        })?;
    let deferrals_text =
        std::fs::read_to_string(&deferrals_path).map_err(|source| LedgerError::Io {
            path: deferrals_path.display().to_string(),
            source,
        })?;

    let milestone_file = parse_milestone_str(&milestone_text)?;
    let ledger = parse_deferrals_str(&deferrals_text)?;
    let current = milestone_file.current;

    let mut violations = Vec::new();
    let mut known_ids: BTreeMap<String, &DeferralEntry> = BTreeMap::new();

    for entry in &ledger.deferral {
        if !is_valid_deferral_id(&entry.id) {
            violations.push(LedgerViolation {
                location: format!("docs/deferrals.toml ({})", entry.id),
                message: format!(
                    "invalid deferral id `{}` (expected `DEF-NNN` with 3 ASCII digits)",
                    entry.id
                ),
            });
        }
        if known_ids.insert(entry.id.clone(), entry).is_some() {
            violations.push(LedgerViolation {
                location: format!("docs/deferrals.toml ({})", entry.id),
                message: format!("duplicate deferral id `{}`", entry.id),
            });
        }
        if entry.title.trim().is_empty() || entry.reason.trim().is_empty() {
            violations.push(LedgerViolation {
                location: format!("docs/deferrals.toml ({})", entry.id),
                message: "deferral `title` and `reason` must be non-empty".to_owned(),
            });
        }
        if entry.owner_milestone > entry.deadline_milestone {
            violations.push(LedgerViolation {
                location: format!("docs/deferrals.toml ({})", entry.id),
                message: format!(
                    "owner_milestone `{}` cannot be later than deadline_milestone `{}`",
                    entry.owner_milestone, entry.deadline_milestone
                ),
            });
        }
        if entry.deadline_milestone < current {
            violations.push(LedgerViolation {
                location: format!("docs/deferrals.toml ({})", entry.id),
                message: format!(
                    "deferral `{}` expired: deadline `{}` precedes current milestone `{}`",
                    entry.id, entry.deadline_milestone, current
                ),
            });
        }
        if entry.refs.is_empty() {
            violations.push(LedgerViolation {
                location: format!("docs/deferrals.toml ({})", entry.id),
                message: "deferral MUST cite at least one specification reference in `refs`"
                    .to_owned(),
            });
        }
        for reference in &entry.refs {
            let file_part = reference.split('#').next().unwrap_or("");
            if file_part.is_empty() || !root.join(file_part).exists() {
                violations.push(LedgerViolation {
                    location: format!("docs/deferrals.toml ({})", entry.id),
                    message: format!("broken deferral reference `{reference}` (file not found)"),
                });
            }
        }
    }

    let valid_ids: BTreeSet<String> = known_ids.keys().cloned().collect();
    let crates_dir = root.join("crates");
    let mut rust_files = Vec::new();
    if crates_dir.is_dir() {
        collect_files_with_ext(&crates_dir, "rs", &mut rust_files)?;
        rust_files.sort();
    }

    for path in &rust_files {
        let content = std::fs::read_to_string(path).map_err(|source| LedgerError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string();
        scan_rust_source(&rel, &content, &valid_ids, &mut violations);
    }

    let (foreign_node_count, active_assumption_count) = count_project_annotations(root)?;

    if !violations.is_empty() {
        return Err(LedgerError::Violations(violations));
    }

    Ok(LedgerReport {
        current_milestone: current,
        open_deferrals: ledger.deferral.len(),
        scanned_rust_files: rust_files.len(),
        foreign_node_count,
        active_assumption_count,
    })
}

fn is_valid_deferral_id(id: &str) -> bool {
    let Some(digits) = id.strip_prefix("DEF-") else {
        return false;
    };
    digits.len() == 3 && digits.bytes().all(|b| b.is_ascii_digit())
}

fn collect_files_with_ext(
    dir: &Path,
    ext: &str,
    out: &mut Vec<PathBuf>,
) -> Result<(), LedgerError> {
    let entries = std::fs::read_dir(dir).map_err(|source| LedgerError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    for entry_res in entries {
        let entry = entry_res.map_err(|source| LedgerError::Io {
            path: dir.display().to_string(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_files_with_ext(&path, ext, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some(ext) {
            out.push(path);
        }
    }
    Ok(())
}

/// Scans a Rust source file lexically (distinguishing code, comments, and string literals):
/// - Outside strings/comments: forbids `todo!(`, `unimplemented!(`, and `#[ignore`.
/// - Inside comments: forbids untracked deferred-work markers without a registered `DEF-nnn`.
pub fn scan_rust_source(
    rel_path: &str,
    src: &str,
    valid_deferral_ids: &BTreeSet<String>,
    violations: &mut Vec<LedgerViolation>,
) {
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut line = 1usize;
    let mut code_line_buf = String::new();
    let mut comment_line_buf = String::new();

    let flush_line = |line_no: usize,
                      code_buf: &mut String,
                      comment_buf: &mut String,
                      violations: &mut Vec<LedgerViolation>| {
        check_code_line(rel_path, line_no, code_buf, violations);
        check_comment_line(
            rel_path,
            line_no,
            comment_buf,
            valid_deferral_ids,
            violations,
        );
        code_buf.clear();
        comment_buf.clear();
    };

    while i < len {
        let b = bytes.get(i).copied().unwrap_or(0);
        let next = bytes.get(i.saturating_add(1)).copied().unwrap_or(0);

        // Line comment: // ...
        if b == b'/' && next == b'/' {
            i = i.saturating_add(2);
            while i < len {
                let cb = bytes.get(i).copied().unwrap_or(0);
                if cb == b'\n' {
                    break;
                }
                comment_line_buf.push(char::from(cb));
                i = i.saturating_add(1);
            }
            continue;
        }

        // Block comment: /* ... */ (Rust supports nested block comments)
        if b == b'/' && next == b'*' {
            let mut depth = 1usize;
            i = i.saturating_add(2);
            while i < len && depth > 0 {
                let cb = bytes.get(i).copied().unwrap_or(0);
                let cnext = bytes.get(i.saturating_add(1)).copied().unwrap_or(0);
                if cb == b'/' && cnext == b'*' {
                    depth = depth.saturating_add(1);
                    i = i.saturating_add(2);
                } else if cb == b'*' && cnext == b'/' {
                    depth = depth.saturating_sub(1);
                    i = i.saturating_add(2);
                } else if cb == b'\n' {
                    flush_line(line, &mut code_line_buf, &mut comment_line_buf, violations);
                    line = line.saturating_add(1);
                    i = i.saturating_add(1);
                } else {
                    comment_line_buf.push(char::from(cb));
                    i = i.saturating_add(1);
                }
            }
            continue;
        }

        // Raw string literal: r"..." or r#"..."# (and br"..." / br#"..."#)
        if let Some(after_raw) = try_skip_raw_string(bytes, i, &mut line) {
            i = after_raw;
            code_line_buf.push(' ');
            continue;
        }

        // Regular string literal: "..."
        if b == b'"' {
            i = i.saturating_add(1);
            while i < len {
                let sb = bytes.get(i).copied().unwrap_or(0);
                if sb == b'\\' {
                    i = i.saturating_add(2);
                    continue;
                }
                if sb == b'\n' {
                    flush_line(line, &mut code_line_buf, &mut comment_line_buf, violations);
                    line = line.saturating_add(1);
                    i = i.saturating_add(1);
                    continue;
                }
                i = i.saturating_add(1);
                if sb == b'"' {
                    break;
                }
            }
            code_line_buf.push(' ');
            continue;
        }

        // Character literal: 'a', '\'', '\n' (avoid confusing lifetimes like `'a`)
        if b == b'\''
            && let Some(after_char) = try_skip_char_literal(bytes, i)
        {
            i = after_char;
            code_line_buf.push(' ');
            continue;
        }

        if b == b'\n' {
            flush_line(line, &mut code_line_buf, &mut comment_line_buf, violations);
            line = line.saturating_add(1);
            i = i.saturating_add(1);
            continue;
        }

        code_line_buf.push(char::from(b));
        i = i.saturating_add(1);
    }

    flush_line(line, &mut code_line_buf, &mut comment_line_buf, violations);
}

fn try_skip_raw_string(bytes: &[u8], start: usize, line: &mut usize) -> Option<usize> {
    let mut pos = start;
    if bytes.get(pos) == Some(&b'b') {
        pos = pos.saturating_add(1);
    }
    if bytes.get(pos) != Some(&b'r') {
        return None;
    }
    // Must not be part of a longer identifier (e.g. `err"`)
    if start > 0
        && let Some(&prev) = bytes.get(start.saturating_sub(1))
        && (prev.is_ascii_alphanumeric() || prev == b'_')
    {
        return None;
    }
    pos = pos.saturating_add(1);
    let mut hashes = 0usize;
    while bytes.get(pos) == Some(&b'#') {
        hashes = hashes.saturating_add(1);
        pos = pos.saturating_add(1);
    }
    if bytes.get(pos) != Some(&b'"') {
        return None;
    }
    pos = pos.saturating_add(1);
    while pos < bytes.len() {
        let cur = bytes.get(pos).copied().unwrap_or(0);
        if cur == b'\n' {
            *line = line.saturating_add(1);
            pos = pos.saturating_add(1);
            continue;
        }
        if cur == b'"' {
            let mut matched = 0usize;
            while matched < hashes
                && bytes.get(pos.saturating_add(1).saturating_add(matched)) == Some(&b'#')
            {
                matched = matched.saturating_add(1);
            }
            if matched == hashes {
                return Some(pos.saturating_add(1).saturating_add(hashes));
            }
        }
        pos = pos.saturating_add(1);
    }
    Some(bytes.len())
}

fn try_skip_char_literal(bytes: &[u8], start: usize) -> Option<usize> {
    let next = bytes.get(start.saturating_add(1)).copied()?;
    if next == b'\\' {
        // Escaped char literal like `'\''`, `'\n'`, `'\x7f'`, `'\u{1234}'`
        let mut pos = start.saturating_add(2);
        let limit = start.saturating_add(12).min(bytes.len());
        while pos < limit {
            let b = bytes.get(pos).copied().unwrap_or(0);
            pos = pos.saturating_add(1);
            if b == b'\'' {
                return Some(pos);
            }
            if b == b'\n' {
                return None;
            }
        }
        None
    } else if bytes.get(start.saturating_add(2)) == Some(&b'\'') {
        Some(start.saturating_add(3))
    } else {
        None
    }
}

fn check_code_line(
    rel_path: &str,
    line_no: usize,
    code: &str,
    violations: &mut Vec<LedgerViolation>,
) {
    for (needle, label) in [
        ("todo!", "`todo!()` stub macro"),
        ("unimplemented!", "`unimplemented!()` stub macro"),
    ] {
        if contains_word_prefix(code, needle) {
            violations.push(LedgerViolation {
                location: format!("{rel_path}:{line_no}"),
                message: format!("forbidden {label} in workspace source"),
            });
        }
    }
    if code.contains("#[ignore") {
        violations.push(LedgerViolation {
            location: format!("{rel_path}:{line_no}"),
            message: "forbidden `#[ignore]` attribute in workspace source".to_owned(),
        });
    }
}

fn check_comment_line(
    rel_path: &str,
    line_no: usize,
    comment: &str,
    valid_deferral_ids: &BTreeSet<String>,
    violations: &mut Vec<LedgerViolation>,
) {
    let has_marker = contains_standalone_token(comment, "TODO")
        || contains_standalone_token(comment, "FIXME")
        || contains_standalone_token(comment, "XXX");
    if !has_marker {
        return;
    }

    let cited_ids = extract_deferral_ids(comment);
    if cited_ids.is_empty() {
        violations.push(LedgerViolation {
            location: format!("{rel_path}:{line_no}"),
            message: "TODO/FIXME/XXX comment MUST reference a registered `DEF-nnn` entry from docs/deferrals.toml".to_owned(),
        });
        return;
    }

    for id in cited_ids {
        if !valid_deferral_ids.contains(&id) {
            violations.push(LedgerViolation {
                location: format!("{rel_path}:{line_no}"),
                message: format!(
                    "comment references unknown deferral `{id}` not present in docs/deferrals.toml"
                ),
            });
        }
    }
}

fn contains_word_prefix(haystack: &str, needle: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut search_from = 0usize;
    while let Some(rel_idx) = haystack
        .get(search_from..)
        .and_then(|rest| rest.find(needle))
    {
        let idx = search_from.saturating_add(rel_idx);
        let prev_ok = idx == 0
            || bytes
                .get(idx.saturating_sub(1))
                .is_none_or(|&b| !b.is_ascii_alphanumeric() && b != b'_');
        if prev_ok {
            return true;
        }
        search_from = idx.saturating_add(needle.len());
    }
    false
}

fn contains_standalone_token(haystack: &str, token: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut search_from = 0usize;
    while let Some(rel_idx) = haystack
        .get(search_from..)
        .and_then(|rest| rest.find(token))
    {
        let idx = search_from.saturating_add(rel_idx);
        let end = idx.saturating_add(token.len());
        let prev_ok = idx == 0
            || bytes
                .get(idx.saturating_sub(1))
                .is_none_or(|&b| !b.is_ascii_alphanumeric() && b != b'_');
        let next_ok = bytes
            .get(end)
            .is_none_or(|&b| !b.is_ascii_alphanumeric() && b != b'_');
        if prev_ok && next_ok {
            return true;
        }
        search_from = end;
    }
    false
}

fn extract_deferral_ids(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut ids = Vec::new();
    let mut i = 0usize;
    while i.saturating_add(7) <= bytes.len() {
        if bytes.get(i..i.saturating_add(4)) == Some(b"DEF-") {
            let digits = bytes.get(i.saturating_add(4)..i.saturating_add(7));
            let next_ok = bytes
                .get(i.saturating_add(7))
                .is_none_or(|&b| !b.is_ascii_alphanumeric());
            if let Some(d) = digits
                && d.iter().all(u8::is_ascii_digit)
                && next_ok
                && let Some(id_str) = text.get(i..i.saturating_add(7))
            {
                ids.push(id_str.to_owned());
                i = i.saturating_add(7);
                continue;
            }
        }
        i = i.saturating_add(1);
    }
    ids
}

fn count_project_annotations(root: &Path) -> Result<(usize, usize), LedgerError> {
    let scenarios_dir = root.join("scenarios");
    if !scenarios_dir.is_dir() {
        return Ok((0, 0));
    }
    let mut tomls = Vec::new();
    collect_files_with_ext(&scenarios_dir, "toml", &mut tomls)?;
    let mut foreign_count = 0usize;
    let mut assumption_count = 0usize;
    for path in tomls {
        let text = std::fs::read_to_string(&path).map_err(|source| LedgerError::Io {
            path: path.display().to_string(),
            source,
        })?;
        foreign_count = foreign_count
            .saturating_add(text.matches("kind = \"foreign\"").count())
            .saturating_add(text.matches("FOREIGN(").count());
        assumption_count =
            assumption_count.saturating_add(text.matches("[[boards.assumptions]]").count());
    }
    Ok((foreign_count, assumption_count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_temp_repo(milestone: &str, deferrals: &str, rs_code: &str) -> TempDir {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("docs/architecture")).unwrap();
        std::fs::create_dir_all(dir.path().join("crates/sample/src")).unwrap();
        std::fs::write(dir.path().join("docs/architecture/01.md"), "# spec\n").unwrap();
        std::fs::write(dir.path().join("docs/milestone.toml"), milestone).unwrap();
        std::fs::write(dir.path().join("docs/deferrals.toml"), deferrals).unwrap();
        std::fs::write(dir.path().join("crates/sample/src/lib.rs"), rs_code).unwrap();
        dir
    }

    #[test]
    fn milestone_order_places_wave2_gate_strictly_before_phase9() {
        assert!(Milestone::Phase0 < Milestone::Phase1);
        assert!(Milestone::Phase6 < Milestone::Phase6b);
        assert!(Milestone::Phase6b < Milestone::Wave1Exit);
        assert!(Milestone::Phase8 < Milestone::Wave2Gate);
        assert!(Milestone::Wave2Gate < Milestone::Phase9);
        assert!(Milestone::Phase9 < Milestone::Wave3Exit);
        assert!(Milestone::Wave3Exit < Milestone::PostWave3);
    }

    #[test]
    fn rejects_unknown_milestone_string() {
        let err = parse_milestone_str("current = \"phase-99\"\n").unwrap_err();
        assert!(matches!(err, LedgerError::MilestoneToml(_)));
    }

    #[test]
    fn passes_clean_repo_with_referenced_todo_comment() {
        let dir = make_temp_repo(
            "current = \"phase-0\"\n",
            r#"
schema_version = "2.0"

[[deferral]]
id = "DEF-001"
title = "PWM transfer functions"
owner_milestone = "phase-3"
deadline_milestone = "phase-3"
reason = "datasheet verification"
refs = ["docs/architecture/01.md#pwm"]
"#,
            "// TODO(DEF-001): refine AVR Fast PWM edge case\npub fn ok() -> u32 { 42 }\n",
        );

        let report = run_ledger_check(dir.path()).unwrap();
        assert_eq!(report.current_milestone, Milestone::Phase0);
        assert_eq!(report.open_deferrals, 1);
        assert_eq!(report.scanned_rust_files, 1);
    }

    fn expect_violations(err: LedgerError) -> Vec<LedgerViolation> {
        match err {
            LedgerError::Violations(vs) => vs,
            LedgerError::Io { path, source } => panic!("unexpected IO error on {path}: {source}"),
            LedgerError::MilestoneToml(e) => panic!("unexpected MilestoneToml error: {e}"),
            LedgerError::DeferralsToml(e) => panic!("unexpected DeferralsToml error: {e}"),
            LedgerError::UnsupportedVersion(v) => panic!("unexpected UnsupportedVersion: {v}"),
        }
    }

    #[test]
    fn all_15_milestones_round_trip_with_as_str() {
        let all = [
            Milestone::Phase0,
            Milestone::Phase1,
            Milestone::Phase2,
            Milestone::Phase3,
            Milestone::Phase4,
            Milestone::Phase5,
            Milestone::Phase6,
            Milestone::Phase6b,
            Milestone::Wave1Exit,
            Milestone::Phase7,
            Milestone::Phase8,
            Milestone::Wave2Gate,
            Milestone::Phase9,
            Milestone::Wave3Exit,
            Milestone::PostWave3,
        ];
        for pair in all.windows(2) {
            assert!(pair[0] < pair[1], "{} must precede {}", pair[0], pair[1]);
        }
        for m in all {
            let doc = format!("current = \"{}\"\n", m.as_str());
            assert_eq!(parse_milestone_str(&doc).unwrap().current, m);
        }
    }

    #[test]
    fn fails_on_expired_deferral_deadline() {
        let dir = make_temp_repo(
            "current = \"phase-9\"\n",
            r#"
schema_version = "2.0"

[[deferral]]
id = "DEF-004"
title = "asl-oop design"
owner_milestone = "wave-2-gate"
deadline_milestone = "wave-2-gate"
reason = "must close before phase-9"
refs = ["docs/architecture/01.md"]
"#,
            "pub fn ok() {}\n",
        );

        let vs = expect_violations(run_ledger_check(dir.path()).unwrap_err());
        assert!(vs.iter().any(|v| v.message.contains("expired")));
    }

    #[test]
    fn fails_on_broken_reference_path() {
        let dir = make_temp_repo(
            "current = \"phase-0\"\n",
            r#"
schema_version = "2.0"

[[deferral]]
id = "DEF-001"
title = "PWM"
owner_milestone = "phase-3"
deadline_milestone = "phase-3"
reason = "verify"
refs = ["docs/architecture/nonexistent.md"]
"#,
            "pub fn ok() {}\n",
        );

        let vs = expect_violations(run_ledger_check(dir.path()).unwrap_err());
        assert!(
            vs.iter()
                .any(|v| v.message.contains("broken deferral reference"))
        );
    }

    #[test]
    fn fails_on_bare_todo_todo_macro_and_ignore_attribute() {
        let dir = make_temp_repo(
            "current = \"phase-0\"\n",
            "schema_version = \"2.0\"\ndeferral = []\n",
            concat!(
                "// TODO: forgotten hack\n",
                "pub fn stub() { todo!() }\n",
                "#[ignore]\n",
                "fn skipped() {}\n",
                "pub fn string_ok() -> &'static str { \"todo!() #[ignore] // TODO\" }\n",
            ),
        );

        let vs = expect_violations(run_ledger_check(dir.path()).unwrap_err());
        assert_eq!(
            vs.len(),
            3,
            "expected exactly 3 real violations, got {vs:?}"
        );
    }

    #[test]
    fn fails_on_unregistered_def_id_in_comment() {
        let dir = make_temp_repo(
            "current = \"phase-0\"\n",
            "schema_version = \"2.0\"\ndeferral = []\n",
            "// FIXME(DEF-999): not registered\npub fn f() {}\n",
        );

        let vs = expect_violations(run_ledger_check(dir.path()).unwrap_err());
        assert!(vs.iter().any(|v| v.message.contains("DEF-999")));
    }
}
