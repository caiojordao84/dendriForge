# dendriForge Fuzzing Harness (CI Gate 11)

This standalone crate (`excluded` from the root Cargo workspace so stable builds stay clean) houses `cargo-fuzz` targets for every parser in `dendriForge`.

## Targets
- **Phase 0:** `ledger_toml` — fuzzes deferral ledger, milestone, and provenance TOML parsers.
- **Phase 1+:** dedicated targets will be added here for `dendri-expr`, `dendri-project`, `dendri-assets` profiles, and every language frontend (`st`, `cpp`, `ladder`, `sfc`).

## Local Usage (Requires Nightly + `cargo-fuzz`)
```bash
cd fuzz
cargo +nightly fuzz run ledger_toml -- -max_total_time=60
```
