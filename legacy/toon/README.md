# Legacy TOON corpus (read-only)

> **Historical only.** TOON is **obsolete** as DendriForge IR. Live programs use
> ASL in **TOML** (`.asl.toml`). See `docs/phase-0/`. The folder name `core/asl/v010/`
> is a path artifact — **not** the current ASL version (ASL is unversioned until MVP → `0.0.1`).

The pre-migration TOON library remains at:

```
core/asl/v010/**/*.toon
core/boards/*.toon
core/components/*.toon
core/screens/*.toon
```

Dialect rule Markdown under `core/**/rules/` (including `toon-dialect-core.md` and
profiles that cite `ASLversion: 0.1.0`) is likewise **legacy documentation** for that
corpus — not the Phase 0 / MVP contract.

**Policy**

- Do **not** author new programs/profiles in TOON.
- Do **not** treat TOON as a supported Phase 1 / MVP input.
- New IR lives under `content/` (`.asl.toml` programs, `.toml` profiles).
- A later cleanup may relocate `core/` → `legacy/toon/core/`; until then this
  directory documents the freeze without a bulk move.
