//! Platform-specific parts of the app. Linux code lives in `linux`, Windows code in `windows`
//! (Windows DDC/CI, tray, autostart, display settings are added in later phases of the port).

#[cfg(unix)]
pub mod linux;
#[cfg(windows)]
pub mod windows;
