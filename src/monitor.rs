// Linux-only code (I2C bus scan, DRM, EDID over I2C) is gated with `cfg(unix)`; the Windows-only parts (sessions,
// identity, bus lock, write levels) are unused on Linux and the other way round, so dead code is not reported here.
#![allow(dead_code)]
#![allow(clippy::items_after_test_module)]

mod kde;
#[cfg(test)]
mod mock;
mod cache;
mod ddc;
pub mod identity;
pub mod transport;
mod session;
mod worker;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    io,
    path::PathBuf,
    sync::mpsc,
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
    sync::{Arc, OnceLock},
    thread,
    time::{Duration, Instant},
};
#[cfg(unix)]
use std::{fs::File, io::Read, path::Path};

#[cfg(not(windows))]
use std::process::Command;

use serde::{Deserialize, Serialize};
use transport::{FrameError, VcpReply, VcpTransport};
use cache::*;
use ddc::*;
#[allow(unused_imports)]
pub use ddc::{IdentityGuard, WriteLevel};
#[allow(unused_imports)]
pub use identity::MonitorId;
#[allow(unused_imports)]
pub use session::{MonitorSession, Router, SessionEvent, SessionManager, forward_tagged};
pub use ddc::is_custom_profile;
use worker::*;

pub const VCP_BRIGHTNESS: u8 = 0x10;
pub const VCP_CONTRAST: u8 = 0x12;
pub const VCP_COLOR_TEMP: u8 = 0x14;
pub const VCP_RED: u8 = 0x16;
pub const VCP_GREEN: u8 = 0x17;
pub const VCP_BLUE: u8 = 0x18;
pub const VCP_RESET_FACTORY: u8 = 0x04;
pub const VCP_RESET_BC: u8 = 0x05;
pub const VCP_RESET_COLOR: u8 = 0x08;
pub const VCP_GAMMA: u8 = 0x26;
pub const VCP_SHARPNESS: u8 = 0x87;
pub const VCP_OSD_LANG: u8 = 0xCC;
/// MCCS power mode (plain): 1 on, 3 turns the USB hub off, 5 turns the whole monitor off (2 and 4 untested).
pub const VCP_DPMS: u8 = 0xD6;
/// MCCS Input Source (plain register of 0x60): 0x0F DP-1, 0x10 DP-2, 0x11 HDMI-1, 0x12 HDMI-2.
pub const VCP_INPUT_SOURCE: u8 = 0x60;
pub const VCP_MODE: u8 = 0x22;
/// Low Blue Light 0-4 (manufacturer register).
pub const VCP_LOW_BLUE: u8 = 0xD8;

pub const VCP_USER1_RED: u8 = 0x16;
pub const VCP_USER1_GREEN: u8 = 0x17;
pub const VCP_USER1_BLUE: u8 = 0x18;
pub const VCP_USER2_RED: u8 = 0x19;
pub const VCP_USER2_GREEN: u8 = 0x1A;
pub const VCP_USER2_BLUE: u8 = 0x1B;
pub const VCP_USER3_RED: u8 = 0x1C;
pub const VCP_USER3_GREEN: u8 = 0x1D;
pub const VCP_USER3_BLUE: u8 = 0x1E;

/// Refresh rate counter 0/1 (manufacturer register; reported max 12 is wrong, plain register = Low Blue Light 0-100).
pub const VCP_FPS_COUNTER: u8 = 0x30;
/// Refresh rate counter position 0-3 (reported max 12 is wrong).
pub const VCP_FPS_POS: u8 = 0x31;
pub const VCP_CROSSHAIR_COLOR: u8 = 0x32;
pub const VCP_STOPWATCH_TIME: u8 = 0x33;
pub const VCP_CROSSHAIR_SHAPE: u8 = 0x34;
pub const VCP_STOPWATCH_POS: u8 = 0x35;
/// Game time 1 = 15, 2 = 30, 3 = 45, 4 = 60 min.
pub const VCP_GAME_TIME_VAL: u8 = 0x36;
pub const VCP_GAME_TIME_POS: u8 = 0x37;
pub const VCP_MAGNIFIER_POS: u8 = 0x38;
// Confirmed on P275MV PLUS: 0 = rear LEDs on, 1 = off.
pub const VCP_REAR_LED: u8 = 0x39;
/// Full Game: 0 = Wide, 1 = 25" (the monitor accepts only 0/1 over DDC).
pub const VCP_SCREEN_SIZE: u8 = 0x3A;
pub const VCP_ALIGNMENT: u8 = 0x3B;
pub const VCP_CROSSHAIR: u8 = 0x3D;
pub const VCP_STOPWATCH: u8 = 0x3E;
pub const VCP_GAME_TIME: u8 = 0x3F;

pub const VCP_COLOR_ENHANCE: u8 = 0x40;
pub const VCP_CR_ENHANCE: u8 = 0x41;
pub const VCP_SHADOW_BALANCE: u8 = 0x42;
pub const VCP_GAME_RUSH: u8 = 0x43;
pub const VCP_SUPER_RES: u8 = 0x44;
pub const VCP_NIGHT_VISION: u8 = 0x45;
pub const VCP_HALO_CONTROL: u8 = 0x46;
pub const VCP_LOCAL_DIMMING: u8 = 0x47;
pub const VCP_DYDS: u8 = 0x48;
pub const VCP_DYNAMIC_OD: u8 = 0x49;
pub const VCP_HDR: u8 = 0x4A;
pub const VCP_MAGNIFIER: u8 = 0x4B;
pub const VCP_MAGNIFIER_SIZE: u8 = 0x4C;
pub const VCP_MAGNIFIER_ZOOM: u8 = 0x4D;
pub const VCP_SATURATION_RED: u8 = 0x59;
pub const VCP_SATURATION_YELLOW: u8 = 0x5A;
pub const VCP_SATURATION_GREEN: u8 = 0x5B;
pub const VCP_SATURATION_CYAN: u8 = 0x5C;
pub const VCP_SATURATION_BLUE: u8 = 0x5D;
pub const VCP_SATURATION_MAGENTA: u8 = 0x5E;

/// Output Range lives in the manufacturer register of 0x60 (after `GET 99`), while the plain register of 0x60 is
/// the MCCS Input Source. Settings are keyed by code, so Output Range uses this key; [`route`] sends it as 0x60
/// after `99`. Never written on its own (a bare `SET 99` is harmless: it precedes every manufacturer write).
pub const VCP_OUTPUT_RANGE: u8 = 0x99;
pub const VCP_QUICK_BOOT: u8 = 0x61;
/// DCR 0/1 (manufacturer register; the plain register of 0xE1 is Low Blue Light 0-4).
pub const VCP_DCR: u8 = 0xE1;
/// Adaptive-Sync 0/1 (plain register only).
pub const VCP_ADAPTIVE_SYNC: u8 = 0xE2;
pub const VCP_HAWKEYE: u8 = 0x63;
pub const VCP_HAWKEYE_SIZE: u8 = 0x64;
pub const VCP_HAWKEYE_POS: u8 = 0x65;
pub const VCP_HAWKEYE_LEVEL: u8 = 0x66;
/// OSD show time 5-60 s (manufacturer register; the plain register of 0xC0 counts usage hours).
pub const VCP_OSD_TIME: u8 = 0xC0;
pub const VCP_OSD_H_POS: u8 = 0xC1;
pub const VCP_OSD_V_POS: u8 = 0xC2;
/// OSD transparency: OSD levels 0-5 = DDC 0-100 in steps of 20.
pub const VCP_OSD_TRANS: u8 = 0xC3;
/// Power LED: 1 off, 2 level 1, 3 level 2, 4 level 3.
pub const VCP_POWER_LED: u8 = 0xC5;
pub const VCP_POWER_SAVING: u8 = 0xC6;
pub const VCP_HUE_RED: u8 = 0x9B;
pub const VCP_HUE_YELLOW: u8 = 0x9C;
pub const VCP_HUE_GREEN: u8 = 0x9D;
pub const VCP_HUE_CYAN: u8 = 0x9E;
pub const VCP_HUE_BLUE: u8 = 0x9F;
pub const VCP_HUE_MAGENTA: u8 = 0xA0;
pub const VCP_VOLUME: u8 = 0xF6;
/// Audio mute (manufacturer register): 0 = sound, 1 = muted. The plain register of 0xF7 is part of the serial number.
pub const VCP_MUTE: u8 = 0xF7;

