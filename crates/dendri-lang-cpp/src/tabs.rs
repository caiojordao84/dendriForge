//! Multi-tab Arduino `.ino` sketch merger and `.h`/`.cpp` module helper (`docs/lbc/cpp.md` §5).
//!
//! Canonical Arduino merge rules:
//! 1. Primary `<sketch>.ino` first, followed by remaining `.ino` tabs in lexicographic ASCII order.
//! 2. Automatic function forward-declaration hoisting across merged `.ino` tabs (excluding `setup`
//!    and `loop`, never duplicating existing prototypes).
//! 3. Zero banned magic comments; every human comment across all tabs is preserved verbatim.

use crate::lexer::{Token, TokenKind, lex_cpp};
use std::collections::BTreeSet;

/// Merges multi-tab Arduino `.ino` sketch tabs into a single canonical C++ translation unit.
///
/// Ordering:
/// - If any `.ino` tab's parent folder stem equals its file stem (e.g. `blink/blink.ino`),
///   that tab is primary; otherwise the first `.ino` tab (or first tab overall if none end with
///   `.ino`) is primary.
/// - Remaining `.ino` tabs follow in strict lexicographic ASCII order by tab name.
/// - Forward prototypes for all top-level user functions (other than `setup` and `loop`) are
///   automatically hoisted after the leading preprocessor preamble so calls across tabs never
///   fail with `ERR_UNDECLARED_SYMBOL`.
#[must_use]
pub fn merge_arduino_tabs(tabs: &[(&str, &str)]) -> String {
    if tabs.is_empty() {
        return String::new();
    }

    let ino_indices: Vec<usize> = tabs
        .iter()
        .enumerate()
        .filter(|(_, (name, _))| has_extension_ignore_ascii_case(name, ".ino"))
        .map(|(i, _)| i)
        .collect();

    let selected: Vec<usize> = if ino_indices.is_empty() {
        (0..tabs.len()).collect()
    } else {
        ino_indices
    };

    // Determine primary tab index within `selected`.
    let primary_pos = selected
        .iter()
        .position(|&idx| {
            tabs.get(idx)
                .is_some_and(|(name, _)| is_sketch_dir_primary(name))
        })
        .unwrap_or(0);
    let Some(&primary_idx) = selected.get(primary_pos) else {
        return String::new();
    };

    let mut secondary: Vec<(&str, &str)> = selected
        .iter()
        .enumerate()
        .filter(|&(pos, _)| pos != primary_pos)
        .filter_map(|(_, &idx)| tabs.get(idx).copied())
        .collect();
    secondary.sort_by_key(|(name, _)| *name);

    let mut ordered: Vec<(&str, &str)> = Vec::with_capacity(selected.len());
    if let Some(&primary) = tabs.get(primary_idx) {
        ordered.push(primary);
    }
    ordered.extend(secondary);

    // Collect existing prototypes and defined top-level user function signatures across all tabs.
    let mut existing_protos: BTreeSet<String> = BTreeSet::new();
    let mut hoisted_protos: Vec<String> = Vec::new();
    let mut seen_hoisted: BTreeSet<String> = BTreeSet::new();

    for &(_, content) in &ordered {
        let (protos, defs) = scan_top_level_functions(content);
        for p in protos {
            existing_protos.insert(p);
        }
        for (fn_name, signature) in defs {
            if fn_name != "setup"
                && fn_name != "loop"
                && !existing_protos.contains(&fn_name)
                && seen_hoisted.insert(fn_name)
            {
                hoisted_protos.push(format!("{signature};"));
            }
        }
    }

    // Filter out any function that already had an explicit prototype in a later tab.
    let needed_protos: Vec<String> = hoisted_protos
        .into_iter()
        .filter(|proto| {
            let fn_name = extract_fn_name_from_proto(proto);
            !existing_protos.contains(fn_name)
        })
        .collect();

    // Concatenate tabs cleanly, inserting hoisted prototypes right after primary's leading `#include`/`#define` block.
    let mut combined = String::new();
    for (idx, &(_, content)) in ordered.iter().enumerate() {
        if idx == 0 {
            if needed_protos.is_empty() {
                combined.push_str(content);
            } else {
                let split_offset = find_preamble_end_offset(content);
                let head = content.get(..split_offset).unwrap_or("");
                let tail = content.get(split_offset..).unwrap_or("");
                combined.push_str(head);
                if !head.is_empty() && !head.ends_with('\n') {
                    combined.push('\n');
                }
                for proto in &needed_protos {
                    combined.push_str(proto);
                    combined.push('\n');
                }
                if !tail.is_empty() && !tail.starts_with('\n') {
                    combined.push('\n');
                }
                combined.push_str(tail);
            }
        } else {
            if !combined.is_empty() && !combined.ends_with('\n') {
                combined.push('\n');
            }
            if !combined.ends_with("\n\n") && !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(content);
        }
    }

    combined
}

