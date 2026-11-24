//! Pure-Rust recursive-descent and Pratt C++ subset parser (`docs/lbc/cpp.md` §1).
//!
//! Uses `dendri_frontend_core::pratt::{PrattDriver, parse_pratt}` for expressions (enforcing
//! non-chaining relational/equality operators and bounded recursion depth) and preserves
//! unmodeled C++ constructs verbatim across all three `Foreign` levels (`Decl`, `Stmt`, `Expr`).

use crate::board_pins::{is_avr_register, is_vendor_intrinsic};
use crate::cpp_ast::{
    CompoundOp, CppEnumVariant, CppExpr, CppExprKind, CppFieldDecl, CppForInit, CppForStep,
    CppParam, CppStmt, CppStmtKind, CppSwitchCase, CppTopItem, CppTypeSpec, CppUnit,
};
use crate::lexer::{Token, TokenKind, lex_cpp};
use crate::types::{builtin_fb_kind, is_cpp_primitive_type_keyword};
use dendri_ast::{BinaryOp, ForeignFidelity, UnaryOp};
use dendri_frontend_core::pratt::{Associativity, InfixSpec, PrattDriver, PrefixSpec, parse_pratt};
use dendri_span::{Diagnostic, ErrorCode, Span};
use std::collections::BTreeSet;

const MAX_NESTING_DEPTH: usize = 128;

/// Parses a lexed token stream into a [`CppUnit`].
pub(crate) fn parse_cpp_tokens(
    src: &str,
    tokens: &[Token],
    board_id: &str,
) -> Result<CppUnit, Vec<Diagnostic>> {
    let mut parser = Parser {
        src,
        tokens,
        pos: 0,
        board_id,
        known_type_names: BTreeSet::new(),
        known_foreign_classes: BTreeSet::from(["Servo".to_owned(), "String".to_owned()]),
        known_foreign_objects: BTreeSet::from(["Wire".to_owned(), "SPI".to_owned()]),
        expr_depth: 0,
        stmt_depth: 0,
        errors: Vec::new(),
    };
    let items = parser.parse_translation_unit();
    if parser.errors.is_empty() {
        Ok(CppUnit { items })
    } else {
        Err(parser.errors)
    }
}

