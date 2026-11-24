//! Recursive-descent IEC 61131-3 Structured Text parser built on `dendri-frontend-core`'s
//! table-driven [`PrattDriver`] (`docs/lbc/st.md` §1.2–§1.4, §5–§6).

use crate::lexer::{Token, TokenKind, lex_st};
use dendri_ast::{
    BinaryOp, BoardPresentation, BoardProgram, CallArg, CasePattern, ChannelRange, Comment,
    CommentAnchor, CommentOrigin, CommentPlacement as AstCommentPlacement, DataDecl, ElifArm, Expr,
    ExprKind, FbKind, ForeignFidelity, HardwareBinding, HardwareKind, InterruptEdge, LValue,
    Literal, ModuleDecl, NodeId, ObKind, ParamDecl, ParamDirection, Pou, PouBody, PouKind,
    PullMode, ScalarType, SignalKind, Stmt, StmtKind, StorageScope, StructField, SwitchCase,
    TypeDecl, TypeRef, UnaryOp,
};
use dendri_frontend_core::{
    Associativity, BoardContext, BoardProgramBundle, InfixSpec, LanguageId, PackSymbol,
    PackSymbolKind, PrattDriver, PrefixSpec, RawComment, parse_pratt, resolve_foreign_fidelity,
};
use dendri_span::{Diagnostic, ErrorCode, Span};
use std::collections::{BTreeMap, BTreeSet};

/// Parsed ST unit along with auxiliary metadata used by semantic validation.
#[derive(Debug)]
pub(crate) struct ParsedSt {
    pub bundle: BoardProgramBundle,
    pub typed_literals: BTreeMap<Span, ScalarType>,
    pub pack_expr_calls: BTreeMap<Span, (String, PackSymbol, Vec<CallArg>)>,
    pub pack_stmt_calls: BTreeMap<Span, (String, PackSymbol, Vec<CallArg>)>,
}

/// Recorded AST node span for deterministic `ADR-027` comment anchoring and `blank_lines_before`.
#[derive(Debug, Clone)]
struct AnchorSite {
    id: NodeId,
    span: Span,
    start_line: u32,
    end_line: u32,
    first_tok_idx: usize,
    last_tok_idx: usize,
    is_container: bool,
}

/// Parses IEC 61131-3 Structured Text source into a [`ParsedSt`] bundle + metadata.
pub(crate) fn parse_st(src: &str, ctx: &BoardContext) -> Result<ParsedSt, Vec<Diagnostic>> {
    let lexed = lex_st(src, ctx.st_dialect, &ctx.board_id)?;
    let mut parser = Parser {
        src,
        ctx,
        tokens: lexed.tokens,
        pos: 0,
        next_node_id: 1,
        diagnostics: Vec::new(),
        anchors: Vec::new(),
        typed_literals: BTreeMap::new(),
        pack_expr_calls: BTreeMap::new(),
        pack_stmt_calls: BTreeMap::new(),
        current_fn_name: None,
    };

    let mut program = parser.parse_compilation_unit();
    if !parser.diagnostics.is_empty() {
        return Err(parser.diagnostics);
    }

    // Lower IEC 61131-3 FUNCTION return assignments (`MyFunc := expr;`):
    // tail/fused assignments lower directly to `StmtKind::Return`; non-tail or read-back
    // return assignments allocate a collision-free local `VAR_TEMP` return slot.
    lower_all_function_returns(&mut program, &mut || parser.alloc_id());

    // Post-parse resolution:
    // 1. Convert `TypeRef::Named` referring to declared `FUNCTION_BLOCK`s / pack FBs to `TypeRef::Fb`.
    // 2. Convert `StmtKind::Call` targeting declared FB instances to `StmtKind::FbInvoke`.
    // 3. Lift reads of direct-addressed `AnalogIn` hardware tags into `ADC_CODE(tag)`.
    resolve_program_ast(&mut program, ctx);

    let comments = attach_comments(
        &lexed.comments,
        &parser.anchors,
        &parser.tokens,
        &ctx.board_id,
    );

    let mut blank_lines_before = BTreeMap::new();
    for site in &parser.anchors {
        if let Some(tok) = parser.tokens.get(site.first_tok_idx)
            && tok.blank_lines_before > 0
        {
            let clamped = u8::try_from(tok.blank_lines_before.min(2)).unwrap_or(2);
            blank_lines_before.insert(site.id.clone(), clamped);
        }
    }

    let presentation = BoardPresentation {
        eol_style: lexed.eol_style,
        blank_lines_before,
        origin_files: BTreeMap::new(),
        node_coords: BTreeMap::new(),
    };

    let bundle = BoardProgramBundle {
        program,
        comments,
        assumptions: Vec::new(),
        presentation,
    };

    Ok(ParsedSt {
        bundle,
        typed_literals: parser.typed_literals,
        pack_expr_calls: parser.pack_expr_calls,
        pack_stmt_calls: parser.pack_stmt_calls,
    })
}

struct Parser<'a> {
    src: &'a str,
    ctx: &'a BoardContext,
    tokens: Vec<Token>,
    pos: usize,
    next_node_id: u32,
    diagnostics: Vec<Diagnostic>,
    anchors: Vec<AnchorSite>,
    typed_literals: BTreeMap<Span, ScalarType>,
    pack_expr_calls: BTreeMap<Span, (String, PackSymbol, Vec<CallArg>)>,
    pack_stmt_calls: BTreeMap<Span, (String, PackSymbol, Vec<CallArg>)>,
    current_fn_name: Option<String>,
}

impl<'a> Parser<'a> {
    fn alloc_id(&mut self) -> NodeId {
        let id = NodeId::new(format!("st_{:04}", self.next_node_id));
        self.next_node_id = self.next_node_id.saturating_add(1);
        id
    }

    fn record_anchor(
        &mut self,
        id: &NodeId,
        first_tok_idx: usize,
        last_tok_idx: usize,
        is_container: bool,
    ) -> Span {
        let first = self.tokens.get(first_tok_idx);
        let last = self.tokens.get(last_tok_idx);
        let (span, start_line, end_line) = match (first, last) {
            (Some(f), Some(l)) => (f.span.merge(l.span), f.start_line, l.end_line),
            (Some(f), None) => (f.span, f.start_line, f.end_line),
            (None, _) => (Span::default(), 1, 1),
        };
        self.anchors.push(AnchorSite {
            id: id.clone(),
            span,
            start_line,
            end_line,
            first_tok_idx,
            last_tok_idx,
            is_container,
        });
        span
    }

    fn peek(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .or_else(|| self.tokens.last())
            .unwrap_or(&EOF_FALLBACK)
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    fn peek_nth_kind(&self, offset: usize) -> &TokenKind {
        self.tokens
            .get(self.pos.saturating_add(offset))
            .map_or(&TokenKind::Eof, |t| &t.kind)
    }

    fn is_eof(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Eof)
    }

    fn bump(&mut self) -> Token {
        if let Some(tok) = self.tokens.get(self.pos).cloned() {
            if !matches!(tok.kind, TokenKind::Eof) {
                self.pos = self.pos.saturating_add(1);
            }
            tok
        } else {
            EOF_FALLBACK.clone()
        }
    }

    fn last_tok_idx(&self) -> usize {
        self.pos.saturating_sub(1)
    }

    fn push_error(&mut self, code: ErrorCode, span: Span, msg: impl Into<String>) {
        let mut d = Diagnostic::new(code, msg).with_board_id(&self.ctx.board_id);
        d.span = Some(span);
        self.diagnostics.push(d);
    }

    fn make_error(&self, code: ErrorCode, span: Span, msg: impl Into<String>) -> Diagnostic {
        let mut d = Diagnostic::new(code, msg).with_board_id(&self.ctx.board_id);
        d.span = Some(span);
        d
    }

