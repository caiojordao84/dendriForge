# Language Binding Contract — IEC 60848 / IEC 61131-3 GRAFCET / SFC (`sfc`)

> **Normative Specification** | Wave 1 (`asl-core@1`) — Crate `dendri-lang-sfc`  
> **Conventions:** `MUST` / `MUST NOT` / `SHOULD` / `MAY` as in RFC 2119.  
> **Depends on:** [`01-asl-ir-and-project-schema.md`](../architecture/01-asl-ir-and-project-schema.md), [`04-frontends-visual-and-sync.md`](../architecture/04-frontends-visual-and-sync.md), [`05-ide-and-roadmap.md`](../architecture/05-ide-and-roadmap.md)

---

## 0. Frontend Identity & Scope

- **Language ID:** `LanguageId::Sfc` (`"sfc"`)
- **Paradigm:** `Paradigm::Grafcet`
- **Structural IR:** First-class `PouBody::Chart(SfcChart)` (`01` §2.5, ADR-026). **Synthetic temporaries (`__sfc_*`) MUST NEVER be persisted in ASL.**
- **Standards:** IEC 60848 (GRAFCET evolution rules, Rule 5 two-phase clearing/setting, OR priority, AND divergence/convergence, macro-steps) and IEC 61131-3 §6.7 / Table 47 (textual SFC syntax and action qualifiers).
- **Primary Target Profile:** `plc-1200-open-1214` (`execution = "plc_scan"`, `int_bits = 32`), while fully supporting MCU targets (`arduino-uno-r3`, `int_bits = 16`, `execution = "cooperative"`) via the board's canonical I/O Tag Table.
- **Differential Gate Verification (`DEF-003`):** Verified in Phase 6b by projecting `PouBody::Chart` deterministically to IEC 61131-3 ST (`StCommentDialect::RuSTy`) and compiling/executing natively via RuSTy (`plc`) against `dendri-vm` traces.

---

## 1. Accepted Structural Grammar (EBNF), Rule 5 Semantics & Rejected Constructs

### 1.1 Structural IR & Canonical Textual SFC Grammar (`PouBody::Chart`)

`dendri-lang-sfc` parses and emits IEC 61131-3 textual SFC (and binds directly to the `PouBody::Chart` TOML wire representation in `dendri-project`). Every step, transition, and action carries a stable `NodeId`:

```ebnf
SfcPou          ::= "SFC_CHART" Ident [ "(" "STABILITY" ":=" BoolLit ")" ]
                    { VarBlock }
                    { StepDecl | TransitionDecl }
                    "END_SFC_CHART" ;

StepDecl        ::= ( "INITIAL_STEP" | "STEP" ) Ident
                    [ "MACRO" Ident ] ":"
                    { ActionAssoc }
                    "END_STEP" ;

ActionAssoc     ::= Qualifier [ "(" Expr ")" ] ":"
                    ( LValue ";" | "{" { Stmt } "}" | LValue "{" { Stmt } "}" ) ;

Qualifier       ::= "N" | "S" | "R" | "L" | "D" | "P" | "P1" | "P0" ;

TransitionDecl  ::= "TRANSITION" Ident
                    [ "(" "PRIORITY" ":=" UintLiteral ")" ]
                    "FROM" StepList "TO" StepList
                    ":=" Expr ";"
                    "END_TRANSITION" ;

StepList        ::= Ident | "(" Ident { "," Ident } ")" ;
```

- Single `from` + single `to` expresses sequential progression.
- Multiple `from` steps (`FROM (S1, S2)`) express **simultaneous (AND) convergence**.
- Multiple `to` steps (`TO (S3, S4)`) express **simultaneous (AND) divergence**.
- Multiple transitions sharing a `from` step express **selective (OR) divergence**, resolved strictly by ascending `priority: u32` (default `0`, tie-broken deterministically by declaration order / left-to-right visual order).
- Visual step coordinates `(x, y)` live exclusively in `[boards.presentation].node_coords` keyed by `SfcStep.id` (never in executable IR).

