//! Windows platform layer: display enumeration, EDID, and the DDC/CI transport over the Monitor Configuration API
//! (Dxva2). Prototype: `tools/ddc_probe/src/windows_ddc.rs`.
//!
//! Writes (`write_frame`) are reached only through `MsiDdc::send_vcp`, which applies the write level chosen in the
//! window (read-only at every start), the dangerous-write guard and the identity check first.
//! Checked on two P275MV PLUS monitors (reads, manufacturer registers, writes); what was not is in docs/windows-port.md.

use crate::monitor::transport::{FrameError, VcpReply, VcpTransport};
use std::io;
use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR,
    GetVCPFeatureAndVCPFeatureReply, MC_VCP_CODE_TYPE, PHYSICAL_MONITOR, SetVCPFeature,
};
use windows::Win32::Foundation::{
    BOOL, ERROR_GRAPHICS_DDCCI_CURRENT_CURRENT_VALUE_GREATER_THAN_MAXIMUM_VALUE, ERROR_GRAPHICS_DDCCI_INVALID_DATA,
    ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_CHECKSUM, ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_COMMAND,
    ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_LENGTH, ERROR_GRAPHICS_DDCCI_VCP_NOT_SUPPORTED, ERROR_GRAPHICS_MONITOR_NO_LONGER_EXISTS, GetLastError, HANDLE,
    LPARAM, RECT, SetLastError, WIN32_ERROR,
};
use windows::Win32::Graphics::Gdi::{
    DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    MONITORINFOEXW,
};
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_BINARY, RegGetValueW};
use windows::core::{HRESULT, PCWSTR, w};

/// `EDD_GET_DEVICE_INTERFACE_NAME`: DeviceID becomes the monitor interface path `\\?\DISPLAY#...`.
const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 0x1;

/// Keeps the system timer at 1 ms while alive (accuracy of the pacing delays).
pub struct TimerResolution;

impl TimerResolution {
    pub fn new() -> Self {
        // SAFETY: plain call, balanced in Drop.
        unsafe { timeBeginPeriod(1) };
        TimerResolution
    }
}

impl Drop for TimerResolution {
    fn drop(&mut self) {
        // SAFETY: balances timeBeginPeriod(1).
        unsafe { timeEndPeriod(1) };
    }
}

/// One display as seen by Windows.
pub struct WinDisplay {
    /// `\\.\DISPLAYn`
    pub device: String,
    /// left, top, right, bottom (in the DPI context of the calling thread: virtualised when the process is not
    /// DPI aware)
    pub rect: (i32, i32, i32, i32),
    /// Physical size in pixels (from the current display mode; independent of DPI awareness).
    pub physical: Option<(u32, u32)>,
    /// `<hwid>\<instance>` under `HKLM\SYSTEM\CurrentControlSet\Enum\DISPLAY` (identity of the physical monitor).
    pub registry_key: Option<String>,
    pub edid: Option<Vec<u8>>,
    hmonitor: HMONITOR,
}

impl WinDisplay {
    pub fn resolution(&self) -> String {
        match self.physical {
            Some((w, h)) => format!("{w}x{h}"),
            None => format!("{}x{}", self.rect.2 - self.rect.0, self.rect.3 - self.rect.1),
        }
    }
}

fn wide_to_string(wide: &[u16]) -> String {
    let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}

unsafe extern "system" fn collect_monitors(hmon: HMONITOR, _hdc: HDC, _rect: *mut RECT, data: LPARAM) -> BOOL {
    // SAFETY: `data` is the pointer to the Vec passed by `enumerate_displays`.
    let list = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
    list.push(hmon);
    BOOL(1)
}

