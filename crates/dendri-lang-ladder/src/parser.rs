//! Recursive-descent parser for canonical IEC 61131-3 Ladder Diagram textual notation
//! (`docs/lbc/ladder.md` §1.1) into `BoardProgramBundle` (`PouBody::Network(LadderNetwork)`).
//!
//! Guarantees:
//! - Zero synthetic `__rail_N` temporaries stored in ASL.
//! - Preserves explicit `rung.order`, `(row, col)` grid coordinates, exact `NodeId`s, nested
//!   `ParallelOr` branches, all contact/coil/box variants, and verbatim comments (`C1–C5`).
//! - Bounded recursion depth for nested `ParallelOr` branches, expressions, and statements.

use crate::common::{
    parse_builtin_fb_kind, parse_compare_op, parse_fb_kind, shift_expr_spans, shift_lvalue_spans,
};
use crate::lexer::LadderScanner;
use dendri_ast::{
    ActionQualifier, AuthoringLanguage, BoardPresentation, BoardProgram, CallArg, CasePattern,
    ChannelRange, Comment, CommentAnchor, CommentOrigin, CommentPlacement as AstCommentPlacement,
    DataDecl, ElifArm, EnumVariant, Expr, FbKind, ForeignFidelity, HardwareBinding, HardwareKind,
    InterruptEdge, LValue, LadderBoxKind, LadderCell, LadderCellKind, LadderNetwork, LadderRung,
    ModuleDecl, NodeId, ObKind, ParamDecl, ParamDirection, Pou, PouBody, PouKind, PullMode,
    ScalarType, SfcAction, SfcChart, SfcStep, SfcTransition, SignalKind, Stmt, StmtKind,
    StorageScope, StructField, SwitchCase, TypeDecl, TypeRef,
};
use dendri_expr::{parse_expr, parse_lvalue};
use dendri_frontend_core::{BoardContext, BoardProgramBundle, RawComment, detect_eol_style};
use dendri_span::{Diagnostic, Span};

const MAX_NESTING_DEPTH: usize = 64;

/// Parses canonical Ladder textual source into a [`BoardProgramBundle`].
pub(crate) fn parse_ladder_source(
    src: &str,
    ctx: &BoardContext,
) -> Result<BoardProgramBundle, Vec<Diagnostic>> {
    let mut parser = LadderParser::new(src, ctx);
    parser.parse_program()
}

struct ParsedPouHeader {
    start: usize,
    id: NodeId,
    name: String,
    kind: PouKind,
    preferred_view: Option<AuthoringLanguage>,
    params: Vec<ParamDecl>,
    locals: Vec<DataDecl>,
}

struct LadderParser<'a> {
    scanner: LadderScanner<'a>,
    ctx: &'a BoardContext,
    comments: Vec<Comment>,
    next_comment_id: u32,
    next_auto_id: u32,
    /// Temporary sink for IEC `AT %IX0.0` hardware bindings declared inside `VAR` blocks.
    inline_hardware: Vec<HardwareBinding>,
}

impl<'a> LadderParser<'a> {
    fn new(src: &'a str, ctx: &'a BoardContext) -> Self {
        Self {
            scanner: LadderScanner::new(src, ctx.board_id.clone()),
            ctx,
            comments: Vec::new(),
            next_comment_id: 1,
            next_auto_id: 1,
            inline_hardware: Vec::new(),
        }
    }

    fn alloc_id(&mut self, prefix: &str) -> NodeId {
        let id = NodeId::new(format!("n_{prefix}_{}", self.next_auto_id));
        self.next_auto_id = self.next_auto_id.saturating_add(1);
        id
    }

    fn push_comments(
        &mut self,
        raws: Vec<RawComment>,
        anchor: CommentAnchor,
        placement: AstCommentPlacement,
    ) {
        for raw in raws {
            let cid = format!("c_ld_{}", self.next_comment_id);
            self.next_comment_id = self.next_comment_id.saturating_add(1);
            self.comments.push(Comment {
                id: cid,
                anchor: anchor.clone(),
                placement,
                token_ordinal: None,
                text: raw.text,
                origin: CommentOrigin {
                    language: "ladder".to_owned(),
                    style: raw.style,
                },
                span: raw.span,
            });
        }
    }

    fn parse_program(&mut self) -> Result<BoardProgramBundle, Vec<Diagnostic>> {
        let eol_style = detect_eol_style(self.scanner.cursor.source());
        let mut program = BoardProgram::default();

        loop {
            if let Err(d) = self.scanner.skip_trivia() {
                return Err(vec![d]);
            }
            if self.scanner.is_eof() {
                let trailing = self.scanner.take_leading_comments();
                if !trailing.is_empty() {
                    let anchor = if let Some(last_pou) = program.pous.last() {
                        CommentAnchor::Node {
                            id: last_pou.id.clone(),
                        }
                    } else {
                        CommentAnchor::Board {
                            id: self.ctx.board_id.clone(),
                        }
                    };
                    let placement = if anchor.is_node() {
                        AstCommentPlacement::Dangling
                    } else {
                        AstCommentPlacement::Body
                    };
                    self.push_comments(trailing, anchor, placement);
                }
                break;
            }

            if self.scanner.peek_keyword("IMPORT") {
                let leading = self.scanner.take_leading_comments();
                if !leading.is_empty() {
                    self.push_comments(
                        leading,
                        CommentAnchor::Board {
                            id: self.ctx.board_id.clone(),
                        },
                        AstCommentPlacement::Header,
                    );
                }
                let imp = self.parse_import().map_err(|d| vec![d])?;
                program.imports.push(imp);
            } else if self.scanner.peek_keyword("TAG_TABLE") {
                let leading = self.scanner.take_leading_comments();
                if !leading.is_empty() {
                    self.push_comments(
                        leading,
                        CommentAnchor::Board {
                            id: self.ctx.board_id.clone(),
                        },
                        AstCommentPlacement::Header,
                    );
                }
                let tags = self.parse_tag_table().map_err(|d| vec![d])?;
                program.hardware.extend(tags);
            } else if self.scanner.peek_keyword("TYPE") {
                let leading = self.scanner.take_leading_comments();
                let mut types = self.parse_type_block(leading).map_err(|d| vec![d])?;
                program.types.append(&mut types);
            } else if self.scanner.peek_keyword("VAR_GLOBAL") {
                let leading = self.scanner.take_leading_comments();
                let mut decls = self.parse_global_var_block(leading).map_err(|d| vec![d])?;
                program.data.append(&mut decls);
            } else if self.scanner.peek_keyword("MODULE") {
                let leading = self.scanner.take_leading_comments();
                let module = self.parse_module(leading).map_err(|d| vec![d])?;
                program.modules.push(module);
            } else if self.scanner.peek_keyword("NETWORK") {
                let leading = self.scanner.take_leading_comments();
                let pou = self.parse_network_pou(leading).map_err(|d| vec![d])?;
                program.pous.push(pou);
            } else if self.scanner.peek_keyword("STRUCTURED_POU") {
                let leading = self.scanner.take_leading_comments();
                let pou = self.parse_structured_pou(leading).map_err(|d| vec![d])?;
                program.pous.push(pou);
            } else if self.scanner.peek_keyword("CHART_POU") {
                let leading = self.scanner.take_leading_comments();
                let pou = self.parse_chart_pou(leading).map_err(|d| vec![d])?;
                program.pous.push(pou);
            } else {
                let err = if let Some((tok, sp)) = self.scanner.eat_ident() {
                    self.scanner.unknown_field_error(sp, tok)
                } else {
                    self.scanner.syntax_error(
                        "expected `IMPORT`, `TAG_TABLE`, `TYPE`, `VAR_GLOBAL`, `MODULE`, `NETWORK`, `STRUCTURED_POU`, or `CHART_POU`",
                    )
                };
                return Err(vec![err]);
            }
        }

        if !self.inline_hardware.is_empty() {
            program.hardware.append(&mut self.inline_hardware);
        }

        Ok(BoardProgramBundle {
            program,
            comments: std::mem::take(&mut self.comments),
            assumptions: Vec::new(),
            presentation: BoardPresentation {
                eol_style,
                ..BoardPresentation::default()
            },
        })
    }

    fn parse_import(&mut self) -> Result<String, Diagnostic> {
        let _ = self.scanner.expect_keyword("IMPORT")?;
        let (path, _) = self.scanner.expect_import_path()?;
        let _ = self.scanner.expect_punct(";")?;
        Ok(path)
    }

