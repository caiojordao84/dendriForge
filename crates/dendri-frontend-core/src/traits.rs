//! Normative Wave 1 plugin trait, dialect capabilities (`DEF-006`), and per-board context
//! (`04-frontends-visual-and-sync.md` §1.2, §2, §4.3).

use crate::foreign::{PackManifest, PackSymbol};
use dendri_ast::{
    Assumption, AuthoringLanguage, Board, BoardPresentation, BoardProgram, Codec, CodecKind,
    Comment, CommentEncoding, CommentPlacement as AstCommentPlacement, CommentStyle, ScalarType,
};
use dendri_span::{Diagnostic, ErrorCode, Severity};
use dendri_stdlib::StdCatalog;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

/// Canonical alias for a board's executable ASL v2 program module.
pub type AslModule = BoardProgram;

/// Normative Wave 1 frontend language identifier (`04` §1.1–1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguageId {
    /// Arduino C++ subset (`dendri-lang-cpp`).
    Cpp,
    /// IEC 61131-3 Structured Text (`dendri-lang-st`).
    St,
    /// IEC 61131-3 Ladder Diagram (`dendri-lang-ladder`).
    Ladder,
    /// IEC 60848 / IEC 61131-3 GRAFCET / Sequential Function Chart (`dendri-lang-sfc`).
    Sfc,
}

impl LanguageId {
    /// Complete ordered list of Wave 1 languages.
    pub const ALL: &'static [Self] = &[Self::Cpp, Self::St, Self::Ladder, Self::Sfc];

    /// Canonical lowercase language tag matching `origin.language` and `pack.toml`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpp => "cpp",
            Self::St => "st",
            Self::Ladder => "ladder",
            Self::Sfc => "sfc",
        }
    }

    /// Converts to the project-level [`AuthoringLanguage`] enum.
    #[must_use]
    pub const fn to_authoring_language(self) -> AuthoringLanguage {
        match self {
            Self::Cpp => AuthoringLanguage::Cpp,
            Self::St => AuthoringLanguage::St,
            Self::Ladder => AuthoringLanguage::Ladder,
            Self::Sfc => AuthoringLanguage::Sfc,
        }
    }

    /// Converts from a project-level [`AuthoringLanguage`] if it is a Wave 1 frontend.
    #[must_use]
    pub const fn from_authoring_language(lang: AuthoringLanguage) -> Option<Self> {
        match lang {
            AuthoringLanguage::Cpp => Some(Self::Cpp),
            AuthoringLanguage::St => Some(Self::St),
            AuthoringLanguage::Ladder => Some(Self::Ladder),
            AuthoringLanguage::Sfc => Some(Self::Sfc),
            AuthoringLanguage::Asl | AuthoringLanguage::Micropython | AuthoringLanguage::Rust => {
                None
            }
        }
    }

    /// Default authoring [`Paradigm`] associated with this language.
    #[must_use]
    pub const fn default_paradigm(self) -> Paradigm {
        match self {
            Self::Cpp | Self::St => Paradigm::Textual,
            Self::Ladder => Paradigm::Ladder,
            Self::Sfc => Paradigm::Grafcet,
        }
    }
}

impl fmt::Display for LanguageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when parsing an unknown [`LanguageId`] string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseLanguageIdError {
    pub input: String,
}

impl fmt::Display for ParseLanguageIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown Wave 1 language id `{}`", self.input)
    }
}

impl std::error::Error for ParseLanguageIdError {}

impl FromStr for LanguageId {
    type Err = ParseLanguageIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("cpp") {
            Ok(Self::Cpp)
        } else if s.eq_ignore_ascii_case("st") {
            Ok(Self::St)
        } else if s.eq_ignore_ascii_case("ladder") || s.eq_ignore_ascii_case("ld") {
            Ok(Self::Ladder)
        } else if s.eq_ignore_ascii_case("sfc") || s.eq_ignore_ascii_case("grafcet") {
            Ok(Self::Sfc)
        } else {
            Err(ParseLanguageIdError {
                input: s.to_owned(),
            })
        }
    }
}

/// Authoring paradigm declared by each [`LanguageFrontend`] (`04` §1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Paradigm {
    Textual,
    Ladder,
    Grafcet,
}

