# Changelog

This repository ([Dreyark/titan-control-windows](https://github.com/Dreyark/titan-control-windows)) continues [mkarenko/titan-control](https://github.com/mkarenko/titan-control)
from its version 1.0.1 (MIT license, original copyright kept). Earlier history is the original project's.

## 1.1.0 (2026-11-12)

- **Writing is enabled from the first start**, as in the Linux version. The read-only start, the consent dialog, the
  write levels and the "stop writing" switch (window and tray) of the first Windows build were removed. Kept: one
  monitor per write (identity check before the first write of a connection), read-back and correction, and the
  blocked `22 = 0` and `F0`.
- Windows installer (`TitanControl-1.1.0-setup.exe`) and portable ZIP are attached to the GitHub release.

### Windows port (first build, before 1.1.0)

- **Windows 10/11 (x64)** build: DDC/CI through the Monitor Configuration API, portable ZIP and installer (see
  `docs/building-windows.md`). Linux behaviour is unchanged.
- **Two monitors in one window:** a session per monitor, a monitor switch, aliases, *Identify monitors*; every write goes
  to the selected monitor only. Monitors that are plugged or unplugged are noticed at once.
- **Safety:** every write checks that the handle still belongs to the same monitor; the dangerous writes (resets,
  button lock, power modes, `22 = 0`, `F0`) were blocked. (The read-only start and write levels of this build were removed in 1.1.0.)
- **Windows integration:** tray icon with monitor menu, autostart (`--minimized`), one instance, system theme,
  resolution and refresh rate (with confirmation and automatic undo), HDR per display, scaling shown.
- **Development:** `tools/ddc_probe`, a simulated monitor for tests, `TITAN_FAKE_MONITORS` (debug builds),
  `--no-ddc` / `TITAN_NO_DDC=1`, `BLUEPRINT_COMPILER` and the committed `src/ui/window.ui`.