    fn parse_tag_table(&mut self) -> Result<Vec<HardwareBinding>, Diagnostic> {
        let _ = self.scanner.expect_keyword("TAG_TABLE")?;
        let mut bindings = Vec::new();
        loop {
            self.scanner.skip_trivia()?;
            if self.scanner.eat_keyword("END_TAG_TABLE").is_some() {
                break;
            }
            if self.scanner.is_eof() {
                return Err(self
                    .scanner
                    .syntax_error("unterminated `TAG_TABLE` (expected `END_TAG_TABLE`)"));
            }
            let leading = self.scanner.take_leading_comments();
            let start = self.scanner.pos();
            let (label, _) = self.scanner.expect_ident("hardware tag label")?;
            let id = self.parse_optional_paren_node_id("hw")?;
            let _ = self.scanner.expect_keyword("AT")?;
            let (raw_pin, _) = self.scanner.expect_port_or_iec()?;
            let pin = if raw_pin.starts_with('%') {
                self.ctx
                    .port_for_iec(&raw_pin)
                    .map_or(raw_pin, str::to_owned)
            } else {
                raw_pin
            };
            let _ = self.scanner.expect_punct(":")?;
            let (kind_kw, kind_span) = self.scanner.expect_ident("hardware kind")?;
            let kind = parse_hardware_kind(&kind_kw).ok_or_else(|| {
                self.scanner
                    .syntax_error_at(kind_span, format!("unknown hardware tag kind `{kind_kw}`"))
            })?;

            let mut pull = PullMode::None;
            let mut active_low = false;
            let mut codec: Option<String> = None;
            let mut range: Option<ChannelRange> = None;

            loop {
                self.scanner.skip_trivia()?;
                if self.scanner.eat_punct(";") {
                    break;
                }
                if self.scanner.eat_keyword("PULL_NONE").is_some() {
                    pull = PullMode::None;
                } else if self.scanner.eat_keyword("PULL_UP").is_some() {
                    pull = PullMode::PullUp;
                } else if self.scanner.eat_keyword("PULL_DOWN").is_some() {
                    pull = PullMode::PullDown;
                } else if self.scanner.eat_keyword("ACTIVE_LOW").is_some() {
                    active_low = true;
                } else if self.scanner.eat_keyword("CODEC").is_some() {
                    let (codec_id, _) = self.scanner.expect_node_id()?;
                    codec = Some(codec_id);
                } else if self.scanner.eat_keyword("RANGE").is_some() {
                    let (sig_kw, sig_sp) = self.scanner.expect_ident("signal kind")?;
                    let signal = if sig_kw.eq_ignore_ascii_case("VOLTAGE") {
                        SignalKind::Voltage
                    } else if sig_kw.eq_ignore_ascii_case("CURRENT") {
                        SignalKind::Current
                    } else {
                        return Err(self.scanner.syntax_error_at(
                            sig_sp,
                            format!("expected `VOLTAGE` or `CURRENT`, found `{sig_kw}`"),
                        ));
                    };
                    let (low, _) = self.scanner.expect_f64("range low")?;
                    let _ = self.scanner.expect_punct("..")?;
                    let (high, _) = self.scanner.expect_f64("range high")?;
                    let mut live_zero = false;
                    let mut hardware_normalizes_live_zero = false;
                    loop {
                        self.scanner.skip_trivia()?;
                        if self.scanner.eat_keyword("LIVE_ZERO").is_some() {
                            live_zero = true;
                        } else if self.scanner.eat_keyword("HW_NORM").is_some() {
                            hardware_normalizes_live_zero = true;
                        } else {
                            break;
                        }
                    }
                    range = Some(ChannelRange {
                        signal,
                        low,
                        high,
                        live_zero,
                        hardware_normalizes_live_zero,
                    });
                } else if let Some((unknown, sp)) = self.scanner.eat_ident() {
                    return Err(self.scanner.unknown_field_error(sp, unknown));
                } else {
                    return Err(self
                        .scanner
                        .syntax_error("expected `;` or hardware tag attribute"));
                }
            }

            let span = self.scanner.span_from(start);
            let trailing = self.scanner.take_same_line_trailing()?;
            self.push_comments(
                leading,
                CommentAnchor::Node { id: id.clone() },
                AstCommentPlacement::Leading,
            );
            self.push_comments(
                trailing,
                CommentAnchor::Node { id: id.clone() },
                AstCommentPlacement::Trailing,
            );
            bindings.push(HardwareBinding {
                id,
                label,
                pin,
                kind,
                pull,
                active_low,
                codec,
                range,
                span,
            });
        }
        Ok(bindings)
    }

    fn parse_type_block(
        &mut self,
        block_leading: Vec<RawComment>,
    ) -> Result<Vec<TypeDecl>, Diagnostic> {
        let _ = self.scanner.expect_keyword("TYPE")?;
        let mut pending_block = block_leading;
        let mut decls = Vec::new();
        loop {
            self.scanner.skip_trivia()?;
            if self.scanner.eat_keyword("END_TYPE").is_some() {
                break;
            }
            if self.scanner.is_eof() {
                return Err(self
                    .scanner
                    .syntax_error("unterminated `TYPE` block (expected `END_TYPE`)"));
            }
            let mut leading = std::mem::take(&mut pending_block);
            leading.extend(self.scanner.take_leading_comments());
            let start = self.scanner.pos();

            if self.scanner.eat_keyword("STRUCT").is_some() {
                let (name, _) = self.scanner.expect_ident("struct name")?;
                let id = self.parse_optional_paren_node_id("type")?;
                let mut fields = Vec::new();
                loop {
                    self.scanner.skip_trivia()?;
                    if self.scanner.eat_keyword("END_STRUCT").is_some() {
                        break;
                    }
                    let (fname, _) = self.scanner.expect_ident("struct field name")?;
                    let _ = self.scanner.expect_punct(":")?;
                    let ty = self.parse_type_ref()?;
                    let _ = self.scanner.expect_punct(";")?;
                    fields.push(StructField { name: fname, ty });
                }
                let _ = self.scanner.eat_punct(";");
                let span = self.scanner.span_from(start);
                self.push_comments(
                    leading,
                    CommentAnchor::Node { id: id.clone() },
                    AstCommentPlacement::Leading,
                );
                decls.push(TypeDecl::Struct {
                    id,
                    name,
                    fields,
                    span,
                });
            } else if self.scanner.eat_keyword("ENUM").is_some() {
                let (name, _) = self.scanner.expect_ident("enum name")?;
                let id = self.parse_optional_paren_node_id("type")?;
                let _ = self.scanner.expect_punct(":")?;
                let backing_ref = self.parse_type_ref()?;
                let TypeRef::Scalar { ty: backing } = backing_ref else {
                    return Err(self
                        .scanner
                        .syntax_error("enum backing type must be a scalar type"));
                };
                let _ = self.scanner.expect_punct("(")?;
                let mut variants = Vec::new();
                loop {
                    self.scanner.skip_trivia()?;
                    if self.scanner.eat_punct(")") {
                        break;
                    }
                    let (vname, _) = self.scanner.expect_ident("enum variant name")?;
                    let _ = self.scanner.expect_punct("=")?;
                    let (val, _) = self.scanner.expect_i64("enum variant value")?;
                    variants.push(EnumVariant {
                        name: vname,
                        value: val,
                    });
                    self.scanner.skip_trivia()?;
                    let _ = self.scanner.eat_punct(",");
                }
                let _ = self.scanner.expect_keyword("END_ENUM")?;
                let _ = self.scanner.eat_punct(";");
                let span = self.scanner.span_from(start);
                self.push_comments(
                    leading,
                    CommentAnchor::Node { id: id.clone() },
                    AstCommentPlacement::Leading,
                );
                decls.push(TypeDecl::Enum {
                    id,
                    name,
                    backing,
                    variants,
                    span,
                });
            } else if self.scanner.eat_keyword("FOREIGN_TYPE").is_some() {
                self.scanner.skip_trivia()?;
                let name = if self.scanner.peek() == Some('(') {
                    None
                } else {
                    let (n, _) = self.scanner.expect_ident("foreign type name")?;
                    Some(n)
                };
                self.scanner.skip_trivia()?;
                let id = if self.scanner.starts_with("(") && !self.looks_like_foreign_args() {
                    self.parse_optional_paren_node_id("type")?
                } else {
                    self.alloc_id("type")
                };
                let (language, fidelity, text, pack_hint) = self.parse_foreign_payload()?;
                let _ = self.scanner.expect_punct(";")?;
                let span = self.scanner.span_from(start);
                self.push_comments(
                    leading,
                    CommentAnchor::Node { id: id.clone() },
                    AstCommentPlacement::Leading,
                );
                decls.push(TypeDecl::Foreign {
                    id,
                    name,
                    language,
                    fidelity,
                    text,
                    pack_hint,
                    span,
                });
            } else if let Some((tok, sp)) = self.scanner.eat_ident() {
                return Err(self.scanner.unknown_field_error(sp, tok));
            } else {
                return Err(self
                    .scanner
                    .syntax_error("expected `STRUCT`, `ENUM`, or `FOREIGN_TYPE`"));
            }
        }
        Ok(decls)
    }

