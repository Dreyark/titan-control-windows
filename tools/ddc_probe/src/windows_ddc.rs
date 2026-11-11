//! Windows side: monitor enumeration, EDID, `MonitorId` ingredients and the Dxva2 transport.
//! Prototype of `src/platform/windows.rs`. Independent of the probe logic.
//!
//! READ-ONLY by construction: the only DDC call used is `GetVCPFeatureAndVCPFeatureReply`.
//! UNVERIFIED-ON-HW: error mapping and the behaviour of Dxva2 for manufacturer registers.

use crate::transport::{FrameError, VcpReply, VcpTransport};
use std::marker::PhantomData;
use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR,
    GetVCPFeatureAndVCPFeatureReply, MC_VCP_CODE_TYPE, PHYSICAL_MONITOR,
};
use windows::Win32::Foundation::{
    ERROR_GRAPHICS_DDCCI_CURRENT_CURRENT_VALUE_GREATER_THAN_MAXIMUM_VALUE, ERROR_GRAPHICS_DDCCI_INVALID_DATA,
    ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_CHECKSUM, ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_COMMAND,
    ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_LENGTH, ERROR_GRAPHICS_DDCCI_MONITOR_RETURNED_INVALID_TIMING_STATUS_BYTE,
    ERROR_GRAPHICS_DDCCI_VCP_NOT_SUPPORTED, ERROR_GRAPHICS_I2C_ERROR_RECEIVING_DATA,
    ERROR_GRAPHICS_I2C_ERROR_TRANSMITTING_DATA, ERROR_GRAPHICS_I2C_NOT_SUPPORTED, GetLastError, HANDLE, LPARAM,
    RECT, SetLastError, WIN32_ERROR, BOOL,
};
use windows::Win32::Graphics::Gdi::{
    DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    MONITORINFOEXW,
};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_BINARY, RegGetValueW};
use windows::core::{HRESULT, PCWSTR, w};

/// `EDD_GET_DEVICE_INTERFACE_NAME`: DeviceID becomes the monitor interface path `\\?\DISPLAY#...`.
const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 0x1;

pub fn win_error_name(raw: u32) -> String {
    let known: [(HRESULT, &str); 10] = [
        (ERROR_GRAPHICS_DDCCI_VCP_NOT_SUPPORTED, "DDCCI_VCP_NOT_SUPPORTED"),
        (ERROR_GRAPHICS_DDCCI_INVALID_DATA, "DDCCI_INVALID_DATA"),
        (ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_CHECKSUM, "DDCCI_INVALID_MESSAGE_CHECKSUM"),
        (ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_COMMAND, "DDCCI_INVALID_MESSAGE_COMMAND"),
        (ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_LENGTH, "DDCCI_INVALID_MESSAGE_LENGTH"),
        (
            ERROR_GRAPHICS_DDCCI_MONITOR_RETURNED_INVALID_TIMING_STATUS_BYTE,
            "DDCCI_MONITOR_RETURNED_INVALID_TIMING_STATUS_BYTE",
        ),
        (
            ERROR_GRAPHICS_DDCCI_CURRENT_CURRENT_VALUE_GREATER_THAN_MAXIMUM_VALUE,
            "DDCCI_CURRENT_VALUE_GREATER_THAN_MAXIMUM_VALUE",
        ),
        (ERROR_GRAPHICS_I2C_ERROR_RECEIVING_DATA, "I2C_ERROR_RECEIVING_DATA"),
        (ERROR_GRAPHICS_I2C_ERROR_TRANSMITTING_DATA, "I2C_ERROR_TRANSMITTING_DATA"),
        (ERROR_GRAPHICS_I2C_NOT_SUPPORTED, "I2C_NOT_SUPPORTED"),
    ];
    for (code, name) in known {
        if code.0 as u32 == raw {
            return format!("{name} 0x{raw:08X}");
        }
    }
    format!("Win32 error 0x{raw:08X}")
}

fn classify(raw: u32) -> FrameError {
    let is = |code: HRESULT| code.0 as u32 == raw;
    if is(ERROR_GRAPHICS_DDCCI_VCP_NOT_SUPPORTED) {
        FrameError::Unsupported
    } else if is(ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_CHECKSUM)
        || is(ERROR_GRAPHICS_DDCCI_INVALID_DATA)
        || is(ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_COMMAND)
        || is(ERROR_GRAPHICS_DDCCI_INVALID_MESSAGE_LENGTH)
        || is(ERROR_GRAPHICS_DDCCI_CURRENT_CURRENT_VALUE_GREATER_THAN_MAXIMUM_VALUE)
    {
        FrameError::Garbage(raw)
    } else {
        FrameError::Io(raw)
    }
}

fn wide_to_string(wide: &[u16]) -> String {
    let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}

/// One display as seen by Windows.
pub struct DisplayInfo {
    pub hmonitor: HMONITOR,
    /// `\\.\DISPLAYn`
    pub device: String,
    /// left, top, right, bottom
    pub rect: (i32, i32, i32, i32),
    pub primary: bool,
    /// Monitor device interface path (`\\?\DISPLAY#<hwid>#<instance>#{guid}`).
    pub interface_path: Option<String>,
    /// `<hwid>\<instance>` under `HKLM\SYSTEM\CurrentControlSet\Enum\DISPLAY`.
    pub registry_key: Option<String>,
    pub edid: Option<Vec<u8>>,
    pub edid_error: Option<String>,
}

