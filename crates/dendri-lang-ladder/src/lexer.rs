//! UTF-8-safe cursor scanner and verbatim comment extractor for IEC 61131-3 Ladder text.
//!
//! Built directly on `dendri_frontend_core::{Cursor, LineIndex}` and `lex_nested_paren_comment` /
//! `lex_slash_line_comment` with zero unchecked indexing or panics.

use dendri_ast::{CommentStyle, LadderCoilKind, LadderContactKind};
use dendri_frontend_core::{
    Cursor, LineIndex, RawComment, is_ident_continue, is_ident_start, lex_nested_paren_comment,
    lex_slash_line_comment, merge_contiguous_leading_comments, normalize_eol_to_lf,
};
use dendri_span::{Diagnostic, ErrorCode, Span};

/// Zero-copy Ladder source scanner tracking verbatim leading/trailing comments and byte spans.
pub(crate) struct LadderScanner<'a> {
    pub(crate) cursor: Cursor<'a>,
    pub(crate) lines: LineIndex,
    last_code_end_line: Option<u32>,
    pending_leading: Vec<RawComment>,
    board_id: String,
}

impl<'a> LadderScanner<'a> {
    #[must_use]
    pub(crate) fn new(src: &'a str, board_id: impl Into<String>) -> Self {
        Self {
            cursor: Cursor::new(src),
            lines: LineIndex::new(src),
            last_code_end_line: None,
            pending_leading: Vec::new(),
            board_id: board_id.into(),
        }
    }

    #[must_use]
    pub(crate) fn pos(&self) -> usize {
        self.cursor.pos()
    }

    #[must_use]
    pub(crate) fn pos_u32(&self) -> u32 {
        self.cursor.pos_u32()
    }

    #[must_use]
    pub(crate) const fn is_eof(&self) -> bool {
        self.cursor.is_eof()
    }

    #[must_use]
    pub(crate) fn peek(&self) -> Option<char> {
        self.cursor.peek()
    }

    #[must_use]
    pub(crate) fn starts_with(&self, s: &str) -> bool {
        self.cursor.starts_with(s)
    }

    #[must_use]
    pub(crate) fn span_from(&self, start: usize) -> Span {
        self.cursor.span_from(start)
    }

    /// Marks the current cursor position as the end of an executable token for trailing-comment tracking.
    pub(crate) fn mark_code_end(&mut self) {
        let byte_pos = self.cursor.pos_u32().saturating_sub(1);
        self.last_code_end_line = Some(self.lines.line_of(byte_pos));
    }

    /// Skips whitespace and lexes any `//` or nested `(* ... *)` comments into `pending_leading`.
    pub(crate) fn skip_trivia(&mut self) -> Result<(), Diagnostic> {
        loop {
            let blank_lines = self.cursor.skip_whitespace_and_count_blank_lines();
            if self.cursor.starts_with("//") {
                if let Some(raw) = self.lex_ladder_line_comment(blank_lines) {
                    self.pending_leading.push(raw);
                    continue;
                }
            } else if self.cursor.starts_with("(*") {
                let start_pos = self.cursor.pos_u32();
                let start_line = self.lines.line_of(start_pos);
                let preceded_by_code = self.last_code_end_line == Some(start_line);
                let parsed =
                    lex_nested_paren_comment(&mut self.cursor, true).map_err(|mut d| {
                        d.board_id = Some(self.board_id.clone());
                        d
                    })?;
                if let Some((text, span)) = parsed {
                    let end_line = self.lines.line_of(span.end.saturating_sub(1));
                    self.pending_leading.push(RawComment {
                        text,
                        style: CommentStyle::ParenBlock,
                        span,
                        start_line,
                        end_line,
                        blank_lines_before: blank_lines,
                        preceded_by_code_on_line: preceded_by_code,
                    });
                    continue;
                }
            }
            break;
        }
        Ok(())
    }

