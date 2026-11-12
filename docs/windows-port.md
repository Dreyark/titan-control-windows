# The Windows port: design and status

Titan Control runs on Windows 10/11 (x64) next to the Linux version. This repository ([Dreyark/titan-control-windows](https://github.com/Dreyark/titan-control-windows)) is an
independent version of [mkarenko/titan-control](https://github.com/mkarenko/titan-control) 1.0.1 (MIT): the protocol work, the monitor
documentation and the Linux version are the original author's, the port described here is not meant to be sent back to him.
This page explains how the port is built, what was checked on real hardware and what was not. Build instructions are in [building-windows.md](building-windows.md).

## What changed, in one picture

```
 UI (GTK 4 + libadwaita, same code)      src/main.rs, src/multi.rs, src/ui/
        │  WorkerCmd (to the selected monitor)            ▲ SessionEvent { monitor, UiCmd }
        ▼                                                  │
 Router ── one Session per monitor ────────────────────────┘          src/monitor/session.rs
        │
        ▼
 Worker (queue, scheduled reads, writes + read-back)                   src/monitor/worker.rs
        │
        ▼
 MsiDdc (protocol: pacing, retries, settle, vendor registers, table)   src/monitor/ddc.rs
        │   identity check / dangerous-write guard
        ▼
 VcpTransport (one frame per call)                                     src/monitor/transport.rs
   ├─ LinuxI2c   raw I²C (`/dev/i2c-*`)        src/platform/linux.rs
   └─ WinDxva2   Monitor Configuration API     src/platform/windows.rs
```

The protocol layer is the one of the Linux version (timing constants and sequences are unchanged); only the transport
differs. On Linux a single session is started by the old code path, so its behaviour is the same as before.

## Decisions

- **Transport.** The Windows Monitor Configuration API (`dxva2`) is used for reads and writes. The manufacturer's own
  Windows tool does the same, and a spike (`tools/ddc_probe`) showed that plain reads, `GET 99 → GET code` reads and the
  firmware code (`FE = 0000/5101`) work. Windows validates frames itself, so a lost or invalid reply is one error kind
  and the retry logic of the protocol layer deals with it.
- **Two identical monitors.** The EDID of both has the same name, product code and serial number, so the identity
  (`MonitorId`) is the Windows device path plus the EDID data. The cache is keyed by it; an old cache (keyed by name)
  is migrated only when the name is unambiguous. Every write goes to exactly one session, and before the first write of
  a connection the handle's identity is checked again (a renumbered `\\.\DISPLAYn` blocks the write).
- **One bus lock** serialises single exchanges of different monitors; each monitor keeps its own request pacing.
- **Safety.** Writing is on from the start, as on Linux. Before the first write of a connection the handle's identity
  is checked, every write is read back, and `22 = 0` and `F0` are blocked (the other dangerous settings, e.g. resets, are
  written when the window offers them, as on Linux). The first Windows build had a read-only start with write levels 0-3 (consent per level, a stop
  switch); they were removed in 1.1.0 (`WRITE_LEVELS_ENABLED` in `src/monitor.rs` switches the code back on).
- **Hotplug.** A hidden window receives `WM_DISPLAYCHANGE`, `WM_DEVICECHANGE` and resume messages (plus a periodic
  scan); a monitor that appears or comes back gets a new session, one that disappears leaves the list.
- **DPI.** GTK 4 on Windows is DPI aware (sharp text) but scales only fonts, not widgets. The window therefore opens
  larger on a scaled screen and the font defaults to 11 pt (*Preferences → Interface size*).
  `TITAN_BITMAP_SCALING=1` lets Windows scale the whole window instead (soft text, exact proportions).
- **Display settings.** Resolution and refresh rate of the selected screen (tested with the driver, tried for 15 s and
  kept only after confirmation), HDR per display; scaling is read-only (Windows has no public call for it).
- **Windows integration.** Tray (`tray-icon`, own thread), autostart (`HKCU\…\Run`, `--minimized`), one instance (named
  mutex and event), system theme from the registry.

## Verified on hardware (two P275MV PLUS Enhanced, DisplayPort, NVIDIA GPU)

Reads, `GET 99` reads, firmware code; switching between the monitors; writes of level 1, 2 and 3 settings (brightness,
contrast, Low Blue Light, gamma, Night Vision, Local Dimming, DyDs, color temperature, saturation, LED brightness, FPS
counter, stopwatch, crosshair, volume) with read-back, and that a write to one monitor never reaches the other; unplug
and replug of a cable; change of the picture mode in the monitor shown in the window.

## Not verified

- **Linux:** the Linux-only code (`cfg(unix)`: raw I²C, EDID over I²C, DRM scan) was moved and adapted but could not be
  compiled during the port. Please build and test it before relying on it.
- **Profile table over Dxva2.** Both test monitors only reported *Default* profiles (odd codes), where settings are read
  one by one, so reading the Custom profile table through Dxva2 has not been exercised on hardware (the protocol layer
  and its retries are tested against a simulated monitor).
- Changing resolution, refresh rate and HDR from the window; autostart after a reboot; the tray menu; the installer and
  the CI workflow (they could not be run where the port was written).
- **AI Dual Mode** (UHD 160 Hz ⇄ FHD 320 Hz) has no DDC code: scans of every register with the mode off and on found
  only the signal registers. The window shows whether it is active but cannot switch it.

## Tests

`cargo test` needs no hardware: frame encoding and checksums, the table read and error recovery against a simulated
monitor (busy, garbage, lost writes, a second of outage), isolation of two monitors, identity, cache migration, write
levels and the dangerous-write list, hotplug window, autostart, single instance. Debug builds also have
`TITAN_FAKE_MONITORS=2` (two simulated monitors in the real window) with `TITAN_FAKE_SELFTEST=1` (the window checks
itself and quits). `tools/ddc_probe` is a stand-alone read-only probe for a new setup.