    fn looks_like_foreign_args(&mut self) -> bool {
        let cp = self.scanner.cursor.checkpoint();
        let _ = self.scanner.cursor.eat_char('(');
        let _ = self.scanner.cursor.skip_whitespace_and_count_blank_lines();
        let starts_with_quote = self.scanner.peek() == Some('"');
        self.scanner.cursor.restore(cp);
        starts_with_quote
    }

    fn parse_global_var_block(
        &mut self,
        block_leading: Vec<RawComment>,
    ) -> Result<Vec<DataDecl>, Diagnostic> {
        let _ = self.scanner.expect_keyword("VAR_GLOBAL")?;
        let scope = self.parse_optional_storage_scope(StorageScope::Var)?;
        self.parse_data_decl_entries(scope, block_leading)
    }

    fn parse_optional_storage_scope(
        &mut self,
        default_scope: StorageScope,
    ) -> Result<StorageScope, Diagnostic> {
        self.scanner.skip_trivia()?;
        if self.scanner.eat_keyword("CONSTANT").is_some() {
            Ok(StorageScope::Const)
        } else if self.scanner.eat_keyword("RETAIN").is_some() {
            Ok(StorageScope::Retain)
        } else if self.scanner.eat_keyword("PERSISTENT").is_some() {
            Ok(StorageScope::Persistent)
        } else if self.scanner.eat_keyword("TEMP").is_some() {
            Ok(StorageScope::Temp)
        } else {
            Ok(default_scope)
        }
    }

    fn parse_data_decl_entries(
        &mut self,
        scope: StorageScope,
        mut block_leading: Vec<RawComment>,
    ) -> Result<Vec<DataDecl>, Diagnostic> {
        let mut decls = Vec::new();
        loop {
            self.scanner.skip_trivia()?;
            if self.scanner.eat_keyword("END_VAR").is_some() {
                break;
            }
            if self.scanner.is_eof() {
                return Err(self
                    .scanner
                    .syntax_error("unterminated `VAR` block (expected `END_VAR`)"));
            }
            let mut leading = std::mem::take(&mut block_leading);
            leading.extend(self.scanner.take_leading_comments());
            let start = self.scanner.pos();
            let (name, _) = self.scanner.expect_ident("variable name")?;
            let id = self.parse_optional_paren_node_id("data")?;
            self.scanner.skip_trivia()?;

            // Direct IEC hardware address inside VAR block: `StartBtn AT %IX0.0 : BOOL;`
            if self.scanner.eat_keyword("AT").is_some() {
                let (raw_pin, _) = self.scanner.expect_port_or_iec()?;
                let _ = self.scanner.expect_punct(":")?;
                let _ty = self.parse_type_ref()?;
                let _ = self.scanner.expect_punct(";")?;
                let span = self.scanner.span_from(start);
                let kind = infer_hardware_kind_from_iec(&raw_pin);
                let pin = if raw_pin.starts_with('%') {
                    self.ctx
                        .port_for_iec(&raw_pin)
                        .map_or(raw_pin, str::to_owned)
                } else {
                    raw_pin
                };
                let trailing = self.scanner.take_same_line_trailing()?;
                self.push_comments(
                    leading,
                    CommentAnchor::Node { id: id.clone() },
                    AstCommentPlacement::Leading,
                );
                self.push_comments(
                    trailing,
                    CommentAnchor::Node { id: id.clone() },
                    AstCommentPlacement::Trailing,
                );
                self.inline_hardware.push(HardwareBinding {
                    id,
                    label: name,
                    pin,
                    kind,
                    pull: PullMode::None,
                    active_low: false,
                    codec: None,
                    range: None,
                    span,
                });
                continue;
            }

            let _ = self.scanner.expect_punct(":")?;
            let ty = self.parse_type_ref()?;
            self.scanner.skip_trivia()?;
            let initial = if self.scanner.eat_punct(":=") {
                Some(self.parse_asl_expr(&[';'], &[], false)?)
            } else {
                None
            };
            let _ = self.scanner.expect_punct(";")?;
            let span = self.scanner.span_from(start);
            let trailing = self.scanner.take_same_line_trailing()?;
            self.push_comments(
                leading,
                CommentAnchor::Node { id: id.clone() },
                AstCommentPlacement::Leading,
            );
            self.push_comments(
                trailing,
                CommentAnchor::Node { id: id.clone() },
                AstCommentPlacement::Trailing,
            );
            decls.push(DataDecl {
                id,
                name,
                scope,
                ty,
                initial,
                span,
            });
        }
        Ok(decls)
    }

    fn parse_param_block(
        &mut self,
        direction: ParamDirection,
    ) -> Result<Vec<ParamDecl>, Diagnostic> {
        let mut params = Vec::new();
        loop {
            self.scanner.skip_trivia()?;
            if self.scanner.eat_keyword("END_VAR").is_some() {
                break;
            }
            if self.scanner.is_eof() {
                return Err(self
                    .scanner
                    .syntax_error("unterminated parameter `VAR_*` block (expected `END_VAR`)"));
            }
            let (name, _) = self.scanner.expect_ident("parameter name")?;
            let _ = self.scanner.expect_punct(":")?;
            let ty = self.parse_type_ref()?;
            let _ = self.scanner.expect_punct(";")?;
            params.push(ParamDecl {
                name,
                direction,
                ty,
            });
        }
        Ok(params)
    }

    fn parse_module(&mut self, leading: Vec<RawComment>) -> Result<ModuleDecl, Diagnostic> {
        let start = self.scanner.pos();
        let _ = self.scanner.expect_keyword("MODULE")?;
        let (name, _) = self.scanner.expect_ident("module name")?;
        let id = self.parse_optional_paren_node_id("mod")?;
        self.push_comments(
            leading,
            CommentAnchor::Node { id: id.clone() },
            AstCommentPlacement::Header,
        );

        let mut imports = Vec::new();
        let mut exports = Vec::new();
        let mut types = Vec::new();
        let mut data = Vec::new();
        let mut pous = Vec::new();

        loop {
            self.scanner.skip_trivia()?;
            if self.scanner.eat_keyword("END_MODULE").is_some() {
                break;
            }
            if self.scanner.is_eof() {
                return Err(self
                    .scanner
                    .syntax_error("unterminated `MODULE` (expected `END_MODULE`)"));
            }
            if self.scanner.peek_keyword("IMPORT") {
                imports.push(self.parse_import()?);
            } else if self.scanner.eat_keyword("EXPORT").is_some() {
                let (exp, _) = self.scanner.expect_ident("exported symbol")?;
                let _ = self.scanner.expect_punct(";")?;
                exports.push(exp);
            } else if self.scanner.peek_keyword("TYPE") {
                let c = self.scanner.take_leading_comments();
                types.extend(self.parse_type_block(c)?);
            } else if self.scanner.peek_keyword("VAR_GLOBAL") {
                let c = self.scanner.take_leading_comments();
                data.extend(self.parse_global_var_block(c)?);
            } else if self.scanner.peek_keyword("NETWORK") {
                let c = self.scanner.take_leading_comments();
                pous.push(self.parse_network_pou(c)?);
            } else if self.scanner.peek_keyword("STRUCTURED_POU") {
                let c = self.scanner.take_leading_comments();
                pous.push(self.parse_structured_pou(c)?);
            } else if self.scanner.peek_keyword("CHART_POU") {
                let c = self.scanner.take_leading_comments();
                pous.push(self.parse_chart_pou(c)?);
            } else if let Some((tok, sp)) = self.scanner.eat_ident() {
                return Err(self.scanner.unknown_field_error(sp, tok));
            } else {
                return Err(self
                    .scanner
                    .syntax_error("unexpected token inside `MODULE`"));
            }
        }

        let span = self.scanner.span_from(start);
        Ok(ModuleDecl {
            id,
            name,
            imports,
            exports,
            types,
            data,
            pous,
            span,
        })
    }

