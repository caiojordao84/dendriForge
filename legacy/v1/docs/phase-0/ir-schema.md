# ASL TOML IR schema (Phase 0)

**Language:** ASL = **Abstract Syntax Language**  
**Program files:** `*.toml`  
**Rust source of truth:** `crates/dendri-ast`  
**Profiles:** pure TOML under `content/profiles/`

## Design split

| Document kind | Format |
|---|---|
| Boards / components / screens | Pure TOML tables (`params`, `[[gpio]]`, `[[ports]]`, …) |
| Programs | TOML envelope (`metadata`, `hardware`, `data`) + **typed statement AST** arrays (`setup`, `loop`, `functions`) |

Statements are **not** free-form script strings. Prefer tagged tables:

```toml
[[setup]]
kind = "call"
name = "CFG_DIGITAL_OUTPUT"
args = [{ kind = "uint", value = 13 }]
```

Escape hatch (sparingly): `{ kind = "expr_text", text = "…" }` for expressions
too deep for inline tables; Phase 1 must parse these into the same `Expr` enum.

## Program envelope

```toml
[metadata]
asl_version = "0.0.1"          # dendri_ast::ASL_VERSION — sole ASL language tag
project_name = "…"
editor = "dendriForge"
author = "…"
# expect_error = "ERR_…"       # negative fixtures only
# notes = "…"
# See versioning.md — do not add parallel version / ASLversion / schema fields.

[[hardware]]
pin = 13
hw_type = "digital"            # digital | analog | bool
io_mode = "DO"                 # DI | DO | BI (aliases: input/output/bi)
label = "ledPin"
target_role = "digital-out"

[[data]]
name = "DELAY"
type = "TIME"                  # BOOL INT DINT UINT REAL LREAL TIME STRING BYTE WORD DWORD
scope = "CONST"                # CONST VAR RETAIN PERSISTENT TEMP EXTERN
init = "T#500ms"
unit = "ms"

[[setup]]
# Stmt…

[[loop]]
# Stmt…   (Rust field: Program.loop_body)

[[functions]]
name = "helper"
params = ["a", "b"]
body = [
  { kind = "call", name = "SET_BOOL", args = [{ kind = "uint", value = 13 }, { kind = "var", name = "a" }] },
]

# Reserved (empty in Phase 0 MVP fixtures):
# [[ladder]] …
# [[grafcet]] …
```

## Statement `kind` tags

| `kind` | Fields |
|---|---|
| `assign` | `target`, `value` (Expr) |
| `call` | `name`, `args` (Expr[]) |
| `if` | `condition`, `then`, `elif`[{condition,then}], `else` |
| `while` | `condition`, `body` |
| `for` | `counter?`, `to`, `body` |
| `return` | `value?` |
| `exit` / `continue` | — |
| `vendor_extension` | `reason` |

## Expression `kind` tags

| `kind` | Fields |
|---|---|
| `bool` / `uint` / `int` / `real` / `string` / `time` / `hex` | `value` |
| `var` | `name` |
| `call` | `name`, `args` |
| `unary` | `op` (`not`\|`neg`\|`bit_not`), `expr` |
| `binop` | `op`, `left`, `right` |
| `lit` | nested `value` Literal |
| `expr_text` | `text` (Phase 1 parse) |

Binary ops: `add sub mul div mod eq ne lt le gt ge and or xor bit_and bit_or bit_xor shl shr`.

## Ladder / GRAFCET

Types in `dendri_ast::{ladder,grafcet}` serialize as `[[ladder]]` / `[[grafcet]]`.
DEN-8 populates them via visual editors + `dendri-sync` (diagram ↔ TOML ↔ ASL).
See `docs/phase-1/ladder-sfc.md`.

## Profile sketch (board)

See `content/profiles/boards/arduino-uno-r3.toml` — identification, specs,
`[[power_pins]]`, `[[gpio]]`, peripherals, image path (asset optional until renderer).

## Profile sketch (component)

See `content/profiles/components/led.toml` and
`content/profiles/components/cmd-pushbutton-no-start.toml`.