    fn expect_semicolon(&mut self) -> Result<Span, Diagnostic> {
        if matches!(self.peek_kind(), TokenKind::Semicolon) {
            Ok(self.bump().span)
        } else {
            let tok = self.peek().clone();
            Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                tok.span,
                format!("expected `;`, found {:?}", tok.kind),
            ))
        }
    }

    fn expect_ident(&mut self, what: &str) -> Result<(String, Span), Diagnostic> {
        let tok = self.peek().clone();
        if let TokenKind::Ident(name) = tok.kind {
            let _ = self.bump();
            Ok((name, tok.span))
        } else {
            Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                tok.span,
                format!("expected {what} identifier, found {:?}", tok.kind),
            ))
        }
    }

    fn parse_compilation_unit(&mut self) -> BoardProgram {
        let mut program = BoardProgram::default();
        while !self.is_eof() {
            let kind = self.peek_kind().clone();
            if matches!(kind, TokenKind::Using) {
                match self.parse_using_directive() {
                    Ok(imp) => program.imports.push(imp),
                    Err(d) => {
                        self.diagnostics.push(d);
                        self.recover_to_top_level();
                    }
                }
            } else if matches!(kind, TokenKind::Namespace) {
                match self.parse_namespace() {
                    Ok((module, mut hw)) => {
                        program.hardware.append(&mut hw);
                        program.modules.push(module);
                    }
                    Err(d) => {
                        self.diagnostics.push(d);
                        self.recover_to_top_level();
                    }
                }
            } else if matches!(kind, TokenKind::Type) {
                match self.parse_type_block() {
                    Ok(mut decls) => program.types.append(&mut decls),
                    Err(d) => {
                        self.diagnostics.push(d);
                        self.recover_to_top_level();
                    }
                }
            } else if matches!(kind, TokenKind::VarGlobal) {
                match self.parse_global_var_block() {
                    Ok((mut hw, mut data)) => {
                        program.hardware.append(&mut hw);
                        program.data.append(&mut data);
                    }
                    Err(d) => {
                        self.diagnostics.push(d);
                        self.recover_to_top_level();
                    }
                }
            } else if matches!(kind, TokenKind::Program | TokenKind::OrganizationBlock) {
                match self.parse_program_pou() {
                    Ok((pou, mut hw)) => {
                        program.hardware.append(&mut hw);
                        program.pous.push(pou);
                    }
                    Err(d) => {
                        self.diagnostics.push(d);
                        self.recover_to_top_level();
                    }
                }
            } else if matches!(kind, TokenKind::Function) {
                match self.parse_function_pou() {
                    Ok(pou) => program.pous.push(pou),
                    Err(d) => {
                        self.diagnostics.push(d);
                        self.recover_to_top_level();
                    }
                }
            } else if matches!(kind, TokenKind::FunctionBlock) {
                match self.parse_function_block_or_foreign() {
                    Ok(FbOrForeign::Pou(pou)) => program.pous.push(pou),
                    Ok(FbOrForeign::Foreign(td)) => program.types.push(td),
                    Err(d) => {
                        self.diagnostics.push(d);
                        self.recover_to_top_level();
                    }
                }
            } else if matches!(kind, TokenKind::Interface) {
                match self.parse_foreign_block_decl(
                    TokenKind::Interface,
                    TokenKind::EndInterface,
                    "INTERFACE",
                ) {
                    Ok(td) => program.types.push(td),
                    Err(d) => {
                        self.diagnostics.push(d);
                        self.recover_to_top_level();
                    }
                }
            } else if matches!(kind, TokenKind::Method) {
                match self.parse_foreign_block_decl(
                    TokenKind::Method,
                    TokenKind::EndMethod,
                    "METHOD",
                ) {
                    Ok(td) => program.types.push(td),
                    Err(d) => {
                        self.diagnostics.push(d);
                        self.recover_to_top_level();
                    }
                }
            } else if let TokenKind::VendorPragma(text) = kind {
                let first_idx = self.pos;
                let _ = self.bump();
                let id = self.alloc_id();
                let span = self.record_anchor(&id, first_idx, first_idx, false);
                program.types.push(TypeDecl::Foreign {
                    id,
                    name: None,
                    language: "st".to_owned(),
                    fidelity: ForeignFidelity::F0,
                    text,
                    pack_hint: None,
                    span,
                });
            } else if matches!(kind, TokenKind::Eof) {
                break;
            } else {
                let span = self.peek().span;
                let hint_msg = if let TokenKind::Ident(id) = &kind
                    && (id.eq_ignore_ascii_case("SETUP") || id.eq_ignore_ascii_case("LOOP"))
                {
                    format!(
                        "invented keyword `{id}` is forbidden in IEC 61131-3 ST; use `PROGRAM OB_STARTUP` or `PROGRAM OB_MAIN`"
                    )
                } else {
                    format!(
                        "unexpected token `{kind:?}` at top level (expected `USING`, `TYPE`, `VAR_GLOBAL`, `PROGRAM`, `ORGANIZATION_BLOCK`, `FUNCTION`, `FUNCTION_BLOCK`, or `NAMESPACE`)"
                    )
                };
                self.push_error(ErrorCode::ErrTomlSyntax, span, hint_msg);
                let _ = self.bump();
            }
        }
        program
    }

    fn recover_to_top_level(&mut self) {
        while !self.is_eof() {
            if matches!(
                self.peek_kind(),
                TokenKind::Using
                    | TokenKind::Namespace
                    | TokenKind::Type
                    | TokenKind::VarGlobal
                    | TokenKind::Program
                    | TokenKind::OrganizationBlock
                    | TokenKind::Function
                    | TokenKind::FunctionBlock
                    | TokenKind::Interface
                    | TokenKind::Method
            ) {
                break;
            }
            let _ = self.bump();
        }
    }

    fn parse_using_directive(&mut self) -> Result<String, Diagnostic> {
        let _ = self.bump(); // USING
        let (first, _) = self.expect_ident("module path")?;
        let mut path = first;
        while matches!(self.peek_kind(), TokenKind::Dot) {
            let _ = self.bump();
            if matches!(self.peek_kind(), TokenKind::Star) {
                let _ = self.bump();
                path.push_str(".*");
                break;
            }
            let (seg, _) = self.expect_ident("module segment")?;
            path.push('.');
            path.push_str(&seg);
        }
        let _ = self.expect_semicolon()?;
        Ok(path)
    }

    fn parse_namespace(&mut self) -> Result<(ModuleDecl, Vec<HardwareBinding>), Diagnostic> {
        let first_idx = self.pos;
        let _ = self.bump(); // NAMESPACE
        let (name, _) = self.expect_ident("namespace")?;
        let id = self.alloc_id();
        let mut imports = Vec::new();
        let mut types = Vec::new();
        let mut data = Vec::new();
        let mut pous = Vec::new();
        let mut hardware = Vec::new();

        while !self.is_eof() && !matches!(self.peek_kind(), TokenKind::EndNamespace) {
            let kind = self.peek_kind().clone();
            if matches!(kind, TokenKind::Using) {
                imports.push(self.parse_using_directive()?);
            } else if matches!(kind, TokenKind::Type) {
                types.append(&mut self.parse_type_block()?);
            } else if matches!(kind, TokenKind::VarGlobal) {
                let (mut hw, mut d) = self.parse_global_var_block()?;
                hardware.append(&mut hw);
                data.append(&mut d);
            } else if matches!(kind, TokenKind::Program | TokenKind::OrganizationBlock) {
                let (pou, mut hw) = self.parse_program_pou()?;
                hardware.append(&mut hw);
                pous.push(pou);
            } else if matches!(kind, TokenKind::Function) {
                pous.push(self.parse_function_pou()?);
            } else if matches!(kind, TokenKind::FunctionBlock) {
                match self.parse_function_block_or_foreign()? {
                    FbOrForeign::Pou(pou) => pous.push(pou),
                    FbOrForeign::Foreign(td) => types.push(td),
                }
            } else if matches!(kind, TokenKind::Interface) {
                types.push(self.parse_foreign_block_decl(
                    TokenKind::Interface,
                    TokenKind::EndInterface,
                    "INTERFACE",
                )?);
            } else if let TokenKind::VendorPragma(text) = kind {
                let p_idx = self.pos;
                let _ = self.bump();
                let pid = self.alloc_id();
                let pspan = self.record_anchor(&pid, p_idx, p_idx, false);
                types.push(TypeDecl::Foreign {
                    id: pid,
                    name: None,
                    language: "st".to_owned(),
                    fidelity: ForeignFidelity::F0,
                    text,
                    pack_hint: None,
                    span: pspan,
                });
            } else {
                return Err(self.make_error(
                    ErrorCode::ErrTomlSyntax,
                    self.peek().span,
                    format!("unexpected token `{kind:?}` inside `NAMESPACE {name}`"),
                ));
            }
        }

        if !matches!(self.peek_kind(), TokenKind::EndNamespace) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                format!("expected `END_NAMESPACE` for namespace `{name}`"),
            ));
        }
        let _ = self.bump();
        let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), true);
        Ok((
            ModuleDecl {
                id,
                name,
                imports,
                exports: Vec::new(),
                types,
                data,
                pous,
                span,
            },
            hardware,
        ))
    }

    fn parse_type_block(&mut self) -> Result<Vec<TypeDecl>, Diagnostic> {
        let block_start = self.peek().span;
        let _ = self.bump(); // TYPE
        let mut decls = Vec::new();

        while !self.is_eof() && !matches!(self.peek_kind(), TokenKind::EndType) {
            let decl_first_idx = self.pos;
            let (name, _) = self.expect_ident("type")?;
            if !matches!(self.peek_kind(), TokenKind::Colon) {
                return Err(self.make_error(
                    ErrorCode::ErrTomlSyntax,
                    self.peek().span,
                    format!("expected `:` after type name `{name}`"),
                ));
            }
            let _ = self.bump(); // :

            if matches!(self.peek_kind(), TokenKind::Struct) {
                let _ = self.bump(); // STRUCT
                let mut fields = Vec::new();
                while !self.is_eof() && !matches!(self.peek_kind(), TokenKind::EndStruct) {
                    let (fname, _) = self.expect_ident("struct field")?;
                    if !matches!(self.peek_kind(), TokenKind::Colon) {
                        return Err(self.make_error(
                            ErrorCode::ErrTomlSyntax,
                            self.peek().span,
                            format!("expected `:` after struct field `{fname}`"),
                        ));
                    }
                    let _ = self.bump();
                    let fty = self.parse_type_spec()?;
                    let _ = self.expect_semicolon()?;
                    fields.push(StructField {
                        name: fname,
                        ty: fty,
                    });
                }
                if !matches!(self.peek_kind(), TokenKind::EndStruct) {
                    return Err(self.make_error(
                        ErrorCode::ErrTomlSyntax,
                        self.peek().span,
                        format!("expected `END_STRUCT` in type `{name}`"),
                    ));
                }
                let _ = self.bump(); // END_STRUCT
                let _ = self.expect_semicolon()?;
                let id = self.alloc_id();
                let span = self.record_anchor(&id, decl_first_idx, self.last_tok_idx(), false);
                decls.push(TypeDecl::Struct {
                    id,
                    name,
                    fields,
                    span,
                });
            } else if matches!(self.peek_kind(), TokenKind::Pointer | TokenKind::RefTo) {
                // Preserve `POINTER TO` / `REF_TO` type alias verbatim as `Decl::Foreign(F0)`
                while !self.is_eof()
                    && !matches!(self.peek_kind(), TokenKind::Semicolon | TokenKind::EndType)
                {
                    let _ = self.bump();
                }
                let _ = self.expect_semicolon()?;
                let id = self.alloc_id();
                let span = self.record_anchor(&id, decl_first_idx, self.last_tok_idx(), false);
                let raw = span.slice(self.src).unwrap_or("").to_owned();
                let full_text = format!("TYPE\n    {raw}\nEND_TYPE");
                decls.push(TypeDecl::Foreign {
                    id,
                    name: Some(name),
                    language: "st".to_owned(),
                    fidelity: ForeignFidelity::F0,
                    text: full_text,
                    pack_hint: None,
                    span,
                });
            } else if matches!(self.peek_kind(), TokenKind::LParen | TokenKind::Ident(_)) {
                // Enum definition: `[BackingScalar] ( A [:= 0], B [:= 1] );`
                let backing = if matches!(self.peek_kind(), TokenKind::LParen) {
                    ScalarType::Dint
                } else {
                    let ty_span = self.peek().span;
                    let ty = self.parse_type_spec()?;
                    if let TypeRef::Scalar { ty: s } = &ty
                        && (s.is_signed_int() || s.is_unsigned_int() || s.is_bitstring())
                    {
                        s.clone()
                    } else {
                        return Err(self.make_error(
                            ErrorCode::ErrUnknownType,
                            ty_span,
                            format!("enum `{name}` requires an integer backing type, found `{ty}`"),
                        ));
                    }
                };

                if !matches!(self.peek_kind(), TokenKind::LParen) {
                    return Err(self.make_error(
                        ErrorCode::ErrTomlSyntax,
                        self.peek().span,
                        format!("expected `(` starting enum variants for `{name}`"),
                    ));
                }
                let _ = self.bump(); // (

                let mut variants = Vec::new();
                let mut next_val: i64 = 0;
                while !self.is_eof() && !matches!(self.peek_kind(), TokenKind::RParen) {
                    let (vname, _) = self.expect_ident("enum variant")?;
                    let val = if matches!(self.peek_kind(), TokenKind::ColonEq) {
                        let _ = self.bump();
                        let neg = matches!(self.peek_kind(), TokenKind::Minus);
                        if neg || matches!(self.peek_kind(), TokenKind::Plus) {
                            let _ = self.bump();
                        }
                        let tok = self.peek().clone();
                        if let TokenKind::Int { value, .. } = tok.kind {
                            let _ = self.bump();
                            let signed = if neg { -value } else { value };
                            i64::try_from(signed).map_err(|_| {
                                self.make_error(
                                    ErrorCode::ErrTypeMismatch,
                                    tok.span,
                                    format!("enum variant `{vname}` value out of i64 range"),
                                )
                            })?
                        } else {
                            return Err(self.make_error(
                                ErrorCode::ErrTomlSyntax,
                                tok.span,
                                format!("expected integer value for enum variant `{vname}`"),
                            ));
                        }
                    } else {
                        next_val
                    };
                    next_val = val.saturating_add(1);
                    variants.push(dendri_ast::EnumVariant {
                        name: vname,
                        value: val,
                    });
                    if matches!(self.peek_kind(), TokenKind::Comma) {
                        let _ = self.bump();
                    } else {
                        break;
                    }
                }

                if !matches!(self.peek_kind(), TokenKind::RParen) {
                    return Err(self.make_error(
                        ErrorCode::ErrTomlSyntax,
                        self.peek().span,
                        format!("expected `)` closing enum `{name}`"),
                    ));
                }
                let _ = self.bump(); // )
                let _ = self.expect_semicolon()?;
                let id = self.alloc_id();
                let span = self.record_anchor(&id, decl_first_idx, self.last_tok_idx(), false);
                decls.push(TypeDecl::Enum {
                    id,
                    name,
                    backing,
                    variants,
                    span,
                });
            } else {
                let other = self.peek_kind().clone();
                return Err(self.make_error(
                    ErrorCode::ErrTomlSyntax,
                    self.peek().span,
                    format!(
                        "expected `STRUCT`, enum `(`, or `POINTER TO` in type `{name}`, found `{other:?}`"
                    ),
                ));
            }
        }

        if !matches!(self.peek_kind(), TokenKind::EndType) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                block_start,
                "expected `END_TYPE` to close `TYPE` block",
            ));
        }
        let _ = self.bump(); // END_TYPE
        Ok(decls)
    }

    fn parse_global_var_block(
        &mut self,
    ) -> Result<(Vec<HardwareBinding>, Vec<DataDecl>), Diagnostic> {
        let block_start = self.peek().span;
        let _ = self.bump(); // VAR_GLOBAL
        let scope = self.parse_optional_var_qualifier(StorageScope::Var);
        let mut hardware = Vec::new();
        let mut data = Vec::new();

        while !self.is_eof() && !matches!(self.peek_kind(), TokenKind::EndVar) {
            match self.parse_var_decl_item(scope)? {
                VarItem::Hardware(hw) => hardware.push(hw),
                VarItem::Data(d) => data.push(d),
            }
        }

        if !matches!(self.peek_kind(), TokenKind::EndVar) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                block_start,
                "expected `END_VAR` to close `VAR_GLOBAL` block",
            ));
        }
        let _ = self.bump();
        Ok((hardware, data))
    }

    fn parse_optional_var_qualifier(&mut self, default_scope: StorageScope) -> StorageScope {
        if matches!(self.peek_kind(), TokenKind::Constant) {
            let _ = self.bump();
            StorageScope::Const
        } else if matches!(self.peek_kind(), TokenKind::Retain) {
            let _ = self.bump();
            StorageScope::Retain
        } else if matches!(self.peek_kind(), TokenKind::Persistent) {
            let _ = self.bump();
            StorageScope::Persistent
        } else {
            default_scope
        }
    }

    fn parse_var_decl_item(&mut self, scope: StorageScope) -> Result<VarItem, Diagnostic> {
        let first_idx = self.pos;
        let (name, _name_span) = self.expect_ident("variable")?;

        // Optional direct hardware addressing: `AT %IX0.0`
        let direct_addr = if matches!(self.peek_kind(), TokenKind::At) {
            let _ = self.bump();
            let tok = self.peek().clone();
            if let TokenKind::DirectAddress(addr) = tok.kind {
                let _ = self.bump();
                Some((addr, tok.span))
            } else {
                return Err(self.make_error(
                    ErrorCode::ErrTomlSyntax,
                    tok.span,
                    format!(
                        "expected IEC direct address (`%IX...`, `%QX...`, `%IW...`) after `AT`, found `{:?}`",
                        tok.kind
                    ),
                ));
            }
        } else {
            None
        };

        if !matches!(self.peek_kind(), TokenKind::Colon) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                format!("expected `:` after variable `{name}`"),
            ));
        }
        let _ = self.bump(); // :

        let ty = self.parse_type_spec()?;
        let initial = if matches!(self.peek_kind(), TokenKind::ColonEq) {
            let _ = self.bump();
            Some(self.parse_expr()?)
        } else {
            None
        };
        let _ = self.expect_semicolon()?;

        let id = self.alloc_id();
        let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), false);

        if let Some((addr, addr_span)) = direct_addr {
            let hw = self.build_hardware_binding(&id, name, &addr, addr_span, &ty, span)?;
            Ok(VarItem::Hardware(hw))
        } else {
            Ok(VarItem::Data(DataDecl {
                id,
                name,
                scope,
                ty,
                initial,
                span,
            }))
        }
    }

    fn build_hardware_binding(
        &self,
        id: &NodeId,
        label: String,
        iec_addr: &str,
        addr_span: Span,
        ty: &TypeRef,
        decl_span: Span,
    ) -> Result<HardwareBinding, Diagnostic> {
        let Some(port) = self.ctx.port_for_iec(iec_addr) else {
            return Err(self.make_error(
                ErrorCode::ErrUnknownPort,
                addr_span,
                format!(
                    "IEC direct address `{iec_addr}` is not mapped on board profile `{}`",
                    self.ctx.profile_id
                ),
            ));
        };

        let bytes = iec_addr.as_bytes();
        let area = bytes.get(1).copied().unwrap_or(b'?');
        let size = bytes.get(2).copied().unwrap_or(b'?');

        let port_is_digital_in = port.starts_with("Ia.") || port.starts_with("DI");
        let port_is_digital_out = port.starts_with("Qa.") || port.starts_with("DQ");
        let port_is_analog_in = port.starts_with("IW") || port.starts_with("AI");
        let port_is_analog_out = port.starts_with("QW") || port.starts_with("AO");

        let kind = match (area, size, ty) {
            (
                b'I',
                b'X',
                TypeRef::Scalar {
                    ty: ScalarType::Bool,
                },
            ) => {
                if port_is_digital_out || port_is_analog_in || port_is_analog_out {
                    return Err(self.make_error(
                        ErrorCode::ErrPinCapabilityMismatch,
                        addr_span,
                        format!(
                            "port `{port}` (`{iec_addr}`) does not support digital input (`%IX`)"
                        ),
                    ));
                }
                HardwareKind::DigitalIn
            }
            (
                b'Q',
                b'X',
                TypeRef::Scalar {
                    ty: ScalarType::Bool,
                },
            ) => {
                if port_is_digital_in || port_is_analog_in {
                    return Err(self.make_error(
                        ErrorCode::ErrPinCapabilityMismatch,
                        addr_span,
                        format!(
                            "port `{port}` (`{iec_addr}`) is input-only and cannot bind `%QX` output"
                        ),
                    ));
                }
                HardwareKind::DigitalOut
            }
            (
                b'Q',
                b'X',
                TypeRef::Scalar {
                    ty: ScalarType::Duty,
                },
            ) => {
                if port_is_digital_in || port_is_analog_in {
                    return Err(self.make_error(
                        ErrorCode::ErrPinCapabilityMismatch,
                        addr_span,
                        format!("port `{port}` (`{iec_addr}`) cannot bind PWM output"),
                    ));
                }
                if self.ctx.profile_id == "plc-1200-open-1214" && !matches!(port, "Qa.0" | "Qa.1") {
                    return Err(self.make_error(
                        ErrorCode::ErrPinCapabilityMismatch,
                        addr_span,
                        format!(
                            "port `{port}` on `plc-1200-open-1214` does not support PWM output"
                        ),
                    ));
                }
                HardwareKind::PwmOut
            }
            (
                b'I',
                b'W',
                TypeRef::Scalar {
                    ty: ScalarType::Int | ScalarType::Analog | ScalarType::Code { .. },
                },
            ) => {
                if port_is_digital_in || port_is_digital_out || port_is_analog_out {
                    return Err(self.make_error(
                        ErrorCode::ErrPinCapabilityMismatch,
                        addr_span,
                        format!(
                            "port `{port}` (`{iec_addr}`) does not support analog input (`%IW`)"
                        ),
                    ));
                }
                HardwareKind::AnalogIn
            }
            (
                b'Q',
                b'W',
                TypeRef::Scalar {
                    ty: ScalarType::Int | ScalarType::Analog | ScalarType::Code { .. },
                },
            ) => {
                if port_is_digital_in || port_is_analog_in {
                    return Err(self.make_error(
                        ErrorCode::ErrPinCapabilityMismatch,
                        addr_span,
                        format!("port `{port}` (`{iec_addr}`) cannot bind analog output (`%QW`)"),
                    ));
                }
                HardwareKind::AnalogOut
            }
            (
                b'I' | b'Q' | b'M',
                b'B',
                TypeRef::Scalar {
                    ty: ScalarType::Byte,
                },
            ) => HardwareKind::BusPin,
            _ => {
                return Err(self.make_error(
                    ErrorCode::ErrPinCapabilityMismatch,
                    decl_span,
                    format!(
                        "incompatible ST type `{ty}` for IEC direct address `{iec_addr}` on port `{port}`"
                    ),
                ));
            }
        };

        let (codec, range) = match kind {
            HardwareKind::AnalogIn => {
                let (codec_id, low, high) = if port == "IW66" {
                    (
                        self.ctx
                            .codec("plc_bipolar_27648")
                            .or(self.ctx.default_adc_codec.as_ref())
                            .map(|c| c.id.clone()),
                        -10.0,
                        10.0,
                    )
                } else if port == "IW64" {
                    (
                        self.ctx
                            .codec("plc_span_27648")
                            .or(self.ctx.default_adc_codec.as_ref())
                            .map(|c| c.id.clone()),
                        0.0,
                        10.0,
                    )
                } else {
                    (
                        self.ctx.default_adc_codec.as_ref().map(|c| c.id.clone()),
                        0.0,
                        10.0,
                    )
                };
                (
                    codec_id,
                    Some(ChannelRange {
                        signal: SignalKind::Voltage,
                        low,
                        high,
                        live_zero: false,
                        hardware_normalizes_live_zero: false,
                    }),
                )
            }
            HardwareKind::AnalogOut => (
                self.ctx.default_adc_codec.as_ref().map(|c| c.id.clone()),
                Some(ChannelRange {
                    signal: SignalKind::Voltage,
                    low: 0.0,
                    high: 10.0,
                    live_zero: false,
                    hardware_normalizes_live_zero: false,
                }),
            ),
            HardwareKind::PwmOut => (
                self.ctx
                    .codec("plc_span_27648")
                    .or_else(|| self.ctx.codec("pwm_8bit"))
                    .or(self.ctx.default_adc_codec.as_ref())
                    .map(|c| c.id.clone()),
                None,
            ),
            HardwareKind::DigitalIn
            | HardwareKind::DigitalOut
            | HardwareKind::DigitalBidirectional
            | HardwareKind::BusPin => (None, None),
        };

        Ok(HardwareBinding {
            id: id.clone(),
            label,
            pin: port.to_owned(),
            kind,
            pull: PullMode::None,
            active_low: false,
            codec,
            range,
            span: decl_span,
        })
    }

    fn parse_program_pou(&mut self) -> Result<(Pou, Vec<HardwareBinding>), Diagnostic> {
        let first_idx = self.pos;
        let is_ob_kw = matches!(self.peek_kind(), TokenKind::OrganizationBlock);
        let _ = self.bump(); // PROGRAM | ORGANIZATION_BLOCK
        let (name, _) = self.expect_ident("program")?;
        let id = self.alloc_id();
        let ob = classify_ob_kind(&name);

        let (params, locals, hardware) = self.parse_pou_var_blocks(false)?;
        let stmts = self.parse_stmt_list_until(|k| {
            matches!(k, TokenKind::EndProgram | TokenKind::EndOrganizationBlock)
        })?;

        let matched_end = if is_ob_kw {
            matches!(
                self.peek_kind(),
                TokenKind::EndOrganizationBlock | TokenKind::EndProgram
            )
        } else {
            matches!(self.peek_kind(), TokenKind::EndProgram)
        };
        if !matched_end {
            let expected = if is_ob_kw {
                "END_ORGANIZATION_BLOCK"
            } else {
                "END_PROGRAM"
            };
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                format!("expected `{expected}` for `{name}`"),
            ));
        }
        let _ = self.bump();
        let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), true);

        Ok((
            Pou {
                id,
                name,
                kind: PouKind::Ob { ob },
                preferred_view: None,
                params,
                locals,
                body: PouBody::Statements(stmts),
                span,
            },
            hardware,
        ))
    }

    fn parse_function_pou(&mut self) -> Result<Pou, Diagnostic> {
        let first_idx = self.pos;
        let _ = self.bump(); // FUNCTION
        let (name, _) = self.expect_ident("function")?;
        if !matches!(self.peek_kind(), TokenKind::Colon) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                format!("expected `: <ReturnType>` after `FUNCTION {name}`"),
            ));
        }
        let _ = self.bump(); // :
        let return_type = self.parse_type_spec()?;
        let id = self.alloc_id();

        let (params, locals, _hw) = self.parse_pou_var_blocks(true)?;
        let prev_fn = self.current_fn_name.replace(name.clone());
        let stmts_res = self.parse_stmt_list_until(|k| matches!(k, TokenKind::EndFunction));
        self.current_fn_name = prev_fn;
        let stmts = stmts_res?;

        if !matches!(self.peek_kind(), TokenKind::EndFunction) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                format!("expected `END_FUNCTION` for `{name}`"),
            ));
        }
        let _ = self.bump();
        let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), true);

        Ok(Pou {
            id,
            name,
            kind: PouKind::Fc { return_type },
            preferred_view: None,
            params,
            locals,
            body: PouBody::Statements(stmts),
            span,
        })
    }

    fn parse_function_block_or_foreign(&mut self) -> Result<FbOrForeign, Diagnostic> {
        let first_idx = self.pos;
        let start_tok = self.peek().clone();
        // Lookahead to detect Ed. 3 OOP (`EXTENDS`, `IMPLEMENTS`, or nested `METHOD`)
        if self.fb_contains_oop_constructs(first_idx) {
            let td = self.parse_foreign_block_decl(
                TokenKind::FunctionBlock,
                TokenKind::EndFunctionBlock,
                "FUNCTION_BLOCK",
            )?;
            return Ok(FbOrForeign::Foreign(td));
        }

        let _ = self.bump(); // FUNCTION_BLOCK
        let (name, _) = self.expect_ident("function block")?;
        let id = self.alloc_id();
        let (params, locals, _hw) = self.parse_pou_var_blocks(false)?;
        let stmts = self.parse_stmt_list_until(|k| matches!(k, TokenKind::EndFunctionBlock))?;

        if !matches!(self.peek_kind(), TokenKind::EndFunctionBlock) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                start_tok.span,
                format!("expected `END_FUNCTION_BLOCK` for `{name}`"),
            ));
        }
        let _ = self.bump();
        let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), true);

        Ok(FbOrForeign::Pou(Pou {
            id,
            name,
            kind: PouKind::Fb,
            preferred_view: None,
            params,
            locals,
            body: PouBody::Statements(stmts),
            span,
        }))
    }

    fn fb_contains_oop_constructs(&self, start_idx: usize) -> bool {
        let mut idx = start_idx.saturating_add(1);
        while let Some(tok) = self.tokens.get(idx) {
            if matches!(tok.kind, TokenKind::EndFunctionBlock | TokenKind::Eof) {
                return false;
            }
            if matches!(
                tok.kind,
                TokenKind::Extends | TokenKind::Implements | TokenKind::Method
            ) {
                return true;
            }
            idx = idx.saturating_add(1);
        }
        false
    }

    fn parse_foreign_block_decl(
        &mut self,
        open_kw: TokenKind,
        close_kw: TokenKind,
        label: &str,
    ) -> Result<TypeDecl, Diagnostic> {
        let first_idx = self.pos;
        let open_span = self.peek().span;
        let _ = self.bump(); // open_kw
        let name = if let TokenKind::Ident(n) = self.peek_kind().clone() {
            let _ = self.bump();
            Some(n)
        } else {
            None
        };

        let mut depth = 1usize;
        while !self.is_eof() {
            let kind = self.peek_kind().clone();
            if kind == open_kw {
                depth = depth.saturating_add(1);
                let _ = self.bump();
            } else if kind == close_kw {
                let _ = self.bump();
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let id = self.alloc_id();
                    let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), false);
                    let text = span.slice(self.src).unwrap_or("").to_owned();
                    return Ok(TypeDecl::Foreign {
                        id,
                        name,
                        language: "st".to_owned(),
                        fidelity: ForeignFidelity::F0,
                        text,
                        pack_hint: None,
                        span,
                    });
                }
            } else {
                let _ = self.bump();
            }
        }

        Err(self.make_error(
            ErrorCode::ErrTomlSyntax,
            open_span,
            format!("unterminated `{label}` block"),
        ))
    }

    fn parse_pou_var_blocks(&mut self, is_function: bool) -> Result<PouVarDecls, Diagnostic> {
        let mut params = Vec::new();
        let mut locals = Vec::new();
        let mut hardware = Vec::new();

        loop {
            if matches!(self.peek_kind(), TokenKind::VarInput) {
                self.parse_param_block(ParamDirection::In, &mut params)?;
            } else if matches!(self.peek_kind(), TokenKind::VarOutput) {
                self.parse_param_block(ParamDirection::Out, &mut params)?;
            } else if matches!(self.peek_kind(), TokenKind::VarInOut) {
                self.parse_param_block(ParamDirection::InOut, &mut params)?;
            } else if matches!(self.peek_kind(), TokenKind::Var) {
                let block_start = self.peek().span;
                let _ = self.bump();
                let default_scope = if is_function {
                    StorageScope::Temp
                } else {
                    StorageScope::Var
                };
                let scope = self.parse_optional_var_qualifier(default_scope);
                while !self.is_eof() && !matches!(self.peek_kind(), TokenKind::EndVar) {
                    match self.parse_var_decl_item(scope)? {
                        VarItem::Hardware(hw) => hardware.push(hw),
                        VarItem::Data(d) => locals.push(d),
                    }
                }
                if !matches!(self.peek_kind(), TokenKind::EndVar) {
                    return Err(self.make_error(
                        ErrorCode::ErrTomlSyntax,
                        block_start,
                        "expected `END_VAR` to close `VAR` block",
                    ));
                }
                let _ = self.bump();
            } else if matches!(self.peek_kind(), TokenKind::VarTemp) {
                let block_start = self.peek().span;
                let _ = self.bump();
                while !self.is_eof() && !matches!(self.peek_kind(), TokenKind::EndVar) {
                    match self.parse_var_decl_item(StorageScope::Temp)? {
                        VarItem::Hardware(hw) => hardware.push(hw),
                        VarItem::Data(d) => locals.push(d),
                    }
                }
                if !matches!(self.peek_kind(), TokenKind::EndVar) {
                    return Err(self.make_error(
                        ErrorCode::ErrTomlSyntax,
                        block_start,
                        "expected `END_VAR` to close `VAR_TEMP` block",
                    ));
                }
                let _ = self.bump();
            } else {
                break;
            }
        }

        Ok((params, locals, hardware))
    }

    fn parse_param_block(
        &mut self,
        direction: ParamDirection,
        out: &mut Vec<ParamDecl>,
    ) -> Result<(), Diagnostic> {
        let block_start = self.peek().span;
        let _ = self.bump(); // VAR_INPUT | VAR_OUTPUT | VAR_IN_OUT
        while !self.is_eof() && !matches!(self.peek_kind(), TokenKind::EndVar) {
            let (name, _) = self.expect_ident("parameter")?;
            if !matches!(self.peek_kind(), TokenKind::Colon) {
                return Err(self.make_error(
                    ErrorCode::ErrTomlSyntax,
                    self.peek().span,
                    format!("expected `:` after parameter `{name}`"),
                ));
            }
            let _ = self.bump();
            let ty = self.parse_type_spec()?;
            // Optional default initializer on IEC parameter (accepted on parse)
            if matches!(self.peek_kind(), TokenKind::ColonEq) {
                let _ = self.bump();
                let _ = self.parse_expr()?;
            }
            let _ = self.expect_semicolon()?;
            out.push(ParamDecl {
                name,
                direction,
                ty,
            });
        }
        if !matches!(self.peek_kind(), TokenKind::EndVar) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                block_start,
                "expected `END_VAR` to close parameter block",
            ));
        }
        let _ = self.bump();
        Ok(())
    }

    fn parse_type_spec(&mut self) -> Result<TypeRef, Diagnostic> {
        let tok = self.peek().clone();
        if matches!(tok.kind, TokenKind::Array) {
            let _ = self.bump();
            if !matches!(self.peek_kind(), TokenKind::LBracket) {
                return Err(self.make_error(
                    ErrorCode::ErrTomlSyntax,
                    self.peek().span,
                    "expected `[` after `ARRAY`",
                ));
            }
            let _ = self.bump();
            let lo_span = self.peek().span;
            let lo = self.parse_signed_int_literal("array lower bound")?;
            if !matches!(self.peek_kind(), TokenKind::DotDot) {
                return Err(self.make_error(
                    ErrorCode::ErrTomlSyntax,
                    self.peek().span,
                    "expected `..` in `ARRAY[0..N-1]` bounds",
                ));
            }
            let _ = self.bump();
            let hi_span = self.peek().span;
            let hi = self.parse_signed_int_literal("array upper bound")?;
            if !matches!(self.peek_kind(), TokenKind::RBracket) {
                return Err(self.make_error(
                    ErrorCode::ErrTomlSyntax,
                    self.peek().span,
                    "expected `]` closing `ARRAY` bounds",
                ));
            }
            let _ = self.bump();
            if !matches!(self.peek_kind(), TokenKind::Of) {
                return Err(self.make_error(
                    ErrorCode::ErrTomlSyntax,
                    self.peek().span,
                    "expected `OF` after `ARRAY[...]`",
                ));
            }
            let _ = self.bump();

            if lo != 0 || hi < lo {
                return Err(self.make_error(
                    ErrorCode::ErrArrayBoundsConst,
                    lo_span.merge(hi_span),
                    format!(
                        "ASL arrays require 0-based non-empty bounds `ARRAY[0..N-1]`, found `ARRAY[{lo}..{hi}]`"
                    ),
                ));
            }
            let len_i128 = hi.saturating_sub(lo).saturating_add(1);
            let len = u32::try_from(len_i128).map_err(|_| {
                self.make_error(
                    ErrorCode::ErrArrayBoundsConst,
                    lo_span.merge(hi_span),
                    format!("array length {len_i128} exceeds u32 maximum"),
                )
            })?;
            let elem = self.parse_type_spec()?;
            Ok(TypeRef::Array {
                elem: Box::new(elem),
                len,
            })
        } else if let TokenKind::Ident(raw) = tok.kind {
            let _ = self.bump();
            let upper = raw.to_ascii_uppercase();
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
                    let max_len = if matches!(self.peek_kind(), TokenKind::LBracket) {
                        let _ = self.bump();
                        let len_span = self.peek().span;
                        let n = self.parse_signed_int_literal("STRING capacity")?;
                        if !matches!(self.peek_kind(), TokenKind::RBracket) {
                            return Err(self.make_error(
                                ErrorCode::ErrTomlSyntax,
                                self.peek().span,
                                "expected `]` closing `STRING[N]`",
                            ));
                        }
                        let _ = self.bump();
                        u32::try_from(n).ok().filter(|&v| v > 0).ok_or_else(|| {
                            self.make_error(
                                ErrorCode::ErrArrayBoundsConst,
                                len_span,
                                format!("STRING capacity must be >= 1, found {n}"),
                            )
                        })?
                    } else {
                        255
                    };
                    Ok(TypeRef::scalar(ScalarType::String { max_len }))
                }
                "CODE" => {
                    if !matches!(self.peek_kind(), TokenKind::Lt) {
                        return Err(self.make_error(
                            ErrorCode::ErrTomlSyntax,
                            self.peek().span,
                            "expected `<codec_id>` after `CODE`",
                        ));
                    }
                    let _ = self.bump(); // <
                    let (codec_id, codec_span) = self.expect_ident("codec id")?;
                    if !matches!(self.peek_kind(), TokenKind::Gt) {
                        return Err(self.make_error(
                            ErrorCode::ErrTomlSyntax,
                            self.peek().span,
                            "expected `>` closing `CODE<codec_id>`",
                        ));
                    }
                    let _ = self.bump(); // >
                    if !self.ctx.codecs.is_empty() && !self.ctx.codecs.contains_key(&codec_id) {
                        return Err(self.make_error(
                            ErrorCode::ErrUnknownType,
                            codec_span,
                            format!(
                                "unknown codec `{codec_id}` on board profile `{}`",
                                self.ctx.profile_id
                            ),
                        ));
                    }
                    Ok(TypeRef::scalar(ScalarType::Code { codec_id }))
                }
                // Standard IEC & comm Function Blocks (`01` §2.2)
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
                // Explicitly unsupported IEC types (`docs/lbc/st.md` §1.4)
                "WSTRING" | "DATE" | "TOD" | "TIME_OF_DAY" | "DT" | "DATE_AND_TIME" | "LTIME"
                | "CHAR" | "WCHAR" => Err(self.make_error(
                    ErrorCode::ErrUnknownType,
                    tok.span,
                    format!("unsupported IEC type `{raw}` in asl-core@1"),
                )),
                _ => Ok(TypeRef::Named { name: raw }),
            }
        } else {
            Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                tok.span,
                format!("expected type specification, found `{:?}`", tok.kind),
            ))
        }
    }

    fn parse_signed_int_literal(&mut self, what: &str) -> Result<i128, Diagnostic> {
        let neg = matches!(self.peek_kind(), TokenKind::Minus);
        if neg || matches!(self.peek_kind(), TokenKind::Plus) {
            let _ = self.bump();
        }
        let tok = self.peek().clone();
        if let TokenKind::Int { value, .. } = tok.kind {
            let _ = self.bump();
            Ok(if neg { -value } else { value })
        } else {
            Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                tok.span,
                format!(
                    "expected integer literal for {what}, found `{:?}`",
                    tok.kind
                ),
            ))
        }
    }

    fn parse_and_maybe_fuse_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        let mut stmt = self.parse_stmt()?;
        // Fuse `MyFunc := expr; RETURN;` inside a FUNCTION into a single `StmtKind::Return { value: Some(expr) }`
        if let Some(fn_name) = &self.current_fn_name
            && matches!(self.peek_kind(), TokenKind::Return)
            && matches!(self.peek_nth_kind(1), TokenKind::Semicolon)
            && matches!(
                &stmt.kind,
                StmtKind::Assign {
                    target: LValue::Var { name, .. },
                    ..
                } if name.eq_ignore_ascii_case(fn_name)
            )
            && let StmtKind::Assign { value, .. } = stmt.kind
        {
            let _ = self.bump(); // RETURN
            let semi_span = self.bump().span; // ;
            stmt.kind = StmtKind::Return { value: Some(value) };
            stmt.span = stmt.span.merge(semi_span);
            let new_end = self.last_tok_idx();
            let end_line = self.tokens.get(new_end).map(|t| t.end_line);
            if let Some(last_anchor) = self.anchors.last_mut()
                && last_anchor.id == stmt.id
            {
                last_anchor.last_tok_idx = new_end;
                last_anchor.span = stmt.span;
                if let Some(el) = end_line {
                    last_anchor.end_line = el;
                }
            }
        }
        Ok(stmt)
    }

    fn parse_stmt_list_until<F>(&mut self, mut stop: F) -> Result<Vec<Stmt>, Diagnostic>
    where
        F: FnMut(&TokenKind) -> bool,
    {
        let mut stmts = Vec::new();
        while !self.is_eof() && !stop(self.peek_kind()) {
            stmts.push(self.parse_and_maybe_fuse_stmt()?);
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        let first_idx = self.pos;
        let kind = self.peek_kind().clone();
        if matches!(kind, TokenKind::If) {
            self.parse_if_stmt()
        } else if matches!(kind, TokenKind::Case) {
            self.parse_case_stmt()
        } else if matches!(kind, TokenKind::For) {
            self.parse_for_stmt()
        } else if matches!(kind, TokenKind::While) {
            self.parse_while_stmt()
        } else if matches!(kind, TokenKind::Repeat) {
            self.parse_repeat_stmt()
        } else if matches!(kind, TokenKind::Exit) {
            let _ = self.bump();
            let _ = self.expect_semicolon()?;
            let id = self.alloc_id();
            let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), false);
            Ok(Stmt::new(id, StmtKind::Break, span))
        } else if matches!(kind, TokenKind::Continue) {
            let _ = self.bump();
            let _ = self.expect_semicolon()?;
            let id = self.alloc_id();
            let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), false);
            Ok(Stmt::new(id, StmtKind::Continue, span))
        } else if matches!(kind, TokenKind::Return) {
            let _ = self.bump();
            let _ = self.expect_semicolon()?;
            let id = self.alloc_id();
            let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), false);
            Ok(Stmt::new(id, StmtKind::Return { value: None }, span))
        } else if let TokenKind::VendorPragma(text) = kind {
            let _ = self.bump();
            let id = self.alloc_id();
            let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), false);
            Ok(Stmt::new(
                id,
                StmtKind::Foreign {
                    language: "st".to_owned(),
                    fidelity: ForeignFidelity::F0,
                    text,
                    pack_hint: None,
                },
                span,
            ))
        } else if matches!(kind, TokenKind::Ident(_)) {
            self.parse_assign_or_call_stmt()
        } else {
            Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                format!("unexpected token `{kind:?}` at start of statement"),
            ))
        }
    }

    fn parse_assign_or_call_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        let first_idx = self.pos;
        let lvalue = self.parse_lvalue()?;

        // Pointer dereference assignment (`ptr^ := expr;`) -> `Stmt::Foreign(F0)`
        if matches!(self.peek_kind(), TokenKind::Caret) {
            while !self.is_eof() && !matches!(self.peek_kind(), TokenKind::Semicolon) {
                let _ = self.bump();
            }
            let _ = self.expect_semicolon()?;
            let id = self.alloc_id();
            let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), false);
            let text = span.slice(self.src).unwrap_or("").to_owned();
            return Ok(Stmt::new(
                id,
                StmtKind::Foreign {
                    language: "st".to_owned(),
                    fidelity: ForeignFidelity::F0,
                    text,
                    pack_hint: None,
                },
                span,
            ));
        }

        if matches!(self.peek_kind(), TokenKind::ColonEq) {
            let _ = self.bump(); // :=
            let value = self.parse_expr()?;
            let _ = self.expect_semicolon()?;
            let id = self.alloc_id();
            let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), false);

            return Ok(Stmt::new(
                id,
                StmtKind::Assign {
                    target: lvalue,
                    value,
                },
                span,
            ));
        }

        if matches!(self.peek_kind(), TokenKind::LParen) {
            let _ = self.bump(); // (
            let args = self.parse_arg_list()?;
            if !matches!(self.peek_kind(), TokenKind::RParen) {
                return Err(self.make_error(
                    ErrorCode::ErrTomlSyntax,
                    self.peek().span,
                    "expected `)` closing call argument list",
                ));
            }
            let _ = self.bump(); // )
            let _ = self.expect_semicolon()?;
            let id = self.alloc_id();
            let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), false);

            match lvalue {
                LValue::Var { name, .. } => {
                    // Blocking `WAIT(duration);` statement (`docs/lbc/st.md` §1.4)
                    if name.eq_ignore_ascii_case("WAIT") {
                        let mut iter = args.into_iter();
                        let (Some(first_arg), None) = (iter.next(), iter.next()) else {
                            return Err(self.make_error(
                                ErrorCode::ErrArityMismatch,
                                span,
                                "`WAIT` expects exactly 1 duration argument",
                            ));
                        };
                        return Ok(Stmt::new(
                            id,
                            StmtKind::Wait {
                                duration: first_arg.value,
                            },
                            span,
                        ));
                    }

                    // F1/F2 pack symbol in statement position (when not a built-in IEC FB/fn)
                    let (fidelity, pack_hit) =
                        resolve_foreign_fidelity(LanguageId::St, &name, &self.ctx.loaded_packs);
                    if let Some((pack, sym)) = pack_hit
                        && matches!(sym.kind, PackSymbolKind::Function | PackSymbolKind::Method)
                        && self
                            .ctx
                            .std_catalog()
                            .resolve_symbol(&["std.iec.*"], &name)
                            .is_none()
                    {
                        let text = span.slice(self.src).unwrap_or("").to_owned();
                        self.pack_stmt_calls
                            .insert(span, (pack.pack.id.clone(), sym.clone(), args));
                        return Ok(Stmt::new(
                            id,
                            StmtKind::Foreign {
                                language: "st".to_owned(),
                                fidelity,
                                text,
                                pack_hint: Some(pack.pack.id.clone()),
                            },
                            span,
                        ));
                    }

                    Ok(Stmt::new(id, StmtKind::Call { callee: name, args }, span))
                }
                non_var_lvalue @ (LValue::Index { .. } | LValue::Field { .. }) => {
                    // Indexed or field-accessed FB instance invocation: `timers[0](IN := x, PT := T#1s);`
                    let mut inputs = Vec::with_capacity(args.len());
                    for (idx, arg) in args.into_iter().enumerate() {
                        let param_name = arg.name.unwrap_or_else(|| format!("ARG_{idx}"));
                        inputs.push((param_name, arg.value));
                    }
                    Ok(Stmt::new(
                        id,
                        StmtKind::FbInvoke {
                            instance: non_var_lvalue,
                            inputs,
                        },
                        span,
                    ))
                }
            }
        } else {
            Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                format!(
                    "expected `:=` or `(` after `{}`, found `{:?}`",
                    lvalue.root_name(),
                    self.peek_kind()
                ),
            ))
        }
    }

    fn parse_lvalue(&mut self) -> Result<LValue, Diagnostic> {
        let (name, span) = self.expect_ident("lvalue")?;
        let mut lv = LValue::Var { name, span };
        loop {
            if matches!(self.peek_kind(), TokenKind::LBracket) {
                let _ = self.bump();
                let index = self.parse_expr()?;
                if !matches!(self.peek_kind(), TokenKind::RBracket) {
                    return Err(self.make_error(
                        ErrorCode::ErrTomlSyntax,
                        self.peek().span,
                        "expected `]` closing index expression",
                    ));
                }
                let end = self.bump().span;
                let merged = lv.span().merge(end);
                lv = LValue::Index {
                    base: Box::new(lv),
                    index: Box::new(index),
                    span: merged,
                };
            } else if matches!(self.peek_kind(), TokenKind::Dot) {
                let _ = self.bump();
                let (field, fspan) = self.expect_ident("field")?;
                let merged = lv.span().merge(fspan);
                lv = LValue::Field {
                    base: Box::new(lv),
                    field,
                    span: merged,
                };
            } else {
                break;
            }
        }
        Ok(lv)
    }

    fn parse_arg_list(&mut self) -> Result<Vec<CallArg>, Diagnostic> {
        let mut args = Vec::new();
        if matches!(self.peek_kind(), TokenKind::RParen) {
            return Ok(args);
        }
        loop {
            let arg_start = self.peek().span;
            let (name, value) = if let TokenKind::Ident(param) = self.peek_kind().clone()
                && matches!(
                    self.peek_nth_kind(1),
                    TokenKind::ColonEq | TokenKind::FatArrow
                ) {
                let _ = self.bump();
                let _ = self.bump();
                let expr = self.parse_expr()?;
                (Some(param), expr)
            } else {
                (None, self.parse_expr()?)
            };
            let span = arg_start.merge(value.span);
            args.push(CallArg { name, value, span });
            if matches!(self.peek_kind(), TokenKind::Comma) {
                let _ = self.bump();
            } else {
                break;
            }
        }
        Ok(args)
    }

    fn parse_if_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        let first_idx = self.pos;
        let _ = self.bump(); // IF
        let condition = self.parse_expr()?;
        if !matches!(self.peek_kind(), TokenKind::Then) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                "expected `THEN` after `IF` condition",
            ));
        }
        let _ = self.bump(); // THEN

        let then_body = self.parse_stmt_list_until(|k| {
            matches!(k, TokenKind::Elsif | TokenKind::Else | TokenKind::EndIf)
        })?;

        let mut elif_arms = Vec::new();
        while matches!(self.peek_kind(), TokenKind::Elsif) {
            let _ = self.bump();
            let arm_cond = self.parse_expr()?;
            if !matches!(self.peek_kind(), TokenKind::Then) {
                return Err(self.make_error(
                    ErrorCode::ErrTomlSyntax,
                    self.peek().span,
                    "expected `THEN` after `ELSIF` condition",
                ));
            }
            let _ = self.bump();
            let arm_body = self.parse_stmt_list_until(|k| {
                matches!(k, TokenKind::Elsif | TokenKind::Else | TokenKind::EndIf)
            })?;
            elif_arms.push(ElifArm {
                condition: arm_cond,
                body: arm_body,
            });
        }

        let else_body = if matches!(self.peek_kind(), TokenKind::Else) {
            let _ = self.bump();
            self.parse_stmt_list_until(|k| matches!(k, TokenKind::EndIf))?
        } else {
            Vec::new()
        };

        if !matches!(self.peek_kind(), TokenKind::EndIf) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                "expected `END_IF` to close `IF` statement",
            ));
        }
        let _ = self.bump(); // END_IF
        let _ = self.expect_semicolon()?;
        let id = self.alloc_id();
        let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), true);

        Ok(Stmt::new(
            id,
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            },
            span,
        ))
    }

    fn parse_case_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        let first_idx = self.pos;
        let _ = self.bump(); // CASE
        let scrutinee = self.parse_expr()?;
        if !matches!(self.peek_kind(), TokenKind::Of) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                "expected `OF` after `CASE` expression",
            ));
        }
        let _ = self.bump(); // OF

        let mut cases = Vec::new();
        while !self.is_eof() && !matches!(self.peek_kind(), TokenKind::Else | TokenKind::EndCase) {
            let mut patterns = Vec::new();
            loop {
                let start_expr = self.parse_expr()?;
                if matches!(self.peek_kind(), TokenKind::DotDot) {
                    let _ = self.bump();
                    let end_expr = self.parse_expr()?;
                    patterns.push(CasePattern::Range {
                        start: start_expr,
                        end: end_expr,
                    });
                } else {
                    patterns.push(CasePattern::Value(start_expr));
                }
                if matches!(self.peek_kind(), TokenKind::Comma) {
                    let _ = self.bump();
                } else {
                    break;
                }
            }
            if !matches!(self.peek_kind(), TokenKind::Colon) {
                return Err(self.make_error(
                    ErrorCode::ErrTomlSyntax,
                    self.peek().span,
                    "expected `:` after `CASE` pattern(s)",
                ));
            }
            let _ = self.bump(); // :

            let mut body = Vec::new();
            while !self.is_eof()
                && !matches!(self.peek_kind(), TokenKind::Else | TokenKind::EndCase)
                && !self.looks_like_case_label()
            {
                body.push(self.parse_and_maybe_fuse_stmt()?);
            }
            cases.push(SwitchCase { patterns, body });
        }

        let default = if matches!(self.peek_kind(), TokenKind::Else) {
            let _ = self.bump();
            Some(self.parse_stmt_list_until(|k| matches!(k, TokenKind::EndCase))?)
        } else {
            None
        };

        if !matches!(self.peek_kind(), TokenKind::EndCase) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                "expected `END_CASE` to close `CASE` statement",
            ));
        }
        let _ = self.bump(); // END_CASE
        let _ = self.expect_semicolon()?;
        let id = self.alloc_id();
        let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), true);

        Ok(Stmt::new(
            id,
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            },
            span,
        ))
    }

    /// Lookahead predicate distinguishing the start of a new `CASE` arm (`1:`, `1..5:`, `A, B:`)
    /// from an executable statement (`x := ...;`, `fn(...);`, `IF ...`).
    fn looks_like_case_label(&self) -> bool {
        if matches!(
            self.peek_kind(),
            TokenKind::If
                | TokenKind::Case
                | TokenKind::For
                | TokenKind::While
                | TokenKind::Repeat
                | TokenKind::Exit
                | TokenKind::Continue
                | TokenKind::Return
                | TokenKind::VendorPragma(_)
                | TokenKind::Else
                | TokenKind::EndCase
                | TokenKind::Eof
        ) {
            return false;
        }
        let mut idx = self.pos;
        let mut paren_depth = 0usize;
        while let Some(tok) = self.tokens.get(idx) {
            if matches!(tok.kind, TokenKind::LParen | TokenKind::LBracket) {
                paren_depth = paren_depth.saturating_add(1);
            } else if matches!(tok.kind, TokenKind::RParen | TokenKind::RBracket) {
                paren_depth = paren_depth.saturating_sub(1);
            } else if paren_depth == 0
                && matches!(
                    tok.kind,
                    TokenKind::Colon | TokenKind::DotDot | TokenKind::Comma
                )
            {
                return true;
            } else if matches!(
                tok.kind,
                TokenKind::ColonEq
                    | TokenKind::Semicolon
                    | TokenKind::EndCase
                    | TokenKind::Else
                    | TokenKind::Eof
            ) {
                return false;
            }
            idx = idx.saturating_add(1);
        }
        false
    }

    fn parse_for_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        let first_idx = self.pos;
        let _ = self.bump(); // FOR
        let (counter, _) = self.expect_ident("FOR loop counter")?;
        if !matches!(self.peek_kind(), TokenKind::ColonEq) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                format!("expected `:=` after `FOR {counter}`"),
            ));
        }
        let _ = self.bump();
        let from = self.parse_expr()?;
        if !matches!(self.peek_kind(), TokenKind::To) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                "expected `TO` in `FOR` loop header",
            ));
        }
        let _ = self.bump();
        let to = self.parse_expr()?;
        let step = if matches!(self.peek_kind(), TokenKind::By) {
            let _ = self.bump();
            self.parse_expr()?
        } else {
            Expr::new(ExprKind::Literal(Literal::Int(1)), to.span)
        };
        if !matches!(self.peek_kind(), TokenKind::Do) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                "expected `DO` in `FOR` loop header",
            ));
        }
        let _ = self.bump();
        let body = self.parse_stmt_list_until(|k| matches!(k, TokenKind::EndFor))?;
        if !matches!(self.peek_kind(), TokenKind::EndFor) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                "expected `END_FOR` to close `FOR` loop",
            ));
        }
        let _ = self.bump();
        let _ = self.expect_semicolon()?;
        let id = self.alloc_id();
        let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), true);

        Ok(Stmt::new(
            id,
            StmtKind::For {
                counter,
                from,
                to,
                step,
                inclusive: true,
                body,
            },
            span,
        ))
    }

    fn parse_while_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        let first_idx = self.pos;
        let _ = self.bump(); // WHILE
        let condition = self.parse_expr()?;
        if !matches!(self.peek_kind(), TokenKind::Do) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                "expected `DO` after `WHILE` condition",
            ));
        }
        let _ = self.bump();
        let body = self.parse_stmt_list_until(|k| matches!(k, TokenKind::EndWhile))?;
        if !matches!(self.peek_kind(), TokenKind::EndWhile) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                "expected `END_WHILE` to close `WHILE` loop",
            ));
        }
        let _ = self.bump();
        let _ = self.expect_semicolon()?;
        let id = self.alloc_id();
        let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), true);

        Ok(Stmt::new(id, StmtKind::While { condition, body }, span))
    }

    /// Lowers `REPEAT <stmts> UNTIL <cond> END_REPEAT;` deterministically into post-test
    /// `StmtKind::While { condition: TRUE, body: [<stmts>..., IF <cond> THEN EXIT; END_IF;] }`
    /// (`docs/lbc/st.md` §1.3).
    fn parse_repeat_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        let first_idx = self.pos;
        let repeat_span = self.peek().span;
        let _ = self.bump(); // REPEAT
        let mut body = self.parse_stmt_list_until(|k| matches!(k, TokenKind::Until))?;
        if !matches!(self.peek_kind(), TokenKind::Until) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                "expected `UNTIL` in `REPEAT` loop",
            ));
        }
        let _ = self.bump();
        let until_cond = self.parse_expr()?;
        let until_span = until_cond.span;
        if !matches!(self.peek_kind(), TokenKind::EndRepeat) {
            return Err(self.make_error(
                ErrorCode::ErrTomlSyntax,
                self.peek().span,
                "expected `END_REPEAT` to close `REPEAT` loop",
            ));
        }
        let _ = self.bump();
        let _ = self.expect_semicolon()?;

        // Rewrite direct `CONTINUE;` statements inside the REPEAT body (descending into
        // `If`/`Switch`, NOT nested `For`/`While` loops) so `CONTINUE` evaluates the `UNTIL`
        // exit condition before jumping to the next iteration.
        rewrite_repeat_continues(&mut body, &until_cond, until_span, &mut || self.alloc_id());

        let break_id = self.alloc_id();
        let if_id = self.alloc_id();
        body.push(Stmt::new(
            if_id,
            StmtKind::If {
                condition: until_cond,
                then_body: vec![Stmt::new(break_id, StmtKind::Break, until_span)],
                elif_arms: Vec::new(),
                else_body: Vec::new(),
            },
            until_span,
        ));

        let id = self.alloc_id();
        let span = self.record_anchor(&id, first_idx, self.last_tok_idx(), true);

        Ok(Stmt::new(
            id,
            StmtKind::While {
                condition: Expr::new(ExprKind::Literal(Literal::Bool(true)), repeat_span),
                body,
            },
            span,
        ))
    }

    fn parse_expr(&mut self) -> Result<Expr, Diagnostic> {
        let mut driver = StPrattDriver { parser: self };
        parse_pratt(&mut driver)
    }
}