/// IEC 61131-3 Structured Text target comment dialect table (closes `DEF-006`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum StCommentDialect {
    /// IEC 61131-3 3rd Edition: `//`, nested `(* ... *)`, and `/* ... */`.
    #[default]
    Iec61131Ed3,
    /// RuSTy (`plc`) Phase 6b LLVM target: `//`, nested `(* ... *)`, and `/* ... */`.
    RuSTy,
    /// Strict IEC 61131-3 2nd Edition: non-nested `(* ... *)` only.
    StrictEd2,
    /// Classic industrial PLC ST: `//` and non-nested `(* ... *)`.
    ClassicPlc,
}

/// Single authoritative capability-table row for an [`StCommentDialect`] (`DEF-006`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StDialectCapabilityRow {
    pub dialect: StCommentDialect,
    pub supports_line_comments: bool,
    pub supports_paren_block_comments: bool,
    pub supports_nested_paren_comments: bool,
    pub supports_c_block_comments: bool,
}

impl StCommentDialect {
    /// Complete ordered list of all normative IEC ST comment dialects (`DEF-006`).
    pub const ALL: &'static [Self] = &[
        Self::Iec61131Ed3,
        Self::RuSTy,
        Self::StrictEd2,
        Self::ClassicPlc,
    ];

    /// Authoritative static capability table (`DEF-006`).
    pub const CAPABILITY_TABLE: &'static [StDialectCapabilityRow] = &[
        StDialectCapabilityRow {
            dialect: Self::Iec61131Ed3,
            supports_line_comments: true,
            supports_paren_block_comments: true,
            supports_nested_paren_comments: true,
            supports_c_block_comments: true,
        },
        StDialectCapabilityRow {
            dialect: Self::RuSTy,
            supports_line_comments: true,
            supports_paren_block_comments: true,
            supports_nested_paren_comments: true,
            supports_c_block_comments: true,
        },
        StDialectCapabilityRow {
            dialect: Self::StrictEd2,
            supports_line_comments: false,
            supports_paren_block_comments: true,
            supports_nested_paren_comments: false,
            supports_c_block_comments: false,
        },
        StDialectCapabilityRow {
            dialect: Self::ClassicPlc,
            supports_line_comments: true,
            supports_paren_block_comments: true,
            supports_nested_paren_comments: false,
            supports_c_block_comments: false,
        },
    ];

    /// Canonical snake_case dialect identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Iec61131Ed3 => "iec61131_ed3",
            Self::RuSTy => "rusty",
            Self::StrictEd2 => "strict_ed2",
            Self::ClassicPlc => "classic_plc",
        }
    }

    /// Returns the capability row for this dialect.
    #[must_use]
    pub const fn capability_row(self) -> StDialectCapabilityRow {
        StDialectCapabilityRow {
            dialect: self,
            supports_line_comments: self.supports_line_comments(),
            supports_paren_block_comments: self.supports_paren_block_comments(),
            supports_nested_paren_comments: self.supports_nested_paren_comments(),
            supports_c_block_comments: self.supports_c_block_comments(),
        }
    }

    /// Returns `true` iff `// ...` single-line comments are valid in this dialect.
    #[must_use]
    pub const fn supports_line_comments(self) -> bool {
        match self {
            Self::Iec61131Ed3 | Self::RuSTy | Self::ClassicPlc => true,
            Self::StrictEd2 => false,
        }
    }

    /// Returns `true` iff `(* ... *)` parenthesis block comments are valid (always `true` in ST).
    #[must_use]
    pub const fn supports_paren_block_comments(self) -> bool {
        match self {
            Self::Iec61131Ed3 | Self::RuSTy | Self::StrictEd2 | Self::ClassicPlc => true,
        }
    }

    /// Returns `true` iff nested `(* outer (* inner *) outer *)` comments are valid.
    #[must_use]
    pub const fn supports_nested_paren_comments(self) -> bool {
        match self {
            Self::Iec61131Ed3 | Self::RuSTy => true,
            Self::StrictEd2 | Self::ClassicPlc => false,
        }
    }

    /// Returns `true` iff C-style `/* ... */` block comments are valid.
    #[must_use]
    pub const fn supports_c_block_comments(self) -> bool {
        match self {
            Self::Iec61131Ed3 | Self::RuSTy => true,
            Self::StrictEd2 | Self::ClassicPlc => false,
        }
    }

    /// Returns the ordered list of [`CommentStyle`] variants natively accepted by this dialect.
    #[must_use]
    pub fn supported_comment_styles(self) -> Vec<CommentStyle> {
        match self {
            Self::Iec61131Ed3 | Self::RuSTy => vec![
                CommentStyle::Line,
                CommentStyle::ParenBlock,
                CommentStyle::Block,
            ],
            Self::StrictEd2 => vec![CommentStyle::ParenBlock],
            Self::ClassicPlc => vec![CommentStyle::Line, CommentStyle::ParenBlock],
        }
    }
}