// Codes confirmed on the P275MV PLUS (docs/p275mv_plus.md); value ranges in the comments.
/// MCCS New control value: 2 = a setting was changed in the OSD; writing 1 clears it (and 0x52).
pub const VCP_NEW_CONTROL_VALUE: u8 = 0x02;
/// Aspect ratio (manufacturer register, max 6): 1 wide, 2 4:3, 3 1:1, 4 21:9, 5 auto.
pub const VCP_RATIO: u8 = 0x24;
/// PIP/PBP: 0 off, 1 PIP, 2 PBP 1:1, 3 PBP 2:1, 4 PBP 1:2.
pub const VCP_PIP_MODE: u8 = 0x50;
/// PIP sub-signal source: 0 DP, 3 USB-C, 4 HDMI-1, 5 HDMI-2 (low byte of `ff05/69xx`).
pub const VCP_PIP_SOURCE: u8 = 0x51;
/// Audio source: 0 auto, 3 USB-C, 4 HDMI-1, 5 HDMI-2 (low byte of `ff05/69xx`).
pub const VCP_AUDIO_SOURCE: u8 = 0x52;
/// PIP position: 0 top right, 1 top left, 2 bottom right, 3 bottom left.
pub const VCP_PIP_POSITION: u8 = 0x53;
/// PIP size: 0 small, 1 medium, 2 large.
pub const VCP_PIP_SIZE: u8 = 0x54;
/// Swap PIP inputs: 1 = swap.
pub const VCP_PIP_SWAP: u8 = 0x55;
/// Reset PIP settings: 1 = reset.
pub const VCP_PIP_RESET: u8 = 0x56;
/// Input selection: 0 auto, 1/2 DP, 3 USB-C, 5 HDMI-1, 6 HDMI-2 (low byte of `ff05/69xx`).
pub const VCP_INPUT_SELECT: u8 = 0x57;
/// USB hub follows: 0 USB-C, 1 USB-B.
pub const VCP_USB_HUB_SOURCE: u8 = 0x5F;
/// OSD lock: 0 off, 1 on.
pub const VCP_OSD_LOCK: u8 = 0xC4;
/// Front button lock (MCCS OSD/Button control, plain): 1 = buttons 2 and 3 locked, 2 = unlocked.
pub const VCP_BUTTON_LOCK: u8 = 0xCA;
/// Eyeshield reminder: 0 off, 1 = 30 min ... 8 = 4 h (reported max 2 is wrong; DDC writes only 0/1 confirmed).
pub const VCP_EYESHIELD_REMINDER: u8 = 0xD4;
/// USB power in sleep: 0 off, 1 on.
pub const VCP_USB_POWER_SLEEP: u8 = 0xD5;
/// MCCS Display Application (plain): 0 standard, 3 movie, 5 games; changes contrast and local dimming.
pub const VCP_DISPLAY_APPLICATION: u8 = 0xDC;
/// Second Adaptive-Sync switch (manufacturer register, 0/1).
pub const VCP_ADAPTIVE_SYNC_ALT: u8 = 0xE4;
/// LED effects (manufacturer registers). Color: 1 red, 2 green, 3 blue, 4 yellow, 5 purple, 6 cyan,
/// 7 colorful (only in the Breathe and Plain Water modes).
pub const VCP_LED_COLOR: u8 = 0xE5;
/// LED strength: 1 highest, 2 standard, 3 soft.
pub const VCP_LED_STRENGTH: u8 = 0xE6;
/// LED mode: 1 normal, 2 breathe, 3 flicker, 4 plain water, 5 star, 6 colorful pearls, 7 colorful water.
pub const VCP_LED_MODE: u8 = 0xE7;
/// LED front/rear color (only in mode 7 "colorful water"), values 1-6 as [`VCP_LED_COLOR`].
pub const VCP_LED_FRONT_COLOR: u8 = 0xE8;
pub const VCP_LED_REAR_COLOR: u8 = 0xE9;
/// Manufacturer factory reset: `SET 99 = 0` -> `SET C7 = 0` (value 1 does nothing).
pub const VCP_VENDOR_FACTORY_RESET: u8 = 0xC7;

const VCP_REFRESH_RATE: u8 = 0xAE;
const VCP_VERSION: u8 = 0xDF;
const VCP_USAGE_TIME: u8 = 0xF3;
const VCP_FIRMWARE: u8 = 0xFE;
const VCP_DDCCI_INIT: u8 = 0x99;
const PROFILE_TABLE_LEN: usize = 38;
/// Profile code reply plus nine FE fragments, four bytes each.
const PROFILE_TABLE_CHUNKS: usize = 10;
const VCP_RESEND: u8 = 0xFF;
/// Minimum start-to-start interval of requests (reads and writes); shorter gaps produce many lost replies.
const REQUEST_GAP: Duration = Duration::from_millis(100);
/// Pause after a failed reply (as the manufacturer's program).
const RETRY_WAIT: Duration = Duration::from_millis(324);
const UNSUPPORTED_WAIT: Duration = Duration::from_millis(200);
/// Pause between a write and its read-back.
/// Measured on the monitor: a written value is returned by the very next read (plain ~145 ms, after 99 ~245 ms
/// including the read itself), so only a short pause is kept.
const VERIFY_DELAY: Duration = Duration::from_millis(50);
const READ_TRIES: usize = 4;
const TABLE_RESEND_WAIT: Duration = Duration::from_millis(150);
const TABLE_RESEND_TRIES: usize = 12;
const TABLE_BUDGET: Duration = Duration::from_secs(60);
/// Two consecutive identical reads are required before the table is trusted.
const TABLE_CONFIRMATIONS: usize = 2;

#[derive(Clone)]
pub struct MonitorInfo {
    pub name: String,
    pub connector: String,
    pub resolution: String,
    pub hz: u16,
    pub firm: String,
    pub usage_mins: u16,
    pub settings: HashMap<u8, u16>,
}

#[derive(Clone, Debug, Default)]
pub struct SystemDisplayState {
    pub output: String,
    pub current_resolution: String,
    pub current_refresh: String,
    pub current_scale_percent: u16,
    pub available_resolutions: Vec<String>,
    pub refresh_rates: HashMap<String, Vec<String>>,
    pub scaling_options: Vec<u16>,
    /// HDR of the output in the compositor (`None` = not available or unknown).
    pub hdr: Option<bool>,
    /// Desktop number of the output (KDE: wallpaper per screen).
    pub screen_index: Option<usize>,
}

#[derive(Clone, Debug)]
struct DrmMonitorInfo {
    name: String,
    resolution: String,
    serial_number: String,
    connector: String,
}

pub enum WorkerCmd {
    Set(u8, u16),
    ReadUsageTime,
    /// Read the monitor info, the active profile and every setting again (refresh button).
    RefreshAll,
    /// Firmware 2025-09-20 keeps Adaptive-Sync on in DyDs profiles; older firmware needs it off first.
    DydsWithAdaptiveSync(bool),
    /// Chooses the monitor that the following commands act on. Handled by the session router, never by a worker.
    SelectMonitor(MonitorId),
}

pub enum UiCmd {
    /// The monitor answers again (`true`) or its connection is gone (`false`, e.g. the cable was pulled).
    Responding(bool),
    /// The monitors that will be connected (sent first, so the UI can list them before they answer).
    Detected(Vec<MonitorId>),
    MonitorFound(MonitorInfo),
    /// Values read from the monitor.
    Settings(HashMap<u8, u16>),
    /// The monitor did not accept a written value and keeps this one instead (always shown).
    Corrected(HashMap<u8, u16>),
    UsageTimeUpdated(u16),
    /// The settings of the (new) picture profile have been read: its controls can be used again.
    ProfileLoaded,
    /// Monitor info read again (refresh button); `None` = not answered this time.
    InfoRefreshed { hz: Option<u16>, usage_mins: Option<u16> },
    Progress(String),
    Error(String),
}

pub const COLOR_TEMP_VALUES: [u16; 6] = [0x05, 0x06, 0x08, 0x0b, 0x0c, 0x0d];
pub const OUTPUT_RANGE_VALUES: [u16; 3] = [0, 1, 2];
pub const HDR_VALUES: [u16; 4] = [0, 1, 2, 3];
/// Gamma 1.8, 2.0, 2.2, 2.4, 2.6, S-Curve (manufacturer register of 0x26 and profile table byte 33).
pub const GAMMA_VALUES: [u16; 6] = [2, 4, 6, 8, 10, 12];
pub const NIGHT_VISION_VALUES: [u16; 5] = [0, 1, 2, 3, 4];
pub const DYNAMIC_OD_VALUES: [u16; 5] = [0, 1, 2, 3, 4];
/// Local Dimming Off, Low, Smooth, Medium, High (the monitor accepts only 2-6).
pub const LOCAL_DIMMING_VALUES: [u16; 5] = [2, 3, 4, 5, 6];
pub const DYDS_VALUES: [u16; 7] = [2, 3, 4, 5, 6, 7, 8];
pub const POWER_SAVING_VALUES: [u16; 4] = [0, 1, 2, 3];
pub const SATURATION_CODES: [u8; 6] = [
    VCP_SATURATION_RED,
    VCP_SATURATION_GREEN,
    VCP_SATURATION_BLUE,
    VCP_SATURATION_CYAN,
    VCP_SATURATION_MAGENTA,
    VCP_SATURATION_YELLOW,
];

/// Saturation (red, green, blue, cyan, magenta, yellow, as [`SATURATION_CODES`]) that Color Enhance sets itself: with
/// level 1-10 the six saturation axes follow it and are locked. Level 0 has no table (the axes are free).
/// Read from the monitor at each level, 2026-10-02.
pub fn color_enhance_saturation(level: u16) -> Option<[u16; 6]> {
    Some(match level {
        1 => [53, 53, 53, 52, 52, 52],
        2 => [57, 57, 56, 53, 53, 53],
        3 => [58, 58, 58, 55, 55, 54],
        4 => [59, 59, 60, 56, 56, 55],
        5 => [61, 61, 62, 57, 57, 57],
        6 => [63, 63, 64, 59, 58, 58],
        7 => [65, 66, 67, 61, 60, 61],
        8 => [68, 69, 70, 63, 62, 63],
        9 => [71, 73, 74, 65, 64, 65],
        10 => [74, 76, 77, 67, 66, 67],
        _ => return None,
    })
}

