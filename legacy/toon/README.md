# Legacy TOON corpus (read-only)

The pre-migration TOON library remains at:

```
core/asl/v010/**/*.toon
core/boards/*.toon
core/components/*.toon
core/screens/*.toon
```

**Policy**

- Do **not** author new programs/profiles in TOON.
- Do **not** treat TOON as a supported Phase 1 input.
- New IR lives under `content/` (`.asl.toml` programs, `.toml` profiles).
- A later cleanup may relocate `core/` → `legacy/toon/core/`; until then this
  directory documents the freeze without a bulk move.
