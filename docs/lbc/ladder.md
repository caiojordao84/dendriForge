# Language Binding Contract — IEC 61131-3 Ladder Diagram (`ladder`)

> **Normative Specification** | Wave 1 (`asl-core@1`) — Crate `dendri-lang-ladder`  
> **Conventions:** `MUST` / `MUST NOT` / `SHOULD` / `MAY` as in RFC 2119.  
> **Depends on:** [`01-asl-ir-and-project-schema.md`](../architecture/01-asl-ir-and-project-schema.md), [`04-frontends-visual-and-sync.md`](../architecture/04-frontends-visual-and-sync.md), [`05-ide-and-roadmap.md`](../architecture/05-ide-and-roadmap.md)

---

## 0. Frontend Identity & Scope

- **Language ID:** `LanguageId::Ladder` (`"ladder"`)
- **Paradigm:** `Paradigm::Ladder`
- **Structural IR:** First-class `PouBody::Network(LadderNetwork)` (`01` §2.5, ADR-026). **Synthetic temporaries (`__rail_N`) MUST NEVER be persisted in ASL.**
- **Primary Target Profile:** `plc-1200-open-1214` (`execution = "plc_scan"`, `int_bits = 32`), while fully supporting MCU targets (`arduino-uno-r3`, `int_bits = 16`) via the board's canonical I/O Tag Table.
- **Differential Gate Verification (`DEF-003`):** Verified in Phase 6b by projecting `PouBody::Network` deterministically to IEC 61131-3 ST (`StCommentDialect::RuSTy`) and compiling/executing natively via RuSTy (`plc`) against `dendri-vm` traces.

---

## 1. Accepted Structural Grammar (EBNF), Power-Flow Semantics & Rejected Constructs

### 1.1 Structural IR & Canonical Textual/Wire Grammar (`PouBody::Network`)

`dendri-lang-ladder` represents a Ladder POU natively as an ordered collection of `NodeId`-anchored rungs and `(row, col)` grid cells. Both the strict TOML wire representation (`dendri-project`) and the canonical textual Ladder interchange format (`LanguageFrontend::parse` / `emit`) conform to the following grammar:

```ebnf
LadderPou       ::= "NETWORK" Ident { VarBlock } { Rung } "END_NETWORK" ;

Rung            ::= "RUNG" UintLiteral "(" NodeId ")" ":" CellSeries ";" ;
CellSeries      ::= Cell { "--" Cell } ;

Cell            ::= EmptyCell
                  | WireCell
                  | ContactCell
                  | CoilCell
                  | ParallelOrCell
                  | FbBoxCell
                  | CompareBoxCell
                  | CallBoxCell
                  | ForeignBoxCell ;

EmptyCell       ::= "EMPTY" GridPos ;                             (* OPEN CIRCUIT = false *)
WireCell        ::= "WIRE" GridPos ;                              (* Conducts in_rail -> out_rail *)

ContactCell     ::= ( "| |" | "|/|" | "|P|" | "|N|" ) LValue GridPos ;
CoilCell        ::= ( "( )" | "(/)" | "(S)" | "(R)" | "(P)" | "(N)" ) LValue GridPos ;

ParallelOrCell  ::= "BRANCH" GridPos "{" CellSeries { "||" CellSeries } "}" ;

FbBoxCell       ::= "FB" FbKind Ident "(" [ NamedArgList ] ")" GridPos ;
CompareBoxCell  ::= "CMP" CompareOp "(" Expr "," Expr ")" GridPos ;
CompareOp       ::= "EQ" | "NE" | "LT" | "LE" | "GT" | "GE" ;

CallBoxCell     ::= "CALL" Ident "(" [ ArgList ] ")" [ "=>" LValue ] GridPos ;
ForeignBoxCell  ::= "FOREIGN" "(" StringLit "," Fidelity "," StringLit
                    [ "," StringLit ] ")" GridPos ;

GridPos         ::= "@" "(" UintLiteral "," UintLiteral ")" ;     (* (row, col) *)
Fidelity        ::= "F0" | "F1" | "F2" ;
```

### 1.2 Normative Materialized Power-Flow Semantics (Load-Time Lowering)

At load time inside `dendri-vm` (strictly in memory, never written back to ASL), `PouBody::Network` lowers into ephemeral boolean power-rail registers (`RailReg`) and edge-memory slots (`EdgeReg`):

