# Unified ASL error codes

**Frozen scheme:** `ERR_*` (matches the error corpus).  
**Source of truth:** `dendri_ast::ErrorCode` in `crates/dendri-ast/src/error.rs`.

Legacy `PARSE_ERROR_00x` IDs from `docs/grammar.md` are **aliases only** —
validators must emit the `ERR_*` id.

## Catalogue

| Code | Meaning |
|---|---|
| `ERR_METADATA_MISSING_FIELD` | Required `[metadata]` key missing (e.g. `project_name`; `asl_version` defaults to `0.0.1` when omitted) |
| `ERR_BLOCK_COUNT_MISMATCH` | Declared vs actual entry/step count |
| `ERR_HARDWARE_PIN_CONFLICT` | Duplicate pin in `[[hardware]]` |
| `ERR_DATA_UNKNOWN_TYPE` | Unknown `[[data]]` type token |
| `ERR_UNDECLARED_VARIABLE` | Identifier not in data/hardware |
| `ERR_TYPE_MISMATCH` | Expression / call type error |
| `ERR_CONST_WRITE_VIOLATION` | Write to `CONST` scope |
| `ERR_FUNCTION_ARITY_MISMATCH` | Wrong call arity |
| `ERR_RAW_TIME_SUBTRACTION` | Use `TIME_DIFF` instead of `a - b` on time DWORDs |
| `ERR_PIN_LABEL_REQUIRED` | Raw pin int emit forbidden; resolve label |
| `ERR_UNRESOLVED_ISR` | ISR name not in `[[functions]]` |
| `ERR_UNRESOLVED_FUNCTION` | Call not in stdlib or user functions |
| `ERR_VENDOR_EXTENSION_REQUIRED` | Unresolved mapping needs `VENDOR_EXTENSION` |
| `ERR_NESTED_LOOP_DEPTH` | `FOR` nesting > 4 |
| `ERR_RETAIN_WRITE_WITHOUT_SYS` | RETAIN/PERSISTENT write without SYS helper |
| `ERR_ISR_HEAP_ALLOCATION` | Heap call inside `isr_*` |
| `ERR_UNKNOWN_IR_KIND` | Unknown TOML `kind` tag |
| `ERR_IR_DESERIALIZE` | Envelope failed structural deserialize |

## Legacy map (`PARSE_ERROR_*` → `ERR_*`)

| Legacy | Frozen |
|---|---|
| `PARSE_ERROR_001` | `ERR_UNDECLARED_VARIABLE` |
| `PARSE_ERROR_002` | `ERR_TYPE_MISMATCH` |
| `PARSE_ERROR_003` | `ERR_RAW_TIME_SUBTRACTION` |
| `PARSE_ERROR_004` | `ERR_PIN_LABEL_REQUIRED` |
| `PARSE_ERROR_005` | `ERR_UNRESOLVED_ISR` |
| `PARSE_ERROR_006` | `ERR_UNRESOLVED_FUNCTION` |
| `PARSE_ERROR_007` | `ERR_VENDOR_EXTENSION_REQUIRED` |
| `PARSE_ERROR_008`…`012` | `ERR_BLOCK_COUNT_MISMATCH` |
| `PARSE_ERROR_013` | `ERR_NESTED_LOOP_DEPTH` |
| `PARSE_ERROR_014` | `ERR_RETAIN_WRITE_WITHOUT_SYS` |
| `PARSE_ERROR_015` | `ERR_ISR_HEAP_ALLOCATION` |

Negative fixtures set `metadata.expect_error` to the frozen `ERR_*` string.