    /// Collects any trailing comments located on the SAME line immediately after the last code token.
    pub(crate) fn take_same_line_trailing(&mut self) -> Result<Vec<RawComment>, Diagnostic> {
        let mut trailing = Vec::new();
        let Some(code_line) = self.last_code_end_line else {
            return Ok(trailing);
        };
        loop {
            let cp = self.cursor.checkpoint();
            let _ = self.cursor.skip_horizontal_whitespace();
            if self.cursor.starts_with("//") {
                let start_line = self.lines.line_of(self.cursor.pos_u32());
                if start_line == code_line
                    && let Some(raw) = self.lex_ladder_line_comment(0)
                {
                    trailing.push(raw);
                    continue;
                }
                self.cursor.restore(cp);
                break;
            }
            if self.cursor.starts_with("(*") {
                let start_pos = self.cursor.pos_u32();
                let start_line = self.lines.line_of(start_pos);
                if start_line == code_line {
                    let parsed =
                        lex_nested_paren_comment(&mut self.cursor, true).map_err(|mut d| {
                            d.board_id = Some(self.board_id.clone());
                            d
                        })?;
                    if let Some((text, span)) = parsed {
                        let end_line = self.lines.line_of(span.end.saturating_sub(1));
                        trailing.push(RawComment {
                            text,
                            style: CommentStyle::ParenBlock,
                            span,
                            start_line,
                            end_line,
                            blank_lines_before: 0,
                            preceded_by_code_on_line: true,
                        });
                        continue;
                    }
                }
                self.cursor.restore(cp);
                break;
            }
            self.cursor.restore(cp);
            break;
        }
        Ok(trailing)
    }

    /// Drains all buffered leading comments, merging contiguous same-style `//` lines per ADR-027.
    #[must_use]
    pub(crate) fn take_leading_comments(&mut self) -> Vec<RawComment> {
        let raw = std::mem::take(&mut self.pending_leading);
        merge_contiguous_leading_comments(raw)
    }

    fn lex_ladder_line_comment(&mut self, blank_lines_before: u32) -> Option<RawComment> {
        let start_pos = self.cursor.pos_u32();
        let start_line = self.lines.line_of(start_pos);
        let preceded_by_code = self.last_code_end_line == Some(start_line);
        let (body, _style, span) = lex_slash_line_comment(&mut self.cursor)?;
        // In IEC Ladder/ST, `//` is always `CommentStyle::Line`; preserve any leading `/` or `!` verbatim.
        let full_body = if span
            .slice(self.cursor.source())
            .is_some_and(|s| s.starts_with("///") && !s.starts_with("////"))
        {
            format!("/{body}")
        } else if span
            .slice(self.cursor.source())
            .is_some_and(|s| s.starts_with("//!"))
        {
            format!("!{body}")
        } else {
            body
        };
        let end_line = self.lines.line_of(span.end.saturating_sub(1));
        Some(RawComment {
            text: normalize_eol_to_lf(&full_body),
            style: CommentStyle::Line,
            span,
            start_line,
            end_line,
            blank_lines_before,
            preceded_by_code_on_line: preceded_by_code,
        })
    }

    /// Checks whether the next token matches `kw` case-insensitively without consuming it.
    #[must_use]
    pub(crate) fn peek_keyword(&self, kw: &str) -> bool {
        if !self.cursor.starts_with_ascii_ignore_case(kw) {
            return false;
        }
        let next_pos = self.cursor.pos().saturating_add(kw.len());
        let after = self
            .cursor
            .source()
            .get(next_pos..)
            .and_then(|s| s.chars().next());
        !after.is_some_and(is_ident_continue)
    }

    /// Consumes a case-insensitive keyword if present.
    pub(crate) fn eat_keyword(&mut self, kw: &str) -> Option<Span> {
        let sp = self.cursor.eat_keyword_ascii_ignore_case(kw)?;
        self.mark_code_end();
        Some(sp)
    }