pub fn enumerate_displays() -> Vec<WinDisplay> {
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only pushes into `handles`, which outlives the call.
    let ok = unsafe {
        EnumDisplayMonitors(None, None, Some(collect_monitors), LPARAM(&mut handles as *mut _ as isize))
    };
    if !ok.as_bool() {
        log!("[WIN] EnumDisplayMonitors failed");
        return Vec::new();
    }

    let mut result = Vec::new();
    for hmonitor in handles {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: `info` is a valid MONITORINFOEXW with cbSize set; the pointer cast is the documented usage.
        let ok = unsafe { GetMonitorInfoW(hmonitor, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) };
        if !ok.as_bool() {
            continue;
        }
        let device = wide_to_string(&info.szDevice);
        let rc = info.monitorInfo.rcMonitor;
        let mut display = WinDisplay {
            device: device.clone(),
            rect: (rc.left, rc.top, rc.right, rc.bottom),
            physical: None,
            registry_key: None,
            edid: None,
            hmonitor,
        };

        let device_wide: Vec<u16> = device.encode_utf16().chain(std::iter::once(0)).collect();
        let mut current = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
        // SAFETY: `device_wide` is NUL-terminated and `current` has dmSize set.
        if unsafe {
            EnumDisplaySettingsExW(PCWSTR(device_wide.as_ptr()), ENUM_CURRENT_SETTINGS, &mut current, ENUM_DISPLAY_SETTINGS_FLAGS(0))
        }
        .as_bool()
        {
            display.physical = Some((current.dmPelsWidth, current.dmPelsHeight));
        }
        let mut dd = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        // SAFETY: `device_wide` is NUL-terminated, `dd` has cb set.
        let found = unsafe {
            EnumDisplayDevicesW(PCWSTR(device_wide.as_ptr()), 0, &mut dd, EDD_GET_DEVICE_INTERFACE_NAME)
        };
        if found.as_bool() {
            let path = wide_to_string(&dd.DeviceID);
            display.registry_key = registry_key_from_interface(&path);
            match display.registry_key.as_deref().map(read_edid) {
                Some(Ok(data)) => display.edid = Some(data),
                Some(Err(error)) => log!("[WIN] {device}: {error}"),
                None => log!("[WIN] {device}: cannot parse device interface path '{path}'"),
            }
        }
        result.push(display);
    }
    result
}

/// `\\?\DISPLAY#MSI3ACF#5&1b2c&0&UID4352#{guid}` -> `MSI3ACF\5&1b2c&0&UID4352`.
fn registry_key_from_interface(path: &str) -> Option<String> {
    let trimmed = path.trim_start_matches("\\\\?\\");
    let mut parts = trimmed.split('#');
    if !parts.next()?.eq_ignore_ascii_case("DISPLAY") {
        return None;
    }
    let hwid = parts.next()?;
    let instance = parts.next()?;
    if hwid.is_empty() || instance.is_empty() {
        return None;
    }
    Some(format!("{hwid}\\{instance}"))
}

fn read_edid(registry_key: &str) -> Result<Vec<u8>, String> {
    let subkey = format!("SYSTEM\\CurrentControlSet\\Enum\\DISPLAY\\{registry_key}\\Device Parameters");
    let subkey_wide: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let mut len = 0u32;
    // SAFETY: first call only queries the size.
    let status = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, PCWSTR(subkey_wide.as_ptr()), w!("EDID"), RRF_RT_REG_BINARY, None, None, Some(&mut len))
    };
    if status.0 != 0 {
        return Err(format!("registry {subkey}\\EDID: error {}", status.0));
    }
    let mut data = vec![0u8; len as usize];
    // SAFETY: `data` has `len` bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey_wide.as_ptr()),
            w!("EDID"),
            RRF_RT_REG_BINARY,
            None,
            Some(data.as_mut_ptr() as *mut _),
            Some(&mut len),
        )
    };
    if status.0 != 0 {
        return Err(format!("registry {subkey}\\EDID: error {}", status.0));
    }
    data.truncate(len as usize);
    Ok(data)
}

fn classify(raw: u32) -> FrameError {
    let is = |code: HRESULT| code.0 as u32 == raw;
    if is(ERROR_GRAPHICS_MONITOR_NO_LONGER_EXISTS) {
        FrameError::Disconnected
    } else if is(ERROR_GRAPHICS_DDCCI_VCP_NOT_SUPPORTED) {
        FrameError::Unsupported
    } else if is(ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_CHECKSUM)
        || is(ERROR_GRAPHICS_DDCCI_INVALID_DATA)
        || is(ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_COMMAND)
        || is(ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_LENGTH)
        || is(ERROR_GRAPHICS_DDCCI_CURRENT_CURRENT_VALUE_GREATER_THAN_MAXIMUM_VALUE)
    {
        FrameError::Garbage
    } else {
        FrameError::Io(io::Error::other(format!("Dxva2 error 0x{raw:08X}")))
    }
}

/// Dxva2 transport. Owns the physical monitor handles of one display and destroys them on drop.
pub struct WinDxva2 {
    monitors: Vec<PHYSICAL_MONITOR>,
    handle: HANDLE,
    description: String,
}

// SAFETY: the handle is only used by the thread that owns the transport (the worker thread); it is never shared.
unsafe impl Send for WinDxva2 {}

impl Drop for WinDxva2 {
    fn drop(&mut self) {
        // SAFETY: the handles came from GetPhysicalMonitorsFromHMONITOR and are destroyed exactly once.
        let _ = unsafe { DestroyPhysicalMonitors(&self.monitors) };
    }
}