pub const PICTURE_MODE_NAMES: [&str; 15] = [
    "Standard",
    "DyDs/ULL FPS",
    "DyDs/LD",
    "RTS Mode",
    "FPS Mode",
    "MOBA Mode",
    "Movie Mode",
    "Reading Mode",
    "Night Mode",
    "Eye Care Mode",
    "Mac View Mode",
    "E-Book Mode",
    "sRGB Mode",
    "AdobeRGB Mode",
    "DCI-P3 Mode",
];
/// Values of 0x22 per profile (order of [`PICTURE_MODE_NAMES`]), confirmed with Frida and on Linux
/// (`22 = 1` selects Standard Default). Never write 0.
pub const PICTURE_MODE_DEFAULT_VALUES: [u16; 15] = [
    0x01, 0x1B, 0x1D, 0x03, 0x05, 0x07, 0x09, 0x0B, 0x0D, 0x0F, 0x11, 0x13, 0x15, 0x17, 0x19,
];
pub const PICTURE_MODE_CUSTOM_VALUES: [Option<u16>; 15] = [
    Some(0x02),
    Some(0x1C),
    Some(0x1E),
    Some(0x04),
    Some(0x06),
    Some(0x08),
    Some(0x0A),
    Some(0x0C),
    Some(0x0E),
    Some(0x10),
    Some(0x12),
    Some(0x14),
    Some(0x16),
    Some(0x18),
    Some(0x1A),
];

// Value lists of features that the UI does not show yet (docs/p275mv_plus.md). Order = order of the OSD options.
#[allow(dead_code)]
pub mod values {
    /// LED color (0xE5): red, green, blue, yellow, purple, cyan, colorful (7: Breathe and Plain Water only).
    pub const LED_COLOR: [u16; 7] = [1, 2, 3, 4, 5, 6, 7];
    /// LED front/rear colors (0xE8, 0xE9): red, green, blue, yellow, purple, cyan.
    pub const LED_SIDE_COLOR: [u16; 6] = [1, 2, 3, 4, 5, 6];
    pub const LED_COLOR_COLORFUL: u16 = 7;
    /// LED modes that offer the "colorful" color: Breathe, Plain Water.
    pub const LED_MODES_WITH_COLORFUL: [u16; 2] = [2, 4];
    /// LED strength (0xE6): highest, standard, soft.
    pub const LED_STRENGTH: [u16; 3] = [1, 2, 3];
    /// LED mode (0xE7): normal, breathe, flicker, plain water, star, colorful pearls, colorful water.
    pub const LED_MODE: [u16; 7] = [1, 2, 3, 4, 5, 6, 7];
    /// LED mode in which front and rear colors are set separately (0xE8/0xE9).
    pub const LED_MODE_COLORFUL_WATER: u16 = 7;
    /// Rear LED (0x39): 0 = on, 1 = off.
    pub const REAR_LED_ON: u16 = 0;
    pub const REAR_LED_OFF: u16 = 1;
    /// Power LED (0xC5): off, level 1, level 2, level 3.
    pub const POWER_LED: [u16; 4] = [1, 2, 3, 4];

    /// PIP/PBP mode (0x50): off, PIP, PBP 1:1, PBP 2:1, PBP 1:2.
    pub const PIP_MODE: [u16; 5] = [0, 1, 2, 3, 4];
    pub const PIP_OFF: u16 = 0;
    /// PIP sub-signal source (0x51): DP, USB-C, HDMI-1, HDMI-2.
    pub const PIP_SOURCE: [u16; 4] = [0, 3, 4, 5];
    /// PIP position (0x53): top right, top left, bottom right, bottom left.
    pub const PIP_POSITION: [u16; 4] = [0, 1, 2, 3];
    /// PIP size (0x54): small, medium, large.
    pub const PIP_SIZE: [u16; 3] = [0, 1, 2];
    /// Audio source (0x52): auto, USB-C, HDMI-1, HDMI-2 (only connected inputs are accepted).
    pub const AUDIO_SOURCE: [u16; 4] = [0, 3, 4, 5];
    /// Input selection (0x57): auto, DP, DP, USB-C, HDMI-1, HDMI-2.
    pub const INPUT_SELECT: [u16; 6] = [0, 1, 2, 3, 5, 6];
    /// USB hub follows (0x5F): USB-C, USB-B.
    pub const USB_HUB_SOURCE: [u16; 2] = [0, 1];

    /// Button lock (0xCA): buttons 2 and 3 locked / unlocked.
    pub const BUTTONS_LOCKED: u16 = 1;
    pub const BUTTONS_UNLOCKED: u16 = 2;
    /// Eyeshield reminder (0xD4): off, 30 min, 1 h, 1.5 h, 2 h, 2.5 h, 3 h, 3.5 h, 4 h.
    /// All levels are read correctly; writes over DDC were confirmed only for 0 and 1.
    pub const EYESHIELD_REMINDER: [u16; 9] = [0, 1, 2, 3, 4, 5, 6, 7, 8];

    /// Aspect ratio (0x24): wide, 4:3, 1:1, 21:9, auto. 1, 2, 3 and 5 checked in the OSD (2026-10-02); 21:9 = 4 is
    /// the remaining value (0 is not accepted).
    pub const RATIO: [u16; 5] = [1, 2, 3, 4, 5];
    pub const RATIO_AUTO: u16 = 5;
    /// Power mode (0xD6): 1 = on, 5 = monitor off (only the power button turns it on again).
    pub const POWER_ON: u16 = 1;
    pub const POWER_OFF: u16 = 5;
}

/// Settings the monitor turns off while PIP/PBP is active (OSD greys them out).
pub const PIP_DISABLED_CODES: &[u8] = &[
    VCP_DCR, VCP_HDR, VCP_ADAPTIVE_SYNC, VCP_ADAPTIVE_SYNC_ALT, VCP_DYDS, VCP_DYNAMIC_OD, VCP_GAME_RUSH,
    VCP_HALO_CONTROL, VCP_SCREEN_SIZE, VCP_FPS_COUNTER, VCP_CROSSHAIR, VCP_STOPWATCH, VCP_GAME_TIME,
    VCP_MAGNIFIER, VCP_ALIGNMENT, VCP_HAWKEYE,
];

/// Whether a setting can be changed with the given monitor state (`settings` as sent in `UiCmd::Settings`).
#[allow(dead_code)]
pub fn is_setting_available(settings: &HashMap<u8, u16>, code: u8) -> bool {
    let pip_on = settings.get(&VCP_PIP_MODE).is_some_and(|&mode| mode != values::PIP_OFF);
    if pip_on && PIP_DISABLED_CODES.contains(&code) {
        return false;
    }
    match code {
        VCP_PIP_SOURCE | VCP_PIP_POSITION | VCP_PIP_SIZE | VCP_PIP_SWAP => pip_on,
        VCP_LED_FRONT_COLOR | VCP_LED_REAR_COLOR => {
            settings.get(&VCP_LED_MODE) == Some(&values::LED_MODE_COLORFUL_WATER)
        }
        // "Colorful pearls" locks the other LED settings.
        VCP_LED_COLOR | VCP_LED_STRENGTH => !matches!(settings.get(&VCP_LED_MODE), Some(6)),
        _ => true,
    }
}

/// Whether the "colorful" LED color (7) can be chosen in the given LED mode (Breathe, Plain Water).
#[allow(dead_code)]
pub fn led_mode_has_colorful(mode: Option<u16>) -> bool {
    mode.is_some_and(|mode| values::LED_MODES_WITH_COLORFUL.contains(&mode))
}


pub fn picture_mode_vcp_value(index: usize, custom: bool) -> Option<u16> {
    if custom {
        PICTURE_MODE_CUSTOM_VALUES.get(index).copied().flatten()
    } else {
        PICTURE_MODE_DEFAULT_VALUES.get(index).copied()
    }
}


pub fn picture_mode_from_vcp(value: u16) -> Option<(usize, bool)> {
    if let Some(index) = PICTURE_MODE_DEFAULT_VALUES
        .iter()
        .position(|&mapped| mapped == value)
    {
        return Some((index, false));
    }
    PICTURE_MODE_CUSTOM_VALUES
        .iter()
        .position(|&mapped| mapped == Some(value))
        .map(|index| (index, true))
}

pub fn builtin_color_temp_rgb(value: u16) -> Option<(u16, u16, u16)> {
    match value {
        0x05 => Some((48, 50, 47)),
        0x06 => Some((48, 50, 48)),
        0x08 => Some((8, 47, 50)),
        _ => None,
    }
}

pub fn is_user_color_temp(value: u16) -> bool {
    (0x0B..=0x0D).contains(&value)
}

pub fn color_temp_rgb_codes(value: u16) -> Option<(u8, u8, u8)> {
    match value {
        0x0B => Some((VCP_USER1_RED, VCP_USER1_GREEN, VCP_USER1_BLUE)),
        0x0C => Some((VCP_USER2_RED, VCP_USER2_GREEN, VCP_USER2_BLUE)),
        0x0D => Some((VCP_USER3_RED, VCP_USER3_GREEN, VCP_USER3_BLUE)),
        _ => None,
    }
}

pub fn cache_rgb_key_for_vcp_code(code: u8) -> Option<u8> {
    match code {
        VCP_USER1_RED | VCP_USER2_RED | VCP_USER3_RED => Some(VCP_RED),
        VCP_USER1_GREEN | VCP_USER2_GREEN | VCP_USER3_GREEN => Some(VCP_GREEN),
        VCP_USER1_BLUE | VCP_USER2_BLUE | VCP_USER3_BLUE => Some(VCP_BLUE),
        _ => None,
    }
}

pub fn is_color_temp_rgb_code(code: u8) -> bool {
    cache_rgb_key_for_vcp_code(code).is_some()
}

