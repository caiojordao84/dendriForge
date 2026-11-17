# dendriForge — Documentation

> **Live IR:** ASL = **Abstract Syntax Language**, structured in **TOML** (`.asl.toml`).  
> **Versioning:** ASL language tag **`0.0.1`** (`metadata.asl_version`). Do not treat `0.1.0` / `0.1.1` / `v010` as current.  
> **Authority:** [`phase-0/`](./phase-0/) + Rust types in `crates/dendri-ast`.

This folder holds the Phase 0 contract, the living implementation plan, and **superseded** semantic drafts from the pre-TOML era.

---

## Live documents (use these)

| File | Description |
|---|---|
| [phase-0/README.md](./phase-0/README.md) | Phase 0 contract index — TOML IR, versioning, errors, layout, SVG readiness |
| [phase-0/ir-schema.md](./phase-0/ir-schema.md) | Program + profile TOML IR schema |
| [phase-0/versioning.md](./phase-0/versioning.md) | ASL language tag `0.0.1` |
| [Implementation Plan.md](./Implementation%20Plan.md) | Living plan — Phase 0 landed; MVP accepted; ASL `0.0.1` |

**Type source of truth:** `crates/dendri-ast`  
**Stdlib catalog:** `crates/dendri-stdlib/data/catalog.toml`  
**Fixtures:** `content/fixtures/`  
**Legacy TOON (historical):** `core/**/*.toon` — see [`../legacy/toon/README.md`](../legacy/toon/README.md)

---

## Superseded semantic drafts (not the live surface format)

These Markdown drafts capture IEC naming, stdlib intent, and parse/validate ideas. Their original **TOON** surface syntax and any `ASL v0.1.0` / `ASLversion` wording are **obsolete**. Semantics migrate into the TOML IR and `catalog.toml`; do not reintroduce TOON as a supported IR.

| File | Role today |
|---|---|
| [stdlib.md](./stdlib.md) | Historical stdlib dictionary → prefer `crates/dendri-stdlib/data/catalog.toml` |
| [grammar.md](./grammar.md) | Historical EBNF of the old surface → prefer `phase-0/ir-schema.md` + `dendri-ast` |

---

## Architecture Overview

```
 Source Language                 ASL IR (TOML)              Target Language
 (C++, MicroPython, ST,    →   (.asl.toml — this        →  (C++, MicroPython,
  Ladder, SFC/GRAFCET …)        contract)                   ST, Ladder, SFC …)
```

ASL-in-TOML is the **Rosetta Stone** IR. Any supported source language is parsed into ASL. Any supported target language is generated from ASL. This decouples parsers from generators and reduces the problem from N×M to N+M.

**MVP languages (P1):** C++, MicroPython, Structured Text (ST), Ladder, SFC/GRAFCET.  
**Post-MVP (P2):** Rust scripting. App stack remains pure Rust.

---

## Key Design Decisions

- **TOML only** — programs are `.asl.toml`; profiles are `.toml`. TOON is obsolete as IR.
- **Single-file project (MVP)** — one TOML file holds hardware, wiring, original scripts, and ASL.
- **IEC 61131-3 canonical naming** — stdlib functions use IEC names; aliases normalize at parse time.
- **Shared AST** — one Rust AST for parsers, generators, and simulators (`dendri-ast`).
- **Ladder / GRAFCET** — visual editors sync bidirectionally with the same TOML in real time.
- **ASL `0.0.1`** — sole language tag via `metadata.asl_version` / `dendri_ast::ASL_VERSION`.

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

## Version / status

| Field | Value |
|---|---|
| ASL version | **`0.0.1`** |
| Phase 0 | Landed (contract freeze) |
| MVP | Accepted (12/12) |
| Editor | dendriForge |
| Spec status | Phase 0 contract + MVP build |