impl VcpTransport for WinDxva2 {
    fn get_frame(&mut self, code: u8) -> Result<VcpReply, FrameError> {
        let mut vcp_type = MC_VCP_CODE_TYPE::default();
        let mut current = 0u32;
        let mut maximum = 0u32;
        // SAFETY: valid handle owned by `self`, valid out pointers.
        let ok = unsafe {
            SetLastError(WIN32_ERROR(0));
            GetVCPFeatureAndVCPFeatureReply(self.handle, code, Some(&mut vcp_type), &mut current, Some(&mut maximum))
        };
        if ok == 0 {
            // SAFETY: reads the thread's last error right after the failing call.
            let raw = unsafe { GetLastError() }.0;
            return Err(classify(raw));
        }
        if current > 0xFFFF || maximum > 0xFFFF {
            return Err(FrameError::Garbage);
        }
        Ok(VcpReply { maximum: maximum as u16, current: current as u16 })
    }

    /// UNVERIFIED-ON-HW. Reached only through `MsiDdc::send_vcp`, which applies the write level, the dangerous-write
    /// guard and the identity check first.
    fn write_frame(&mut self, code: u8, value: u16) -> io::Result<()> {
        // SAFETY: valid handle owned by `self`.
        let ok = unsafe { SetVCPFeature(self.handle, code, value as u32) };
        if ok == 0 { Err(io::Error::other("SetVCPFeature failed")) } else { Ok(()) }
    }

    fn describe(&self) -> String {
        format!("Dxva2 '{}'", self.description)
    }
}

/// Opens the DDC/CI transport of the display `device` (`\\.\DISPLAYn`). Only a display whose EDID says P275MV is
/// opened; anything else is refused, so no other monitor ever gets DDC traffic.
pub fn open_transport(device: &str) -> io::Result<Box<dyn VcpTransport>> {
    let display = enumerate_displays()
        .into_iter()
        .find(|display| display.device.eq_ignore_ascii_case(device))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("display {device} not found")))?;
    if !display.edid.as_deref().is_some_and(crate::monitor::edid_is_p275mv) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{device} is not a P275MV monitor")));
    }

    let mut count = 0u32;
    // SAFETY: plain query with a valid out pointer.
    unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(display.hmonitor, &mut count) }
        .map_err(|error| io::Error::other(format!("GetNumberOfPhysicalMonitorsFromHMONITOR: {error}")))?;
    if count != 1 {
        return Err(io::Error::other(format!("{device}: {count} physical monitors on one output (clone mode?)")));
    }
    let mut monitors = vec![PHYSICAL_MONITOR::default(); count as usize];
    // SAFETY: the slice has exactly `count` elements.
    unsafe { GetPhysicalMonitorsFromHMONITOR(display.hmonitor, &mut monitors) }
        .map_err(|error| io::Error::other(format!("GetPhysicalMonitorsFromHMONITOR: {error}")))?;
    // Packed struct: copy the fields out before use.
    let monitor = monitors[0];
    let handle = monitor.hPhysicalMonitor;
    let description = wide_to_string(&{ monitor.szPhysicalMonitorDescription });
    Ok(Box::new(WinDxva2 { monitors, handle, description }))
}

/// Identity (EDID + device path) of the monitor that `device` (`\.\DISPLAYn`) is at this moment. Used before the
/// first write after connecting: if the numbering of the displays changed, the identity differs from the one the
/// session was opened for and the write is refused.
pub fn current_identity(device: &str) -> Result<crate::monitor::MonitorId, String> {
    let display = enumerate_displays()
        .into_iter()
        .find(|display| display.device.eq_ignore_ascii_case(device))
        .ok_or_else(|| format!("display {device} not found"))?;
    let edid = display.edid.as_deref().ok_or_else(|| format!("{device}: no EDID"))?;
    let path = display.registry_key.clone().unwrap_or_default();
    crate::monitor::identity::from_edid(edid, &path).ok_or_else(|| format!("{device}: invalid EDID"))
}

// ---- display changes ----------------------------------------------------------------------------------------

use std::sync::{Mutex, OnceLock, mpsc};
use windows::Win32::Foundation::{HWND, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, RegisterClassW, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_DEVICECHANGE, WM_DISPLAYCHANGE, WM_POWERBROADCAST, WNDCLASSW,
};

static CHANGE_TX: OnceLock<Mutex<mpsc::Sender<()>>> = OnceLock::new();