fn factory_reset_defaults() -> HashMap<u8, u16> {
    HashMap::from([
        (VCP_MODE, PICTURE_MODE_DEFAULT_VALUES[0]),
        // Values measured after a factory reset (brightness 90, contrast 50).
        (VCP_BRIGHTNESS, 90),
        (VCP_CONTRAST, 50),
        (VCP_SHARPNESS, 0),
        (VCP_COLOR_TEMP, 0x05),
        (VCP_RED, 48),
        (VCP_GREEN, 50),
        (VCP_BLUE, 47),
        (VCP_LOW_BLUE, 0),
        (VCP_GAMMA, GAMMA_VALUES[2]),
        (VCP_SHADOW_BALANCE, 50),
        (VCP_CR_ENHANCE, 0),
        (VCP_COLOR_ENHANCE, 0),
        (VCP_SUPER_RES, 0),
        (VCP_HUE_RED, 50),
        (VCP_HUE_GREEN, 50),
        (VCP_HUE_BLUE, 50),
        (VCP_HUE_CYAN, 50),
        (VCP_HUE_MAGENTA, 50),
        (VCP_HUE_YELLOW, 50),
        (VCP_SATURATION_RED, 50),
        (VCP_SATURATION_GREEN, 50),
        (VCP_SATURATION_BLUE, 50),
        (VCP_SATURATION_CYAN, 50),
        (VCP_SATURATION_MAGENTA, 50),
        (VCP_SATURATION_YELLOW, 50),
        (VCP_HALO_CONTROL, 0),
        (VCP_HDR, 0),
        (VCP_DCR, 0),
        (VCP_NIGHT_VISION, 0),
        (VCP_DYNAMIC_OD, 0),
        (VCP_ADAPTIVE_SYNC, 0),
        (VCP_GAME_RUSH, 1),
        (VCP_LOCAL_DIMMING, LOCAL_DIMMING_VALUES[4]),
        (VCP_DYDS, DYDS_VALUES[0]),
        (VCP_FPS_COUNTER, 0),
        (VCP_CROSSHAIR, 1),
        (VCP_STOPWATCH, 0),
        (VCP_GAME_TIME, 0),
        (VCP_MAGNIFIER, 0),
        (VCP_HAWKEYE, 0),
        (VCP_CROSSHAIR_COLOR, 8),
        (VCP_VOLUME, 50),
        (VCP_MUTE, 0),
        (VCP_OUTPUT_RANGE, OUTPUT_RANGE_VALUES[0]),
        (VCP_QUICK_BOOT, 0),
        (VCP_OSD_LANG, 0x02),
        (VCP_OSD_TIME, 10),
        (VCP_OSD_H_POS, 50),
        (VCP_OSD_V_POS, 50),
        (VCP_OSD_TRANS, 0),
        (VCP_POWER_SAVING, POWER_SAVING_VALUES[0]),
        (VCP_POWER_LED, 3),
        (VCP_REAR_LED, 0),
    ])
}

const EDID_HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

fn parse_edid_name(data: &[u8]) -> Option<String> {
    if data.len() < 128 || data[0..8] != EDID_HEADER {
        return None;
    }
    let mfg = format!(
        "{}{}{}",
        (((data[8] >> 2) & 0x1F) + 64) as char,
        ((((data[8] & 0x03) << 3) | ((data[9] >> 5) & 0x07)) + 64) as char,
        ((data[9] & 0x1F) + 64) as char
    );
    for b in 0..4 {
        let o = 54 + (b * 18);
        if data[o..o + 4] == [0, 0, 0, 0xFC] {
            let n: String = data[o + 5..o + 18]
                .iter()
                .filter(|&&c| (32..=126).contains(&c))
                .map(|&c| c as char)
                .collect();
            return Some(normalize_monitor_name(&format!("{} {}", mfg, n.trim())));
        }
    }
    Some(normalize_monitor_name(&mfg))
}

fn parse_edid_serial(data: &[u8]) -> Option<String> {
    if data.len() < 128 || data[0..8] != EDID_HEADER {
        return None;
    }
    for b in 0..4 {
        let o = 54 + (b * 18);
        if data[o..o + 4] == [0, 0, 0, 0xFF] {
            let serial: String = data[o + 5..o + 18]
                .iter()
                .filter(|&&c| (32..=126).contains(&c))
                .map(|&c| c as char)
                .collect();
            let serial = serial.trim().to_string();
            if !serial.is_empty() {
                return Some(serial);
            }
        }
    }
    None
}

fn normalize_monitor_name(name: &str) -> String {
    let trimmed = name.trim();
    trimmed
        .strip_prefix("LHC ")
        .or_else(|| trimmed.strip_prefix("LHC"))
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

#[cfg(unix)]
fn get_edid_name(path: &str) -> Option<String> {
    let data = crate::platform::linux::read_edid_block(path)?;
    parse_edid_name(&data[..])
}

#[cfg(unix)]
fn read_drm_resolution(path: &Path) -> String {
    for file_name in ["mode", "modes"] {
        if let Ok(contents) = fs::read_to_string(path.join(file_name))
            && let Some(mode) = contents.lines().find(|line| !line.trim().is_empty())
        {
            return mode.trim().to_string();
        }
    }
    String::new()
}

fn current_session_is_kde_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE").unwrap_or_default().eq_ignore_ascii_case("wayland")
        && std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default()
            .split(':').any(|desktop| desktop.eq_ignore_ascii_case("KDE"))
}

fn current_session_is_cosmic_wayland() -> bool {
    let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    session_type.eq_ignore_ascii_case("wayland") && desktop.to_ascii_uppercase().contains("COSMIC")
}

fn is_internal_output(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("EDP") || upper.starts_with("LVDS") || upper.starts_with("DSI")
}

fn strip_ansi_sequences(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if matches!(chars.peek(), Some('[')) {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
                continue;
            }
            continue;
        }

        output.push(ch);
    }

    output
}

#[derive(Default)]
struct CosmicOutputState {
    name: String,
    enabled: bool,
    scale_percent: u16,
    modes: Vec<(String, String, bool)>,
}

fn parse_cosmic_randr_list(text: &str) -> Vec<CosmicOutputState> {
    let mut outputs = Vec::new();
    let mut current: Option<CosmicOutputState> = None;

    for raw_line in text.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let is_output_header = !raw_line.starts_with(' ')
            && (trimmed.contains("(enabled)") || trimmed.contains("(disabled)"));

        if is_output_header {
            if let Some(output) = current.take() {
                outputs.push(output);
            }

            let name = trimmed
                .split_once(" (")
                .map(|(name, _)| name)
                .unwrap_or(trimmed)
                .trim();
            let enabled = trimmed.contains("(enabled)");
            current = Some(CosmicOutputState {
                name: name.to_string(),
                enabled,
                scale_percent: 100,
                modes: Vec::new(),
            });
            continue;
        }

        if let Some(output) = current.as_mut()
            && let Some(scale_value) = trimmed.strip_prefix("Scale: ")
        {
            if let Ok(scale_percent) = scale_value.trim_end_matches('%').trim().parse::<u16>() {
                output.scale_percent = scale_percent.clamp(100, 300);
            }
            continue;
        }

        if raw_line.starts_with("    ")
            && trimmed.contains(" @ ")
            && let Some(output) = current.as_mut()
            && let Some((resolution, rest)) = trimmed.split_once(" @ ")
        {
            let refresh = rest
                .split_whitespace()
                .next()
                .unwrap_or("60.000")
                .to_string();
            let is_current = trimmed.contains("(current)");
            output
                .modes
                .push((resolution.to_string(), refresh, is_current));
        }
    }

    if let Some(output) = current.take() {
        outputs.push(output);
    }

    outputs
}

pub fn query_system_display_state(preferred_output: Option<&str>) -> io::Result<SystemDisplayState> {
    #[cfg(windows)]
    return query_windows_display_state(preferred_output);
    #[cfg(not(windows))]
    query_linux_display_state(preferred_output)
}

/// Windows: the real current mode, supported modes and scaling (changing them is in `set_windows_display_mode`).
#[cfg(windows)]
fn query_windows_display_state(preferred_output: Option<&str>) -> io::Result<SystemDisplayState> {
    let (device, info) = crate::platform::windows::query_display_modes(preferred_output)
        .ok_or_else(|| io::Error::other("The display mode of this screen could not be read."))?;
    let resolution = |(w, h, _): crate::platform::windows::DisplayMode| format!("{w}x{h}");
    let refresh = |(_, _, hz): crate::platform::windows::DisplayMode| format!("{:.3}", hz as f64);
    let mut modes = info.modes.clone();
    // Largest resolution first, then the highest refresh rate, as on Linux.
    modes.sort_by(|a, b| (b.0 * b.1).cmp(&(a.0 * a.1)).then(b.2.cmp(&a.2)));
    let mut available_resolutions: Vec<String> = Vec::new();
    let mut refresh_rates: HashMap<String, Vec<String>> = HashMap::new();
    for mode in modes {
        let key = resolution(mode);
        if !available_resolutions.contains(&key) {
            available_resolutions.push(key.clone());
        }
        refresh_rates.entry(key).or_default().push(refresh(mode));
    }
    Ok(SystemDisplayState {
        output: device,
        current_resolution: resolution(info.current),
        current_refresh: refresh(info.current),
        current_scale_percent: info.scale_percent,
        available_resolutions,
        refresh_rates,
        // Windows has no public call to set the scaling.
        scaling_options: vec![info.scale_percent],
        hdr: None,
        screen_index: None,
    })
}