    fn parse_pou_header(
        &mut self,
        keyword: &str,
        leading: Vec<RawComment>,
    ) -> Result<ParsedPouHeader, Diagnostic> {
        let start = self.scanner.pos();
        let _ = self.scanner.expect_keyword(keyword)?;
        let (name, _) = self.scanner.expect_ident("POU name")?;
        let id = self.parse_optional_paren_node_id("pou")?;

        self.scanner.skip_trivia()?;
        let kind = if self.scanner.eat_keyword("KIND").is_some() {
            self.parse_pou_kind()?
        } else if name.eq_ignore_ascii_case("OB_STARTUP") {
            PouKind::Ob {
                ob: ObKind::Startup,
            }
        } else {
            PouKind::Ob { ob: ObKind::Main }
        };

        self.scanner.skip_trivia()?;
        let preferred_view = if self.scanner.eat_keyword("VIEW").is_some() {
            let (v, sp) = self.scanner.expect_ident("authoring language")?;
            let lang = parse_authoring_language(&v).ok_or_else(|| {
                self.scanner
                    .syntax_error_at(sp, format!("unknown authoring view `{v}`"))
            })?;
            Some(lang)
        } else {
            None
        };

        let header_trailing = self.scanner.take_same_line_trailing()?;
        self.push_comments(
            leading,
            CommentAnchor::Node { id: id.clone() },
            AstCommentPlacement::Header,
        );
        self.push_comments(
            header_trailing,
            CommentAnchor::Node { id: id.clone() },
            AstCommentPlacement::Trailing,
        );

        let mut params = Vec::new();
        let mut locals = Vec::new();
        loop {
            self.scanner.skip_trivia()?;
            if self.scanner.eat_keyword("VAR_INPUT").is_some() {
                params.extend(self.parse_param_block(ParamDirection::In)?);
            } else if self.scanner.eat_keyword("VAR_OUTPUT").is_some() {
                params.extend(self.parse_param_block(ParamDirection::Out)?);
            } else if self.scanner.eat_keyword("VAR_IN_OUT").is_some() {
                params.extend(self.parse_param_block(ParamDirection::InOut)?);
            } else if self.scanner.eat_keyword("VAR_TEMP").is_some() {
                let c = self.scanner.take_leading_comments();
                locals.extend(self.parse_data_decl_entries(StorageScope::Temp, c)?);
            } else if self.scanner.eat_keyword("VAR").is_some() {
                let c = self.scanner.take_leading_comments();
                let scope = self.parse_optional_storage_scope(StorageScope::Var)?;
                locals.extend(self.parse_data_decl_entries(scope, c)?);
            } else {
                break;
            }
        }

        Ok(ParsedPouHeader {
            start,
            id,
            name,
            kind,
            preferred_view,
            params,
            locals,
        })
    }

    fn parse_network_pou(&mut self, leading: Vec<RawComment>) -> Result<Pou, Diagnostic> {
        let ParsedPouHeader {
            start,
            id,
            name,
            kind,
            preferred_view,
            params,
            locals,
        } = self.parse_pou_header("NETWORK", leading)?;

        let mut rungs = Vec::new();
        loop {
            self.scanner.skip_trivia()?;
            if self.scanner.eat_keyword("END_NETWORK").is_some() {
                let dangling = self.scanner.take_leading_comments();
                self.push_comments(
                    dangling,
                    CommentAnchor::Node { id: id.clone() },
                    AstCommentPlacement::Dangling,
                );
                break;
            }
            if self.scanner.is_eof() {
                return Err(self
                    .scanner
                    .syntax_error("unterminated `NETWORK` (expected `END_NETWORK`)"));
            }
            if self.scanner.peek_keyword("RUNG") {
                let rung_comments = self.scanner.take_leading_comments();
                let rung = self.parse_rung(rung_comments)?;
                rungs.push(rung);
            } else if let Some((tok, sp)) = self.scanner.eat_ident() {
                return Err(self.scanner.unknown_field_error(sp, tok));
            } else {
                return Err(self
                    .scanner
                    .syntax_error("expected `RUNG` or `END_NETWORK` inside `NETWORK`"));
            }
        }

        let span = self.scanner.span_from(start);
        Ok(Pou {
            id,
            name,
            kind,
            preferred_view,
            params,
            locals,
            body: PouBody::Network(LadderNetwork { rungs }),
            span,
        })
    }

    fn parse_rung(&mut self, leading: Vec<RawComment>) -> Result<LadderRung, Diagnostic> {
        let _ = self.scanner.expect_keyword("RUNG")?;
        let (order, _) = self.scanner.expect_u32("rung order")?;
        let _ = self.scanner.expect_punct("(")?;
        let (raw_id, _) = self.scanner.expect_node_id()?;
        let id = NodeId::new(raw_id);
        let _ = self.scanner.expect_punct(")")?;
        let _ = self.scanner.expect_punct(":")?;

        // First leading comment on a rung maps to Title; subsequent comments map to Body (`04` §5.1, LBC §4).
        for (idx, raw) in leading.into_iter().enumerate() {
            let placement = if idx == 0 {
                AstCommentPlacement::Title
            } else {
                AstCommentPlacement::Body
            };
            self.push_comments(vec![raw], CommentAnchor::Node { id: id.clone() }, placement);
        }

        self.scanner.skip_trivia()?;
        let cells = if self.scanner.eat_punct(";") {
            let empty_rung_notes = self.scanner.take_leading_comments();
            self.push_comments(
                empty_rung_notes,
                CommentAnchor::Node { id: id.clone() },
                AstCommentPlacement::Body,
            );
            Vec::new()
        } else {
            let list = self.parse_cell_series(0)?;
            let _ = self.scanner.expect_punct(";")?;
            list
        };
        let rung_trailing = self.scanner.take_same_line_trailing()?;
        self.push_comments(
            rung_trailing,
            CommentAnchor::Node { id: id.clone() },
            AstCommentPlacement::Trailing,
        );

        Ok(LadderRung { id, order, cells })
    }

    fn parse_cell_series(&mut self, depth: usize) -> Result<Vec<LadderCell>, Diagnostic> {
        if depth > MAX_NESTING_DEPTH {
            return Err(self
                .scanner
                .syntax_error("maximum Ladder branch nesting depth exceeded"));
        }
        let mut cells = Vec::new();
        loop {
            self.scanner.skip_trivia()?;
            let cell = self.parse_cell(depth)?;
            cells.push(cell);
            self.scanner.skip_trivia()?;
            if self.scanner.eat_punct("--") {
                continue;
            }
            break;
        }
        Ok(cells)
    }

