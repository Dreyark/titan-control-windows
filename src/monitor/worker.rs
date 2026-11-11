//! The worker that talks to one monitor: command queue, scheduled reads, writes with read-back.
//! Split out of `monitor.rs` without changes of the logic.

use super::*;

pub(super) fn worker_cmd_code(cmd: &WorkerCmd) -> Option<u8> {
    match cmd {
        WorkerCmd::Set(code, _) => Some(*code),
        WorkerCmd::ReadUsageTime | WorkerCmd::RefreshAll | WorkerCmd::DydsWithAdaptiveSync(_) | WorkerCmd::SelectMonitor(_) => None,
    }
}

pub(super) fn collapse_worker_cmds(mut batch: Vec<WorkerCmd>) -> Vec<WorkerCmd> {
    let mut seen = HashSet::new();
    let mut collapsed = Vec::with_capacity(batch.len());

    while let Some(cmd) = batch.pop() {
        let Some(code) = worker_cmd_code(&cmd) else {
            collapsed.push(cmd);
            continue;
        };
        if seen.insert(code) {
            collapsed.push(cmd);
        }
    }

    collapsed.reverse();
    collapsed
}

/// Settings that are read from the monitor after connecting. Picture settings of a Custom profile come from the
/// profile table; they are read one by one for a Default profile or when the table could not be read.
pub(super) fn startup_read_keys(table_read: bool) -> Vec<u8> {
    if table_read {
        return GLOBAL_SETTING_KEYS.to_vec();
    }
    let mut keys = vec![VCP_MODE];
    keys.extend_from_slice(PROFILE_SETTING_KEYS);
    keys.extend_from_slice(GLOBAL_SETTING_KEYS);
    keys
}

/// Settings of the active picture profile that can be read one by one (Default profiles, failed table reads).
/// The User 1-3 RGB gains are only in the profile table: 0x17, 0x19, 0x1B-0x1D answer "unsupported" and 0x1E
/// reads as max 2 (checked on the monitor 2026-10-01).
pub(super) const PROFILE_SETTING_KEYS: &[u8] = &[
    VCP_BRIGHTNESS, VCP_CONTRAST, VCP_SHARPNESS, VCP_COLOR_ENHANCE, VCP_CR_ENHANCE, VCP_SHADOW_BALANCE,
    VCP_COLOR_TEMP, VCP_HUE_RED, VCP_HUE_GREEN, VCP_HUE_BLUE,
    VCP_HUE_YELLOW, VCP_HUE_CYAN, VCP_HUE_MAGENTA, VCP_SATURATION_RED, VCP_SATURATION_GREEN,
    VCP_SATURATION_BLUE, VCP_SATURATION_YELLOW, VCP_SATURATION_CYAN, VCP_SATURATION_MAGENTA, VCP_LOW_BLUE,
    VCP_HDR, VCP_GAMMA, VCP_SUPER_RES, VCP_NIGHT_VISION, VCP_DYNAMIC_OD,
];

/// Settings outside the profile table, read from the monitor after connecting.
pub(super) const GLOBAL_SETTING_KEYS: &[u8] = &[
    // Picture (global)
    // Halo Control is not read: its register does not return the set value (see UNRELIABLE_READBACK);
    // the slider shows the last value written by the app (cache).
    VCP_DCR, VCP_LOCAL_DIMMING, VCP_DYDS, VCP_ADAPTIVE_SYNC, VCP_RATIO, VCP_EYESHIELD_REMINDER,
    // Game Aid
    VCP_SCREEN_SIZE, VCP_FPS_COUNTER, VCP_FPS_POS, VCP_CROSSHAIR, VCP_CROSSHAIR_SHAPE, VCP_CROSSHAIR_COLOR,
    VCP_STOPWATCH, VCP_STOPWATCH_TIME, VCP_STOPWATCH_POS, VCP_GAME_TIME, VCP_GAME_TIME_VAL, VCP_GAME_TIME_POS,
    VCP_MAGNIFIER, VCP_MAGNIFIER_SIZE, VCP_MAGNIFIER_ZOOM, VCP_MAGNIFIER_POS, VCP_HAWKEYE,
    VCP_HAWKEYE_SIZE, VCP_HAWKEYE_POS, VCP_HAWKEYE_LEVEL,
    // Inputs, PIP/PBP, USB
    VCP_INPUT_SOURCE, VCP_INPUT_SELECT, VCP_OUTPUT_RANGE, VCP_PIP_MODE, VCP_PIP_SOURCE, VCP_PIP_POSITION,
    VCP_PIP_SIZE, VCP_AUDIO_SOURCE, VCP_USB_HUB_SOURCE, VCP_USB_POWER_SLEEP,
    // Audio
    VCP_VOLUME, VCP_MUTE,
    // LED
    VCP_REAR_LED, VCP_POWER_LED, VCP_LED_MODE, VCP_LED_COLOR, VCP_LED_STRENGTH, VCP_LED_FRONT_COLOR,
    VCP_LED_REAR_COLOR,
    // OSD and system
    VCP_OSD_LANG, VCP_OSD_TIME, VCP_OSD_H_POS, VCP_OSD_V_POS, VCP_OSD_TRANS, VCP_OSD_LOCK, VCP_BUTTON_LOCK,
    VCP_QUICK_BOOT, VCP_POWER_SAVING,
];