/// Returns `true` iff `name` ends with `ext` (ASCII case-insensitive).
#[must_use]
pub(crate) fn has_extension_ignore_ascii_case(name: &str, ext: &str) -> bool {
    let Some(tail) = name
        .len()
        .checked_sub(ext.len())
        .and_then(|i| name.get(i..))
    else {
        return false;
    };
    tail.eq_ignore_ascii_case(ext)
}

/// Returns `true` iff `path` is `<dir>/<stem>.ino` where `<dir> == <stem>`.
fn is_sketch_dir_primary(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let mut parts = normalized.rsplit('/');
    let Some(file_name) = parts.next() else {
        return false;
    };
    let Some(parent_dir) = parts.next() else {
        return false;
    };
    let Some(stem) = file_name
        .strip_suffix(".ino")
        .or_else(|| file_name.strip_suffix(".INO"))
    else {
        return false;
    };
    !stem.is_empty() && stem == parent_dir
}

/// Finds the byte offset right after leading `#include` / `#define` lines (and leading comments)
/// in `src` so hoisted forward declarations land after includes and before function bodies.
fn find_preamble_end_offset(src: &str) -> usize {
    let Ok(lexed) = lex_cpp(src, "merge") else {
        return 0;
    };
    let mut end_offset = 0usize;
    for tok in &lexed.tokens {
        if let TokenKind::Preproc { raw } = &tok.kind {
            let trimmed = raw.trim_start();
            if trimmed.starts_with("#include") || trimmed.starts_with("#define") {
                end_offset = usize::try_from(tok.span.end).unwrap_or(end_offset);
                // Advance past rest of that line (including any trailing comment/newline).
                if let Some(rest) = src.get(end_offset..) {
                    if let Some(nl) = rest.find('\n') {
                        end_offset = end_offset.saturating_add(nl).saturating_add(1);
                    } else {
                        end_offset = src.len();
                    }
                }
            } else {
                break;
            }
        } else {
            break;
        }
    }
    end_offset.min(src.len())
}

/// Scans top-level tokens of `src` to discover existing function prototypes and function definitions.
///
/// Returns `(existing_prototype_names, Vec<(fn_name, raw_signature_without_semicolon)>)`.
fn scan_top_level_functions(src: &str) -> (Vec<String>, Vec<(String, String)>) {
    let Ok(lexed) = lex_cpp(src, "merge") else {
        return (Vec::new(), Vec::new());
    };
    let tokens = &lexed.tokens;
    let mut protos = Vec::new();
    let mut defs = Vec::new();
    let mut i = 0usize;
    let mut brace_depth = 0usize;

    while let Some(tok) = tokens.get(i) {
        if matches!(tok.kind, TokenKind::Eof) {
            break;
        }
        if matches!(tok.kind, TokenKind::LBrace) {
            brace_depth = brace_depth.saturating_add(1);
            i = i.saturating_add(1);
        } else if matches!(tok.kind, TokenKind::RBrace) {
            brace_depth = brace_depth.saturating_sub(1);
            i = i.saturating_add(1);
        } else if matches!(tok.kind, TokenKind::Preproc { .. }) || brace_depth > 0 {
            i = i.saturating_add(1);
        } else if let Some((fn_name, sig, is_def, next_i)) = try_match_fn_header(src, tokens, i) {
            if is_def {
                defs.push((fn_name, sig));
            } else {
                protos.push(fn_name);
            }
            i = next_i;
        } else {
            i = i.saturating_add(1);
        }
    }

    (protos, defs)
}

