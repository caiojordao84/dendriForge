# Ladder fixtures (DEN-8 / DEN-12)

| File | Intent |
|---|---|
| `L01-blink-actions.ladder.toml` | Action-cell blink → sim `SET_BOOL` / `WAIT_TIME` |
| `L02-contact-coil.ladder.toml` | Series NO contact + coil + pin write |
| `L03-parallel-or.ladder.toml` | One-level parallel OR rail → coil + pin write |
| `L04-iec-fbs.ladder.toml` | Honest `R_TRIG` / `TON` / `TOF` / `CTU` lower + pin write |

Diagram text uses the same TOML schema as `[[asl.ladder]]` / `LadderDocument`.
