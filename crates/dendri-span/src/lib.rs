//! Core byte-span, source-map, and frozen diagnostic primitives for `dendriForge` v2.
//!
//! Zero internal workspace dependencies (Layer 0). Every public operation is
//! total and UTF-8 boundary safe: out-of-range or misaligned spans never panic.

pub mod diagnostic;
pub mod error_code;
pub mod source_map;

pub use diagnostic::{Diagnostic, DiagnosticBag, Severity};
pub use error_code::{ErrorCode, ParseErrorCodeError};
pub use source_map::{FileId, LineCol, SourceFile, SourceMap};

use serde::{Deserialize, Serialize};
use std::fmt;

/// Half-open byte offset range `[start, end)` into a [`SourceFile`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Span {
    /// Inclusive start byte offset.
    pub start: u32,
    /// Exclusive end byte offset (`end >= start`).
    pub end: u32,
}

impl Span {
    /// Constructs a normalized span, clamping `end` up to `start` if inverted.
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        let normalized_end = if end < start { start } else { end };
        Self {
            start,
            end: normalized_end,
        }
    }

    /// Constructs a zero-width point span at `offset`.
    #[must_use]
    pub const fn point(offset: u32) -> Self {
        Self {
            start: offset,
            end: offset,
        }
    }

    /// Byte length of this span (`end - start`).
    #[must_use]
    pub const fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    /// Returns `true` if `start == end`.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start >= self.end
    }

    /// Returns the smallest span covering both `self` and `other`.
    #[must_use]
    pub const fn merge(self, other: Self) -> Self {
        let start = if self.start < other.start {
            self.start
        } else {
            other.start
        };
        let end = if self.end > other.end {
            self.end
        } else {
            other.end
        };
        Self { start, end }
    }

    /// Returns `true` if byte offset `offset` lies within `[start, end)`.
    #[must_use]
    pub const fn contains(self, offset: u32) -> bool {
        offset >= self.start && offset < self.end
    }

    /// Safely slices `src` at `[start, end)`.
    ///
    /// Returns `None` if offsets exceed `src.len()` or do not fall on UTF-8 char boundaries.
    #[must_use]
    pub fn slice(self, src: &str) -> Option<&str> {
        let start = usize::try_from(self.start).ok()?;
        let end = usize::try_from(self.end).ok()?;
        src.get(start..end)
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn span_normalizes_inverted_bounds_and_slices_safely() {
        let inverted = Span::new(10, 4);
        assert_eq!(inverted, Span::new(10, 10));
        assert!(inverted.is_empty());
        assert_eq!(inverted.len(), 0);

        let src = "olá mundo";
        // 'á' is 2 bytes at indices 2..4
        assert_eq!(Span::new(0, 4).slice(src), Some("olá"));
        // Splitting inside multibyte UTF-8 character returns None instead of panicking
        assert_eq!(Span::new(0, 3).slice(src), None);
        assert_eq!(Span::new(0, 999).slice(src), None);
    }

    #[test]
    fn span_merge_and_contains() {
        let a = Span::new(4, 10);
        let b = Span::new(2, 8);
        assert_eq!(a.merge(b), Span::new(2, 10));
        assert!(a.contains(4));
        assert!(a.contains(9));
        assert!(!a.contains(10));
        assert!(!Span::point(5).contains(5));
    }

    proptest! {
        #[test]
        fn slice_never_panics_on_arbitrary_utf8_and_offsets(
            src in "\\PC{0,64}",
            a in 0u32..128,
            b in 0u32..128,
        ) {
            let span = Span::new(a, b);
            assert!(span.end >= span.start);
            if let Some(sub) = span.slice(&src) {
                assert_eq!(sub.len(), span.len() as usize);
            }
        }
    }
}