type PouVarDecls = (Vec<ParamDecl>, Vec<DataDecl>, Vec<HardwareBinding>);

enum VarItem {
    Hardware(HardwareBinding),
    Data(DataDecl),
}

enum FbOrForeign {
    Pou(Pou),
    Foreign(TypeDecl),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StInfixOp {
    Binary(BinaryOp),
    Pow,
}

struct StPrattDriver<'p, 'a> {
    parser: &'p mut Parser<'a>,
}

impl PrattDriver for StPrattDriver<'_, '_> {
    type Expr = Expr;
    type PrefixOp = (UnaryOp, Span);
    type InfixOp = (StInfixOp, Span);
    type Error = Diagnostic;

    fn peek_prefix(&mut self) -> Option<PrefixSpec<Self::PrefixOp>> {
        let tok = self.parser.peek();
        let op = if matches!(tok.kind, TokenKind::Minus) {
            UnaryOp::Neg
        } else if matches!(tok.kind, TokenKind::Plus) {
            UnaryOp::Pos
        } else if matches!(tok.kind, TokenKind::Not) {
            UnaryOp::Not
        } else if matches!(tok.kind, TokenKind::BitNot)
            && !matches!(self.parser.peek_nth_kind(1), TokenKind::LParen)
        {
            UnaryOp::BitNot
        } else {
            return None;
        };
        Some(PrefixSpec {
            op: (op, tok.span),
            right_bp: 190,
        })
    }

