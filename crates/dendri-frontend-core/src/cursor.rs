//! Zero-copy UTF-8-safe cursor, byte-span, and line/column primitives (`04` §1.2).
//!
//! Every operation is total and UTF-8 boundary safe: out-of-range offsets,
//! multibyte Unicode characters, CRLF/LF endings, and EOF never panic or slice
//! with unchecked indexing.

use dendri_ast::EolStyle;
use dendri_span::{LineCol, Span};

/// Saved cursor position for zero-cost lookahead and backtracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CursorCheckpoint {
    pos: usize,
}

impl CursorCheckpoint {
    /// Byte offset captured in this checkpoint.
    #[must_use]
    pub const fn byte_offset(self) -> usize {
        self.pos
    }
}

/// Generic AST or token payload paired with its exact source [`Span`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    /// Wraps `node` with `span`.
    #[must_use]
    pub const fn new(node: T, span: Span) -> Self {
        Self { node, span }
    }

    /// Transforms the inner payload while preserving the exact byte span.
    #[must_use]
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Spanned<U> {
        Spanned {
            node: f(self.node),
            span: self.span,
        }
    }

    /// Merges this span with `other_span`.
    #[must_use]
    pub const fn merge_span(&self, other_span: Span) -> Span {
        self.span.merge(other_span)
    }
}

/// Detects the dominant line-ending convention (`EolStyle::Crlf` vs `EolStyle::Lf`)
/// from the first line break present in `src`. Defaults to `EolStyle::Lf`.
#[must_use]
pub fn detect_eol_style(src: &str) -> EolStyle {
    let bytes = src.as_bytes();
    for (idx, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            if idx > 0 && bytes.get(idx.saturating_sub(1)) == Some(&b'\r') {
                return EolStyle::Crlf;
            }
            return EolStyle::Lf;
        }
    }
    EolStyle::Lf
}

/// Returns `true` if `ch` is a valid ASCII identifier start (`A-Z`, `a-z`, `_`).
#[must_use]
pub const fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

/// Returns `true` if `ch` is a valid ASCII identifier continuation (`A-Z`, `a-z`, `0-9`, `_`).
#[must_use]
pub const fn is_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

/// Precomputed line-start table for O(log N) byte-offset to 1-indexed [`LineCol`] lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    line_starts: Vec<u32>,
    source_len: usize,
}

impl LineIndex {
    /// Builds a line-start index over `src`.
    #[must_use]
    pub fn new(src: &str) -> Self {
        let mut line_starts = vec![0u32];
        for (idx, byte) in src.bytes().enumerate() {
            if byte == b'\n'
                && let Ok(next) = u32::try_from(idx.saturating_add(1))
            {
                line_starts.push(next);
            }
        }
        Self {
            line_starts,
            source_len: src.len(),
        }
    }

    /// Total number of lines (always at least 1).
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Returns the 1-indexed line number containing `byte_offset`.
    #[must_use]
    pub fn line_of(&self, byte_offset: u32) -> u32 {
        let max_len = u32::try_from(self.source_len).unwrap_or(u32::MAX);
        let offset = byte_offset.min(max_len);
        let idx = self
            .line_starts
            .partition_point(|&start| start <= offset)
            .saturating_sub(1);
        u32::try_from(idx.saturating_add(1)).unwrap_or(u32::MAX)
    }

    /// Returns `true` iff byte offsets `a` and `b` fall on the same 1-indexed line.
    #[must_use]
    pub fn is_same_line(&self, a: u32, b: u32) -> bool {
        self.line_of(a) == self.line_of(b)
    }

    /// Resolves `byte_offset` against `src` into a 1-indexed [`LineCol`] (Unicode scalar column).
    ///
    /// Out-of-range or mid-character offsets clamp cleanly without panicking.
    #[must_use]
    pub fn line_col(&self, src: &str, byte_offset: u32) -> LineCol {
        let max_len = u32::try_from(src.len()).unwrap_or(u32::MAX);
        let offset = byte_offset.min(max_len);
        let line_idx = self
            .line_starts
            .partition_point(|&start| start <= offset)
            .saturating_sub(1);
        let line_start = self.line_starts.get(line_idx).copied().unwrap_or(0) as usize;
        let target = clamp_to_char_boundary(src, offset as usize);
        let col_chars = src
            .get(line_start..target)
            .map_or(0, |prefix| prefix.chars().count());
        let line = u32::try_from(line_idx.saturating_add(1)).unwrap_or(u32::MAX);
        let column = u32::try_from(col_chars.saturating_add(1)).unwrap_or(u32::MAX);
        LineCol { line, column }
    }