struct Parser<'a> {
    src: &'a str,
    tokens: &'a [Token],
    pos: usize,
    board_id: &'a str,
    known_type_names: BTreeSet<String>,
    known_foreign_classes: BTreeSet<String>,
    known_foreign_objects: BTreeSet<String>,
    expr_depth: usize,
    stmt_depth: usize,
    errors: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> &'a Token {
        self.tokens
            .get(self.pos)
            .or_else(|| self.tokens.last())
            .unwrap_or(&EOF_TOKEN)
    }

    fn peek_at(&self, offset: usize) -> &'a Token {
        self.tokens
            .get(self.pos.saturating_add(offset))
            .or_else(|| self.tokens.last())
            .unwrap_or(&EOF_TOKEN)
    }

    fn is_eof(&self) -> bool {
        matches!(self.peek().kind, TokenKind::Eof)
    }

    fn bump(&mut self) -> &'a Token {
        let tok = self.peek();
        if self.pos < self.tokens.len().saturating_sub(1) {
            self.pos = self.pos.saturating_add(1);
        }
        tok
    }

    fn check_ident(&self, kw: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Ident(s) if s == kw)
    }

    fn eat_ident(&mut self, kw: &str) -> Option<Span> {
        if self.check_ident(kw) {
            Some(self.bump().span)
        } else {
            None
        }
    }

    fn slice_span(&self, span: Span) -> &'a str {
        span.slice(self.src).unwrap_or("")
    }

    fn push_syntax_err(&mut self, msg: impl Into<String>, span: Span) {
        let mut diag = Diagnostic::new(ErrorCode::ErrTomlSyntax, msg).with_board_id(self.board_id);
        diag.span = Some(span);
        self.errors.push(diag);
    }

    fn expect_ident(&mut self, context: &str) -> Option<(String, Span)> {
        let tok = self.peek();
        if let TokenKind::Ident(name) = &tok.kind {
            let s = name.clone();
            let sp = tok.span;
            let _ = self.bump();
            Some((s, sp))
        } else {
            self.push_syntax_err(format!("expected identifier in {context}"), tok.span);
            None
        }
    }

    fn expect_semi(&mut self, context: &str) -> Option<Span> {
        if matches!(self.peek().kind, TokenKind::Semi) {
            Some(self.bump().span)
        } else {
            self.push_syntax_err(format!("expected `;` after {context}"), self.peek().span);
            None
        }
    }

    fn expect_lparen(&mut self, context: &str) -> Option<Span> {
        if matches!(self.peek().kind, TokenKind::LParen) {
            Some(self.bump().span)
        } else {
            self.push_syntax_err(format!("expected `(` in {context}"), self.peek().span);
            None
        }
    }

    fn expect_rparen(&mut self, context: &str) -> Option<Span> {
        if matches!(self.peek().kind, TokenKind::RParen) {
            Some(self.bump().span)
        } else {
            self.push_syntax_err(format!("expected `)` in {context}"), self.peek().span);
            None
        }
    }

    fn expect_lbrace(&mut self, context: &str) -> Option<Span> {
        if matches!(self.peek().kind, TokenKind::LBrace) {
            Some(self.bump().span)
        } else {
            self.push_syntax_err(format!("expected `{{` in {context}"), self.peek().span);
            None
        }
    }

    fn expect_rbrace(&mut self, context: &str) -> Option<Span> {
        if matches!(self.peek().kind, TokenKind::RBrace) {
            Some(self.bump().span)
        } else {
            self.push_syntax_err(format!("expected `}}` in {context}"), self.peek().span);
            None
        }
    }

    fn parse_translation_unit(&mut self) -> Vec<CppTopItem> {
        let mut items = Vec::new();
        while !self.is_eof() {
            let before = self.pos;
            if let Some(item) = self.parse_top_item() {
                items.push(item);
            }
            if self.pos == before && !self.is_eof() {
                // Guarantee forward progress on malformed input
                let _ = self.bump();
            }
        }
        items
    }

    fn parse_top_item(&mut self) -> Option<CppTopItem> {
        // 1. Preprocessor directive
        if let TokenKind::Preproc { raw } = &self.peek().kind {
            let raw = raw.clone();
            let span = self.bump().span;
            return Some(self.parse_preproc_top_item(&raw, span));
        }

        // 2. Skip stray top-level semicolons
        if matches!(self.peek().kind, TokenKind::Semi) {
            let _ = self.bump();
            return None;
        }

        // 3. `class` / `template` / `namespace` / `using` / `typedef` / `extern` -> Decl::Foreign(F0)
        if self.check_ident("class")
            || self.check_ident("template")
            || self.check_ident("namespace")
            || self.check_ident("using")
            || self.check_ident("typedef")
            || self.check_ident("extern")
        {
            return Some(self.parse_foreign_top_decl());
        }

        // 4. `struct` (flat struct vs struct with methods/inheritance -> Foreign)
        if self.check_ident("struct") {
            return Some(self.parse_struct_decl());
        }

        // 5. `enum` / `enum class`
        if self.check_ident("enum") {
            return self.parse_enum_decl();
        }

        // 6. Global variable, foreign object declaration, function prototype, or function definition
        self.parse_var_or_function_decl()
    }

    fn parse_preproc_top_item(&mut self, raw: &str, span: Span) -> CppTopItem {
        let trimmed = raw.trim();
        let after_hash = trimmed.strip_prefix('#').unwrap_or("").trim_start();

        // `#include <Header.h>` or `#include "local.h"`
        if let Some(rest) = after_hash.strip_prefix("include")
            && (rest.is_empty() || rest.starts_with([' ', '\t', '<', '"']))
        {
            let target = rest.trim();
            if let Some(inner) = target.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
                return CppTopItem::Include {
                    header: inner.trim().to_owned(),
                    is_angle: true,
                    raw: trimmed.to_owned(),
                    span,
                };
            }
            if let Some(inner) = target.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                return CppTopItem::Include {
                    header: inner.trim().to_owned(),
                    is_angle: false,
                    raw: trimmed.to_owned(),
                    span,
                };
            }
        }

        // `#define NAME EXPR` (object-like constant only; function-like `NAME(...)` or `##`/`#` -> Foreign)
        if let Some(rest) = after_hash.strip_prefix("define")
            && rest.starts_with([' ', '\t'])
        {
            let rest = rest.trim_start();
            let name_len = rest
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(rest.len());
            let name = rest.get(..name_len).unwrap_or("");
            let tail = rest.get(name_len..).unwrap_or("");
            // Function-like macro `NAME(...)` (no space between name and `(`) or token pasting -> Foreign!
            let is_func_like = tail.starts_with('(');
            let value_str = tail.trim();
            let has_hash_ops =
                value_str.contains("##") || value_str.contains('#') || value_str.contains('\\');
            if !name.is_empty()
                && !is_func_like
                && !value_str.is_empty()
                && !has_hash_ops
                && let Some(expr) = self.try_parse_inline_expr(value_str, span)
            {
                return CppTopItem::DefineConst {
                    name: name.to_owned(),
                    expr,
                    span,
                };
            }
        }

        CppTopItem::ForeignDecl {
            name: None,
            fidelity: ForeignFidelity::F0,
            text: trimmed.to_owned(),
            pack_hint: None,
            instance_binding: None,
            span,
        }
    }

    fn try_parse_inline_expr(&self, expr_src: &str, outer_span: Span) -> Option<CppExpr> {
        let lexed = lex_cpp(expr_src, self.board_id).ok()?;
        let mut sub = Parser {
            src: expr_src,
            tokens: &lexed.tokens,
            pos: 0,
            board_id: self.board_id,
            known_type_names: self.known_type_names.clone(),
            known_foreign_classes: self.known_foreign_classes.clone(),
            known_foreign_objects: self.known_foreign_objects.clone(),
            expr_depth: 0,
            stmt_depth: 0,
            errors: Vec::new(),
        };
        let mut expr = sub.parse_expr()?;
        if !sub.is_eof() || !sub.errors.is_empty() {
            return None;
        }
        expr.span = outer_span;
        Some(expr)
    }

    fn parse_foreign_top_decl(&mut self) -> CppTopItem {
        let start_pos = self.pos;
        let start_span = self.peek().span;
        let first_kw = if let TokenKind::Ident(s) = &self.peek().kind {
            s.clone()
        } else {
            String::new()
        };

        // Extract class/struct/namespace name if available
        let mut decl_name: Option<String> = None;
        if (first_kw == "class" || first_kw == "struct" || first_kw == "namespace")
            && let TokenKind::Ident(n) = &self.peek_at(1).kind
        {
            decl_name = Some(n.clone());
            if first_kw == "class" || first_kw == "struct" {
                self.known_foreign_classes.insert(n.clone());
            }
        }

        // Consume tokens to the end of the declaration (`;` or balanced `{ ... }` + optional `;`)
        let end_span = self.skip_foreign_item_tokens(start_span);
        let full_span = start_span.merge(end_span);
        let text = self.slice_span(full_span).trim().to_owned();
        if self.pos == start_pos && !self.is_eof() {
            let _ = self.bump();
        }
        CppTopItem::ForeignDecl {
            name: decl_name,
            fidelity: ForeignFidelity::F0,
            text,
            pack_hint: None,
            instance_binding: None,
            span: full_span,
        }
    }

    fn skip_foreign_item_tokens(&mut self, fallback: Span) -> Span {
        let mut last_span = fallback;
        let mut brace_depth = 0usize;
        let mut saw_brace = false;
        while !self.is_eof() {
            let tok = self.bump();
            last_span = tok.span;
            if matches!(tok.kind, TokenKind::LBrace) {
                saw_brace = true;
                brace_depth = brace_depth.saturating_add(1);
            } else if matches!(tok.kind, TokenKind::RBrace) {
                brace_depth = brace_depth.saturating_sub(1);
                if saw_brace && brace_depth == 0 {
                    if matches!(self.peek().kind, TokenKind::Semi) {
                        last_span = self.bump().span;
                    }
                    break;
                }
            } else if matches!(tok.kind, TokenKind::Semi) && brace_depth == 0 {
                break;
            }
        }
        last_span
    }

    fn parse_struct_decl(&mut self) -> CppTopItem {
        let saved_pos = self.pos;
        let start_span = self.peek().span;

        // Speculatively check if struct is flat (no inheritance `:`, no methods `(`, no `public:`/`private:`/`protected:`/`~`)
        if self.struct_has_oop_features() {
            return self.parse_foreign_top_decl();
        }

        let _ = self.eat_ident("struct");
        let Some((name, _)) = self.expect_ident("struct declaration") else {
            self.pos = saved_pos;
            return self.parse_foreign_top_decl();
        };

        if self.expect_lbrace("struct body").is_none() {
            self.pos = saved_pos;
            return self.parse_foreign_top_decl();
        }

        let mut fields = Vec::new();
        while !self.is_eof() && !matches!(self.peek().kind, TokenKind::RBrace) {
            let field_start = self.peek().span;
            let Some(ty) = self.parse_type_spec() else {
                self.pos = saved_pos;
                self.errors.clear();
                return self.parse_foreign_top_decl();
            };
            let Some((fname, fname_span)) = self.expect_ident("struct field") else {
                self.pos = saved_pos;
                return self.parse_foreign_top_decl();
            };
            let array_len = if matches!(self.peek().kind, TokenKind::LBracket) {
                let _ = self.bump();
                let len_expr = self.parse_expr();
                if matches!(self.peek().kind, TokenKind::RBracket) {
                    let _ = self.bump();
                } else {
                    self.push_syntax_err("expected `]` in array field", self.peek().span);
                }
                len_expr
            } else {
                None
            };
            let semi_span = self.expect_semi("struct field").unwrap_or(fname_span);
            fields.push(CppFieldDecl {
                ty,
                name: fname,
                array_len,
                span: field_start.merge(semi_span),
            });
        }

        let rbrace_span = self.expect_rbrace("struct body").unwrap_or(start_span);
        let semi_span = self
            .expect_semi("struct declaration")
            .unwrap_or(rbrace_span);
        self.known_type_names.insert(name.clone());

        CppTopItem::Struct {
            name,
            fields,
            span: start_span.merge(semi_span),
        }
    }

    /// Scans ahead over `struct Name ... { ... };` to detect inheritance, methods, constructors,
    /// destructors, or access specifiers (which must lift to `Decl::Foreign(F0)` per `DEF-004`).
    fn struct_has_oop_features(&self) -> bool {
        let mut idx = self.pos.saturating_add(1); // after `struct`
        let mut brace_depth = 0usize;
        let mut saw_lbrace = false;
        while let Some(tok) = self.tokens.get(idx) {
            if matches!(tok.kind, TokenKind::Eof) {
                return false;
            }
            if matches!(tok.kind, TokenKind::Colon | TokenKind::Semi) && !saw_lbrace {
                return true;
            }
            if matches!(tok.kind, TokenKind::LBrace) {
                saw_lbrace = true;
                brace_depth = brace_depth.saturating_add(1);
            } else if matches!(tok.kind, TokenKind::RBrace) {
                brace_depth = brace_depth.saturating_sub(1);
                if saw_lbrace && brace_depth == 0 {
                    return false;
                }
            } else if saw_lbrace && matches!(tok.kind, TokenKind::LParen | TokenKind::Tilde) {
                return true;
            } else if saw_lbrace
                && let TokenKind::Ident(w) = &tok.kind
                && matches!(
                    w.as_str(),
                    "public" | "private" | "protected" | "virtual" | "static" | "operator"
                )
            {
                return true;
            }
            idx = idx.saturating_add(1);
        }
        false
    }

    fn parse_enum_decl(&mut self) -> Option<CppTopItem> {
        let start_span = self.eat_ident("enum")?;
        let _is_class = self.eat_ident("class").is_some();
        let (name, _) = self.expect_ident("enum declaration")?;
        let backing = if matches!(self.peek().kind, TokenKind::Colon) {
            let _ = self.bump();
            self.parse_type_spec()
        } else {
            None
        };
        self.expect_lbrace("enum body")?;
        let mut variants = Vec::new();
        while !self.is_eof() && !matches!(self.peek().kind, TokenKind::RBrace) {
            let (vname, vspan) = self.expect_ident("enum variant")?;
            let value = if matches!(self.peek().kind, TokenKind::Assign) {
                let _ = self.bump();
                self.parse_expr()
            } else {
                None
            };
            let end_sp = value.as_ref().map_or(vspan, |e| e.span);
            variants.push(CppEnumVariant {
                name: vname,
                value,
                span: vspan.merge(end_sp),
            });
            if matches!(self.peek().kind, TokenKind::Comma) {
                let _ = self.bump();
            } else {
                break;
            }
        }
        let rbrace = self.expect_rbrace("enum body").unwrap_or(start_span);
        let semi = self.expect_semi("enum declaration").unwrap_or(rbrace);
        self.known_type_names.insert(name.clone());
        Some(CppTopItem::Enum {
            name,
            backing,
            variants,
            span: start_span.merge(semi),
        })
    }

    fn parse_var_or_function_decl(&mut self) -> Option<CppTopItem> {
        let saved_pos = self.pos;
        let start_span = self.peek().span;

        let mut is_const = false;
        let mut is_constexpr = false;
        loop {
            if self.eat_ident("static").is_some()
                || self.eat_ident("inline").is_some()
                || self.eat_ident("volatile").is_some()
            {
                // accepted C++ storage/qualifier on declarations
            } else if self.eat_ident("constexpr").is_some() {
                is_constexpr = true;
                is_const = true;
            } else if self.eat_ident("const").is_some() {
                is_const = true;
            } else {
                break;
            }
        }

        let Some(mut ty) = self.parse_type_spec() else {
            self.push_syntax_err("expected top-level declaration", start_span);
            let _ = self.skip_foreign_item_tokens(start_span);
            return None;
        };
        if is_const {
            ty.is_const = true;
        }

        // Check for foreign class instance declaration (e.g. `Servo myServo;`, `MyClass obj;`, `String s = "hi";`)
        if !ty.is_pointer
            && !ty.is_ref
            && ty.words.len() == 1
            && let Some(class_name) = ty.words.first()
            && self.known_foreign_classes.contains(class_name)
        {
            let class_name = class_name.clone();
            let var_name = if let TokenKind::Ident(n) = &self.peek().kind {
                Some(n.clone())
            } else {
                None
            };
            // Only treat as foreign instance declaration if followed by `;` or `=` or `(` constructor call (not function `{`)
            let is_fn_def = self.looks_like_function_after_name();
            if !is_fn_def {
                let end_span = self.skip_foreign_item_tokens(start_span);
                let full_span = start_span.merge(end_span);
                let text = self.slice_span(full_span).trim().to_owned();
                if let Some(ref vn) = var_name {
                    self.known_foreign_objects.insert(vn.clone());
                }
                let pack_hint = if class_name == "Servo" {
                    Some("arduino-servo".to_owned())
                } else {
                    None
                };
                return Some(CppTopItem::ForeignDecl {
                    name: var_name.clone(),
                    fidelity: ForeignFidelity::F0,
                    text,
                    pack_hint,
                    instance_binding: var_name.map(|vn| (vn, class_name)),
                    span: full_span,
                });
            }
        }

        // Raw pointer declaration other than `const char*` -> Decl::Foreign(F0)
        if ty.is_pointer
            && !(ty.is_const
                && ty.words.len() == 1
                && ty.words.first().map(String::as_str) == Some("char"))
        {
            self.pos = saved_pos;
            return Some(self.parse_foreign_top_decl());
        }

        let Some((name, name_span)) = self.expect_ident("declaration name") else {
            let _ = self.skip_foreign_item_tokens(start_span);
            return None;
        };

        // Function definition or prototype: `<ty> <name> ( ... ) { ... }` or `;`
        if matches!(self.peek().kind, TokenKind::LParen) {
            let _ = self.bump();
            let mut params = Vec::new();
            if !matches!(self.peek().kind, TokenKind::RParen) {
                // Handle C-style `(void)` parameter list
                let is_lone_void =
                    self.check_ident("void") && matches!(self.peek_at(1).kind, TokenKind::RParen);
                if is_lone_void {
                    let _ = self.bump();
                } else {
                    loop {
                        let pstart = self.peek().span;
                        let Some(pty) = self.parse_type_spec() else {
                            break;
                        };
                        let Some((pname, pspan)) = self.expect_ident("function parameter") else {
                            break;
                        };
                        let array_len = if matches!(self.peek().kind, TokenKind::LBracket) {
                            let _ = self.bump();
                            let len_e = if !matches!(self.peek().kind, TokenKind::RBracket) {
                                self.parse_expr()
                            } else {
                                None
                            };
                            if matches!(self.peek().kind, TokenKind::RBracket) {
                                let _ = self.bump();
                            }
                            len_e
                        } else {
                            None
                        };
                        params.push(CppParam {
                            ty: pty,
                            name: pname,
                            array_len,
                            span: pstart.merge(pspan),
                        });
                        if matches!(self.peek().kind, TokenKind::Comma) {
                            let _ = self.bump();
                        } else {
                            break;
                        }
                    }
                }
            }
            let rparen = self
                .expect_rparen("function parameter list")
                .unwrap_or(name_span);

            // Function forward prototype `;`
            if matches!(self.peek().kind, TokenKind::Semi) {
                let _ = self.bump();
                return Some(CppTopItem::FunctionProto { name });
            }

            let (body, body_span) = self.parse_block_stmts("function body")?;
            return Some(CppTopItem::Function {
                return_ty: ty,
                name,
                params,
                body,
                body_span,
                span: start_span.merge(body_span.merge(rparen)),
            });
        }

        // Global variable declaration (scalar, array, or FB instance)
        let array_len = if matches!(self.peek().kind, TokenKind::LBracket) {
            let _ = self.bump();
            let len_expr = self.parse_expr();
            if matches!(self.peek().kind, TokenKind::RBracket) {
                let _ = self.bump();
            } else {
                self.push_syntax_err("expected `]` in global array declaration", self.peek().span);
            }
            len_expr
        } else {
            None
        };

        let initial = if matches!(self.peek().kind, TokenKind::Assign) {
            let _ = self.bump();
            self.parse_expr()
        } else {
            None
        };

        let semi = self
            .expect_semi("global variable declaration")
            .unwrap_or(name_span);
        Some(CppTopItem::GlobalVar {
            is_const,
            is_constexpr,
            ty,
            name,
            array_len,
            initial,
            span: start_span.merge(semi),
        })
    }

    fn looks_like_function_after_name(&self) -> bool {
        if !matches!(self.peek_at(1).kind, TokenKind::LParen) {
            return false;
        }
        let mut depth = 0usize;
        let mut i = self.pos.saturating_add(1);
        while let Some(t) = self.tokens.get(i) {
            if matches!(t.kind, TokenKind::LParen) {
                depth = depth.saturating_add(1);
            } else if matches!(t.kind, TokenKind::RParen) {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return matches!(
                        self.tokens.get(i.saturating_add(1)).map(|nt| &nt.kind),
                        Some(TokenKind::LBrace)
                    );
                }
            } else if matches!(t.kind, TokenKind::Eof) {
                return false;
            }
            i = i.saturating_add(1);
        }
        false
    }

    fn parse_type_spec(&mut self) -> Option<CppTypeSpec> {
        let start_span = self.peek().span;
        let mut is_const = false;
        while self.eat_ident("const").is_some()
            || self.eat_ident("constexpr").is_some()
            || self.eat_ident("volatile").is_some()
        {
            is_const = true;
        }
        // Optional `struct` / `enum` elaborated type specifier
        let _ = self.eat_ident("struct");
        let _ = self.eat_ident("enum");

        let mut words = Vec::new();
        let mut end_span = start_span;

        // Multi-word C++ primitive types: `unsigned long long int`, `signed short`, etc.
        while let TokenKind::Ident(w) = &self.peek().kind {
            let is_modifier = matches!(w.as_str(), "signed" | "unsigned" | "short" | "long");
            if words.is_empty() {
                words.push(w.clone());
                end_span = self.bump().span;
                if !is_modifier {
                    break;
                }
            } else if matches!(w.as_str(), "int" | "char" | "short" | "long" | "double") {
                words.push(w.clone());
                end_span = self.bump().span;
                if matches!(w.as_str(), "int" | "char" | "double") {
                    break;
                }
            } else {
                break;
            }
        }

        if words.is_empty() {
            return None;
        }

        // Trailing `const` (e.g. `char const*`)
        if self.eat_ident("const").is_some() {
            is_const = true;
        }

        let mut is_pointer = false;
        let mut is_ref = false;
        if matches!(self.peek().kind, TokenKind::Star) {
            is_pointer = true;
            end_span = self.bump().span;
        } else if matches!(self.peek().kind, TokenKind::Amp) {
            is_ref = true;
            end_span = self.bump().span;
        }

        Some(CppTypeSpec {
            is_const,
            is_pointer,
            is_ref,
            words,
            span: start_span.merge(end_span),
        })
    }

    fn parse_block_stmts(&mut self, context: &str) -> Option<(Vec<CppStmt>, Span)> {
        let lbrace = self.expect_lbrace(context)?;
        let mut stmts = Vec::new();
        while !self.is_eof() && !matches!(self.peek().kind, TokenKind::RBrace) {
            let before = self.pos;
            if let Some(stmt) = self.parse_stmt() {
                stmts.push(stmt);
            }
            if self.pos == before && !self.is_eof() {
                let _ = self.bump();
            }
        }
        let rbrace = self.expect_rbrace(context).unwrap_or(lbrace);
        Some((stmts, lbrace.merge(rbrace)))
    }

    fn parse_stmt(&mut self) -> Option<CppStmt> {
        if self.stmt_depth >= MAX_NESTING_DEPTH {
            let mut d = Diagnostic::new(
                ErrorCode::ErrRecursionDepth,
                format!("statement nesting exceeds maximum depth of {MAX_NESTING_DEPTH}"),
            )
            .with_board_id(self.board_id);
            d.span = Some(self.peek().span);
            self.errors.push(d);
            let _ = self.bump();
            return None;
        }
        self.stmt_depth = self.stmt_depth.saturating_add(1);
        let res = self.parse_stmt_inner();
        self.stmt_depth = self.stmt_depth.saturating_sub(1);
        res
    }

    fn parse_stmt_inner(&mut self) -> Option<CppStmt> {
        let start_span = self.peek().span;

        // 1. Preprocessor directive inside a block -> Stmt::Foreign(F0)
        if let TokenKind::Preproc { raw } = &self.peek().kind {
            let text = raw.trim().to_owned();
            let span = self.bump().span;
            return Some(CppStmt {
                kind: CppStmtKind::Foreign {
                    text,
                    pack_hint: None,
                    receiver_class: None,
                    method_name: None,
                    call_args: Vec::new(),
                },
                span,
            });
        }

        // 2. Empty statement `;`
        if matches!(self.peek().kind, TokenKind::Semi) {
            let _ = self.bump();
            return None;
        }

        // 3. Nested block `{ ... }`
        if matches!(self.peek().kind, TokenKind::LBrace) {
            let (stmts, span) = self.parse_block_stmts("block statement")?;
            return Some(CppStmt {
                kind: CppStmtKind::Block { stmts },
                span,
            });
        }

        // 4. Control flow keywords
        if self.check_ident("if") {
            return self.parse_if_stmt();
        }
        if self.check_ident("switch") {
            return self.parse_switch_stmt();
        }
        if self.check_ident("while") {
            return self.parse_while_stmt();
        }
        if self.check_ident("do") {
            return self.parse_do_while_stmt();
        }
        if self.check_ident("for") {
            return self.parse_for_stmt();
        }
        if self.eat_ident("break").is_some() {
            let semi = self.expect_semi("break").unwrap_or(start_span);
            return Some(CppStmt {
                kind: CppStmtKind::Break,
                span: start_span.merge(semi),
            });
        }
        if self.eat_ident("continue").is_some() {
            let semi = self.expect_semi("continue").unwrap_or(start_span);
            return Some(CppStmt {
                kind: CppStmtKind::Continue,
                span: start_span.merge(semi),
            });
        }
        if self.eat_ident("return").is_some() {
            let value = if !matches!(self.peek().kind, TokenKind::Semi) {
                self.parse_expr()
            } else {
                None
            };
            let semi = self.expect_semi("return").unwrap_or(start_span);
            return Some(CppStmt {
                kind: CppStmtKind::Return { value },
                span: start_span.merge(semi),
            });
        }

        // 5. Foreign keywords inside statements (`asm`, `__asm__`, `goto`, `delete`, `try`, `throw`)
        if self.check_ident("asm")
            || self.check_ident("__asm__")
            || self.check_ident("__asm")
            || self.check_ident("goto")
            || self.check_ident("delete")
            || self.check_ident("try")
            || self.check_ident("throw")
        {
            return Some(self.parse_foreign_stmt());
        }

        // 6. Direct AVR register statement (e.g. `PORTB |= (1 << 5);`) or raw pointer deref (`*p = 1;`)
        if let TokenKind::Ident(ident) = &self.peek().kind
            && is_avr_register(ident)
        {
            return Some(self.parse_foreign_stmt());
        }
        if matches!(self.peek().kind, TokenKind::Star) {
            return Some(self.parse_foreign_stmt());
        }

        // 7. Prefix `++x;` / `--x;`
        if matches!(
            self.peek().kind,
            TokenKind::PlusPlus | TokenKind::MinusMinus
        ) {
            let is_increment = matches!(self.bump().kind, TokenKind::PlusPlus);
            let target = self.parse_expr()?;
            let semi = self
                .expect_semi("prefix increment/decrement")
                .unwrap_or(target.span);
            return Some(CppStmt {
                kind: CppStmtKind::IncDec {
                    target,
                    is_increment,
                },
                span: start_span.merge(semi),
            });
        }

        // 8. Local variable declaration (`static`, `const`, `auto`, primitive type, known struct/enum/FB, or `UnknownType var;`)
        if self.looks_like_local_decl() {
            return self.parse_local_decl_stmt();
        }

        // 9. Statement starting with expression -> assignment, compound assignment, `++`/`--`, or expression statement
        let stmt_start_pos = self.pos;
        let expr = self.parse_expr()?;

        // Check for raw pointer arrow `p->x = ...` captured cleanly as Foreign stmt
        if matches!(self.peek().kind, TokenKind::Arrow) {
            self.pos = stmt_start_pos;
            return Some(self.parse_foreign_stmt());
        }

        // Postfix `x++;` / `x--;`
        if matches!(
            self.peek().kind,
            TokenKind::PlusPlus | TokenKind::MinusMinus
        ) {
            let is_increment = matches!(self.bump().kind, TokenKind::PlusPlus);
            let semi = self
                .expect_semi("postfix increment/decrement")
                .unwrap_or(expr.span);
            return Some(CppStmt {
                kind: CppStmtKind::IncDec {
                    target: expr,
                    is_increment,
                },
                span: start_span.merge(semi),
            });
        }

        // Assignment or compound assignment
        if let Some(assign_op) = self.eat_assign_op() {
            let value = self.parse_expr()?;
            let semi = self.expect_semi("assignment").unwrap_or(value.span);
            return Some(CppStmt {
                kind: CppStmtKind::Assign {
                    target: expr,
                    op: assign_op,
                    value,
                },
                span: start_span.merge(semi),
            });
        }

        let semi = self
            .expect_semi("expression statement")
            .unwrap_or(expr.span);
        let full_span = start_span.merge(semi);

        // If the statement is a method call on a known foreign/pack object (`Wire.*`, `SPI.*`, `myServo.*`),
        // emit `CppStmtKind::Foreign` carrying its structured call args for F1 signature checking!
        if let CppExprKind::MethodCall {
            receiver,
            method,
            args,
        } = &expr.kind
            && let CppExprKind::Ident(obj_name) = &receiver.kind
            && obj_name != "Serial"
            && (self.known_foreign_objects.contains(obj_name)
                || matches!(obj_name.as_str(), "Wire" | "SPI"))
        {
            let text = self.slice_span(full_span).trim().to_owned();
            return Some(CppStmt {
                kind: CppStmtKind::Foreign {
                    text,
                    pack_hint: None,
                    receiver_class: Some(obj_name.clone()),
                    method_name: Some(method.clone()),
                    call_args: args.clone(),
                },
                span: full_span,
            });
        }

        // If the statement is a vendor intrinsic call (`cli();`, `wdt_reset();`, `__builtin_*();`),
        // preserve as `Stmt::Foreign(F0)`.
        if let CppExprKind::Call { callee, .. } = &expr.kind
            && is_vendor_intrinsic(callee)
        {
            let text = self.slice_span(full_span).trim().to_owned();
            return Some(CppStmt {
                kind: CppStmtKind::Foreign {
                    text,
                    pack_hint: None,
                    receiver_class: None,
                    method_name: None,
                    call_args: Vec::new(),
                },
                span: full_span,
            });
        }

        // If the expression itself was captured as Foreign (e.g. `new Foo()`), preserve as `Stmt::Foreign`.
        if matches!(expr.kind, CppExprKind::Foreign { .. }) {
            let text = self.slice_span(full_span).trim().to_owned();
            return Some(CppStmt {
                kind: CppStmtKind::Foreign {
                    text,
                    pack_hint: None,
                    receiver_class: None,
                    method_name: None,
                    call_args: Vec::new(),
                },
                span: full_span,
            });
        }

        Some(CppStmt {
            kind: CppStmtKind::ExprStmt { expr },
            span: full_span,
        })
    }

    fn parse_foreign_stmt(&mut self) -> CppStmt {
        let start_span = self.peek().span;
        let end_span = self.skip_foreign_item_tokens(start_span);
        let full_span = start_span.merge(end_span);
        let text = self.slice_span(full_span).trim().to_owned();
        CppStmt {
            kind: CppStmtKind::Foreign {
                text,
                pack_hint: None,
                receiver_class: None,
                method_name: None,
                call_args: Vec::new(),
            },
            span: full_span,
        }
    }

    fn looks_like_local_decl(&self) -> bool {
        let TokenKind::Ident(first) = &self.peek().kind else {
            return false;
        };
        if matches!(
            first.as_str(),
            "static" | "const" | "constexpr" | "volatile" | "auto" | "struct" | "enum"
        ) {
            return true;
        }
        if is_cpp_primitive_type_keyword(first)
            || builtin_fb_kind(first).is_some()
            || self.known_type_names.contains(first)
            || self.known_foreign_classes.contains(first)
        {
            return true;
        }
        // Two consecutive identifiers (`UnknownType varName;` or `UnknownType varName = ...;`)
        // indicate a typed variable declaration so `sema` can report `ERR_UNKNOWN_TYPE`!
        matches!(self.peek_at(1).kind, TokenKind::Ident(_))
            && matches!(
                self.peek_at(2).kind,
                TokenKind::Semi | TokenKind::Assign | TokenKind::LBracket
            )
    }

    fn parse_local_decl_stmt(&mut self) -> Option<CppStmt> {
        let saved_pos = self.pos;
        let start_span = self.peek().span;
        let mut is_static = false;
        let mut is_const = false;
        loop {
            if self.eat_ident("static").is_some() {
                is_static = true;
            } else if self.eat_ident("const").is_some() || self.eat_ident("constexpr").is_some() {
                is_const = true;
            } else if self.eat_ident("volatile").is_some() {
                // accepted qualifier
            } else {
                break;
            }
        }

        let (is_auto, ty) = if self.eat_ident("auto").is_some() {
            (true, None)
        } else {
            let mut spec = self.parse_type_spec()?;
            if is_const {
                spec.is_const = true;
            }
            // Local foreign object (`Servo localServo;`, `String s = "hi";`) or raw pointer other than `const char*`
            let is_const_char_ptr = spec.is_pointer
                && spec.is_const
                && spec.words.len() == 1
                && spec.words.first().map(String::as_str) == Some("char");
            let is_foreign_class = !spec.is_pointer
                && spec.words.len() == 1
                && spec
                    .words
                    .first()
                    .is_some_and(|w| self.known_foreign_classes.contains(w));
            if (spec.is_pointer && !is_const_char_ptr) || is_foreign_class {
                if is_foreign_class && let TokenKind::Ident(vn) = &self.peek().kind {
                    self.known_foreign_objects.insert(vn.clone());
                }
                self.pos = saved_pos;
                return Some(self.parse_foreign_stmt());
            }
            (false, Some(spec))
        };

        let (name, name_span) = self.expect_ident("local variable declaration")?;
        let array_len = if matches!(self.peek().kind, TokenKind::LBracket) {
            let _ = self.bump();
            let len_expr = self.parse_expr();
            if matches!(self.peek().kind, TokenKind::RBracket) {
                let _ = self.bump();
            } else {
                self.push_syntax_err("expected `]` in local array declaration", self.peek().span);
            }
            len_expr
        } else {
            None
        };

        let initial = if matches!(self.peek().kind, TokenKind::Assign) {
            let _ = self.bump();
            self.parse_expr()
        } else {
            None
        };
        let semi = self
            .expect_semi("local variable declaration")
            .unwrap_or(name_span);

        Some(CppStmt {
            kind: CppStmtKind::LocalVar {
                is_static,
                is_const,
                is_auto,
                ty,
                name,
                array_len,
                initial,
            },
            span: start_span.merge(semi),
        })
    }

    fn parse_if_stmt(&mut self) -> Option<CppStmt> {
        let start_span = self.eat_ident("if")?;
        self.expect_lparen("if condition")?;
        let condition = self.parse_expr()?;
        self.expect_rparen("if condition")?;
        let (then_body, mut end_span) = self.parse_stmt_or_block("if body")?;

        let mut elif_arms = Vec::new();
        let mut else_body = Vec::new();

        while self.eat_ident("else").is_some() {
            if self.eat_ident("if").is_some() {
                self.expect_lparen("else if condition")?;
                let c = self.parse_expr()?;
                self.expect_rparen("else if condition")?;
                let (b, sp) = self.parse_stmt_or_block("else if body")?;
                end_span = sp;
                elif_arms.push((c, b));
            } else {
                let (b, sp) = self.parse_stmt_or_block("else body")?;
                end_span = sp;
                else_body = b;
                break;
            }
        }

        Some(CppStmt {
            kind: CppStmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            },
            span: start_span.merge(end_span),
        })
    }

    fn parse_stmt_or_block(&mut self, context: &str) -> Option<(Vec<CppStmt>, Span)> {
        if matches!(self.peek().kind, TokenKind::LBrace) {
            self.parse_block_stmts(context)
        } else {
            let stmt = self.parse_stmt()?;
            let sp = stmt.span;
            Some((vec![stmt], sp))
        }
    }

    fn parse_switch_stmt(&mut self) -> Option<CppStmt> {
        let saved_pos = self.pos;
        let start_span = self.eat_ident("switch")?;
        self.expect_lparen("switch scrutinee")?;
        let scrutinee = self.parse_expr()?;
        self.expect_rparen("switch scrutinee")?;
        self.expect_lbrace("switch body")?;

        let mut cases: Vec<CppSwitchCase> = Vec::new();
        let mut default: Option<Vec<CppStmt>> = None;
        let mut has_implicit_fallthrough = false;

        while !self.is_eof() && !matches!(self.peek().kind, TokenKind::RBrace) {
            if self.check_ident("case") {
                let mut values = Vec::new();
                while self.eat_ident("case").is_some() {
                    let val = self.parse_expr()?;
                    if matches!(self.peek().kind, TokenKind::Colon) {
                        let _ = self.bump();
                    } else {
                        self.push_syntax_err("expected `:` after case label", self.peek().span);
                    }
                    values.push(val);
                }
                let mut body = Vec::new();
                let mut terminated = false;
                while !self.is_eof()
                    && !matches!(self.peek().kind, TokenKind::RBrace)
                    && !self.check_ident("case")
                    && !self.check_ident("default")
                {
                    if let Some(stmt) = self.parse_stmt() {
                        if matches!(stmt.kind, CppStmtKind::Break) {
                            terminated = true;
                            break;
                        } else if matches!(stmt.kind, CppStmtKind::Return { .. }) {
                            body.push(stmt);
                            terminated = true;
                            break;
                        } else {
                            body.push(stmt);
                        }
                    }
                }
                if !body.is_empty() && !terminated {
                    has_implicit_fallthrough = true;
                }
                cases.push(CppSwitchCase { values, body });
            } else if self.eat_ident("default").is_some() {
                if matches!(self.peek().kind, TokenKind::Colon) {
                    let _ = self.bump();
                } else {
                    self.push_syntax_err("expected `:` after `default`", self.peek().span);
                }
                let mut body = Vec::new();
                while !self.is_eof()
                    && !matches!(self.peek().kind, TokenKind::RBrace)
                    && !self.check_ident("case")
                    && !self.check_ident("default")
                {
                    if let Some(stmt) = self.parse_stmt() {
                        if matches!(stmt.kind, CppStmtKind::Break) {
                            break;
                        }
                        body.push(stmt);
                    }
                }
                default = Some(body);
            } else {
                self.push_syntax_err(
                    "expected `case` or `default` inside `switch`",
                    self.peek().span,
                );
                let _ = self.bump();
            }
        }

        let rbrace = self.expect_rbrace("switch body").unwrap_or(start_span);
        let full_span = start_span.merge(rbrace);

        if has_implicit_fallthrough {
            let text = self.slice_span(full_span).trim().to_owned();
            let _ = saved_pos;
            return Some(CppStmt {
                kind: CppStmtKind::Foreign {
                    text,
                    pack_hint: None,
                    receiver_class: None,
                    method_name: None,
                    call_args: Vec::new(),
                },
                span: full_span,
            });
        }

        Some(CppStmt {
            kind: CppStmtKind::Switch {
                scrutinee,
                cases,
                default,
            },
            span: full_span,
        })
    }

    fn parse_while_stmt(&mut self) -> Option<CppStmt> {
        let start_span = self.eat_ident("while")?;
        self.expect_lparen("while condition")?;
        let condition = self.parse_expr()?;
        self.expect_rparen("while condition")?;
        let (body, end_span) = self.parse_stmt_or_block("while body")?;
        Some(CppStmt {
            kind: CppStmtKind::While { condition, body },
            span: start_span.merge(end_span),
        })
    }

    fn parse_do_while_stmt(&mut self) -> Option<CppStmt> {
        let start_span = self.eat_ident("do")?;
        let (body, _) = self.parse_stmt_or_block("do-while body")?;
        if self.eat_ident("while").is_none() {
            self.push_syntax_err("expected `while` after `do` block", self.peek().span);
            return None;
        }
        self.expect_lparen("do-while condition")?;
        let condition = self.parse_expr()?;
        let rparen = self
            .expect_rparen("do-while condition")
            .unwrap_or(condition.span);
        let semi = self.expect_semi("do-while statement").unwrap_or(rparen);
        Some(CppStmt {
            kind: CppStmtKind::DoWhile { body, condition },
            span: start_span.merge(semi),
        })
    }

    fn parse_for_stmt(&mut self) -> Option<CppStmt> {
        let start_span = self.eat_ident("for")?;
        self.expect_lparen("for header")?;

        // Init clause
        let init = if matches!(self.peek().kind, TokenKind::Semi) {
            None
        } else if self.looks_like_local_decl() {
            let init_start = self.peek().span;
            let ty = self.parse_type_spec()?;
            let (name, _) = self.expect_ident("for-loop variable")?;
            if !matches!(self.peek().kind, TokenKind::Assign) {
                self.push_syntax_err("expected `=` in for-loop initializer", self.peek().span);
                return None;
            }
            let _ = self.bump();
            let val = self.parse_expr()?;
            let span = init_start.merge(val.span);
            Some(Box::new(CppForInit::Decl {
                ty,
                name,
                init: val,
                span,
            }))
        } else {
            let init_start = self.peek().span;
            let (name, _) = self.expect_ident("for-loop counter")?;
            if !matches!(self.peek().kind, TokenKind::Assign) {
                self.push_syntax_err("expected `=` in for-loop initializer", self.peek().span);
                return None;
            }
            let _ = self.bump();
            let val = self.parse_expr()?;
            let span = init_start.merge(val.span);
            Some(Box::new(CppForInit::Assign {
                name,
                init: val,
                span,
            }))
        };
        self.expect_semi("for-loop initializer")?;

        // Condition clause
        let condition = if matches!(self.peek().kind, TokenKind::Semi) {
            None
        } else {
            self.parse_expr()
        };
        self.expect_semi("for-loop condition")?;

        // Step clause
        let step = if matches!(self.peek().kind, TokenKind::RParen) {
            None
        } else if matches!(
            self.peek().kind,
            TokenKind::PlusPlus | TokenKind::MinusMinus
        ) {
            let is_inc = matches!(self.bump().kind, TokenKind::PlusPlus);
            let (name, _) = self.expect_ident("for-loop step")?;
            Some(Box::new(if is_inc {
                CppForStep::Inc { name }
            } else {
                CppForStep::Dec { name }
            }))
        } else {
            let (name, _) = self.expect_ident("for-loop step")?;
            if matches!(self.peek().kind, TokenKind::PlusPlus) {
                let _ = self.bump();
                Some(Box::new(CppForStep::Inc { name }))
            } else if matches!(self.peek().kind, TokenKind::MinusMinus) {
                let _ = self.bump();
                Some(Box::new(CppForStep::Dec { name }))
            } else if let Some(maybe_op) = self.eat_assign_op() {
                let rhs = self.parse_expr()?;
                Some(Box::new(match maybe_op {
                    Some(op) => CppForStep::Compound { name, op, rhs },
                    None => CppForStep::Assign { name, rhs },
                }))
            } else {
                self.push_syntax_err("invalid for-loop step expression", self.peek().span);
                None
            }
        };

        self.expect_rparen("for header")?;
        let (body, end_span) = self.parse_stmt_or_block("for body")?;

        Some(CppStmt {
            kind: CppStmtKind::For {
                init,
                condition,
                step,
                body,
            },
            span: start_span.merge(end_span),
        })
    }

    fn eat_assign_op(&mut self) -> Option<Option<CompoundOp>> {
        let res = match self.peek().kind {
            TokenKind::Assign => Some(None),
            TokenKind::PlusAssign => Some(Some(CompoundOp::Add)),
            TokenKind::MinusAssign => Some(Some(CompoundOp::Sub)),
            TokenKind::StarAssign => Some(Some(CompoundOp::Mul)),
            TokenKind::SlashAssign => Some(Some(CompoundOp::Div)),
            TokenKind::PercentAssign => Some(Some(CompoundOp::Mod)),
            TokenKind::AmpAssign => Some(Some(CompoundOp::BitAnd)),
            TokenKind::PipeAssign => Some(Some(CompoundOp::BitOr)),
            TokenKind::CaretAssign => Some(Some(CompoundOp::BitXor)),
            TokenKind::ShlAssign => Some(Some(CompoundOp::Shl)),
            TokenKind::ShrAssign => Some(Some(CompoundOp::Shr)),
            TokenKind::Ident(_)
            | TokenKind::IntLit { .. }
            | TokenKind::FloatLit { .. }
            | TokenKind::CharLit(_)
            | TokenKind::StringLit(_)
            | TokenKind::Preproc { .. }
            | TokenKind::LParen
            | TokenKind::RParen
            | TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::LBracket
            | TokenKind::RBracket
            | TokenKind::Semi
            | TokenKind::Comma
            | TokenKind::Colon
            | TokenKind::ColonColon
            | TokenKind::Dot
            | TokenKind::Arrow
            | TokenKind::Question
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::PlusPlus
            | TokenKind::MinusMinus
            | TokenKind::EqEq
            | TokenKind::BangEq
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::AmpAmp
            | TokenKind::PipePipe
            | TokenKind::Bang
            | TokenKind::Amp
            | TokenKind::Pipe
            | TokenKind::Caret
            | TokenKind::Tilde
            | TokenKind::Shl
            | TokenKind::Shr
            | TokenKind::Eof => None,
        };
        if res.is_some() {
            let _ = self.bump();
        }
        res
    }

    /// Parses a full C++ expression including right-associative ternary `cond ? then : else`.
    fn parse_expr(&mut self) -> Option<CppExpr> {
        if self.expr_depth >= MAX_NESTING_DEPTH {
            let mut d = Diagnostic::new(
                ErrorCode::ErrRecursionDepth,
                format!(
                    "expression nesting exceeds maximum recursion depth of {MAX_NESTING_DEPTH}"
                ),
            )
            .with_board_id(self.board_id);
            d.span = Some(self.peek().span);
            self.errors.push(d);
            return None;
        }
        self.expr_depth = self.expr_depth.saturating_add(1);
        let res = self.parse_expr_inner();
        self.expr_depth = self.expr_depth.saturating_sub(1);
        res
    }

    fn parse_expr_inner(&mut self) -> Option<CppExpr> {
        let cond = match parse_pratt(self) {
            Ok(e) => e,
            Err(diag) => {
                self.errors.push(diag);
                return None;
            }
        };

        if matches!(self.peek().kind, TokenKind::Question) {
            let _ = self.bump();
            let then_expr = self.parse_expr()?;
            if matches!(self.peek().kind, TokenKind::Colon) {
                let _ = self.bump();
            } else {
                self.push_syntax_err("expected `:` in ternary `? :` expression", self.peek().span);
                return None;
            }
            let else_expr = self.parse_expr()?;
            let span = cond.span.merge(else_expr.span);
            return Some(CppExpr {
                kind: CppExprKind::Ternary {
                    condition: Box::new(cond),
                    then_expr: Box::new(then_expr),
                    else_expr: Box::new(else_expr),
                },
                span,
            });
        }

        Some(cond)
    }

    /// Checks whether the current `(` starts a C-style cast `(TypeSpec)operand`.
    fn try_parse_c_style_cast(&mut self) -> Option<CppTypeSpec> {
        if !matches!(self.peek().kind, TokenKind::LParen) {
            return None;
        }
        let saved = self.pos;
        let _ = self.bump(); // `(`
        let is_type_start = if let TokenKind::Ident(w) = &self.peek().kind {
            w == "const" || is_cpp_primitive_type_keyword(w) || self.known_type_names.contains(w)
        } else {
            false
        };
        if !is_type_start {
            self.pos = saved;
            return None;
        }
        let Some(ty) = self.parse_type_spec() else {
            self.pos = saved;
            return None;
        };
        if !matches!(self.peek().kind, TokenKind::RParen) {
            self.pos = saved;
            return None;
        }
        let _ = self.bump(); // `)`
        // Ensure next token can start a cast operand (not `;`, `)`, `,`, binary op except `+`/`-`/`*`/`&`)
        let can_start_operand = matches!(
            self.peek().kind,
            TokenKind::Ident(_)
                | TokenKind::IntLit { .. }
                | TokenKind::FloatLit { .. }
                | TokenKind::CharLit(_)
                | TokenKind::StringLit(_)
                | TokenKind::LParen
                | TokenKind::Bang
                | TokenKind::Tilde
                | TokenKind::Minus
                | TokenKind::Plus
        );
        if !can_start_operand {
            self.pos = saved;
            return None;
        }
        Some(ty)
    }
}