/// Declared frontend capabilities for comments, encodings, and ASL capability modules (`04` §2 #8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageCapabilities {
    pub supported_comment_styles: Vec<CommentStyle>,
    pub supports_nested_paren_comments: bool,
    pub supports_line_comments: bool,
    pub comment_encoding: CommentEncoding,
    pub capabilities: Vec<String>,
    pub st_dialect: Option<StCommentDialect>,
}

impl LanguageCapabilities {
    /// Standard capabilities for the Wave 1 Arduino C++ frontend (`dendri-lang-cpp`).
    #[must_use]
    pub fn cpp(comment_encoding: CommentEncoding) -> Self {
        Self {
            supported_comment_styles: vec![
                CommentStyle::Line,
                CommentStyle::Block,
                CommentStyle::DocLine,
                CommentStyle::DocBlock,
            ],
            supports_nested_paren_comments: false,
            supports_line_comments: true,
            comment_encoding,
            capabilities: vec!["asl-core@1".to_owned()],
            st_dialect: None,
        }
    }

    /// Standard capabilities for the Wave 1 IEC 61131-3 ST frontend (`dendri-lang-st`, `DEF-006`).
    #[must_use]
    pub fn st(dialect: StCommentDialect, comment_encoding: CommentEncoding) -> Self {
        Self {
            supported_comment_styles: dialect.supported_comment_styles(),
            supports_nested_paren_comments: dialect.supports_nested_paren_comments(),
            supports_line_comments: dialect.supports_line_comments(),
            comment_encoding,
            capabilities: vec!["asl-core@1".to_owned()],
            st_dialect: Some(dialect),
        }
    }

    /// Standard capabilities for the Wave 1 IEC 61131-3 Ladder frontend (`dendri-lang-ladder`).
    #[must_use]
    pub fn ladder(comment_encoding: CommentEncoding) -> Self {
        Self {
            supported_comment_styles: vec![CommentStyle::Visual],
            supports_nested_paren_comments: false,
            supports_line_comments: false,
            comment_encoding,
            capabilities: vec!["asl-core@1".to_owned()],
            st_dialect: None,
        }
    }

    /// Standard capabilities for the Wave 1 IEC 60848 GRAFCET/SFC frontend (`dendri-lang-sfc`).
    #[must_use]
    pub fn sfc(comment_encoding: CommentEncoding) -> Self {
        Self {
            supported_comment_styles: vec![CommentStyle::Visual],
            supports_nested_paren_comments: false,
            supports_line_comments: false,
            comment_encoding,
            capabilities: vec!["asl-core@1".to_owned()],
            st_dialect: None,
        }
    }

    /// Returns `true` iff `style` is natively supported by this frontend.
    #[must_use]
    pub fn supports_style(&self, style: CommentStyle) -> bool {
        self.supported_comment_styles.contains(&style)
    }

    /// Returns `true` iff C-style `/* ... */` block comments are natively supported.
    #[must_use]
    pub fn supports_c_block_comments(&self) -> bool {
        self.supports_style(CommentStyle::Block)
    }

    /// Returns `true` iff IEC `(* ... *)` parenthesis block comments are natively supported.
    #[must_use]
    pub fn supports_paren_block_comments(&self) -> bool {
        self.supports_style(CommentStyle::ParenBlock)
    }
}

/// Comment placement passed to [`LanguageFrontend::render_comment`], carrying
/// the required `token_ordinal` payload directly on [`Self::Inner`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CommentPlacement {
    Leading,
    Trailing,
    Inner { token_ordinal: u32 },
    Dangling,
    Header,
    Title,
    Body,
}

