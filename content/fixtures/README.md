# ASL fixtures (TOML IR)

ASL programs for the live IR. **No ASL version field** — unversioned until MVP acceptance (`0.0.1`).

| Tree | Contents |
|---|---|
| `golden/` | Positive goldens — MVP: G01, G03, G05, G08, G17 |
| `errors/` | Negative corpus E01–E07 with `metadata.expect_error` |
| `examples/` | Migrated examples (basic-blink) |

Extension: **`.asl.toml`**. Legacy TOON sources remain under `core/asl/v010/` (read-only; historical path name only — see `legacy/toon/README.md`).

Structural load test: `cargo test -p dendri-ast --test load_fixtures`.