    /// Requires a specific case-insensitive keyword or returns `ERR_TOML_SYNTAX`.
    pub(crate) fn expect_keyword(&mut self, kw: &str) -> Result<Span, Diagnostic> {
        self.skip_trivia()?;
        if let Some(sp) = self.eat_keyword(kw) {
            Ok(sp)
        } else {
            Err(self.syntax_error(format!("expected keyword `{kw}`")))
        }
    }

    /// Consumes exact punctuation `lit` if present.
    pub(crate) fn eat_punct(&mut self, lit: &str) -> bool {
        if self.cursor.eat_str(lit) {
            self.mark_code_end();
            true
        } else {
            false
        }
    }

    /// Requires exact punctuation `lit` or returns `ERR_TOML_SYNTAX`.
    pub(crate) fn expect_punct(&mut self, lit: &str) -> Result<Span, Diagnostic> {
        self.skip_trivia()?;
        let start = self.pos();
        if self.eat_punct(lit) {
            Ok(self.span_from(start))
        } else {
            Err(self.syntax_error(format!("expected `{lit}`")))
        }
    }

    /// Consumes an ASCII identifier (`[A-Za-z_][A-Za-z0-9_]*`).
    pub(crate) fn eat_ident(&mut self) -> Option<(&'a str, Span)> {
        let (name, span) = self.cursor.eat_ident()?;
        self.mark_code_end();
        Some((name, span))
    }

    /// Requires an ASCII identifier or returns `ERR_TOML_SYNTAX`.
    pub(crate) fn expect_ident(&mut self, desc: &str) -> Result<(String, Span), Diagnostic> {
        self.skip_trivia()?;
        if let Some((name, span)) = self.eat_ident() {
            Ok((name.to_owned(), span))
        } else {
            Err(self.syntax_error(format!("expected {desc}")))
        }
    }