    /// Returns the 1-indexed line text from `src` without its trailing `\r\n` or `\n`.
    #[must_use]
    pub fn line_text<'a>(&self, src: &'a str, line_number: u32) -> Option<&'a str> {
        let idx = usize::try_from(line_number.checked_sub(1)?).ok()?;
        let start = usize::try_from(*self.line_starts.get(idx)?).ok()?;
        let raw_end = match self.line_starts.get(idx.saturating_add(1)) {
            Some(&next) => usize::try_from(next).ok()?,
            None => src.len(),
        };
        let slice = src.get(start..raw_end)?;
        let trimmed = slice
            .strip_suffix("\r\n")
            .or_else(|| slice.strip_suffix('\n'))
            .unwrap_or(slice);
        Some(trimmed)
    }
}

/// Walks backwards from `offset` to the nearest UTF-8 character boundary in `s`.
fn clamp_to_char_boundary(s: &str, offset: usize) -> usize {
    let mut pos = offset.min(s.len());
    while pos > 0 && !s.is_char_boundary(pos) {
        pos = pos.saturating_sub(1);
    }
    pos
}

/// Zero-copy, UTF-8-safe source cursor with exact byte-offset tracking.
#[derive(Debug, Clone)]
pub struct Cursor<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// Creates a cursor positioned at byte offset `0` of `src`.
    #[must_use]
    pub const fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    /// Returns the full underlying source slice.
    #[must_use]
    pub const fn source(&self) -> &'a str {
        self.src
    }

    /// Returns the unread tail of the source starting at the current byte position.
    #[must_use]
    pub fn remaining(&self) -> &'a str {
        self.src.get(self.pos..).unwrap_or("")
    }

    /// Current byte offset into the source.
    #[must_use]
    pub const fn pos(&self) -> usize {
        self.pos
    }

    /// Current byte offset clamped to `u32` for constructing [`Span`] values.
    #[must_use]
    pub fn pos_u32(&self) -> u32 {
        u32::try_from(self.pos).unwrap_or(u32::MAX)
    }

    /// Returns `true` when no unread bytes remain.
    #[must_use]
    pub const fn is_eof(&self) -> bool {
        self.pos >= self.src.len()
    }

    /// Captures the current position for later backtracking via [`Self::restore`].
    #[must_use]
    pub const fn checkpoint(&self) -> CursorCheckpoint {
        CursorCheckpoint { pos: self.pos }
    }

    /// Restores a previously captured [`CursorCheckpoint`].
    ///
    /// Ignores invalid or non-char-boundary offsets safely.
    pub fn restore(&mut self, checkpoint: CursorCheckpoint) {
        if checkpoint.pos <= self.src.len() && self.src.is_char_boundary(checkpoint.pos) {
            self.pos = checkpoint.pos;
        }
    }

    /// Returns the next Unicode scalar value without advancing.
    #[must_use]
    pub fn peek(&self) -> Option<char> {
        self.remaining().chars().next()
    }

    /// Returns the second unread Unicode scalar value without advancing.
    #[must_use]
    pub fn peek_second(&self) -> Option<char> {
        let mut chars = self.remaining().chars();
        chars.next()?;
        chars.next()
    }

    /// Returns the `n`-th unread Unicode scalar value (0-indexed) without advancing.
    #[must_use]
    pub fn peek_nth(&self, n: usize) -> Option<char> {
        self.remaining().chars().nth(n)
    }

    /// Returns the next raw UTF-8 byte without advancing.
    #[must_use]
    pub fn peek_byte(&self) -> Option<u8> {
        self.src.as_bytes().get(self.pos).copied()
    }

    /// Returns `true` if the remaining input starts with `pat`.
    #[must_use]
    pub fn starts_with(&self, pat: &str) -> bool {
        self.remaining().starts_with(pat)
    }

    /// Returns `true` if the remaining input starts with `ascii_pat` ignoring ASCII case.
    #[must_use]
    pub fn starts_with_ascii_ignore_case(&self, ascii_pat: &str) -> bool {
        let rem = self.remaining();
        let Some(prefix) = rem.get(..ascii_pat.len()) else {
            return false;
        };
        prefix.eq_ignore_ascii_case(ascii_pat)
    }

    /// Advances by one Unicode scalar value and returns it, or `None` at EOF.
    pub fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos = self.pos.saturating_add(ch.len_utf8());
        Some(ch)
    }

    /// Consumes `expected` if it is the next character.
    pub fn eat_char(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.pos = self.pos.saturating_add(expected.len_utf8());
            true
        } else {
            false
        }
    }

    /// Consumes `expected` if the remaining input starts with it.
    pub fn eat_str(&mut self, expected: &str) -> bool {
        if self.starts_with(expected) {
            self.pos = self.pos.saturating_add(expected.len());
            true
        } else {
            false
        }
    }

    /// Consumes `ascii_expected` case-insensitively if the remaining input starts with it.
    pub fn eat_str_ascii_ignore_case(&mut self, ascii_expected: &str) -> bool {
        if self.starts_with_ascii_ignore_case(ascii_expected) {
            self.pos = self.pos.saturating_add(ascii_expected.len());
            true
        } else {
            false
        }
    }

    /// Consumes an ASCII case-insensitive keyword only when not immediately followed
    /// by an identifier continuation character (`[A-Za-z0-9_]`).
    pub fn eat_keyword_ascii_ignore_case(&mut self, keyword: &str) -> Option<Span> {
        if !self.starts_with_ascii_ignore_case(keyword) {
            return None;
        }
        let next_pos = self.pos.saturating_add(keyword.len());
        let after = self.src.get(next_pos..).and_then(|s| s.chars().next());
        if after.is_some_and(is_ident_continue) {
            return None;
        }
        let start = self.pos;
        self.pos = next_pos;
        Some(self.span_from(start))
    }

    /// Advances while `pred` returns `true` and returns the zero-copy consumed slice.
    pub fn eat_while<F>(&mut self, mut pred: F) -> &'a str
    where
        F: FnMut(char) -> bool,
    {
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if pred(ch) {
                self.pos = self.pos.saturating_add(ch.len_utf8());
            } else {
                break;
            }
        }
        self.slice_from(start)
    }

    /// Advances while `pred` returns `true` and returns both the consumed slice and its [`Span`].
    pub fn eat_while_spanned<F>(&mut self, pred: F) -> (&'a str, Span)
    where
        F: FnMut(char) -> bool,
    {
        let start = self.pos;
        let text = self.eat_while(pred);
        (text, self.span_from(start))
    }

    /// Consumes up to (but not including) the first occurrence of `needle`.
    ///
    /// Returns `(consumed_slice, found_needle)`. If `needle` does not occur, consumes to EOF
    /// and returns `(rest, false)`.
    pub fn eat_until_str(&mut self, needle: &str) -> (&'a str, bool) {
        let start = self.pos;
        if needle.is_empty() {
            return ("", true);
        }
        let rem = self.remaining();
        if let Some(rel) = rem.find(needle) {
            self.pos = self.pos.saturating_add(rel);
            (self.slice_from(start), true)
        } else {
            self.pos = self.src.len();
            (self.slice_from(start), false)
        }
    }

    /// Consumes characters up to (but not including) `\r\n`, `\n`, or EOF.
    ///
    /// Note: lone `\r` not followed by `\n` is also treated as a line terminator boundary
    /// only if followed by `\n`; otherwise it remains part of the line until `\n`/EOF.
    pub fn eat_line(&mut self) -> &'a str {
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if ch == '\n' {
                break;
            }
            if ch == '\r' && self.peek_second() == Some('\n') {
                break;
            }
            self.pos = self.pos.saturating_add(ch.len_utf8());
        }
        self.slice_from(start)
    }

    /// Consumes a single newline (`\r\n` or `\n`) and reports its [`EolStyle`].
    pub fn eat_newline(&mut self) -> Option<EolStyle> {
        if self.eat_str("\r\n") {
            Some(EolStyle::Crlf)
        } else if self.eat_char('\n') {
            Some(EolStyle::Lf)
        } else {
            None
        }
    }

    /// Skips horizontal ASCII whitespace (`' '` and `'\t'`) without crossing line breaks.
    ///
    /// Returns the number of skipped bytes.
    pub fn skip_horizontal_whitespace(&mut self) -> usize {
        let start = self.pos;
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.pos = self.pos.saturating_add(1);
        }
        self.pos.saturating_sub(start)
    }

    /// Skips ASCII whitespace (`' '`, `'\t'`, `'\r'`, `'\n'`) and returns the number of
    /// blank (whitespace-only) lines crossed.
    ///
    /// When the cursor starts mid-line (i.e. not at offset 0 and not immediately after `'\n'`),
    /// the first line break ends the current line and does not count as a blank line.
    pub fn skip_whitespace_and_count_blank_lines(&mut self) -> u32 {
        let started_at_line_start =
            self.pos == 0 || self.src.as_bytes().get(self.pos.saturating_sub(1)) == Some(&b'\n');
        let mut newlines = 0u32;
        loop {
            match self.peek() {
                Some(' ' | '\t') => {
                    self.pos = self.pos.saturating_add(1);
                }
                Some('\r') if self.peek_second() == Some('\n') => {
                    self.pos = self.pos.saturating_add(2);
                    newlines = newlines.saturating_add(1);
                }
                Some('\n') => {
                    self.pos = self.pos.saturating_add(1);
                    newlines = newlines.saturating_add(1);
                }
                Some('\r') => {
                    self.pos = self.pos.saturating_add(1);
                }
                _ => break,
            }
        }
        if started_at_line_start {
            newlines
        } else {
            newlines.saturating_sub(1)
        }
    }

    /// Consumes an ASCII identifier (`[A-Za-z_][A-Za-z0-9_]*`), returning its slice and [`Span`].
    pub fn eat_ident(&mut self) -> Option<(&'a str, Span)> {
        let first = self.peek()?;
        if !is_ident_start(first) {
            return None;
        }
        let start = self.pos;
        self.pos = self.pos.saturating_add(first.len_utf8());
        let _ = self.eat_while(is_ident_continue);
        Some((self.slice_from(start), self.span_from(start)))
    }

    /// Returns the zero-copy source slice from byte offset `start` up to `self.pos()`.
    #[must_use]
    pub fn slice_from(&self, start: usize) -> &'a str {
        self.src.get(start..self.pos).unwrap_or("")
    }

    /// Constructs a [`Span`] from byte offset `start` up to `self.pos()`.
    #[must_use]
    pub fn span_from(&self, start: usize) -> Span {
        let start_u32 = u32::try_from(start).unwrap_or(u32::MAX);
        Span::new(start_u32, self.pos_u32())
    }

    /// Computes the 1-indexed [`LineCol`] at the cursor's current position.
    #[must_use]
    pub fn line_col(&self) -> LineCol {
        let prefix = self.src.get(..self.pos).unwrap_or("");
        let mut line = 1u32;
        let mut column = 1u32;
        for ch in prefix.chars() {
            if ch == '\n' {
                line = line.saturating_add(1);
                column = 1;
            } else {
                column = column.saturating_add(1);
            }
        }
        LineCol { line, column }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn cursor_tracks_utf8_spans_and_line_col_accurately() {
        let src = "VAR\r\n  ação := 42;\nEND_VAR";
        assert_eq!(detect_eol_style(src), EolStyle::Crlf);

        let index = LineIndex::new(src);
        assert_eq!(index.line_count(), 3);
        assert_eq!(index.line_text(src, 1), Some("VAR"));
        assert_eq!(index.line_text(src, 2), Some("  ação := 42;"));
        assert_eq!(index.line_text(src, 3), Some("END_VAR"));
        assert_eq!(index.line_text(src, 0), None);
        assert_eq!(index.line_text(src, 4), None);

        let mut cur = Cursor::new(src);
        let kw_span = cur.eat_keyword_ascii_ignore_case("var").unwrap();
        assert_eq!(kw_span.slice(src), Some("VAR"));
        assert_eq!(cur.eat_newline(), Some(EolStyle::Crlf));
        assert_eq!(cur.skip_horizontal_whitespace(), 2);

        let start = cur.pos();
        let word = cur.eat_while(|c| !c.is_whitespace());
        assert_eq!(word, "ação");
        let word_span = cur.span_from(start);
        assert_eq!(word_span.slice(src), Some("ação"));
        assert_eq!(
            index.line_col(src, word_span.start),
            LineCol { line: 2, column: 3 }
        );
        assert_eq!(cur.line_col(), LineCol { line: 2, column: 7 });
        assert!(index.is_same_line(word_span.start, word_span.end));
    }

    #[test]
    fn cursor_blank_line_counting_matches_adr027_splitting() {
        let mut cur = Cursor::new("// a\n// b\n\n// c\n  \r\n\r\n// d");
        assert_eq!(cur.eat_line(), "// a");
        assert_eq!(cur.skip_whitespace_and_count_blank_lines(), 0);
        assert_eq!(cur.eat_line(), "// b");
        assert_eq!(cur.skip_whitespace_and_count_blank_lines(), 1);
        assert_eq!(cur.eat_line(), "// c");
        assert_eq!(cur.skip_whitespace_and_count_blank_lines(), 2);
        assert_eq!(cur.eat_line(), "// d");
        assert!(cur.is_eof());

        // Leading blank lines at start of file
        let mut cur2 = Cursor::new("\n\nfoo");
        assert_eq!(cur2.skip_whitespace_and_count_blank_lines(), 2);
        let (ident, span) = cur2.eat_ident().unwrap();
        assert_eq!(ident, "foo");
        assert_eq!(span, Span::new(2, 5));
    }

    #[test]
    fn cursor_checkpoint_restore_and_keyword_boundary() {
        let mut cur = Cursor::new("IF_READY IF");
        // Must NOT match `IF` as prefix of `IF_READY`
        assert_eq!(cur.eat_keyword_ascii_ignore_case("if"), None);
        let cp = cur.checkpoint();
        let (ident, _) = cur.eat_ident().unwrap();
        assert_eq!(ident, "IF_READY");
        cur.restore(cp);
        assert_eq!(cur.pos(), 0);

        let _ = cur.eat_ident();
        cur.skip_horizontal_whitespace();
        let if_span = cur.eat_keyword_ascii_ignore_case("if").unwrap();
        assert_eq!(if_span.slice(cur.source()), Some("IF"));
    }

    #[test]
    fn cursor_eat_until_str_and_spanned_helpers() {
        let mut cur = Cursor::new("hello *) world");
        let (before, found) = cur.eat_until_str("*)");
        assert!(found);
        assert_eq!(before, "hello ");
        assert!(cur.eat_str("*)"));
        let (rest, found_missing) = cur.eat_until_str("NOT_THERE");
        assert!(!found_missing);
        assert_eq!(rest, " world");
        assert!(cur.is_eof());

        let sp = Spanned::new(10u32, Span::new(0, 2)).map(|v| v + 5);
        assert_eq!(sp.node, 15);
        assert_eq!(sp.merge_span(Span::new(2, 6)), Span::new(0, 6));
    }

    proptest! {
        #[test]
        fn cursor_and_line_index_never_panic_on_arbitrary_utf8(
            src in "\\PC{0,128}",
            offset in 0u32..256,
        ) {
            let idx = LineIndex::new(&src);
            let lc = idx.line_col(&src, offset);
            assert!(lc.line >= 1);
            assert!(lc.column >= 1);

            let mut cur = Cursor::new(&src);
            let mut count = 0usize;
            while let Some(ch) = cur.bump() {
                count += ch.len_utf8();
            }
            assert_eq!(count, src.len());
            assert!(cur.is_eof());
            assert_eq!(cur.peek(), None);
            assert_eq!(cur.remaining(), "");
        }
    }
}
