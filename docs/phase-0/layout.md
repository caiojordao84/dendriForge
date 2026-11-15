# Phase 0 content layout

## Decision (locked)

| Choice | Value |
|---|---|
| Program extension | **`.asl.toml`** (TOML envelope + structured statement AST) |
| Profile extension | **`.toml`** under `content/profiles/` |
| ASL versioning | **None** until MVP; then `0.0.1` (see [`versioning.md`](./versioning.md)). Legacy folder `core/asl/v010/` is historical path only. |
| New tree | **`content/`** — IR fixtures, sample profiles, standards, asset notes |
| Legacy TOON | **Left in place** at `core/**/*.toon` (read-only). Documented by `legacy/toon/README.md`. Not relocated in Phase 0 to keep the PR focused. |
| Visual MVP assets | Paths noted in `docs/phase-0/svg-readiness.md`; existing SVGs stay under `core/boards/` and `core/components/` until Phase 1 asset packing |

## Tree

```
dendriforge/
├── Cargo.toml                 # workspace skeleton
├── crates/
│   ├── dendri-ast/            # IR types (source of truth)
│   └── dendri-stdlib/         # IEC catalog data + alias resolve
├── content/
│   ├── fixtures/{golden,errors,examples}/
│   ├── profiles/{boards,components,screens}/   # sample TOML profiles
│   ├── standards/bindings.toml
│   └── assets/svg/README.md
├── docs/phase-0/              # this contract pack
├── core/                      # LEGACY TOON corpus (do not edit for new IR)
└── legacy/toon/README.md      # pointer + policy
```

## Migration policy

- Phase 0 exit does **not** require old `.toon` to parse.
- Phase 1 implements parsers against `.asl.toml` / profile `.toml` only.
- A later cleanup may `git mv core → legacy/toon/core`; until then treat `core/` as frozen.
