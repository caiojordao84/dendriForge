# SFC / GRAFCET fixtures (DEN-8 / DEN-12)

| File | Intent |
|---|---|
| `G01-blink-two-step.sfc.toml` | Two-step blink chart → step-token state machine |
| `G02-and-divergence.sfc.toml` | AND fork then AND join (multi `to` / multi `from`) |
| `G03-electro-pneumatic.sfc.toml` | A+ B+ A- B- sequence (scenario 6 subset) |

Diagram text uses the same TOML schema as `[[asl.grafcet]]` / `GrafcetDocument`.
Nested / macro-steps remain deferred.