impl CommentPlacement {
    /// Converts from the AST `(placement, token_ordinal)` representation, enforcing
    /// ADR-027's `ERR_COMMENT_FIELD` invariant (`token_ordinal` required iff `Inner`).
    pub const fn from_ast(
        placement: AstCommentPlacement,
        token_ordinal: Option<u32>,
    ) -> Result<Self, ErrorCode> {
        match (placement, token_ordinal) {
            (AstCommentPlacement::Inner, Some(ord)) => Ok(Self::Inner { token_ordinal: ord }),
            (AstCommentPlacement::Inner, None) => Err(ErrorCode::ErrCommentField),
            (AstCommentPlacement::Leading, None) => Ok(Self::Leading),
            (AstCommentPlacement::Trailing, None) => Ok(Self::Trailing),
            (AstCommentPlacement::Dangling, None) => Ok(Self::Dangling),
            (AstCommentPlacement::Header, None) => Ok(Self::Header),
            (AstCommentPlacement::Title, None) => Ok(Self::Title),
            (AstCommentPlacement::Body, None) => Ok(Self::Body),
            (
                AstCommentPlacement::Leading
                | AstCommentPlacement::Trailing
                | AstCommentPlacement::Dangling
                | AstCommentPlacement::Header
                | AstCommentPlacement::Title
                | AstCommentPlacement::Body,
                Some(_),
            ) => Err(ErrorCode::ErrCommentField),
        }
    }

    /// Extracts and validates the placement from an AST [`Comment`].
    pub const fn from_comment(comment: &Comment) -> Result<Self, ErrorCode> {
        Self::from_ast(comment.placement, comment.token_ordinal)
    }

    /// Converts back into the canonical AST `(CommentPlacement, Option<u32>)` pair.
    #[must_use]
    pub const fn to_ast(self) -> (AstCommentPlacement, Option<u32>) {
        match self {
            Self::Leading => (AstCommentPlacement::Leading, None),
            Self::Trailing => (AstCommentPlacement::Trailing, None),
            Self::Inner { token_ordinal } => (AstCommentPlacement::Inner, Some(token_ordinal)),
            Self::Dangling => (AstCommentPlacement::Dangling, None),
            Self::Header => (AstCommentPlacement::Header, None),
            Self::Title => (AstCommentPlacement::Title, None),
            Self::Body => (AstCommentPlacement::Body, None),
        }
    }
}

/// Output of [`LanguageFrontend::render_comment`]: hazard-safe rendered text plus diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedComment {
    /// Emitted comment string including target-language delimiters (or raw text for visual views).
    pub rendered: String,
    /// Actual comment style used for this projection (may differ from stored `origin.style`).
    pub emitted_style: CommentStyle,
    /// `true` iff projection-only degradation occurred (bidi escape, ASCII fold, delimiter escape,
    /// or style fallback). Stored verbatim comment text is NEVER mutated.
    pub degraded: bool,
    /// Non-fatal diagnostics produced while rendering (`WARN_COMMENT_BIDI`, `WARN_COMMENT_ENCODING`,
    /// `INFO_COMMENT_RELOCATED`).
    pub diagnostics: Vec<Diagnostic>,
}

/// Complete board-scoped parse/emit payload exchanged across [`LanguageFrontend`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoardProgramBundle {
    /// Sole executable ASL v2 program (`imports`, `hardware`, `types`, `data`, `modules`, `pous`).
    pub program: BoardProgram,
    /// First-class verbatim human comments (`[[boards.comments]]`).
    pub comments: Vec<Comment>,
    /// Explicit simulation assumptions (`[[boards.assumptions]]`).
    pub assumptions: Vec<Assumption>,
    /// Optional non-executable presentation hints (`eol_style`, `blank_lines_before`, `origin_files`).
    pub presentation: BoardPresentation,
}

impl BoardProgramBundle {
    /// Wraps an executable [`BoardProgram`] with empty comments, assumptions, and default presentation.
    #[must_use]
    pub fn new(program: BoardProgram) -> Self {
        Self {
            program,
            comments: Vec::new(),
            assumptions: Vec::new(),
            presentation: BoardPresentation::default(),
        }
    }

