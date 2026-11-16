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