#[cfg(not(windows))]
fn query_linux_display_state(preferred_output: Option<&str>) -> io::Result<SystemDisplayState> {
    // sysfs reports card1-DP-3; compositors use DP-3.
    let preferred_output = preferred_output.map(|name| {
        match name.split_once('-') {
            Some((card, connector)) if card.strip_prefix("card")
                .is_some_and(|index| !index.is_empty() && index.bytes().all(|c| c.is_ascii_digit())) => connector,
            _ => name,
        }
    });
    if current_session_is_kde_wayland() {
        return kde::query(preferred_output);
    }
    if !current_session_is_cosmic_wayland() {
        return Err(io::Error::other(
            "System display controls are currently supported on KDE Wayland and COSMIC Wayland.",
        ));
    }

    let output = Command::new("cosmic-randr")
        .arg("list")
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(stderr.trim().to_string()));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    log!("[COSMIC-RANDR] raw list output:\n{text}");
    let plain_text = strip_ansi_sequences(&text);
    log!("[COSMIC-RANDR] sanitized list output:\n{plain_text}");
    let outputs = parse_cosmic_randr_list(&plain_text);
    log!(
        "[COSMIC-RANDR] parsed outputs: {:?}",
        outputs
            .iter()
            .map(|output| format!(
                "{} enabled={} modes={}",
                output.name,
                output.enabled,
                output.modes.len()
            ))
            .collect::<Vec<_>>()
    );
    let selected = preferred_output
        .and_then(|name| outputs.iter().find(|output| output.name == name && output.enabled))
        .or_else(|| {
            outputs
                .iter()
                .find(|output| output.enabled && !is_internal_output(&output.name))
        })
        .or_else(|| outputs.iter().find(|output| output.enabled))
        .ok_or_else(|| {
            io::Error::other(format!(
                "No enabled display outputs were found. Parsed {} outputs.",
                outputs.len()
            ))
        })?;

    let mut available_resolutions = Vec::new();
    let mut refresh_rates: HashMap<String, Vec<String>> = HashMap::new();
    let mut current_resolution = String::new();
    let mut current_refresh = String::new();

    for (resolution, refresh, is_current) in &selected.modes {
        if !available_resolutions.contains(resolution) {
            available_resolutions.push(resolution.clone());
        }
        let rates = refresh_rates.entry(resolution.clone()).or_default();
        if !rates.contains(refresh) {
            rates.push(refresh.clone());
        }
        if *is_current {
            current_resolution = resolution.clone();
            current_refresh = refresh.clone();
        }
    }

    if current_resolution.is_empty()
        && let Some((resolution, refresh, _)) = selected.modes.first()
    {
        current_resolution = resolution.clone();
        current_refresh = refresh.clone();
    }

    Ok(SystemDisplayState {
        output: selected.name.clone(),
        current_resolution,
        current_refresh,
        current_scale_percent: selected.scale_percent.max(100),
        available_resolutions,
        refresh_rates,
        scaling_options: vec![100, 125, 150, 175, 200],
        hdr: None,
        screen_index: None,
    })
}

/// Turns HDR of the output on or off in the compositor (KDE Wayland only).
pub fn set_system_hdr(output_name: &str, enable: bool) -> io::Result<()> {
    #[cfg(windows)]
    return crate::platform::windows::set_hdr(output_name, enable).map_err(io::Error::other);
    #[cfg(not(windows))]
    set_system_hdr_linux(output_name, enable)
}

#[cfg(not(windows))]
fn set_system_hdr_linux(output_name: &str, enable: bool) -> io::Result<()> {
    if current_session_is_kde_wayland() {
        return kde::set_hdr(output_name, enable);
    }
    Err(io::Error::other("System HDR is currently supported on KDE Wayland."))
}

/// Desktop wallpaper of the output (KDE Wayland only).
pub fn system_wallpaper(state: &SystemDisplayState) -> Option<std::path::PathBuf> {
    if current_session_is_kde_wayland() {
        return kde::wallpaper(state.screen_index?);
    }
    None
}

pub fn set_system_display_mode(
    output_name: &str,
    resolution: &str,
    refresh: Option<&str>,
    scale_percent: Option<u16>,
) -> io::Result<()> {
    #[cfg(windows)]
    return set_windows_display_mode(output_name, resolution, refresh, scale_percent);
    #[cfg(not(windows))]
    set_other_display_mode(output_name, resolution, refresh, scale_percent)
}

/// Windows: changes the resolution and refresh rate of ONE display (tested with the driver first, not saved until the
/// user keeps it; the caller asks and reverts). Scaling has no public call: it is only compared with the current.
#[cfg(windows)]
fn set_windows_display_mode(
    output_name: &str,
    resolution: &str,
    refresh: Option<&str>,
    scale_percent: Option<u16>,
) -> io::Result<()> {
    let (width, height) = resolution
        .split_once('x')
        .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid resolution format"))?;
    let (_, current) = crate::platform::windows::query_display_modes(Some(output_name))
        .ok_or_else(|| io::Error::other("The display mode of this screen could not be read."))?;
    if scale_percent.is_some_and(|scale| scale != current.scale_percent) {
        return Err(io::Error::other("The scaling can only be changed in the Windows display settings."));
    }
    let hz = match refresh {
        Some(text) => text.parse::<f64>().map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid refresh rate"))?.round() as u32,
        None => current.current.2,
    };
    crate::platform::windows::apply_display_mode(output_name, (width, height, hz), false).map_err(io::Error::other)
}

/// Windows: keeps the mode that is shown now (writes it to the registry).
#[cfg(windows)]
pub fn keep_system_display_mode(output_name: &str) -> io::Result<()> {
    crate::platform::windows::persist_current_display_mode(output_name).map_err(io::Error::other)
}

/// Windows: returns the display to its saved mode.
#[cfg(windows)]
pub fn revert_system_display_mode(output_name: &str) -> io::Result<()> {
    crate::platform::windows::revert_display_mode(output_name).map_err(io::Error::other)
}

/// HDR of one display in Windows: `(supported, enabled)`.
#[cfg(windows)]
pub fn windows_hdr_state(output_name: &str) -> Option<(bool, bool)> {
    crate::platform::windows::hdr_state(output_name)
}

#[cfg(not(windows))]
fn set_other_display_mode(
    output_name: &str,
    resolution: &str,
    refresh: Option<&str>,
    scale_percent: Option<u16>,
) -> io::Result<()> {
    if current_session_is_kde_wayland() {
        return kde::apply(output_name, resolution, refresh, scale_percent);
    }
    if !current_session_is_cosmic_wayland() {
        return Err(io::Error::other(
            "System display controls are currently supported on KDE Wayland and COSMIC Wayland.",
        ));
    }

    let (width, height) = resolution
        .split_once('x')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid resolution format"))?;

    let mut command = Command::new("cosmic-randr");
    command.args(["mode", output_name, width, height]);

    if let Some(refresh) = refresh {
        command.args(["--refresh", refresh]);
    }

    if let Some(scale_percent) = scale_percent {
        let scale_value = format!("{:.2}", (scale_percent as f64) / 100.0);
        command.args(["--scale", &scale_value]);
    }

    log!(
        "[COSMIC-RANDR] mode output={} resolution={} refresh={:?} scale={:?}",
        output_name, resolution, refresh, scale_percent
    );
    let output = command.output()?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log!(
            "[COSMIC-RANDR] failed output={} resolution={} refresh={:?} scale={:?}: {}",
            output_name,
            resolution,
            refresh,
            scale_percent,
            stderr.trim()
        );
        Err(io::Error::other(stderr.trim().to_string()))
    }
}

#[cfg(unix)]
fn get_drm_monitors() -> Vec<DrmMonitorInfo> {
    let mut monitors = Vec::new();
    let drm_dir = Path::new("/sys/class/drm");
    let entries = match fs::read_dir(drm_dir) {
        Ok(e) => e,
        Err(_) => return monitors,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let dir_name = entry.file_name();
        let dir_str = dir_name.to_string_lossy();
        if !dir_str.contains('-') || dir_str.starts_with("render") {
            continue;
        }
        let status_path = path.join("status");
        if let Ok(status) = fs::read_to_string(&status_path) {
            if status.trim() != "connected" {
                continue;
            }
        } else {
            continue;
        }
        let edid_path = path.join("edid");
        let mut edid_data = Vec::new();
        if let Ok(mut f) = File::open(&edid_path)
            && f.read_to_end(&mut edid_data).is_ok()
            && edid_data.len() >= 128
            && let Some(name) = parse_edid_name(&edid_data)
        {
            let resolution = read_drm_resolution(&path);
            let serial_number = parse_edid_serial(&edid_data).unwrap_or_default();
            log!("[DRM] {} connected: {} [{}]", dir_str, name, resolution);
            monitors.push(DrmMonitorInfo {
                name,
                resolution,
                serial_number,
                connector: dir_str.to_string(),
            });
        }
    }
    monitors
}

#[cfg(unix)]
fn get_edid_name_retry(path: &str, attempts: u8) -> Option<String> {
    for attempt in 0..attempts {
        if let Some(name) = get_edid_name(path) {
            return Some(name);
        }
        if attempt + 1 < attempts {
            thread::sleep(Duration::from_millis(150));
        }
    }
    None
}

/// Opens the DDC/CI transport of `path` (an I2C device on Linux, `\.DISPLAYn` on Windows).
#[cfg(unix)]
fn open_ddc(path: &str) -> io::Result<MsiDdc> {
    MsiDdc::open(path)
}

#[cfg(windows)]
fn open_ddc(path: &str) -> io::Result<MsiDdc> {
    let transport = crate::platform::windows::open_transport(path)?;
    // One lock for the whole bus: exchanges of different monitors never interleave.
    let mut ddc = MsiDdc::new(Box::new(transport::BusLocked::new(transport, transport::global_bus())));
    // Writes follow the level chosen in the window (0 = read-only at every start), and never the advanced
    // write level (resets and the like).
    ddc.set_level_source(user_write_level_source());
    ddc.set_write_level(WriteLevel::Normal);
    // The first write after connecting checks that the handle still belongs to the same monitor.
    let path = path.to_string();
    let expected = crate::platform::windows::current_identity(&path).map_err(io::Error::other)?;
    ddc.set_identity_guard(Box::new(move || {
        let now = crate::platform::windows::current_identity(&path)?;
        if now == expected { Ok(()) } else { Err(format!("{} is now {}, expected {}", path, now.label(), expected.label())) }
    }));
    Ok(ddc)
}