/// Window class of the watcher; the process id keeps another running instance (or a test) from being addressed.
fn watcher_class_name() -> Vec<u16> {
    format!("TitanControlDisplayWatcher{}\0", std::process::id()).encode_utf16().collect()
}

unsafe extern "system" fn watcher_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Display added/removed/reconfigured, device nodes changed (cable, USB-C, dock), resume from sleep.
    if matches!(msg, WM_DISPLAYCHANGE | WM_DEVICECHANGE | WM_POWERBROADCAST)
        && let Some(tx) = CHANGE_TX.get()
        && let Ok(tx) = tx.lock()
    {
        let _ = tx.send(());
    }
    // SAFETY: forwards the message unchanged.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Starts a thread with a hidden window that reports every display change (`WM_DISPLAYCHANGE`, `WM_DEVICECHANGE`,
/// `WM_POWERBROADCAST`) on `tx`. Bursts are normal: the receiver should wait a moment and re-enumerate once.
/// A hidden top-level window is needed: message-only windows do not receive these broadcasts.
pub fn spawn_display_watcher(tx: mpsc::Sender<()>) {
    if CHANGE_TX.set(Mutex::new(tx)).is_err() {
        return;
    }
    std::thread::spawn(|| {
        // SAFETY: standard window class registration and message loop on this thread; the window is never shown.
        unsafe {
            let Ok(instance) = GetModuleHandleW(None) else { return };
            let class_name = watcher_class_name();
            let class = PCWSTR(class_name.as_ptr());
            let wc = WNDCLASSW {
                lpfnWndProc: Some(watcher_proc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                log!("[WIN] display watcher: RegisterClassW failed");
                return;
            }
            let Ok(hwnd) = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!("Titan Control display watcher"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                instance,
                None,
            ) else {
                log!("[WIN] display watcher: CreateWindowExW failed");
                return;
            };
            let _ = hwnd;
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SendMessageW};

    /// The hidden window is created and a `WM_DISPLAYCHANGE` reaches the channel (no monitor is touched).
    #[test]
    fn display_watcher_reports_a_display_change() {
        let (tx, rx) = mpsc::channel();
        spawn_display_watcher(tx);
        let mut hwnd = HWND::default();
        for _ in 0..50 {
            // SAFETY: plain lookup by class name.
            hwnd = unsafe { FindWindowW(PCWSTR(watcher_class_name().as_ptr()), None) }.unwrap_or_default();
            if !hwnd.is_invalid() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!hwnd.is_invalid(), "the watcher window was not created");
        // SAFETY: sends a message to our own hidden window.
        unsafe { SendMessageW(hwnd, WM_DISPLAYCHANGE, WPARAM(0), LPARAM(0)) };
        assert!(rx.recv_timeout(Duration::from_secs(2)).is_ok(), "no notification arrived");
    }
}

// ---- display mode (read-only) -------------------------------------------------------------------------------

use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, ENUM_DISPLAY_SETTINGS_FLAGS, ENUM_DISPLAY_SETTINGS_MODE, EnumDisplaySettingsExW};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetThreadDpiAwarenessContext};

/// A display mode: width, height, refresh rate in Hz.
pub type DisplayMode = (u32, u32, u32);

pub struct DisplayModes {
    pub current: DisplayMode,
    /// Every mode the display supports (with duplicates removed).
    pub modes: Vec<DisplayMode>,
    /// Effective scaling of the display in percent (100 = 96 dpi).
    pub scale_percent: u16,
}

fn devmode_to_mode(mode: &DEVMODEW) -> DisplayMode {
    (mode.dmPelsWidth, mode.dmPelsHeight, mode.dmDisplayFrequency)
}

/// Current mode, supported modes and scaling of `device` (`\.\DISPLAYn`), or of the first display when `None`.
/// Only reads: nothing about the display is changed.
pub fn query_display_modes(device: Option<&str>) -> Option<(String, DisplayModes)> {
    let displays = enumerate_displays();
    let display = match device {
        Some(device) => displays.iter().find(|d| d.device.eq_ignore_ascii_case(device))?,
        None => displays.first()?,
    };
    let name: Vec<u16> = display.device.encode_utf16().chain(std::iter::once(0)).collect();

    let mut current = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
    // SAFETY: `name` is NUL-terminated and `current` has dmSize set.
    if !unsafe { EnumDisplaySettingsExW(PCWSTR(name.as_ptr()), ENUM_CURRENT_SETTINGS, &mut current, ENUM_DISPLAY_SETTINGS_FLAGS(0)) }
        .as_bool()
    {
        return None;
    }

    let mut modes: Vec<DisplayMode> = Vec::new();
    for index in 0..4096u32 {
        let mut mode = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
        // SAFETY: as above.
        let found = unsafe {
            EnumDisplaySettingsExW(PCWSTR(name.as_ptr()), ENUM_DISPLAY_SETTINGS_MODE(index), &mut mode, ENUM_DISPLAY_SETTINGS_FLAGS(0))
        };
        if !found.as_bool() {
            break;
        }
        let entry = devmode_to_mode(&mode);
        if entry.2 > 1 && !modes.contains(&entry) {
            modes.push(entry);
        }
    }

    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    // Without per-monitor awareness Windows answers 96 dpi for everything: ask in an aware context (this thread only).
    // SAFETY: thread-scoped context switch, restored right after the query; valid handle and out pointers.
    unsafe {
        let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = GetDpiForMonitor(display.hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        SetThreadDpiAwarenessContext(previous);
    }
    let scale_percent = ((dpi_x as f64 / 96.0) * 100.0).round() as u16;

    Some((display.device.clone(), DisplayModes { current: devmode_to_mode(&current), modes, scale_percent }))
}

#[cfg(test)]
mod display_mode_tests {
    use super::*;

    /// Reads the real display mode of this computer (no monitor traffic, nothing is changed).
    #[test]
    fn reads_the_current_display_mode() {
        let Some((device, modes)) = query_display_modes(None) else {
            eprintln!("no display in this session: skipped");
            return;
        };
        println!("{device}: {:?}, scale {} %, {} modes", modes.current, modes.scale_percent, modes.modes.len());
        assert!(modes.current.0 >= 640 && modes.current.1 >= 480 && modes.current.2 >= 24);
        assert!(modes.modes.contains(&modes.current) || modes.modes.is_empty());
        assert!(modes.scale_percent >= 100);
    }
}

// ---- theme ----------------------------------------------------------------------------------------------------

use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_DWORD, RegDeleteKeyValueW, RegSetKeyValueW,
};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn read_user_dword(subkey: &str, value: &str) -> Option<u32> {
    let subkey = wide(subkey);
    let value = wide(value);
    let mut data = 0u32;
    let mut len = size_of::<u32>() as u32;
    // SAFETY: valid NUL-terminated names and a 4-byte output buffer.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut len),
        )
    };
    (status.0 == 0).then_some(data)
}