    fn consume_prefix(&mut self) -> Result<Self::PrefixOp, Diagnostic> {
        let tok = self.parser.bump();
        let op = if matches!(tok.kind, TokenKind::Minus) {
            UnaryOp::Neg
        } else if matches!(tok.kind, TokenKind::Plus) {
            UnaryOp::Pos
        } else if matches!(tok.kind, TokenKind::Not) {
            UnaryOp::Not
        } else if matches!(tok.kind, TokenKind::BitNot) {
            UnaryOp::BitNot
        } else {
            return Err(self.parser.make_error(
                ErrorCode::ErrTomlSyntax,
                tok.span,
                format!("expected unary operator, found `{:?}`", tok.kind),
            ));
        };
        Ok((op, tok.span))
    }

    fn parse_primary(&mut self) -> Result<Expr, Diagnostic> {
        let tok = self.parser.peek().clone();
        if let TokenKind::Bool(b) = tok.kind {
            let _ = self.parser.bump();
            Ok(Expr::new(ExprKind::Literal(Literal::Bool(b)), tok.span))
        } else if let TokenKind::Int { value, typed } = tok.kind {
            let _ = self.parser.bump();
            if let Some(ty) = typed {
                self.parser.typed_literals.insert(tok.span, ty);
            }
            Ok(Expr::new(ExprKind::Literal(Literal::Int(value)), tok.span))
        } else if let TokenKind::Float { value, typed } = tok.kind {
            let _ = self.parser.bump();
            if let Some(ty) = typed {
                self.parser.typed_literals.insert(tok.span, ty);
            }
            Ok(Expr::new(
                ExprKind::Literal(Literal::Float(value)),
                tok.span,
            ))
        } else if let TokenKind::TimeUs(us) = tok.kind {
            let _ = self.parser.bump();
            Ok(Expr::new(ExprKind::Literal(Literal::TimeUs(us)), tok.span))
        } else if let TokenKind::String(s) = tok.kind {
            let _ = self.parser.bump();
            Ok(Expr::new(ExprKind::Literal(Literal::String(s)), tok.span))
        } else if matches!(tok.kind, TokenKind::LParen) {
            let _ = self.parser.bump();
            let inner = self.parser.parse_expr()?;
            if !matches!(self.parser.peek_kind(), TokenKind::RParen) {
                return Err(self.parser.make_error(
                    ErrorCode::ErrTomlSyntax,
                    self.parser.peek().span,
                    "expected `)` closing parenthesized expression",
                ));
            }
            let end = self.parser.bump().span;
            Ok(Expr::new(inner.kind, tok.span.merge(end)))
        } else if matches!(
            tok.kind,
            TokenKind::Shl
                | TokenKind::Shr
                | TokenKind::BitAnd
                | TokenKind::BitXor
                | TokenKind::BitOr
                | TokenKind::BitNot
        ) && matches!(self.parser.peek_nth_kind(1), TokenKind::LParen)
        {
            // IEC call-syntax bit/shift ops (`SHL(x, n)`, `SHR(x, n)`, `BIT_AND(a, b)`, `BIT_NOT(a)`)
            let kw = self.parser.bump();
            let _ = self.parser.bump(); // (
            let args = self.parser.parse_arg_list()?;
            if !matches!(self.parser.peek_kind(), TokenKind::RParen) {
                return Err(self.parser.make_error(
                    ErrorCode::ErrTomlSyntax,
                    self.parser.peek().span,
                    "expected `)` closing operator call",
                ));
            }
            let end = self.parser.bump().span;
            let span = kw.span.merge(end);
            if matches!(kw.kind, TokenKind::BitNot) {
                let mut iter = args.into_iter();
                let (Some(arg), None) = (iter.next(), iter.next()) else {
                    return Err(self.parser.make_error(
                        ErrorCode::ErrArityMismatch,
                        span,
                        "`BIT_NOT` expects exactly 1 argument",
                    ));
                };
                return Ok(Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::BitNot,
                        operand: Box::new(arg.value),
                    },
                    span,
                ));
            }
            if args.len() != 2 {
                return Err(self.parser.make_error(
                    ErrorCode::ErrArityMismatch,
                    span,
                    format!(
                        "operator `{:?}` expects 2 arguments, got {}",
                        kw.kind,
                        args.len()
                    ),
                ));
            }
            let bin_op = if matches!(kw.kind, TokenKind::Shl) {
                BinaryOp::Shl
            } else if matches!(kw.kind, TokenKind::Shr) {
                BinaryOp::Shr
            } else if matches!(kw.kind, TokenKind::BitAnd) {
                BinaryOp::BitAnd
            } else if matches!(kw.kind, TokenKind::BitXor) {
                BinaryOp::BitXor
            } else {
                BinaryOp::BitOr
            };
            let mut iter = args.into_iter();
            if let (Some(left), Some(right)) = (iter.next(), iter.next()) {
                return Ok(Expr::new(
                    ExprKind::Binary {
                        op: bin_op,
                        left: Box::new(left.value),
                        right: Box::new(right.value),
                    },
                    span,
                ));
            }
            Err(self.parser.make_error(
                ErrorCode::ErrTomlSyntax,
                span,
                "internal argument unpacking error",
            ))
        } else if let TokenKind::Ident(name) = tok.kind {
            let _ = self.parser.bump();
            if matches!(self.parser.peek_kind(), TokenKind::LParen) {
                let _ = self.parser.bump(); // (
                let args = self.parser.parse_arg_list()?;
                if !matches!(self.parser.peek_kind(), TokenKind::RParen) {
                    return Err(self.parser.make_error(
                        ErrorCode::ErrTomlSyntax,
                        self.parser.peek().span,
                        format!("expected `)` closing call to `{name}`"),
                    ));
                }
                let end = self.parser.bump().span;
                let span = tok.span.merge(end);

                // `ADR(x)` preserves as `Expr::Foreign(F0)` (`docs/lbc/st.md` §1.4)
                if name.eq_ignore_ascii_case("ADR") {
                    let text = span.slice(self.parser.src).unwrap_or("").to_owned();
                    return Ok(Expr::new(
                        ExprKind::Foreign {
                            language: "st".to_owned(),
                            text,
                        },
                        span,
                    ));
                }

                // F1/F2 pack symbol recognition (`ctx.lookup_pack_symbol`)
                if let Some((pack, sym)) = self.parser.ctx.lookup_pack_symbol(LanguageId::St, &name)
                    && matches!(sym.kind, PackSymbolKind::Function | PackSymbolKind::Method)
                    && self
                        .parser
                        .ctx
                        .std_catalog()
                        .resolve_symbol(&["std.iec.*"], &name)
                        .is_none()
                {
                    let text = span.slice(self.parser.src).unwrap_or("").to_owned();
                    self.parser
                        .pack_expr_calls
                        .insert(span, (pack.pack.id.clone(), sym.clone(), args));
                    return Ok(Expr::new(
                        ExprKind::Foreign {
                            language: "st".to_owned(),
                            text,
                        },
                        span,
                    ));
                }

                Ok(Expr::new(ExprKind::Call { callee: name, args }, span))
            } else {
                Ok(Expr::new(ExprKind::Ident(name), tok.span))
            }
        } else {
            Err(self.parser.make_error(
                ErrorCode::ErrTomlSyntax,
                tok.span,
                format!("expected expression, found `{:?}`", tok.kind),
            ))
        }
    }

    fn parse_postfix(&mut self, mut expr: Expr) -> Result<Expr, Diagnostic> {
        loop {
            if matches!(self.parser.peek_kind(), TokenKind::LBracket) {
                let _ = self.parser.bump();
                let index = self.parser.parse_expr()?;
                if !matches!(self.parser.peek_kind(), TokenKind::RBracket) {
                    return Err(self.parser.make_error(
                        ErrorCode::ErrTomlSyntax,
                        self.parser.peek().span,
                        "expected `]` closing array index",
                    ));
                }
                let end = self.parser.bump().span;
                let span = expr.span.merge(end);
                expr = Expr::new(
                    ExprKind::Index {
                        base: Box::new(expr),
                        index: Box::new(index),
                    },
                    span,
                );
            } else if matches!(self.parser.peek_kind(), TokenKind::Dot) {
                let _ = self.parser.bump();
                let (field, fspan) = self.parser.expect_ident("field")?;
                let span = expr.span.merge(fspan);
                expr = Expr::new(
                    ExprKind::Field {
                        base: Box::new(expr),
                        field,
                    },
                    span,
                );
            } else if matches!(self.parser.peek_kind(), TokenKind::Caret) {
                // Pointer dereference (`ptr^`) preserves verbatim as `Expr::Foreign(F0)`
                let end = self.parser.bump().span;
                let span = expr.span.merge(end);
                let text = span.slice(self.parser.src).unwrap_or("").to_owned();
                expr = Expr::new(
                    ExprKind::Foreign {
                        language: "st".to_owned(),
                        text,
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
        let (op, precedence, assoc) = if matches!(tok.kind, TokenKind::Or) {
            (StInfixOp::Binary(BinaryOp::Or), 10, Associativity::Left)
        } else if matches!(tok.kind, TokenKind::Xor) {
            (StInfixOp::Binary(BinaryOp::Xor), 20, Associativity::Left)
        } else if matches!(tok.kind, TokenKind::And | TokenKind::Ampersand) {
            (StInfixOp::Binary(BinaryOp::And), 30, Associativity::Left)
        } else if matches!(tok.kind, TokenKind::BitAnd) {
            (StInfixOp::Binary(BinaryOp::BitAnd), 40, Associativity::Left)
        } else if matches!(tok.kind, TokenKind::BitXor) {
            (StInfixOp::Binary(BinaryOp::BitXor), 40, Associativity::Left)
        } else if matches!(tok.kind, TokenKind::BitOr) {
            (StInfixOp::Binary(BinaryOp::BitOr), 40, Associativity::Left)
        } else if matches!(tok.kind, TokenKind::Eq) {
            (StInfixOp::Binary(BinaryOp::Eq), 50, Associativity::NonAssoc)
        } else if matches!(tok.kind, TokenKind::Ne) {
            (StInfixOp::Binary(BinaryOp::Ne), 50, Associativity::NonAssoc)
        } else if matches!(tok.kind, TokenKind::Lt) {
            (StInfixOp::Binary(BinaryOp::Lt), 60, Associativity::NonAssoc)
        } else if matches!(tok.kind, TokenKind::Le) {
            (StInfixOp::Binary(BinaryOp::Le), 60, Associativity::NonAssoc)
        } else if matches!(tok.kind, TokenKind::Gt) {
            (StInfixOp::Binary(BinaryOp::Gt), 60, Associativity::NonAssoc)
        } else if matches!(tok.kind, TokenKind::Ge) {
            (StInfixOp::Binary(BinaryOp::Ge), 60, Associativity::NonAssoc)
        } else if matches!(tok.kind, TokenKind::Shl) {
            (StInfixOp::Binary(BinaryOp::Shl), 70, Associativity::Left)
        } else if matches!(tok.kind, TokenKind::Shr) {
            (StInfixOp::Binary(BinaryOp::Shr), 70, Associativity::Left)
        } else if matches!(tok.kind, TokenKind::Plus) {
            (StInfixOp::Binary(BinaryOp::Add), 80, Associativity::Left)
        } else if matches!(tok.kind, TokenKind::Minus) {
            (StInfixOp::Binary(BinaryOp::Sub), 80, Associativity::Left)
        } else if matches!(tok.kind, TokenKind::Star) {
            (StInfixOp::Binary(BinaryOp::Mul), 90, Associativity::Left)
        } else if matches!(tok.kind, TokenKind::Slash) {
            (StInfixOp::Binary(BinaryOp::Div), 90, Associativity::Left)
        } else if matches!(tok.kind, TokenKind::Mod) {
            (StInfixOp::Binary(BinaryOp::Mod), 90, Associativity::Left)
        } else if matches!(tok.kind, TokenKind::StarStar) {
            (StInfixOp::Pow, 100, Associativity::Right)
        } else {
            return None;
        };
        Some(InfixSpec {
            op: (op, tok.span),
            precedence,
            assoc,
        })
    }

    fn consume_infix(&mut self) -> Result<Self::InfixOp, Diagnostic> {
        let Some(spec) = self.peek_infix() else {
            return Err(self.parser.make_error(
                ErrorCode::ErrTomlSyntax,
                self.parser.peek().span,
                "expected infix operator",
            ));
        };
        let _ = self.parser.bump();
        Ok(spec.op)
    }

    fn build_prefix(&mut self, op: Self::PrefixOp, operand: Expr) -> Result<Expr, Diagnostic> {
        let span = op.1.merge(operand.span);
        Ok(Expr::new(
            ExprKind::Unary {
                op: op.0,
                operand: Box::new(operand),
            },
            span,
        ))
    }

    fn build_infix(
        &mut self,
        op: Self::InfixOp,
        left: Expr,
        right: Expr,
    ) -> Result<Expr, Diagnostic> {
        let span = left.span.merge(right.span);
        match op.0 {
            StInfixOp::Binary(bin_op) => Ok(Expr::new(
                ExprKind::Binary {
                    op: bin_op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            )),
            StInfixOp::Pow => {
                let text = span.slice(self.parser.src).unwrap_or("").to_owned();
                Ok(Expr::new(
                    ExprKind::Foreign {
                        language: "st".to_owned(),
                        text,
                    },
                    span,
                ))
            }
        }
    }

    fn error_non_assoc_chain(&mut self, prev: Self::InfixOp, next: Self::InfixOp) -> Diagnostic {
        self.parser.make_error(
            ErrorCode::ErrTomlSyntax,
            prev.1.merge(next.1),
            "chained comparison or equality operators are forbidden in ASL; add explicit parentheses",
        )
    }

    fn error_recursion_limit(&mut self, max_depth: usize) -> Diagnostic {
        self.parser.make_error(
            ErrorCode::ErrRecursionDepth,
            self.parser.peek().span,
            format!("expression nesting depth exceeded maximum limit ({max_depth})"),
        )
    }
}

/// Maps canonical IEC `PROGRAM` / `ORGANIZATION_BLOCK` names to [`ObKind`] (`docs/lbc/st.md` §1.3).
fn classify_ob_kind(name: &str) -> ObKind {
    let upper = name.to_ascii_uppercase();
    if upper == "OB_STARTUP" || upper == "OB100" {
        return ObKind::Startup;
    }
    if upper == "OB_MAIN" || upper == "OB1" {
        return ObKind::Main;
    }
    if let Some(rest) = upper.strip_prefix("OB_CYCLIC_") {
        let period_us = rest.parse::<u64>().unwrap_or(10_000);
        return ObKind::CyclicInt { period_us };
    }
    if let Some(rest) = upper.strip_prefix("OB_HW_") {
        return parse_hw_ob_suffix(name, rest);
    }
    if let Some(rest) = upper.strip_prefix("OB_COMM_") {
        let orig_bus = name.get(8..).unwrap_or(rest);
        return ObKind::CommEvent {
            bus_id: orig_bus.to_owned(),
        };
    }
    if let Some(num_str) = upper.strip_prefix("OB")
        && let Ok(num) = num_str.parse::<u32>()
    {
        if (30..=39).contains(&num) {
            return ObKind::CyclicInt { period_us: 10_000 };
        }
        if (40..=49).contains(&num) {
            return ObKind::HwInt {
                pin_label: format!("HW_{num}"),
                edge: InterruptEdge::Rising,
                priority: 1,
            };
        }
        if (50..=59).contains(&num) {
            return ObKind::CommEvent {
                bus_id: "ETH0".to_owned(),
            };
        }
    }
    ObKind::Main
}

fn parse_hw_ob_suffix(original_name: &str, upper_suffix: &str) -> ObKind {
    // Support `OB_HW_<pin>_<EDGE>_<prio>` as well as `OB_HW_<pin>`
    let orig_suffix = original_name.get(6..).unwrap_or(upper_suffix);
    let parts: Vec<&str> = orig_suffix.rsplitn(3, '_').collect();
    if parts.len() == 3
        && let (Some(prio_s), Some(edge_s), Some(pin_s)) =
            (parts.first(), parts.get(1), parts.get(2))
        && let Ok(priority) = prio_s.parse::<u8>()
        && let Some(edge) = parse_interrupt_edge(edge_s)
    {
        return ObKind::HwInt {
            pin_label: (*pin_s).to_owned(),
            edge,
            priority,
        };
    }
    ObKind::HwInt {
        pin_label: orig_suffix.to_owned(),
        edge: InterruptEdge::Rising,
        priority: 1,
    }
}

fn parse_interrupt_edge(raw: &str) -> Option<InterruptEdge> {
    match raw.to_ascii_uppercase().as_str() {
        "RISING" => Some(InterruptEdge::Rising),
        "FALLING" => Some(InterruptEdge::Falling),
        "CHANGE" => Some(InterruptEdge::Change),
        "LOW" => Some(InterruptEdge::Low),
        "HIGH" => Some(InterruptEdge::High),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Post-Parse Resolution (User FB types, FB invocations, AnalogIn -> ADC_CODE)
// ---------------------------------------------------------------------------

fn resolve_program_ast(program: &mut BoardProgram, ctx: &BoardContext) {
    // 1. Collect all user FB names & F1 pack FB names (case-insensitive -> canonical spelling)
    let mut fb_names: BTreeMap<String, String> = BTreeMap::new();
    for pou in &program.pous {
        if matches!(pou.kind, PouKind::Fb) {
            fb_names.insert(pou.name.to_ascii_uppercase(), pou.name.clone());
        }
    }
    for m in &program.modules {
        for pou in &m.pous {
            if matches!(pou.kind, PouKind::Fb) {
                fb_names.insert(pou.name.to_ascii_uppercase(), pou.name.clone());
            }
        }
    }
    for pack in &ctx.loaded_packs {
        if pack.pack.language == LanguageId::St {
            for sym in &pack.symbols {
                if sym.kind == PackSymbolKind::Fb {
                    fb_names
                        .entry(sym.name.to_ascii_uppercase())
                        .or_insert_with(|| sym.name.clone());
                }
            }
        }
    }

    // Rewrite TypeRef::Named -> TypeRef::Fb for user/pack FBs
    rewrite_types_in_program(program, &fb_names);

    // 2. Collect AnalogIn hardware tag labels and global FB variable names (case-insensitive)
    let analog_in_tags: BTreeSet<String> = program
        .hardware
        .iter()
        .filter(|h| h.kind == HardwareKind::AnalogIn)
        .map(|h| h.label.to_ascii_uppercase())
        .collect();

    let mut global_fb_vars: BTreeMap<String, TypeRef> = BTreeMap::new();
    for d in &program.data {
        if matches!(d.ty, TypeRef::Fb { .. }) {
            global_fb_vars.insert(d.name.to_ascii_uppercase(), d.ty.clone());
        }
    }
    for m in &program.modules {
        for d in &m.data {
            if matches!(d.ty, TypeRef::Fb { .. }) {
                global_fb_vars.insert(d.name.to_ascii_uppercase(), d.ty.clone());
            }
        }
    }

    // Collect FB IN parameter lists for positional -> named mapping
    let fb_in_params = collect_fb_input_names(program, ctx);

    // 3. Rewrite statements & expressions in all POUs (and module POUs)
    for pou in &mut program.pous {
        resolve_pou_body(pou, &global_fb_vars, &fb_in_params, &analog_in_tags);
    }
    for m in &mut program.modules {
        for pou in &mut m.pous {
            resolve_pou_body(pou, &global_fb_vars, &fb_in_params, &analog_in_tags);
        }
    }
}

fn rewrite_type_ref(ty: &mut TypeRef, fb_names: &BTreeMap<String, String>) {
    match ty {
        TypeRef::Named { name } => {
            if let Some(canonical) = fb_names.get(&name.to_ascii_uppercase()) {
                *ty = TypeRef::Fb {
                    fb_kind: FbKind::User {
                        fb_name: canonical.clone(),
                    },
                };
            }
        }
        TypeRef::Array { elem, .. } => rewrite_type_ref(elem, fb_names),
        TypeRef::Void | TypeRef::Scalar { .. } | TypeRef::Fb { .. } => {}
    }
}

fn rewrite_types_in_program(program: &mut BoardProgram, fb_names: &BTreeMap<String, String>) {
    for td in &mut program.types {
        if let TypeDecl::Struct { fields, .. } = td {
            for f in fields {
                rewrite_type_ref(&mut f.ty, fb_names);
            }
        }
    }
    for d in &mut program.data {
        rewrite_type_ref(&mut d.ty, fb_names);
    }
    for pou in &mut program.pous {
        if let PouKind::Fc { return_type } = &mut pou.kind {
            rewrite_type_ref(return_type, fb_names);
        }
        for p in &mut pou.params {
            rewrite_type_ref(&mut p.ty, fb_names);
        }
        for l in &mut pou.locals {
            rewrite_type_ref(&mut l.ty, fb_names);
        }
    }
    for m in &mut program.modules {
        for td in &mut m.types {
            if let TypeDecl::Struct { fields, .. } = td {
                for f in fields {
                    rewrite_type_ref(&mut f.ty, fb_names);
                }
            }
        }
        for d in &mut m.data {
            rewrite_type_ref(&mut d.ty, fb_names);
        }
        for pou in &mut m.pous {
            if let PouKind::Fc { return_type } = &mut pou.kind {
                rewrite_type_ref(return_type, fb_names);
            }
            for p in &mut pou.params {
                rewrite_type_ref(&mut p.ty, fb_names);
            }
            for l in &mut pou.locals {
                rewrite_type_ref(&mut l.ty, fb_names);
            }
        }
    }
}

fn collect_fb_input_names(
    program: &BoardProgram,
    ctx: &BoardContext,
) -> BTreeMap<String, Vec<String>> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let cat = ctx.std_catalog();
    for module in cat.modules() {
        for fb in module.function_blocks {
            let names = fb
                .fields
                .iter()
                .filter(|f| f.direction != dendri_stdlib::ParamDir::Out)
                .map(|f| f.name.to_owned())
                .collect();
            map.insert(fb.name.to_ascii_uppercase(), names);
        }
    }
    for pack in &ctx.loaded_packs {
        if pack.pack.language == LanguageId::St {
            for sym in &pack.symbols {
                if sym.kind == PackSymbolKind::Fb {
                    let names = sym.params.iter().map(|p| p.name.clone()).collect();
                    map.insert(sym.name.to_ascii_uppercase(), names);
                }
            }
        }
    }
    for pou in program
        .pous
        .iter()
        .chain(program.modules.iter().flat_map(|m| m.pous.iter()))
    {
        if matches!(pou.kind, PouKind::Fb) {
            let names = pou
                .params
                .iter()
                .filter(|p| p.direction != ParamDirection::Out)
                .map(|p| p.name.clone())
                .collect();
            map.insert(pou.name.to_ascii_uppercase(), names);
        }
    }
    map
}

fn resolve_pou_body(
    pou: &mut Pou,
    global_fb_vars: &BTreeMap<String, TypeRef>,
    fb_in_params: &BTreeMap<String, Vec<String>>,
    analog_in_tags: &BTreeSet<String>,
) {
    let mut fb_vars = global_fb_vars.clone();
    for p in &pou.params {
        if matches!(p.ty, TypeRef::Fb { .. }) {
            fb_vars.insert(p.name.to_ascii_uppercase(), p.ty.clone());
        }
    }
    for l in &pou.locals {
        if matches!(l.ty, TypeRef::Fb { .. }) {
            fb_vars.insert(l.name.to_ascii_uppercase(), l.ty.clone());
        }
    }
    let shadowed_locals: BTreeSet<String> = pou
        .params
        .iter()
        .map(|p| p.name.to_ascii_uppercase())
        .chain(pou.locals.iter().map(|l| l.name.to_ascii_uppercase()))
        .collect();

    for l in &mut pou.locals {
        if let Some(init) = &mut l.initial {
            lift_analog_reads_in_expr(init, analog_in_tags, &shadowed_locals, false);
        }
    }

    if let PouBody::Statements(stmts) = &mut pou.body {
        resolve_stmts(
            stmts,
            &fb_vars,
            fb_in_params,
            analog_in_tags,
            &shadowed_locals,
        );
    }
}

fn resolve_stmts(
    stmts: &mut [Stmt],
    fb_vars: &BTreeMap<String, TypeRef>,
    fb_in_params: &BTreeMap<String, Vec<String>>,
    analog_in_tags: &BTreeSet<String>,
    shadowed_locals: &BTreeSet<String>,
) {
    for stmt in stmts {
        let stmt_span = stmt.span;
        match &mut stmt.kind {
            StmtKind::Assign { target, value } => {
                lift_analog_reads_in_lvalue(target, analog_in_tags, shadowed_locals);
                lift_analog_reads_in_expr(value, analog_in_tags, shadowed_locals, false);
            }
            StmtKind::Call { callee, args } => {
                let is_adc = callee.eq_ignore_ascii_case("ADC_CODE");
                for arg in args.iter_mut() {
                    lift_analog_reads_in_expr(
                        &mut arg.value,
                        analog_in_tags,
                        shadowed_locals,
                        is_adc,
                    );
                }
                if let Some(fb_ty) = fb_vars.get(&callee.to_ascii_uppercase()) {
                    let fb_key = fb_ty.to_string().to_ascii_uppercase();
                    let param_names = fb_in_params.get(&fb_key);
                    let mut inputs = Vec::with_capacity(args.len());
                    for (idx, arg) in std::mem::take(args).into_iter().enumerate() {
                        let pname = arg.name.unwrap_or_else(|| {
                            param_names
                                .and_then(|list| list.get(idx))
                                .cloned()
                                .unwrap_or_else(|| format!("IN_{idx}"))
                        });
                        inputs.push((pname, arg.value));
                    }
                    let inst_name = std::mem::take(callee);
                    stmt.kind = StmtKind::FbInvoke {
                        instance: LValue::Var {
                            name: inst_name,
                            span: stmt_span,
                        },
                        inputs,
                    };
                }
            }
            StmtKind::FbInvoke { instance, inputs } => {
                lift_analog_reads_in_lvalue(instance, analog_in_tags, shadowed_locals);
                for (_, expr) in inputs {
                    lift_analog_reads_in_expr(expr, analog_in_tags, shadowed_locals, false);
                }
            }
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => {
                lift_analog_reads_in_expr(condition, analog_in_tags, shadowed_locals, false);
                resolve_stmts(
                    then_body,
                    fb_vars,
                    fb_in_params,
                    analog_in_tags,
                    shadowed_locals,
                );
                for arm in elif_arms {
                    lift_analog_reads_in_expr(
                        &mut arm.condition,
                        analog_in_tags,
                        shadowed_locals,
                        false,
                    );
                    resolve_stmts(
                        &mut arm.body,
                        fb_vars,
                        fb_in_params,
                        analog_in_tags,
                        shadowed_locals,
                    );
                }
                resolve_stmts(
                    else_body,
                    fb_vars,
                    fb_in_params,
                    analog_in_tags,
                    shadowed_locals,
                );
            }
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => {
                lift_analog_reads_in_expr(scrutinee, analog_in_tags, shadowed_locals, false);
                for c in cases {
                    for pat in &mut c.patterns {
                        match pat {
                            CasePattern::Value(e) => {
                                lift_analog_reads_in_expr(
                                    e,
                                    analog_in_tags,
                                    shadowed_locals,
                                    false,
                                );
                            }
                            CasePattern::Range { start, end } => {
                                lift_analog_reads_in_expr(
                                    start,
                                    analog_in_tags,
                                    shadowed_locals,
                                    false,
                                );
                                lift_analog_reads_in_expr(
                                    end,
                                    analog_in_tags,
                                    shadowed_locals,
                                    false,
                                );
                            }
                        }
                    }
                    resolve_stmts(
                        &mut c.body,
                        fb_vars,
                        fb_in_params,
                        analog_in_tags,
                        shadowed_locals,
                    );
                }
                if let Some(def) = default {
                    resolve_stmts(def, fb_vars, fb_in_params, analog_in_tags, shadowed_locals);
                }
            }
            StmtKind::While { condition, body } => {
                lift_analog_reads_in_expr(condition, analog_in_tags, shadowed_locals, false);
                resolve_stmts(body, fb_vars, fb_in_params, analog_in_tags, shadowed_locals);
            }
            StmtKind::For {
                from,
                to,
                step,
                body,
                ..
            } => {
                lift_analog_reads_in_expr(from, analog_in_tags, shadowed_locals, false);
                lift_analog_reads_in_expr(to, analog_in_tags, shadowed_locals, false);
                lift_analog_reads_in_expr(step, analog_in_tags, shadowed_locals, false);
                resolve_stmts(body, fb_vars, fb_in_params, analog_in_tags, shadowed_locals);
            }
            StmtKind::Return { value } => {
                if let Some(expr) = value {
                    lift_analog_reads_in_expr(expr, analog_in_tags, shadowed_locals, false);
                }
            }
            StmtKind::Wait { duration } => {
                lift_analog_reads_in_expr(duration, analog_in_tags, shadowed_locals, false);
            }
            StmtKind::Break | StmtKind::Continue | StmtKind::Foreign { .. } => {}
        }
    }
}

fn lift_analog_reads_in_lvalue(
    lv: &mut LValue,
    analog_in_tags: &BTreeSet<String>,
    shadowed_locals: &BTreeSet<String>,
) {
    match lv {
        LValue::Var { .. } => {}
        LValue::Index { base, index, .. } => {
            lift_analog_reads_in_lvalue(base, analog_in_tags, shadowed_locals);
            lift_analog_reads_in_expr(index, analog_in_tags, shadowed_locals, false);
        }
        LValue::Field { base, .. } => {
            lift_analog_reads_in_lvalue(base, analog_in_tags, shadowed_locals);
        }
    }
}

fn lift_analog_reads_in_expr(
    expr: &mut Expr,
    analog_in_tags: &BTreeSet<String>,
    shadowed_locals: &BTreeSet<String>,
    inside_adc_code_arg: bool,
) {
    let span = expr.span;
    match &mut expr.kind {
        ExprKind::Ident(name) => {
            let upper = name.to_ascii_uppercase();
            if !inside_adc_code_arg
                && analog_in_tags.contains(&upper)
                && !shadowed_locals.contains(&upper)
            {
                let label = name.clone();
                expr.kind = ExprKind::Call {
                    callee: "ADC_CODE".to_owned(),
                    args: vec![CallArg {
                        name: None,
                        value: Expr::new(ExprKind::Ident(label), span),
                        span,
                    }],
                };
            }
        }
        ExprKind::Index { base, index } => {
            lift_analog_reads_in_expr(base, analog_in_tags, shadowed_locals, false);
            lift_analog_reads_in_expr(index, analog_in_tags, shadowed_locals, false);
        }
        ExprKind::Field { base, .. } => {
            lift_analog_reads_in_expr(base, analog_in_tags, shadowed_locals, false);
        }
        ExprKind::Call { callee, args } => {
            let is_adc = callee.eq_ignore_ascii_case("ADC_CODE");
            for arg in args {
                lift_analog_reads_in_expr(&mut arg.value, analog_in_tags, shadowed_locals, is_adc);
            }
        }
        ExprKind::Unary { operand, .. } => {
            lift_analog_reads_in_expr(operand, analog_in_tags, shadowed_locals, false);
        }
        ExprKind::Binary { left, right, .. } => {
            lift_analog_reads_in_expr(left, analog_in_tags, shadowed_locals, false);
            lift_analog_reads_in_expr(right, analog_in_tags, shadowed_locals, false);
        }
        ExprKind::Literal(_) | ExprKind::Foreign { .. } => {}
    }
}

// ---------------------------------------------------------------------------
// Deterministic ADR-027 Comment Attachment (`C1–C5`)
// ---------------------------------------------------------------------------

fn attach_comments(
    raw_comments: &[RawComment],
    anchors: &[AnchorSite],
    tokens: &[Token],
    board_id: &str,
) -> Vec<Comment> {
    let mut sorted_anchors: Vec<&AnchorSite> = anchors.iter().collect();
    sorted_anchors.sort_by_key(|a| (a.span.start, std::cmp::Reverse(a.span.len())));

    let first_code_span = tokens
        .iter()
        .find(|t| !matches!(t.kind, TokenKind::Eof))
        .map(|t| t.span);

    let mut result = Vec::with_capacity(raw_comments.len());
    for (idx, raw) in raw_comments.iter().enumerate() {
        let id = format!("c_{:04}", idx.saturating_add(1));
        let (anchor, placement, token_ordinal) =
            classify_comment(raw, &sorted_anchors, tokens, first_code_span, board_id);
        result.push(Comment {
            id,
            anchor,
            placement,
            token_ordinal,
            text: raw.text.clone(),
            origin: CommentOrigin {
                language: "st".to_owned(),
                style: raw.style,
            },
            span: raw.span,
        });
    }
    result
}

fn classify_comment(
    raw: &RawComment,
    anchors: &[&AnchorSite],
    tokens: &[Token],
    first_code_span: Option<Span>,
    board_id: &str,
) -> (CommentAnchor, AstCommentPlacement, Option<u32>) {
    // 1. Trailing or Inner comment (preceded by executable code on the same line)
    if raw.preceded_by_code_on_line {
        let has_code_after_on_line = tokens.iter().any(|t| {
            !matches!(t.kind, TokenKind::Eof)
                && t.start_line == raw.start_line
                && t.span.start >= raw.span.end
        });

        if has_code_after_on_line {
            // Inner comment: anchor to innermost enclosing node with exact `token_ordinal`
            if let Some(enclosing) = innermost_enclosing(anchors, raw.span, false) {
                let ord = tokens
                    .get(enclosing.first_tok_idx..=enclosing.last_tok_idx)
                    .map_or(0, |slice| {
                        slice
                            .iter()
                            .filter(|t| t.span.end <= raw.span.start)
                            .count()
                    });
                let ord_u32 = u32::try_from(ord).unwrap_or(u32::MAX);
                return (
                    CommentAnchor::Node {
                        id: enclosing.id.clone(),
                    },
                    AstCommentPlacement::Inner,
                    Some(ord_u32),
                );
            }
            return (
                CommentAnchor::Board {
                    id: board_id.to_owned(),
                },
                AstCommentPlacement::Header,
                None,
            );
        }

        // Trailing comment at end of line: prefer leaf node ending on this line
        if let Some(ended) = anchors
            .iter()
            .filter(|a| a.end_line == raw.start_line && a.span.end <= raw.span.start)
            .min_by_key(|a| a.span.len())
        {
            return (
                CommentAnchor::Node {
                    id: ended.id.clone(),
                },
                AstCommentPlacement::Trailing,
                None,
            );
        }
        // Or container header line (e.g. `PROGRAM OB_MAIN // trailing`)
        if let Some(started) = anchors
            .iter()
            .filter(|a| a.start_line == raw.start_line && a.span.start <= raw.span.start)
            .min_by_key(|a| a.span.len())
        {
            return (
                CommentAnchor::Node {
                    id: started.id.clone(),
                },
                AstCommentPlacement::Trailing,
                None,
            );
        }
        return (
            CommentAnchor::Board {
                id: board_id.to_owned(),
            },
            AstCommentPlacement::Body,
            None,
        );
    }

    // 2. Leading header comment before all code (separated by blank line from first AST node, or no nodes)
    if let Some(first_anchor) = anchors.first() {
        let before_all_code = first_code_span.is_none_or(|fc| raw.span.end <= fc.start);
        if before_all_code && first_anchor.start_line > raw.end_line.saturating_add(1) {
            return (
                CommentAnchor::Board {
                    id: board_id.to_owned(),
                },
                AstCommentPlacement::Header,
                None,
            );
        }
    } else {
        return (
            CommentAnchor::Board {
                id: board_id.to_owned(),
            },
            AstCommentPlacement::Header,
            None,
        );
    }

    // 3. Leading comment on next sibling AST node inside the same enclosing container
    let enclosing_container = innermost_enclosing(anchors, raw.span, true);
    let next_sibling = anchors
        .iter()
        .filter(|a| {
            a.span.start >= raw.span.end
                && match enclosing_container {
                    Some(enc) => a.span.end <= enc.span.end && a.id != enc.id,
                    None => true,
                }
        })
        .min_by_key(|a| (a.span.start, std::cmp::Reverse(a.span.len())));

    if let Some(next_node) = next_sibling {
        return (
            CommentAnchor::Node {
                id: next_node.id.clone(),
            },
            AstCommentPlacement::Leading,
            None,
        );
    }

    // 4. Dangling comment inside an enclosing container (e.g., before `END_PROGRAM` / `END_IF`)
    if let Some(enc) = enclosing_container {
        return (
            CommentAnchor::Node { id: enc.id.clone() },
            AstCommentPlacement::Dangling,
            None,
        );
    }

    // 5. Trailing EOF comment after all top-level nodes -> Board Body (`C5`-compliant)
    (
        CommentAnchor::Board {
            id: board_id.to_owned(),
        },
        AstCommentPlacement::Body,
        None,
    )
}

fn innermost_enclosing<'a>(
    anchors: &[&'a AnchorSite],
    span: Span,
    containers_only: bool,
) -> Option<&'a AnchorSite> {
    anchors
        .iter()
        .copied()
        .filter(|a| {
            (!containers_only || a.is_container)
                && a.span.start <= span.start
                && span.end <= a.span.end
        })
        .min_by_key(|a| a.span.len())
}

fn rewrite_repeat_continues<F>(
    stmts: &mut Vec<Stmt>,
    until_cond: &Expr,
    until_span: Span,
    alloc_id: &mut F,
) where
    F: FnMut() -> NodeId,
{
    let old = std::mem::take(stmts);
    let mut out = Vec::with_capacity(old.len());
    for mut stmt in old {
        match &mut stmt.kind {
            StmtKind::Continue => {
                let break_id = alloc_id();
                let if_id = alloc_id();
                out.push(Stmt::new(
                    if_id,
                    StmtKind::If {
                        condition: until_cond.clone(),
                        then_body: vec![Stmt::new(break_id, StmtKind::Break, until_span)],
                        elif_arms: Vec::new(),
                        else_body: Vec::new(),
                    },
                    until_span,
                ));
                out.push(stmt);
            }
            StmtKind::If {
                then_body,
                elif_arms,
                else_body,
                ..
            } => {
                rewrite_repeat_continues(then_body, until_cond, until_span, alloc_id);
                for arm in elif_arms {
                    rewrite_repeat_continues(&mut arm.body, until_cond, until_span, alloc_id);
                }
                rewrite_repeat_continues(else_body, until_cond, until_span, alloc_id);
                out.push(stmt);
            }
            StmtKind::Switch { cases, default, .. } => {
                for c in cases {
                    rewrite_repeat_continues(&mut c.body, until_cond, until_span, alloc_id);
                }
                if let Some(def) = default {
                    rewrite_repeat_continues(def, until_cond, until_span, alloc_id);
                }
                out.push(stmt);
            }
            // Do NOT descend into nested loops: their `CONTINUE` targets the inner loop!
            StmtKind::For { .. }
            | StmtKind::While { .. }
            | StmtKind::Assign { .. }
            | StmtKind::Call { .. }
            | StmtKind::FbInvoke { .. }
            | StmtKind::Break
            | StmtKind::Return { .. }
            | StmtKind::Wait { .. }
            | StmtKind::Foreign { .. } => {
                out.push(stmt);
            }
        }
    }
    *stmts = out;
}

fn lower_all_function_returns<F>(program: &mut BoardProgram, alloc_id: &mut F)
where
    F: FnMut() -> NodeId,
{
    let mut global_names = BTreeSet::new();
    for hw in &program.hardware {
        global_names.insert(hw.label.to_ascii_uppercase());
    }
    for d in &program.data {
        global_names.insert(d.name.to_ascii_uppercase());
    }
    for td in &program.types {
        match td {
            TypeDecl::Struct { name, .. } => {
                global_names.insert(name.to_ascii_uppercase());
            }
            TypeDecl::Enum { name, variants, .. } => {
                global_names.insert(name.to_ascii_uppercase());
                for v in variants {
                    global_names.insert(v.name.to_ascii_uppercase());
                }
            }
            TypeDecl::Foreign { name, .. } => {
                if let Some(n) = name {
                    global_names.insert(n.to_ascii_uppercase());
                }
            }
        }
    }
    for pou in &program.pous {
        global_names.insert(pou.name.to_ascii_uppercase());
    }
    for m in &program.modules {
        for d in &m.data {
            global_names.insert(d.name.to_ascii_uppercase());
        }
        for pou in &m.pous {
            global_names.insert(pou.name.to_ascii_uppercase());
        }
    }

    for pou in &mut program.pous {
        lower_pou_function_returns(pou, &global_names, alloc_id);
    }
    for m in &mut program.modules {
        for pou in &mut m.pous {
            lower_pou_function_returns(pou, &global_names, alloc_id);
        }
    }
}

#[derive(Default)]
struct FnReturnUsage {
    any_fn_assign: bool,
    has_non_tail_fn_assign: bool,
    has_fn_read_or_partial_write: bool,
    has_bare_return: bool,
}

fn lower_pou_function_returns<F>(pou: &mut Pou, global_names: &BTreeSet<String>, alloc_id: &mut F)
where
    F: FnMut() -> NodeId,
{
    let return_type = match &pou.kind {
        PouKind::Fc { return_type } => return_type.clone(),
        PouKind::Ob { .. } | PouKind::Fb => return,
    };
    let PouBody::Statements(stmts) = &mut pou.body else {
        return;
    };

    let fn_name = pou.name.clone();
    let mut usage = FnReturnUsage::default();
    scan_fn_return_usage(stmts, &fn_name, true, &mut usage);

    if !usage.any_fn_assign && !usage.has_fn_read_or_partial_write {
        return;
    }

    if usage.any_fn_assign
        && !usage.has_non_tail_fn_assign
        && !usage.has_fn_read_or_partial_write
        && !usage.has_bare_return
    {
        // Fast pure-tail path: every `MyFunc := expr;` is strictly in tail position -> lower directly to `StmtKind::Return`.
        rewrite_tail_fn_assigns_to_return(stmts, &fn_name);
        return;
    }

    // Non-tail assignment, read-back, or bare `RETURN;` alongside `MyFunc := expr;`:
    // allocate a collision-free local `VAR_TEMP` return slot.
    let mut used = global_names.clone();
    used.insert(fn_name.to_ascii_uppercase());
    for p in &pou.params {
        used.insert(p.name.to_ascii_uppercase());
    }
    for l in &pou.locals {
        used.insert(l.name.to_ascii_uppercase());
    }
    let slot_name = pick_fresh_name("_ret", &used);

    pou.locals.push(DataDecl {
        id: alloc_id(),
        name: slot_name.clone(),
        scope: StorageScope::Temp,
        ty: return_type,
        initial: None,
        span: pou.span,
    });

    rewrite_fn_slot_in_stmts(stmts, &fn_name, &slot_name);

    let ends_with_return = matches!(stmts.last().map(|s| &s.kind), Some(StmtKind::Return { .. }));
    if !ends_with_return {
        stmts.push(Stmt::new(
            alloc_id(),
            StmtKind::Return {
                value: Some(Expr::new(ExprKind::Ident(slot_name), pou.span)),
            },
            pou.span,
        ));
    }
}

fn pick_fresh_name(base: &str, used_upper: &BTreeSet<String>) -> String {
    if !used_upper.contains(&base.to_ascii_uppercase()) {
        return base.to_owned();
    }
    let mut suffix = 1u32;
    loop {
        let candidate = format!("{base}_{suffix}");
        if !used_upper.contains(&candidate.to_ascii_uppercase()) {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
}

fn scan_fn_return_usage(
    stmts: &[Stmt],
    fn_name: &str,
    list_is_tail: bool,
    usage: &mut FnReturnUsage,
) {
    let last_idx = stmts.len().saturating_sub(1);
    for (idx, stmt) in stmts.iter().enumerate() {
        let is_tail = list_is_tail && idx == last_idx;
        match &stmt.kind {
            StmtKind::Assign { target, value } => {
                scan_expr_for_fn_read(value, fn_name, usage);
                match target {
                    LValue::Var { name, .. } if name.eq_ignore_ascii_case(fn_name) => {
                        usage.any_fn_assign = true;
                        if !is_tail {
                            usage.has_non_tail_fn_assign = true;
                        }
                    }
                    LValue::Var { .. } => {}
                    LValue::Index { .. } | LValue::Field { .. } => {
                        if target.root_name().eq_ignore_ascii_case(fn_name) {
                            usage.has_fn_read_or_partial_write = true;
                        }
                        scan_lvalue_indices_for_fn_read(target, fn_name, usage);
                    }
                }
            }
            StmtKind::Call { args, .. } => {
                for arg in args {
                    scan_expr_for_fn_read(&arg.value, fn_name, usage);
                }
            }
            StmtKind::FbInvoke { instance, inputs } => {
                scan_lvalue_indices_for_fn_read(instance, fn_name, usage);
                for (_, expr) in inputs {
                    scan_expr_for_fn_read(expr, fn_name, usage);
                }
            }
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => {
                scan_expr_for_fn_read(condition, fn_name, usage);
                scan_fn_return_usage(then_body, fn_name, is_tail, usage);
                for arm in elif_arms {
                    scan_expr_for_fn_read(&arm.condition, fn_name, usage);
                    scan_fn_return_usage(&arm.body, fn_name, is_tail, usage);
                }
                scan_fn_return_usage(else_body, fn_name, is_tail, usage);
            }
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => {
                scan_expr_for_fn_read(scrutinee, fn_name, usage);
                for c in cases {
                    for pat in &c.patterns {
                        match pat {
                            CasePattern::Value(e) => scan_expr_for_fn_read(e, fn_name, usage),
                            CasePattern::Range { start, end } => {
                                scan_expr_for_fn_read(start, fn_name, usage);
                                scan_expr_for_fn_read(end, fn_name, usage);
                            }
                        }
                    }
                    scan_fn_return_usage(&c.body, fn_name, is_tail, usage);
                }
                if let Some(def) = default {
                    scan_fn_return_usage(def, fn_name, is_tail, usage);
                }
            }
            StmtKind::While { condition, body } => {
                scan_expr_for_fn_read(condition, fn_name, usage);
                // Inside loops is NEVER tail position
                scan_fn_return_usage(body, fn_name, false, usage);
            }
            StmtKind::For {
                from,
                to,
                step,
                body,
                ..
            } => {
                scan_expr_for_fn_read(from, fn_name, usage);
                scan_expr_for_fn_read(to, fn_name, usage);
                scan_expr_for_fn_read(step, fn_name, usage);
                scan_fn_return_usage(body, fn_name, false, usage);
            }
            StmtKind::Return { value } => match value {
                Some(e) => scan_expr_for_fn_read(e, fn_name, usage),
                None => usage.has_bare_return = true,
            },
            StmtKind::Wait { duration } => {
                scan_expr_for_fn_read(duration, fn_name, usage);
            }
            StmtKind::Break | StmtKind::Continue | StmtKind::Foreign { .. } => {}
        }
    }
}

fn scan_lvalue_indices_for_fn_read(lv: &LValue, fn_name: &str, usage: &mut FnReturnUsage) {
    match lv {
        LValue::Var { .. } => {}
        LValue::Index { base, index, .. } => {
            scan_lvalue_indices_for_fn_read(base, fn_name, usage);
            scan_expr_for_fn_read(index, fn_name, usage);
        }
        LValue::Field { base, .. } => {
            scan_lvalue_indices_for_fn_read(base, fn_name, usage);
        }
    }
}

fn scan_expr_for_fn_read(expr: &Expr, fn_name: &str, usage: &mut FnReturnUsage) {
    match &expr.kind {
        ExprKind::Ident(name) => {
            if name.eq_ignore_ascii_case(fn_name) {
                usage.has_fn_read_or_partial_write = true;
            }
        }
        ExprKind::Index { base, index } => {
            scan_expr_for_fn_read(base, fn_name, usage);
            scan_expr_for_fn_read(index, fn_name, usage);
        }
        ExprKind::Field { base, .. } => {
            scan_expr_for_fn_read(base, fn_name, usage);
        }
        ExprKind::Call { args, .. } => {
            for arg in args {
                scan_expr_for_fn_read(&arg.value, fn_name, usage);
            }
        }
        ExprKind::Unary { operand, .. } => {
            scan_expr_for_fn_read(operand, fn_name, usage);
        }
        ExprKind::Binary { left, right, .. } => {
            scan_expr_for_fn_read(left, fn_name, usage);
            scan_expr_for_fn_read(right, fn_name, usage);
        }
        ExprKind::Literal(_) | ExprKind::Foreign { .. } => {}
    }
}

fn rewrite_tail_fn_assigns_to_return(stmts: &mut [Stmt], fn_name: &str) {
    let Some(last) = stmts.last_mut() else {
        return;
    };
    let is_fn_assign = matches!(
        &last.kind,
        StmtKind::Assign {
            target: LValue::Var { name, .. },
            ..
        } if name.eq_ignore_ascii_case(fn_name)
    );
    if is_fn_assign {
        let prev = std::mem::replace(&mut last.kind, StmtKind::Break);
        if let StmtKind::Assign { value, .. } = prev {
            last.kind = StmtKind::Return { value: Some(value) };
        }
        return;
    }
    match &mut last.kind {
        StmtKind::If {
            then_body,
            elif_arms,
            else_body,
            ..
        } => {
            rewrite_tail_fn_assigns_to_return(then_body, fn_name);
            for arm in elif_arms {
                rewrite_tail_fn_assigns_to_return(&mut arm.body, fn_name);
            }
            rewrite_tail_fn_assigns_to_return(else_body, fn_name);
        }
        StmtKind::Switch { cases, default, .. } => {
            for c in cases {
                rewrite_tail_fn_assigns_to_return(&mut c.body, fn_name);
            }
            if let Some(def) = default {
                rewrite_tail_fn_assigns_to_return(def, fn_name);
            }
        }
        StmtKind::Assign { .. }
        | StmtKind::Call { .. }
        | StmtKind::FbInvoke { .. }
        | StmtKind::While { .. }
        | StmtKind::For { .. }
        | StmtKind::Break
        | StmtKind::Continue
        | StmtKind::Return { .. }
        | StmtKind::Wait { .. }
        | StmtKind::Foreign { .. } => {}
    }
}

fn rewrite_fn_slot_in_stmts(stmts: &mut [Stmt], fn_name: &str, slot_name: &str) {
    for stmt in stmts {
        let span = stmt.span;
        match &mut stmt.kind {
            StmtKind::Assign { target, value } => {
                rewrite_fn_slot_in_lvalue(target, fn_name, slot_name);
                rewrite_fn_slot_in_expr(value, fn_name, slot_name);
            }
            StmtKind::Call { args, .. } => {
                for arg in args {
                    rewrite_fn_slot_in_expr(&mut arg.value, fn_name, slot_name);
                }
            }
            StmtKind::FbInvoke { instance, inputs } => {
                rewrite_fn_slot_in_lvalue(instance, fn_name, slot_name);
                for (_, expr) in inputs {
                    rewrite_fn_slot_in_expr(expr, fn_name, slot_name);
                }
            }
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            } => {
                rewrite_fn_slot_in_expr(condition, fn_name, slot_name);
                rewrite_fn_slot_in_stmts(then_body, fn_name, slot_name);
                for arm in elif_arms {
                    rewrite_fn_slot_in_expr(&mut arm.condition, fn_name, slot_name);
                    rewrite_fn_slot_in_stmts(&mut arm.body, fn_name, slot_name);
                }
                rewrite_fn_slot_in_stmts(else_body, fn_name, slot_name);
            }
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            } => {
                rewrite_fn_slot_in_expr(scrutinee, fn_name, slot_name);
                for c in cases {
                    for pat in &mut c.patterns {
                        match pat {
                            CasePattern::Value(e) => {
                                rewrite_fn_slot_in_expr(e, fn_name, slot_name);
                            }
                            CasePattern::Range { start, end } => {
                                rewrite_fn_slot_in_expr(start, fn_name, slot_name);
                                rewrite_fn_slot_in_expr(end, fn_name, slot_name);
                            }
                        }
                    }
                    rewrite_fn_slot_in_stmts(&mut c.body, fn_name, slot_name);
                }
                if let Some(def) = default {
                    rewrite_fn_slot_in_stmts(def, fn_name, slot_name);
                }
            }
            StmtKind::While { condition, body } => {
                rewrite_fn_slot_in_expr(condition, fn_name, slot_name);
                rewrite_fn_slot_in_stmts(body, fn_name, slot_name);
            }
            StmtKind::For {
                from,
                to,
                step,
                body,
                ..
            } => {
                rewrite_fn_slot_in_expr(from, fn_name, slot_name);
                rewrite_fn_slot_in_expr(to, fn_name, slot_name);
                rewrite_fn_slot_in_expr(step, fn_name, slot_name);
                rewrite_fn_slot_in_stmts(body, fn_name, slot_name);
            }
            StmtKind::Return { value } => match value {
                Some(e) => rewrite_fn_slot_in_expr(e, fn_name, slot_name),
                None => {
                    *value = Some(Expr::new(ExprKind::Ident(slot_name.to_owned()), span));
                }
            },
            StmtKind::Wait { duration } => {
                rewrite_fn_slot_in_expr(duration, fn_name, slot_name);
            }
            StmtKind::Break | StmtKind::Continue | StmtKind::Foreign { .. } => {}
        }
    }
}

fn rewrite_fn_slot_in_lvalue(lv: &mut LValue, fn_name: &str, slot_name: &str) {
    match lv {
        LValue::Var { name, .. } => {
            if name.eq_ignore_ascii_case(fn_name) {
                *name = slot_name.to_owned();
            }
        }
        LValue::Index { base, index, .. } => {
            rewrite_fn_slot_in_lvalue(base, fn_name, slot_name);
            rewrite_fn_slot_in_expr(index, fn_name, slot_name);
        }
        LValue::Field { base, .. } => {
            rewrite_fn_slot_in_lvalue(base, fn_name, slot_name);
        }
    }
}

fn rewrite_fn_slot_in_expr(expr: &mut Expr, fn_name: &str, slot_name: &str) {
    match &mut expr.kind {
        ExprKind::Ident(name) => {
            if name.eq_ignore_ascii_case(fn_name) {
                *name = slot_name.to_owned();
            }
        }
        ExprKind::Index { base, index } => {
            rewrite_fn_slot_in_expr(base, fn_name, slot_name);
            rewrite_fn_slot_in_expr(index, fn_name, slot_name);
        }
        ExprKind::Field { base, .. } => {
            rewrite_fn_slot_in_expr(base, fn_name, slot_name);
        }
        ExprKind::Call { args, .. } => {
            for arg in args {
                rewrite_fn_slot_in_expr(&mut arg.value, fn_name, slot_name);
            }
        }
        ExprKind::Unary { operand, .. } => {
            rewrite_fn_slot_in_expr(operand, fn_name, slot_name);
        }
        ExprKind::Binary { left, right, .. } => {
            rewrite_fn_slot_in_expr(left, fn_name, slot_name);
            rewrite_fn_slot_in_expr(right, fn_name, slot_name);
        }
        ExprKind::Literal(_) | ExprKind::Foreign { .. } => {}
    }
}

static EOF_FALLBACK: Token = Token {
    kind: TokenKind::Eof,
    span: Span { start: 0, end: 0 },
    start_line: 1,
    end_line: 1,
    blank_lines_before: 0,
};