1. **Deterministic Ordering:**
   - Rungs MUST execute sorted ascending by explicit `rung.order: u32` — implementations MUST NOT trust raw array ordering.
   - Within each rung and within each branch of a `ParallelOr`, cells MUST execute sorted left-to-right by `(col, row)`.
   - Every rung starts with left power rail initialized to `true`.
2. **Cell Power-Flow Transfer Rules:**
   - **`Empty`:** Sets `out_rail = false`. An empty cell is ALWAYS an **open circuit** and MUST NEVER conduct power.
   - **`Wire`:** Copies `out_rail = in_rail`.
   - **`Contact`:** Reads boolean `bit` from `target`:
     - `NormallyOpen` (`--| |--`): `out_rail = in_rail && bit`
     - `NormallyClosed` (`--|/|--`): `out_rail = in_rail && !bit`
     - `PositiveTransition` (`--|P|--`): `out_rail = in_rail && (bit && !prev_bit)` (updates ephemeral edge slot with `bit`)
     - `NegativeTransition` (`--|N|--`): `out_rail = in_rail && (!bit && prev_bit)` (updates ephemeral edge slot with `bit`)
   - **`Coil`:** Receives `in_rail` and ALWAYS propagates the **pre-mutation** materialized rail `out_rail = in_rail` downstream:
     - `Normal` (`--( )--`): **Unconditionally** writes `target := in_rail` on BOTH `true` and `false`.
     - `Negated` (`--(/)--`): **Unconditionally** writes `target := !in_rail`.
     - `Set` (`--(S)--`): Writes `target := true` iff `in_rail == true`; leaves `target` unchanged otherwise.
     - `Reset` (`--(R)--`): Writes `target := false` iff `in_rail == true`; leaves `target` unchanged otherwise.
     - `PositiveTransition` (`--(P)--`): Writes `target := (in_rail && !prev_rail)` (1-scan rising pulse).
     - `NegativeTransition` (`--(N)--`): Writes `target := (!in_rail && prev_rail)` (1-scan falling pulse).
     - **Mid-Rung Mutation Immunity:** Because `out_rail` preserves the pre-mutation materialized rail, a rung such as `[a] --(R a)-- (b)` guarantees `(b)` observes the energized rail that entered `--(R a)--`, never the post-reset value of `a`.
   - **`ParallelOr`:** Initializes `out_rail = false`; evaluates each branch independently starting from the identical materialized `in_rail` (avoiding side-effect duplication or $O(2^k)$ expression blowup), then computes `out_rail = branch_0_out || branch_1_out || ...`.
   - **`Compare` Box (`EQ | NE | LT | LE | GT | GE`):** Strictly typed enum (`LadderCompareOp`, never string-matched fallback). Gated by `EN = in_rail`: if `!EN`, skips evaluation and outputs `out_rail = false`; if `EN`, evaluates `left op right` and outputs `out_rail = EN && cmp_result`.
   - **`Fb` Box (`EN`/`IN` & `Q` Propagation):** Evaluates explicit named inputs, binds the incoming power rail `in_rail` to the FB's primary boolean trigger pin (unless explicitly overridden in `inputs`), invokes the stateful FB instance, and propagates its primary boolean output onto `out_rail`:
     - `TON`, `TOF`, `TP`: rail -> `IN`, `out_rail = fb.Q`
     - `CTU`, `CTUD`: rail -> `CU`, `out_rail = fb.Q` / `fb.QU`
     - `CTD`: rail -> `CD`, `out_rail = fb.Q`
     - `R_TRIG`, `F_TRIG`: rail -> `CLK`, `out_rail = fb.Q`
     - `SR` / `RS`: rail -> `S1` / `S`, `out_rail = fb.Q1`
     - `UART_SEND`, `I2C_XFER`, `SPI_XFER`, `MB_CLIENT`: rail -> `REQ`, `out_rail = fb.DONE`
     - `UART_RECV`: rail -> `EN_R`, `out_rail = fb.NDR`
     - `MB_SERVER`: rail -> `ENABLE`, `out_rail = fb.DONE` (or `NDR` per bus event)
     - User `FB`: rail -> `EN`, `out_rail = ENO`
   - **`Call` Box (Math / Move / Convert / Callable `Structured POU`):** Standard IEC `EN`/`ENO` semantics. Executes `callee(args...)` ONLY when `EN == true`, writes the return value to `output` if bound, and passes `ENO = EN` downstream.
   - **`Foreign` Box:** Renders as a locked box in the Ladder grid. Under default simulation policy (`halt`), hitting an unmodeled `Foreign` box stops execution with a 4-part diagnostic; when governed by `[[boards.assumptions]]`, applies the recorded policy and propagates `out_rail = in_rail`.