unsafe extern "system" fn collect_monitors(hmon: HMONITOR, _hdc: HDC, _rect: *mut RECT, data: LPARAM) -> BOOL {
    // SAFETY: `data` is the pointer to the Vec passed by `enumerate_displays`.
    let list = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
    list.push(hmon);
    BOOL(1)
}

pub fn enumerate_displays() -> Result<Vec<DisplayInfo>, String> {
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only pushes into `handles`, which outlives the call.
    let ok = unsafe {
        EnumDisplayMonitors(None, None, Some(collect_monitors), LPARAM(&mut handles as *mut _ as isize))
    };
    if !ok.as_bool() {
        return Err("EnumDisplayMonitors failed".into());
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

        let mut display = DisplayInfo {
            hmonitor,
            device: device.clone(),
            rect: (rc.left, rc.top, rc.right, rc.bottom),
            primary: info.monitorInfo.dwFlags & 1 != 0,
            interface_path: None,
            registry_key: None,
            edid: None,
            edid_error: None,
        };

        let device_wide: Vec<u16> = device.encode_utf16().chain(std::iter::once(0)).collect();
        let mut dd = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        // SAFETY: `device_wide` is NUL-terminated, `dd` has cb set.
        let found = unsafe {
            EnumDisplayDevicesW(PCWSTR(device_wide.as_ptr()), 0, &mut dd, EDD_GET_DEVICE_INTERFACE_NAME)
        };
        if found.as_bool() {
            let path = wide_to_string(&dd.DeviceID);
            match registry_key_from_interface(&path) {
                Some(key) => match read_edid(&key) {
                    Ok(data) => display.edid = Some(data),
                    Err(error) => display.edid_error = Some(error),
                },
                None => display.edid_error = Some(format!("cannot parse device interface path '{path}'")),
            }
            display.registry_key = registry_key_from_interface(&path);
            display.interface_path = Some(path);
        } else {
            display.edid_error = Some("EnumDisplayDevicesW found no monitor on this output".into());
        }
        result.push(display);
    }
    Ok(result)
}

/// `\\?\DISPLAY#MSI3ACF#5&1b2c&0&UID0000#{guid}` -> `MSI3ACF\5&1b2c&0&UID0000`.
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

/// Physical monitor handles of one `HMONITOR`; destroyed on drop.
pub struct PhysicalMonitors {
    monitors: Vec<PHYSICAL_MONITOR>,
}

impl PhysicalMonitors {
    pub fn open(hmonitor: HMONITOR) -> Result<Self, String> {
        let mut count = 0u32;
        // SAFETY: plain query with a valid out pointer.
        unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(hmonitor, &mut count) }
            .map_err(|error| format!("GetNumberOfPhysicalMonitorsFromHMONITOR: {error}"))?;
        if count == 0 {
            return Err("no physical monitors for this HMONITOR".into());
        }
        let mut monitors = vec![PHYSICAL_MONITOR::default(); count as usize];
        // SAFETY: the slice has exactly `count` elements.
        unsafe { GetPhysicalMonitorsFromHMONITOR(hmonitor, &mut monitors) }
            .map_err(|error| format!("GetPhysicalMonitorsFromHMONITOR: {error}"))?;
        Ok(PhysicalMonitors { monitors })
    }

    pub fn len(&self) -> usize {
        self.monitors.len()
    }

    pub fn transport(&self, index: usize) -> WinDxva2<'_> {
        let monitor = self.monitors[index];
        // Packed struct: copy the fields out before use.
        let handle = monitor.hPhysicalMonitor;
        let description = monitor.szPhysicalMonitorDescription;
        WinDxva2 { handle, description: wide_to_string(&description), _owner: PhantomData }
    }
}

impl Drop for PhysicalMonitors {
    fn drop(&mut self) {
        // SAFETY: the handles came from GetPhysicalMonitorsFromHMONITOR and are destroyed exactly once.
        let _ = unsafe { DestroyPhysicalMonitors(&self.monitors) };
    }
}

/// Dxva2 transport. READ-ONLY: has no write method.
pub struct WinDxva2<'a> {
    handle: HANDLE,
    description: String,
    _owner: PhantomData<&'a PhysicalMonitors>,
}

impl VcpTransport for WinDxva2<'_> {
    fn get_frame(&mut self, code: u8) -> Result<VcpReply, FrameError> {
        let mut vcp_type = MC_VCP_CODE_TYPE::default();
        let mut current = 0u32;
        let mut maximum = 0u32;
        // SAFETY: valid handle owned by `PhysicalMonitors`, valid out pointers.
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
            return Err(FrameError::Garbage(0));
        }
        Ok(VcpReply { maximum: maximum as u16, current: current as u16 })
    }

    fn describe(&self) -> String {
        format!("Dxva2 '{}'", self.description)
    }
}