/// Settings read per worker step, so user commands are not delayed by a long read.
pub(super) const READ_CHUNK: usize = 4;
/// The picture mode is compared with the known one every this many polls.
pub(super) const MODE_POLL_EVERY: u32 = 3;
/// Interval of the OSD change check (`0x02` New control value).
#[cfg(unix)]
pub(super) const OSD_POLL_INTERVAL: Duration = Duration::from_secs(3);
/// Windows: one plain read per second is about 5% of the bus time per monitor (a Dxva2 read takes ~50 ms), and a
/// change made with the monitor buttons shows up quickly. UNVERIFIED-ON-HW for long runs.
#[cfg(not(unix))]
pub(super) const OSD_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Pause after a reset before its results are read back.
pub(super) const RESET_SETTLE: Duration = Duration::from_millis(1500);
/// Read-backs of one write: the first mismatch resends the value, the last one reports what the monitor keeps.
pub(super) const VERIFY_ATTEMPTS: u8 = 3;
/// Pause before the last read-back of a write.
pub(super) const FINAL_VERIFY_DELAY: Duration = Duration::from_millis(1200);
/// Settings written by the app are not re-read after an OSD-change flag for this long.
pub(super) const OWN_WRITE_QUIET: Duration = Duration::from_secs(5);
/// Settings whose register does not reliably show a written value (the OSD does change): a mismatch is only logged.
/// Halo Control: after writing 70 (OSD showed it) `46 [99]` read `ff64/0001` (2026-10-01).
pub(super) const UNRELIABLE_READBACK: &[u8] = &[VCP_HALO_CONTROL];

/// Background work of the worker. User commands always go first; the UI is never blocked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Task {
    /// Read a written setting back; `attempt` counts read-backs of this write.
    Verify { code: u8, value: u16, attempt: u8 },
    /// Read the active profile number and its table.
    RefreshProfile,
    /// Read settings from the monitor (startup, after a reset or an OSD change).
    Read(Vec<u8>),
    /// Tells the UI that the profile settings queued before it have been read.
    ProfileLoaded,
    /// Reads the profile table again until two reads agree and corrects the settings if the first read was wrong.
    ConfirmProfile,
}

pub(super) struct Scheduled {
    task: Task,
    due: Instant,
}

pub(super) struct Worker {
    ddc: MsiDdc,
    cache: HashMap<u8, u16>,
    monitor_name: String,
    ui_tx: async_channel::Sender<UiCmd>,
    tasks: VecDeque<Scheduled>,
    last_poll: Instant,
    /// When each setting was last written by the app (OSD-change reads of these are skipped for a while).
    recent_writes: HashMap<u8, Instant>,
    /// DyDs profiles may be selected with Adaptive-Sync on (firmware 2025-09-20).
    dyds_with_adaptive_sync: bool,
    /// The UI was told that the monitor is gone (see `report_connection`).
    reported_gone: bool,
    /// Counts polls (see `poll_mode`).
    mode_poll: u32,
}

impl Worker {
    pub(super) fn new(
        ddc: MsiDdc,
        monitor_name: String,
        ui_tx: async_channel::Sender<UiCmd>,
        table_read: bool,
    ) -> Self {
        let mut worker = Self {
            cache: load_cache_for(&monitor_name),
            ddc,
            monitor_name,
            ui_tx,
            tasks: VecDeque::new(),
            last_poll: Instant::now(),
            recent_writes: HashMap::new(),
            dyds_with_adaptive_sync: false,
            reported_gone: false,
            mode_poll: 0,
        };
        worker.queue_reads(&startup_read_keys(table_read), Instant::now());
        worker
    }