    fn parse_cell(&mut self, depth: usize) -> Result<LadderCell, Diagnostic> {
        self.scanner.skip_trivia()?;
        let leading = self.scanner.take_leading_comments();

        let (kind, row, col, id) = if self.scanner.eat_keyword("EMPTY").is_some() {
            let (row, col, id) = self.parse_grid_pos("cell")?;
            (LadderCellKind::Empty, row, col, id)
        } else if self.scanner.eat_keyword("WIRE").is_some() {
            let (row, col, id) = self.parse_grid_pos("cell")?;
            (LadderCellKind::Wire, row, col, id)
        } else if let Some((contact_kind, _)) = self.scanner.try_eat_contact() {
            let target = self.parse_asl_lvalue()?;
            let (row, col, id) = self.parse_grid_pos("cell")?;
            (
                LadderCellKind::Contact {
                    kind: contact_kind,
                    target,
                },
                row,
                col,
                id,
            )
        } else if let Some((coil_kind, _)) = self.scanner.try_eat_coil() {
            let target = self.parse_asl_lvalue()?;
            let (row, col, id) = self.parse_grid_pos("cell")?;
            (
                LadderCellKind::Coil {
                    kind: coil_kind,
                    target,
                },
                row,
                col,
                id,
            )
        } else if self.scanner.eat_keyword("BRANCH").is_some() {
            let (row, col, id) = self.parse_grid_pos("branch")?;
            let _ = self.scanner.expect_punct("{")?;
            let mut branches = Vec::new();
            self.scanner.skip_trivia()?;
            if !self.scanner.eat_punct("}") {
                loop {
                    self.scanner.skip_trivia()?;
                    let branch =
                        if self.scanner.starts_with("||") || self.scanner.peek() == Some('}') {
                            Vec::new()
                        } else {
                            self.parse_cell_series(depth.saturating_add(1))?
                        };
                    branches.push(branch);
                    self.scanner.skip_trivia()?;
                    if self.scanner.eat_punct("||") {
                        continue;
                    }
                    let _ = self.scanner.expect_punct("}")?;
                    break;
                }
            }
            (LadderCellKind::ParallelOr { branches }, row, col, id)
        } else if self.scanner.eat_keyword("FB").is_some() {
            let (kind_raw, _) = self.scanner.expect_ident("FB kind")?;
            let fb_kind = parse_fb_kind(&kind_raw);
            let instance = self.parse_asl_lvalue()?;
            let _ = self.scanner.expect_punct("(")?;
            let inputs = self.parse_named_arg_list()?;
            let (row, col, id) = self.parse_grid_pos("fb")?;
            (
                LadderCellKind::Box(LadderBoxKind::Fb {
                    kind: fb_kind,
                    instance,
                    inputs,
                }),
                row,
                col,
                id,
            )
        } else if self.scanner.eat_keyword("CMP").is_some() {
            let (op_raw, op_sp) = self.scanner.expect_ident("compare operator")?;
            let op = parse_compare_op(&op_raw).ok_or_else(|| {
                self.scanner.syntax_error_at(
                    op_sp,
                    format!(
                        "invalid Ladder compare operator `{op_raw}` (expected EQ, NE, LT, LE, GT, or GE)"
                    ),
                )
            })?;
            let _ = self.scanner.expect_punct("(")?;
            let left = self.parse_asl_expr(&[','], &[], false)?;
            let _ = self.scanner.expect_punct(",")?;
            let right = self.parse_asl_expr(&[')'], &[], false)?;
            let _ = self.scanner.expect_punct(")")?;
            let (row, col, id) = self.parse_grid_pos("cmp")?;
            (
                LadderCellKind::Box(LadderBoxKind::Compare { op, left, right }),
                row,
                col,
                id,
            )
        } else if self.scanner.eat_keyword("CALL").is_some() {
            let (callee, _) = self.scanner.expect_ident("callee name")?;
            let _ = self.scanner.expect_punct("(")?;
            let args = self.parse_mixed_arg_list()?;
            self.scanner.skip_trivia()?;
            let output = if self.scanner.eat_punct("=>") {
                Some(self.parse_asl_lvalue()?)
            } else {
                None
            };
            let (row, col, id) = self.parse_grid_pos("call")?;
            (
                LadderCellKind::Box(LadderBoxKind::Call {
                    callee,
                    args,
                    output,
                }),
                row,
                col,
                id,
            )
        } else if self.scanner.eat_keyword("FOREIGN").is_some() {
            let (language, fidelity, text, pack_hint) = self.parse_foreign_payload()?;
            let (row, col, id) = self.parse_grid_pos("foreign")?;
            (
                LadderCellKind::Box(LadderBoxKind::Foreign {
                    language,
                    fidelity,
                    text,
                    pack_hint,
                }),
                row,
                col,
                id,
            )
        } else {
            return Err(self.scanner.syntax_error(
                "expected Ladder cell (`EMPTY`, `WIRE`, contact `| |`/`|/`/`|P|`/`|N|`, coil `( )`/`(/)`/`(S)`/`(R)`/`(P)`/`(N)`, `BRANCH`, `FB`, `CMP`, `CALL`, or `FOREIGN`)",
            ));
        };

        let trailing = self.scanner.take_same_line_trailing()?;
        self.push_comments(
            leading,
            CommentAnchor::Node { id: id.clone() },
            AstCommentPlacement::Leading,
        );
        self.push_comments(
            trailing,
            CommentAnchor::Node { id: id.clone() },
            AstCommentPlacement::Trailing,
        );

        Ok(LadderCell { id, row, col, kind })
    }

    fn parse_grid_pos(&mut self, prefix: &str) -> Result<(u32, u32, NodeId), Diagnostic> {
        let _ = self.scanner.expect_punct("@")?;
        let _ = self.scanner.expect_punct("(")?;
        let (row, _) = self.scanner.expect_u32("grid row")?;
        let _ = self.scanner.expect_punct(",")?;
        let (col, _) = self.scanner.expect_u32("grid col")?;
        self.scanner.skip_trivia()?;
        let id = if self.scanner.eat_punct(",") {
            let (raw_id, _) = self.scanner.expect_node_id()?;
            NodeId::new(raw_id)
        } else {
            self.alloc_id(prefix)
        };
        let _ = self.scanner.expect_punct(")")?;
        Ok((row, col, id))
    }

    fn parse_optional_paren_node_id(&mut self, prefix: &str) -> Result<NodeId, Diagnostic> {
        self.scanner.skip_trivia()?;
        if self.scanner.eat_punct("(") {
            let (raw_id, _) = self.scanner.expect_node_id()?;
            let _ = self.scanner.expect_punct(")")?;
            Ok(NodeId::new(raw_id))
        } else {
            Ok(self.alloc_id(prefix))
        }
    }

    fn parse_named_arg_list(&mut self) -> Result<Vec<(String, Expr)>, Diagnostic> {
        let mut args = Vec::new();
        self.scanner.skip_trivia()?;
        if self.scanner.eat_punct(")") {
            return Ok(args);
        }
        loop {
            let (name, _) = self.scanner.expect_ident("FB input pin name")?;
            let _ = self.scanner.expect_punct(":=")?;
            let expr = self.parse_asl_expr(&[',', ')'], &[], false)?;
            args.push((name, expr));
            self.scanner.skip_trivia()?;
            if self.scanner.eat_punct(",") {
                continue;
            }
            let _ = self.scanner.expect_punct(")")?;
            break;
        }
        Ok(args)
    }

    fn parse_mixed_arg_list(&mut self) -> Result<Vec<(Option<String>, Expr)>, Diagnostic> {
        let mut args = Vec::new();
        self.scanner.skip_trivia()?;
        if self.scanner.eat_punct(")") {
            return Ok(args);
        }
        loop {
            self.scanner.skip_trivia()?;
            let cp = self.scanner.cursor.checkpoint();
            let named_opt = if let Some((name, _)) = self.scanner.eat_ident() {
                let _ = self.scanner.cursor.skip_whitespace_and_count_blank_lines();
                if self.scanner.eat_punct(":=") {
                    Some(name.to_owned())
                } else {
                    self.scanner.cursor.restore(cp);
                    None
                }
            } else {
                None
            };
            let expr = self.parse_asl_expr(&[',', ')'], &[], false)?;
            args.push((named_opt, expr));
            self.scanner.skip_trivia()?;
            if self.scanner.eat_punct(",") {
                continue;
            }
            let _ = self.scanner.expect_punct(")")?;
            break;
        }
        Ok(args)
    }

    fn parse_foreign_payload(
        &mut self,
    ) -> Result<(String, ForeignFidelity, String, Option<String>), Diagnostic> {
        let _ = self.scanner.expect_punct("(")?;
        let (language, _) = self.scanner.expect_string_lit()?;
        let _ = self.scanner.expect_punct(",")?;
        let (fid_raw, fid_sp) = self
            .scanner
            .expect_ident("Foreign fidelity (`F0`|`F1`|`F2`)")?;
        let fidelity = if fid_raw.eq_ignore_ascii_case("F0") {
            ForeignFidelity::F0
        } else if fid_raw.eq_ignore_ascii_case("F1") {
            ForeignFidelity::F1
        } else if fid_raw.eq_ignore_ascii_case("F2") {
            ForeignFidelity::F2
        } else {
            return Err(self.scanner.syntax_error_at(
                fid_sp,
                format!("expected `F0`, `F1`, or `F2`, found `{fid_raw}`"),
            ));
        };
        let _ = self.scanner.expect_punct(",")?;
        let (text, _) = self.scanner.expect_string_lit()?;
        self.scanner.skip_trivia()?;
        let pack_hint = if self.scanner.eat_punct(",") {
            let (hint, _) = self.scanner.expect_string_lit()?;
            Some(hint)
        } else {
            None
        };
        let _ = self.scanner.expect_punct(")")?;
        Ok((language, fidelity, text, pack_hint))
    }

