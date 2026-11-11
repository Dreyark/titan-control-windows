//! Per-monitor settings cache (`settings_cache.json`).

use super::*;
use super::identity::MonitorId;

pub(super) fn load_cached_settings(name: &str, ui_tx: &async_channel::Sender<UiCmd>) -> HashMap<u8, u16> {
    let _ = ui_tx.send_blocking(UiCmd::Progress("Loading cached settings...".into()));
    load_cache_for(name)
}

// -----------------------------------------------------------------------------
// Cache helpers
// -----------------------------------------------------------------------------

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct CachedMonitorSnapshot {
    pub(super) resolution: String,
    pub(super) hz: u16,
    pub(super) serial_number: String,
    pub(super) firm: String,
    pub(super) usage_mins: u16,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct CacheStore {
    pub(super) monitors: HashMap<String, HashMap<u8, u16>>,
    pub(super) color_profiles: HashMap<String, HashMap<u16, HashMap<u8, u16>>>,
    pub(super) snapshots: HashMap<String, CachedMonitorSnapshot>,
    pub(super) last_monitor: Option<String>,
    pub(super) last_i2c_path: Option<String>,
}

pub(super) fn cache_file() -> PathBuf {
    match crate::paths::config_base() {
        Some(base) => base.join("titan_control").join("settings_cache.json"),
        None => PathBuf::from("./settings_cache.json"),
    }
}

pub(super) fn load_cache_store() -> CacheStore {
    let path = cache_file();
    if let Ok(data) = fs::read_to_string(&path)
        && let Ok(store) = serde_json::from_str(&data)
    {
        return store;
    }
    CacheStore::default()
}

pub(super) fn load_cache_for(name: &str) -> HashMap<u8, u16> {
    load_cache_store()
        .monitors
        .get(name)
        .cloned()
        .unwrap_or_default()
}

pub fn load_color_temp_profile_cache(name: &str, color_temp: u16) -> HashMap<u8, u16> {
    load_cache_store()
        .color_profiles
        .get(name)
        .and_then(|profiles| profiles.get(&color_temp))
        .cloned()
        .unwrap_or_default()
}

pub(super) fn load_cached_snapshot(name: &str) -> Option<CachedMonitorSnapshot> {
    load_cache_store().snapshots.get(name).cloned()
}

pub fn load_last_monitor_name() -> Option<String> {
    load_cache_store().last_monitor
}

pub(super) fn load_last_i2c_path() -> Option<String> {
    load_cache_store().last_i2c_path
}

/// One writer at a time: with several monitors the sessions run in parallel threads, and every update below is a
/// read-modify-write of the whole file.
static CACHE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Applies `change` to the stored cache and writes it back; `change` returns false when nothing changed.
fn update_store(change: impl FnOnce(&mut CacheStore) -> bool) {
    let _lock = CACHE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = cache_file();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut store = load_cache_store();
    if !change(&mut store) {
        return;
    }
    if let Ok(data) = serde_json::to_string_pretty(&store) {
        let _ = fs::write(&path, data);
    }
}

impl CacheStore {
    fn touch(&mut self, key: &str, i2c_path: Option<&str>) {
        self.last_monitor = Some(key.to_string());
        if let Some(i2c_path) = i2c_path {
            self.last_i2c_path = Some(i2c_path.to_string());
        }
    }

    pub(super) fn put_settings(&mut self, key: &str, map: &HashMap<u8, u16>, i2c_path: Option<&str>) {
        self.monitors.insert(key.to_string(), map.clone());
        self.touch(key, i2c_path);
    }

    pub(super) fn put_color_profile(
        &mut self,
        key: &str,
        color_temp: u16,
        map: &HashMap<u8, u16>,
        i2c_path: Option<&str>,
    ) {
        self.color_profiles.entry(key.to_string()).or_default().insert(color_temp, map.clone());
        self.touch(key, i2c_path);
    }

    pub(super) fn put_snapshot(&mut self, key: &str, snapshot: CachedMonitorSnapshot, i2c_path: Option<&str>) {
        self.snapshots.insert(key.to_string(), snapshot);
        self.touch(key, i2c_path);
    }

    /// Moves entries stored under a bare monitor name (the old format) to the key of the monitor they belong to.
    /// Only done when the name identifies exactly one of the `known` monitors: with two identical monitors nobody
    /// can tell whose settings these were, so they stay where they are and both monitors start with a fresh cache.
    /// Returns true when something moved.
    pub(super) fn migrate_legacy_keys(&mut self, known: &[MonitorId]) -> bool {
        let mut changed = false;
        for id in known.iter().filter(|id| !id.is_legacy()) {
            let same_name = known.iter().filter(|other| other.name == id.name).count();
            if same_name != 1 {
                continue;
            }
            let key = id.cache_key();
            if let Some(value) = self.monitors.remove(&id.name) {
                self.monitors.entry(key.clone()).or_insert(value);
                changed = true;
            }
            if let Some(value) = self.color_profiles.remove(&id.name) {
                self.color_profiles.entry(key.clone()).or_insert(value);
                changed = true;
            }
            if let Some(value) = self.snapshots.remove(&id.name) {
                self.snapshots.entry(key.clone()).or_insert(value);
                changed = true;
            }
            if self.last_monitor.as_deref() == Some(id.name.as_str()) {
                self.last_monitor = Some(key);
                changed = true;
            }
        }
        changed
    }
}

/// Migrates the cache file to `MonitorId` keys (see [`CacheStore::migrate_legacy_keys`]).
pub(super) fn migrate_legacy_cache(known: &[MonitorId]) {
    update_store(|store| store.migrate_legacy_keys(known));
}

pub(super) fn save_cache_for(name: &str, map: &HashMap<u8, u16>, i2c_path: Option<&str>) {
    update_store(|store| {
        store.put_settings(name, map, i2c_path);
        true
    });
}

pub(super) fn save_color_temp_profile_cache(
    name: &str,
    color_temp: u16,
    map: &HashMap<u8, u16>,
    i2c_path: Option<&str>,
) {
    update_store(|store| {
        store.put_color_profile(name, color_temp, map, i2c_path);
        true
    });
}

pub(super) fn clear_color_temp_profile_cache(name: &str) {
    update_store(|store| {
        store.color_profiles.remove(name);
        store.last_monitor = Some(name.to_string());
        true
    });
}

pub(super) fn save_cached_snapshot(name: &str, snapshot: CachedMonitorSnapshot, i2c_path: Option<&str>) {
    update_store(|store| {
        store.put_snapshot(name, snapshot, i2c_path);
        true
    });
}

pub(super) fn update_cached_snapshot_usage_time(name: &str, usage_mins: u16) {
    update_store(|store| match store.snapshots.get_mut(name) {
        Some(snapshot) => {
            snapshot.usage_mins = usage_mins;
            true
        }
        None => false,
    });
}

pub(super) fn apply_color_temp_cache(name: &str, settings: &mut HashMap<u8, u16>) {
    let Some(&color_temp) = settings.get(&VCP_COLOR_TEMP) else {
        return;
    };

    if let Some((red, green, blue)) = builtin_color_temp_rgb(color_temp) {
        settings.insert(VCP_RED, red);
        settings.insert(VCP_GREEN, green);
        settings.insert(VCP_BLUE, blue);
        return;
    }

    if is_user_color_temp(color_temp) {
        for (code, value) in load_color_temp_profile_cache(name, color_temp) {
            settings.insert(code, value);
        }
    }
}

pub(super) fn persist_active_color_temp_profile(
    name: &str,
    settings: &HashMap<u8, u16>,
    i2c_path: Option<&str>,
) {
    let Some(&color_temp) = settings.get(&VCP_COLOR_TEMP) else {
        return;
    };

    if !is_user_color_temp(color_temp) {
        return;
    }

    let mut profile = load_color_temp_profile_cache(name, color_temp);
    for code in [VCP_RED, VCP_GREEN, VCP_BLUE] {
        if let Some(&value) = settings.get(&code) {
            profile.insert(code, value);
        }
    }
    save_color_temp_profile_cache(name, color_temp, &profile, i2c_path);
}