fn try_connect(
    dev_path: &str,
    name: &str,
    // Key of the settings cache: the monitor name on Linux, the full `MonitorId` key on Windows.
    cache_key: &str,
    drm_info: Option<&DrmMonitorInfo>,
    ui_tx: &async_channel::Sender<UiCmd>,
) -> Option<(MsiDdc, MonitorInfo, bool)> {
    let mut ddc = match open_ddc(dev_path) {
        Ok(d) => d,
        Err(e) => {
            log!("[DDC] {} open failed: {}, retrying...", dev_path, e);
            thread::sleep(Duration::from_millis(200));
            match open_ddc(dev_path) {
                Ok(d) => d,
                Err(e) => {
                    log!("[DDC] {} retry failed: {}", dev_path, e);
                    return None;
                }
            }
        }
    };

    // Verify DDC/CI works: two valid replies in a row (a second round covers a periodic outage).
    if !wake_ddc(&mut ddc) && !wake_ddc(&mut ddc) {
        log!("[DDC] {} DDC/CI test failed", dev_path);
        return None;
    }

    log!("[DDC] Monitor ready on {}: {}", dev_path, name);
    let cached_snapshot = load_cached_snapshot(cache_key);

    // The signal can change between sessions, so the refresh rate is always read (plain, cheap).
    let hz = read_vcp(&mut ddc, VCP_REFRESH_RATE)
        .map(|v| v / 100)
        .or_else(|| cached_snapshot.as_ref().map(|snapshot| snapshot.hz))
        .unwrap_or(0);
    let fe = if cached_snapshot.is_some() {
        0
    } else {
        read_vcp(&mut ddc, VCP_FIRMWARE).unwrap_or(0)
    };
    let mins = read_vcp(&mut ddc, VCP_USAGE_TIME).unwrap_or(0);

    // Cached values are shown at once; the worker then reads every setting from the monitor
    // (see `startup_read_keys`), so the cache is only a fast first draft.
    let mut settings = load_cached_settings(cache_key, ui_tx);
    let _ = ui_tx.send_blocking(UiCmd::Progress("Reading picture profile...".to_string()));
    let table_read = match ddc.read_active_profile(TABLE_CONFIRMATIONS) {
        Ok(ActiveProfile::Custom(table)) => {
            log!("[DDC] Active profile table 0x{:02X} read", table.scene_id);
            settings.insert(VCP_MODE, table.scene_id as u16);
            settings.extend(table.settings());
            true
        }
        Ok(ActiveProfile::Default(scene_id)) => {
            log!("[DDC] Active profile 0x{scene_id:02X} (Default): settings are read one by one");
            settings.insert(VCP_MODE, scene_id as u16);
            false
        }
        Err(error) => {
            log!("[DDC] Active profile table read failed: {}", error);
            false
        }
    };
    apply_color_temp_cache(cache_key, &mut settings);
    save_cache_for(cache_key, &settings, Some(dev_path));

    let resolution = drm_info
        .map(|info| info.resolution.clone())
        .or_else(|| {
            cached_snapshot
                .as_ref()
                .map(|snapshot| snapshot.resolution.clone())
        })
        .unwrap_or_default();
    let serial_number = drm_info
        .map(|info| info.serial_number.clone())
        .or_else(|| {
            cached_snapshot
                .as_ref()
                .map(|snapshot| snapshot.serial_number.clone())
        })
        .unwrap_or_default();
    let firm = cached_snapshot
        .as_ref()
        .map(|snapshot| snapshot.firm.clone())
        .unwrap_or_else(|| format!("v {}.{}.{}", (fe >> 12) & 0xF, (fe >> 8) & 0xF, fe & 0xFF));

    let info = MonitorInfo {
        name: name.to_string(),
        connector: drm_info.map(|info| info.connector.clone()).unwrap_or_default(),
        resolution,
        hz,
        firm,
        usage_mins: mins,
        settings,
    };

    save_cached_snapshot(
        cache_key,
        CachedMonitorSnapshot {
            resolution: info.resolution.clone(),
            hz: info.hz,
            serial_number,
            firm: info.firm.clone(),
            usage_mins: info.usage_mins,
        },
        Some(dev_path),
    );

    Some((ddc, info, table_read))
}

/// Safe mode: no access to the monitor hardware at all (UI work, CI). Set by `--no-ddc` or `TITAN_NO_DDC=1`.
static NO_DDC: AtomicBool = AtomicBool::new(false);

pub fn set_no_ddc(enabled: bool) {
    NO_DDC.store(enabled, Ordering::Relaxed);
}

pub fn no_ddc() -> bool {
    NO_DDC.load(Ordering::Relaxed)
}

/// Id used for events that belong to no particular monitor (progress, "no monitor found") and by Linux, which
/// has always driven a single monitor.
fn system_id() -> MonitorId {
    MonitorId::legacy("monitor")
}

/// Windows: writes follow a level the user chooses in the window (0 = read-only, the default at every start). Linux has
/// never had such a limit.
pub fn write_policy_active() -> bool {
    cfg!(windows)
}

/// The write level shared by the window and every session. Changing it takes effect at the next write attempt, so
/// setting it to 0 is an immediate emergency stop.
pub fn user_write_level_source() -> Arc<AtomicU8> {
    static LEVEL: OnceLock<Arc<AtomicU8>> = OnceLock::new();
    LEVEL.get_or_init(|| Arc::new(AtomicU8::new(0))).clone()
}

pub fn set_user_write_level(level: u8) {
    user_write_level_source().store(level.min(3), Ordering::SeqCst);
    log!("[DDC] write level: {}", level.min(3));
}

pub fn user_write_level() -> u8 {
    user_write_level_source().load(Ordering::SeqCst)
}

/// Whether a write of `code` is allowed at `level` (see [`write_class`]).
/// Experimental (Windows, write level 2+): the settings of a Default profile are offered for writing as well. The
/// monitor keeps factory values there per its documentation, so such a write may be ignored; read-back shows what
/// really happened.
pub fn default_profile_unlocked() -> bool {
    write_policy_active() && user_write_level() >= 2
}

pub fn write_allowed(code: u8, level: u8) -> bool {
    write_class(code).is_some_and(|required| required <= level)
}

pub fn start_worker(worker_rx: mpsc::Receiver<WorkerCmd>, events: async_channel::Sender<SessionEvent>) {
    #[cfg(debug_assertions)]
    if fake_monitors(&events) {
        thread::spawn(move || while worker_rx.recv().is_ok() {});
        return;
    }
    if no_ddc() {
        log!("[DDC] Safe mode (--no-ddc): the monitor is not accessed");
        thread::spawn(move || {
            let _ = events.send_blocking(SessionEvent { monitor: system_id(), cmd: UiCmd::Error("Monitor not found".into()) });
            drop(worker_rx);
        });
        return;
    }
    #[cfg(unix)]
    {
        let (ui_tx, ui_rx) = async_channel::unbounded::<UiCmd>();
        thread::spawn(move || forward_tagged(system_id(), ui_rx, events, None));
        start_worker_hw(worker_rx, ui_tx);
    }
    #[cfg(windows)]
    start_sessions(worker_rx, events);
}

/// True when the EDID names a P275MV: the only monitors that ever receive DDC traffic.
pub(crate) fn edid_is_p275mv(edid: &[u8]) -> bool {
    parse_edid_name(edid).is_some_and(|name| name.contains("P275MV"))
}

/// A P275MV found by the enumeration, with everything a session needs (plain data, so it can move to a thread).
#[cfg(windows)]
struct SessionTarget {
    device: String,
    id: MonitorId,
    drm: DrmMonitorInfo,
}

/// The P275MV monitors Windows currently shows (other monitors are never touched).
#[cfg(windows)]
fn enumerate_targets() -> Vec<SessionTarget> {
    let mut targets = Vec::new();
    for display in crate::platform::windows::enumerate_displays() {
        let Some(edid) = display.edid.as_deref() else { continue };
        if !edid_is_p275mv(edid) {
            continue;
        }
        let path = display.registry_key.clone().unwrap_or_default();
        let Some(id) = identity::from_edid(edid, &path) else { continue };
        targets.push(SessionTarget {
            device: display.device.clone(),
            drm: DrmMonitorInfo {
                name: id.name.clone(),
                resolution: display.resolution(),
                serial_number: id.serial.clone(),
                connector: display.device.clone(),
            },
            id,
        });
    }
    targets
}

/// What the session thread waits for.
#[cfg(windows)]
enum SessionMsg {
    Command(WorkerCmd),
    /// Something about the displays changed (or it is time to look): enumerate again.
    Rescan,
}

/// How often the displays are enumerated even when Windows reported nothing (missed messages are harmless).
#[cfg(windows)]
const RESCAN_INTERVAL: Duration = Duration::from_secs(5);
/// Display changes come in bursts; the monitors are enumerated once this long after the last message.
#[cfg(windows)]
const RESCAN_SETTLE: Duration = Duration::from_millis(600);