    fn parse_pou_kind(&mut self) -> Result<PouKind, Diagnostic> {
        let (kw, sp) = self.scanner.expect_ident("POU kind")?;
        if kw.eq_ignore_ascii_case("OB_MAIN") {
            Ok(PouKind::Ob { ob: ObKind::Main })
        } else if kw.eq_ignore_ascii_case("OB_STARTUP") {
            Ok(PouKind::Ob {
                ob: ObKind::Startup,
            })
        } else if kw.eq_ignore_ascii_case("OB_CYCLIC_INT") {
            let _ = self.scanner.expect_punct("(")?;
            let (period_us, _) = self.scanner.expect_u64("cyclic interrupt period_us")?;
            let _ = self.scanner.expect_punct(")")?;
            Ok(PouKind::Ob {
                ob: ObKind::CyclicInt { period_us },
            })
        } else if kw.eq_ignore_ascii_case("OB_HW_INT") {
            let _ = self.scanner.expect_punct("(")?;
            let (pin_label, _) = self.scanner.expect_ident("interrupt pin_label")?;
            let _ = self.scanner.expect_punct(",")?;
            let (edge_kw, edge_sp) = self.scanner.expect_ident("interrupt edge")?;
            let edge = parse_interrupt_edge(&edge_kw).ok_or_else(|| {
                self.scanner
                    .syntax_error_at(edge_sp, format!("unknown interrupt edge `{edge_kw}`"))
            })?;
            let _ = self.scanner.expect_punct(",")?;
            let (prio_u64, prio_sp) = self.scanner.expect_u64("interrupt priority")?;
            let priority = u8::try_from(prio_u64).map_err(|_| {
                self.scanner.syntax_error_at(
                    prio_sp,
                    format!("interrupt priority `{prio_u64}` out of u8 range"),
                )
            })?;
            let _ = self.scanner.expect_punct(")")?;
            Ok(PouKind::Ob {
                ob: ObKind::HwInt {
                    pin_label,
                    edge,
                    priority,
                },
            })
        } else if kw.eq_ignore_ascii_case("OB_COMM_EVENT") {
            let _ = self.scanner.expect_punct("(")?;
            let (bus_id, _) = self.scanner.expect_node_id()?;
            let _ = self.scanner.expect_punct(")")?;
            Ok(PouKind::Ob {
                ob: ObKind::CommEvent { bus_id },
            })
        } else if kw.eq_ignore_ascii_case("FC") {
            let _ = self.scanner.expect_punct(":")?;
            let return_type = self.parse_type_ref()?;
            Ok(PouKind::Fc { return_type })
        } else if kw.eq_ignore_ascii_case("FB") {
            Ok(PouKind::Fb)
        } else {
            Err(self
                .scanner
                .syntax_error_at(sp, format!("unknown POU kind `{kw}`")))
        }
    }

    fn parse_type_ref(&mut self) -> Result<TypeRef, Diagnostic> {
        self.scanner.skip_trivia()?;
        let (kw, sp) = self.scanner.expect_ident("type name")?;
        if kw.eq_ignore_ascii_case("VOID") {
            return Ok(TypeRef::Void);
        }
        if kw.eq_ignore_ascii_case("ARRAY") {
            let _ = self.scanner.expect_punct("[")?;
            let (len, _) = self.scanner.expect_u32("array length")?;
            let _ = self.scanner.expect_punct("]")?;
            let _ = self.scanner.expect_keyword("OF")?;
            let elem = self.parse_type_ref()?;
            return Ok(TypeRef::Array {
                elem: Box::new(elem),
                len,
            });
        }
        if kw.eq_ignore_ascii_case("FB") {
            let (fb_name, _) = self.scanner.expect_ident("user FB type name")?;
            return Ok(TypeRef::Fb {
                fb_kind: FbKind::User { fb_name },
            });
        }
        if kw.eq_ignore_ascii_case("STRING") {
            self.scanner.skip_trivia()?;
            let max_len = if self.scanner.eat_punct("[") {
                let (n, _) = self.scanner.expect_u32("STRING max length")?;
                let _ = self.scanner.expect_punct("]")?;
                n
            } else {
                255
            };
            return Ok(TypeRef::scalar(ScalarType::String { max_len }));
        }
        if kw.eq_ignore_ascii_case("CODE") {
            let _ = self.scanner.expect_punct("<")?;
            let (codec_id, _) = self.scanner.expect_node_id()?;
            let _ = self.scanner.expect_punct(">")?;
            return Ok(TypeRef::scalar(ScalarType::Code { codec_id }));
        }
        if let Some(scalar) = parse_simple_scalar(&kw) {
            return Ok(TypeRef::scalar(scalar));
        }
        if let Some(fb_kind) = parse_builtin_fb_kind(&kw) {
            return Ok(TypeRef::Fb { fb_kind });
        }
        if kw.is_empty() {
            return Err(self.scanner.syntax_error_at(sp, "expected type reference"));
        }
        Ok(TypeRef::Named { name: kw })
    }

    fn parse_asl_lvalue(&mut self) -> Result<LValue, Diagnostic> {
        let (slice, base_offset, span) = self.scanner.scan_lvalue_slice()?;
        let mut lv = parse_lvalue(slice).map_err(|err| {
            self.scanner
                .syntax_error_at(span, format!("invalid Ladder LValue `{slice}`: {err}"))
        })?;
        let offset_u32 = u32::try_from(base_offset).unwrap_or(u32::MAX);
        shift_lvalue_spans(&mut lv, offset_u32);
        Ok(lv)
    }

    fn parse_asl_expr(
        &mut self,
        stop_chars: &[char],
        stop_keywords: &[&str],
        stop_dotdot: bool,
    ) -> Result<Expr, Diagnostic> {
        let (slice, base_offset, span) =
            self.scanner
                .scan_expr_slice(stop_chars, stop_keywords, stop_dotdot)?;
        let mut expr = parse_expr(slice).map_err(|err| {
            self.scanner
                .syntax_error_at(span, format!("invalid Ladder expression `{slice}`: {err}"))
        })?;
        let offset_u32 = u32::try_from(base_offset).unwrap_or(u32::MAX);
        shift_expr_spans(&mut expr, offset_u32);
        Ok(expr)
    }

    // -----------------------------------------------------------------------
    // Intact `Structured POU` (`PouBody::Statements`) & `Chart` preservation
    // -----------------------------------------------------------------------

    fn parse_structured_pou(&mut self, leading: Vec<RawComment>) -> Result<Pou, Diagnostic> {
        let ParsedPouHeader {
            start,
            id,
            name,
            kind,
            preferred_view,
            params,
            locals,
        } = self.parse_pou_header("STRUCTURED_POU", leading)?;
        let _ = self.scanner.expect_keyword("BEGIN")?;
        let stmts = self.parse_stmt_block(&["END_STRUCTURED_POU"], 0)?;
        let _ = self.scanner.expect_keyword("END_STRUCTURED_POU")?;
        let dangling = self.scanner.take_leading_comments();
        self.push_comments(
            dangling,
            CommentAnchor::Node { id: id.clone() },
            AstCommentPlacement::Dangling,
        );
        let span = self.scanner.span_from(start);
        Ok(Pou {
            id,
            name,
            kind,
            preferred_view,
            params,
            locals,
            body: PouBody::Statements(stmts),
            span,
        })
    }

