# Titan Control: FAQ

Questions and answers about the supported monitors, missing features, greyed-out settings and firmware. For
installation and usage see the [README](README.md); every VCP code is in [docs/p275mv_plus.md](docs/p275mv_plus.md).

- [Which monitors are supported?](#which-monitors-are-supported)
- [Why is there no light sensor switch?](#why-is-there-no-light-sensor-switch)
- [What can only be changed in the OSD?](#what-can-only-be-changed-in-the-osd)
- [Why can't I change brightness, colors or gamma?](#why-cant-i-change-brightness-colors-or-gamma)
- [Why are some settings greyed out?](#why-are-some-settings-greyed-out)
- [Why does DyDs switch Adaptive-Sync off?](#why-does-dyds-switch-adaptive-sync-off)
- [How do I tell the P275MV PLUS from the P275MV PLUS Enhanced?](#how-do-i-tell-the-p275mv-plus-from-the-p275mv-plus-enhanced)
- [Which firmware do I have?](#which-firmware-do-i-have)
- [How do I update the firmware, and which file is for my monitor?](#how-do-i-update-the-firmware-and-which-file-is-for-my-monitor)
- [The Halo Control slider does not show the value from the OSD](#the-halo-control-slider-does-not-show-the-value-from-the-osd)
- ["Monitor not found"](#monitor-not-found)
- [Does it work on X11, GNOME or other desktops?](#does-it-work-on-x11-gnome-or-other-desktops)
- [Is it safe?](#is-it-safe)
- [Windows questions](#windows-questions)

## Which monitors are supported?

The **Titan Army P275MV PLUS**, regular and Enhanced (增强版) versions. Only an Enhanced unit has been tested.
Other Titan Army models (P275MV, P275MV MAX, other Mstar-based monitors) may use the same scheme of manufacturer
registers, but their codes and values are unknown and the app does not check the model. Basic MCCS settings such as
brightness and contrast work on most monitors.

## Why is there no light sensor switch?

The firmware does not offer it over DDC/CI.

- The manufacturer's Windows tool (VIEW MORE WIDGET 1.0.2.8) has a Light Sensor page that writes VCP `DA`. On this
  monitor `DA` is an empty register and writes do nothing, so the tool hides the page too.
- About 120 unused codes were also tried; none switched the sensor.

The sensor can only be turned on and off in the OSD. While it is on, the brightness the app shows follows the
sensor.

## What can only be changed in the OSD?

These settings exist on the monitor but cannot be changed (or fully changed) from a computer:

| Setting | Why it is not in the app |
| --- | --- |
| Light sensor | No working DDC/CI code (see above); only its effect, the brightness change, is visible |
| Brightness mode (Normal / Highlight) | No DDC/CI code |
| Screen mode (Wide / 2K / FHD / 3K / 3K Wide) | No DDC/CI code; it changes the resolution and the picture size |
| AI Dual Mode (UHD 160 Hz / FHD 320 Hz) | No DDC/CI code found (clean scans, setting off and on); the monitor switches the signal itself. Titan Control only shows whether it is active (*Overview → Display mode*) |
| Full Game: sPX | Over DDC/CI the monitor accepts only Wide and 25" |
| Night Vision in the magnifier | DDC/CI code unknown |
| Break reminder interval | Over DDC/CI only on (30 min) and off; the app shows the interval chosen in the OSD |
| Game Rush, alignment lines | Have DDC/CI codes, but the app leaves them out |

Some features of the manufacturer's tool do not exist in this monitor's firmware at all:

- **DyDs/ULL Pulse, DyDs/LD Pulse and 3D mode** are for newer models. "DyDs Pulse" on this monitor is just the
  DyDs/ULL FPS and DyDs/LD picture modes.
- **Display Application** (MCCS `DC`) works over DDC/CI but is not in the OSD, so the app leaves it out.

The [code reference](docs/p275mv_plus.md#not-available-over-ddc) has the details.

## Why can't I change brightness, colors or gamma?

Each picture mode has a **Default** variant (factory values, locked) and a **Custom** variant (your values). Switch
*Mode settings* to *Custom* on the *Picture mode* page. That page holds what the OSD keeps per profile (Profile →
Custom): brightness, contrast, saturation, hue, color temperature, gamma, sharpness, Low Blue Light, Color Enhance,
CR Enhance, Shadow Balance, Night Vision, Super Resolution, Dynamic OD, local dimming, Halo Control and DyDs. HDR and
DCR can be changed in any profile.

## Why are some settings greyed out?

The monitor enforces rules between settings, and the app follows them:

- PIP/PBP turns off DCR, HDR, Adaptive-Sync, DyDs and the game overlays. Its source, audio source, position and
  size are greyed out while PIP/PBP is off.
- Full Game 25" turns off Adaptive-Sync, DyDs, Dynamic OD, the magnifier and HawkEye.
- DCR excludes local dimming. Halo Control needs local dimming.
- The magnifier does not work while Adaptive-Sync is on (HawkEye does), and the magnifier and HawkEye exclude each
  other.
- Color Enhance sets the saturation itself: with Color Enhance above 0 the saturation sliders are locked and show
  the values for that level.
- The 21:9 aspect ratio needs a wide Full Game screen mode (such as 3K Wide), which the monitor does not accept over
  DDC, so it is always greyed out.
- With firmware 2025-09-20 and HDR on, the monitor locks brightness, contrast, DCR, Low Blue Light, Color Enhance,
  CR Enhance, Shadow Balance, RGB output range, the picture mode, and turning local dimming off. DyDs, Night Vision
  and the magnifier are locked while Adaptive-Sync is off.

## Why does DyDs switch Adaptive-Sync off?

On firmware 2024-12-11 the monitor allows DyDs only without Adaptive-Sync. Firmware **2025-09-20 (Enhanced only)**
adds VRR to DyDs, so both can stay on; with Adaptive-Sync on, only ULL 1–3 are offered. After updating, select
*2025-09-20* under *Monitor → Information*.

## How do I tell the P275MV PLUS from the P275MV PLUS Enhanced?

Only the Enhanced (增强版) version has the picture modes **DyDs/ULL FPS** and **DyDs/LD** in its OSD (*Profile*).
If your monitor has them, it is the Enhanced one; if the list ends at DCI-P3, it is the regular one.

This distinction comes from the author's own observation, not from the manufacturer, so treat it as a hint and not as
a guarantee. Also look at the label on the back of the monitor and at the box (Enhanced is sold as "增强版"). If you are
not sure, do **not** flash firmware: see [firmware/](firmware/README.md).

## Which firmware do I have?

The OSD and DDC/CI both report only **5.1.1**. That is the USB firmware version, the same for every package. The
real version is the date of the package and cannot be read from the monitor. If you never updated, it is the factory
firmware. You can tell the 2025-09-20 firmware by these signs:

- the OSD lets you use DyDs together with Adaptive-Sync;
- you have **three custom buttons** and one of the options is switching the brightness mode (**Normal / Highlight**).

The second sign is the author's own observation, not something the manufacturer documents, so treat it as a hint and
not as a guarantee. If in doubt, assume the older firmware and choose *2024-12-11* in the app.

## How do I update the firmware, and which file is for my monitor?

The packages, translated instructions and warnings are in [firmware/](firmware/README.md). In short:

- `P275MV_PLUS_M001_20241211_5.1.1.bin`: regular P275MV PLUS.
- `P275MV_PLUS_Enhanced_M001_20250920_5.1.1.bin`: **P275MV PLUS Enhanced only.** The manufacturer warns that it
  must not be used on the P275MV, the regular P275MV PLUS or any other model.

To update:

1. Use a FAT32 USB stick with a single partition of **at most 4000 MB** (and more than 1 GB). A larger partition is
   not accepted by the monitor, so the update will not start: on a bigger stick create a 4000 MB partition and leave
   the rest unused. Copy the file to it and rename it to `MERGE.bin`.
2. Plug the stick into the monitor's USB-A port closest to the USB-B port.
3. Run *Factory Reset*, then *System Settings → Software Update → USB Update* in the OSD.
4. Do not cut the power until the update has finished and the monitor has restarted.

## The Halo Control slider does not show the value from the OSD

The monitor accepts Halo Control writes but always reports the same value when read. The app therefore shows the
last value it wrote itself.

## "Monitor not found"

- Check that your user can open `/dev/i2c-*` ([I2C access](README.md#2-i2c-access)).
- Check that `ddcutil detect` (if installed) lists the monitor.
- DisplayPort and USB-C work best. With some GPUs and drivers DDC/CI over HDMI is unreliable.
- The monitor briefly stops answering every few seconds; the app retries, so a slow first read is normal.

## Does it work on X11, GNOME or other desktops?

Monitor control works on any desktop. Changing the display mode is supported on KDE Plasma and COSMIC (Wayland);
system HDR sync and the wallpaper preview need KDE Plasma. The tray needs StatusNotifierItem support.

## Is it safe?

The app never sends writes known to be harmful: picture profile 0, VCP `F0` (turns the monitor off), and
unconfirmed power modes. Resets and power off ask for confirmation. Every write is read back, and settings the
monitor rejects jump back to the real value.

## Windows questions

### How does the Windows version talk to the monitor?

Through the Windows Monitor Configuration API (`dxva2`: `GetVCPFeatureAndVCPFeatureReply`, `SetVCPFeature`), the same
calls the manufacturer's own Windows tool uses. The protocol layer (pacing of requests, the `GET 99` → `GET code`
sequence for manufacturer registers, retries) is the one of the Linux version; only the transport differs. Windows
checks every frame itself and hides the raw bytes, so a reply the monitor sends "wrongly" is reported as an error and
retried. A cable pulled or a monitor switched off is noticed at once (display-change messages of Windows) and the
monitor returns to the list when it is back.

### Is writing to the monitor safe?

Titan Control changes settings that live in the monitor's own memory. Writing is on from the start, as in the Linux
version. Every write goes to exactly one monitor (the identity of the handle is checked before the first write of a
connection) and is read back and corrected. `22 = 0` and `F0` (which can switch the monitor off) are never written;
everything else the window offers is written, also the resets, so confirm those only when you mean them. The first
Windows build had a read-only start and write levels; they were removed in 1.1.0.

### Brightness and contrast are greyed out, or a value is "not accepted"

The monitor keeps the factory values of a *Default* profile and may ignore a write there. The program shows what the
monitor really reports after every write; a refused value goes back to the monitor's value, and the log says
`not confirmed` / `not accepted` (`%LOCALAPPDATA%\titan_control\logs\titan_control.log`). Some registers, for example
Halo Control, cannot be read back at all; the app then trusts the write.

### Two monitors: which one is which?

Both have the same name, product code and (on the author's units) the same serial number, so the app uses the Windows
device path (`…\UID0001` / `…\UID0002`) as identity. Use the list at the bottom of the sidebar (visible with two
monitors), set an alias, and press *Identify monitors*. The device path can change when a cable is moved to another
port; the alias is then set again.

### Other DDC programs

*VIEW MORE WIDGET*, Twinkle Tray, Dell Display Manager, vendor GPU tools and monitor software send their own DDC/CI
requests. Two programs on the bus make replies get lost. Close them while you use Titan Control. A GPU driver update
or a docking station can also change how DDC/CI is passed on; if all readings fail, reconnect the cable or try another
port.

### The monitor is stuck after a bad write

A monitor has its physical power button and a factory reset in the OSD. Button lock (`CA = 1`, buttons 2 and 3) is
undone with `CA = 2`; `D6 = 5` keeps the monitor off until a button is pressed. Titan Control does not send any of
these on its own. Keep a note (or photo) of your OSD settings before you try new settings.

### "Monitor not found" on Windows

The app only talks to monitors whose EDID name contains `P275MV`. Check that the monitor shows up in the Windows display
settings, that DDC/CI is enabled in its OSD, that no other DDC program is running, and that the cable is a full DisplayPort
or HDMI cable (some adapters and KVM switches do not pass DDC/CI). The log lists every display it found
(`[WIN] \.\DISPLAYn EDID: …`).

### The window is too small, or the text is soft, on a scaled screen

GTK 4 on Windows is DPI aware: the text is sharp and follows the Windows scaling, but the widgets (rows, switches,
sliders) keep their size. The app therefore opens a larger window on a scaled screen (1280×900 times the scaling, at most
90% of the screen) and uses an 11 pt font (Windows offers 9 pt, which looks small): *Preferences → Interface size*
changes it. If you prefer every proportion scaled and accept soft text, start the app with `TITAN_BITMAP_SCALING=1`:
Windows then scales the whole window like a bitmap. The log (`[UI] screen …`, `[UI] DPI awareness …`,
`[UI] GTK font …`, `[UI] window default size …`) shows what GTK sees; please attach it to a bug report about sizes.
