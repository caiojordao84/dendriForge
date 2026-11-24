# dendriForge Fuzzing Harness (CI Gate 11)

This standalone crate (`excluded` from the root Cargo workspace so stable builds stay clean) houses `cargo-fuzz` targets for every parser in `dendriForge`.

## Targets
- **Phase 0:** `ledger_toml` — fuzzes deferral ledger, milestone, and provenance TOML parsers.
- **Phase 1:**
  - `expr_parser` — fuzzes `dendri-expr` Pratt parser, `LValue` parser, and `print_expr` round-trip.
  - `project_toml` — fuzzes `dendri-project` strict TOML loader and serializer.
- **Phase 4:** `asset_profile` — fuzzes `dendri-assets` board/component/review TOML parsers, SVG `<g id="df-pins">` validation, and canonical SVG sanitization.
- **Phase 6 (Wave 1 Frontends):**
  - `lang_st` — fuzzes `dendri-lang-st` IEC 61131-3 ST parser & emitter round-trip.
  - `lang_cpp` — fuzzes `dendri-lang-cpp` Arduino C++ parser & emitter round-trip.
  - `lang_ladder` — fuzzes `dendri-lang-ladder` IEC 61131-3 Ladder parser & emitter round-trip.
  - `lang_sfc` — fuzzes `dendri-lang-sfc` IEC 60848 GRAFCET/SFC parser & emitter round-trip.

## Local Usage (Requires Nightly + `cargo-fuzz`)
```bash
cd fuzz
cargo +nightly fuzz run ledger_toml -- -max_total_time=60
cargo +nightly fuzz run expr_parser -- -max_total_time=60
cargo +nightly fuzz run project_toml -- -max_total_time=60
cargo +nightly fuzz run asset_profile -- -max_total_time=60
cargo +nightly fuzz run lang_st -- -max_total_time=60
cargo +nightly fuzz run lang_cpp -- -max_total_time=60
cargo +nightly fuzz run lang_ladder -- -max_total_time=60
cargo +nightly fuzz run lang_sfc -- -max_total_time=60
```