### 1.3 Conservative Structural Lifting vs. Intact `Structured POU` Boxes

- **Derived Projection (Network -> ST / C++):** Emitting ST or C++ from `PouBody::Network` is a total, deterministic projection using temporary local boolean rails strictly inside the emitted target code (never written back to ASL), preserving all rung and element comments.
- **Conservative Lifting (Statements -> Network):** Lifting a textual `PouBody::Statements` POU into `PouBody::Network` MUST recognize **only** these declared canonical patterns:
  1. Pure boolean coil assignments (`out := expr;` where `expr` is an acyclic boolean composition of contacts, `NOT`, `AND`, `OR`).
  2. Guarded latch/unlatch (`IF cond THEN out := TRUE; END_IF;` -> `Set`; `IF cond THEN out := FALSE; END_IF;` -> `Reset`).
  3. Canonical FB call followed by output wire (`ton1(IN := rail, PT := T#500ms); q := ton1.Q;`).
  4. `EN`-gated comparison (`out := rail AND (a >= b);` -> `Compare`).
  5. `EN`-gated stateless call/math/move (`IF rail THEN dst := callee(args...); END_IF;` -> `Call` box).
- **Non-Liftable POUs Stay Intact:** Any POU containing loops (`FOR`/`WHILE`), multi-branch `CASE`/`Switch`, arbitrary mutation sequences, string tokenizers, or `Foreign` statements MUST NOT be flattened or corrupted. It remains an intact typed **`Structured POU`** (`PouBody::Statements`) callable as a `LadderBoxKind::Call` box from any Ladder rung, preserving all `OB_STARTUP`, `OB_HW_INT` (ISR), and helper `FC`/`FB` definitions.

### 1.4 Rejected Constructs & Exact `ErrorCode` Mapping

| Violation | Frozen `ErrorCode` |
|---|---|
| Malformed LD wire/text syntax or unknown TOML key | `ERR_TOML_SYNTAX` / `ERR_UNKNOWN_FIELD` |
| Duplicate `NodeId` across rungs or cells | `ERR_DUPLICATE_ID` |
| Contact or coil referencing non-`BOOL` variable/tag | `ERR_TYPE_MISMATCH` |
| Coil writing to `CONST` or input-only hardware tag (`%IX*`, `%IW*`) | `ERR_CONST_WRITE` / `ERR_HARDWARE_LABEL_WRITE` |
| Contact, coil, or box referencing undeclared tag or variable | `ERR_UNDECLARED_SYMBOL` |
| `Fb` box referencing undeclared FB instance or wrong FB kind | `ERR_TYPE_MISMATCH` / `ERR_UNDECLARED_SYMBOL` |
| `Compare` box comparing incompatible types (e.g., `INT` vs `UINT` without cast) | `ERR_TYPE_MISMATCH` |
| `Call` box invoking unresolved function, wrong arity, or mismatched output type | `ERR_UNRESOLVED_FUNCTION` / `ERR_ARITY_MISMATCH` / `ERR_RETURN_TYPE_MISMATCH` |
| Blocking `WAIT_TIME` / `WAIT_TIME_US` box inside `OB_HW_INT` or `OB_CYCLIC_INT` Ladder network | `ERR_ISR_FORBIDDEN_OP` |
| Unresolved hardware port or capability mismatch on bound tag | `ERR_UNKNOWN_PORT` / `ERR_PIN_CAPABILITY_MISMATCH` |
| Comment referencing deleted/nonexistent rung or cell `NodeId` | `ERR_COMMENT_ANCHOR` |
| Invalid `token_ordinal` / placement combination on LD comment | `ERR_COMMENT_FIELD` |
| Cross-language export of Ladder network containing `ForeignBox` without `--allow-foreign` | `ERR_FOREIGN_EXPORT_BLOCKED` |

---

## 2. Per-Board Type Mapping

Ladder contacts and coils operate strictly on ASL `BOOL` (1-bit). Boxes (`Compare`, `Fb`, `Call`) support the full ASL v2 type system (`01` §2):