    /// Clones the parse/emit bundle out of a project [`Board`].
    #[must_use]
    pub fn from_board(board: &Board) -> Self {
        Self {
            program: board.program.clone(),
            comments: board.comments.clone(),
            assumptions: board.assumptions.clone(),
            presentation: board.presentation.clone(),
        }
    }

    /// Applies this bundle back onto an existing [`Board`] without modifying board identity or layout.
    pub fn apply_to_board(&self, board: &mut Board) {
        board.program = self.program.clone();
        board.comments = self.comments.clone();
        board.assumptions = self.assumptions.clone();
        board.presentation = self.presentation.clone();
    }
}

/// Successful frontend output carrying both the produced value and non-fatal diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontendOutput<T> {
    pub value: T,
    pub diagnostics: Vec<Diagnostic>,
}

impl<T> FrontendOutput<T> {
    /// Wraps `value` with no diagnostics.
    #[must_use]
    pub const fn ok(value: T) -> Self {
        Self {
            value,
            diagnostics: Vec::new(),
        }
    }

    /// Wraps `value` with an existing list of diagnostics.
    #[must_use]
    pub const fn with_diagnostics(value: T, diagnostics: Vec<Diagnostic>) -> Self {
        Self { value, diagnostics }
    }

    /// Maps the output payload while preserving all collected diagnostics.
    #[must_use]
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> FrontendOutput<U> {
        FrontendOutput {
            value: f(self.value),
            diagnostics: self.diagnostics,
        }
    }

    /// Appends a diagnostic.
    pub fn push_diagnostic(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Appends multiple diagnostics.
    pub fn extend_diagnostics(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        self.diagnostics.extend(diagnostics);
    }

    /// Returns `true` iff any attached diagnostic has [`Severity::Error`].
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }
}

/// Unified result returned by [`LanguageFrontend::parse`] and [`LanguageFrontend::emit`].
pub type FrontendResult<T> = Result<FrontendOutput<T>, Vec<Diagnostic>>;

/// Per-board context supplied to every frontend parse and emit call (`04` §1.2, §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardContext {
    pub board_id: String,
    pub profile_id: String,
    /// Native C/MCU `int` width from `[cpu] int_bits` (`16` on AVR, `32` on 32-bit cores).
    pub int_bits: u8,
    /// Board default ADC codec (e.g. `uno_10bit` on Arduino Uno R3, `plc_span_27648` on PLC 1200).
    pub default_adc_codec: Option<Codec>,
    /// All codecs declared by the active board profile, keyed by codec ID.
    pub codecs: BTreeMap<String, Codec>,
    /// Deterministic mapping from IEC direct address (`"%IX0.0"`) to board port ID (`"Ia.0"`).
    pub iec_to_port: BTreeMap<String, String>,
    /// Deterministic reverse mapping from board port ID (`"Ia.0"`) to IEC direct address (`"%IX0.0"`).
    pub port_to_iec: BTreeMap<String, String>,
    /// Mirrors CLI `--allow-foreign` for cross-language export (`04` §3.3).
    pub allow_foreign_export: bool,
    /// Target projection encoding policy for comments (`01` §7, `04` §4.3).
    pub comment_encoding: CommentEncoding,
    /// Active IEC 61131-3 ST comment dialect (`DEF-006`).
    pub st_dialect: StCommentDialect,
    /// Hash-locked `pack.toml` manifests loaded for this board (`DEF-008`).
    pub loaded_packs: Vec<PackManifest>,
}

impl BoardContext {
    /// Creates a minimal board context with default Wave 1 settings.
    #[must_use]
    pub fn new(board_id: impl Into<String>, profile_id: impl Into<String>, int_bits: u8) -> Self {
        Self {
            board_id: board_id.into(),
            profile_id: profile_id.into(),
            int_bits,
            default_adc_codec: None,
            codecs: BTreeMap::new(),
            iec_to_port: BTreeMap::new(),
            port_to_iec: BTreeMap::new(),
            allow_foreign_export: false,
            comment_encoding: CommentEncoding::Utf8Passthrough,
            st_dialect: StCommentDialect::Iec61131Ed3,
            loaded_packs: Vec::new(),
        }
    }

