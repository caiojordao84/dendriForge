# ASL fixtures (TOML IR)

ASL programs for the live IR. **No ASL version field** — unversioned until MVP acceptance (`0.0.1`).

| Tree | Contents |
|---|---|
| `golden/` | Positive goldens — MVP: G01, G03, G05, G08, G17 (Phase 0 structural + Phase 1 validate/round-trip) |
| `errors/` | Negative corpus E01–E07 with `metadata.expect_error` (optional `fixture_inject`) |
| `examples/` | Migrated examples (basic-blink) |
| `sim/` | Deterministic `dendri-sim` trace fixtures (`S01`, `S02`) |
| `projects/` | Single-file `.dendri.toml` projects (hardware + wiring + sources + ASL) |
| `frontends/` | P1 language sources (C++ / MicroPython / ST) for parse→ASL→sim acceptance |

Extensions: **`.asl.toml`** (programs), **`.dendri.toml`** (simulation projects). Legacy TOON sources remain under `core/asl/v010/` (read-only; historical path name only — see `legacy/toon/README.md`).

Structural load test: `cargo test -p dendri-ast --test load_fixtures`.  
Phase 1: `cargo test --workspace` (validate, round-trip, sim snapshots).
