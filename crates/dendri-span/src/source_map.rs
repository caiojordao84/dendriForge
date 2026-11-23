//! UTF-8-safe source registry mapping byte offsets to 1-indexed line/column coordinates.

use crate::Span;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Opaque identifier of a file registered in a [`SourceMap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FileId(pub u32);

/// 1-indexed line and UTF-8 scalar-value column position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LineCol {
    /// 1-indexed line number.
    pub line: u32,
    /// 1-indexed Unicode scalar column within the line.
    pub column: u32,
}

impl fmt::Display for LineCol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

/// Registered source text with precomputed line-start byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    id: FileId,
    name: String,
    source: String,
    line_starts: Vec<u32>,
}

impl SourceFile {
    /// Creates a new [`SourceFile`] and indexes its line boundaries.
    #[must_use]
    pub fn new(id: FileId, name: impl Into<String>, source: impl Into<String>) -> Self {
        let source = source.into();
        let mut line_starts = vec![0u32];
        for (idx, byte) in source.bytes().enumerate() {
            if byte == b'\n'
                && let Ok(next) = u32::try_from(idx.saturating_add(1))
            {
                line_starts.push(next);
            }
        }
        Self {
            id,
            name: name.into(),
            source,
            line_starts,
        }
    }

    /// File identifier.
    #[must_use]
    pub const fn id(&self) -> FileId {
        self.id
    }

    /// Display path or name of the source file.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Full UTF-8 text of the source file.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Total number of lines (at least 1).
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Resolves a byte offset (clamped to `source.len()`) into a 1-indexed [`LineCol`].
    #[must_use]
    pub fn line_col(&self, byte_offset: u32) -> LineCol {
        let clamped_len = u32::try_from(self.source.len()).unwrap_or(u32::MAX);
        let offset = byte_offset.min(clamped_len);
        let line_idx = self
            .line_starts
            .partition_point(|&start| start <= offset)
            .saturating_sub(1);
        let line_start = self.line_starts.get(line_idx).copied().unwrap_or(0) as usize;
        let target = clamp_to_char_boundary(&self.source, offset as usize);
        let col_chars = self
            .source
            .get(line_start..target)
            .map_or(0, |prefix| prefix.chars().count());
        let line = u32::try_from(line_idx.saturating_add(1)).unwrap_or(u32::MAX);
        let column = u32::try_from(col_chars.saturating_add(1)).unwrap_or(u32::MAX);
        LineCol { line, column }
    }

    /// Returns the text of 1-indexed `line_number` without trailing `\n` or `\r\n`.
    #[must_use]
    pub fn line_text(&self, line_number: u32) -> Option<&str> {
        let idx = usize::try_from(line_number.checked_sub(1)?).ok()?;
        let start = usize::try_from(*self.line_starts.get(idx)?).ok()?;
        let raw_end = match self.line_starts.get(idx.saturating_add(1)) {
            Some(&next) => usize::try_from(next).ok()?,
            None => self.source.len(),
        };
        let slice = self.source.get(start..raw_end)?;
        let trimmed = slice
            .strip_suffix("\r\n")
            .or_else(|| slice.strip_suffix('\n'))
            .unwrap_or(slice);
        Some(trimmed)
    }

    /// Formats a human-readable single-line caret snippet for `span`, if possible.
    #[must_use]
    pub fn render_snippet(&self, span: Span) -> Option<String> {
        let start_lc = self.line_col(span.start);
        let end_lc = self.line_col(span.end);
        let line_str = self.line_text(start_lc.line)?;
        let gutter = start_lc.line.to_string();
        let pad = " ".repeat(gutter.len());
        let caret_offset = usize::try_from(start_lc.column.saturating_sub(1)).ok()?;
        let caret_len = if start_lc.line == end_lc.line && end_lc.column > start_lc.column {
            usize::try_from(end_lc.column.saturating_sub(start_lc.column)).unwrap_or(1)
        } else {
            1
        };
        let mut out = String::new();
        out.push_str(&format!("{pad} |\n"));
        out.push_str(&format!("{gutter} | {line_str}\n"));
        out.push_str(&format!(
            "{pad} | {}{}",
            " ".repeat(caret_offset),
            "^".repeat(caret_len)
        ));
        Some(out)
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

/// Ordered registry of all source files active during compilation or validation.
#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    /// Creates an empty [`SourceMap`].
    #[must_use]
    pub const fn new() -> Self {
        Self { files: Vec::new() }
    }

    /// Adds a file and returns its unique [`FileId`].
    pub fn add_file(&mut self, name: impl Into<String>, source: impl Into<String>) -> FileId {
        let id = FileId(u32::try_from(self.files.len()).unwrap_or(u32::MAX));
        self.files.push(SourceFile::new(id, name, source));
        id
    }

    /// Looks up a registered file by [`FileId`].
    #[must_use]
    pub fn get(&self, id: FileId) -> Option<&SourceFile> {
        usize::try_from(id.0)
            .ok()
            .and_then(|idx| self.files.get(idx))
    }

    /// Number of registered files.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Returns `true` if no files are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn line_col_handles_lf_crlf_and_multibyte_utf8() {
        let mut sm = SourceMap::new();
        let fid = sm.add_file("test.st", "VAR\r\n  ação := 1;\nEND_VAR");
        let file = sm.get(fid).unwrap();
        assert_eq!(file.line_count(), 3);
        assert_eq!(file.line_text(1), Some("VAR"));
        assert_eq!(file.line_text(2), Some("  ação := 1;"));
        assert_eq!(file.line_text(3), Some("END_VAR"));
        assert_eq!(file.line_text(0), None);
        assert_eq!(file.line_text(4), None);

        // "VAR\r\n" is 5 bytes. "  " is 2 bytes (offset 7). "ação" is 6 bytes ('ç'=2, 'ã'=2).
        // So ':' starts at offset 7 + 6 + 1 = 14, which is line 2, column 8 (2 spaces + 4 chars + 1 space + 1)
        let colon_offset = file.source().find(":=").unwrap() as u32;
        assert_eq!(file.line_col(colon_offset), LineCol { line: 2, column: 8 });

        let snippet = file
            .render_snippet(Span::new(colon_offset, colon_offset + 2))
            .unwrap();
        assert!(snippet.contains("2 |   ação := 1;"));
        assert!(snippet.contains("       ^^"));
    }

    #[test]
    fn line_col_clamps_out_of_range_and_mid_utf8_offsets() {
        let file = SourceFile::new(FileId(0), "u.txt", "é");
        // 'é' is 2 bytes: offset 1 is mid-char, offset 99 is past EOF
        assert_eq!(file.line_col(1), LineCol { line: 1, column: 1 });
        assert_eq!(file.line_col(99), LineCol { line: 1, column: 2 });
    }

    proptest! {
        #[test]
        fn line_col_never_panics_for_any_text_and_offset(
            src in "\\PC{0,128}",
            offset in 0u32..256,
        ) {
            let file = SourceFile::new(FileId(0), "fuzz.txt", src);
            let lc = file.line_col(offset);
            assert!(lc.line >= 1);
            assert!(lc.column >= 1);
        }
    }
}
