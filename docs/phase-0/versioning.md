# ASL versioning policy (Phase 0 lock)

**ASL is unversioned at this stage.**

There is no ASL language version string (`0.1.0`, `v010`, `ASLversion`, `asl_version`,
schema version fields, etc.) in the Phase 0 IR contract.

| Until | Policy |
|---|---|
| Now → parsers + MVP | No ASL version field in `.asl.toml`, profiles, stdlib catalog, or `dendri-ast` metadata |
| After MVP | Introduce first tag **`0.0.1`** |

## What this does *not* change

- Legacy path `core/asl/v010/` stays as a historical TOON folder name (read-only). Do not rename for this policy.
- Draft docs under `docs/grammar.md` / `docs/stdlib.md` are **superseded** semantic drafts (historical TOON surface). They are not the live version authority; banners in those files point here.
- Cargo `workspace.package.version` is crate packaging only, not an ASL release.

## IR consequence

`[metadata]` no longer carries `asl_version` or document `version`. Required identity fields for validate are `project_name` (and conventionally `editor` / `author`).
