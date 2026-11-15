# Phase 0 — ASL contract freeze

**ASL = Abstract Syntax Language** (frozen; do not rename).

This folder documents the TOML-based IR contract that Phase 1 will implement
(`dendri-parse` / `dendri-validate` / `dendri-emit` / `dendri-sim`).

| Document | Purpose |
|---|---|
| [`ir-schema.md`](./ir-schema.md) | Program + profile TOML IR |
| [`error-codes.md`](./error-codes.md) | Unified `ERR_*` enum (+ legacy map) |
| [`layout.md`](./layout.md) | Repo layout: `content/` vs `legacy` `core/` |
| [`svg-readiness.md`](./svg-readiness.md) | SVG conventions, gap inventory, visual MVP |

**Type source of truth:** `crates/dendri-ast`  
**Stdlib data:** `crates/dendri-stdlib/data/catalog.toml`  
**Compliance spine:** `content/standards/bindings.toml`  
**Fixtures:** `content/fixtures/{golden,errors,examples}/`