    pub(super) fn run(&mut self, worker_rx: &mpsc::Receiver<WorkerCmd>) {
        loop {
            // Wait for a command until the next task is due (or the next OSD check when there is no task).
            let now = Instant::now();
            let wake = self
                .tasks
                .iter()
                .map(|scheduled| scheduled.due)
                .min()
                .unwrap_or(self.last_poll + OSD_POLL_INTERVAL);
            let wait = wake.saturating_duration_since(now).max(Duration::from_millis(1));
            match worker_rx.recv_timeout(wait) {
                Ok(first_cmd) => {
                    let mut batch = vec![first_cmd];
                    while let Ok(cmd) = worker_rx.try_recv() {
                        batch.push(cmd);
                    }
                    self.handle_batch(batch);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(task) = self.take_due_task() {
                        self.run_task(task);
                        self.report_connection();
                    } else if self.tasks.is_empty() && self.last_poll.elapsed() >= OSD_POLL_INTERVAL {
                        self.poll_osd_changes();
                        self.last_poll = Instant::now();
                    }
                }
            }
        }
    }

    /// First task in queue order whose time has come.
    pub(super) fn take_due_task(&mut self) -> Option<Task> {
        let now = Instant::now();
        let index = self.tasks.iter().position(|scheduled| scheduled.due <= now)?;
        self.tasks.remove(index).map(|scheduled| scheduled.task)
    }

    pub(super) fn handle_batch(&mut self, batch: Vec<WorkerCmd>) {
        // The UI stays usable: commands are queued and a newer value of a setting replaces the older one.
        for cmd in collapse_worker_cmds(batch) {
            match cmd {
                WorkerCmd::Set(code, value) => {
                    if let Err(error) = self.ddc.check_user_write(code) {
                        log!("[DDC] monitor={} VCP 0x{code:02X} = {value} refused: {error}", self.monitor_name);
                        // The control shows what the monitor really has, not the value that was refused.
                        if let Some(&current) = self.cache.get(&code) {
                            let _ = self.ui_tx.send_blocking(UiCmd::Corrected(HashMap::from([(code, current)])));
                        }
                        let message = if self.ddc.is_read_only() { "Writes are disabled (read-only mode)" } else { "Write not allowed at this level" };
                        let _ = self.ui_tx.send_blocking(UiCmd::Error(message.into()));
                        if code == VCP_MODE {
                            let _ = self.ui_tx.send_blocking(UiCmd::ProfileLoaded);
                        }
                        continue;
                    }
                    self.write(code, value);
                    // A refused or failed profile change reads nothing: unlock the profile controls at once.
                    if code == VCP_MODE && !self.tasks.iter().any(|task| task.task == Task::RefreshProfile) {
                        let _ = self.ui_tx.send_blocking(UiCmd::ProfileLoaded);
                    }
                }
                WorkerCmd::ReadUsageTime => {
                    if let Some(mins) = read_vcp(&mut self.ddc, VCP_USAGE_TIME) {
                        update_cached_snapshot_usage_time(&self.monitor_name, mins);
                        let _ = self.ui_tx.send_blocking(UiCmd::UsageTimeUpdated(mins));
                    }
                }
                WorkerCmd::RefreshAll => {
                    self.refresh_info();
                    let now = Instant::now();
                    self.tasks.retain(|scheduled| !matches!(scheduled.task, Task::Read(_) | Task::RefreshProfile | Task::ConfirmProfile));
                    self.tasks.push_front(Scheduled { task: Task::RefreshProfile, due: now });
                    self.queue_reads(GLOBAL_SETTING_KEYS, now);
                }
                WorkerCmd::DydsWithAdaptiveSync(allowed) => self.dyds_with_adaptive_sync = allowed,
                WorkerCmd::SelectMonitor(_) => {}
            }
        }
        // Our own writes may raise the OSD change flag; check it only after the monitor is quiet again.
        self.last_poll = Instant::now();
    }

