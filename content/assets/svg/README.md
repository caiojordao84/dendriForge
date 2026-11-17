# SVG asset staging

Phase 0 does not relocate binary assets. See `docs/phase-0/svg-readiness.md`.

Planned Phase 1 layout:

```
content/assets/svg/
  boards/arduino-uno-r3.svg
  components/led.svg
  components/cmd-pushbutton-no-start.svg   # authored DEN-8
  screens/
```

MVP trio staged here; seeds also under `core/boards/*.svg` and `core/components/{led,cmd-pushbutton-no-start}.svg`.

`dendri-canvas` (DEN-13) prefers these `content/assets/svg/` paths and falls back to `core/`. Profile `svg_path` still names `core/` until DEN-14 migration.
