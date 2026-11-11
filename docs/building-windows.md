# Building Titan Control on Windows

The Windows build uses the same Rust code as Linux (GTK 4 + libadwaita) and is built in an **MSYS2 UCRT64**
environment, because GTK is easiest to get there. Nothing here talks to a monitor.

```sh
git clone https://github.com/Dreyark/titan-control-windows.git
cd titan-control-windows
```

## 1. Tools

1. Install [MSYS2](https://www.msys2.org/) (for example `winget install MSYS2.MSYS2`, default folder `C:\msys64`).
2. Open the **MSYS2 UCRT64** shell and run:

   ```sh
   pacman -Syu                      # the shell may close: open it again and continue
   pacman -S --needed mingw-w64-ucrt-x86_64-toolchain mingw-w64-ucrt-x86_64-rust \
     mingw-w64-ucrt-x86_64-gtk4 mingw-w64-ucrt-x86_64-libadwaita \
     mingw-w64-ucrt-x86_64-blueprint-compiler mingw-w64-ucrt-x86_64-pkgconf mingw-w64-ucrt-x86_64-python zip
   ```

Use the Rust of MSYS2 (`rustc` from `/ucrt64/bin`), not a rustup MSVC toolchain: GTK from MSYS2 cannot be linked from
MSVC. (`tools/ddc_probe` is separate and builds with any Rust.)

## 2. Build and run

```sh
cargo build --release
./target/release/titan_control.exe            # run from this shell: it finds the GTK DLLs
./target/release/titan_control.exe --no-ddc   # safe mode: no access to the monitors at all
```

From PowerShell, `scripts\run_windows.ps1` does the same through MSYS2 (`-NoDdc`, `-Release`).

`blueprint-compiler` is a Python script that Windows cannot start from Rust, so `build.rs` falls back to the committed
`src/ui/window.ui`. After every change of `src/ui/window.blp` run `scripts/generate_ui.sh` and commit the result (set
`BLUEPRINT_COMPILER` to use another compiler).

## 3. Package

```sh
scripts/package_windows.sh
```

creates `dist/TitanControl-<version>-windows-x64/` and the ZIP next to it: the executable and every DLL it needs in
`bin/`, the GTK image loaders, compiled settings schemas, the Adwaita icons, translations and the app's `assets/`.
Check it on a PATH without MSYS2:

```powershell
.\scripts\smoke_windows.ps1 -Dir dist\TitanControl-<version>-windows-x64
```

The installer is made from the same folder with [Inno Setup](https://jrsoftware.org/isinfo.php):
`iscc /DVersion=<version> packaging\windows\titan_control.iss`. CI (`.github/workflows/windows.yml`) does all of this on
`windows-latest` and keeps the ZIP and the installer as artifacts.

## 4. Tests

```sh
cargo test             # no hardware: protocol on a simulated monitor, sessions, identity, cache, write levels
cargo clippy --all-targets
```

Development aids (debug builds only): `TITAN_FAKE_MONITORS=2` shows two simulated monitors without touching hardware,
and with `TITAN_FAKE_SELFTEST=1` the window checks switching, write levels and "identify" and quits (`[SELFTEST]` in
the log). Hardware tests (`#[ignore]`, they read from a real monitor) take the target from `TITAN_DDC_TARGET`:
`\.\DISPLAY6` on Windows, `/dev/i2c-14` on Linux (`TITAN_I2C` still works on Linux), for example
`TITAN_DDC_TARGET=\.\DISPLAY6 cargo test reads_all_settings -- --ignored --nocapture`.

## 5. Where things are

| Path | What |
|---|---|
| `src/platform/windows.rs` | display enumeration, EDID, Dxva2 transport, display change watcher, display modes, HDR, autostart, single instance |
| `src/monitor/` | protocol (`ddc`), per-monitor worker, sessions, identity, cache, simulated monitor (tests) |
| `src/multi.rs` | several monitors in the window, write levels, display settings rows |
| `src/tray_windows.rs` | the tray (`tray-icon`) |
| `assets/windows/` | icon, version info and the manifest (no DPI awareness on purpose, see the FAQ) embedded by `build.rs` |
| `tools/ddc_probe/` | stand-alone read-only probe of the Dxva2 transport |
| `docs/windows-port.md` | design, decisions and what was verified on hardware |