const PERSONALIZE: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";

/// Whether applications use the light theme (`AppsUseLightTheme`); `None` when Windows does not say.
pub fn apps_use_light_theme() -> Option<bool> {
    read_user_dword(PERSONALIZE, "AppsUseLightTheme").map(|v| v != 0)
}

/// Whether the taskbar (and so the tray) is light (`SystemUsesLightTheme`); `None` when Windows does not say.
pub fn system_uses_light_theme() -> Option<bool> {
    read_user_dword(PERSONALIZE, "SystemUsesLightTheme").map(|v| v != 0)
}

// ---- autostart ------------------------------------------------------------------------------------------------

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
pub const AUTOSTART_VALUE: &str = "TitanControl";

/// Adds or removes the `HKCU\...\Run` entry that starts the app (minimised, to the tray) at logon.
pub fn set_autostart_value(value_name: &str, command: Option<&str>) -> io::Result<()> {
    let key = wide(RUN_KEY);
    let name = wide(value_name);
    match command {
        Some(command) => {
            let data = wide(command);
            // SAFETY: NUL-terminated strings; the size is in bytes and includes the terminator.
            let status = unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    PCWSTR(key.as_ptr()),
                    PCWSTR(name.as_ptr()),
                    REG_SZ.0,
                    Some(data.as_ptr() as *const _),
                    (data.len() * 2) as u32,
                )
            };
            if status.0 != 0 {
                return Err(io::Error::from_raw_os_error(status.0 as i32));
            }
        }
        None => {
            // SAFETY: NUL-terminated strings.
            let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR(name.as_ptr())) };
            // 2 = ERROR_FILE_NOT_FOUND: nothing to remove.
            if status.0 != 0 && status.0 != 2 {
                return Err(io::Error::from_raw_os_error(status.0 as i32));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub fn autostart_value(value_name: &str) -> Option<String> {
    use windows::Win32::System::Registry::RRF_RT_REG_SZ;
    let key = wide(RUN_KEY);
    let name = wide(value_name);
    let mut len = 0u32;
    // SAFETY: first call only queries the size.
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR(name.as_ptr()), RRF_RT_REG_SZ, None, None, Some(&mut len))
    };
    if status.0 != 0 {
        return None;
    }
    let mut data = vec![0u16; (len as usize).div_ceil(2)];
    // SAFETY: the buffer holds `len` bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(data.as_mut_ptr() as *mut _),
            Some(&mut len),
        )
    };
    (status.0 == 0).then(|| wide_to_string(&data))
}

