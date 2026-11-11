# Changelog

## Unreleased: Windows port

- **Windows 10/11 (x64)** build: DDC/CI through the Monitor Configuration API, portable ZIP and installer (see
  `docs/building-windows.md`). Linux behaviour is unchanged.
- **Two monitors in one window:** a session per monitor, a monitor switch, aliases, *Identify monitors*; every write goes
  to the selected monitor only. Monitors that are plugged or unplugged are noticed at once.
- **Safety:** read-only at every start; writes need an explicit consent and a write level (1 brightness, contrast,
  volume; 2 other picture settings; 3 game aids, PIP, LED, OSD, inputs); the dangerous writes (resets, button lock,
  power modes) are blocked; every write checks that the handle still belongs to the same monitor; a stop switch in the
  window and in the tray.
- **Windows integration:** tray icon with monitor menu, autostart (`--minimized`), one instance, system theme,
  resolution and refresh rate (with confirmation and automatic undo), HDR per display, scaling shown.
- **Development:** `tools/ddc_probe`, a simulated monitor for tests, `TITAN_FAKE_MONITORS` (debug builds),
  `--no-ddc` / `TITAN_NO_DDC=1`, `BLUEPRINT_COMPILER` and the committed `src/ui/window.ui`.