    /// Switches Adaptive-Sync off and confirms it (a prerequisite of `feature`); false = the feature write is cancelled.
    pub(super) fn adaptive_sync_off(&mut self, feature: &str) -> bool {
        match self.ddc.set_vcp(VCP_ADAPTIVE_SYNC, 0) {
            Ok(None) => {
                self.recent_writes.insert(VCP_ADAPTIVE_SYNC, Instant::now());
                self.store(HashMap::from([(VCP_ADAPTIVE_SYNC, 0)]));
                true
            }
            Ok(Some(actual)) => {
                log!("[DDC] Adaptive Sync stays {actual}; {feature} change cancelled");
                self.store(HashMap::from([(VCP_ADAPTIVE_SYNC, actual)]));
                false
            }
            Err(error) => {
                log!("[DDC] Cannot disable Adaptive Sync; {feature} change cancelled: {error}");
                false
            }
        }
    }

    /// Reads the refresh rate and usage time again and sends them to the UI.
    pub(super) fn refresh_info(&mut self) {
        let hz = read_vcp(&mut self.ddc, VCP_REFRESH_RATE).map(|v| v / 100);
        let usage_mins = read_vcp(&mut self.ddc, VCP_USAGE_TIME);
        if let Some(mins) = usage_mins {
            update_cached_snapshot_usage_time(&self.monitor_name, mins);
        }
        let _ = self.ui_tx.send_blocking(UiCmd::InfoRefreshed { hz, usage_mins });
    }

    /// Sends a write and schedules its read-back.
    pub(super) fn write(&mut self, code: u8, value: u16) {
        // A newer value replaces the one still waiting for its read-back (and a newer profile its table read).
        self.tasks.retain(|scheduled| match scheduled.task {
            Task::Verify { code: pending, .. } => pending != code,
            Task::RefreshProfile | Task::ConfirmProfile => code != VCP_MODE,
            Task::Read(_) | Task::ProfileLoaded => true,
        });

        // DyDs profiles need Adaptive-Sync off on firmware older than 2025-09-20; this one is confirmed before the
        // profile is written, after queue coalescing, so tray/default/custom paths all use the same order.
        if code == VCP_MODE
            && (0x1b..=0x1e).contains(&value)
            && !self.dyds_with_adaptive_sync
            && !self.adaptive_sync_off("DyDs")
        {
            return;
        }

        if let Err(error) = self.ddc.send_vcp(code, value) {
            log!("[DDC] VCP 0x{code:02X} error: {error}");
            return;
        }
        self.recent_writes.insert(code, Instant::now());
        log!("[DDC] monitor={} VCP 0x{code:02X} = {value}", self.monitor_name);

        if is_reset_action(code) {
            if code == VCP_RESET_FACTORY || code == VCP_VENDOR_FACTORY_RESET {
                self.cache = factory_reset_defaults();
                apply_color_temp_cache(&self.monitor_name, &mut self.cache);
                clear_color_temp_profile_cache(&self.monitor_name);
                save_cache_for(&self.monitor_name, &self.cache, None);
            }
            // Read back what the reset really changed, once the monitor has applied it.
            let due = Instant::now() + RESET_SETTLE;
            self.tasks.push_front(Scheduled { task: Task::RefreshProfile, due });
            self.queue_reads(GLOBAL_SETTING_KEYS, due);
            return;
        }

        // Optimistic: the value counts as set; the read-back corrects it if the monitor disagrees.
        self.apply_written(code, value);
        if code == VCP_MODE {
            // A profile change loads that profile's picture settings (after the profile write is confirmed).
            self.tasks.push_front(Scheduled { task: Task::RefreshProfile, due: Instant::now() + VERIFY_DELAY });
        }
        // Settings the monitor changes by itself after this one (e.g. DCR turns local dimming off).
        let dependents = dependent_reads(code);
        if !dependents.is_empty() {
            self.tasks.push_front(Scheduled {
                task: Task::Read(dependents.to_vec()),
                due: Instant::now() + DEPENDENT_READ_DELAY,
            });
        }
        if is_verifiable(code) {
            self.tasks.push_front(Scheduled {
                task: Task::Verify { code, value, attempt: 1 },
                due: Instant::now() + VERIFY_DELAY,
            });
        }
    }