### 1.2 Normative IEC 60848 Rule 5 Two-Phase Evolution Semantics

At load time inside `dendri-vm` (strictly in memory, never written back to ASL), `PouBody::Chart` lowers to a compiled SFC engine executing each scan in four deterministic stages:

1. **First-Scan Initialization & Step Timer Advance:**
   - On Scan 1 (first execution of the chart), all steps with `initial == true` become active with `<Step>.T := 0us`. If an initial step is a macro-step (`macro_chart = Some(sub)`), `sub` is cleanly reset and initialized on Scan 1 so initial macro-steps never skip entry initialization.
   - On subsequent scans, every currently active step increments `<Step>.T` by `dt_us` (saturating `u64`/`i64`).
   - Snapshot `scan_start_active = prev_active_steps` for edge qualifiers (`P`, `P1`, `P0`).
2. **Phase 1 — Frozen Pre-Evolution Transition Evaluation:**
   - Freeze `frozen_active = current active_steps`. All `<Step>.X` references inside transition receptivities MUST observe `frozen_active` (never partially mutated state).
   - Iterate transitions in pre-sorted `(priority ascending, original_index ascending)` order:
     - **AND Convergence Barrier:** Skip transition unless `!from_steps.is_empty()` AND every step in `from_steps` is present in `frozen_active`.
     - **Deterministic OR Priority:** Maintain `claimed_from_steps`. If any step in `from_steps` is already in `claimed_from_steps`, skip this lower-priority transition.
     - Evaluate `receptivity` (`BOOL`). If `true`, insert all `from_steps` into `claimed_from_steps` and append the transition to `fired`.
3. **Phase 2 — Exact Rule 5 Two-Phase Clearing & Setting:**
   - **Phase 2a (Deactivate):** For every transition in `fired`, remove all `from_steps` from `active_steps`.
   - **Phase 2b (Activate — STRICTLY AFTER Phase 2a):** For every transition in `fired`, insert all `to_steps` into `active_steps` and reset each activated step's `<Step>.T := 0us`.
   - **Rule 5 Guarantee:** If step `S` is simultaneously deactivated by transition `T_b` (`S ∈ T_b.from_steps`) and activated by transition `T_a` (`S ∈ T_a.to_steps`), `S` **MUST remain ACTIVE** after evolution because Phase 2b executes strictly after all Phase 2a clears.
   - **Macro-Step Re-Entry Reset:** Whenever a macro-step appears in `to_steps` of a fired transition — including same-scan re-entry — its enclosed sub-chart MUST be cleanly reset and re-initialized (`active_steps`, `step_elapsed_us`, `n_owned_coils`, `s_latched_coils`) with zero duplicate setup artifacts.
   - **Transient / Instability Guard:** If `evolve_to_stability == false`, stop after one evolution pass per scan. If `evolve_to_stability == true`, repeat Phases 1–2 until `fired` is empty; exceeding `max_sfc_stability_steps` aborts deterministically with an instability-loop runtime fault rather than hanging.
4. **Stage 3 — Active Macro-Steps & IEC 61131-3 Table 47 Action Execution:**
   - Active macro-steps execute one scan tick of their enclosed sub-chart.
   - Evaluate every action across all steps per Table 47 (§1.3).

### 1.3 Normative IEC 61131-3 Table 47 Action Qualifiers