#[derive(Debug, Clone, Copy)]
enum CppPrefixOp {
    Unary(UnaryOp, Span),
    CastPlaceholder,
}

impl PrattDriver for Parser<'_> {
    type Expr = CppExpr;
    type PrefixOp = CppPrefixOp;
    type InfixOp = BinaryOp;
    type Error = Diagnostic;

    fn peek_prefix(&mut self) -> Option<PrefixSpec<Self::PrefixOp>> {
        let span = self.peek().span;
        match self.peek().kind {
            TokenKind::Minus => Some(PrefixSpec {
                op: CppPrefixOp::Unary(UnaryOp::Neg, span),
                right_bp: 22,
            }),
            TokenKind::Plus => Some(PrefixSpec {
                op: CppPrefixOp::Unary(UnaryOp::Pos, span),
                right_bp: 22,
            }),
            TokenKind::Bang => Some(PrefixSpec {
                op: CppPrefixOp::Unary(UnaryOp::Not, span),
                right_bp: 22,
            }),
            TokenKind::Tilde => Some(PrefixSpec {
                op: CppPrefixOp::Unary(UnaryOp::BitNot, span),
                right_bp: 22,
            }),
            // Check C-style cast `(Type)expr` cleanly inside parse_primary/peek_prefix?
            // We handle C-style casts inside parse_primary to avoid carrying TypeSpec in Copy PrefixOp!
            TokenKind::Ident(_)
            | TokenKind::IntLit { .. }
            | TokenKind::FloatLit { .. }
            | TokenKind::CharLit(_)
            | TokenKind::StringLit(_)
            | TokenKind::Preproc { .. }
            | TokenKind::LParen
            | TokenKind::RParen
            | TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::LBracket
            | TokenKind::RBracket
            | TokenKind::Semi
            | TokenKind::Comma
            | TokenKind::Colon
            | TokenKind::ColonColon
            | TokenKind::Dot
            | TokenKind::Arrow
            | TokenKind::Question
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::PlusPlus
            | TokenKind::MinusMinus
            | TokenKind::Assign
            | TokenKind::PlusAssign
            | TokenKind::MinusAssign
            | TokenKind::StarAssign
            | TokenKind::SlashAssign
            | TokenKind::PercentAssign
            | TokenKind::AmpAssign
            | TokenKind::PipeAssign
            | TokenKind::CaretAssign
            | TokenKind::ShlAssign
            | TokenKind::ShrAssign
            | TokenKind::EqEq
            | TokenKind::BangEq
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::AmpAmp
            | TokenKind::PipePipe
            | TokenKind::Amp
            | TokenKind::Pipe
            | TokenKind::Caret
            | TokenKind::Shl
            | TokenKind::Shr
            | TokenKind::Eof => None,
        }
    }

    fn consume_prefix(&mut self) -> Result<Self::PrefixOp, Self::Error> {
        let tok = self.bump();
        let op = if matches!(tok.kind, TokenKind::Minus) {
            CppPrefixOp::Unary(UnaryOp::Neg, tok.span)
        } else if matches!(tok.kind, TokenKind::Plus) {
            CppPrefixOp::Unary(UnaryOp::Pos, tok.span)
        } else if matches!(tok.kind, TokenKind::Bang) {
            CppPrefixOp::Unary(UnaryOp::Not, tok.span)
        } else if matches!(tok.kind, TokenKind::Tilde) {
            CppPrefixOp::Unary(UnaryOp::BitNot, tok.span)
        } else {
            CppPrefixOp::CastPlaceholder
        };
        Ok(op)
    }

    fn parse_primary(&mut self) -> Result<Self::Expr, Self::Error> {
        let start_span = self.peek().span;

        // C++ lambda `[...](...) { ... }` -> Expr::Foreign(F0)
        if matches!(self.peek().kind, TokenKind::LBracket) {
            let mut end_span = start_span;
            let mut bracket_depth = 0usize;
            while !self.is_eof() {
                let t = self.bump();
                end_span = t.span;
                if matches!(t.kind, TokenKind::LBracket) {
                    bracket_depth = bracket_depth.saturating_add(1);
                } else if matches!(t.kind, TokenKind::RBracket) {
                    bracket_depth = bracket_depth.saturating_sub(1);
                    if bracket_depth == 0 {
                        break;
                    }
                }
            }
            // Optional `(...)` and `-> Type` and `{ ... }`
            if matches!(self.peek().kind, TokenKind::LParen) {
                let mut pdepth = 0usize;
                while !self.is_eof() {
                    let t = self.bump();
                    end_span = t.span;
                    if matches!(t.kind, TokenKind::LParen) {
                        pdepth = pdepth.saturating_add(1);
                    } else if matches!(t.kind, TokenKind::RParen) {
                        pdepth = pdepth.saturating_sub(1);
                        if pdepth == 0 {
                            break;
                        }
                    }
                }
            }
            if matches!(self.peek().kind, TokenKind::Arrow) {
                let _ = self.bump();
                let _ = self.parse_type_spec();
            }
            if matches!(self.peek().kind, TokenKind::LBrace) {
                let mut bdepth = 0usize;
                while !self.is_eof() {
                    let t = self.bump();
                    end_span = t.span;
                    if matches!(t.kind, TokenKind::LBrace) {
                        bdepth = bdepth.saturating_add(1);
                    } else if matches!(t.kind, TokenKind::RBrace) {
                        bdepth = bdepth.saturating_sub(1);
                        if bdepth == 0 {
                            break;
                        }
                    }
                }
            }
            let full = start_span.merge(end_span);
            return Ok(CppExpr {
                kind: CppExprKind::Foreign {
                    text: self.slice_span(full).trim().to_owned(),
                },
                span: full,
            });
        }

        // Raw pointer deref `*p` or address-of `&x` -> Expr::Foreign(F0)
        if matches!(self.peek().kind, TokenKind::Star | TokenKind::Amp) {
            let _ = self.bump();
            let inner = self.parse_primary()?;
            let full = start_span.merge(inner.span);
            return Ok(CppExpr {
                kind: CppExprKind::Foreign {
                    text: self.slice_span(full).trim().to_owned(),
                },
                span: full,
            });
        }

        // C-style cast `(Type)expr` OR parenthesized `(expr)`
        if matches!(self.peek().kind, TokenKind::LParen) {
            if let Some(target_ty) = self.try_parse_c_style_cast() {
                let operand = parse_pratt_unary(self)?;
                let span = start_span.merge(operand.span);
                return Ok(CppExpr {
                    kind: CppExprKind::Cast {
                        target: target_ty,
                        operand: Box::new(operand),
                    },
                    span,
                });
            }
            let _ = self.bump(); // `(`
            let inner = self.parse_expr().ok_or_else(|| {
                let mut d = Diagnostic::new(
                    ErrorCode::ErrTomlSyntax,
                    "expected expression inside parentheses",
                )
                .with_board_id(self.board_id);
                d.span = Some(self.peek().span);
                d
            })?;
            let rparen = if matches!(self.peek().kind, TokenKind::RParen) {
                self.bump().span
            } else {
                let mut d = Diagnostic::new(
                    ErrorCode::ErrTomlSyntax,
                    "expected closing `)` after parenthesized expression",
                )
                .with_board_id(self.board_id);
                d.span = Some(self.peek().span);
                return Err(d);
            };
            return Ok(CppExpr {
                kind: inner.kind,
                span: start_span.merge(rparen),
            });
        }

        let tok = self.bump();
        match &tok.kind {
            TokenKind::IntLit {
                value,
                unsigned,
                long_count,
            } => Ok(CppExpr {
                kind: CppExprKind::IntLit {
                    value: *value,
                    unsigned: *unsigned,
                    long_count: *long_count,
                },
                span: tok.span,
            }),
            TokenKind::FloatLit { value, is_f32 } => Ok(CppExpr {
                kind: CppExprKind::FloatLit {
                    value: *value,
                    is_f32: *is_f32,
                },
                span: tok.span,
            }),
            TokenKind::CharLit(ch) => Ok(CppExpr {
                kind: CppExprKind::CharLit(*ch),
                span: tok.span,
            }),
            TokenKind::StringLit(s) => Ok(CppExpr {
                kind: CppExprKind::StringLit(s.clone()),
                span: tok.span,
            }),
            TokenKind::Ident(ident) => {
                if ident == "true" || ident == "HIGH" {
                    return Ok(CppExpr {
                        kind: CppExprKind::BoolLit(true),
                        span: tok.span,
                    });
                }
                if ident == "false" || ident == "LOW" {
                    return Ok(CppExpr {
                        kind: CppExprKind::BoolLit(false),
                        span: tok.span,
                    });
                }
                if ident == "static_cast" && matches!(self.peek().kind, TokenKind::Lt) {
                    let _ = self.bump(); // `<`
                    let Some(target) = self.parse_type_spec() else {
                        let mut d = Diagnostic::new(
                            ErrorCode::ErrTomlSyntax,
                            "expected target type inside `static_cast<...>`",
                        )
                        .with_board_id(self.board_id);
                        d.span = Some(self.peek().span);
                        return Err(d);
                    };
                    if !matches!(self.peek().kind, TokenKind::Gt) {
                        let mut d = Diagnostic::new(
                            ErrorCode::ErrTomlSyntax,
                            "expected `>` closing `static_cast<Type>`",
                        )
                        .with_board_id(self.board_id);
                        d.span = Some(self.peek().span);
                        return Err(d);
                    }
                    let _ = self.bump(); // `>`
                    if !matches!(self.peek().kind, TokenKind::LParen) {
                        let mut d = Diagnostic::new(
                            ErrorCode::ErrTomlSyntax,
                            "expected `(` after `static_cast<Type>`",
                        )
                        .with_board_id(self.board_id);
                        d.span = Some(self.peek().span);
                        return Err(d);
                    }
                    let _ = self.bump(); // `(`
                    let operand = self.parse_expr().ok_or_else(|| {
                        let mut d = Diagnostic::new(
                            ErrorCode::ErrTomlSyntax,
                            "expected operand inside `static_cast<Type>(...)`",
                        )
                        .with_board_id(self.board_id);
                        d.span = Some(self.peek().span);
                        d
                    })?;
                    let rparen = if matches!(self.peek().kind, TokenKind::RParen) {
                        self.bump().span
                    } else {
                        let mut d = Diagnostic::new(
                            ErrorCode::ErrTomlSyntax,
                            "expected `)` closing `static_cast`",
                        )
                        .with_board_id(self.board_id);
                        d.span = Some(self.peek().span);
                        return Err(d);
                    };
                    return Ok(CppExpr {
                        kind: CppExprKind::Cast {
                            target,
                            operand: Box::new(operand),
                        },
                        span: start_span.merge(rparen),
                    });
                }
                if ident == "new" {
                    let mut end_sp = tok.span;
                    while !self.is_eof()
                        && !matches!(
                            self.peek().kind,
                            TokenKind::Semi | TokenKind::Comma | TokenKind::RParen
                        )
                    {
                        end_sp = self.bump().span;
                    }
                    let full = start_span.merge(end_sp);
                    return Ok(CppExpr {
                        kind: CppExprKind::Foreign {
                            text: self.slice_span(full).trim().to_owned(),
                        },
                        span: full,
                    });
                }
                if is_avr_register(ident) {
                    return Ok(CppExpr {
                        kind: CppExprKind::Foreign {
                            text: ident.clone(),
                        },
                        span: tok.span,
                    });
                }
                // Scoped identifier `Enum::Variant`
                if matches!(self.peek().kind, TokenKind::ColonColon) {
                    let _ = self.bump();
                    let member_tok = self.bump();
                    if let TokenKind::Ident(member) = &member_tok.kind {
                        return Ok(CppExpr {
                            kind: CppExprKind::ScopedIdent {
                                scope: ident.clone(),
                                member: member.clone(),
                            },
                            span: start_span.merge(member_tok.span),
                        });
                    }
                    let mut d =
                        Diagnostic::new(ErrorCode::ErrTomlSyntax, "expected identifier after `::`")
                            .with_board_id(self.board_id);
                    d.span = Some(member_tok.span);
                    return Err(d);
                }
                Ok(CppExpr {
                    kind: CppExprKind::Ident(ident.clone()),
                    span: tok.span,
                })
            }
            TokenKind::Preproc { .. }
            | TokenKind::LParen
            | TokenKind::RParen
            | TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::LBracket
            | TokenKind::RBracket
            | TokenKind::Semi
            | TokenKind::Comma
            | TokenKind::Colon
            | TokenKind::ColonColon
            | TokenKind::Dot
            | TokenKind::Arrow
            | TokenKind::Question
            | TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::PlusPlus
            | TokenKind::MinusMinus
            | TokenKind::Assign
            | TokenKind::PlusAssign
            | TokenKind::MinusAssign
            | TokenKind::StarAssign
            | TokenKind::SlashAssign
            | TokenKind::PercentAssign
            | TokenKind::AmpAssign
            | TokenKind::PipeAssign
            | TokenKind::CaretAssign
            | TokenKind::ShlAssign
            | TokenKind::ShrAssign
            | TokenKind::EqEq
            | TokenKind::BangEq
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::AmpAmp
            | TokenKind::PipePipe
            | TokenKind::Bang
            | TokenKind::Amp
            | TokenKind::Pipe
            | TokenKind::Caret
            | TokenKind::Tilde
            | TokenKind::Shl
            | TokenKind::Shr
            | TokenKind::Eof => {
                let mut d = Diagnostic::new(ErrorCode::ErrTomlSyntax, "expected expression")
                    .with_board_id(self.board_id);
                d.span = Some(tok.span);
                Err(d)
            }
        }
    }

    fn parse_postfix(&mut self, mut expr: Self::Expr) -> Result<Self::Expr, Self::Error> {
        loop {
            match self.peek().kind {
                TokenKind::LBracket => {
                    let _ = self.bump();
                    let index = self.parse_expr().ok_or_else(|| {
                        let mut d = Diagnostic::new(
                            ErrorCode::ErrTomlSyntax,
                            "expected index expression inside `[...]`",
                        )
                        .with_board_id(self.board_id);
                        d.span = Some(self.peek().span);
                        d
                    })?;
                    let rbrack = if matches!(self.peek().kind, TokenKind::RBracket) {
                        self.bump().span
                    } else {
                        let mut d = Diagnostic::new(
                            ErrorCode::ErrTomlSyntax,
                            "expected `]` closing index expression",
                        )
                        .with_board_id(self.board_id);
                        d.span = Some(self.peek().span);
                        return Err(d);
                    };
                    let span = expr.span.merge(rbrack);
                    expr = CppExpr {
                        kind: CppExprKind::Index {
                            base: Box::new(expr),
                            index: Box::new(index),
                        },
                        span,
                    };
                }
                TokenKind::Dot => {
                    let _ = self.bump();
                    let field_tok = self.bump();
                    let TokenKind::Ident(field) = &field_tok.kind else {
                        let mut d = Diagnostic::new(
                            ErrorCode::ErrTomlSyntax,
                            "expected field or method name after `.`",
                        )
                        .with_board_id(self.board_id);
                        d.span = Some(field_tok.span);
                        return Err(d);
                    };
                    let field_name = field.clone();
                    if matches!(self.peek().kind, TokenKind::LParen) {
                        let (args, rparen) = parse_call_args(self)?;
                        let span = expr.span.merge(rparen);
                        expr = CppExpr {
                            kind: CppExprKind::MethodCall {
                                receiver: Box::new(expr),
                                method: field_name,
                                args,
                            },
                            span,
                        };
                    } else {
                        let span = expr.span.merge(field_tok.span);
                        expr = CppExpr {
                            kind: CppExprKind::Field {
                                base: Box::new(expr),
                                field: field_name,
                            },
                            span,
                        };
                    }
                }
                TokenKind::Arrow => {
                    // Pointer arrow `p->field` / `p->method(...)` -> Expr::Foreign(F0)
                    let _ = self.bump();
                    let member_tok = self.bump();
                    let mut end_sp = member_tok.span;
                    if matches!(self.peek().kind, TokenKind::LParen) {
                        let (_, rparen) = parse_call_args(self)?;
                        end_sp = rparen;
                    }
                    let span = expr.span.merge(end_sp);
                    expr = CppExpr {
                        kind: CppExprKind::Foreign {
                            text: self.slice_span(span).trim().to_owned(),
                        },
                        span,
                    };
                }
                TokenKind::LParen => {
                    // Immediately-invoked lambda `([](...) { ... })(args)` -> Expr::Foreign(F0)
                    if matches!(expr.kind, CppExprKind::Foreign { .. }) {
                        let (_, rparen) = parse_call_args(self)?;
                        let span = expr.span.merge(rparen);
                        expr = CppExpr {
                            kind: CppExprKind::Foreign {
                                text: self.slice_span(span).trim().to_owned(),
                            },
                            span,
                        };
                        continue;
                    }
                    let CppExprKind::Ident(callee) = expr.kind else {
                        let mut d = Diagnostic::new(
                            ErrorCode::ErrTomlSyntax,
                            "cannot invoke non-function expression",
                        )
                        .with_board_id(self.board_id);
                        d.span = Some(self.peek().span);
                        return Err(d);
                    };
                    // Designated FB call `fb({.IN = a, .PT = b})`
                    if matches!(self.peek_at(1).kind, TokenKind::LBrace)
                        && matches!(self.peek_at(2).kind, TokenKind::Dot)
                    {
                        let _ = self.bump(); // `(`
                        let _ = self.bump(); // `{`
                        let mut fields = Vec::new();
                        while !self.is_eof() && !matches!(self.peek().kind, TokenKind::RBrace) {
                            if !matches!(self.peek().kind, TokenKind::Dot) {
                                break;
                            }
                            let _ = self.bump();
                            let Some((fname, _)) = self.expect_ident("designated FB input") else {
                                break;
                            };
                            if matches!(self.peek().kind, TokenKind::Assign) {
                                let _ = self.bump();
                            }
                            if let Some(val) = self.parse_expr() {
                                fields.push((fname, val));
                            }
                            if matches!(self.peek().kind, TokenKind::Comma) {
                                let _ = self.bump();
                            } else {
                                break;
                            }
                        }
                        let _ = self.expect_rbrace("designated FB input");
                        let rparen = self
                            .expect_rparen("designated FB call")
                            .unwrap_or(expr.span);
                        expr = CppExpr {
                            kind: CppExprKind::DesignatedCall { callee, fields },
                            span: expr.span.merge(rparen),
                        };
                        continue;
                    }

                    // Special `sizeof(Type)` -> Foreign(F0)
                    if callee == "sizeof" {
                        let mut end_sp = expr.span;
                        let mut pdepth = 0usize;
                        while !self.is_eof() {
                            let t = self.bump();
                            end_sp = t.span;
                            if matches!(t.kind, TokenKind::LParen) {
                                pdepth = pdepth.saturating_add(1);
                            } else if matches!(t.kind, TokenKind::RParen) {
                                pdepth = pdepth.saturating_sub(1);
                                if pdepth == 0 {
                                    break;
                                }
                            }
                        }
                        let span = expr.span.merge(end_sp);
                        expr = CppExpr {
                            kind: CppExprKind::Foreign {
                                text: self.slice_span(span).trim().to_owned(),
                            },
                            span,
                        };
                        continue;
                    }

                    let (args, rparen) = parse_call_args(self)?;
                    let span = expr.span.merge(rparen);
                    if is_vendor_intrinsic(&callee) {
                        expr = CppExpr {
                            kind: CppExprKind::Foreign {
                                text: self.slice_span(span).trim().to_owned(),
                            },
                            span,
                        };
                    } else {
                        expr = CppExpr {
                            kind: CppExprKind::Call { callee, args },
                            span,
                        };
                    }
                }
                TokenKind::Ident(_)
                | TokenKind::IntLit { .. }
                | TokenKind::FloatLit { .. }
                | TokenKind::CharLit(_)
                | TokenKind::StringLit(_)
                | TokenKind::Preproc { .. }
                | TokenKind::RParen
                | TokenKind::LBrace
                | TokenKind::RBrace
                | TokenKind::RBracket
                | TokenKind::Semi
                | TokenKind::Comma
                | TokenKind::Colon
                | TokenKind::ColonColon
                | TokenKind::Question
                | TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Star
                | TokenKind::Slash
                | TokenKind::Percent
                | TokenKind::PlusPlus
                | TokenKind::MinusMinus
                | TokenKind::Assign
                | TokenKind::PlusAssign
                | TokenKind::MinusAssign
                | TokenKind::StarAssign
                | TokenKind::SlashAssign
                | TokenKind::PercentAssign
                | TokenKind::AmpAssign
                | TokenKind::PipeAssign
                | TokenKind::CaretAssign
                | TokenKind::ShlAssign
                | TokenKind::ShrAssign
                | TokenKind::EqEq
                | TokenKind::BangEq
                | TokenKind::Lt
                | TokenKind::Le
                | TokenKind::Gt
                | TokenKind::Ge
                | TokenKind::AmpAmp
                | TokenKind::PipePipe
                | TokenKind::Bang
                | TokenKind::Amp
                | TokenKind::Pipe
                | TokenKind::Caret
                | TokenKind::Tilde
                | TokenKind::Shl
                | TokenKind::Shr
                | TokenKind::Eof => break,
            }
        }
        Ok(expr)
    }

    fn peek_infix(&mut self) -> Option<InfixSpec<Self::InfixOp>> {
        match self.peek().kind {
            TokenKind::PipePipe => Some(InfixSpec {
                op: BinaryOp::Or,
                precedence: 1,
                assoc: Associativity::Left,
            }),
            TokenKind::AmpAmp => Some(InfixSpec {
                op: BinaryOp::And,
                precedence: 2,
                assoc: Associativity::Left,
            }),
            TokenKind::Pipe => Some(InfixSpec {
                op: BinaryOp::BitOr,
                precedence: 3,
                assoc: Associativity::Left,
            }),
            TokenKind::Caret => Some(InfixSpec {
                op: BinaryOp::BitXor,
                precedence: 4,
                assoc: Associativity::Left,
            }),
            TokenKind::Amp => Some(InfixSpec {
                op: BinaryOp::BitAnd,
                precedence: 5,
                assoc: Associativity::Left,
            }),
            TokenKind::EqEq => Some(InfixSpec {
                op: BinaryOp::Eq,
                precedence: 6,
                assoc: Associativity::NonAssoc,
            }),
            TokenKind::BangEq => Some(InfixSpec {
                op: BinaryOp::Ne,
                precedence: 6,
                assoc: Associativity::NonAssoc,
            }),
            TokenKind::Lt => Some(InfixSpec {
                op: BinaryOp::Lt,
                precedence: 7,
                assoc: Associativity::NonAssoc,
            }),
            TokenKind::Le => Some(InfixSpec {
                op: BinaryOp::Le,
                precedence: 7,
                assoc: Associativity::NonAssoc,
            }),
            TokenKind::Gt => Some(InfixSpec {
                op: BinaryOp::Gt,
                precedence: 7,
                assoc: Associativity::NonAssoc,
            }),
            TokenKind::Ge => Some(InfixSpec {
                op: BinaryOp::Ge,
                precedence: 7,
                assoc: Associativity::NonAssoc,
            }),
            TokenKind::Shl => Some(InfixSpec {
                op: BinaryOp::Shl,
                precedence: 8,
                assoc: Associativity::Left,
            }),
            TokenKind::Shr => Some(InfixSpec {
                op: BinaryOp::Shr,
                precedence: 8,
                assoc: Associativity::Left,
            }),
            TokenKind::Plus => Some(InfixSpec {
                op: BinaryOp::Add,
                precedence: 9,
                assoc: Associativity::Left,
            }),
            TokenKind::Minus => Some(InfixSpec {
                op: BinaryOp::Sub,
                precedence: 9,
                assoc: Associativity::Left,
            }),
            TokenKind::Star => Some(InfixSpec {
                op: BinaryOp::Mul,
                precedence: 10,
                assoc: Associativity::Left,
            }),
            TokenKind::Slash => Some(InfixSpec {
                op: BinaryOp::Div,
                precedence: 10,
                assoc: Associativity::Left,
            }),
            TokenKind::Percent => Some(InfixSpec {
                op: BinaryOp::Mod,
                precedence: 10,
                assoc: Associativity::Left,
            }),
            TokenKind::Ident(_)
            | TokenKind::IntLit { .. }
            | TokenKind::FloatLit { .. }
            | TokenKind::CharLit(_)
            | TokenKind::StringLit(_)
            | TokenKind::Preproc { .. }
            | TokenKind::LParen
            | TokenKind::RParen
            | TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::LBracket
            | TokenKind::RBracket
            | TokenKind::Semi
            | TokenKind::Comma
            | TokenKind::Colon
            | TokenKind::ColonColon
            | TokenKind::Dot
            | TokenKind::Arrow
            | TokenKind::Question
            | TokenKind::PlusPlus
            | TokenKind::MinusMinus
            | TokenKind::Assign
            | TokenKind::PlusAssign
            | TokenKind::MinusAssign
            | TokenKind::StarAssign
            | TokenKind::SlashAssign
            | TokenKind::PercentAssign
            | TokenKind::AmpAssign
            | TokenKind::PipeAssign
            | TokenKind::CaretAssign
            | TokenKind::ShlAssign
            | TokenKind::ShrAssign
            | TokenKind::Bang
            | TokenKind::Tilde
            | TokenKind::Eof => None,
        }
    }

    fn consume_infix(&mut self) -> Result<Self::InfixOp, Self::Error> {
        let Some(spec) = self.peek_infix() else {
            let mut d = Diagnostic::new(ErrorCode::ErrTomlSyntax, "expected binary operator")
                .with_board_id(self.board_id);
            d.span = Some(self.peek().span);
            return Err(d);
        };
        let _ = self.bump();
        Ok(spec.op)
    }

    fn build_prefix(
        &mut self,
        op: Self::PrefixOp,
        operand: Self::Expr,
    ) -> Result<Self::Expr, Self::Error> {
        match op {
            CppPrefixOp::Unary(uop, op_span) => {
                let span = op_span.merge(operand.span);
                Ok(CppExpr {
                    kind: CppExprKind::Unary {
                        op: uop,
                        operand: Box::new(operand),
                    },
                    span,
                })
            }
            CppPrefixOp::CastPlaceholder => Ok(operand),
        }
    }

    fn build_infix(
        &mut self,
        op: Self::InfixOp,
        left: Self::Expr,
        right: Self::Expr,
    ) -> Result<Self::Expr, Self::Error> {
        let span = left.span.merge(right.span);
        Ok(CppExpr {
            kind: CppExprKind::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            },
            span,
        })
    }

    fn error_non_assoc_chain(&mut self, prev: Self::InfixOp, next: Self::InfixOp) -> Self::Error {
        let mut d = Diagnostic::new(
            ErrorCode::ErrTomlSyntax,
            format!(
                "chained comparison operators `{prev:?}` and `{next:?}` are forbidden; use explicit `&&` or parentheses"
            ),
        )
        .with_board_id(self.board_id);
        d.span = Some(self.peek().span);
        d
    }

    fn error_recursion_limit(&mut self, max_depth: usize) -> Self::Error {
        let mut d = Diagnostic::new(
            ErrorCode::ErrRecursionDepth,
            format!("expression nesting exceeds maximum recursion depth of {max_depth}"),
        )
        .with_board_id(self.board_id);
        d.span = Some(self.peek().span);
        d
    }
}