    fn parse_stmt_block(
        &mut self,
        terminators: &[&str],
        depth: usize,
    ) -> Result<Vec<Stmt>, Diagnostic> {
        if depth > MAX_NESTING_DEPTH {
            return Err(self
                .scanner
                .syntax_error("maximum statement nesting depth exceeded"));
        }
        let mut stmts = Vec::new();
        loop {
            self.scanner.skip_trivia()?;
            if self.scanner.is_eof() || terminators.iter().any(|kw| self.scanner.peek_keyword(kw)) {
                break;
            }
            stmts.push(self.parse_stmt(depth)?);
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self, depth: usize) -> Result<Stmt, Diagnostic> {
        self.scanner.skip_trivia()?;
        let leading = self.scanner.take_leading_comments();
        let start = self.scanner.pos();
        let id = if self.scanner.eat_punct("#") {
            let (raw_id, _) = self.scanner.expect_node_id()?;
            let _ = self.scanner.expect_punct(":")?;
            NodeId::new(raw_id)
        } else {
            self.alloc_id("stmt")
        };

        self.scanner.skip_trivia()?;
        let kind = if self.scanner.eat_keyword("IF").is_some() {
            let condition = self.parse_asl_expr(&[], &["THEN"], false)?;
            let _ = self.scanner.expect_keyword("THEN")?;
            let then_body =
                self.parse_stmt_block(&["ELSIF", "ELSE", "END_IF"], depth.saturating_add(1))?;
            let mut elif_arms = Vec::new();
            while self.scanner.eat_keyword("ELSIF").is_some() {
                let arm_cond = self.parse_asl_expr(&[], &["THEN"], false)?;
                let _ = self.scanner.expect_keyword("THEN")?;
                let arm_body =
                    self.parse_stmt_block(&["ELSIF", "ELSE", "END_IF"], depth.saturating_add(1))?;
                elif_arms.push(ElifArm {
                    condition: arm_cond,
                    body: arm_body,
                });
            }
            let else_body = if self.scanner.eat_keyword("ELSE").is_some() {
                self.parse_stmt_block(&["END_IF"], depth.saturating_add(1))?
            } else {
                Vec::new()
            };
            let _ = self.scanner.expect_keyword("END_IF")?;
            let _ = self.scanner.expect_punct(";")?;
            StmtKind::If {
                condition,
                then_body,
                elif_arms,
                else_body,
            }
        } else if self.scanner.eat_keyword("WHILE").is_some() {
            let condition = self.parse_asl_expr(&[], &["DO"], false)?;
            let _ = self.scanner.expect_keyword("DO")?;
            let body = self.parse_stmt_block(&["END_WHILE"], depth.saturating_add(1))?;
            let _ = self.scanner.expect_keyword("END_WHILE")?;
            let _ = self.scanner.expect_punct(";")?;
            StmtKind::While { condition, body }
        } else if self.scanner.eat_keyword("FOR").is_some() {
            let (counter, _) = self.scanner.expect_ident("FOR loop counter")?;
            let _ = self.scanner.expect_punct(":=")?;
            let from = self.parse_asl_expr(&[], &["TO", "UNTIL"], false)?;
            self.scanner.skip_trivia()?;
            let inclusive = if self.scanner.eat_keyword("TO").is_some() {
                true
            } else if self.scanner.eat_keyword("UNTIL").is_some() {
                false
            } else {
                return Err(self
                    .scanner
                    .syntax_error("expected `TO` or `UNTIL` in `FOR` loop"));
            };
            let to = self.parse_asl_expr(&[], &["BY", "DO"], false)?;
            self.scanner.skip_trivia()?;
            let step = if self.scanner.eat_keyword("BY").is_some() {
                self.parse_asl_expr(&[], &["DO"], false)?
            } else {
                Expr::new(
                    dendri_ast::ExprKind::Literal(dendri_ast::Literal::Int(1)),
                    Span::default(),
                )
            };
            let _ = self.scanner.expect_keyword("DO")?;
            let body = self.parse_stmt_block(&["END_FOR"], depth.saturating_add(1))?;
            let _ = self.scanner.expect_keyword("END_FOR")?;
            let _ = self.scanner.expect_punct(";")?;
            StmtKind::For {
                counter,
                from,
                to,
                step,
                inclusive,
                body,
            }
        } else if self.scanner.eat_keyword("CASE").is_some() {
            let scrutinee = self.parse_asl_expr(&[], &["OF"], false)?;
            let _ = self.scanner.expect_keyword("OF")?;
            let mut cases = Vec::new();
            let mut default = None;
            loop {
                self.scanner.skip_trivia()?;
                if self.scanner.eat_keyword("END_CASE").is_some() {
                    break;
                }
                if self.scanner.eat_keyword("ELSE").is_some() {
                    let def_body = self.parse_stmt_block(&["END_CASE"], depth.saturating_add(1))?;
                    default = Some(def_body);
                    let _ = self.scanner.expect_keyword("END_CASE")?;
                    break;
                }
                if self.scanner.eat_keyword("WHEN").is_some() {
                    let mut patterns = Vec::new();
                    loop {
                        let first = self.parse_asl_expr(&[',', ':'], &[], true)?;
                        self.scanner.skip_trivia()?;
                        let pat = if self.scanner.eat_punct("..") {
                            let end = self.parse_asl_expr(&[',', ':'], &[], false)?;
                            CasePattern::Range { start: first, end }
                        } else {
                            CasePattern::Value(first)
                        };
                        patterns.push(pat);
                        self.scanner.skip_trivia()?;
                        if self.scanner.eat_punct(",") {
                            continue;
                        }
                        let _ = self.scanner.expect_punct(":")?;
                        break;
                    }
                    let body = self
                        .parse_stmt_block(&["WHEN", "ELSE", "END_CASE"], depth.saturating_add(1))?;
                    cases.push(SwitchCase { patterns, body });
                } else {
                    return Err(self
                        .scanner
                        .syntax_error("expected `WHEN`, `ELSE`, or `END_CASE`"));
                }
            }
            let _ = self.scanner.expect_punct(";")?;
            StmtKind::Switch {
                scrutinee,
                cases,
                default,
            }
        } else if self.scanner.eat_keyword("EXIT").is_some() {
            let _ = self.scanner.expect_punct(";")?;
            StmtKind::Break
        } else if self.scanner.eat_keyword("CONTINUE").is_some() {
            let _ = self.scanner.expect_punct(";")?;
            StmtKind::Continue
        } else if self.scanner.eat_keyword("RETURN").is_some() {
            self.scanner.skip_trivia()?;
            let value = if self.scanner.eat_punct(";") {
                None
            } else {
                let e = self.parse_asl_expr(&[';'], &[], false)?;
                let _ = self.scanner.expect_punct(";")?;
                Some(e)
            };
            StmtKind::Return { value }
        } else if self.scanner.eat_keyword("WAIT").is_some() {
            let duration = self.parse_asl_expr(&[';'], &[], false)?;
            let _ = self.scanner.expect_punct(";")?;
            StmtKind::Wait { duration }
        } else if self.scanner.eat_keyword("FOREIGN").is_some() {
            let (language, fidelity, text, pack_hint) = self.parse_foreign_payload()?;
            let _ = self.scanner.expect_punct(";")?;
            StmtKind::Foreign {
                language,
                fidelity,
                text,
                pack_hint,
            }
        } else if self.scanner.eat_keyword("CALL").is_some() {
            let (callee, _) = self.scanner.expect_ident("callee")?;
            let _ = self.scanner.expect_punct("(")?;
            let raw_args = self.parse_mixed_arg_list()?;
            let _ = self.scanner.expect_punct(";")?;
            let args = raw_args
                .into_iter()
                .map(|(name, value)| {
                    let sp = value.span;
                    CallArg {
                        name,
                        value,
                        span: sp,
                    }
                })
                .collect();
            StmtKind::Call { callee, args }
        } else if self.scanner.eat_keyword("FB_INVOKE").is_some() {
            let instance = self.parse_asl_lvalue()?;
            let _ = self.scanner.expect_punct("(")?;
            let inputs = self.parse_named_arg_list()?;
            let _ = self.scanner.expect_punct(";")?;
            StmtKind::FbInvoke { instance, inputs }
        } else {
            let target = self.parse_asl_lvalue()?;
            let _ = self.scanner.expect_punct(":=")?;
            let value = self.parse_asl_expr(&[';'], &[], false)?;
            let _ = self.scanner.expect_punct(";")?;
            StmtKind::Assign { target, value }
        };

        let span = self.scanner.span_from(start);
        let trailing = self.scanner.take_same_line_trailing()?;
        self.push_comments(
            leading,
            CommentAnchor::Node { id: id.clone() },
            AstCommentPlacement::Leading,
        );
        self.push_comments(
            trailing,
            CommentAnchor::Node { id: id.clone() },
            AstCommentPlacement::Trailing,
        );
        Ok(Stmt::new(id, kind, span))
    }

    fn parse_chart_pou(&mut self, leading: Vec<RawComment>) -> Result<Pou, Diagnostic> {
        let ParsedPouHeader {
            start,
            id,
            name,
            kind,
            preferred_view,
            params,
            locals,
        } = self.parse_pou_header("CHART_POU", leading)?;
        let mut evolve_to_stability = false;
        let mut steps = Vec::new();
        let mut transitions = Vec::new();
        loop {
            self.scanner.skip_trivia()?;
            if self.scanner.eat_keyword("END_CHART_POU").is_some() {
                break;
            }
            if self.scanner.eat_keyword("STABILITY").is_some() {
                let _ = self.scanner.expect_punct(";")?;
                evolve_to_stability = true;
            } else if self.scanner.eat_keyword("STEP").is_some() {
                let (sname, _) = self.scanner.expect_ident("step name")?;
                let sid = self.parse_optional_paren_node_id("step")?;
                self.scanner.skip_trivia()?;
                let initial = self.scanner.eat_keyword("INITIAL").is_some();
                self.scanner.skip_trivia()?;
                let macro_chart = if self.scanner.eat_keyword("MACRO").is_some() {
                    let (m, _) = self.scanner.expect_ident("macro chart name")?;
                    Some(m)
                } else {
                    None
                };
                let mut actions = Vec::new();
                loop {
                    self.scanner.skip_trivia()?;
                    if self.scanner.eat_keyword("END_STEP").is_some() {
                        break;
                    }
                    let _ = self.scanner.expect_keyword("ACTION")?;
                    let aid = self.parse_optional_paren_node_id("act")?;
                    let (q_kw, q_sp) = self.scanner.expect_ident("action qualifier")?;
                    let qualifier = parse_action_qualifier(&q_kw).ok_or_else(|| {
                        self.scanner
                            .syntax_error_at(q_sp, format!("unknown SFC action qualifier `{q_kw}`"))
                    })?;
                    self.scanner.skip_trivia()?;
                    let duration = if self.scanner.eat_keyword("FOR").is_some() {
                        Some(self.parse_asl_expr(&[';'], &["TARGET", "DO"], false)?)
                    } else {
                        None
                    };
                    self.scanner.skip_trivia()?;
                    let target = if self.scanner.eat_keyword("TARGET").is_some() {
                        Some(self.parse_asl_lvalue()?)
                    } else {
                        None
                    };
                    self.scanner.skip_trivia()?;
                    let body = if self.scanner.eat_keyword("DO").is_some() {
                        let stmts = self.parse_stmt_block(&["END_ACTION"], 0)?;
                        let _ = self.scanner.expect_keyword("END_ACTION")?;
                        stmts
                    } else {
                        let _ = self.scanner.expect_punct(";")?;
                        Vec::new()
                    };
                    actions.push(SfcAction {
                        id: aid,
                        qualifier,
                        duration,
                        target,
                        body,
                    });
                }
                steps.push(SfcStep {
                    id: sid,
                    name: sname,
                    initial,
                    actions,
                    macro_chart,
                });
            } else if self.scanner.eat_keyword("TRANSITION").is_some() {
                let (tname, _) = self.scanner.expect_ident("transition name")?;
                let tid = self.parse_optional_paren_node_id("tr")?;
                let _ = self.scanner.expect_keyword("PRIORITY")?;
                let (priority, _) = self.scanner.expect_u32("transition priority")?;
                let _ = self.scanner.expect_keyword("FROM")?;
                let from_steps = self.parse_ident_list()?;
                let _ = self.scanner.expect_keyword("TO")?;
                let to_steps = self.parse_ident_list()?;
                let _ = self.scanner.expect_punct(":")?;
                let receptivity = self.parse_asl_expr(&[';'], &[], false)?;
                let _ = self.scanner.expect_punct(";")?;
                transitions.push(SfcTransition {
                    id: tid,
                    name: tname,
                    from_steps,
                    to_steps,
                    priority,
                    receptivity,
                });
            } else {
                return Err(self.scanner.syntax_error(
                    "expected `STEP`, `TRANSITION`, `STABILITY`, or `END_CHART_POU`",
                ));
            }
        }
        let span = self.scanner.span_from(start);
        Ok(Pou {
            id,
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
            span,
        })
    }

    fn parse_ident_list(&mut self) -> Result<Vec<String>, Diagnostic> {
        let _ = self.scanner.expect_punct("(")?;
        let mut items = Vec::new();
        self.scanner.skip_trivia()?;
        if self.scanner.eat_punct(")") {
            return Ok(items);
        }
        loop {
            let (name, _) = self.scanner.expect_ident("step identifier")?;
            items.push(name);
            self.scanner.skip_trivia()?;
            if self.scanner.eat_punct(",") {
                continue;
            }
            let _ = self.scanner.expect_punct(")")?;
            break;
        }
        Ok(items)
    }
}

fn parse_hardware_kind(raw: &str) -> Option<HardwareKind> {
    if raw.eq_ignore_ascii_case("DIGITAL_IN") {
        Some(HardwareKind::DigitalIn)
    } else if raw.eq_ignore_ascii_case("DIGITAL_OUT") {
        Some(HardwareKind::DigitalOut)
    } else if raw.eq_ignore_ascii_case("DIGITAL_BIDIRECTIONAL") {
        Some(HardwareKind::DigitalBidirectional)
    } else if raw.eq_ignore_ascii_case("ANALOG_IN") {
        Some(HardwareKind::AnalogIn)
    } else if raw.eq_ignore_ascii_case("ANALOG_OUT") {
        Some(HardwareKind::AnalogOut)
    } else if raw.eq_ignore_ascii_case("PWM_OUT") {
        Some(HardwareKind::PwmOut)
    } else if raw.eq_ignore_ascii_case("BUS_PIN") {
        Some(HardwareKind::BusPin)
    } else {
        None
    }
}

fn infer_hardware_kind_from_iec(raw_pin: &str) -> HardwareKind {
    let upper = raw_pin.to_ascii_uppercase();
    if upper.starts_with("%QX") || upper.starts_with("QA.") {
        HardwareKind::DigitalOut
    } else if upper.starts_with("%IW") || upper.starts_with("IW") {
        HardwareKind::AnalogIn
    } else if upper.starts_with("%QW") || upper.starts_with("QW") {
        HardwareKind::AnalogOut
    } else {
        HardwareKind::DigitalIn
    }
}

fn parse_interrupt_edge(raw: &str) -> Option<InterruptEdge> {
    if raw.eq_ignore_ascii_case("RISING") {
        Some(InterruptEdge::Rising)
    } else if raw.eq_ignore_ascii_case("FALLING") {
        Some(InterruptEdge::Falling)
    } else if raw.eq_ignore_ascii_case("CHANGE") {
        Some(InterruptEdge::Change)
    } else if raw.eq_ignore_ascii_case("LOW") {
        Some(InterruptEdge::Low)
    } else if raw.eq_ignore_ascii_case("HIGH") {
        Some(InterruptEdge::High)
    } else {
        None
    }
}

fn parse_authoring_language(raw: &str) -> Option<AuthoringLanguage> {
    if raw.eq_ignore_ascii_case("asl") {
        Some(AuthoringLanguage::Asl)
    } else if raw.eq_ignore_ascii_case("cpp") {
        Some(AuthoringLanguage::Cpp)
    } else if raw.eq_ignore_ascii_case("st") {
        Some(AuthoringLanguage::St)
    } else if raw.eq_ignore_ascii_case("ladder") {
        Some(AuthoringLanguage::Ladder)
    } else if raw.eq_ignore_ascii_case("sfc") {
        Some(AuthoringLanguage::Sfc)
    } else if raw.eq_ignore_ascii_case("micropython") {
        Some(AuthoringLanguage::Micropython)
    } else if raw.eq_ignore_ascii_case("rust") {
        Some(AuthoringLanguage::Rust)
    } else {
        None
    }
}

fn parse_simple_scalar(raw: &str) -> Option<ScalarType> {
    if raw.eq_ignore_ascii_case("BOOL") {
        Some(ScalarType::Bool)
    } else if raw.eq_ignore_ascii_case("SINT") {
        Some(ScalarType::Sint)
    } else if raw.eq_ignore_ascii_case("INT") {
        Some(ScalarType::Int)
    } else if raw.eq_ignore_ascii_case("DINT") {
        Some(ScalarType::Dint)
    } else if raw.eq_ignore_ascii_case("LINT") {
        Some(ScalarType::Lint)
    } else if raw.eq_ignore_ascii_case("USINT") {
        Some(ScalarType::Usint)
    } else if raw.eq_ignore_ascii_case("UINT") {
        Some(ScalarType::Uint)
    } else if raw.eq_ignore_ascii_case("UDINT") {
        Some(ScalarType::Udint)
    } else if raw.eq_ignore_ascii_case("ULINT") {
        Some(ScalarType::Ulint)
    } else if raw.eq_ignore_ascii_case("BYTE") {
        Some(ScalarType::Byte)
    } else if raw.eq_ignore_ascii_case("WORD") {
        Some(ScalarType::Word)
    } else if raw.eq_ignore_ascii_case("DWORD") {
        Some(ScalarType::Dword)
    } else if raw.eq_ignore_ascii_case("LWORD") {
        Some(ScalarType::Lword)
    } else if raw.eq_ignore_ascii_case("REAL") {
        Some(ScalarType::Real)
    } else if raw.eq_ignore_ascii_case("LREAL") {
        Some(ScalarType::Lreal)
    } else if raw.eq_ignore_ascii_case("TIME") {
        Some(ScalarType::Time)
    } else if raw.eq_ignore_ascii_case("ANALOG") {
        Some(ScalarType::Analog)
    } else if raw.eq_ignore_ascii_case("DUTY") {
        Some(ScalarType::Duty)
    } else {
        None
    }
}

fn parse_action_qualifier(raw: &str) -> Option<ActionQualifier> {
    if raw.eq_ignore_ascii_case("N") {
        Some(ActionQualifier::N)
    } else if raw.eq_ignore_ascii_case("S") {
        Some(ActionQualifier::S)
    } else if raw.eq_ignore_ascii_case("R") {
        Some(ActionQualifier::R)
    } else if raw.eq_ignore_ascii_case("L") {
        Some(ActionQualifier::L)
    } else if raw.eq_ignore_ascii_case("D") {
        Some(ActionQualifier::D)
    } else if raw.eq_ignore_ascii_case("P") {
        Some(ActionQualifier::P)
    } else if raw.eq_ignore_ascii_case("P1") {
        Some(ActionQualifier::P1)
    } else if raw.eq_ignore_ascii_case("P0") {
        Some(ActionQualifier::P0)
    } else {
        None
    }
}