// ---- single instance ------------------------------------------------------------------------------------------

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent, WaitForSingleObject,
};

/// Keeps the "one instance" mutex alive; dropping it lets the next instance start.
pub struct SingleInstance {
    mutex: HANDLE,
    event: HANDLE,
}

// SAFETY: the handles are only closed in Drop; waiting on them from another thread is allowed.
unsafe impl Send for SingleInstance {}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        // SAFETY: handles created by this struct, closed once.
        unsafe {
            let _ = CloseHandle(self.event);
            let _ = CloseHandle(self.mutex);
        }
    }
}

/// Two instances must never talk to the monitors at the same time. The first instance takes a named
/// mutex and an event; a later one signals the event ("show your window") and gets `None`, so it must exit.
pub fn acquire_single_instance(name: &str) -> Option<(SingleInstance, mpsc::Receiver<()>)> {
    let mutex_name = wide(&format!("Local\\{name}.Instance"));
    let event_name = wide(&format!("Local\\{name}.Show"));
    // SAFETY: named kernel objects with NUL-terminated names.
    unsafe {
        let mutex = CreateMutexW(None, true, PCWSTR(mutex_name.as_ptr())).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(mutex);
            if let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(event_name.as_ptr())) {
                let _ = SetEvent(event);
                let _ = CloseHandle(event);
            }
            return None;
        }
        let event = CreateEventW(None, false, false, PCWSTR(event_name.as_ptr())).ok()?;
        let (tx, rx) = mpsc::channel();
        let raw = event.0 as usize;
        std::thread::spawn(move || {
            let event = HANDLE(raw as *mut _);
            // Ends with the process; a signal means "another instance wants the window shown".
            while WaitForSingleObject(event, INFINITE) == WAIT_OBJECT_0 {
                if tx.send(()).is_err() {
                    break;
                }
            }
        });
        Some((SingleInstance { mutex, event }, rx))
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    fn autostart_entry_is_written_read_and_removed() {
        let name = "TitanControlTest";
        let command = "\"C:\\x\\titan_control.exe\" --minimized";
        set_autostart_value(name, Some(command)).expect("write");
        assert_eq!(autostart_value(name).as_deref(), Some(command));
        set_autostart_value(name, None).expect("remove");
        assert_eq!(autostart_value(name), None);
        // Removing what is not there is not an error.
        set_autostart_value(name, None).expect("remove again");
    }

    #[test]
    fn second_instance_is_refused_and_signals_the_first() {
        let name = format!("TitanControlTest{}", std::process::id());
        let (first, shown) = acquire_single_instance(&name).expect("first instance");
        assert!(acquire_single_instance(&name).is_none(), "a second instance must be refused");
        assert!(shown.recv_timeout(std::time::Duration::from_secs(2)).is_ok(), "the first instance was not told to show");
        drop(first);
    }

    #[test]
    fn theme_registry_values_can_be_read() {
        // Present on every Windows 10/11 user profile; the call must at least not crash.
        println!("apps light: {:?}, system light: {:?}", apps_use_light_theme(), system_uses_light_theme());
    }
}

// ---- display settings ---------------------------------------------------------------------------------------

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE,
    DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    DisplayConfigSetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Gdi::{
    CDS_TEST, CDS_TYPE, CDS_UPDATEREGISTRY, ChangeDisplaySettingsExW, DEVMODE_FIELD_FLAGS, DISP_CHANGE,
    DISP_CHANGE_BADDUALVIEW, DISP_CHANGE_BADFLAGS, DISP_CHANGE_BADMODE, DISP_CHANGE_BADPARAM,
    DISP_CHANGE_FAILED, DISP_CHANGE_NOTUPDATED, DISP_CHANGE_RESTART, DISP_CHANGE_SUCCESSFUL,
};

const DM_PELSWIDTH: u32 = 0x0008_0000;
const DM_PELSHEIGHT: u32 = 0x0010_0000;
const DM_DISPLAYFREQUENCY: u32 = 0x0040_0000;

