//! First-class verbatim human comments (`01-asl-ir-and-project-schema.md` §7, ADR-027).
//!
//! Comments live in their own validated tables (`[[boards.comments]]` and top-level
//! `[[comments]]`) — NEVER inside optional `[boards.presentation]`.

use crate::ident::NodeId;
use dendri_span::Span;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Tagged anchor linking a [`Comment`] to a program `NodeId` or project-level entity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommentAnchor {
    Node { id: NodeId },
    Project,
    Board { id: String },
    Component { id: String },
    Wire { id: String },
}

impl CommentAnchor {
    /// Returns `true` iff this anchor targets an in-program [`NodeId`].
    #[must_use]
    pub const fn is_node(&self) -> bool {
        matches!(self, Self::Node { .. })
    }
}

/// Syntactic placement of a comment relative to its anchor (`01` §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentPlacement {
    Leading,
    Trailing,
    Dangling,
    Header,
    Inner,
    Title,
    Body,
}

/// Closed enum of original comment syntactic styles (`01` §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentStyle {
    /// `// ...` (C++, IEC ST 3rd ed., Rust) or `# ...` (MicroPython, ASL/TOML).
    Line,
    /// C-style `/* ... */`.
    Block,
    /// Classic IEC 61131-3 ST `(* ... *)` (distinct from `/* ... */`, supports nesting).
    ParenBlock,
    /// `/// ...` (`leading`) or `//! ...` (`header`).
    DocLine,
    /// `/** ... */` (`leading`) or `/*! ... */` (`header`).
    DocBlock,
    /// Authored in visual UI surfaces (Ladder rung title/body, GRAFCET note, Tag Table, Canvas).
    Visual,
}

/// Language & original syntax metadata of a comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentOrigin {
    pub language: String,
    pub style: CommentStyle,
}

/// Target emission encoding policy for comment projections (`01` §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentEncoding {
    #[default]
    Utf8Passthrough,
    AsciiFold,
    WarnOnly,
}

/// First-class verbatim human comment stored in `[[boards.comments]]` or top-level `[[comments]]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Comment {
    pub id: String,
    pub anchor: CommentAnchor,
    pub placement: CommentPlacement,
    /// REQUIRED iff `placement == CommentPlacement::Inner`, FORBIDDEN otherwise (`ERR_COMMENT_FIELD`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_ordinal: Option<u32>,
    /// Verbatim text between comment delimiters: zero trimming, zero Unicode normalization, LF line endings.
    pub text: String,
    pub origin: CommentOrigin,
    #[serde(skip)]
    pub span: Span,
}

/// Detects Unicode bidirectional control characters (Trojan-Source class, `WARN_COMMENT_BIDI`).
#[must_use]
pub fn contains_bidi_control(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(
            c,
            '\u{061C}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
        )
    })
}

/// Computes the multiset of verbatim comment texts (`C1` conservation invariant).
#[must_use]
pub fn comment_multiset(comments: &[Comment]) -> BTreeMap<&str, usize> {
    let mut map = BTreeMap::new();
    for c in comments {
        *map.entry(c.text.as_str()).or_insert(0) += 1;
    }
    map
}

/// Non-destructively transitions all comments anchored on `deleted_node` to `dangling`
/// on `parent_scope` (`C2` orphan preservation, returns count of orphaned comments).
///
/// Deleting a node MUST NEVER delete its comment.
pub fn orphan_comments_for_node(
    comments: &mut [Comment],
    deleted_node: &NodeId,
    parent_scope: &NodeId,
) -> usize {
    let mut orphaned = 0usize;
    for comment in comments {
        if let CommentAnchor::Node { id } = &comment.anchor
            && id == deleted_node
        {
            comment.anchor = CommentAnchor::Node {
                id: parent_scope.clone(),
            };
            comment.placement = CommentPlacement::Dangling;
            comment.token_ordinal = None;
            orphaned = orphaned.saturating_add(1);
        }
    }
    orphaned
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_comment(id: &str, node: &str, text: &str) -> Comment {
        Comment {
            id: id.to_owned(),
            anchor: CommentAnchor::Node {
                id: NodeId::new(node),
            },
            placement: CommentPlacement::Leading,
            token_ordinal: None,
            text: text.to_owned(),
            origin: CommentOrigin {
                language: "cpp".to_owned(),
                style: CommentStyle::Line,
            },
            span: Span::default(),
        }
    }

    #[test]
    fn c1_and_c2_orphan_preservation_never_loses_comment_text_multiset() {
        let mut comments = vec![
            sample_comment(
                "c_1",
                "n_stmt_1",
                " Liga o motor (24 V)\n com intertravamento",
            ),
            Comment {
                id: "c_2".to_owned(),
                anchor: CommentAnchor::Node {
                    id: NodeId::new("n_stmt_1"),
                },
                placement: CommentPlacement::Inner,
                token_ordinal: Some(2),
                text: " offset calibração".to_owned(),
                origin: CommentOrigin {
                    language: "cpp".to_owned(),
                    style: CommentStyle::Block,
                },
                span: Span::default(),
            },
            sample_comment("c_3", "n_stmt_2", " outra linha"),
        ];

        let before: BTreeMap<String, usize> = comment_multiset(&comments)
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();

        let count = orphan_comments_for_node(
            &mut comments,
            &NodeId::new("n_stmt_1"),
            &NodeId::new("n_pou_main"),
        );
        assert_eq!(count, 2);
        assert_eq!(comments[0].placement, CommentPlacement::Dangling);
        assert_eq!(comments[1].placement, CommentPlacement::Dangling);
        assert_eq!(comments[1].token_ordinal, None);
        assert_eq!(
            comments[0].anchor,
            CommentAnchor::Node {
                id: NodeId::new("n_pou_main")
            }
        );

        let after: BTreeMap<String, usize> = comment_multiset(&comments)
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        assert_eq!(
            before, after,
            "C1/C2: comment text multiset must remain invariant"
        );
    }

    #[test]
    fn detects_trojan_source_bidi_controls() {
        assert!(!contains_bidi_control("Normal português çãé 🚀"));
        assert!(contains_bidi_control("safe \u{202E}reversed"));
        assert!(contains_bidi_control("isolate \u{2066}x\u{2069}"));
    }
}