    /// Preconfigured [`BoardContext`] matching certified `arduino-uno-r3` (`int_bits = 16`,
    /// `uno_10bit` ADC codec, `pwm_8bit` PWM codec).
    #[must_use]
    pub fn arduino_uno_r3(board_id: impl Into<String>) -> Self {
        let uno_10bit = Codec {
            id: "uno_10bit".to_owned(),
            kind: CodecKind::Binned,
            d: 1024,
            n_max: 1023,
            source_ref: Some(
                "Microchip ATmega328P Datasheet DS40002061B §24.7 (ADC = Vin * 1024 / VREF)"
                    .to_owned(),
            ),
        };
        let pwm_8bit = Codec {
            id: "pwm_8bit".to_owned(),
            kind: CodecKind::Binned,
            d: 256,
            n_max: 255,
            source_ref: Some(
                "Microchip ATmega328P Datasheet DS40002061B §15-§18 (8-bit Timer0/1/2 PWM)"
                    .to_owned(),
            ),
        };
        let mut ctx = Self::new(board_id, "arduino-uno-r3", 16);
        ctx.default_adc_codec = Some(uno_10bit.clone());
        ctx.codecs.insert(uno_10bit.id.clone(), uno_10bit);
        ctx.codecs.insert(pwm_8bit.id.clone(), pwm_8bit);
        ctx
    }

    /// Preconfigured [`BoardContext`] matching certified `plc-1200-open-1214` (`int_bits = 32`,
    /// `plc_span_27648` + `plc_bipolar_27648`, full `%IX0.0..7` / `%QX0.0..7` / `%IW64` / `%IW66`
    /// IEC direct-address mappings).
    #[must_use]
    pub fn plc_1200_open_1214(board_id: impl Into<String>) -> Self {
        let span_27648 = Codec {
            id: "plc_span_27648".to_owned(),
            kind: CodecKind::Span,
            d: 27_648,
            n_max: 27_648,
            source_ref: Some(
                "IEC 61131-2 / Industrial 27648-count unipolar analog scaling (0..27648 nominal)"
                    .to_owned(),
            ),
        };
        let bipolar_27648 = Codec {
            id: "plc_bipolar_27648".to_owned(),
            kind: CodecKind::SpanBipolar,
            d: 27_648,
            n_max: 27_648,
            source_ref: Some(
                "IEC 61131-2 / Industrial 27648-count bipolar analog scaling (-27648..+27648 nominal)"
                    .to_owned(),
            ),
        };
        let mut ctx = Self::new(board_id, "plc-1200-open-1214", 32);
        ctx.default_adc_codec = Some(span_27648.clone());
        ctx.codecs.insert(span_27648.id.clone(), span_27648);
        ctx.codecs.insert(bipolar_27648.id.clone(), bipolar_27648);
        for bit in 0u8..=7 {
            ctx.register_iec_mapping(format!("Ia.{bit}"), format!("%IX0.{bit}"));
            ctx.register_iec_mapping(format!("Qa.{bit}"), format!("%QX0.{bit}"));
        }
        ctx.register_iec_mapping("IW64", "%IW64");
        ctx.register_iec_mapping("IW66", "%IW66");
        ctx
    }

    /// Registers a bidirectional board port <-> IEC direct address mapping.
    pub fn register_iec_mapping(
        &mut self,
        port: impl Into<String>,
        iec_address: impl Into<String>,
    ) {
        let port = port.into();
        let iec = iec_address.into();
        self.iec_to_port.insert(iec.clone(), port.clone());
        self.port_to_iec.insert(port, iec);
    }

    /// Resolves an IEC direct address (e.g. `"%IX0.0"` or `"%ix0.0"`) to its canonical board port ID.
    #[must_use]
    pub fn port_for_iec(&self, iec_address: &str) -> Option<&str> {
        if let Some(port) = self.iec_to_port.get(iec_address) {
            return Some(port.as_str());
        }
        let upper = iec_address.to_ascii_uppercase();
        self.iec_to_port.get(&upper).map(String::as_str)
    }

    /// Resolves a board port ID (e.g. `"Ia.0"`, `"IW64"`) to its IEC direct address.
    #[must_use]
    pub fn iec_for_port(&self, port_id: &str) -> Option<&str> {
        self.port_to_iec.get(port_id).map(String::as_str)
    }

