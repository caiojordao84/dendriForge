//! Recursive-descent parser for IEC 60848 / IEC 61131-3 GRAFCET / SFC
//! (`docs/lbc/sfc.md` §1.1, `04-frontends-visual-and-sync.md` §5.2).
//!
//! Uses `dendri_frontend_core::pratt::parse_pratt` for expression parsing with
//! strict non-chaining comparison checks and bounded recursion depth.
//! Produces first-class `PouBody::Chart(SfcChart)` with ZERO synthetic `__sfc_*`
//! temporaries stored in ASL.

use crate::lexer::{Token, TokenKind, tokenize};
use crate::validate::validate_parsed_program;
use dendri_ast::{
    ActionQualifier, AuthoringLanguage, BinaryOp, BoardPresentation, BoardProgram, CallArg,
    CasePattern, ChannelRange, Comment, CommentAnchor, CommentOrigin,
    CommentPlacement as AstCommentPlacement, DataDecl, ElifArm, EnumVariant, Expr, ExprKind,
    FbKind, ForeignFidelity, HardwareBinding, HardwareKind, InterruptEdge, LValue, Literal,
    ModuleDecl, NodeId, ObKind, ParamDecl, ParamDirection, Pou, PouBody, PouKind, PullMode,
    ScalarType, SfcAction, SfcChart, SfcStep, SfcTransition, SignalKind, Stmt, StmtKind,
    StorageScope, StructField, SwitchCase, TypeDecl, TypeRef, UnaryOp,
};
use dendri_frontend_core::comments::RawComment;
use dendri_frontend_core::cursor::detect_eol_style;
use dendri_frontend_core::foreign::resolve_foreign_fidelity;
use dendri_frontend_core::pratt::{Associativity, InfixSpec, PrattDriver, PrefixSpec, parse_pratt};
use dendri_frontend_core::traits::{
    BoardContext, BoardProgramBundle, FrontendOutput, FrontendResult, LanguageId,
};
use dendri_span::{Diagnostic, ErrorCode, Span};
use std::collections::BTreeSet;

/// Parses IEC 60848 / IEC 61131-3 textual GRAFCET/SFC source into a canonical [`BoardProgramBundle`].
pub(crate) fn parse_sfc(src: &str, ctx: &BoardContext) -> FrontendResult<BoardProgramBundle> {
    let eol_style = detect_eol_style(src);
    let lexed = tokenize(src, &ctx.board_id)?;

    let mut parser = Parser::new(lexed.tokens, lexed.comments, ctx);
    let program = parser.parse_program()?;

    // Attach any trailing/unanchored comments at EOF so C1 never drops commentary
    parser.attach_eof_comments(&program);

    // Run static SFC semantic validation per `docs/lbc/sfc.md` §1.5
    let validation_diags = validate_parsed_program(&program, ctx);
    if !validation_diags.is_empty() {
        return Err(validation_diags);
    }

    let bundle = BoardProgramBundle {
        program,
        comments: parser.anchored_comments,
        assumptions: Vec::new(),
        presentation: BoardPresentation {
            eol_style,
            ..BoardPresentation::default()
        },
    };

    Ok(FrontendOutput::with_diagnostics(bundle, parser.warnings))
}

struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    raw_comments: Vec<RawComment>,
    comment_pos: usize,
    anchored_comments: Vec<Comment>,
    next_node_id: u32,
    next_comment_id: u32,
    ctx: &'a BoardContext,
    fb_vars: BTreeSet<String>,
    warnings: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    fn new(tokens: Vec<Token>, raw_comments: Vec<RawComment>, ctx: &'a BoardContext) -> Self {
        Self {
            tokens,
            pos: 0,
            raw_comments,
            comment_pos: 0,
            anchored_comments: Vec::new(),
            next_node_id: 1,
            next_comment_id: 1,
            ctx,
            fb_vars: BTreeSet::new(),
            warnings: Vec::new(),
        }
    }

    fn alloc_node_id(&mut self) -> NodeId {
        let id = NodeId::new(format!("n_{:04}", self.next_node_id));
        self.next_node_id = self.next_node_id.saturating_add(1);
        id
    }

    fn alloc_comment_id(&mut self) -> String {
        let id = format!("c_{:04}", self.next_comment_id);
        self.next_comment_id = self.next_comment_id.saturating_add(1);
        id
    }

    fn peek(&self) -> &Token {
        static EOF_TOK: Token = Token {
            kind: TokenKind::Eof,
            span: Span::point(0),
            start_line: 1,
            end_line: 1,
        };
        self.tokens
            .get(self.pos)
            .or_else(|| self.tokens.last())
            .unwrap_or(&EOF_TOK)
    }

    fn peek_offset(&self, delta: usize) -> Option<&Token> {
        self.tokens.get(self.pos.saturating_add(delta))
    }

    fn bump(&mut self) -> Token {
        let tok = self.peek().clone();
        if self.pos < self.tokens.len() {
            self.pos = self.pos.saturating_add(1);
        }
        tok
    }

    fn is_eof(&self) -> bool {
        self.peek().kind == TokenKind::Eof
    }

    fn eat_kw(&mut self, kw: &str) -> Option<Token> {
        if self.peek().is_kw(kw) {
            Some(self.bump())
        } else {
            None
        }
    }

    fn syntax_err(&self, span: Span, msg: impl Into<String>) -> Vec<Diagnostic> {
        let mut d =
            Diagnostic::new(ErrorCode::ErrTomlSyntax, msg).with_board_id(&self.ctx.board_id);
        d.span = Some(span);
        vec![d]
    }

    fn unknown_field_err(&self, span: Span, msg: impl Into<String>) -> Vec<Diagnostic> {
        let mut d =
            Diagnostic::new(ErrorCode::ErrUnknownField, msg).with_board_id(&self.ctx.board_id);
        d.span = Some(span);
        vec![d]
    }

    fn expect_kw(&mut self, kw: &str) -> Result<Token, Vec<Diagnostic>> {
        if self.peek().is_kw(kw) {
            Ok(self.bump())
        } else {
            let tok = self.peek().clone();
            Err(self.syntax_err(tok.span, format!("expected `{kw}`, found `{:?}`", tok.kind)))
        }
    }

    fn expect_kind(
        &mut self,
        expected: &TokenKind,
        desc: &'static str,
    ) -> Result<Token, Vec<Diagnostic>> {
        if &self.peek().kind == expected {
            Ok(self.bump())
        } else {
            let tok = self.peek().clone();
            Err(self.syntax_err(tok.span, format!("expected {desc}, found `{:?}`", tok.kind)))
        }
    }

    fn expect_ident(&mut self, desc: &'static str) -> Result<(String, Span), Vec<Diagnostic>> {
        let tok = self.peek().clone();
        if let TokenKind::Ident(name) = tok.kind {
            let span = tok.span;
            self.bump();
            Ok((name, span))
        } else {
            Err(self.syntax_err(tok.span, format!("expected {desc}, found `{:?}`", tok.kind)))
        }
    }

    fn expect_string(&mut self, desc: &'static str) -> Result<(String, Span), Vec<Diagnostic>> {
        let tok = self.peek().clone();
        if let TokenKind::String(s) = tok.kind {
            let span = tok.span;
            self.bump();
            Ok((s, span))
        } else {
            Err(self.syntax_err(tok.span, format!("expected {desc}, found `{:?}`", tok.kind)))
        }
    }

    fn expect_bool(&mut self, desc: &'static str) -> Result<(bool, Span), Vec<Diagnostic>> {
        let tok = self.peek().clone();
        if let TokenKind::Bool(b) = tok.kind {
            let span = tok.span;
            self.bump();
            Ok((b, span))
        } else {
            Err(self.syntax_err(tok.span, format!("expected {desc}, found `{:?}`", tok.kind)))
        }
    }

    fn expect_u64(&mut self, desc: &'static str) -> Result<(u64, Span), Vec<Diagnostic>> {
        let tok = self.peek().clone();
        if let TokenKind::Int(n) = tok.kind
            && let Ok(v) = u64::try_from(n)
        {
            let span = tok.span;
            self.bump();
            return Ok((v, span));
        }
        Err(self.syntax_err(
            tok.span,
            format!(
                "expected non-negative integer for {desc}, found `{:?}`",
                tok.kind
            ),
        ))
    }

    fn expect_f64(&mut self, desc: &'static str) -> Result<(f64, Span), Vec<Diagnostic>> {
        let neg = if self.peek().kind == TokenKind::Minus {
            self.bump();
            true
        } else {
            false
        };
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::Float(f) => {
                self.bump();
                Ok((if neg { -f } else { f }, tok.span))
            }
            TokenKind::Int(n) => {
                self.bump();
                let f = n as f64;
                Ok((if neg { -f } else { f }, tok.span))
            }
            TokenKind::Bool(_)
            | TokenKind::TimeUs(_)
            | TokenKind::String(_)
            | TokenKind::DirectAddr(_)
            | TokenKind::Ident(_)
            | TokenKind::LParen
            | TokenKind::RParen
            | TokenKind::LBracket
            | TokenKind::RBracket
            | TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::Comma
            | TokenKind::Dot
            | TokenKind::DotDot
            | TokenKind::Colon
            | TokenKind::ColonEq
            | TokenKind::Semi
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::Eq
            | TokenKind::EqEq
            | TokenKind::Ne
            | TokenKind::BangEq
            | TokenKind::Eof => Err(self.syntax_err(
                tok.span,
                format!("expected number for {desc}, found `{:?}`", tok.kind),
            )),
        }
    }

    // -----------------------------------------------------------------------
    // ADR-027 Comment Attachment
    // -----------------------------------------------------------------------

    fn attach_comments_before(
        &mut self,
        byte_offset: u32,
        anchor: &NodeId,
        placement: AstCommentPlacement,
    ) {
        while let Some(rc) = self.raw_comments.get(self.comment_pos).cloned() {
            if rc.span.end <= byte_offset {
                self.comment_pos = self.comment_pos.saturating_add(1);
                let id = self.alloc_comment_id();
                self.anchored_comments.push(Comment {
                    id,
                    anchor: CommentAnchor::Node { id: anchor.clone() },
                    placement,
                    token_ordinal: None,
                    text: rc.text,
                    origin: CommentOrigin {
                        language: LanguageId::Sfc.as_str().to_owned(),
                        style: rc.style,
                    },
                    span: rc.span,
                });
            } else {
                break;
            }
        }
    }

    fn attach_inner_and_trailing_comments(
        &mut self,
        node_end_byte: u32,
        node_end_line: u32,
        anchor: &NodeId,
    ) {
        let mut inner_ord = 1u32;
        while let Some(rc) = self.raw_comments.get(self.comment_pos).cloned() {
            if rc.span.end <= node_end_byte {
                // Comment appeared strictly inside this node's tokens
                self.comment_pos = self.comment_pos.saturating_add(1);
                let id = self.alloc_comment_id();
                self.anchored_comments.push(Comment {
                    id,
                    anchor: CommentAnchor::Node { id: anchor.clone() },
                    placement: AstCommentPlacement::Inner,
                    token_ordinal: Some(inner_ord),
                    text: rc.text,
                    origin: CommentOrigin {
                        language: LanguageId::Sfc.as_str().to_owned(),
                        style: rc.style,
                    },
                    span: rc.span,
                });
                inner_ord = inner_ord.saturating_add(1);
            } else if rc.preceded_by_code_on_line && rc.start_line == node_end_line {
                // Comment appeared on the same line immediately after this node
                self.comment_pos = self.comment_pos.saturating_add(1);
                let id = self.alloc_comment_id();
                self.anchored_comments.push(Comment {
                    id,
                    anchor: CommentAnchor::Node { id: anchor.clone() },
                    placement: AstCommentPlacement::Trailing,
                    token_ordinal: None,
                    text: rc.text,
                    origin: CommentOrigin {
                        language: LanguageId::Sfc.as_str().to_owned(),
                        style: rc.style,
                    },
                    span: rc.span,
                });
            } else {
                break;
            }
        }
    }

    fn attach_eof_comments(&mut self, program: &BoardProgram) {
        let fallback_anchor = program
            .pous
            .last()
            .map(|p| CommentAnchor::Node { id: p.id.clone() })
            .or_else(|| {
                program
                    .modules
                    .last()
                    .map(|m| CommentAnchor::Node { id: m.id.clone() })
            })
            .or_else(|| {
                program
                    .data
                    .last()
                    .map(|d| CommentAnchor::Node { id: d.id.clone() })
            })
            .or_else(|| {
                program
                    .hardware
                    .last()
                    .map(|h| CommentAnchor::Node { id: h.id.clone() })
            });

        while let Some(rc) = self.raw_comments.get(self.comment_pos).cloned() {
            self.comment_pos = self.comment_pos.saturating_add(1);
            let id = self.alloc_comment_id();
            let (anchor, placement) = match &fallback_anchor {
                Some(node_anchor) => (node_anchor.clone(), AstCommentPlacement::Dangling),
                None => (
                    CommentAnchor::Board {
                        id: self.ctx.board_id.clone(),
                    },
                    AstCommentPlacement::Header,
                ),
            };
            self.anchored_comments.push(Comment {
                id,
                anchor,
                placement,
                token_ordinal: None,
                text: rc.text,
                origin: CommentOrigin {
                    language: LanguageId::Sfc.as_str().to_owned(),
                    style: rc.style,
                },
                span: rc.span,
            });
        }
    }

    // -----------------------------------------------------------------------
    // Top-Level Program & Module Parsing
    // -----------------------------------------------------------------------

    fn parse_program(&mut self) -> Result<BoardProgram, Vec<Diagnostic>> {
        let mut program = BoardProgram::default();

        while !self.is_eof() {
            if self.peek().is_kw("IMPORT") {
                let imp = self.parse_import()?;
                program.imports.push(imp);
            } else if self.peek().is_kw("TYPE") {
                let mut tys = self.parse_type_block()?;
                program.types.append(&mut tys);
            } else if self.peek().is_kw("FOREIGN_TYPE") {
                let ty = self.parse_foreign_type_decl()?;
                program.types.push(ty);
            } else if self.peek().is_kw("VAR_GLOBAL") {
                self.parse_var_global_block(&mut program.hardware, &mut program.data)?;
            } else if self.peek().is_kw("MODULE") {
                let m = self.parse_module()?;
                program.modules.push(m);
            } else if self.peek().is_kw("SFC_CHART") || self.peek().is_kw("CHART") {
                let pou = self.parse_sfc_chart_pou()?;
                program.pous.push(pou);
            } else if self.peek().is_kw("PROGRAM")
                || self.peek().is_kw("FUNCTION")
                || self.peek().is_kw("FUNCTION_BLOCK")
            {
                let pou = self.parse_stmt_pou()?;
                program.pous.push(pou);
            } else {
                let tok = self.peek().clone();
                return Err(self.syntax_err(
                    tok.span,
                    format!(
                        "unexpected token `{:?}` at top level (expected `SFC_CHART`, `CHART`, `VAR_GLOBAL`, `TYPE`, `IMPORT`, `MODULE`, `PROGRAM`, `FUNCTION`, or `FUNCTION_BLOCK`)",
                        tok.kind
                    ),
                ));
            }
        }

        Ok(program)
    }

    fn parse_import(&mut self) -> Result<String, Vec<Diagnostic>> {
        let _ = self.expect_kw("IMPORT")?;
        let (first, _) = self.expect_ident("import path")?;
        let mut path = first;
        while self.peek().kind == TokenKind::Dot {
            self.bump();
            if self.peek().kind == TokenKind::Star {
                self.bump();
                path.push_str(".*");
                break;
            }
            let (seg, _) = self.expect_ident("import path segment")?;
            path.push('.');
            path.push_str(&seg);
        }
        let _ = self.expect_kind(&TokenKind::Semi, "`;`")?;
        Ok(path)
    }

    fn parse_module(&mut self) -> Result<ModuleDecl, Vec<Diagnostic>> {
        let start_tok = self.expect_kw("MODULE")?;
        let id = self.alloc_node_id();
        self.attach_comments_before(start_tok.span.start, &id, AstCommentPlacement::Leading);
        let (name, _) = self.expect_ident("module name")?;
        self.attach_inner_and_trailing_comments(start_tok.span.end, start_tok.end_line, &id);

        let mut imports = Vec::new();
        let mut exports = Vec::new();
        let mut types = Vec::new();
        let mut data = Vec::new();
        let mut pous = Vec::new();
        let mut dummy_hw = Vec::new();

        while !self.is_eof() && !self.peek().is_kw("END_MODULE") {
            if self.peek().is_kw("IMPORT") {
                imports.push(self.parse_import()?);
            } else if self.peek().is_kw("EXPORT") {
                self.bump();
                let (exp, _) = self.expect_ident("exported symbol name")?;
                let _ = self.expect_kind(&TokenKind::Semi, "`;`")?;
                exports.push(exp);
            } else if self.peek().is_kw("TYPE") {
                let mut tys = self.parse_type_block()?;
                types.append(&mut tys);
            } else if self.peek().is_kw("FOREIGN_TYPE") {
                types.push(self.parse_foreign_type_decl()?);
            } else if self.peek().is_kw("VAR_GLOBAL") {
                self.parse_var_global_block(&mut dummy_hw, &mut data)?;
            } else if self.peek().is_kw("SFC_CHART") || self.peek().is_kw("CHART") {
                pous.push(self.parse_sfc_chart_pou()?);
            } else if self.peek().is_kw("PROGRAM")
                || self.peek().is_kw("FUNCTION")
                || self.peek().is_kw("FUNCTION_BLOCK")
            {
                pous.push(self.parse_stmt_pou()?);
            } else {
                let tok = self.peek().clone();
                return Err(self.syntax_err(
                    tok.span,
                    format!("unexpected token `{:?}` inside MODULE `{name}`", tok.kind),
                ));
            }
        }

        let end_tok = self.expect_kw("END_MODULE")?;
        self.attach_comments_before(end_tok.span.start, &id, AstCommentPlacement::Dangling);
        Ok(ModuleDecl {
            id,
            name,
            imports,
            exports,
            types,
            data,
            pous,
            span: start_tok.span.merge(end_tok.span),
        })
    }

    // -----------------------------------------------------------------------
    // TYPE / FOREIGN_TYPE Parsing
    // -----------------------------------------------------------------------

    fn parse_type_block(&mut self) -> Result<Vec<TypeDecl>, Vec<Diagnostic>> {
        let _ = self.expect_kw("TYPE")?;
        let mut decls = Vec::new();
        while !self.is_eof() && !self.peek().is_kw("END_TYPE") {
            let start_tok = self.peek().clone();
            let id = self.alloc_node_id();
            self.attach_comments_before(start_tok.span.start, &id, AstCommentPlacement::Leading);
            let (name, name_span) = self.expect_ident("type name")?;
            let _ = self.expect_kind(&TokenKind::Colon, "`:`")?;

            if self.peek().is_kw("STRUCT") {
                self.bump();
                let mut fields = Vec::new();
                while !self.is_eof() && !self.peek().is_kw("END_STRUCT") {
                    let (fname, _) = self.expect_ident("struct field name")?;
                    let _ = self.expect_kind(&TokenKind::Colon, "`:`")?;
                    let fty = self.parse_type_ref()?;
                    let _ = self.expect_kind(&TokenKind::Semi, "`;`")?;
                    fields.push(StructField {
                        name: fname,
                        ty: fty,
                    });
                }
                let end_struct = self.expect_kw("END_STRUCT")?;
                let end_semi = if self.peek().kind == TokenKind::Semi {
                    self.bump()
                } else {
                    end_struct
                };
                self.attach_inner_and_trailing_comments(end_semi.span.end, end_semi.end_line, &id);
                decls.push(TypeDecl::Struct {
                    id,
                    name,
                    fields,
                    span: name_span.merge(end_semi.span),
                });
            } else if self.peek().kind == TokenKind::LParen {
                self.bump();
                let mut variants = Vec::new();
                let mut next_val = 0i64;
                while !self.is_eof() && self.peek().kind != TokenKind::RParen {
                    let (vname, _) = self.expect_ident("enum variant name")?;
                    let val = if self.peek().kind == TokenKind::ColonEq {
                        self.bump();
                        let neg = if self.peek().kind == TokenKind::Minus {
                            self.bump();
                            true
                        } else {
                            false
                        };
                        let ntok = self.peek().clone();
                        let TokenKind::Int(n) = ntok.kind else {
                            return Err(self
                                .syntax_err(ntok.span, "expected integer value for enum variant"));
                        };
                        self.bump();
                        let signed = if neg { -n } else { n };
                        i64::try_from(signed).map_err(|_| {
                            self.syntax_err(ntok.span, "enum variant value out of i64 range")
                        })?
                    } else {
                        next_val
                    };
                    next_val = val.saturating_add(1);
                    variants.push(EnumVariant {
                        name: vname,
                        value: val,
                    });
                    if self.peek().kind == TokenKind::Comma {
                        self.bump();
                    } else {
                        break;
                    }
                }
                let _ = self.expect_kind(&TokenKind::RParen, "`)`")?;
                let backing = if self.peek().kind == TokenKind::Colon {
                    self.bump();
                    let tr = self.parse_type_ref()?;
                    match tr {
                        TypeRef::Scalar { ty } => ty,
                        TypeRef::Void
                        | TypeRef::Array { .. }
                        | TypeRef::Named { .. }
                        | TypeRef::Fb { .. } => {
                            return Err(self.syntax_err(
                                self.peek().span,
                                "enum backing type must be a scalar integer type",
                            ));
                        }
                    }
                } else {
                    ScalarType::Dint
                };
                let semi = self.expect_kind(&TokenKind::Semi, "`;`")?;
                self.attach_inner_and_trailing_comments(semi.span.end, semi.end_line, &id);
                decls.push(TypeDecl::Enum {
                    id,
                    name,
                    backing,
                    variants,
                    span: name_span.merge(semi.span),
                });
            } else {
                let tok = self.peek().clone();
                return Err(self.syntax_err(
                    tok.span,
                    format!(
                        "expected `STRUCT` or `(` in TYPE `{name}`, found `{:?}`",
                        tok.kind
                    ),
                ));
            }
        }
        let _ = self.expect_kw("END_TYPE")?;
        Ok(decls)
    }

    fn parse_foreign_type_decl(&mut self) -> Result<TypeDecl, Vec<Diagnostic>> {
        let start = self.expect_kw("FOREIGN_TYPE")?;
        let id = self.alloc_node_id();
        self.attach_comments_before(start.span.start, &id, AstCommentPlacement::Leading);

        let mut name = None;
        let mut pack_hint = None;
        let mut fidelity = ForeignFidelity::F0;

        loop {
            if self.eat_kw("NAME").is_some() {
                let (n, _) = self.expect_ident("foreign type name")?;
                name = Some(n);
            } else if self.eat_kw("PACK").is_some() {
                let (p, _) = self.expect_string("pack hint string")?;
                pack_hint = Some(p);
            } else if self.eat_kw("F0").is_some() {
                fidelity = ForeignFidelity::F0;
            } else if self.eat_kw("F1").is_some() {
                fidelity = ForeignFidelity::F1;
            } else if self.eat_kw("F2").is_some() {
                fidelity = ForeignFidelity::F2;
            } else {
                break;
            }
        }

        let _ = self.expect_kind(&TokenKind::LParen, "`(`")?;
        let (language, _) = self.expect_string("foreign language tag")?;
        let _ = self.expect_kind(&TokenKind::Comma, "`,`")?;
        let (text, _) = self.expect_string("foreign declaration text")?;
        let _ = self.expect_kind(&TokenKind::RParen, "`)`")?;
        let semi = self.expect_kind(&TokenKind::Semi, "`;`")?;
        self.attach_inner_and_trailing_comments(semi.span.end, semi.end_line, &id);

        Ok(TypeDecl::Foreign {
            id,
            name,
            language,
            fidelity,
            text,
            pack_hint,
            span: start.span.merge(semi.span),
        })
    }

    // -----------------------------------------------------------------------
    // VAR_GLOBAL / VAR / Hardware AT Bindings
    // -----------------------------------------------------------------------

    fn parse_var_global_block(
        &mut self,
        hardware: &mut Vec<HardwareBinding>,
        data: &mut Vec<DataDecl>,
    ) -> Result<(), Vec<Diagnostic>> {
        let _ = self.expect_kw("VAR_GLOBAL")?;
        let scope = self.parse_optional_storage_qualifier(StorageScope::Var);

        while !self.is_eof() && !self.peek().is_kw("END_VAR") {
            let start_tok = self.peek().clone();
            let id = self.alloc_node_id();
            self.attach_comments_before(start_tok.span.start, &id, AstCommentPlacement::Leading);
            let (name, name_span) = self.expect_ident("variable or hardware tag name")?;

            if self.eat_kw("AT").is_some() {
                let hw = self.parse_hardware_binding_tail(id, name, name_span)?;
                hardware.push(hw);
            } else {
                let decl = self.parse_data_decl_tail(id, name, name_span, scope)?;
                if matches!(decl.ty, TypeRef::Fb { .. }) {
                    self.fb_vars.insert(decl.name.clone());
                }
                data.push(decl);
            }
        }

        let _ = self.expect_kw("END_VAR")?;
        Ok(())
    }

    fn parse_optional_storage_qualifier(&mut self, default_scope: StorageScope) -> StorageScope {
        if self.eat_kw("CONSTANT").is_some() {
            StorageScope::Const
        } else if self.eat_kw("RETAIN").is_some() {
            StorageScope::Retain
        } else if self.eat_kw("PERSISTENT").is_some() {
            StorageScope::Persistent
        } else {
            default_scope
        }
    }

    fn parse_hardware_binding_tail(
        &mut self,
        id: NodeId,
        label: String,
        start_span: Span,
    ) -> Result<HardwareBinding, Vec<Diagnostic>> {
        let addr_tok = self.peek().clone();
        let (pin, inferred_kind) = match &addr_tok.kind {
            TokenKind::DirectAddr(iec) => {
                let iec_owned = iec.clone();
                self.bump();
                let resolved_pin = self
                    .ctx
                    .port_for_iec(&iec_owned)
                    .map_or_else(|| iec_owned.clone(), str::to_owned);
                let kind = infer_kind_from_iec_addr(&iec_owned);
                (resolved_pin, kind)
            }
            TokenKind::Ident(port) => {
                let p = port.clone();
                self.bump();
                (p, None)
            }
            TokenKind::String(port) => {
                let p = port.clone();
                self.bump();
                (p, None)
            }
            TokenKind::Bool(_)
            | TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::TimeUs(_)
            | TokenKind::LParen
            | TokenKind::RParen
            | TokenKind::LBracket
            | TokenKind::RBracket
            | TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::Comma
            | TokenKind::Dot
            | TokenKind::DotDot
            | TokenKind::Colon
            | TokenKind::ColonEq
            | TokenKind::Semi
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::Eq
            | TokenKind::EqEq
            | TokenKind::Ne
            | TokenKind::BangEq
            | TokenKind::Eof => {
                return Err(self.syntax_err(
                    addr_tok.span,
                    "expected IEC direct address (e.g. `%IX0.0`, `%QX0.0`, `%IW64`) or board port after `AT`",
                ));
            }
        };

        let _ = self.expect_kind(&TokenKind::Colon, "`:`")?;
        let ty = self.parse_type_ref()?;

        let mut kind = inferred_kind.unwrap_or(match &ty {
            TypeRef::Scalar {
                ty: ScalarType::Duty,
            } => HardwareKind::PwmOut,
            TypeRef::Scalar {
                ty: ScalarType::Code { .. } | ScalarType::Analog,
            } => HardwareKind::AnalogIn,
            TypeRef::Void
            | TypeRef::Scalar { .. }
            | TypeRef::Array { .. }
            | TypeRef::Named { .. }
            | TypeRef::Fb { .. } => HardwareKind::DigitalIn,
        });
        let mut codec = match &ty {
            TypeRef::Scalar {
                ty: ScalarType::Code { codec_id },
            } => Some(codec_id.clone()),
            TypeRef::Void
            | TypeRef::Scalar { .. }
            | TypeRef::Array { .. }
            | TypeRef::Named { .. }
            | TypeRef::Fb { .. } => None,
        };
        let mut pull = PullMode::None;
        let mut active_low = false;
        let mut range = None;

        // Optional hardware attribute tuple: `(DIGITAL_OUT, PULL := PULL_UP, ACTIVE_LOW := TRUE, CODEC := "...", RANGE := ...)`
        if self.peek().kind == TokenKind::LParen {
            self.bump();
            while !self.is_eof() && self.peek().kind != TokenKind::RParen {
                let (attr, attr_span) = self.expect_ident("hardware attribute")?;
                if attr.eq_ignore_ascii_case("DIGITAL_IN") {
                    kind = HardwareKind::DigitalIn;
                } else if attr.eq_ignore_ascii_case("DIGITAL_OUT") {
                    kind = HardwareKind::DigitalOut;
                } else if attr.eq_ignore_ascii_case("DIGITAL_BIDIR") {
                    kind = HardwareKind::DigitalBidirectional;
                } else if attr.eq_ignore_ascii_case("ANALOG_IN") {
                    kind = HardwareKind::AnalogIn;
                } else if attr.eq_ignore_ascii_case("ANALOG_OUT") {
                    kind = HardwareKind::AnalogOut;
                } else if attr.eq_ignore_ascii_case("PWM_OUT") {
                    kind = HardwareKind::PwmOut;
                } else if attr.eq_ignore_ascii_case("BUS_PIN") {
                    kind = HardwareKind::BusPin;
                } else if attr.eq_ignore_ascii_case("PULL") {
                    let _ = self.expect_kind(&TokenKind::ColonEq, "`:=`")?;
                    let (val, vspan) = self.expect_ident("pull mode")?;
                    pull = if val.eq_ignore_ascii_case("NONE") {
                        PullMode::None
                    } else if val.eq_ignore_ascii_case("PULL_UP") {
                        PullMode::PullUp
                    } else if val.eq_ignore_ascii_case("PULL_DOWN") {
                        PullMode::PullDown
                    } else {
                        return Err(self.unknown_field_err(
                            vspan,
                            format!("unknown pull mode `{val}` (expected `NONE`, `PULL_UP`, or `PULL_DOWN`)"),
                        ));
                    };
                } else if attr.eq_ignore_ascii_case("ACTIVE_LOW") {
                    let _ = self.expect_kind(&TokenKind::ColonEq, "`:=`")?;
                    let (b, _) = self.expect_bool("ACTIVE_LOW boolean")?;
                    active_low = b;
                } else if attr.eq_ignore_ascii_case("CODEC") {
                    let _ = self.expect_kind(&TokenKind::ColonEq, "`:=`")?;
                    let (c, _) = self.expect_string("codec identifier string")?;
                    codec = Some(c);
                } else if attr.eq_ignore_ascii_case("RANGE") {
                    let _ = self.expect_kind(&TokenKind::ColonEq, "`:=`")?;
                    range = Some(self.parse_channel_range()?);
                } else {
                    return Err(self.unknown_field_err(
                        attr_span,
                        format!("unknown hardware binding attribute `{attr}`"),
                    ));
                }

                if self.peek().kind == TokenKind::Comma {
                    self.bump();
                } else {
                    break;
                }
            }
            let _ = self.expect_kind(&TokenKind::RParen, "`)`")?;
        }

        let semi = self.expect_kind(&TokenKind::Semi, "`;`")?;
        self.attach_inner_and_trailing_comments(semi.span.end, semi.end_line, &id);
        Ok(HardwareBinding {
            id,
            label,
            pin,
            kind,
            pull,
            active_low,
            codec,
            range,
            span: start_span.merge(semi.span),
        })
    }

    fn parse_channel_range(&mut self) -> Result<ChannelRange, Vec<Diagnostic>> {
        let (sig_kw, sig_span) =
            self.expect_ident("channel range signal (`VOLTAGE` or `CURRENT`)")?;
        let signal = if sig_kw.eq_ignore_ascii_case("VOLTAGE") {
            SignalKind::Voltage
        } else if sig_kw.eq_ignore_ascii_case("CURRENT") {
            SignalKind::Current
        } else {
            return Err(self.unknown_field_err(
                sig_span,
                format!(
                    "unknown channel range signal `{sig_kw}` (expected `VOLTAGE` or `CURRENT`)"
                ),
            ));
        };
        let _ = self.expect_kind(&TokenKind::LParen, "`(`")?;
        let (low, _) = self.expect_f64("range low bound")?;
        let _ = self.expect_kind(&TokenKind::DotDot, "`..`")?;
        let (high, _) = self.expect_f64("range high bound")?;
        let mut live_zero = false;
        let mut hardware_normalizes_live_zero = false;
        while self.peek().kind == TokenKind::Comma {
            self.bump();
            let (flag, fspan) = self.expect_ident("range flag")?;
            if flag.eq_ignore_ascii_case("LIVE_ZERO") {
                live_zero = true;
            } else if flag.eq_ignore_ascii_case("HW_NORM") {
                hardware_normalizes_live_zero = true;
            } else {
                return Err(
                    self.unknown_field_err(fspan, format!("unknown channel range flag `{flag}`"))
                );
            }
        }
        let _ = self.expect_kind(&TokenKind::RParen, "`)`")?;
        Ok(ChannelRange {
            signal,
            low,
            high,
            live_zero,
            hardware_normalizes_live_zero,
        })
    }

    fn parse_data_decl_tail(
        &mut self,
        id: NodeId,
        name: String,
        start_span: Span,
        scope: StorageScope,
    ) -> Result<DataDecl, Vec<Diagnostic>> {
        let _ = self.expect_kind(&TokenKind::Colon, "`:`")?;
        let ty = self.parse_type_ref()?;
        let initial = if self.peek().kind == TokenKind::ColonEq {
            self.bump();
            Some(self.parse_expr()?)
        } else {
            None
        };
        let semi = self.expect_kind(&TokenKind::Semi, "`;`")?;
        self.attach_inner_and_trailing_comments(semi.span.end, semi.end_line, &id);
        Ok(DataDecl {
            id,
            name,
            scope,
            ty,
            initial,
            span: start_span.merge(semi.span),
        })
    }

    fn parse_pou_var_block(
        &mut self,
        params: &mut Vec<ParamDecl>,
        locals: &mut Vec<DataDecl>,
    ) -> Result<bool, Vec<Diagnostic>> {
        if self.eat_kw("VAR_INPUT").is_some() {
            self.parse_param_block_body(ParamDirection::In, params)?;
            return Ok(true);
        }
        if self.eat_kw("VAR_OUTPUT").is_some() {
            self.parse_param_block_body(ParamDirection::Out, params)?;
            return Ok(true);
        }
        if self.eat_kw("VAR_IN_OUT").is_some() {
            self.parse_param_block_body(ParamDirection::InOut, params)?;
            return Ok(true);
        }
        if self.eat_kw("VAR_TEMP").is_some() {
            self.parse_local_var_body(StorageScope::Temp, locals)?;
            return Ok(true);
        }
        if self.eat_kw("VAR").is_some() {
            let scope = self.parse_optional_storage_qualifier(StorageScope::Var);
            self.parse_local_var_body(scope, locals)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn parse_param_block_body(
        &mut self,
        direction: ParamDirection,
        params: &mut Vec<ParamDecl>,
    ) -> Result<(), Vec<Diagnostic>> {
        while !self.is_eof() && !self.peek().is_kw("END_VAR") {
            let (name, _) = self.expect_ident("parameter name")?;
            let _ = self.expect_kind(&TokenKind::Colon, "`:`")?;
            let ty = self.parse_type_ref()?;
            let _ = self.expect_kind(&TokenKind::Semi, "`;`")?;
            if matches!(ty, TypeRef::Fb { .. }) {
                self.fb_vars.insert(name.clone());
            }
            params.push(ParamDecl {
                name,
                direction,
                ty,
            });
        }
        let _ = self.expect_kw("END_VAR")?;
        Ok(())
    }

    fn parse_local_var_body(
        &mut self,
        scope: StorageScope,
        locals: &mut Vec<DataDecl>,
    ) -> Result<(), Vec<Diagnostic>> {
        while !self.is_eof() && !self.peek().is_kw("END_VAR") {
            let start_tok = self.peek().clone();
            let id = self.alloc_node_id();
            self.attach_comments_before(start_tok.span.start, &id, AstCommentPlacement::Leading);
            let (name, name_span) = self.expect_ident("local variable name")?;
            let decl = self.parse_data_decl_tail(id, name, name_span, scope)?;
            if matches!(decl.ty, TypeRef::Fb { .. }) {
                self.fb_vars.insert(decl.name.clone());
            }
            locals.push(decl);
        }
        let _ = self.expect_kw("END_VAR")?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Type Reference Parsing
    // -----------------------------------------------------------------------

    fn parse_type_ref(&mut self) -> Result<TypeRef, Vec<Diagnostic>> {
        let (name, span) = self.expect_ident("type name")?;
        let upper = name.to_ascii_uppercase();
        match upper.as_str() {
            "VOID" => Ok(TypeRef::Void),
            "BOOL" => Ok(TypeRef::scalar(ScalarType::Bool)),
            "SINT" => Ok(TypeRef::scalar(ScalarType::Sint)),
            "INT" => Ok(TypeRef::scalar(ScalarType::Int)),
            "DINT" => Ok(TypeRef::scalar(ScalarType::Dint)),
            "LINT" => Ok(TypeRef::scalar(ScalarType::Lint)),
            "USINT" => Ok(TypeRef::scalar(ScalarType::Usint)),
            "UINT" => Ok(TypeRef::scalar(ScalarType::Uint)),
            "UDINT" => Ok(TypeRef::scalar(ScalarType::Udint)),
            "ULINT" => Ok(TypeRef::scalar(ScalarType::Ulint)),
            "BYTE" => Ok(TypeRef::scalar(ScalarType::Byte)),
            "WORD" => Ok(TypeRef::scalar(ScalarType::Word)),
            "DWORD" => Ok(TypeRef::scalar(ScalarType::Dword)),
            "LWORD" => Ok(TypeRef::scalar(ScalarType::Lword)),
            "REAL" => Ok(TypeRef::scalar(ScalarType::Real)),
            "LREAL" => Ok(TypeRef::scalar(ScalarType::Lreal)),
            "TIME" => Ok(TypeRef::scalar(ScalarType::Time)),
            "ANALOG" => Ok(TypeRef::scalar(ScalarType::Analog)),
            "DUTY" => Ok(TypeRef::scalar(ScalarType::Duty)),
            "STRING" => {
                let max_len = if self.peek().kind == TokenKind::LBracket {
                    self.bump();
                    let (n, nspan) = self.expect_u64("STRING length")?;
                    let len = u32::try_from(n)
                        .map_err(|_| self.syntax_err(nspan, "STRING length exceeds u32 range"))?;
                    let _ = self.expect_kind(&TokenKind::RBracket, "`]`")?;
                    len
                } else {
                    255
                };
                Ok(TypeRef::scalar(ScalarType::String { max_len }))
            }
            "CODE" => {
                let _ = self.expect_kind(&TokenKind::Lt, "`<`")?;
                let (codec_id, _) = self.expect_ident("codec identifier")?;
                let _ = self.expect_kind(&TokenKind::Gt, "`>`")?;
                Ok(TypeRef::scalar(ScalarType::Code { codec_id }))
            }
            "ARRAY" => {
                let _ = self.expect_kind(&TokenKind::LBracket, "`[`")?;
                let (first, fspan) = self.expect_u64("array length or lower bound")?;
                let len = if self.peek().kind == TokenKind::DotDot {
                    self.bump();
                    let (hi, hspan) = self.expect_u64("array upper bound")?;
                    if hi < first {
                        return Err(self.syntax_err(hspan, "array upper bound < lower bound"));
                    }
                    let count = hi.saturating_sub(first).saturating_add(1);
                    u32::try_from(count)
                        .map_err(|_| self.syntax_err(hspan, "array length exceeds u32 range"))?
                } else {
                    u32::try_from(first)
                        .map_err(|_| self.syntax_err(fspan, "array length exceeds u32 range"))?
                };
                let _ = self.expect_kind(&TokenKind::RBracket, "`]`")?;
                let _ = self.expect_kw("OF")?;
                let elem = self.parse_type_ref()?;
                Ok(TypeRef::Array {
                    elem: Box::new(elem),
                    len,
                })
            }
            "TON" => Ok(TypeRef::Fb {
                fb_kind: FbKind::Ton,
            }),
            "TOF" => Ok(TypeRef::Fb {
                fb_kind: FbKind::Tof,
            }),
            "TP" => Ok(TypeRef::Fb {
                fb_kind: FbKind::Tp,
            }),
            "CTU" => Ok(TypeRef::Fb {
                fb_kind: FbKind::Ctu,
            }),
            "CTD" => Ok(TypeRef::Fb {
                fb_kind: FbKind::Ctd,
            }),
            "CTUD" => Ok(TypeRef::Fb {
                fb_kind: FbKind::Ctud,
            }),
            "R_TRIG" => Ok(TypeRef::Fb {
                fb_kind: FbKind::RTrig,
            }),
            "F_TRIG" => Ok(TypeRef::Fb {
                fb_kind: FbKind::FTrig,
            }),
            "SR" => Ok(TypeRef::Fb {
                fb_kind: FbKind::Sr,
            }),
            "RS" => Ok(TypeRef::Fb {
                fb_kind: FbKind::Rs,
            }),
            "UART_SEND" => Ok(TypeRef::Fb {
                fb_kind: FbKind::UartSend,
            }),
            "UART_RECV" => Ok(TypeRef::Fb {
                fb_kind: FbKind::UartRecv,
            }),
            "I2C_XFER" => Ok(TypeRef::Fb {
                fb_kind: FbKind::I2cXfer,
            }),
            "SPI_XFER" => Ok(TypeRef::Fb {
                fb_kind: FbKind::SpiXfer,
            }),
            "MB_CLIENT" => Ok(TypeRef::Fb {
                fb_kind: FbKind::MbClient,
            }),
            "MB_SERVER" => Ok(TypeRef::Fb {
                fb_kind: FbKind::MbServer,
            }),
            "FB" => {
                // Explicit user FB type reference: `FB<MyFb>`
                let _ = self.expect_kind(&TokenKind::Lt, "`<`")?;
                let (fb_name, _) = self.expect_ident("user FB name")?;
                let _ = self.expect_kind(&TokenKind::Gt, "`>`")?;
                Ok(TypeRef::Fb {
                    fb_kind: FbKind::User { fb_name },
                })
            }
            _ => {
                let _ = span;
                Ok(TypeRef::Named { name })
            }
        }
    }

    // -----------------------------------------------------------------------
    // SFC Chart POU Parsing (`SFC_CHART` / `CHART`)
    // -----------------------------------------------------------------------

    fn parse_sfc_chart_pou(&mut self) -> Result<Pou, Vec<Diagnostic>> {
        let (start_tok, end_kw) = if let Some(t) = self.eat_kw("SFC_CHART") {
            (t, "END_SFC_CHART")
        } else {
            (self.expect_kw("CHART")?, "END_CHART")
        };

        let pou_id = self.alloc_node_id();
        self.attach_comments_before(start_tok.span.start, &pou_id, AstCommentPlacement::Header);

        let (name, name_span) = self.expect_ident("SFC chart name")?;
        let mut kind = PouKind::Ob { ob: ObKind::Main };
        let mut preferred_view = None;
        let mut evolve_to_stability = false;
        let mut header_end_tok = Token {
            kind: TokenKind::Ident(name.clone()),
            span: name_span,
            start_line: start_tok.start_line,
            end_line: start_tok.end_line,
        };

        if self.peek().kind == TokenKind::LParen {
            header_end_tok = self.parse_pou_header_options(
                &mut kind,
                &mut preferred_view,
                Some(&mut evolve_to_stability),
            )?;
        }

        self.attach_inner_and_trailing_comments(
            header_end_tok.span.end,
            header_end_tok.end_line,
            &pou_id,
        );

        let mut params = Vec::new();
        let mut locals = Vec::new();
        let mut steps = Vec::new();
        let mut transitions = Vec::new();
        let mut auto_tr_idx = 1u32;

        while !self.is_eof()
            && !self.peek().is_kw(end_kw)
            && !self.peek().is_kw("END_SFC_CHART")
            && !self.peek().is_kw("END_CHART")
        {
            if self.parse_pou_var_block(&mut params, &mut locals)? {
                continue;
            }
            if self.eat_kw("EVOLVE_TO_STABILITY").is_some() {
                if self.peek().kind == TokenKind::ColonEq {
                    self.bump();
                    let (b, _) = self.expect_bool("EVOLVE_TO_STABILITY boolean")?;
                    evolve_to_stability = b;
                    let _ = self.expect_kind(&TokenKind::Semi, "`;`")?;
                } else {
                    evolve_to_stability = true;
                    if self.peek().kind == TokenKind::Semi {
                        self.bump();
                    }
                }
                continue;
            }
            if self.peek().is_kw("INITIAL_STEP")
                || self.peek().is_kw("STEP")
                || self.peek().is_kw("MACRO_STEP")
                || self.peek().is_kw("INITIAL_MACRO_STEP")
            {
                let step = self.parse_sfc_step()?;
                steps.push(step);
                continue;
            }
            if self.peek().is_kw("TRANSITION") {
                let tr = self.parse_sfc_transition(&mut auto_tr_idx)?;
                transitions.push(tr);
                continue;
            }

            let tok = self.peek().clone();
            return Err(self.syntax_err(
                tok.span,
                format!(
                    "unexpected token `{:?}` inside SFC chart `{name}` (expected `INITIAL_STEP`, `STEP`, `MACRO_STEP`, `TRANSITION`, `VAR`, `EVOLVE_TO_STABILITY`, or `{end_kw}`)",
                    tok.kind
                ),
            ));
        }

        let end_tok = if let Some(t) = self.eat_kw(end_kw) {
            t
        } else {
            let tok = self.peek().clone();
            return Err(self.syntax_err(
                tok.span,
                format!("expected `{end_kw}` to close SFC chart `{name}`"),
            ));
        };
        self.attach_comments_before(end_tok.span.start, &pou_id, AstCommentPlacement::Dangling);

        Ok(Pou {
            id: pou_id,
            name,
            kind,
            preferred_view,
            params,
            locals,
            body: PouBody::Chart(SfcChart {
                steps,
                transitions,
                evolve_to_stability,
            }),
            span: start_tok.span.merge(end_tok.span),
        })
    }

    fn parse_pou_header_options(
        &mut self,
        kind: &mut PouKind,
        preferred_view: &mut Option<AuthoringLanguage>,
        mut evolve_to_stability: Option<&mut bool>,
    ) -> Result<Token, Vec<Diagnostic>> {
        let _ = self.expect_kind(&TokenKind::LParen, "`(`")?;
        while !self.is_eof() && self.peek().kind != TokenKind::RParen {
            let (opt_key, opt_span) = self.expect_ident("POU header option")?;
            let _ = self.expect_kind(&TokenKind::ColonEq, "`:=`")?;
            if opt_key.eq_ignore_ascii_case("STABILITY")
                || opt_key.eq_ignore_ascii_case("EVOLVE_TO_STABILITY")
            {
                let (b, _) = self.expect_bool("STABILITY boolean")?;
                if let Some(stab) = evolve_to_stability.as_deref_mut() {
                    *stab = b;
                } else {
                    return Err(self.unknown_field_err(
                        opt_span,
                        "`STABILITY` option is only valid on SFC charts",
                    ));
                }
            } else if opt_key.eq_ignore_ascii_case("KIND") {
                *kind = self.parse_pou_kind_value()?;
            } else if opt_key.eq_ignore_ascii_case("VIEW") {
                let (v, vspan) = self.expect_ident("preferred view language")?;
                *preferred_view = Some(parse_authoring_language(&v).ok_or_else(|| {
                    self.unknown_field_err(vspan, format!("unknown preferred view `{v}`"))
                })?);
            } else {
                return Err(self.unknown_field_err(
                    opt_span,
                    format!("unknown POU header option `{opt_key}`"),
                ));
            }
            if self.peek().kind == TokenKind::Comma {
                self.bump();
            } else {
                break;
            }
        }
        self.expect_kind(&TokenKind::RParen, "`)`")
    }

    fn parse_pou_kind_value(&mut self) -> Result<PouKind, Vec<Diagnostic>> {
        let (k, kspan) = self.expect_ident("POU kind")?;
        if k.eq_ignore_ascii_case("MAIN") || k.eq_ignore_ascii_case("OB_MAIN") {
            Ok(PouKind::Ob { ob: ObKind::Main })
        } else if k.eq_ignore_ascii_case("STARTUP") || k.eq_ignore_ascii_case("OB_STARTUP") {
            Ok(PouKind::Ob {
                ob: ObKind::Startup,
            })
        } else if k.eq_ignore_ascii_case("FB") {
            Ok(PouKind::Fb)
        } else if k.eq_ignore_ascii_case("FC") {
            let _ = self.expect_kind(&TokenKind::LParen, "`(`")?;
            let ret = self.parse_type_ref()?;
            let _ = self.expect_kind(&TokenKind::RParen, "`)`")?;
            Ok(PouKind::Fc { return_type: ret })
        } else if k.eq_ignore_ascii_case("CYCLIC_INT") || k.eq_ignore_ascii_case("OB_CYCLIC_INT") {
            let _ = self.expect_kind(&TokenKind::LParen, "`(`")?;
            let (period_us, _) = self.expect_u64("CYCLIC_INT period_us")?;
            let _ = self.expect_kind(&TokenKind::RParen, "`)`")?;
            Ok(PouKind::Ob {
                ob: ObKind::CyclicInt { period_us },
            })
        } else if k.eq_ignore_ascii_case("HW_INT") || k.eq_ignore_ascii_case("OB_HW_INT") {
            let _ = self.expect_kind(&TokenKind::LParen, "`(`")?;
            let (pin_label, _) = self.expect_ident("HW_INT pin_label")?;
            let _ = self.expect_kind(&TokenKind::Comma, "`,`")?;
            let (edge_kw, espan) = self.expect_ident("HW_INT edge")?;
            let edge = parse_interrupt_edge(&edge_kw).ok_or_else(|| {
                self.unknown_field_err(espan, format!("unknown interrupt edge `{edge_kw}`"))
            })?;
            let _ = self.expect_kind(&TokenKind::Comma, "`,`")?;
            let (prio_u64, pspan) = self.expect_u64("HW_INT priority")?;
            let priority = u8::try_from(prio_u64)
                .map_err(|_| self.syntax_err(pspan, "HW_INT priority exceeds u8 range"))?;
            let _ = self.expect_kind(&TokenKind::RParen, "`)`")?;
            Ok(PouKind::Ob {
                ob: ObKind::HwInt {
                    pin_label,
                    edge,
                    priority,
                },
            })
        } else if k.eq_ignore_ascii_case("COMM_EVENT") || k.eq_ignore_ascii_case("OB_COMM_EVENT") {
            let _ = self.expect_kind(&TokenKind::LParen, "`(`")?;
            let (bus_id, _) = self.expect_string("COMM_EVENT bus_id")?;
            let _ = self.expect_kind(&TokenKind::RParen, "`)`")?;
            Ok(PouKind::Ob {
                ob: ObKind::CommEvent { bus_id },
            })
        } else {
            Err(self.unknown_field_err(kspan, format!("unknown POU kind `{k}`")))
        }
    }

    fn parse_sfc_step(&mut self) -> Result<SfcStep, Vec<Diagnostic>> {
        let kw_tok = self.bump();
        let (initial, is_macro_kw) = if kw_tok.is_kw("INITIAL_STEP") {
            (true, false)
        } else if kw_tok.is_kw("INITIAL_MACRO_STEP") {
            (true, true)
        } else if kw_tok.is_kw("MACRO_STEP") {
            (false, true)
        } else {
            (false, false)
        };

        let step_id = self.alloc_node_id();
        self.attach_comments_before(kw_tok.span.start, &step_id, AstCommentPlacement::Leading);

        let (name, name_span) = self.expect_ident("step name")?;
        let mut macro_chart = None;

        if is_macro_kw {
            if self.peek().kind == TokenKind::LParen {
                self.bump();
                let (sub, _) = self.expect_ident("macro sub-chart name")?;
                let _ = self.expect_kind(&TokenKind::RParen, "`)`")?;
                macro_chart = Some(sub);
            } else {
                let _ = self.eat_kw("MACRO");
                let (sub, _) = self.expect_ident("macro sub-chart name")?;
                macro_chart = Some(sub);
            }
        } else if self.eat_kw("MACRO").is_some() {
            let (sub, _) = self.expect_ident("macro sub-chart name")?;
            macro_chart = Some(sub);
        }

        let header_end = if self.peek().kind == TokenKind::Colon {
            self.bump()
        } else {
            Token {
                kind: TokenKind::Colon,
                span: name_span,
                start_line: kw_tok.start_line,
                end_line: kw_tok.end_line,
            }
        };
        self.attach_inner_and_trailing_comments(header_end.span.end, header_end.end_line, &step_id);

        let mut actions = Vec::new();
        while !self.is_eof() && !self.peek().is_kw("END_STEP") {
            let act = self.parse_sfc_action()?;
            actions.push(act);
        }

        let end_tok = self.expect_kw("END_STEP")?;
        self.attach_comments_before(end_tok.span.start, &step_id, AstCommentPlacement::Dangling);

        Ok(SfcStep {
            id: step_id,
            name,
            initial,
            actions,
            macro_chart,
        })
    }

    fn parse_sfc_action(&mut self) -> Result<SfcAction, Vec<Diagnostic>> {
        let start_tok = self.peek().clone();
        let action_id = self.alloc_node_id();
        self.attach_comments_before(
            start_tok.span.start,
            &action_id,
            AstCommentPlacement::Leading,
        );

        // Verbose IEC 61131-3 form: `ACTION [ActName] (Qualifier [, Duration]): ... END_ACTION`
        if self.eat_kw("ACTION").is_some() {
            // Optional action name before `(`
            if self.peek().kind != TokenKind::LParen {
                let _ = self.expect_ident("action name")?;
            }
            let _ = self.expect_kind(&TokenKind::LParen, "`(`")?;
            let (q_name, q_span) = self.expect_ident("action qualifier")?;
            let qualifier = parse_action_qualifier(&q_name).ok_or_else(|| {
                self.syntax_err(
                    q_span,
                    format!(
                        "unknown SFC action qualifier `{q_name}` (expected one of `N`, `S`, `R`, `L`, `D`, `P`, `P1`, `P0`)"
                    ),
                )
            })?;
            let duration = if self.peek().kind == TokenKind::Comma {
                self.bump();
                Some(self.parse_expr()?)
            } else {
                None
            };
            let _ = self.expect_kind(&TokenKind::RParen, "`)`")?;
            let _ = self.expect_kind(&TokenKind::Colon, "`:`")?;

            let mut target = None;
            let mut body = Vec::new();

            if self.peek().kind == TokenKind::LBrace {
                body = self.parse_brace_stmt_block(&action_id)?;
            } else if !self.peek().is_kw("END_ACTION") {
                // Check if single LValue coil target `target;` or `target { ... }` vs statements
                let saved_pos = self.pos;
                if let Ok(lv) = self.try_parse_lvalue_only() {
                    if self.peek().kind == TokenKind::Semi {
                        let _ = self.bump();
                        target = Some(lv);
                    } else if self.peek().kind == TokenKind::LBrace {
                        target = Some(lv);
                        body = self.parse_brace_stmt_block(&action_id)?;
                    } else {
                        self.pos = saved_pos;
                        while !self.is_eof() && !self.peek().is_kw("END_ACTION") {
                            body.push(self.parse_stmt()?);
                        }
                    }
                } else {
                    self.pos = saved_pos;
                    while !self.is_eof() && !self.peek().is_kw("END_ACTION") {
                        body.push(self.parse_stmt()?);
                    }
                }
            }

            let end_tok = self.expect_kw("END_ACTION")?;
            self.attach_inner_and_trailing_comments(end_tok.span.end, end_tok.end_line, &action_id);
            return Ok(SfcAction {
                id: action_id,
                qualifier,
                duration,
                target,
                body,
            });
        }

        // Canonical `docs/lbc/sfc.md` §1.1 form:
        //   `Qualifier [ "(" Duration ")" ] ":" ( LValue ";" | "{" { Stmt } "}" | LValue "{" { Stmt } "}" )`
        // and shorthand IEC forms:
        //   `Qualifier "(" LValue ")" ";"` or `Qualifier "(" Duration "," LValue ")" ";"`
        let (q_name, q_span) =
            self.expect_ident("action qualifier (`N`, `S`, `R`, `L`, `D`, `P`, `P1`, `P0`)")?;
        let qualifier = parse_action_qualifier(&q_name).ok_or_else(|| {
            self.syntax_err(
                q_span,
                format!(
                    "unknown SFC action qualifier `{q_name}` (expected one of `N`, `S`, `R`, `L`, `D`, `P`, `P1`, `P0`)"
                ),
            )
        })?;

        let mut duration = None;
        if self.peek().kind == TokenKind::LParen {
            self.bump();
            let first_expr = self.parse_expr()?;
            if self.peek().kind == TokenKind::Comma {
                // Shorthand `L(T#500ms, Alarm);`
                self.bump();
                let target_lv = self.parse_lvalue()?;
                let _ = self.expect_kind(&TokenKind::RParen, "`)`")?;
                let end_tok = self.expect_kind(&TokenKind::Semi, "`;`")?;
                self.attach_inner_and_trailing_comments(
                    end_tok.span.end,
                    end_tok.end_line,
                    &action_id,
                );
                return Ok(SfcAction {
                    id: action_id,
                    qualifier,
                    duration: Some(first_expr),
                    target: Some(target_lv),
                    body: Vec::new(),
                });
            }
            let _ = self.expect_kind(&TokenKind::RParen, "`)`")?;

            // Disambiguate `N(MotorOut);` vs `L(T#500ms): MotorOut;`
            if self.peek().kind == TokenKind::Semi {
                let end_tok = self.bump();
                let target_lv = self.expr_to_lvalue(first_expr)?;
                self.attach_inner_and_trailing_comments(
                    end_tok.span.end,
                    end_tok.end_line,
                    &action_id,
                );
                return Ok(SfcAction {
                    id: action_id,
                    qualifier,
                    duration: None,
                    target: Some(target_lv),
                    body: Vec::new(),
                });
            }
            duration = Some(first_expr);
        }

        let _ = self.expect_kind(&TokenKind::Colon, "`:`")?;

        let (target, body, end_tok) = if self.peek().kind == TokenKind::LBrace {
            let close = self.peek().clone();
            let stmts = self.parse_brace_stmt_block(&action_id)?;
            let last_tok = self
                .tokens
                .get(self.pos.saturating_sub(1))
                .cloned()
                .unwrap_or(close);
            (None, stmts, last_tok)
        } else {
            let lv = self.parse_lvalue()?;
            if self.peek().kind == TokenKind::LBrace {
                let close = self.peek().clone();
                let stmts = self.parse_brace_stmt_block(&action_id)?;
                let last_tok = if self.peek().kind == TokenKind::Semi {
                    self.bump()
                } else {
                    self.tokens
                        .get(self.pos.saturating_sub(1))
                        .cloned()
                        .unwrap_or(close)
                };
                (Some(lv), stmts, last_tok)
            } else {
                let semi = self.expect_kind(&TokenKind::Semi, "`;`")?;
                (Some(lv), Vec::new(), semi)
            }
        };

        self.attach_inner_and_trailing_comments(end_tok.span.end, end_tok.end_line, &action_id);

        Ok(SfcAction {
            id: action_id,
            qualifier,
            duration,
            target,
            body,
        })
    }

    fn parse_brace_stmt_block(&mut self, owner_id: &NodeId) -> Result<Vec<Stmt>, Vec<Diagnostic>> {
        let _ = self.expect_kind(&TokenKind::LBrace, "`{`")?;
        let mut stmts = Vec::new();
        while !self.is_eof() && self.peek().kind != TokenKind::RBrace {
            stmts.push(self.parse_stmt()?);
        }
        let close = self.expect_kind(&TokenKind::RBrace, "`}`")?;
        self.attach_comments_before(close.span.start, owner_id, AstCommentPlacement::Dangling);
        Ok(stmts)
    }

    fn parse_sfc_transition(
        &mut self,
        auto_tr_idx: &mut u32,
    ) -> Result<SfcTransition, Vec<Diagnostic>> {
        let start_tok = self.expect_kw("TRANSITION")?;
        let tr_id = self.alloc_node_id();
        self.attach_comments_before(start_tok.span.start, &tr_id, AstCommentPlacement::Leading);

        // Optional transition name (omitted if `FROM`, `PRIORITY`, or `(` follows directly)
        let name = if self.peek().is_kw("FROM")
            || self.peek().is_kw("PRIORITY")
            || self.peek().kind == TokenKind::LParen
        {
            let generated = format!("T_{}", *auto_tr_idx);
            *auto_tr_idx = auto_tr_idx.saturating_add(1);
            generated
        } else {
            let (n, _) = self.expect_ident("transition name")?;
            n
        };

        // Optional priority: `(PRIORITY := n)` OR `PRIORITY n`
        let mut priority = 0u32;
        if self.peek().kind == TokenKind::LParen {
            self.bump();
            let (opt_key, opt_span) = self.expect_ident("transition option (`PRIORITY`)")?;
            if !opt_key.eq_ignore_ascii_case("PRIORITY") {
                return Err(self.unknown_field_err(
                    opt_span,
                    format!("unknown TRANSITION option `{opt_key}` (expected `PRIORITY`)"),
                ));
            }
            let _ = self.expect_kind(&TokenKind::ColonEq, "`:=`")?;
            let (p_u64, p_span) = self.expect_u64("transition priority")?;
            priority = u32::try_from(p_u64)
                .map_err(|_| self.syntax_err(p_span, "transition priority exceeds u32 range"))?;
            let _ = self.expect_kind(&TokenKind::RParen, "`)`")?;
        } else if self.eat_kw("PRIORITY").is_some() {
            if self.peek().kind == TokenKind::ColonEq {
                self.bump();
            }
            let (p_u64, p_span) = self.expect_u64("transition priority")?;
            priority = u32::try_from(p_u64)
                .map_err(|_| self.syntax_err(p_span, "transition priority exceeds u32 range"))?;
        }

        let _ = self.expect_kw("FROM")?;
        let from_steps = self.parse_step_list()?;
        let _ = self.expect_kw("TO")?;
        let to_steps = self.parse_step_list()?;
        let _ = self.expect_kind(&TokenKind::ColonEq, "`:=`")?;
        let receptivity = self.parse_expr()?;
        let semi = self.expect_kind(&TokenKind::Semi, "`;`")?;
        self.attach_inner_and_trailing_comments(semi.span.end, semi.end_line, &tr_id);

        let end_tok = self.expect_kw("END_TRANSITION")?;
        self.attach_inner_and_trailing_comments(end_tok.span.end, end_tok.end_line, &tr_id);

        Ok(SfcTransition {
            id: tr_id,
            name,
            from_steps,
            to_steps,
            priority,
            receptivity,
        })
    }

    fn parse_step_list(&mut self) -> Result<Vec<String>, Vec<Diagnostic>> {
        if self.peek().kind == TokenKind::LParen {
            self.bump();
            let mut steps = Vec::new();
            while !self.is_eof() && self.peek().kind != TokenKind::RParen {
                let (s, _) = self.expect_ident("step name in step list")?;
                steps.push(s);
                if self.peek().kind == TokenKind::Comma {
                    self.bump();
                } else {
                    break;
                }
            }
            let close = self.expect_kind(&TokenKind::RParen, "`)`")?;
            if steps.is_empty() {
                return Err(self.syntax_err(close.span, "transition step list must not be empty"));
            }
            Ok(steps)
        } else {
            let (s, _) = self.expect_ident("step name")?;
            Ok(vec![s])
        }
    }

    // -----------------------------------------------------------------------
    // Statement POU Parsing (`PROGRAM`, `FUNCTION`, `FUNCTION_BLOCK`)
    // -----------------------------------------------------------------------

    fn parse_stmt_pou(&mut self) -> Result<Pou, Vec<Diagnostic>> {
        let (start_tok, mut kind, end_kw) = if let Some(t) = self.eat_kw("PROGRAM") {
            (t, PouKind::Ob { ob: ObKind::Main }, "END_PROGRAM")
        } else if let Some(t) = self.eat_kw("FUNCTION_BLOCK") {
            (t, PouKind::Fb, "END_FUNCTION_BLOCK")
        } else {
            let t = self.expect_kw("FUNCTION")?;
            (
                t,
                PouKind::Fc {
                    return_type: TypeRef::Void,
                },
                "END_FUNCTION",
            )
        };

        let pou_id = self.alloc_node_id();
        self.attach_comments_before(start_tok.span.start, &pou_id, AstCommentPlacement::Header);
        let (name, _) = self.expect_ident("POU name")?;
        let mut preferred_view = None;

        if self.peek().kind == TokenKind::LParen {
            let _ = self.parse_pou_header_options(&mut kind, &mut preferred_view, None)?;
        }

        if end_kw == "END_FUNCTION" && self.peek().kind == TokenKind::Colon {
            self.bump();
            let ret_ty = self.parse_type_ref()?;
            kind = PouKind::Fc {
                return_type: ret_ty,
            };
        }

        let header_end = self
            .tokens
            .get(self.pos.saturating_sub(1))
            .cloned()
            .unwrap_or(start_tok.clone());
        self.attach_inner_and_trailing_comments(header_end.span.end, header_end.end_line, &pou_id);

        let mut params = Vec::new();
        let mut locals = Vec::new();
        while !self.is_eof() && self.parse_pou_var_block(&mut params, &mut locals)? {}

        let mut stmts = Vec::new();
        while !self.is_eof() && !self.peek().is_kw(end_kw) {
            stmts.push(self.parse_stmt()?);
        }
        let end_tok = self.expect_kw(end_kw)?;
        self.attach_comments_before(end_tok.span.start, &pou_id, AstCommentPlacement::Dangling);

        Ok(Pou {
            id: pou_id,
            name,
            kind,
            preferred_view,
            params,
            locals,
            body: PouBody::Statements(stmts),
            span: start_tok.span.merge(end_tok.span),
        })
    }

    // -----------------------------------------------------------------------
    // Statements inside Action Bodies & Statement POUs
    // -----------------------------------------------------------------------

    fn parse_stmt(&mut self) -> Result<Stmt, Vec<Diagnostic>> {
        let start_tok = self.peek().clone();
        let stmt_id = self.alloc_node_id();
        self.attach_comments_before(start_tok.span.start, &stmt_id, AstCommentPlacement::Leading);

        if self.peek().is_kw("IF") {
            return self.parse_if_stmt(stmt_id);
        }
        if self.peek().is_kw("CASE") {
            return self.parse_switch_stmt(stmt_id);
        }
        if self.peek().is_kw("WHILE") {
            return self.parse_while_stmt(stmt_id);
        }
        if self.peek().is_kw("FOR") {
            return self.parse_for_stmt(stmt_id);
        }
        if self.eat_kw("EXIT").is_some() || self.eat_kw("BREAK").is_some() {
            let semi = self.expect_kind(&TokenKind::Semi, "`;`")?;
            self.attach_inner_and_trailing_comments(semi.span.end, semi.end_line, &stmt_id);
            return Ok(Stmt::new(
                stmt_id,
                StmtKind::Break,
                start_tok.span.merge(semi.span),
            ));
        }
        if self.eat_kw("CONTINUE").is_some() {
            let semi = self.expect_kind(&TokenKind::Semi, "`;`")?;
            self.attach_inner_and_trailing_comments(semi.span.end, semi.end_line, &stmt_id);
            return Ok(Stmt::new(
                stmt_id,
                StmtKind::Continue,
                start_tok.span.merge(semi.span),
            ));
        }
        if self.eat_kw("RETURN").is_some() {
            let value = if self.peek().kind == TokenKind::Semi {
                None
            } else {
                Some(self.parse_expr()?)
            };
            let semi = self.expect_kind(&TokenKind::Semi, "`;`")?;
            self.attach_inner_and_trailing_comments(semi.span.end, semi.end_line, &stmt_id);
            return Ok(Stmt::new(
                stmt_id,
                StmtKind::Return { value },
                start_tok.span.merge(semi.span),
            ));
        }
        if self.eat_kw("WAIT").is_some() {
            let duration = if self.peek().kind == TokenKind::LParen {
                self.bump();
                let d = self.parse_expr()?;
                let _ = self.expect_kind(&TokenKind::RParen, "`)`")?;
                d
            } else {
                self.parse_expr()?
            };
            let semi = self.expect_kind(&TokenKind::Semi, "`;`")?;
            self.attach_inner_and_trailing_comments(semi.span.end, semi.end_line, &stmt_id);
            return Ok(Stmt::new(
                stmt_id,
                StmtKind::Wait { duration },
                start_tok.span.merge(semi.span),
            ));
        }
        if self.eat_kw("INVOKE").is_some() {
            let instance = self.parse_lvalue()?;
            let args = self.parse_call_arg_list()?;
            let inputs = self.args_to_fb_inputs(args)?;
            let semi = self.expect_kind(&TokenKind::Semi, "`;`")?;
            self.attach_inner_and_trailing_comments(semi.span.end, semi.end_line, &stmt_id);
            return Ok(Stmt::new(
                stmt_id,
                StmtKind::FbInvoke { instance, inputs },
                start_tok.span.merge(semi.span),
            ));
        }
        if self.peek().is_kw("FOREIGN_STMT") {
            return self.parse_foreign_stmt(stmt_id, true);
        }
        if self.peek().is_kw("FOREIGN")
            && self
                .peek_offset(1)
                .is_some_and(|t| t.kind == TokenKind::LParen)
        {
            return self.parse_foreign_stmt(stmt_id, false);
        }

        // Assignment, FB invocation, or function call
        let lvalue = self.parse_lvalue()?;
        if self.peek().kind == TokenKind::ColonEq {
            self.bump();
            let value = self.parse_expr()?;
            let semi = self.expect_kind(&TokenKind::Semi, "`;`")?;
            self.attach_inner_and_trailing_comments(semi.span.end, semi.end_line, &stmt_id);
            Ok(Stmt::new(
                stmt_id,
                StmtKind::Assign {
                    target: lvalue,
                    value,
                },
                start_tok.span.merge(semi.span),
            ))
        } else if self.peek().kind == TokenKind::LParen {
            let args = self.parse_call_arg_list()?;
            let semi = self.expect_kind(&TokenKind::Semi, "`;`")?;
            self.attach_inner_and_trailing_comments(semi.span.end, semi.end_line, &stmt_id);

            let is_fb = match &lvalue {
                LValue::Var { name, .. } => {
                    self.fb_vars.contains(name) && args.iter().all(|a| a.name.is_some())
                }
                LValue::Index { .. } | LValue::Field { .. } => true,
            };

            let kind = if is_fb {
                let inputs = self.args_to_fb_inputs(args)?;
                StmtKind::FbInvoke {
                    instance: lvalue,
                    inputs,
                }
            } else {
                let callee = match lvalue {
                    LValue::Var { name, .. } => name,
                    LValue::Index { .. } | LValue::Field { .. } => lvalue.root_name().to_owned(),
                };
                StmtKind::Call { callee, args }
            };
            Ok(Stmt::new(stmt_id, kind, start_tok.span.merge(semi.span)))
        } else {
            let tok = self.peek().clone();
            Err(self.syntax_err(
                tok.span,
                format!(
                    "expected `:=` or `(` after statement target, found `{:?}`",
                    tok.kind
                ),
            ))
        }
    }

    fn parse_foreign_stmt(
        &mut self,
        stmt_id: NodeId,
        explicit_form: bool,
    ) -> Result<Stmt, Vec<Diagnostic>> {
        let start = self.bump();
        let _ = self.expect_kind(&TokenKind::LParen, "`(`")?;
        let mut fidelity = ForeignFidelity::F0;
        let mut pack_hint = None;

        if explicit_form {
            let (fid_kw, fspan) = self.expect_ident("foreign fidelity (`F0`, `F1`, `F2`)")?;
            fidelity = if fid_kw.eq_ignore_ascii_case("F0") {
                ForeignFidelity::F0
            } else if fid_kw.eq_ignore_ascii_case("F1") {
                ForeignFidelity::F1
            } else if fid_kw.eq_ignore_ascii_case("F2") {
                ForeignFidelity::F2
            } else {
                return Err(self.unknown_field_err(
                    fspan,
                    format!("unknown Foreign fidelity `{fid_kw}` (expected `F0`, `F1`, or `F2`)"),
                ));
            };
            let _ = self.expect_kind(&TokenKind::Comma, "`,`")?;
        }

        let (language, _) = self.expect_string("foreign language string")?;
        let _ = self.expect_kind(&TokenKind::Comma, "`,`")?;
        let (text, _) = self.expect_string("foreign statement text")?;

        if self.peek().kind == TokenKind::Comma {
            self.bump();
            let (pk, pspan) = self.expect_ident("`PACK`")?;
            if !pk.eq_ignore_ascii_case("PACK") {
                return Err(self.unknown_field_err(
                    pspan,
                    format!("unknown FOREIGN_STMT option `{pk}` (expected `PACK`)"),
                ));
            }
            let _ = self.expect_kind(&TokenKind::ColonEq, "`:=`")?;
            let (p, _) = self.expect_string("pack hint")?;
            pack_hint = Some(p);
        }

        let _ = self.expect_kind(&TokenKind::RParen, "`)`")?;
        let semi = self.expect_kind(&TokenKind::Semi, "`;`")?;
        self.attach_inner_and_trailing_comments(semi.span.end, semi.end_line, &stmt_id);

        // If shorthand `FOREIGN("lang", "text");`, upgrade fidelity if declared by a loaded pack
        if !explicit_form && let Ok(lang_id) = language.parse::<LanguageId>() {
            let sym_prefix = text
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.')
                .next()
                .unwrap_or("");
            let (resolved, found) =
                resolve_foreign_fidelity(lang_id, sym_prefix, &self.ctx.loaded_packs);
            if let Some((pack, _)) = found {
                fidelity = resolved;
                pack_hint = Some(pack.pack.id.clone());
            }
        }

        Ok(Stmt::new(
            stmt_id,
            StmtKind::Foreign {
                language,
                fidelity,
                text,
                pack_hint,
            },
            start.span.merge(semi.span),
        ))
    }

    fn parse_if_stmt(&mut self, stmt_id: NodeId) -> Result<Stmt, Vec<Diagnostic>> {
        let start = self.expect_kw("IF")?;
        let condition = self.parse_expr()?;
        let _ = self.expect_kw("THEN")?;
        let mut then_body = Vec::new();
        while !self.is_eof()
            && !self.peek().is_kw("ELSIF")
            && !self.peek().is_kw("ELSE")
            && !self.peek().is_kw("END_IF")
        {
            then_body.push(self.parse_stmt()?);
        }

        let mut elif_arms = Vec::new();
        while self.eat_kw("ELSIF").is_some() {
            let cond = self.parse_expr()?;
            let _ = self.expect_kw("THEN")?;
            let mut body = Vec::new();
            while !self.is_eof()
                && !self.peek().is_kw("ELSIF")
                && !self.peek().is_kw("ELSE")
                && !self.peek().is_kw("END_IF")
            {
                body.push(self.parse_stmt()?);
            }
            elif_arms.push(ElifArm {
                condition: cond,
                body,
            });
        }

        let mut else_body = Vec::new();
        if self.eat_kw("ELSE").is_some() {
            while !self.is_eof() && !self.peek().is_kw("END_IF") {
                else_body.push(self.parse_stmt()?);
            }
        }

        let end_kw = self.expect_kw("END_IF")?;
        let end_tok = if self.peek().kind == TokenKind::Semi {
            self.bump()
        } else {
            end_kw
        };
        self.attach_inner_and_trailing_comments(end_tok.span.end, end_tok.end_line, &stmt_id);

        Ok(Stmt::new(
            stmt_id,
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            },
            start.span.merge(end_tok.span),
        ))
    }

    fn parse_switch_stmt(&mut self, stmt_id: NodeId) -> Result<Stmt, Vec<Diagnostic>> {
        let start = self.expect_kw("CASE")?;
        let scrutinee = self.parse_expr()?;
        let _ = self.expect_kw("OF")?;

        let mut cases = Vec::new();
        let mut default = None;

        while !self.is_eof() && !self.peek().is_kw("END_CASE") {
            if self.eat_kw("ELSE").is_some() {
                let mut def_body = Vec::new();
                while !self.is_eof() && !self.peek().is_kw("END_CASE") {
                    def_body.push(self.parse_stmt()?);
                }
                default = Some(def_body);
                break;
            }
            let mut patterns = Vec::new();
            loop {
                let first = self.parse_expr()?;
                if self.peek().kind == TokenKind::DotDot {
                    self.bump();
                    let end = self.parse_expr()?;
                    patterns.push(CasePattern::Range { start: first, end });
                } else {
                    patterns.push(CasePattern::Value(first));
                }
                if self.peek().kind == TokenKind::Comma {
                    self.bump();
                } else {
                    break;
                }
            }
            let _ = self.expect_kind(&TokenKind::Colon, "`:`")?;
            let mut body = Vec::new();
            while !self.is_eof()
                && !self.peek().is_kw("ELSE")
                && !self.peek().is_kw("END_CASE")
                && !self.looks_like_case_arm_start()
            {
                body.push(self.parse_stmt()?);
            }
            cases.push(SwitchCase { patterns, body });
        }

        let end_kw = self.expect_kw("END_CASE")?;
        let end_tok = if self.peek().kind == TokenKind::Semi {
            self.bump()
        } else {
            end_kw
        };
        self.attach_inner_and_trailing_comments(end_tok.span.end, end_tok.end_line, &stmt_id);

        Ok(Stmt::new(
            stmt_id,
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            },
            start.span.merge(end_tok.span),
        ))
    }

    fn looks_like_case_arm_start(&self) -> bool {
        // Scan forward on current token sequence up to `:` / `;` / statement keywords
        let first = self.peek();
        if matches!(
            &first.kind,
            TokenKind::Ident(s) if s.eq_ignore_ascii_case("IF")
                || s.eq_ignore_ascii_case("CASE")
                || s.eq_ignore_ascii_case("WHILE")
                || s.eq_ignore_ascii_case("FOR")
                || s.eq_ignore_ascii_case("EXIT")
                || s.eq_ignore_ascii_case("BREAK")
                || s.eq_ignore_ascii_case("CONTINUE")
                || s.eq_ignore_ascii_case("RETURN")
                || s.eq_ignore_ascii_case("WAIT")
                || s.eq_ignore_ascii_case("INVOKE")
                || s.eq_ignore_ascii_case("FOREIGN_STMT")
        ) {
            return false;
        }
        let mut idx = self.pos;
        let mut paren_depth = 0usize;
        while let Some(tok) = self.tokens.get(idx) {
            match tok.kind {
                TokenKind::LParen | TokenKind::LBracket => {
                    paren_depth = paren_depth.saturating_add(1);
                }
                TokenKind::RParen | TokenKind::RBracket => {
                    paren_depth = paren_depth.saturating_sub(1);
                }
                TokenKind::Colon if paren_depth == 0 => return true,
                TokenKind::Semi | TokenKind::ColonEq | TokenKind::Eof => return false,
                TokenKind::Bool(_)
                | TokenKind::Int(_)
                | TokenKind::Float(_)
                | TokenKind::TimeUs(_)
                | TokenKind::String(_)
                | TokenKind::DirectAddr(_)
                | TokenKind::Ident(_)
                | TokenKind::LBrace
                | TokenKind::RBrace
                | TokenKind::Comma
                | TokenKind::Dot
                | TokenKind::DotDot
                | TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Star
                | TokenKind::Slash
                | TokenKind::Lt
                | TokenKind::Le
                | TokenKind::Gt
                | TokenKind::Ge
                | TokenKind::Eq
                | TokenKind::EqEq
                | TokenKind::Ne
                | TokenKind::BangEq => {}
                TokenKind::Colon => {}
            }
            idx = idx.saturating_add(1);
        }
        false
    }

    fn parse_while_stmt(&mut self, stmt_id: NodeId) -> Result<Stmt, Vec<Diagnostic>> {
        let start = self.expect_kw("WHILE")?;
        let condition = self.parse_expr()?;
        let _ = self.expect_kw("DO")?;
        let mut body = Vec::new();
        while !self.is_eof() && !self.peek().is_kw("END_WHILE") {
            body.push(self.parse_stmt()?);
        }
        let end_kw = self.expect_kw("END_WHILE")?;
        let end_tok = if self.peek().kind == TokenKind::Semi {
            self.bump()
        } else {
            end_kw
        };
        self.attach_inner_and_trailing_comments(end_tok.span.end, end_tok.end_line, &stmt_id);
        Ok(Stmt::new(
            stmt_id,
            StmtKind::While { condition, body },
            start.span.merge(end_tok.span),
        ))
    }

    fn parse_for_stmt(&mut self, stmt_id: NodeId) -> Result<Stmt, Vec<Diagnostic>> {
        let start = self.expect_kw("FOR")?;
        let (counter, _) = self.expect_ident("FOR loop counter")?;
        let _ = self.expect_kind(&TokenKind::ColonEq, "`:=`")?;
        let from = self.parse_expr()?;
        let inclusive = if self.eat_kw("TO").is_some() {
            true
        } else if self.eat_kw("UNTIL").is_some() {
            false
        } else {
            let tok = self.peek().clone();
            return Err(self.syntax_err(tok.span, "expected `TO` or `UNTIL` in FOR loop"));
        };
        let to = self.parse_expr()?;
        let step = if self.eat_kw("BY").is_some() {
            self.parse_expr()?
        } else {
            Expr::new(ExprKind::Literal(Literal::Int(1)), start.span)
        };
        let _ = self.expect_kw("DO")?;
        let mut body = Vec::new();
        while !self.is_eof() && !self.peek().is_kw("END_FOR") {
            body.push(self.parse_stmt()?);
        }
        let end_kw = self.expect_kw("END_FOR")?;
        let end_tok = if self.peek().kind == TokenKind::Semi {
            self.bump()
        } else {
            end_kw
        };
        self.attach_inner_and_trailing_comments(end_tok.span.end, end_tok.end_line, &stmt_id);
        Ok(Stmt::new(
            stmt_id,
            StmtKind::For {
                counter,
                from,
                to,
                step,
                inclusive,
                body,
            },
            start.span.merge(end_tok.span),
        ))
    }

    fn parse_call_arg_list(&mut self) -> Result<Vec<CallArg>, Vec<Diagnostic>> {
        let _ = self.expect_kind(&TokenKind::LParen, "`(`")?;
        let mut args = Vec::new();
        if self.peek().kind != TokenKind::RParen {
            loop {
                let arg = self.parse_call_arg()?;
                args.push(arg);
                if self.peek().kind == TokenKind::Comma {
                    self.bump();
                } else {
                    break;
                }
            }
        }
        let _ = self.expect_kind(&TokenKind::RParen, "`)`")?;
        Ok(args)
    }

    fn parse_call_arg(&mut self) -> Result<CallArg, Vec<Diagnostic>> {
        if let TokenKind::Ident(_) = self.peek().kind
            && self
                .peek_offset(1)
                .is_some_and(|t| t.kind == TokenKind::ColonEq)
        {
            let name_tok = self.bump();
            let TokenKind::Ident(name) = name_tok.kind else {
                return Err(self.syntax_err(name_tok.span, "expected argument name"));
            };
            self.bump(); // ':='
            let value = self.parse_expr()?;
            let span = name_tok.span.merge(value.span);
            return Ok(CallArg {
                name: Some(name),
                value,
                span,
            });
        }
        let value = self.parse_expr()?;
        let span = value.span;
        Ok(CallArg {
            name: None,
            value,
            span,
        })
    }

    fn args_to_fb_inputs(
        &self,
        args: Vec<CallArg>,
    ) -> Result<Vec<(String, Expr)>, Vec<Diagnostic>> {
        let mut inputs = Vec::with_capacity(args.len());
        for arg in args {
            let Some(name) = arg.name else {
                return Err(self.syntax_err(
                    arg.span,
                    "Function Block invocation requires named inputs (e.g. `IN := expr`)",
                ));
            };
            inputs.push((name, arg.value));
        }
        Ok(inputs)
    }

    // -----------------------------------------------------------------------
    // LValue & Pratt Expression Parsing (`dendri_frontend_core::pratt`)
    // -----------------------------------------------------------------------

    fn parse_lvalue(&mut self) -> Result<LValue, Vec<Diagnostic>> {
        let (root_name, root_span) = self.expect_ident("assignable target")?;
        let mut lv = LValue::Var {
            name: root_name,
            span: root_span,
        };
        loop {
            if self.peek().kind == TokenKind::LBracket {
                self.bump();
                let index = self.parse_expr()?;
                let close = self.expect_kind(&TokenKind::RBracket, "`]`")?;
                let span = lv.span().merge(close.span);
                lv = LValue::Index {
                    base: Box::new(lv),
                    index: Box::new(index),
                    span,
                };
            } else if self.peek().kind == TokenKind::Dot {
                self.bump();
                let (field, fspan) = self.expect_ident("field name")?;
                let span = lv.span().merge(fspan);
                lv = LValue::Field {
                    base: Box::new(lv),
                    field,
                    span,
                };
            } else {
                break;
            }
        }
        Ok(lv)
    }

    fn try_parse_lvalue_only(&mut self) -> Result<LValue, Vec<Diagnostic>> {
        self.parse_lvalue()
    }

    fn expr_to_lvalue(&self, expr: Expr) -> Result<LValue, Vec<Diagnostic>> {
        let span = expr.span;
        match expr.kind {
            ExprKind::Ident(name) => Ok(LValue::Var { name, span }),
            ExprKind::Index { base, index } => Ok(LValue::Index {
                base: Box::new(self.expr_to_lvalue(*base)?),
                index,
                span,
            }),
            ExprKind::Field { base, field } => Ok(LValue::Field {
                base: Box::new(self.expr_to_lvalue(*base)?),
                field,
                span,
            }),
            ExprKind::Literal(_)
            | ExprKind::Call { .. }
            | ExprKind::Unary { .. }
            | ExprKind::Binary { .. }
            | ExprKind::Foreign { .. } => {
                Err(self.syntax_err(span, "expected assignable variable or coil target"))
            }
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, Vec<Diagnostic>> {
        let mut driver = SfcExprDriver { parser: self };
        parse_pratt(&mut driver)
    }
}

struct SfcExprDriver<'p, 'a> {
    parser: &'p mut Parser<'a>,
}

impl PrattDriver for SfcExprDriver<'_, '_> {
    type Expr = Expr;
    type PrefixOp = (UnaryOp, Span);
    type InfixOp = (BinaryOp, Span);
    type Error = Vec<Diagnostic>;

    fn peek_prefix(&mut self) -> Option<PrefixSpec<Self::PrefixOp>> {
        let tok = self.parser.peek();
        let op = match &tok.kind {
            TokenKind::Minus => Some(UnaryOp::Neg),
            TokenKind::Plus => Some(UnaryOp::Pos),
            TokenKind::Ident(s) if s.eq_ignore_ascii_case("NOT") => Some(UnaryOp::Not),
            TokenKind::Ident(s) if s.eq_ignore_ascii_case("BIT_NOT") => Some(UnaryOp::BitNot),
            TokenKind::Bool(_)
            | TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::TimeUs(_)
            | TokenKind::String(_)
            | TokenKind::DirectAddr(_)
            | TokenKind::Ident(_)
            | TokenKind::LParen
            | TokenKind::RParen
            | TokenKind::LBracket
            | TokenKind::RBracket
            | TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::Comma
            | TokenKind::Dot
            | TokenKind::DotDot
            | TokenKind::Colon
            | TokenKind::ColonEq
            | TokenKind::Semi
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::Eq
            | TokenKind::EqEq
            | TokenKind::Ne
            | TokenKind::BangEq
            | TokenKind::Eof => None,
        }?;
        // Higher than Multiplicative (prec 9 => l_bp 18, r_bp 19)
        Some(PrefixSpec {
            op: (op, tok.span),
            right_bp: 20,
        })
    }

    fn consume_prefix(&mut self) -> Result<Self::PrefixOp, Self::Error> {
        let spec = self.peek_prefix().ok_or_else(|| {
            self.parser
                .syntax_err(self.parser.peek().span, "expected prefix operator")
        })?;
        self.parser.bump();
        Ok(spec.op)
    }

    fn parse_primary(&mut self) -> Result<Self::Expr, Self::Error> {
        let tok = self.parser.bump();
        match tok.kind {
            TokenKind::Bool(b) => Ok(Expr::new(ExprKind::Literal(Literal::Bool(b)), tok.span)),
            TokenKind::Int(n) => Ok(Expr::new(ExprKind::Literal(Literal::Int(n)), tok.span)),
            TokenKind::Float(f) => Ok(Expr::new(ExprKind::Literal(Literal::Float(f)), tok.span)),
            TokenKind::TimeUs(us) => {
                Ok(Expr::new(ExprKind::Literal(Literal::TimeUs(us)), tok.span))
            }
            TokenKind::String(s) => Ok(Expr::new(ExprKind::Literal(Literal::String(s)), tok.span)),
            TokenKind::Ident(name) => {
                if name.eq_ignore_ascii_case("FOREIGN")
                    && self.parser.peek().kind == TokenKind::LParen
                {
                    let _ = self.parser.expect_kind(&TokenKind::LParen, "`(`")?;
                    let (language, _) = self.parser.expect_string("foreign language string")?;
                    let _ = self.parser.expect_kind(&TokenKind::Comma, "`,`")?;
                    let (text, _) = self.parser.expect_string("foreign expression text")?;
                    let close = self.parser.expect_kind(&TokenKind::RParen, "`)`")?;
                    return Ok(Expr::new(
                        ExprKind::Foreign { language, text },
                        tok.span.merge(close.span),
                    ));
                }
                if self.parser.peek().kind == TokenKind::LParen {
                    let args = self.parser.parse_call_arg_list()?;
                    let end_span = self
                        .parser
                        .tokens
                        .get(self.parser.pos.saturating_sub(1))
                        .map_or(tok.span, |t| t.span);
                    Ok(Expr::new(
                        ExprKind::Call { callee: name, args },
                        tok.span.merge(end_span),
                    ))
                } else {
                    Ok(Expr::new(ExprKind::Ident(name), tok.span))
                }
            }
            TokenKind::LParen => {
                let inner = self.parser.parse_expr()?;
                let close = self.parser.expect_kind(&TokenKind::RParen, "`)`")?;
                Ok(Expr::new(inner.kind, tok.span.merge(close.span)))
            }
            TokenKind::DirectAddr(_)
            | TokenKind::RParen
            | TokenKind::LBracket
            | TokenKind::RBracket
            | TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::Comma
            | TokenKind::Dot
            | TokenKind::DotDot
            | TokenKind::Colon
            | TokenKind::ColonEq
            | TokenKind::Semi
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::Eq
            | TokenKind::EqEq
            | TokenKind::Ne
            | TokenKind::BangEq
            | TokenKind::Eof => Err(self.parser.syntax_err(
                tok.span,
                format!("unexpected token `{:?}` while parsing expression", tok.kind),
            )),
        }
    }

    fn parse_postfix(&mut self, mut expr: Self::Expr) -> Result<Self::Expr, Self::Error> {
        loop {
            if self.parser.peek().kind == TokenKind::LBracket {
                self.parser.bump();
                let index = self.parser.parse_expr()?;
                let close = self.parser.expect_kind(&TokenKind::RBracket, "`]`")?;
                let span = expr.span.merge(close.span);
                expr = Expr::new(
                    ExprKind::Index {
                        base: Box::new(expr),
                        index: Box::new(index),
                    },
                    span,
                );
            } else if self.parser.peek().kind == TokenKind::Dot {
                self.parser.bump();
                let (field, fspan) = self.parser.expect_ident("field or step attribute name")?;
                let span = expr.span.merge(fspan);
                expr = Expr::new(
                    ExprKind::Field {
                        base: Box::new(expr),
                        field,
                    },
                    span,
                );
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn peek_infix(&mut self) -> Option<InfixSpec<Self::InfixOp>> {
        let tok = self.parser.peek();
        let (op, precedence, assoc) = match &tok.kind {
            TokenKind::Ident(s) if s.eq_ignore_ascii_case("OR") => {
                (BinaryOp::Or, 1, Associativity::Left)
            }
            TokenKind::Ident(s) if s.eq_ignore_ascii_case("XOR") => {
                (BinaryOp::Xor, 2, Associativity::Left)
            }
            TokenKind::Ident(s) if s.eq_ignore_ascii_case("AND") => {
                (BinaryOp::And, 3, Associativity::Left)
            }
            TokenKind::Ident(s) if s.eq_ignore_ascii_case("BIT_OR") => {
                (BinaryOp::BitOr, 4, Associativity::Left)
            }
            TokenKind::Ident(s) if s.eq_ignore_ascii_case("BIT_XOR") => {
                (BinaryOp::BitXor, 4, Associativity::Left)
            }
            TokenKind::Ident(s) if s.eq_ignore_ascii_case("BIT_AND") => {
                (BinaryOp::BitAnd, 4, Associativity::Left)
            }
            TokenKind::Eq | TokenKind::EqEq => (BinaryOp::Eq, 5, Associativity::NonAssoc),
            TokenKind::Ne | TokenKind::BangEq => (BinaryOp::Ne, 5, Associativity::NonAssoc),
            TokenKind::Lt => (BinaryOp::Lt, 6, Associativity::NonAssoc),
            TokenKind::Le => (BinaryOp::Le, 6, Associativity::NonAssoc),
            TokenKind::Gt => (BinaryOp::Gt, 6, Associativity::NonAssoc),
            TokenKind::Ge => (BinaryOp::Ge, 6, Associativity::NonAssoc),
            TokenKind::Ident(s) if s.eq_ignore_ascii_case("SHL") => {
                (BinaryOp::Shl, 7, Associativity::Left)
            }
            TokenKind::Ident(s) if s.eq_ignore_ascii_case("SHR") => {
                (BinaryOp::Shr, 7, Associativity::Left)
            }
            TokenKind::Plus => (BinaryOp::Add, 8, Associativity::Left),
            TokenKind::Minus => (BinaryOp::Sub, 8, Associativity::Left),
            TokenKind::Star => (BinaryOp::Mul, 9, Associativity::Left),
            TokenKind::Slash => (BinaryOp::Div, 9, Associativity::Left),
            TokenKind::Ident(s) if s.eq_ignore_ascii_case("MOD") => {
                (BinaryOp::Mod, 9, Associativity::Left)
            }
            TokenKind::Bool(_)
            | TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::TimeUs(_)
            | TokenKind::String(_)
            | TokenKind::DirectAddr(_)
            | TokenKind::Ident(_)
            | TokenKind::LParen
            | TokenKind::RParen
            | TokenKind::LBracket
            | TokenKind::RBracket
            | TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::Comma
            | TokenKind::Dot
            | TokenKind::DotDot
            | TokenKind::Colon
            | TokenKind::ColonEq
            | TokenKind::Semi
            | TokenKind::Eof => return None,
        };
        Some(InfixSpec {
            op: (op, tok.span),
            precedence,
            assoc,
        })
    }

    fn consume_infix(&mut self) -> Result<Self::InfixOp, Self::Error> {
        let spec = self.peek_infix().ok_or_else(|| {
            self.parser
                .syntax_err(self.parser.peek().span, "expected infix operator")
        })?;
        self.parser.bump();
        Ok(spec.op)
    }

    fn build_prefix(
        &mut self,
        op: Self::PrefixOp,
        operand: Self::Expr,
    ) -> Result<Self::Expr, Self::Error> {
        let (unary_op, op_span) = op;
        let span = op_span.merge(operand.span);
        Ok(Expr::new(
            ExprKind::Unary {
                op: unary_op,
                operand: Box::new(operand),
            },
            span,
        ))
    }

    fn build_infix(
        &mut self,
        op: Self::InfixOp,
        left: Self::Expr,
        right: Self::Expr,
    ) -> Result<Self::Expr, Self::Error> {
        let (bin_op, op_span) = op;
        let span = left.span.merge(op_span).merge(right.span);
        Ok(Expr::new(
            ExprKind::Binary {
                op: bin_op,
                left: Box::new(left),
                right: Box::new(right),
            },
            span,
        ))
    }

    fn error_non_assoc_chain(&mut self, _prev: Self::InfixOp, next: Self::InfixOp) -> Self::Error {
        self.parser.syntax_err(
            next.1,
            "chained comparison operators are forbidden in ASL v2; wrap sub-comparisons in parentheses",
        )
    }

    fn error_recursion_limit(&mut self, max_depth: usize) -> Self::Error {
        self.parser.syntax_err(
            self.parser.peek().span,
            format!("expression nesting depth exceeded limit ({max_depth})"),
        )
    }
}

fn infer_kind_from_iec_addr(iec: &str) -> Option<HardwareKind> {
    let upper = iec.to_ascii_uppercase();
    if upper.starts_with("%IX") {
        Some(HardwareKind::DigitalIn)
    } else if upper.starts_with("%QX") {
        Some(HardwareKind::DigitalOut)
    } else if upper.starts_with("%IW") || upper.starts_with("%ID") {
        Some(HardwareKind::AnalogIn)
    } else if upper.starts_with("%QW") || upper.starts_with("%QD") {
        Some(HardwareKind::AnalogOut)
    } else {
        None
    }
}

fn parse_action_qualifier(raw: &str) -> Option<ActionQualifier> {
    let upper = raw.to_ascii_uppercase();
    match upper.as_str() {
        "N" => Some(ActionQualifier::N),
        "S" => Some(ActionQualifier::S),
        "R" => Some(ActionQualifier::R),
        "L" => Some(ActionQualifier::L),
        "D" => Some(ActionQualifier::D),
        "P" => Some(ActionQualifier::P),
        "P1" => Some(ActionQualifier::P1),
        "P0" => Some(ActionQualifier::P0),
        _ => None,
    }
}

fn parse_authoring_language(raw: &str) -> Option<AuthoringLanguage> {
    let upper = raw.to_ascii_uppercase();
    match upper.as_str() {
        "ASL" => Some(AuthoringLanguage::Asl),
        "CPP" => Some(AuthoringLanguage::Cpp),
        "ST" => Some(AuthoringLanguage::St),
        "LADDER" | "LD" => Some(AuthoringLanguage::Ladder),
        "SFC" | "GRAFCET" => Some(AuthoringLanguage::Sfc),
        "MICROPYTHON" => Some(AuthoringLanguage::Micropython),
        "RUST" => Some(AuthoringLanguage::Rust),
        _ => None,
    }
}

fn parse_interrupt_edge(raw: &str) -> Option<InterruptEdge> {
    let upper = raw.to_ascii_uppercase();
    match upper.as_str() {
        "RISING" => Some(InterruptEdge::Rising),
        "FALLING" => Some(InterruptEdge::Falling),
        "CHANGE" => Some(InterruptEdge::Change),
        "LOW" => Some(InterruptEdge::Low),
        "HIGH" => Some(InterruptEdge::High),
        _ => None,
    }
}
