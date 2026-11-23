# SFC / GRAFCET fixtures (DEN-8 / DEN-12 / DEN-19)

| File | Intent |
|---|---|
| `G01-blink-two-step.sfc.toml` | Two-step blink chart → step-token state machine |
| `G02-and-divergence.sfc.toml` | AND fork then AND join (multi `to` / multi `from`) |
| `G03-electro-pneumatic.sfc.toml` | A+ B+ A- B- sequence (scenario 6) |
| `G04-elevator-nested.sfc.toml` | 2-floor elevator; `S_doors.enclosed` door macro-step (scenario 5) |

Diagram text uses the same TOML schema as `[[asl.grafcet]]` / `GrafcetDocument`.
Nested / macro-steps via `GrafcetStep.enclosed` (DEN-19).
