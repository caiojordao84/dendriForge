# ASL fixtures (TOML IR)

ASL programs for the live IR. **No ASL version field** — unversioned until MVP acceptance (`0.0.1`).

| Tree | Contents |
|---|---|
| `golden/` | Positive goldens **G01–G21** (migrated from `core/asl/v010/golden/`) — validate + round-trip |
| `errors/` | Negative corpus E01–E07 with `metadata.expect_error` (optional `fixture_inject`) |
| `examples/` | All ten v010 examples migrated to `.asl.toml` (sim may be blocked) |
| `sim/` | Deterministic `dendri-sim` fixtures (`S01`–`S13`; S08=sc1 blink; S09 strings; S10 conveyor/`IN_RANGE`; S11 retain; S12 I2C/SPI xfer; S13 SQRT/LOG) |
| `projects/` | Single-file `.dendri.toml` projects (hardware + wiring + sources + ASL) |
| `frontends/` | P1 language sources (C++ / MicroPython / ST) for parse→ASL→sim acceptance |
| `ladder/` | Ladder diagram TOML (`L01`–`L05`; L05 = scenario 6 electro-pneumatic twin) |
| `sfc/` | SFC/GRAFCET diagram TOML (`G01`–`G03`; G03 = scenario 6 sequence) |
| `canvas/` | Live SVG sim projects (`C01` button→LED; `C02` scenario 1 modes) |

Extensions: **`.asl.toml`** (programs), **`.dendri.toml`** (simulation projects). Legacy TOON sources remain under `core/asl/v010/` (read-only; historical path name only — see `legacy/toon/README.md`).

Re-migrate from TOON (skips hand-protected fixtures): `python3 tools/migrate_asl_toon.py`.

Structural load: `cargo test -p dendri-ast --test load_fixtures`.  
Validate / round-trip: `cargo test -p dendri-validate -p dendri-emit`.  
Scenario matrix 1–12: `cargo test -p dendri-frontends --test mvp_scenarios`.