| Ladder Element | Allowed ASL Types | Board `[cpu] int_bits` Interaction |
|---|---|---|
| Contacts (`NO`, `NC`, `P`, `N`) & Coils (`Normal`, `Negated`, `S`, `R`, `P`, `N`) | `BOOL` only (scalar, `ARRAY` element of `BOOL`, `STRUCT` field of `BOOL`, FB `.Q`/`.QU`/`.QD`/`.Q1`/`.DONE`/`.NDR`/`.ERR`, SFC `<Step>.X` / `<Macro>.DONE`, digital hardware tag) | Independent of `int_bits` |
| `Compare` (`EQ..GE`) | Matching scalar pair (`BOOL`, signed int, unsigned int, bit-string, `REAL`/`LREAL`, `TIME`, `ANALOG`, `DUTY`, same-codec `CODE`); zero silent mixed-sign comparison | Preserves explicit widths across `16`-bit AVR and `32`-bit PLC projections |
| `Fb` (`TON`/`TOF`/`TP`, `CTU`/`CTD`/`CTUD`, `R_TRIG`/`F_TRIG`, `SR`/`RS`, `UART_*`, `I2C_XFER`, `SPI_XFER`, `MB_CLIENT`, `MB_SERVER`, user `FB`) | Exact typed FB pins (`TIME`, `DINT`, `BOOL`, `BYTE`, `STRING`, `UINT`, `USINT`) per `dendri-stdlib` | Fixed IEC widths |
| `Call` (`ADD`, `SUB`, `MUL`, `DIV`, `MOD`, `MOVE`, `LIMIT`, `CLAMP`, `MAP_RANGE`, `NORM_X`, `SCALE_X`, user `FC` / `Structured POU`) | Static signature checked against `dendri-stdlib` / project POUs | When projected to C++, `INT` (16-bit) vs `DINT` (32-bit) honors `[cpu] int_bits` |

---

## 3. Numeric Codecs & Intrinsic Mapping

- **Analog Tags in Ladder:** Analog inputs (e.g., `PressRaw` bound to `plc-1200-open-1214` `IW64` / `%IW64`, codec `plc_span_27648`; or `PotPin` bound to `arduino-uno-r3` `A0`, codec `uno_10bit`) are read as `CODE<codec_id>` inside `Compare` and `Call` boxes (`CODE_TO_ANALOG`, `ANALOG_TO_CODE`, `ANALOG_TO_DUTY`, `PWM_WRITE`).
- **Two-Layer Separation:** Codecs operate in exact integer arithmetic at `M = 10⁹` (`01` §3); physical channel bands (`IW64` underrange `-4864` / overrange `32511`, `IW66` bipolar `-27648..+27648`) and PWM timer transfer functions live strictly in the board profile.
- **Reduced-Rational Cross-Platform Projection:** When projecting a Ladder `Call`/`Compare` box containing cross-platform analog code scaling to ST or C++, the emitter **MUST NEVER** emit `M = 10⁹`. It emits the clamped reduced rational `min(n_max2, floor(n * D2_red / D1_red))` with 32-bit vs 64-bit intermediate width selected by `n_max1 * D2_red < 2^32` and mandatory clamping for `span -> binned`.

---

## 4. Comment Mapping & `comment_styles`

Ladder comments are stored verbatim in `[[boards.comments]]` anchored by stable `NodeId`s (never inside `[boards.presentation]`):

| Ladder Surface | Anchor `NodeId` | `CommentPlacement` | Default `origin.style` |
|---|---|---|---|
| Network / Rung Title | `LadderRung.id` | `Title` | `Visual` |
| Rung Descriptive Block | `LadderRung.id` | `Body` / `Leading` | `Visual` (or preserved `Line`/`ParenBlock`/`Block` from text) |
| Cell / Contact / Coil / Box Note | `LadderCell.id` | `Body` / `Trailing` / `Leading` | `Visual` |
| POU Header Comment | `Pou.id` | `Header` | `Visual` / `DocBlock` / `Line` |
| Orphaned Deleted Rung/Cell Comment | Enclosing `Pou.id` | `Dangling` | Preserved original (`INFO_COMMENT_ORPHANED`) |

- **Verbatim Conservation (`C1–C5`):** Switching a Ladder POU to ST or C++ view and back preserves every comment's verbatim `text` without trimming, Unicode normalization, or loss.
- **Hazard & Encoding Rules:** When projecting Ladder comments to ST or C++, target comment delimiters, bidi controls (`WARN_COMMENT_BIDI`), and `comment_encoding` (`WARN_COMMENT_ENCODING`) follow the target frontend's `render_comment` / `matches_comment` rules without mutating stored `text`. When emitting Ladder text syntax, `//` and `(* ... *)` follow `dendri-frontend-core`'s hazard-safe comment engine with re-import idempotence (`C4`).