/// Attempts to recognize a top-level C++ function header starting at token index `start`.
fn try_match_fn_header(
    src: &str,
    tokens: &[Token],
    start: usize,
) -> Option<(String, String, bool, usize)> {
    // Reject declarations that clearly start with non-function keywords.
    if let Some(Token {
        kind: TokenKind::Ident(first),
        ..
    }) = tokens.get(start)
        && matches!(
            first.as_str(),
            "struct" | "class" | "enum" | "namespace" | "template" | "using" | "typedef"
        )
    {
        return None;
    }

    // Scan forward up to `(` at depth 0, without crossing `;`, `{`, `}`, `=`.
    let mut j = start;
    while let Some(t) = tokens.get(j) {
        if matches!(t.kind, TokenKind::LParen) {
            break;
        }
        if matches!(
            t.kind,
            TokenKind::Semi
                | TokenKind::LBrace
                | TokenKind::RBrace
                | TokenKind::Assign
                | TokenKind::Preproc { .. }
                | TokenKind::Eof
        ) {
            return None;
        }
        j = j.saturating_add(1);
    }

    // Need at least a return type token before the function name token before `(`.
    if j < start.saturating_add(2) {
        return None;
    }
    let fn_name = if let Some(Token {
        kind: TokenKind::Ident(name),
        ..
    }) = tokens.get(j.saturating_sub(1))
    {
        name.clone()
    } else {
        return None;
    };

    // Match balanced `(...)`.
    let mut paren_depth = 0usize;
    let mut k = j;
    while let Some(t) = tokens.get(k) {
        if matches!(t.kind, TokenKind::LParen) {
            paren_depth = paren_depth.saturating_add(1);
        } else if matches!(t.kind, TokenKind::RParen) {
            paren_depth = paren_depth.saturating_sub(1);
            if paren_depth == 0 {
                break;
            }
        } else if matches!(t.kind, TokenKind::Eof) {
            return None;
        }
        k = k.saturating_add(1);
    }

    let rparen_tok = tokens.get(k)?;
    let after_tok = tokens.get(k.saturating_add(1))?;
    let start_byte = usize::try_from(tokens.get(start)?.span.start).ok()?;
    let end_byte = usize::try_from(rparen_tok.span.end).ok()?;
    let raw_sig = src.get(start_byte..end_byte)?.trim().to_owned();

    if matches!(after_tok.kind, TokenKind::LBrace) {
        // Skip entire function body to find next top-level token.
        let mut bdepth = 0usize;
        let mut m = k.saturating_add(1);
        while let Some(t) = tokens.get(m) {
            if matches!(t.kind, TokenKind::LBrace) {
                bdepth = bdepth.saturating_add(1);
            } else if matches!(t.kind, TokenKind::RBrace) {
                bdepth = bdepth.saturating_sub(1);
                if bdepth == 0 {
                    return Some((fn_name, raw_sig, true, m.saturating_add(1)));
                }
            } else if matches!(t.kind, TokenKind::Eof) {
                break;
            }
            m = m.saturating_add(1);
        }
        Some((fn_name, raw_sig, true, m))
    } else if matches!(after_tok.kind, TokenKind::Semi) {
        Some((fn_name, raw_sig, false, k.saturating_add(2)))
    } else {
        None
    }
}

fn extract_fn_name_from_proto(proto: &str) -> &str {
    let before_paren = proto.split('(').next().unwrap_or(proto).trim_end();
    before_paren
        .rsplit(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .next()
        .unwrap_or("")
}