| Qualifier | Duration Required | Firing Condition | Boolean Coil (`target`) Semantics | Inline Body (`body`) Semantics |
|---|---|---|---|---|
| `N` (Non-stored) | Forbidden | `is_active` | Sets `target := true` while active; **automatically resets `target := false` on step deactivation** unless driven by another active `N`/`L`/`D` action or latched by `S` | Executes every scan while step is active |
| `S` (Set / Stored) | Forbidden | `is_active` | Latches `target := true` and marks `target` as `S`-latched until overridden by `R` | Executes every scan while step is active |
| `R` (Overriding Reset) | Forbidden | `is_active` | Clears `S`-latch and writes `target := false`; **strictly overrides both `N` and `S`** on the same scan | Executes every scan while step is active |
| `L` (Time Limited) | **Required (`TIME`)** | `is_active && elapsed < duration` | Behaves like `N` while `elapsed < duration`; auto-resets `target := false` once `elapsed >= duration` or step deactivates | Executes while `is_active && elapsed < duration` |
| `D` (Time Delayed) | **Required (`TIME`)** | `is_active && elapsed >= duration` | Behaves like `N` once `elapsed >= duration`; auto-resets `target := false` on step deactivation | Executes while `is_active && elapsed >= duration` |
| `P` (Pulse) | Forbidden | `is_active && !was_active` | Writes `target := true` on activation scan (1-scan pulse) | Executes once on step activation |
| `P1` (Rising Edge) | Forbidden | `is_active && !was_active` | Writes `target := true` on step activation edge | Executes once on step activation |
| `P0` (Falling Edge) | Forbidden | `!is_active && was_active` | Writes `target := true` on step deactivation edge | Executes once on step deactivation |

### 1.4 Native Step Attributes & Macro-Steps (Zero `__sfc_*` Mangling)

- **`<StepName>.X` (`BOOL`):** First-class read-only step activity flag accessible from transition receptivities, SFC actions, ST, C++, and Ladder contacts.
- **`<StepName>.T` (`TIME`):** First-class read-only step elapsed active duration (microseconds `i64`), reset to `T#0us` on every activation.
- **`<MacroStep>.DONE` (`BOOL`):** First-class read-only macro-step completion flag returning `true` iff the enclosed sub-chart's exit step (its final declared step) is currently active.
- Direct writes to `<StepName>.X`, `<StepName>.T`, or `<MacroStep>.DONE` are forbidden (`ERR_CONST_WRITE`).

### 1.5 Rejected Constructs & Exact `ErrorCode` Mapping

| Violation | Frozen `ErrorCode` |
|---|---|
| Malformed SFC syntax or unknown TOML wire key | `ERR_TOML_SYNTAX` / `ERR_UNKNOWN_FIELD` |
| Duplicate step name, transition name, or `NodeId` inside a chart | `ERR_DUPLICATE_ID` / `ERR_DUPLICATE_DECL` |
| Transition referencing undeclared step in `from_steps` or `to_steps` | `ERR_UNDECLARED_SYMBOL` |
| Non-`BOOL` transition receptivity expression | `ERR_TYPE_MISMATCH` |
| `L` or `D` qualifier missing `TIME` duration, or non-`TIME` duration expression | `ERR_TYPE_MISMATCH` |
| Action coil `target` bound to non-`BOOL` variable, `CONST`, or read-only input tag | `ERR_TYPE_MISMATCH` / `ERR_CONST_WRITE` / `ERR_HARDWARE_LABEL_WRITE` |
| Direct assignment to read-only `<Step>.X`, `<Step>.T`, or `<Macro>.DONE` | `ERR_CONST_WRITE` |
| Invalid attribute access on a step (anything other than `.X`, `.T`, or `.DONE` on a macro-step) | `ERR_UNKNOWN_FIELD_ACCESS` |
| Macro-step referencing nonexistent sub-chart POU or forming a recursive macro-chart cycle | `ERR_UNRESOLVED_FUNCTION` / `ERR_RECURSION_DEPTH` |
| Blocking `WAIT` inside an SFC chart attached to `OB_HW_INT` / `OB_CYCLIC_INT` | `ERR_ISR_FORBIDDEN_OP` |
| Unresolved comment anchor or invalid `token_ordinal` | `ERR_COMMENT_ANCHOR` / `ERR_COMMENT_FIELD` |
| Cross-language export of an SFC action containing `Foreign` without `--allow-foreign` | `ERR_FOREIGN_EXPORT_BLOCKED` |

---

## 2. Per-Board Type Mapping

SFC receptivities and inline action bodies use the full ASL v2 static type system (`01` §2):