---

## 5. Module, Multi-Sheet Hierarchy, Import & Pack Mapping

- **Multi-Sheet Ladder Hierarchy (`04` §3.1):** `OB_MAIN` (`OB1`) invokes secondary and tertiary Ladder networks (`Main` -> `Conveyor_Ctrl` -> `Safety_Interlock`) or `Structured POU` boxes via `LadderBoxKind::Call` / `LadderBoxKind::Fb`.
- **Module & Stdlib Scoping:** `Fb` and `Call` boxes resolve strictly against `[boards.program].imports` (`std.iec.*`, explicit `std.plc.*` / `std.arduino.*`, embedded `[[boards.program.modules]]`, external `.dendrilib` locks, and hash-locked `pack.toml` libraries such as `assets/packs/plc-modbus-tcp.pack.toml` for `MB_CLIENT` / `MB_SERVER`).
- **Cross-Language Export Guard:** Exporting a Ladder network containing `LadderBoxKind::Foreign` to ST or C++ is rejected with `ERR_FOREIGN_EXPORT_BLOCKED` unless `allow_foreign_export` (`--allow-foreign`) is enabled, emitting a clearly demarcated `"NOT TRANSLATED"` comment block.

---

## 6. Hardware Address & Tag Mapping

- Contacts and coils bind strictly to declared `[[boards.program.hardware]]` tag labels or typed `BOOL` variables — never raw pin strings and never magic comments.
- On `plc-1200-open-1214`, logical tags display alongside their bidirectional `iec_address` (`Ia.0 ⇄ %IX0.0`, `Qa.0 ⇄ %QX0.0`, `IW64 ⇄ %IW64`, `IW66 ⇄ %IW66`) sourced directly from the board profile.
- On `arduino-uno-r3`, logical tags display alongside their canonical port IDs (`D0..D13`, `A0..A5`) from `[[boards.program.hardware]]`.

---

## 7. Mandatory Round-Trip Class — `LOSSLESS_ASL` & Conformance Suite

`dendri-lang-ladder` MUST satisfy **`LOSSLESS_ASL`**:

1. **Structural Identity:** For every valid `PouBody::Network(LadderNetwork)`:
   $$\text{parse}(\text{emit}(M, \text{ctx}), \text{ctx}) \equiv M$$
   preserving exact `NodeId`s, `rung.order`, `(row, col)` cell coordinates, `ParallelOr` nesting, `LadderCompareOp`, `FbKind`, `ForeignBox` payloads, and `[[boards.comments]]` multiset with zero persisted `__rail_N` temporaries.
2. **Mandatory Conformance Suite (`04` §2, `05` §4):**
   - `proptest` verifying `LOSSLESS_ASL` across nested `ParallelOr` branches, all contact/coil/box variants, and non-trivial `(row, col)` / `order` permutations.
   - Mid-rung mutation hazard test (`[a] --(R a)-- (b)`), empty-cell open-circuit test (`Empty` NEVER conducts), normal-coil unconditional false write test, and `EN`/`ENO`/`Q` propagation tests.
   - Conservative lifting tests confirming canonical patterns lift cleanly while non-liftable POUs remain intact `Structured POU` boxes.
   - Comment conservation (`C1–C5`), three-level `Foreign` preservation, and one genuine negative fixture per produced `ErrorCode`.
   - **Phase 6b Differential Gate:** Emitted ST from every Wave 1 Ladder golden scenario compiles via RuSTy (`plc`) and matches `dendri-vm` traces bit-for-bit.

---

## 8. `LanguageCapabilities` & Feature Matrix Contract

```rust
// Normative capability profile for `dendri-lang-ladder`
LanguageCapabilities {
    supported_comment_styles: vec![
        CommentStyle::Visual,
        CommentStyle::Line,
        CommentStyle::ParenBlock,
    ],
    supports_nested_paren_comments: true,
    supports_line_comments: true,
    comment_encoding: ctx.comment_encoding, // Utf8Passthrough | AsciiFold | WarnOnly
    capabilities: vec!["asl-core@1".to_owned()],
    st_dialect: None,
}
```

- **Auto-Generated Feature Matrix:** The published Ladder feature-support matrix MUST be generated directly from the `dendri-lang-ladder` conformance test suite via `dendri-frontend-core::conformance` and MUST NOT be maintained by hand (`04` §2).