    /// Looks up a declared board codec by ID.
    #[must_use]
    pub fn codec(&self, codec_id: &str) -> Option<&Codec> {
        self.codecs.get(codec_id)
    }

    /// Maps C/C++ `int` to the board's exact ASL signed integer type per `[cpu] int_bits` (`04` §2 #2).
    #[must_use]
    pub const fn c_int_asl_type(&self) -> ScalarType {
        match self.int_bits {
            8 => ScalarType::Sint,
            16 => ScalarType::Int,
            64 => ScalarType::Lint,
            _ => ScalarType::Dint,
        }
    }

    /// Maps C/C++ `unsigned int` to the board's exact ASL unsigned integer type per `[cpu] int_bits`.
    #[must_use]
    pub const fn c_uint_asl_type(&self) -> ScalarType {
        match self.int_bits {
            8 => ScalarType::Usint,
            16 => ScalarType::Uint,
            64 => ScalarType::Ulint,
            _ => ScalarType::Udint,
        }
    }

    /// Looks up an `F1` / `F2` declared symbol in the loaded hash-locked packs for `language`.
    #[must_use]
    pub fn lookup_pack_symbol(
        &self,
        language: LanguageId,
        symbol_name: &str,
    ) -> Option<(&PackManifest, &PackSymbol)> {
        for pack in &self.loaded_packs {
            if pack.pack.language == language
                && let Some(sym) = pack.lookup_symbol(symbol_name)
            {
                return Some((pack, sym));
            }
        }
        None
    }

    /// Returns the built-in domain-split ASL standard library catalog (`std.iec.*`, `std.arduino.*`, `std.plc.*`).
    #[must_use]
    pub fn std_catalog(&self) -> StdCatalog {
        StdCatalog::builtin()
    }
}

/// Unified pure-Rust polyglot frontend plugin contract (`04-frontends-visual-and-sync.md` §1.2).
pub trait LanguageFrontend: Send + Sync {
    /// Language identifier implemented by this frontend crate.
    fn id(&self) -> LanguageId;

    /// Primary authoring paradigm (`Textual`, `Ladder`, or `Grafcet`).
    fn paradigm(&self) -> Paradigm;

    /// Parses source text into a canonical [`BoardProgramBundle`] (ASL IR + verbatim comments + assumptions).
    fn parse(&self, src: &str, ctx: &BoardContext) -> FrontendResult<BoardProgramBundle>;

    /// Emits idiomatic target source text from a [`BoardProgramBundle`] with zero machine magic comments.
    fn emit(&self, bundle: &BoardProgramBundle, ctx: &BoardContext) -> FrontendResult<String>;

    /// Renders a verbatim comment for this target language/dialect without mutating stored text (`ADR-027`).
    fn render_comment(
        &self,
        text: &str,
        style: CommentStyle,
        placement: CommentPlacement,
        ctx: &BoardContext,
    ) -> RenderedComment;

    /// Re-import idempotence matcher (`C4`): returns `true` iff `rendered` (with or without target
    /// delimiters) represents a valid projection of `stored_text` under this frontend's rules.
    fn matches_comment(&self, rendered: &str, stored_text: &str) -> bool;