    /// Updates the cache after a write (also keeps the color-temperature RGB profiles in sync).
    pub(super) fn apply_written(&mut self, code: u8, value: u16) {
        self.cache.insert(code, value);
        if let Some(cache_code) = cache_rgb_key_for_vcp_code(code) {
            self.cache.insert(cache_code, value);
        }
        if code == VCP_COLOR_TEMP {
            apply_color_temp_cache(&self.monitor_name, &mut self.cache);
        }
        if is_color_temp_rgb_code(code) {
            persist_active_color_temp_profile(&self.monitor_name, &self.cache, None);
        }
        save_cache_for(&self.monitor_name, &self.cache, None);
    }

    pub(super) fn run_task(&mut self, task: Task) {
        match task {
            Task::Verify { code, value, attempt } => self.verify(code, value, attempt),
            Task::RefreshProfile => self.refresh_profile(),
            Task::Read(keys) => self.read_keys(keys),
            Task::ProfileLoaded => {
                let _ = self.ui_tx.send_blocking(UiCmd::ProfileLoaded);
            }
            Task::ConfirmProfile => {
                if let Ok(ActiveProfile::Custom(table)) = self.ddc.read_active_profile(TABLE_CONFIRMATIONS) {
                    let mut settings = table.settings();
                    settings.insert(VCP_MODE, table.scene_id as u16);
                    self.store(settings);
                }
            }
        }
    }

    pub(super) fn verify(&mut self, code: u8, value: u16, attempt: u8) {
        match self.ddc.verify_vcp(code, value) {
            Readback::Confirmed => log!("[DDC] VCP 0x{code:02X} = {value} confirmed"),
            Readback::Unreadable if attempt < VERIFY_ATTEMPTS => self.schedule_verify(code, value, attempt + 1),
            Readback::Unreadable => log!("[DDC] VCP 0x{code:02X} = {value} could not be read back"),
            Readback::Differs(actual) if attempt < VERIFY_ATTEMPTS => {
                // A single write is sometimes lost: send it again once, then only read.
                if attempt == 1 {
                    log!("[DDC] VCP 0x{code:02X} = {value} not confirmed (read {actual}), resending");
                    if let Err(error) = self.ddc.send_vcp(code, value) {
                        log!("[DDC] VCP 0x{code:02X} error: {error}");
                    }
                }
                self.schedule_verify(code, value, attempt + 1);
            }
            Readback::Differs(actual) if UNRELIABLE_READBACK.contains(&code) => {
                log!("[DDC] VCP 0x{code:02X} = {value} written, register reads {actual} (read-back ignored)");
            }
            Readback::Differs(actual) => {
                // The monitor rejected or adjusted the value: keep what it reports and show it.
                log!("[DDC] VCP 0x{code:02X} = {value} not accepted, monitor reports {actual}");
                self.apply_written(code, actual);
                let _ = self.ui_tx.send_blocking(UiCmd::Corrected(HashMap::from([(code, actual)])));
            }
        }
    }

    pub(super) fn schedule_verify(&mut self, code: u8, value: u16, attempt: u8) {
        // The last read-back waits longer: some settings take a moment before their register follows.
        let delay = if attempt >= VERIFY_ATTEMPTS { FINAL_VERIFY_DELAY } else { VERIFY_DELAY * 4 };
        self.tasks.push_front(Scheduled { task: Task::Verify { code, value, attempt }, due: Instant::now() + delay });
    }

    /// Reads settings from the monitor and sends the ones that could be read to the UI.
    pub(super) fn read_keys(&mut self, keys: Vec<u8>) {
        let mut settings = HashMap::new();
        for key in keys {
            // Replies are validated (header, checksum, code), so every value read is real.
            if let Some(value) = read_vcp(&mut self.ddc, key) {
                settings.insert(key, value);
            }
        }
        self.store(settings);
    }

    /// Reads the active profile number and its table (picture settings).
    /// Reads the active profile. The first valid table is taken at once so the UI is unlocked quickly; a second,
    /// confirming read follows in the background (`Task::ConfirmProfile`).
    pub(super) fn refresh_profile(&mut self) {
        match self.ddc.read_active_profile(1) {
            Ok(ActiveProfile::Custom(table)) => {
                let mut settings = table.settings();
                settings.insert(VCP_MODE, table.scene_id as u16);
                self.store(settings);
                let _ = self.ui_tx.send_blocking(UiCmd::ProfileLoaded);
                self.tasks.push_back(Scheduled { task: Task::ConfirmProfile, due: Instant::now() });
            }
            Ok(ActiveProfile::Default(scene_id)) => {
                self.store(HashMap::from([(VCP_MODE, scene_id as u16)]));
                self.read_profile_settings(PROFILE_SETTING_KEYS);
            }
            Err(error) => {
                log!("[DDC] Profile table read failed: {error}; reading settings one by one");
                let mut keys = vec![VCP_MODE];
                keys.extend_from_slice(PROFILE_SETTING_KEYS);
                self.read_profile_settings(&keys);
            }
        }
    }