| SFC Construct | Allowed ASL Types | Board `[cpu] int_bits` Interaction |
|---|---|---|
| Transition `receptivity` | `BOOL` strictly (may combine contacts, comparisons, `<Step>.X`, `<Step>.T >= T#...`, `<Macro>.DONE`, FB `.Q`) | Fixed IEC widths in comparisons |
| Step attributes `.X` / `.T` / `.DONE` | `BOOL` / `TIME` (64-bit microseconds) / `BOOL` | Independent of `int_bits` |
| Action `duration` (`L`, `D`) | `TIME` strictly (`T#500ms`, `T#2s`, or `TIME` variable) | Independent of `int_bits` |
| Action `target` coil | `BOOL` writable variable or output hardware tag | Independent of `int_bits` |
| Action inline `body: Vec<Stmt>` | Full `asl-core@1` statement/expression subset (`Assign`, `Call`, `FbInvoke`, `If`, `Switch`, `For`, `While`, `Foreign`) | Projections to C++ honor `[cpu] int_bits` (`16` AVR / `32` PLC) |

---

## 3. Numeric Codecs & Intrinsic Mapping

- Transition receptivities and inline action bodies access analog inputs and PWM outputs through the exact same ASL numeric contract (`M = 10⁹`, `01` §3):
  - Reading a bound analog sensor (e.g., `PressRaw` on `plc-1200-open-1214` `IW64` `plc_span_27648`, or `PotPin` on `arduino-uno-r3` `A0` `uno_10bit`) yields `CODE<codec_id>`, convertible via `CODE_TO_ANALOG` / `ANALOG_TO_CODE` / `ANALOG_TO_DUTY`.
  - Example receptivity on a 4–20 mA pressure transmitter over `IW64`: `CODE_TO_ANALOG(PressRaw, 'plc_span_27648') >= 200000000` (2.0 V / 4 mA threshold) or native `PressRaw >= 5529`.
- **Reduced-Rational Cross-Platform Emission Rule:** When projecting an SFC chart to ST or C++ across boards with different ADC codecs $(D_1, n_{\max,1}) \to (D_2, n_{\max,2})$, the emitter **MUST NEVER** emit `M = 10⁹`. It emits the clamped reduced rational `min(n_max2, floor(n * D2_red / D1_red))` with 32-bit vs 64-bit intermediate integer width chosen by `n_max1 * D2_red < 2^32` and mandatory clamping for `span -> binned`.

---

## 4. Comment Mapping & `comment_styles`

All SFC annotations are stored verbatim in `[[boards.comments]]` anchored by `NodeId` (never inside `[boards.presentation]`):

| SFC Element | Anchor `NodeId` | `CommentPlacement` | Default `origin.style` |
|---|---|---|---|
| Chart Header | `Pou.id` | `Header` / `Title` | `Visual` / `Line` / `ParenBlock` |
| Step Note | `SfcStep.id` | `Title` / `Body` / `Leading` | `Visual` |
| Transition Note | `SfcTransition.id` | `Body` / `Leading` / `Trailing` | `Visual` |
| Action Note | `SfcAction.id` | `Body` / `Trailing` | `Visual` |
| Deleted Step / Transition / Action Comment | Enclosing `Pou.id` | `Dangling` | Preserved original (`INFO_COMMENT_ORPHANED`) |

