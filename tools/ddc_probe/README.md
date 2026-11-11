# ddc_probe: a read-only DDC/CI probe for Windows

A small stand-alone tool (no GTK, not part of the main build) that talks to Titan Army P275MV PLUS monitors through the
Windows Monitor Configuration API (`dxva2`) the same way Titan Control does, so you can check a new computer, GPU or
cable before using the app, and find registers.

## What it sends

**Only GET requests**, through one function: `GetVCPFeatureAndVCPFeatureReply`. The code contains no
`SetVCPFeature` and no write path. Monitors without `P275MV` in their EDID are listed but never get any DDC traffic.

| Step | Codes | How |
|---|---|---|
| Plain registers | `DF`, `FE` (expected `0000/5101`), `F3`, `AE`, `C8` | plain GET |
| Manufacturer registers | `F6`, `E1`, `47`, `E2` | `GET 99` → `GET code` |
| Profile table | `GET 99` → `GET 22` → `GET profile` → `GET FE` … → `0000/5101` | repeated (default 10 times) |

Timing and recovery are those of `src/monitor/` (requests at least 100 ms apart per monitor, retry after 324 ms,
`settle` with `GET DF`, `GET FF` to repeat a lost table fragment). The table is only read for a *Custom* profile (even
code); for a *Default* profile the sequence is checked and no table is read.

## Build and run

```powershell
cd tools\ddc_probe
cargo build --release                       # any Rust with the MSVC or GNU Windows target; no GTK
cargo test --release                        # on a simulated transport, no monitor

.\target\release\ddc_probe.exe --list --out .\results   # displays and EDID identity only, no DDC traffic
.\target\release\ddc_probe.exe --out .\results          # the probe (waits for Enter)
.\target\release\ddc_probe.exe --monitor DISPLAY6 --reps 10 --out .\results
```

Close other DDC programs (the manufacturer's tool, Twinkle Tray, Dell Display Manager …) first. The result is
`ddc_probe_result.json` (all data and a trace of every frame) and `ddc_probe.log`.

## Reading the result

For the transport to be good on a monitor: `FE` = `0000/5101`, the profile table is read in at least 9 of 10 tries on
a Custom profile, and no failure leaves the monitor in a state that `settle` cannot clear. `gate_auto` in the JSON is a
hint only; compare the values with the OSD. Sporadic `0xC0262589` (invalid message command) errors are normal: the
manufacturer's own tool sees them too and retries.

## Finding the register of an OSD-only setting: `--scan` and `--diff`

Read every register with the setting off, change it in the OSD, read again, and compare. Both commands are read-only.

```powershell
.\target\release\ddc_probe.exe --monitor DISPLAY6 --scan off.json    # setting OFF, takes minutes
# ... change the setting in the monitor's OSD, wait for a stable picture ...
.\target\release\ddc_probe.exe --monitor DISPLAY6 --scan on.json
.\target\release\ddc_probe.exe --diff off.json on.json               # no monitor traffic
```

The scan reads every code `00`–`FF` plain, and every code `1F`–`FF` after `GET 99` (except `22`, `99`, `FE`, `FF`,
which start a table stream), with up to three tries per code because the monitor loses replies. The diff counts a
register as changed only if both scans have a valid reply and they differ. Expected noise: `0B`, `13`, `C0`, `F3`
(counters and values that move by themselves), `02`/`52` (OSD change flags) and `AC`/`AE`/`69` (the signal).

## Known risk

Windows checks every frame itself, so it can refuse a reply that the Linux code would accept (for example a table
fragment whose "current" value is larger than its "maximum"). In practice `FE = 0000/5101` is returned correctly. A
refused fragment shows up in the log as an error with the raw Win32 code.