    /// Declares supported comment styles, encoding behavior, ST dialect, and ASL capabilities.
    fn capabilities(&self) -> LanguageCapabilities;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn def_006_st_comment_dialect_capability_table_is_exact() {
        assert_eq!(StCommentDialect::ALL.len(), 4);
        assert_eq!(StCommentDialect::CAPABILITY_TABLE.len(), 4);

        // IEC 61131-3 Ed.3 & RuSTy: //, nested (* *), /* */
        for dialect in [StCommentDialect::Iec61131Ed3, StCommentDialect::RuSTy] {
            let row = dialect.capability_row();
            assert_eq!(row.dialect, dialect);
            assert!(row.supports_line_comments);
            assert!(row.supports_paren_block_comments);
            assert!(row.supports_nested_paren_comments);
            assert!(row.supports_c_block_comments);
            assert_eq!(
                dialect.supported_comment_styles(),
                vec![
                    CommentStyle::Line,
                    CommentStyle::ParenBlock,
                    CommentStyle::Block
                ]
            );
            let caps = LanguageCapabilities::st(dialect, CommentEncoding::Utf8Passthrough);
            assert!(caps.supports_line_comments);
            assert!(caps.supports_nested_paren_comments);
            assert!(caps.supports_c_block_comments());
            assert!(caps.supports_paren_block_comments());
            assert_eq!(caps.st_dialect, Some(dialect));
        }

        // StrictEd2: non-nested (* *) only
        let ed2 = StCommentDialect::StrictEd2.capability_row();
        assert!(!ed2.supports_line_comments);
        assert!(ed2.supports_paren_block_comments);
        assert!(!ed2.supports_nested_paren_comments);
        assert!(!ed2.supports_c_block_comments);
        assert_eq!(
            StCommentDialect::StrictEd2.supported_comment_styles(),
            vec![CommentStyle::ParenBlock]
        );

        // ClassicPlc: // and non-nested (* *), no /* */
        let classic = StCommentDialect::ClassicPlc.capability_row();
        assert!(classic.supports_line_comments);
        assert!(classic.supports_paren_block_comments);
        assert!(!classic.supports_nested_paren_comments);
        assert!(!classic.supports_c_block_comments);
        assert_eq!(
            StCommentDialect::ClassicPlc.supported_comment_styles(),
            vec![CommentStyle::Line, CommentStyle::ParenBlock]
        );
    }

    #[test]
    fn board_context_presets_validate_codecs_and_iec_addresses() {
        let uno = BoardContext::arduino_uno_r3("mcu_1");
        assert_eq!(uno.int_bits, 16);
        assert_eq!(uno.c_int_asl_type(), ScalarType::Int);
        assert_eq!(uno.c_uint_asl_type(), ScalarType::Uint);
        for codec in uno.codecs.values() {
            assert!(codec.validate().is_ok());
        }
        assert_eq!(uno.default_adc_codec.as_ref().unwrap().d, 1024);

        let plc = BoardContext::plc_1200_open_1214("plc_main");
        assert_eq!(plc.int_bits, 32);
        assert_eq!(plc.c_int_asl_type(), ScalarType::Dint);
        assert_eq!(plc.c_uint_asl_type(), ScalarType::Udint);
        for codec in plc.codecs.values() {
            assert!(codec.validate().is_ok());
        }
        assert_eq!(plc.port_for_iec("%IX0.0"), Some("Ia.0"));
        assert_eq!(plc.port_for_iec("%ix0.7"), Some("Ia.7"));
        assert_eq!(plc.port_for_iec("%QX0.3"), Some("Qa.3"));
        assert_eq!(plc.port_for_iec("%IW64"), Some("IW64"));
        assert_eq!(plc.iec_for_port("Ia.0"), Some("%IX0.0"));
        assert_eq!(plc.iec_for_port("IW66"), Some("%IW66"));
    }

    #[test]
    fn comment_placement_enforces_token_ordinal_contract() {
        assert_eq!(
            CommentPlacement::from_ast(AstCommentPlacement::Inner, Some(3)),
            Ok(CommentPlacement::Inner { token_ordinal: 3 })
        );
        assert_eq!(
            CommentPlacement::from_ast(AstCommentPlacement::Inner, None),
            Err(ErrorCode::ErrCommentField)
        );
        assert_eq!(
            CommentPlacement::from_ast(AstCommentPlacement::Leading, Some(0)),
            Err(ErrorCode::ErrCommentField)
        );
        assert_eq!(
            CommentPlacement::Inner { token_ordinal: 5 }.to_ast(),
            (AstCommentPlacement::Inner, Some(5))
        );
    }

    #[test]
    fn language_id_round_trips_with_strings_and_authoring_language() {
        for &lang in LanguageId::ALL {
            assert_eq!(LanguageId::from_str(lang.as_str()).unwrap(), lang);
            assert_eq!(
                LanguageId::from_authoring_language(lang.to_authoring_language()),
                Some(lang)
            );
        }
        assert_eq!(
            LanguageId::from_authoring_language(AuthoringLanguage::Asl),
            None
        );
        assert!(LanguageId::from_str("cobol").is_err());
    }
}
