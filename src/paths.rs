//! Per-platform directories. On Linux the XDG rules apply (unchanged from before the Windows port);
//! on Windows config and data live under `%APPDATA%`, logs under `%LOCALAPPDATA%\titan_control\logs`.

use std::path::PathBuf;

/// Base directory of configuration files (the app adds `titan_control/`); `None` when it cannot be determined.
#[cfg(unix)]
pub fn config_base() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        Some(PathBuf::from(dir))
    } else {
        dirs::home_dir().map(|home| home.join(".config"))
    }
}

#[cfg(not(unix))]
pub fn config_base() -> Option<PathBuf> {
    dirs::config_dir()
}

/// Base directory of user data (menu entries, icons, assets installed per user).
#[cfg(unix)]
pub fn data_base() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        Some(PathBuf::from(dir))
    } else {
        dirs::home_dir().map(|home| home.join(".local").join("share"))
    }
}

#[cfg(not(unix))]
pub fn data_base() -> Option<PathBuf> {
    dirs::data_dir()
}

/// Directory of the log file.
#[cfg(unix)]
pub fn log_dir() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("state")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("titan_control")
}

#[cfg(not(unix))]
pub fn log_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("titan_control")
        .join("logs")
}