    /// Parses a `NodeId` token (`[A-Za-z0-9_][A-Za-z0-9_.-]*`, stopping before `--`).
    pub(crate) fn expect_node_id(&mut self) -> Result<(String, Span), Diagnostic> {
        self.skip_trivia()?;
        let start = self.pos();
        let Some(first) = self.peek() else {
            return Err(self.syntax_error("expected NodeId, found end of input"));
        };
        if !(first.is_ascii_alphanumeric() || first == '_') {
            return Err(self.syntax_error(format!("expected NodeId, found `{first}`")));
        }
        while let Some(ch) = self.peek() {
            if ch == '-' && self.cursor.starts_with("--") {
                break;
            }
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                let _ = self.cursor.bump();
            } else {
                break;
            }
        }
        let span = self.span_from(start);
        let text = self.cursor.slice_from(start).to_owned();
        self.mark_code_end();
        Ok((text, span))
    }

    /// Parses a port ID or IEC direct address token (e.g. `%IX0.0`, `Ia.0`, `D13`, `IW64`).
    pub(crate) fn expect_port_or_iec(&mut self) -> Result<(String, Span), Diagnostic> {
        self.skip_trivia()?;
        let start = self.pos();
        let Some(first) = self.peek() else {
            return Err(self.syntax_error("expected port or IEC direct address"));
        };
        if !(first == '%' || first.is_ascii_alphanumeric() || first == '_') {
            return Err(self.syntax_error(format!(
                "expected port or IEC direct address, found `{first}`"
            )));
        }
        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphanumeric() || matches!(ch, '%' | '_' | '.' | '-' | '+' | '/') {
                if ch == '-' && self.cursor.starts_with("--") {
                    break;
                }
                let _ = self.cursor.bump();
            } else {
                break;
            }
        }
        let span = self.span_from(start);
        let text = self.cursor.slice_from(start).to_owned();
        if text.is_empty() {
            return Err(self.syntax_error("expected port or IEC direct address"));
        }
        self.mark_code_end();
        Ok((text, span))
    }

    /// Parses an import path (e.g. `std.iec.timers` or `std.iec.*`).
    pub(crate) fn expect_import_path(&mut self) -> Result<(String, Span), Diagnostic> {
        self.skip_trivia()?;
        let start = self.pos();
        let Some(first) = self.peek() else {
            return Err(self.syntax_error("expected module import path"));
        };
        if !is_ident_start(first) {
            return Err(self.syntax_error(format!("expected module import path, found `{first}`")));
        }
        while let Some(ch) = self.peek() {
            if is_ident_continue(ch) || matches!(ch, '.' | '*') {
                let _ = self.cursor.bump();
            } else {
                break;
            }
        }
        let span = self.span_from(start);
        let text = self.cursor.slice_from(start).to_owned();
        self.mark_code_end();
        Ok((text, span))
    }

    /// Parses an unsigned decimal integer (`u64`).
    pub(crate) fn expect_u64(&mut self, desc: &str) -> Result<(u64, Span), Diagnostic> {
        self.skip_trivia()?;
        let (digits, span) = self.cursor.eat_while_spanned(|c| c.is_ascii_digit());
        if digits.is_empty() {
            return Err(self.syntax_error(format!("expected {desc}")));
        }
        let val = digits
            .parse::<u64>()
            .map_err(|_| self.syntax_error_at(span, format!("integer out of range for {desc}")))?;
        self.mark_code_end();
        Ok((val, span))
    }

    /// Parses an unsigned 32-bit decimal integer (`u32`).
    pub(crate) fn expect_u32(&mut self, desc: &str) -> Result<(u32, Span), Diagnostic> {
        let (val, span) = self.expect_u64(desc)?;
        let narrow = u32::try_from(val)
            .map_err(|_| self.syntax_error_at(span, format!("integer out of u32 range: {val}")))?;
        Ok((narrow, span))
    }

    /// Parses a signed 64-bit decimal integer (`i64`).
    pub(crate) fn expect_i64(&mut self, desc: &str) -> Result<(i64, Span), Diagnostic> {
        self.skip_trivia()?;
        let start = self.pos();
        let _ = self.cursor.eat_char('-');
        let digits = self.cursor.eat_while(|c| c.is_ascii_digit());
        if digits.is_empty() {
            return Err(self.syntax_error(format!("expected {desc}")));
        }
        let span = self.span_from(start);
        let raw = self.cursor.slice_from(start);
        let val = raw
            .parse::<i64>()
            .map_err(|_| self.syntax_error_at(span, format!("integer out of i64 range: {raw}")))?;
        self.mark_code_end();
        Ok((val, span))
    }

    /// Parses a floating-point literal (`f64`, optional leading `-`, stopping before `..`).
    pub(crate) fn expect_f64(&mut self, desc: &str) -> Result<(f64, Span), Diagnostic> {
        self.skip_trivia()?;
        let start = self.pos();
        let _ = self.cursor.eat_char('-');
        let int_digits = self.cursor.eat_while(|c| c.is_ascii_digit());
        if int_digits.is_empty() {
            return Err(self.syntax_error(format!("expected {desc}")));
        }
        if self.cursor.peek() == Some('.') && !self.cursor.starts_with("..") {
            let _ = self.cursor.bump();
            let _ = self.cursor.eat_while(|c| c.is_ascii_digit());
        }
        if matches!(self.cursor.peek(), Some('e' | 'E')) {
            let _ = self.cursor.bump();
            if matches!(self.cursor.peek(), Some('+' | '-')) {
                let _ = self.cursor.bump();
            }
            let _ = self.cursor.eat_while(|c| c.is_ascii_digit());
        }
        let span = self.span_from(start);
        let raw = self.cursor.slice_from(start);
        let val = raw
            .parse::<f64>()
            .map_err(|_| self.syntax_error_at(span, format!("invalid float `{raw}` for {desc}")))?;
        self.mark_code_end();
        Ok((val, span))
    }

    /// Parses a double-quoted UTF-8 string literal supporting `\n`, `\r`, `\t`, `\0`, `\"`, `\\`, `\u{...}`.
    pub(crate) fn expect_string_lit(&mut self) -> Result<(String, Span), Diagnostic> {
        self.skip_trivia()?;
        let start = self.pos();
        if !self.cursor.eat_char('"') {
            return Err(self.syntax_error("expected string literal"));
        }
        let mut out = String::new();
        while let Some(ch) = self.cursor.bump() {
            match ch {
                '"' => {
                    let span = self.span_from(start);
                    self.mark_code_end();
                    return Ok((out, span));
                }
                '\\' => {
                    let Some(esc) = self.cursor.bump() else {
                        return Err(self.syntax_error_at(
                            self.span_from(start),
                            "unterminated escape sequence in string literal",
                        ));
                    };
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        '0' => out.push('\0'),
                        'u' => {
                            if !self.cursor.eat_char('{') {
                                return Err(self.syntax_error_at(
                                    self.span_from(start),
                                    "expected `{` after `\\u` escape",
                                ));
                            }
                            let hex = self.cursor.eat_while(|c| c.is_ascii_hexdigit());
                            if !self.cursor.eat_char('}') || hex.is_empty() {
                                return Err(self.syntax_error_at(
                                    self.span_from(start),
                                    "malformed `\\u{...}` escape in string literal",
                                ));
                            }
                            let cp = u32::from_str_radix(hex, 16).map_err(|_| {
                                self.syntax_error_at(
                                    self.span_from(start),
                                    "invalid hex codepoint in `\\u{...}` escape",
                                )
                            })?;
                            let decoded = char::from_u32(cp).ok_or_else(|| {
                                self.syntax_error_at(
                                    self.span_from(start),
                                    "invalid Unicode scalar value in `\\u{...}` escape",
                                )
                            })?;
                            out.push(decoded);
                        }
                        other => {
                            return Err(self.syntax_error_at(
                                self.span_from(start),
                                format!("unknown escape sequence `\\{other}` in string literal"),
                            ));
                        }
                    }
                }
                c => out.push(c),
            }
        }
        Err(self.syntax_error_at(self.span_from(start), "unterminated string literal"))
    }

    /// Attempts to consume a Ladder contact glyph (`| |`, `|/|`, `|P|`, `|N|`).
    ///
    /// Never consumes `||` (ParallelOr branch separator).
    pub(crate) fn try_eat_contact(&mut self) -> Option<(LadderContactKind, Span)> {
        if self.peek() != Some('|') || self.starts_with("||") {
            return None;
        }
        let cp = self.cursor.checkpoint();
        let start = self.pos();
        let _ = self.cursor.bump(); // '|'
        let kind = match self.peek() {
            Some(' ') => {
                let _ = self.cursor.eat_while(|c| c == ' ');
                if !self.cursor.eat_char('|') {
                    self.cursor.restore(cp);
                    return None;
                }
                LadderContactKind::NormallyOpen
            }
            Some('/') => {
                let _ = self.cursor.bump();
                if !self.cursor.eat_char('|') {
                    self.cursor.restore(cp);
                    return None;
                }
                LadderContactKind::NormallyClosed
            }
            Some('P' | 'p') => {
                let _ = self.cursor.bump();
                if !self.cursor.eat_char('|') {
                    self.cursor.restore(cp);
                    return None;
                }
                LadderContactKind::PositiveTransition
            }
            Some('N' | 'n') => {
                let _ = self.cursor.bump();
                if !self.cursor.eat_char('|') {
                    self.cursor.restore(cp);
                    return None;
                }
                LadderContactKind::NegativeTransition
            }
            _ => {
                self.cursor.restore(cp);
                return None;
            }
        };
        let span = self.span_from(start);
        self.mark_code_end();
        Some((kind, span))
    }

    /// Attempts to consume a Ladder coil glyph (`( )`, `(/)`, `(S)`, `(R)`, `(P)`, `(N)`).
    pub(crate) fn try_eat_coil(&mut self) -> Option<(LadderCoilKind, Span)> {
        if self.peek() != Some('(') || self.starts_with("(*") {
            return None;
        }
        let cp = self.cursor.checkpoint();
        let start = self.pos();
        let _ = self.cursor.bump(); // '('
        let kind = match self.peek() {
            Some(' ') => {
                let _ = self.cursor.eat_while(|c| c == ' ');
                if !self.cursor.eat_char(')') {
                    self.cursor.restore(cp);
                    return None;
                }
                LadderCoilKind::Normal
            }
            Some('/') => {
                let _ = self.cursor.bump();
                if !self.cursor.eat_char(')') {
                    self.cursor.restore(cp);
                    return None;
                }
                LadderCoilKind::Negated
            }
            Some('S' | 's') => {
                let _ = self.cursor.bump();
                if !self.cursor.eat_char(')') {
                    self.cursor.restore(cp);
                    return None;
                }
                LadderCoilKind::Set
            }
            Some('R' | 'r') => {
                let _ = self.cursor.bump();
                if !self.cursor.eat_char(')') {
                    self.cursor.restore(cp);
                    return None;
                }
                LadderCoilKind::Reset
            }
            Some('P' | 'p') => {
                let _ = self.cursor.bump();
                if !self.cursor.eat_char(')') {
                    self.cursor.restore(cp);
                    return None;
                }
                LadderCoilKind::PositiveTransition
            }
            Some('N' | 'n') => {
                let _ = self.cursor.bump();
                if !self.cursor.eat_char(')') {
                    self.cursor.restore(cp);
                    return None;
                }
                LadderCoilKind::NegativeTransition
            }
            _ => {
                self.cursor.restore(cp);
                return None;
            }
        };
        let span = self.span_from(start);
        self.mark_code_end();
        Some((kind, span))
    }

    /// Scans an `LValue` slice (`ident`, `ident.field`, `ident[expr].field`, etc.)
    /// respecting balanced brackets and string literals.
    pub(crate) fn scan_lvalue_slice(&mut self) -> Result<(&'a str, usize, Span), Diagnostic> {
        self.skip_trivia()?;
        let start = self.pos();
        if self.cursor.eat_ident().is_none() {
            return Err(self.syntax_error("expected LValue identifier"));
        }
        loop {
            if self.cursor.peek() == Some('.') {
                let cp = self.cursor.checkpoint();
                let _ = self.cursor.bump();
                if self.cursor.eat_ident().is_none() {
                    self.cursor.restore(cp);
                    break;
                }
            } else if self.cursor.peek() == Some('[') {
                self.consume_balanced_delim('[', ']')?;
            } else {
                break;
            }
        }
        let span = self.span_from(start);
        let text = self.cursor.slice_from(start);
        self.mark_code_end();
        Ok((text, start, span))
    }

    /// Scans an expression slice at nesting depth 0 until one of `stop_chars`, `stop_keywords`,
    /// or `..` (when `stop_dotdot` is `true`) is encountered.
    pub(crate) fn scan_expr_slice(
        &mut self,
        stop_chars: &[char],
        stop_keywords: &[&str],
        stop_dotdot: bool,
    ) -> Result<(&'a str, usize, Span), Diagnostic> {
        self.skip_trivia()?;
        let start = self.pos();
        let mut paren_depth = 0usize;
        let mut bracket_depth = 0usize;
        let mut brace_depth = 0usize;

        while let Some(ch) = self.peek() {
            if ch == '"' {
                self.consume_raw_string()?;
                continue;
            }
            if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 {
                if self.starts_with("//") || self.starts_with("(*") {
                    break;
                }
                if stop_dotdot && self.starts_with("..") {
                    break;
                }
                if stop_chars.contains(&ch) {
                    break;
                }
                if is_ident_start(ch) && stop_keywords.iter().any(|kw| self.peek_keyword(kw)) {
                    break;
                }
            }
            match ch {
                '(' => {
                    paren_depth = paren_depth.saturating_add(1);
                    let _ = self.cursor.bump();
                }
                ')' => {
                    if let Some(next) = paren_depth.checked_sub(1) {
                        paren_depth = next;
                        let _ = self.cursor.bump();
                    } else {
                        break;
                    }
                }
                '[' => {
                    bracket_depth = bracket_depth.saturating_add(1);
                    let _ = self.cursor.bump();
                }
                ']' => {
                    if let Some(next) = bracket_depth.checked_sub(1) {
                        bracket_depth = next;
                        let _ = self.cursor.bump();
                    } else {
                        break;
                    }
                }
                '{' => {
                    brace_depth = brace_depth.saturating_add(1);
                    let _ = self.cursor.bump();
                }
                '}' => {
                    if let Some(next) = brace_depth.checked_sub(1) {
                        brace_depth = next;
                        let _ = self.cursor.bump();
                    } else {
                        break;
                    }
                }
                _ => {
                    let _ = self.cursor.bump();
                }
            }
        }

        if paren_depth > 0 || bracket_depth > 0 || brace_depth > 0 {
            return Err(self.syntax_error_at(
                self.span_from(start),
                "unclosed delimiter inside Ladder expression",
            ));
        }

        let raw = self.cursor.slice_from(start);
        let trimmed = raw.trim_end();
        if trimmed.is_empty() {
            return Err(self.syntax_error("expected expression"));
        }
        let end_u32 = u32::try_from(start.saturating_add(trimmed.len())).unwrap_or(u32::MAX);
        let start_u32 = u32::try_from(start).unwrap_or(u32::MAX);
        let span = Span::new(start_u32, end_u32);
        self.last_code_end_line = Some(self.lines.line_of(end_u32.saturating_sub(1)));
        Ok((trimmed, start, span))
    }

    fn consume_balanced_delim(&mut self, open: char, close: char) -> Result<(), Diagnostic> {
        let start = self.pos();
        if !self.cursor.eat_char(open) {
            return Err(self.syntax_error(format!("expected `{open}`")));
        }
        let mut depth = 1usize;
        while let Some(ch) = self.peek() {
            if ch == '"' {
                self.consume_raw_string()?;
                continue;
            }
            let _ = self.cursor.bump();
            if ch == open {
                depth = depth.saturating_add(1);
            } else if ch == close {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(());
                }
            }
        }
        Err(self.syntax_error_at(
            self.span_from(start),
            format!("unclosed `{open}...{close}` delimiter"),
        ))
    }

    fn consume_raw_string(&mut self) -> Result<(), Diagnostic> {
        let start = self.pos();
        let _ = self.cursor.bump(); // opening '"'
        while let Some(ch) = self.cursor.bump() {
            match ch {
                '"' => return Ok(()),
                '\\' if self.cursor.bump().is_none() => break,
                _ => {}
            }
        }
        Err(self.syntax_error_at(self.span_from(start), "unterminated string literal"))
    }

    #[must_use]
    pub(crate) fn syntax_error(&self, msg: impl Into<String>) -> Diagnostic {
        let p = self.pos_u32();
        self.syntax_error_at(Span::point(p), msg)
    }

    #[must_use]
    pub(crate) fn syntax_error_at(&self, span: Span, msg: impl Into<String>) -> Diagnostic {
        let mut diag = Diagnostic::new(ErrorCode::ErrTomlSyntax, msg).with_board_id(&self.board_id);
        diag.span = Some(span);
        diag
    }

    #[must_use]
    pub(crate) fn unknown_field_error(&self, span: Span, field: &str) -> Diagnostic {
        let mut diag = Diagnostic::new(
            ErrorCode::ErrUnknownField,
            format!("unknown Ladder directive or attribute `{field}`"),
        )
        .with_board_id(&self.board_id);
        diag.span = Some(span);
        diag
    }
}
