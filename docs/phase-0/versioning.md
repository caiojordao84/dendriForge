# ASL versioning policy

**Current ASL language version: `0.0.1`.**

Assigned after MVP acceptance (fixture round-trip + deterministic simulation).
This is the first ASL tag. Do not treat legacy `0.1.0` / `0.1.1` / `v010` /
TOON `ASLversion` as current.

| Field | Where | Value |
|---|---|---|
| `asl_version` | `[metadata]` in `.toml` and `[asl.metadata]` in `.toml` | `"0.0.1"` |
| Rust constant | `dendri_ast::ASL_VERSION` | `"0.0.1"` |

## Sole scheme

One language version string only: **`asl_version`**. No parallel schema version,
document `version`, Cargo-as-ASL, or `ASLversion` spellings in the live IR.

| Artifact | Policy |
|---|---|
| `.toml` / embedded ASL | Carry `metadata.asl_version = "0.0.1"` (serde defaults to current if omitted) |
| Profiles (`content/profiles/**/*.toml`) | Hardware/profile docs — **no** language version field |
| Stdlib `catalog.toml` | Documents ASL `0.0.1`; no separate catalog language version |
| Cargo `workspace.package.version` | Crate packaging only — **not** an ASL release |

## What this does *not* change

- Legacy path `core/asl/v010/` stays as a historical TOON folder name (read-only).
- Draft docs under `docs/grammar.md` / `docs/stdlib.md` remain superseded semantic drafts.
- Required identity field for validate remains `project_name` (plus conventionally `editor` / `author`).