- **Verbatim Conservation (`C1–C5`):** All comments survive SFC ↔ ST ↔ C++ projections with invariant verbatim text multisets, projection-only hazard avoidance (`*/`, `*)`, trailing `\`), Trojan-Source bidi escaping (`WARN_COMMENT_BIDI`), `comment_encoding` policy (`WARN_COMMENT_ENCODING`), and re-import idempotence via `matches_comment` (`C4`).

---

## 5. Module, Macro-Chart Hierarchy, Import & Pack Mapping

- **Macro-Step Sub-Charts (`04` §3.2 & §5.2):** A step declaring `macro_chart = Some("SubChartName")` references another `PouBody::Chart` POU in the same board program or an imported module/library. Sub-chart invocation is non-recursive (`ERR_RECURSION_DEPTH` on cycles).
- **Callable POUs & FBs inside Actions:** Action `body` statements MAY invoke `std.iec.*` FBs (`TON`, `CTU`, `UART_SEND`, `MB_CLIENT`), project `FC`/`FB` POUs, `Structured POU` blocks, or `F1` pack symbols (such as `assets/packs/plc-modbus-tcp.pack.toml`) strictly according to `[boards.program].imports` (platform isolation enforced: `ERR_UNRESOLVED_IMPORT` for unimported platform domains).
- **Cross-Language Export Guard:** Exporting an SFC chart containing `Stmt::Foreign` or `Expr::Foreign` inside any action or receptivity to a different language is blocked with `ERR_FOREIGN_EXPORT_BLOCKED` unless `allow_foreign_export` (`--allow-foreign`) is enabled, emitting a demarcated `"NOT TRANSLATED"` comment block.

---

## 6. Hardware Address & Tag Mapping

- Action targets (`N`/`S`/`R`/`L`/`D`/`P`/`P1`/`P0` coils), action bodies, and transition receptivities reference board I/O exclusively via declared logical tag labels in `[[boards.program.hardware]]` (zero machine magic comments).
- On `plc-1200-open-1214`, logical tags bind deterministically to `%IX0.0`..`%IX0.7` (`Ia.0`..`Ia.7`), `%QX0.0`..`%QX0.7` (`Qa.0`..`Qa.7`), `%IW64`, and `%IW66` via `BoardContext.port_to_iec` / `iec_to_port`.
- Electro-pneumatic sequence charts (e.g., `A+ B+ A- B-` driving 5/2 valves and sampling cylinder limit switches) bind valve coils and limit-switch contacts cleanly to hardware tags without any vendor-specific addressing.

---

## 7. Mandatory Round-Trip Class — `LOSSLESS_ASL` & Conformance Suite

`dendri-lang-sfc` MUST satisfy **`LOSSLESS_ASL`**:

1. **Structural Identity:** For every valid `PouBody::Chart(SfcChart)`:
   $$\text{parse}(\text{emit}(M, \text{ctx}), \text{ctx}) \equiv M$$
   preserving exact `NodeId`s, `initial` flags, `macro_chart` links, `priority` values, AND/OR topologies, all 8 Table 47 qualifiers + durations, inline action bodies, `evolve_to_stability`, and `[[boards.comments]]` multisets with zero persisted `__sfc_*` temporaries.
2. **Mandatory Conformance Suite (`04` §2, `05` §4):**
   - `proptest` verifying `LOSSLESS_ASL` across sequential, OR-divergence (explicit priorities), AND-divergence/convergence, and macro-step charts.
   - Exact IEC 60848 Rule 5 verification: simultaneous deactivate + activate keeps step ACTIVE; frozen Phase 1 receptivities; OR priority strictly selects lowest `priority` value; AND barrier waits for all upstream steps; instability loop guard catches zero-time cycles.
   - Exhaustive Table 47 qualifier tests (`N` auto-reset, `S` latch, `R` overriding both `N` and `S`, `L` time limit, `D` time delay, `P`/`P1`/`P0` edge pulses).
   - Macro-step Scan-1 initial entry test, `<MacroStep>.DONE` exit-step verification, and same-scan re-entry reset test (zero duplicate setup statements across round-trips).
   - Comment conservation (`C1–C5`), three-level `Foreign` preservation, and one genuine negative fixture per produced `ErrorCode`.
   - **Phase 6b Differential Gate:** Emitted ST from every Wave 1 GRAFCET/SFC golden scenario compiles via RuSTy (`plc`) and matches `dendri-vm` traces bit-for-bit.

---

## 8. `LanguageCapabilities` & Feature Matrix Contract

```rust
// Normative capability profile for `dendri-lang-sfc`
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

- **Auto-Generated Feature Matrix:** The published GRAFCET/SFC feature-support matrix MUST be generated directly from the `dendri-lang-sfc` conformance test suite via `dendri-frontend-core::conformance` and MUST NOT be maintained by hand (`04` §2).