fn display_change_text(code: DISP_CHANGE) -> String {
    let name = match code {
        c if c == DISP_CHANGE_SUCCESSFUL => "success",
        c if c == DISP_CHANGE_RESTART => "a restart is needed",
        c if c == DISP_CHANGE_FAILED => "the display driver failed the mode",
        c if c == DISP_CHANGE_BADMODE => "the mode is not supported",
        c if c == DISP_CHANGE_NOTUPDATED => "the setting could not be written to the registry",
        c if c == DISP_CHANGE_BADFLAGS => "bad flags",
        c if c == DISP_CHANGE_BADPARAM => "bad parameter",
        c if c == DISP_CHANGE_BADDUALVIEW => "not possible with this multi-display setup",
        _ => "unknown error",
    };
    format!("{name} (code {})", code.0)
}

/// The current mode of `device` as a DEVMODE, with the width, height and refresh rate of `mode` put in.
fn devmode_with(device: &[u16], mode: DisplayMode) -> Option<DEVMODEW> {
    let mut devmode = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
    // SAFETY: `device` is NUL-terminated and `devmode` has dmSize set.
    if !unsafe { EnumDisplaySettingsExW(PCWSTR(device.as_ptr()), ENUM_CURRENT_SETTINGS, &mut devmode, ENUM_DISPLAY_SETTINGS_FLAGS(0)) }
        .as_bool()
    {
        return None;
    }
    devmode.dmPelsWidth = mode.0;
    devmode.dmPelsHeight = mode.1;
    devmode.dmDisplayFrequency = mode.2;
    devmode.dmFields = DEVMODE_FIELD_FLAGS(DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY);
    Some(devmode)
}

/// Asks the driver whether it would accept `mode` on `device` (`\\.\DISPLAYn`) without changing anything.
pub fn test_display_mode(device: &str, mode: DisplayMode) -> Result<(), String> {
    let name = wide(device);
    let devmode = devmode_with(&name, mode).ok_or("the current display mode could not be read")?;
    // SAFETY: valid NUL-terminated name and DEVMODE; CDS_TEST changes nothing.
    let result = unsafe { ChangeDisplaySettingsExW(PCWSTR(name.as_ptr()), Some(&devmode), None, CDS_TEST, None) };
    if result == DISP_CHANGE_SUCCESSFUL { Ok(()) } else { Err(display_change_text(result)) }
}

/// Changes the mode of ONE display. Without `persist` the change is not written to the registry: reverting
/// ([`revert_display_mode`]) returns to the saved mode, so a mode the user cannot see (black screen) is never kept.
pub fn apply_display_mode(device: &str, mode: DisplayMode, persist: bool) -> Result<(), String> {
    test_display_mode(device, mode)?;
    let name = wide(device);
    let devmode = devmode_with(&name, mode).ok_or("the current display mode could not be read")?;
    let flags = if persist { CDS_UPDATEREGISTRY } else { CDS_TYPE(0) };
    // SAFETY: valid NUL-terminated name and DEVMODE, tested just before.
    let result = unsafe { ChangeDisplaySettingsExW(PCWSTR(name.as_ptr()), Some(&devmode), None, flags, None) };
    if result == DISP_CHANGE_SUCCESSFUL { Ok(()) } else { Err(display_change_text(result)) }
}

/// Puts `device` back to its saved (registry) mode, undoing a change made without `persist`.
pub fn revert_display_mode(device: &str) -> Result<(), String> {
    let name = wide(device);
    // SAFETY: a NULL DEVMODE means "use the saved settings".
    let result = unsafe { ChangeDisplaySettingsExW(PCWSTR(name.as_ptr()), None, None, CDS_TYPE(0), None) };
    if result == DISP_CHANGE_SUCCESSFUL { Ok(()) } else { Err(display_change_text(result)) }
}

/// Writes the mode that `device` shows now to the registry (the user kept it).
pub fn persist_current_display_mode(device: &str) -> Result<(), String> {
    let name = wide(device);
    let mut current = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
    // SAFETY: valid NUL-terminated name, dmSize set.
    if !unsafe { EnumDisplaySettingsExW(PCWSTR(name.as_ptr()), ENUM_CURRENT_SETTINGS, &mut current, ENUM_DISPLAY_SETTINGS_FLAGS(0)) }
        .as_bool()
    {
        return Err("the current display mode could not be read".into());
    }
    // SAFETY: the mode comes from the driver.
    let result = unsafe { ChangeDisplaySettingsExW(PCWSTR(name.as_ptr()), Some(&current), None, CDS_UPDATEREGISTRY, None) };
    if result == DISP_CHANGE_SUCCESSFUL { Ok(()) } else { Err(display_change_text(result)) }
}