    /// Reads the profile settings one by one, ahead of everything else in the queue, then tells the UI.
    pub(super) fn read_profile_settings(&mut self, keys: &[u8]) {
        self.tasks.retain(|scheduled| !matches!(scheduled.task, Task::ProfileLoaded));
        let mut chunks: VecDeque<Scheduled> = keys
            .chunks(READ_CHUNK)
            .map(|chunk| Scheduled { task: Task::Read(chunk.to_vec()), due: Instant::now() })
            .collect();
        chunks.push_back(Scheduled { task: Task::ProfileLoaded, due: Instant::now() });
        for task in chunks.into_iter().rev() {
            self.tasks.push_front(task);
        }
    }

    /// Queues reads in chunks at the end of the queue, skipping settings already waiting to be read.
    pub(super) fn queue_reads(&mut self, keys: &[u8], due: Instant) {
        let queued: HashSet<u8> = self
            .tasks
            .iter()
            .filter_map(|scheduled| match &scheduled.task {
                Task::Read(keys) => Some(keys.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        let missing: Vec<u8> = keys.iter().copied().filter(|key| !queued.contains(key)).collect();
        for chunk in missing.chunks(READ_CHUNK) {
            self.tasks.push_back(Scheduled { task: Task::Read(chunk.to_vec()), due });
        }
    }

    /// Updates the cache and the UI.
    pub(super) fn store(&mut self, mut settings: HashMap<u8, u16>) {
        if settings.is_empty() {
            return;
        }
        if settings.contains_key(&VCP_COLOR_TEMP) {
            apply_color_temp_cache(&self.monitor_name, &mut settings);
        }
        self.cache.extend(settings.iter().map(|(&code, &value)| (code, value)));
        save_cache_for(&self.monitor_name, &self.cache, None);
        let _ = self.ui_tx.send_blocking(UiCmd::Settings(settings));
    }

    /// Detects settings changed with the monitor buttons: `0x02` = 2 means "changed in the OSD" and the plain
    /// register of `0x52` names the last changed code. Writing `0x02 = 1` clears both. Manufacturer settings
    /// do not always appear in `0x52`; an unknown code triggers a full read.
    /// Tells the UI when the monitor disappears or comes back.
    pub(super) fn report_connection(&mut self) {
        let gone = self.ddc.is_gone();
        if gone != self.reported_gone {
            self.reported_gone = gone;
            log!("[DDC] monitor {}", if gone { "disconnected" } else { "responding again" });
            let _ = self.ui_tx.send_blocking(UiCmd::Responding(!gone));
        }
    }

    pub(super) fn poll_osd_changes(&mut self) {
        // Clearing the OSD change flag is a write; without it the flag would stay set and trigger endless re-reads.
        // Read-only mode only checks that the monitor is still there (a plain read).
        if self.ddc.is_read_only() {
            let _ = self.ddc.get_vcp(VCP_VERSION);
            self.report_connection();
            self.mode_poll = self.mode_poll.wrapping_add(1);
            if cfg!(not(unix)) && self.mode_poll.is_multiple_of(MODE_POLL_EVERY) && !self.ddc.is_gone() {
                self.poll_mode();
            }
            return;
        }
        let flag = self.ddc.get_vcp(VCP_NEW_CONTROL_VALUE);
        self.report_connection();
        let Ok(flag) = flag else {
            return;
        };
        if flag == 2 {
            self.handle_osd_flag();
        }
        // A change of the picture mode in the monitor does not always raise the flag, so the mode is also compared
        // with the known one every few polls (Windows; Linux keeps relying on the flag as before).
        self.mode_poll = self.mode_poll.wrapping_add(1);
        if cfg!(not(unix)) && self.mode_poll.is_multiple_of(MODE_POLL_EVERY) {
            self.poll_mode();
        }
    }

    fn poll_mode(&mut self) {
        if self.tasks.iter().any(|scheduled| scheduled.task == Task::RefreshProfile) {
            return;
        }
        let Ok(mode) = self.ddc.get_vcp(VCP_MODE) else {
            return;
        };
        let own = self.recent_writes.get(&VCP_MODE).is_some_and(|at| at.elapsed() < OWN_WRITE_QUIET);
        if self.cache.get(&VCP_MODE) != Some(&mode) && !own {
            log!("[DDC] monitor={} picture mode changed in the monitor: {mode}", self.monitor_name);
            self.cache.insert(VCP_MODE, mode);
            self.tasks.push_back(Scheduled { task: Task::RefreshProfile, due: Instant::now() });
        }
    }

    fn handle_osd_flag(&mut self) {
        let changed = self.ddc.get_active_control().ok();
        if let Err(error) = self.ddc.send_vcp(VCP_NEW_CONTROL_VALUE, 1) {
            log!("[DDC] Cannot clear the OSD change flag: {error}");
        }
        log!("[DDC] Setting changed in the OSD: {changed:02X?}");
        let now = Instant::now();
        // Our own writes also raise the flag; reading such a setting back could show a lagging register value.
        if let Some(code) = changed
            && self.recent_writes.get(&code).is_some_and(|at| at.elapsed() < OWN_WRITE_QUIET)
        {
            return;
        }
        match changed {
            Some(VCP_MODE) => self.tasks.push_back(Scheduled { task: Task::RefreshProfile, due: now }),
            Some(code) if PROFILE_SETTING_KEYS.contains(&code) || GLOBAL_SETTING_KEYS.contains(&code) => {
                self.queue_reads(&[code], now);
            }
            _ => {
                self.tasks.push_back(Scheduled { task: Task::RefreshProfile, due: now });
                self.queue_reads(GLOBAL_SETTING_KEYS, now);
            }
        }
    }
}

/// Pause before settings that depend on a write are read back.
pub(super) const DEPENDENT_READ_DELAY: Duration = Duration::from_millis(800);

/// Settings the monitor may change by itself after a write of `code` (OSD rules, observed in the old UI logic).
pub(super) fn dependent_reads(code: u8) -> &'static [u8] {
    match code {
        VCP_SCREEN_SIZE => &[VCP_ADAPTIVE_SYNC, VCP_DYDS, VCP_DYNAMIC_OD, VCP_MAGNIFIER, VCP_HAWKEYE],
        VCP_ADAPTIVE_SYNC => &[VCP_DYDS, VCP_MAGNIFIER, VCP_HAWKEYE],
        VCP_DYDS => &[VCP_LOCAL_DIMMING, VCP_DCR, VCP_ADAPTIVE_SYNC],
        VCP_LOCAL_DIMMING => &[VCP_DCR],
        // Firmware 2025-09-20: HDR on sets local dimming to High and locks brightness, contrast and DCR.
        VCP_HDR => &[VCP_LOCAL_DIMMING, VCP_DCR, VCP_BRIGHTNESS, VCP_CONTRAST],
        VCP_DCR => &[VCP_BRIGHTNESS, VCP_CONTRAST, VCP_LOCAL_DIMMING],
        VCP_MAGNIFIER => &[VCP_HAWKEYE],
        VCP_HAWKEYE => &[VCP_MAGNIFIER],
        VCP_LED_MODE => &[VCP_LED_COLOR, VCP_LED_STRENGTH, VCP_LED_FRONT_COLOR, VCP_LED_REAR_COLOR],
        VCP_PIP_MODE => &[
            VCP_PIP_SOURCE, VCP_PIP_POSITION, VCP_PIP_SIZE, VCP_AUDIO_SOURCE, VCP_DCR, VCP_HDR, VCP_ADAPTIVE_SYNC,
            VCP_DYDS, VCP_DYNAMIC_OD, VCP_FPS_COUNTER, VCP_CROSSHAIR, VCP_STOPWATCH,
            VCP_GAME_TIME, VCP_MAGNIFIER, VCP_HAWKEYE,
        ],
        VCP_PIP_SWAP => &[VCP_PIP_SOURCE, VCP_INPUT_SELECT, VCP_AUDIO_SOURCE],
        _ => &[],
    }
}

/// Writes after which the monitor changes several settings at once.
pub(super) fn is_reset_action(code: u8) -> bool {
    RESET_CODES.contains(&code) || code == VCP_VENDOR_FACTORY_RESET || code == VCP_PIP_RESET
}