/// Windows: one session per P275MV. Every session reports to the UI with its own id; commands from the UI go to the
/// monitor chosen with `WorkerCmd::SelectMonitor` (see [`Router`]). A monitor that appears, disappears or comes back
/// (cable, sleep, another port) is noticed from the display-change messages of Windows: its session is started or
/// replaced, the others are left alone.
#[cfg(windows)]
fn start_sessions(worker_rx: mpsc::Receiver<WorkerCmd>, events: async_channel::Sender<SessionEvent>) {
    thread::spawn(move || {
        let system_events = events.clone();
        let system = move |cmd: UiCmd| {
            let _ = system_events.send_blocking(SessionEvent { monitor: system_id(), cmd });
        };
        system(UiCmd::Progress("Detecting monitors...".into()));
        let targets = enumerate_targets();
        for target in &targets {
            log!("[WIN] {} EDID: {}", target.device, target.id.label());
        }
        if targets.is_empty() {
            log!("[DDC] No compatible monitor found");
            system(UiCmd::Error("Monitor not found".into()));
            return;
        }
        let ids: Vec<MonitorId> = targets.iter().map(|target| target.id.clone()).collect();
        migrate_legacy_cache(&ids);
        // The UI lists the monitors before they answer.
        let _ = events.send_blocking(SessionEvent { monitor: ids[0].clone(), cmd: UiCmd::Detected(ids) });

        let mut router = Router::new(SessionManager::default());
        for target in targets {
            start_session(&mut router, target, &events);
        }

        // One queue for the commands of the UI and the display changes.
        let (msg_tx, msg_rx) = mpsc::channel::<SessionMsg>();
        let commands = msg_tx.clone();
        thread::spawn(move || {
            while let Ok(cmd) = worker_rx.recv() {
                if commands.send(SessionMsg::Command(cmd)).is_err() {
                    break;
                }
            }
        });
        let (change_tx, change_rx) = mpsc::channel::<()>();
        crate::platform::windows::spawn_display_watcher(change_tx);
        let changes = msg_tx.clone();
        thread::spawn(move || {
            while change_rx.recv().is_ok() {
                // Wait until the burst is over, then ask for one scan.
                while change_rx.recv_timeout(RESCAN_SETTLE).is_ok() {}
                if changes.send(SessionMsg::Rescan).is_err() {
                    break;
                }
            }
        });

        loop {
            match msg_rx.recv_timeout(RESCAN_INTERVAL) {
                Ok(SessionMsg::Command(cmd)) => router.on_command(cmd),
                Ok(SessionMsg::Rescan) | Err(mpsc::RecvTimeoutError::Timeout) => rescan(&mut router, &events),
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    });
}

/// Starts (or restarts) the session of `target` and puts it under the router.
#[cfg(windows)]
fn start_session(router: &mut Router, target: SessionTarget, events: &async_channel::Sender<SessionEvent>) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();
    let dead = std::sync::Arc::new(AtomicBool::new(false));
    router.manager_mut().replace_or_add(MonitorSession {
        id: target.id.clone(),
        device: target.device.clone(),
        cmd_tx,
        dead: dead.clone(),
    });
    spawn_session(target, cmd_rx, events.clone(), dead);
}

/// Compares the monitors Windows shows with the sessions: a monitor without a (living) session gets one.
#[cfg(windows)]
fn rescan(router: &mut Router, events: &async_channel::Sender<SessionEvent>) {
    let mut changed = false;
    let targets = enumerate_targets();
    // A monitor that Windows no longer lists (cable pulled, switched off) leaves the list.
    for id in router.manager().ids() {
        if !targets.iter().any(|target| target.id == id) {
            log!("[WIN] {} is no longer connected", id.label());
            router.forget(&id);
            changed = true;
        }
    }
    for target in targets {
        if router.manager().is_dead(&target.id) == Some(false) {
            continue;
        }
        log!("[WIN] {} ({}) appeared or came back: starting its session", target.id.label(), target.device);
        start_session(router, target, events);
        changed = true;
    }
    if changed {
        // The authoritative list of the monitors that exist now.
        let ids = router.manager().ids();
        let monitor = ids.first().cloned().unwrap_or_else(system_id);
        let _ = events.send_blocking(SessionEvent { monitor, cmd: UiCmd::Detected(ids) });
    }
}

/// Runs one monitor: connects, then serves its commands. Its events are tagged with the monitor id.
#[cfg(windows)]
fn spawn_session(
    target: SessionTarget,
    cmd_rx: mpsc::Receiver<WorkerCmd>,
    events: async_channel::Sender<SessionEvent>,
    dead: std::sync::Arc<AtomicBool>,
) {
    let (ui_tx, ui_rx) = async_channel::unbounded::<UiCmd>();
    let id = target.id.clone();
    thread::spawn(move || forward_tagged(id, ui_rx, events, Some(dead)));
    thread::spawn(move || {
        let _timer = crate::platform::windows::TimerResolution::new();
        let key = target.id.cache_key();
        let Some((ddc, info, table_read)) = try_connect(&target.device, &target.id.name, &key, Some(&target.drm), &ui_tx)
        else {
            log!("[DDC] {}: could not connect", target.id.label());
            let _ = ui_tx.send_blocking(UiCmd::Error("Monitor not found".into()));
            return;
        };
        let _ = ui_tx.send_blocking(UiCmd::MonitorFound(info));
        Worker::new(ddc, key, ui_tx, table_read).run(&cmd_rx);
    });
}

#[cfg(unix)]
fn start_worker_hw(worker_rx: mpsc::Receiver<WorkerCmd>, ui_tx: async_channel::Sender<UiCmd>) {
    thread::spawn(move || {
        let mut found = None;
        let mut current_monitor_name = None;

        let _ = ui_tx.send_blocking(UiCmd::Progress("Detecting monitors...".into()));
        let drm_monitors = get_drm_monitors();
        let cached_monitor_name = load_last_monitor_name();
        let cached_i2c_path = load_last_i2c_path();
        let target_name = drm_monitors
            .iter()
            .find(|n| {
                cached_monitor_name
                    .as_ref()
                    .map(|cached| n.name == *cached)
                    .unwrap_or(false)
            })
            .or_else(|| drm_monitors.iter().find(|n| n.name.contains("P275MV")))
            .map(|n| n.name.clone());
        if let Some(ref name) = target_name {
            log!("[DRM] Target monitor: {}", name);
        }

        if let Some(ref cached_path) = cached_i2c_path
            && Path::new(cached_path).exists()
        {
            let cached_name = target_name
                .clone()
                .or_else(|| get_edid_name_retry(cached_path, 1));
            if let Some(name) = cached_name {
                let drm_info = drm_monitors.iter().find(|monitor| monitor.name == name);
                if let Some((ddc, info, table_read)) = try_connect(cached_path, &name, &name, drm_info, &ui_tx) {
                    let _ = ui_tx.send_blocking(UiCmd::MonitorFound(info));
                    current_monitor_name = Some(name);
                    found = Some((ddc, table_read));
                }
            }
        }

        let _ = ui_tx.send_blocking(UiCmd::Progress("Searching for monitors...".into()));
        for i in 0..32 {
            if found.is_some() {
                break;
            }
            let p = format!("/dev/i2c-{}", i);
            if !Path::new(&p).exists() {
                continue;
            }
            if cached_i2c_path.as_deref() == Some(p.as_str()) {
                continue;
            }

            let retries = if target_name.is_some() { 3 } else { 1 };
            let name = match get_edid_name_retry(&p, retries) {
                Some(n) => n,
                _none => continue,
            };
            log!("[I2C] {} EDID: {}", p, name);

            if let Some(ref target) = target_name
                && !name.contains("P275MV")
                && target.contains("P275MV")
            {
                log!("[I2C] {} skipping (not target)", p);
                continue;
            }

            let drm_info = drm_monitors.iter().find(|monitor| monitor.name == name);
            if let Some((ddc, info, table_read)) = try_connect(&p, &name, &name, drm_info, &ui_tx) {
                let _ = ui_tx.send_blocking(UiCmd::MonitorFound(info));
                current_monitor_name = Some(name.clone());
                found = Some((ddc, table_read));
                break;
            }
        }

        let Some((ddc, table_read)) = found else {
            log!("[DDC] No compatible monitor found");
            let _ = ui_tx.send_blocking(UiCmd::Error("Monitor not found".into()));
            return;
        };

        let monitor_name = current_monitor_name.unwrap_or_else(|| "default".into());
        Worker::new(ddc, monitor_name, ui_tx, table_read).run(&worker_rx);
    });
}


#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn dummy_info() -> MonitorInfo {
        MonitorInfo {
            name: "P275MV PLUS".into(),
            connector: "DISPLAY1".into(),
            resolution: "2560x1440".into(),
            hz: 144,
            firm: "v 5.1.1".into(),
            usage_mins: 0,
            settings: HashMap::new(),
        }
    }


    #[test]
    fn color_enhance_sets_the_saturation() {
        assert_eq!(color_enhance_saturation(0), None);
        assert_eq!(color_enhance_saturation(11), None);
        // Level 10 as read: red 74, green 76, blue 77, yellow 67, cyan 67, magenta 66.
        assert_eq!(color_enhance_saturation(10), Some([74, 76, 77, 67, 66, 67]));
        assert_eq!(SATURATION_CODES[3], VCP_SATURATION_CYAN);
        assert_eq!(SATURATION_CODES[5], VCP_SATURATION_YELLOW);
    }

    #[test]
    fn registers_follow_the_code_map() {
        // Manufacturer register (GET 99 first).
        for key in [VCP_LOCAL_DIMMING, VCP_GAMMA, VCP_MUTE, VCP_OSD_TIME, VCP_DCR, VCP_LED_MODE, VCP_PIP_MODE] {
            assert_eq!(route(key).access, Access::Vendor, "0x{key:02X}");
        }
        // Profile table selectors: plain read, write with 99.
        for key in [VCP_BRIGHTNESS, VCP_CONTRAST, VCP_COLOR_TEMP, VCP_USER3_BLUE] {
            assert_eq!(route(key).access, Access::Table, "0x{key:02X}");
        }
        // Plain MCCS registers.
        for key in [VCP_REAR_LED, VCP_ADAPTIVE_SYNC, VCP_INPUT_SOURCE, VCP_DPMS, VCP_DISPLAY_APPLICATION,
            VCP_BUTTON_LOCK, VCP_RESET_FACTORY, VCP_RESET_BC, VCP_RESET_COLOR, VCP_NEW_CONTROL_VALUE]
        {
            assert_eq!(route(key).access, Access::Plain, "0x{key:02X}");
        }
        // Output Range is 0x60 after 99; the plain 0x60 is the input source.
        let output_range = route(VCP_OUTPUT_RANGE);
        assert_eq!((output_range.code, output_range.access), (0x60, Access::Vendor));
        // The manufacturer factory reset is written with 99.
        assert_eq!(route(VCP_VENDOR_FACTORY_RESET).access, Access::Vendor);
    }

    #[test]
    fn actions_are_not_verified() {
        for key in [VCP_RESET_FACTORY, VCP_RESET_BC, VCP_RESET_COLOR, VCP_VENDOR_FACTORY_RESET, VCP_DPMS,
            VCP_PIP_SWAP, VCP_PIP_RESET, VCP_NEW_CONTROL_VALUE]
        {
            assert!(!route(key).verify, "0x{key:02X}");
        }
        assert!(route(VCP_BRIGHTNESS).verify);
        assert!(route(VCP_LED_COLOR).verify);
    }

    #[test]
    fn dangerous_writes_are_blocked() {
        assert!(check_write(VCP_MODE, 0).is_err());
        assert!(check_write(0xF0, 1).is_err());
        assert!(check_write(VCP_MODE, 1).is_ok());
        assert!(check_write(VCP_DPMS, values::POWER_OFF).is_ok());
    }

    #[test]
    fn source_registers_use_the_low_byte() {
        let vendor = route(VCP_INPUT_SELECT);
        assert_eq!(vendor.value(VcpReply { maximum: 0xFF05, current: 0x6903 }), 3);
        assert_eq!(vendor.value(VcpReply { maximum: 0xFF64, current: 50 }), 50);
        // Plain registers keep the full value (e.g. usage minutes).
        let plain = route(VCP_USAGE_TIME);
        assert_eq!(plain.value(VcpReply { maximum: 0, current: 0x0E10 }), 3600);
    }

    #[test]
    fn profile_table_low_blue_light_is_a_level() {
        let mut bytes = [0u8; PROFILE_TABLE_LEN];
        for (raw, level) in [(0u8, 0u16), (25, 1), (50, 2), (75, 3), (100, 4)] {
            bytes[30] = raw;
            let table = ProfileTable { scene_id: 2, bytes };
            assert_eq!(table.settings()[&VCP_LOW_BLUE], level);
        }
    }

    #[test]
    fn pip_and_led_dependencies() {
        let mut settings = HashMap::from([(VCP_PIP_MODE, 0), (VCP_LED_MODE, 1)]);
        assert!(is_setting_available(&settings, VCP_HDR));
        assert!(!is_setting_available(&settings, VCP_PIP_SOURCE));
        assert!(!is_setting_available(&settings, VCP_LED_FRONT_COLOR));
        settings.insert(VCP_PIP_MODE, 1);
        settings.insert(VCP_LED_MODE, values::LED_MODE_COLORFUL_WATER);
        assert!(!is_setting_available(&settings, VCP_HDR));
        assert!(is_setting_available(&settings, VCP_PIP_SOURCE));
        assert!(is_setting_available(&settings, VCP_LED_FRONT_COLOR));
    }

    #[test]
    fn picture_modes_match_the_code_map() {
        assert_eq!(picture_mode_from_vcp(1), Some((0, false)));
        assert_eq!(picture_mode_from_vcp(2), Some((0, true)));
        assert_eq!(picture_mode_from_vcp(5), Some((4, false)));
        assert_eq!(picture_mode_from_vcp(6), Some((4, true)));
        assert!(PICTURE_MODE_DEFAULT_VALUES.iter().all(|&value| value != 0));
    }

    /// Target of the hardware tests: `TITAN_DDC_TARGET`, else `TITAN_I2C`, else a default for the platform.
    fn hardware_target() -> String {
        std::env::var("TITAN_DDC_TARGET")
            .or_else(|_| std::env::var("TITAN_I2C"))
            .unwrap_or_else(|_| if cfg!(windows) { r"\\.\DISPLAY6".into() } else { "/dev/i2c-14".into() })
    }

    /// Hardware test that WRITES: brightness and OSD H-position are changed by 1 and restored.
    /// Measures how long the monitor needs before a read returns the written value (basis for `VERIFY_DELAY`).
    /// `TITAN_I2C=/dev/i2c-14 cargo test write_readback_delay -- --ignored --nocapture`
    #[test]
    #[cfg(unix)]
    #[ignore]
    fn write_readback_delay() {
        let path = std::env::var("TITAN_I2C").unwrap_or_else(|_| "/dev/i2c-14".into());
        let mut ddc = MsiDdc::open(&path).expect("open i2c device");
        for key in [VCP_BRIGHTNESS, VCP_OSD_H_POS] {
            let original = ddc.get_vcp(key).expect("read original value");
            let target = if original >= 100 { original - 1 } else { original + 1 };
            for (step, value) in [target, original, target, original, target, original].into_iter().enumerate() {
                ddc.send_vcp(key, value).expect("write");
                let sent = Instant::now();
                let mut reads = 0;
                let elapsed = loop {
                    reads += 1;
                    match ddc.verify_vcp(key, value) {
                        Readback::Confirmed => break Some(sent.elapsed()),
                        _ if sent.elapsed() > Duration::from_secs(3) => break None,
                        _ => {}
                    }
                };
                println!("0x{key:02X} step {step}: {value} confirmed after {elapsed:?} ({reads} reads)");
            }
            assert_eq!(ddc.get_vcp(key).expect("read restored value"), original, "0x{key:02X} restored");
        }
    }

    /// Hardware test, reads only. The target is `TITAN_DDC_TARGET` (`/dev/i2c-14` on Linux, `\\.\DISPLAY6` on Windows;
    /// `TITAN_I2C` still works on Linux): `TITAN_DDC_TARGET=... cargo test reads_all_settings -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn reads_all_settings() {
        let mut ddc = open_ddc(&hardware_target()).expect("open the DDC/CI connection");
        let mut failed = Vec::new();
        for &key in GLOBAL_SETTING_KEYS.iter().chain(PROFILE_SETTING_KEYS) {
            match ddc.get_vcp(key) {
                Ok(value) => println!("0x{key:02X} = {value}"),
                Err(error) => {
                    println!("0x{key:02X}: {error}");
                    failed.push(key);
                }
            }
        }
        assert!(failed.is_empty(), "unreadable: {failed:02X?}");
    }

    /// Hardware test (reads only): `TITAN_DDC_TARGET=... cargo test profile_table -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn profile_table_reads_are_reliable() {
        let mut ddc = open_ddc(&hardware_target()).expect("open the DDC/CI connection");
        let mut first = None;
        for round in 0..10 {
            let started = Instant::now();
            let table = match ddc.read_active_profile(TABLE_CONFIRMATIONS).expect("table read") {
                ActiveProfile::Custom(table) => table,
                ActiveProfile::Default(scene_id) => {
                    println!("profile 0x{scene_id:02X} is a Default profile: no table, switch to a Custom one");
                    return;
                }
            };
            println!("round {round}: profile {} {:?} in {:?}", table.scene_id, &table.bytes[2..10], started.elapsed());
            match &first {
                None => first = Some(table.bytes),
                Some(bytes) => assert_eq!(bytes, &table.bytes),
            }
        }
    }
}

