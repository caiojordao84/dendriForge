# dendriForge — Documentation

> ASL v0.1.0 — TOON Language Specification

This folder contains the formal specification documents for the **TOON** intermediate representation language used by dendriForge's parser and code generator.

---

## Documents

| File | Description |
|---|---|
| [stdlib.md](./stdlib.md) | TOON Standard Library — full function dictionary with IEC 61131-3 canonical names, aliases, signatures, C++ and MicroPython codegen mappings, and per-function trigger rules |
| [grammar.md](./grammar.md) | TOON Formal Grammar — EBNF grammar for the full TOON language, alias normalization table, parse error catalogue, and symbol table definition |

---

## Architecture Overview

```
 Source Language          TOON IR (ASL)           Target Language
 (C++, MicroPython,  →   (this spec)         →   (C++, MicroPython,
  IEC ST, Codesys...)                              IEC ST, Codesys...)
```

TOON acts as the **Rosetta Stone** IR. Any supported source language is parsed into TOON. Any supported target language is generated from TOON. This decouples parsers from generators and reduces the problem from N×M to N+M.

---

## Key Design Decisions

- **IEC 61131-3 canonical naming** — all stdlib functions use IEC names. Generic aliases are accepted by the parser but normalized at parse time (Pass 0).
- **Type inference** — function parameter types are inferred from the symbol table built during the full-document parse pass. No explicit type annotations required in `functions:`.
- **Typed `data[]` header** — `data[N|]{name|type|scope|init_value|unit}` provides unambiguous typed declarations for all generators.
- **Label resolution** — hardware pin integers are always resolved to their `hardware[]` label constant. Raw integers are never emitted in generated output.
- **`TIME_DIFF` enforcement** — raw subtraction of time-typed `DWORD` values is a `PARSE_ERROR`. `TIME_DIFF(a, b)` is always required.
- **`VENDOR_EXTENSION` fallback** — any unresolvable function emits a `VENDOR_EXTENSION("CATEGORY:DETAIL")` marker and a `# WARN` comment. Nothing is silently skipped.

---

## Target Platforms

```
TARGET_PLATFORM ∈ {
  arduino-avr       | arduino-esp32      |
  micropython-esp32 | micropython-rp2    |
  bare-metal-stm32  | codesys            |
  tia-portal
}
```

---

## Version

| Field | Value |
|---|---|
| ASL version | 0.1.0 |
| Editor | dendriForge |
| Spec status | Draft |
