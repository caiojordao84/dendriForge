# Phase 0 — ASL contract freeze

**Status:** landed (merged). **MVP:** accepted (12/12).  
**ASL = Abstract Syntax Language** (frozen; do not rename), structured in **TOML**.  
**ASL version:** **`0.0.1`** (`metadata.asl_version` / `dendri_ast::ASL_VERSION`).

This folder documents the TOML-based IR contract that Phase 1 / MVP implement
(`dendri-parse` / `dendri-validate` / `dendri-emit` / `dendri-sim`).

| Document | Purpose |
|---|---|
| [`ir-schema.md`](./ir-schema.md) | Program + profile TOML IR |
| [`versioning.md`](./versioning.md) | ASL language tag `0.0.1` |
| [`error-codes.md`](./error-codes.md) | Unified `ERR_*` enum (+ legacy map) |
| [`layout.md`](./layout.md) | Repo layout: `content/` vs `legacy` `core/` |
| [`svg-readiness.md`](./svg-readiness.md) | SVG conventions, gap inventory, visual MVP |

**Type source of truth:** `crates/dendri-ast`  
**Stdlib data:** `crates/dendri-stdlib/data/catalog.toml`  
**Compliance spine:** `content/standards/bindings.toml`  
**Fixtures:** `content/fixtures/{golden,errors,examples}/`