/// Development aid (debug builds): `TITAN_FAKE_MONITORS=2` makes the window believe in that many monitors, without
/// any hardware access, to check the multi-monitor UI. Commands go nowhere.
#[cfg(debug_assertions)]
pub fn fake_monitors(events: &async_channel::Sender<SessionEvent>) -> bool {
    let Some(count) = std::env::var("TITAN_FAKE_MONITORS").ok().and_then(|v| v.parse::<usize>().ok()) else {
        return false;
    };
    let ids: Vec<MonitorId> = (0..count)
        .map(|i| MonitorId {
            name: "P275MV PLUS".into(),
            edid_id: "LHC907D".into(),
            serial: "0000000000000".into(),
            path: format!("LHC907D{}FAKE{}", '\\', i + 1),
        })
        .collect();
    let events = events.clone();
    thread::spawn(move || {
        let send = |monitor: &MonitorId, cmd: UiCmd| {
            let _ = events.send_blocking(SessionEvent { monitor: monitor.clone(), cmd });
        };
        send(&ids[0], UiCmd::Detected(ids.clone()));
        for (index, id) in ids.iter().enumerate() {
            thread::sleep(Duration::from_millis(200 + 100 * index as u64));
            let brightness = 30 + 50 * index as u16;
            let settings = HashMap::from([(VCP_BRIGHTNESS, brightness), (VCP_CONTRAST, 40 + index as u16), (VCP_MODE, 0x1A)]);
            send(
                id,
                UiCmd::MonitorFound(MonitorInfo {
                    name: id.name.clone(),
                    connector: format!("{0}{0}.{0}DISPLAY{1}", '\\', index + 6),
                    resolution: "2560x1440".into(),
                    hz: 144,
                    firm: "v 5.1.1".into(),
                    usage_mins: 100 * (index as u16 + 1),
                    settings,
                }),
            );
        }
    });
    true
}