/// Adapter and target id of the display `device` in the display configuration (what HDR is asked about).
fn display_target(device: &str) -> Option<(LUID, u32)> {
    let (mut paths_len, mut modes_len) = (0u32, 0u32);
    // SAFETY: plain size query.
    if unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths_len, &mut modes_len) }.is_err() {
        return None;
    }
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); paths_len as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); modes_len as usize];
    // SAFETY: the buffers have the sizes that were asked for.
    if unsafe { QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut paths_len, paths.as_mut_ptr(), &mut modes_len, modes.as_mut_ptr(), None) }
        .is_err()
    {
        return None;
    }
    for path in &paths[..paths_len as usize] {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            },
            ..Default::default()
        };
        // SAFETY: the header describes the structure it is the first field of.
        if unsafe { DisplayConfigGetDeviceInfo(&mut source.header) } != 0 {
            continue;
        }
        if wide_to_string(&source.viewGdiDeviceName).eq_ignore_ascii_case(device) {
            return Some((path.targetInfo.adapterId, path.targetInfo.id));
        }
    }
    None
}

/// HDR of one display in Windows: `(supported, enabled)`; `None` when it cannot be asked.
pub fn hdr_state(device: &str) -> Option<(bool, bool)> {
    let (adapter, id) = display_target(device)?;
    let mut info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
            size: size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32,
            adapterId: adapter,
            id,
        },
        ..Default::default()
    };
    // SAFETY: the header describes the structure it is the first field of.
    if unsafe { DisplayConfigGetDeviceInfo(&mut info.header) } != 0 {
        return None;
    }
    // SAFETY: both union members are plain u32 bit sets.
    let value = unsafe { info.Anonymous.value };
    Some((value & 0b01 != 0, value & 0b10 != 0))
}

/// Turns HDR ("advanced color") of ONE display on or off.
pub fn set_hdr(device: &str, enable: bool) -> Result<(), String> {
    let (adapter, id) = display_target(device).ok_or("the display was not found in the display configuration")?;
    let mut state = DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE,
            size: size_of::<DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE>() as u32,
            adapterId: adapter,
            id,
        },
        ..Default::default()
    };
    state.Anonymous.value = enable as u32;
    // SAFETY: the header describes the structure it is the first field of.
    let result = unsafe { DisplayConfigSetDeviceInfo(&state.header) };
    if result == 0 { Ok(()) } else { Err(format!("Windows refused to change HDR (error {result})")) }
}

#[cfg(test)]
mod display_settings_tests {
    use super::*;

    /// Nothing is changed: `CDS_TEST` only asks the driver; the HDR state is only read.
    #[test]
    fn driver_accepts_the_current_mode_and_refuses_nonsense() {
        let Some((device, modes)) = query_display_modes(None) else {
            eprintln!("no display in this session: skipped");
            return;
        };
        assert_eq!(test_display_mode(&device, modes.current), Ok(()));
        assert!(test_display_mode(&device, (123, 45, 6)).is_err(), "a nonsense mode must be refused");
        // Every mode the list offers is one the driver would take.
        let sample: Vec<_> = modes.modes.iter().take(5).copied().collect();
        for mode in sample {
            assert_eq!(test_display_mode(&device, mode), Ok(()), "{mode:?}");
        }
        println!("{device}: hdr (supported, enabled) = {:?}", hdr_state(&device));
    }
}

/// How Windows treats this process with regard to DPI, for the log (0 unaware, 1 system aware, 2 per-monitor aware),
/// the system DPI and the DPI of the primary display.
pub fn dpi_report() -> String {
    use windows::Win32::UI::HiDpi::{GetAwarenessFromDpiAwarenessContext, GetDpiForSystem, GetThreadDpiAwarenessContext};
    // SAFETY: plain queries without arguments.
    let (awareness, system_dpi) = unsafe { (GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext()), GetDpiForSystem()) };
    let scale = query_display_modes(None).map(|(_, modes)| modes.scale_percent);
    format!("DPI awareness {} (0 unaware, 1 system, 2 per-monitor), system DPI {system_dpi}, Windows scaling of the first display {scale:?} %", awareness.0)
}

#[cfg(test)]
mod mode_list_probe {
    /// Prints the modes of the first display (reads only): `cargo test print_display_modes -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn print_display_modes() {
        let Some((device, info)) = super::query_display_modes(None) else { return };
        println!("{device}: current {:?}", info.current);
        let mut modes = info.modes.clone();
        modes.sort();
        for resolution in [(1920u32, 1080u32), (2560, 1440), (3840, 2160)] {
            let rates: Vec<u32> = modes.iter().filter(|m| (m.0, m.1) == resolution).map(|m| m.2).collect();
            println!("{}x{}: {:?}", resolution.0, resolution.1, rates);
        }
    }
}