/// Parses a single unary-precedence operand for C-style casts `(Type)operand`.
fn parse_pratt_unary(parser: &mut Parser<'_>) -> Result<CppExpr, Diagnostic> {
    if let Some(_spec) = parser.peek_prefix() {
        let op = parser.consume_prefix()?;
        let rhs = parse_pratt_unary(parser)?;
        parser.build_prefix(op, rhs)
    } else {
        let primary = parser.parse_primary()?;
        parser.parse_postfix(primary)
    }
}

fn parse_call_args(parser: &mut Parser<'_>) -> Result<(Vec<CppExpr>, Span), Diagnostic> {
    let lparen = parser.bump().span;
    let mut args = Vec::new();
    if !matches!(parser.peek().kind, TokenKind::RParen) {
        loop {
            let arg = parser.parse_expr().ok_or_else(|| {
                let mut d = Diagnostic::new(
                    ErrorCode::ErrTomlSyntax,
                    "expected argument expression in function call",
                )
                .with_board_id(parser.board_id);
                d.span = Some(parser.peek().span);
                d
            })?;
            args.push(arg);
            if matches!(parser.peek().kind, TokenKind::Comma) {
                let _ = parser.bump();
            } else {
                break;
            }
        }
    }
    let rparen = if matches!(parser.peek().kind, TokenKind::RParen) {
        parser.bump().span
    } else {
        let mut d = Diagnostic::new(
            ErrorCode::ErrTomlSyntax,
            "expected `)` closing argument list",
        )
        .with_board_id(parser.board_id);
        d.span = Some(parser.peek().span);
        return Err(d);
    };
    Ok((args, lparen.merge(rparen)))
}

static EOF_TOKEN: Token = Token {
    kind: TokenKind::Eof,
    span: Span { start: 0, end: 0 },
    start_line: 1,
    end_line: 1,
    blank_lines_before: 0,
};
