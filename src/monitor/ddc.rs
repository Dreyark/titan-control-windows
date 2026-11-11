//! DDC/CI protocol layer: registers, routing, pacing, retries, settle and profile table reads (`MsiDdc`).
//! Split out of `monitor.rs` without changes of the logic.

use super::*;

#[derive(Clone, Debug)]
pub struct ProfileTable {
    pub scene_id: u8,
    pub bytes: [u8; PROFILE_TABLE_LEN],
}

impl ProfileTable {
    pub fn settings(&self) -> HashMap<u8, u16> {
        let mut settings = HashMap::new();

        settings.insert(VCP_BRIGHTNESS, self.bytes[2] as u16);
        settings.insert(VCP_CONTRAST, self.bytes[3] as u16);
        settings.insert(VCP_SHARPNESS, self.bytes[4] as u16);
        settings.insert(VCP_COLOR_ENHANCE, self.bytes[5] as u16);
        settings.insert(VCP_CR_ENHANCE, self.bytes[6] as u16);
        settings.insert(VCP_SHADOW_BALANCE, self.bytes[7] as u16);
        settings.insert(VCP_COLOR_TEMP, self.bytes[8] as u16);
        settings.insert(VCP_USER1_RED, self.bytes[9] as u16);
        settings.insert(VCP_USER1_GREEN, self.bytes[10] as u16);
        settings.insert(VCP_USER1_BLUE, self.bytes[11] as u16);
        settings.insert(VCP_USER2_RED, self.bytes[12] as u16);
        settings.insert(VCP_USER2_GREEN, self.bytes[13] as u16);
        settings.insert(VCP_USER2_BLUE, self.bytes[14] as u16);
        settings.insert(VCP_USER3_RED, self.bytes[15] as u16);
        settings.insert(VCP_USER3_GREEN, self.bytes[16] as u16);
        settings.insert(VCP_USER3_BLUE, self.bytes[17] as u16);
        settings.insert(VCP_HUE_RED, self.bytes[18] as u16);
        settings.insert(VCP_HUE_GREEN, self.bytes[19] as u16);
        settings.insert(VCP_HUE_BLUE, self.bytes[20] as u16);
        settings.insert(VCP_HUE_YELLOW, self.bytes[21] as u16);
        settings.insert(VCP_HUE_CYAN, self.bytes[22] as u16);
        settings.insert(VCP_HUE_MAGENTA, self.bytes[23] as u16);
        settings.insert(VCP_SATURATION_RED, self.bytes[24] as u16);
        settings.insert(VCP_SATURATION_GREEN, self.bytes[25] as u16);
        settings.insert(VCP_SATURATION_BLUE, self.bytes[26] as u16);
        settings.insert(VCP_SATURATION_YELLOW, self.bytes[27] as u16);
        settings.insert(VCP_SATURATION_CYAN, self.bytes[28] as u16);
        settings.insert(VCP_SATURATION_MAGENTA, self.bytes[29] as u16);
        // The table keeps Low Blue Light as 0-100 in steps of 25; the setting (0xD8) is the level 0-4.
        settings.insert(VCP_LOW_BLUE, (self.bytes[30] as u16 + 12) / 25);
        settings.insert(VCP_HDR, self.bytes[31] as u16);
        settings.insert(VCP_GAMMA, self.bytes[33] as u16);
        settings.insert(VCP_SUPER_RES, self.bytes[34] as u16);
        settings.insert(VCP_NIGHT_VISION, self.bytes[35] as u16);
        settings.insert(VCP_DYNAMIC_OD, self.bytes[36] as u16);

        settings
    }
}

pub struct MsiDdc {
    transport: Box<dyn VcpTransport>,
    /// Refuses every write before anything is sent (a hard read-only switch, independent of the write level).
    read_only: bool,
    /// Windows: the user-chosen write level (shared with the UI, read at every write; 0 = read-only). `None` = every write
    /// the UI offers is allowed (Linux, as always).
    level_source: Option<Arc<AtomicU8>>,
    /// Highest write class this connection may send (see [`guard_write`]).
    write_level: WriteLevel,
    /// Re-reads the identity of the monitor behind the transport; `Err` = it is not the expected one.
    identity_guard: Option<IdentityGuard>,
    /// The guard runs before the first write after (re)connecting.
    identity_verified: bool,
    gone: bool,
    last_request: Option<Instant>,
    /// Set after a failed exchange: the next operation first waits for the monitor to answer again.
    needs_settle: bool,
}

/// Active picture profile as read from the monitor.
#[derive(Clone, Debug)]
pub enum ActiveProfile {
    /// Custom profile with its settings table.
    Custom(ProfileTable),
    /// Default profile (odd code): its settings are read one by one.
    Default(u8),
}

pub(super) enum TableRead {
    Table(u8, [u8; PROFILE_TABLE_CHUNKS * 4]),
    Default(u8),
}

/// Custom profiles have even codes (Standard Custom = 2 ... DyDs/LD Custom = 0x1E), Default profiles odd ones.
pub fn is_custom_profile(scene_id: u8) -> bool {
    scene_id.is_multiple_of(2)
}

/// Register a setting lives in (docs/p275mv_plus.md, "Dostęp do kodów").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Access {
    /// Manufacturer register: read `GET 99` -> `GET code`, write `SET 99 = v` -> `SET code = v`.
    Vendor,
    /// Codes 00-1E: read plain (after `GET 99` they select a profile table stream), written with `99` like VMW.
    Table,
    /// Standard MCCS register: plain read and plain write.
    Plain,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Route {
    pub(super) code: u8,
    pub(super) access: Access,
    /// Read the value back after a write (actions such as resets and power modes cannot be verified).
    pub(super) verify: bool,
}

impl Route {
    /// Value of a reply. Manufacturer registers such as 0x51/0x52/0x57 answer `ff05/69xx`: the low byte is the value.
    pub(super) fn value(&self, reply: VcpReply) -> u16 {
        if self.access == Access::Vendor
            && reply.maximum >> 8 == 0xFF
            && reply.maximum != 0xFFFF
            && reply.current > 0xFF
        {
            reply.current & 0xFF
        } else {
            reply.current
        }
    }
}

/// Codes whose working register is the plain MCCS one, although some of them also answer after `GET 99`.
pub(super) const PLAIN_CODES: &[u8] = &[
    0x02, // New control value
    0x39, // LED: only a plain SET changes it
    VCP_INPUT_SOURCE,
    0x62, // Volume (MCCS mirror of 0xF6)
    0x68, // OSD language (0 = English)
    0x69, // input signal active lines
    0x6C, 0x6E, 0x70, // video black level R/G/B
    0x8D, // Audio mute (MCCS mirror of 0xF7, inverted)
    0xAC, 0xAE, // signal frequencies
    0xB0, // save/restore settings
    0xC8, // controller type
    VCP_BUTTON_LOCK,
    VCP_OSD_LANG,
    VCP_DPMS,
    VCP_DISPLAY_APPLICATION,
    VCP_VERSION,
    VCP_ADAPTIVE_SYNC,
    VCP_USAGE_TIME,
    VCP_FIRMWARE,
];

/// MCCS restore commands: written plain with value 1 (0x04 factory, 0x05 brightness/contrast, 0x06 geometry,
/// 0x08 colors, 0x0A TV).
pub(super) const RESET_CODES: [u8; 5] = [0x04, 0x05, 0x06, 0x08, 0x0A];

pub(super) fn route(key: u8) -> Route {
    if key == VCP_OUTPUT_RANGE {
        return Route { code: VCP_INPUT_SOURCE, access: Access::Vendor, verify: true };
    }
    let access = if RESET_CODES.contains(&key) || PLAIN_CODES.contains(&key) {
        Access::Plain
    } else if key <= 0x1E {
        Access::Table
    } else {
        Access::Vendor
    };
    let verify = !RESET_CODES.contains(&key)
        && ![
            VCP_NEW_CONTROL_VALUE,
            0xB0,
            VCP_VENDOR_FACTORY_RESET,
            VCP_DPMS,
            VCP_PIP_SWAP,
            VCP_PIP_RESET,
        ]
        .contains(&key);
    Route { code: key, access, verify }
}

/// Result of reading a written setting back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readback {
    Confirmed,
    /// The monitor reports another value (write lost, rejected or adjusted).
    Differs(u16),
    Unreadable,
}

/// Whether a write can be checked by reading the setting back (actions such as resets cannot).
pub fn is_verifiable(key: u8) -> bool {
    route(key).verify
}

/// Writes that are never sent: `0x22 = 0` and `0xF0` (values 1 and 2 turn the monitor off).
/// Lowest write level that allows a write of `key` : 1 = brightness, contrast, volume (and the OSD change
/// flag housekeeping); 2 = the other picture settings; 3 = game aids, PIP, LED, OSD, inputs, audio, system settings.
/// `None` = not on the list, never written without the advanced level (resets and anything unknown).
pub fn write_class(key: u8) -> Option<u8> {
    if matches!(key, VCP_BRIGHTNESS | VCP_CONTRAST | VCP_VOLUME | VCP_NEW_CONTROL_VALUE) {
        return Some(1);
    }
    if key == VCP_MODE
        || PROFILE_SETTING_KEYS.contains(&key)
        || is_color_temp_rgb_code(key)
        || matches!(key, VCP_DCR | VCP_LOCAL_DIMMING | VCP_DYDS | VCP_HALO_CONTROL | VCP_ADAPTIVE_SYNC | VCP_RATIO | VCP_EYESHIELD_REMINDER)
    {
        return Some(2);
    }
    if GLOBAL_SETTING_KEYS.contains(&key) {
        return Some(3);
    }
    None
}

pub(super) fn check_write(key: u8, value: u16) -> io::Result<()> {
    if (key == VCP_MODE && value == 0) || key == 0xF0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("VCP 0x{key:02X} = {value} is blocked"),
        ));
    }
    Ok(())
}

/// Classes of writes. `Normal` is everything the UI offers except the dangerous list; `Advanced` also allows it
/// (resets and the like), and only after an explicit decision of the user (level 4, not offered in the window).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteLevel {
    Normal,
    Advanced,
}

/// Check of the identity of the monitor behind a connection.
pub type IdentityGuard = Box<dyn Fn() -> Result<(), String> + Send>;

/// Codes and values that can lock the monitor, switch it off or reset it (docs/p275mv_plus.md).
pub(super) fn is_dangerous_write(key: u8, value: u16) -> bool {
    (key == VCP_MODE && value == 0)
        || key == 0xF0
        || (key == 0xD6 && matches!(value, 3..=5))
        || (key == VCP_BUTTON_LOCK && value == 1)
        || matches!(key, 0x6C | 0x6E | 0x70)
        || (key == VCP_VENDOR_FACTORY_RESET && value == 0)
        || RESET_CODES.contains(&key)
}

/// Refuses the dangerous writes unless the caller works at the `Advanced` level. Complements [`check_write`],
/// which stays as it was and blocks `22 = 0` and `F0` at every level.
pub(super) fn guard_write(key: u8, value: u16, level: WriteLevel) -> io::Result<()> {
    if level != WriteLevel::Advanced && is_dangerous_write(key, value) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("VCP 0x{key:02X} = {value} needs the advanced write level"),
        ));
    }
    Ok(())
}


impl MsiDdc {
    /// Protocol layer on top of any transport (raw I2C on Linux).
    pub fn new(transport: Box<dyn VcpTransport>) -> Self {
        Self {
            transport,
            read_only: false,
            level_source: None,
            // Linux has always allowed every write the UI offers; the Windows port starts restricted.
            write_level: WriteLevel::Advanced,
            identity_guard: None,
            identity_verified: false,
            gone: false,
            last_request: None,
            needs_settle: true,
        }
    }

    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// Writes follow the level the user chose (see [`write_class`]); `0` blocks every write.
    pub fn set_level_source(&mut self, level: Arc<AtomicU8>) {
        self.level_source = Some(level);
    }

    fn user_level(&self) -> Option<u8> {
        self.level_source.as_ref().map(|level| level.load(Ordering::SeqCst))
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only || self.user_level() == Some(0)
    }

    /// Whether a write of `key` is allowed at the current level, without sending anything.
    pub fn check_user_write(&self, key: u8) -> io::Result<()> {
        let Some(level) = self.user_level() else { return Ok(()) };
        if level == 0 || self.read_only {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "writes are disabled (read-only mode)"));
        }
        match write_class(key) {
            Some(required) if required <= level => Ok(()),
            Some(required) => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("write of 0x{key:02X} needs write level {required} (now {level})"),
            )),
            None if self.write_level == WriteLevel::Advanced => Ok(()),
            None => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("write of 0x{key:02X} is not on the list of allowed writes"),
            )),
        }
    }

    pub fn set_write_level(&mut self, level: WriteLevel) {
        self.write_level = level;
    }

    /// Installs the identity check that precedes the first write (and the first write after `require_identity_check`).
    pub fn set_identity_guard(&mut self, guard: IdentityGuard) {
        self.identity_guard = Some(guard);
        self.identity_verified = false;
    }

    /// Call after a reconnect: the next write checks the identity again.
    pub fn require_identity_check(&mut self) {
        self.identity_verified = false;
    }

    #[cfg(unix)]
    pub fn open(path: &str) -> io::Result<Self> {
        Ok(Self::new(Box::new(crate::platform::linux::LinuxI2c::open(path)?)))
    }

    /// Waits until `REQUEST_GAP` has passed since the previous request (reads and writes alike).
    pub(super) fn pace(&mut self) {
        if let Some(last) = self.last_request
            && let Some(wait) = REQUEST_GAP.checked_sub(last.elapsed())
        {
            thread::sleep(wait);
        }
        self.last_request = Some(Instant::now());
    }

    /// One SET VCP frame: `51 84 03 code hi lo chk`.
    pub(super) fn write_vcp_frame(&mut self, code: u8, value: u16) -> io::Result<()> {
        if self.read_only {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "writes are disabled (read-only mode)"));
        }
        self.pace();
        self.transport.write_frame(code, value)
    }

    /// Sends a write the way the manufacturer's program does, without checking the result.
    ///
    /// Manufacturer and table registers: `SET 99 = value`, then `SET code = value`; plain MCCS registers:
    /// `SET code = value`. An I/O error (usually a short outage of the monitor) is retried once.
    pub fn send_vcp(&mut self, key: u8, value: u16) -> io::Result<()> {
        if self.read_only {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "writes are disabled (read-only mode)"));
        }
        self.check_user_write(key)?;
        check_write(key, value)?;
        guard_write(key, value, self.write_level)?;
        if !self.identity_verified {
            if let Some(guard) = &self.identity_guard {
                guard().map_err(|reason| {
                    io::Error::new(io::ErrorKind::PermissionDenied, format!("monitor identity check failed: {reason}"))
                })?;
            }
            self.identity_verified = true;
        }
        let route = route(key);
        if let Err(error) = self.send_write(route, value) {
            log!("[DDC] VCP 0x{:02X} write failed: {error}, retrying", route.code);
            self.needs_settle = true;
            thread::sleep(RETRY_WAIT);
            return self.send_write(route, value);
        }
        Ok(())
    }

    /// Reads a written setting back (some time after the write, see `VERIFY_DELAY`).
    pub fn verify_vcp(&mut self, key: u8, value: u16) -> Readback {
        let route = route(key);
        match self.read_register(route, 3) {
            Ok(reply) => {
                let current = route.value(reply);
                if current == value { Readback::Confirmed } else { Readback::Differs(current) }
            }
            Err(error) => {
                log!("[DDC] VCP 0x{:02X} verify read failed: {}", route.code, error.describe());
                Readback::Unreadable
            }
        }
    }

    /// Writes a setting and waits for the read-back; a write whose read-back differs is sent once more.
    /// Returns `Some(actual)` when the monitor reports a different value afterwards (rejected or adjusted),
    /// `None` when the value is confirmed or cannot be read back. Used where the next step depends on the result.
    pub fn set_vcp(&mut self, key: u8, value: u16) -> io::Result<Option<u16>> {
        let mut actual = None;
        for attempt in 0..2 {
            self.send_vcp(key, value)?;
            if !is_verifiable(key) {
                return Ok(None);
            }
            thread::sleep(VERIFY_DELAY);
            match self.verify_vcp(key, value) {
                Readback::Confirmed | Readback::Unreadable => return Ok(None),
                Readback::Differs(current) => actual = Some(current),
            }
            if attempt == 0 {
                log!("[DDC] VCP 0x{key:02X} = {value} not confirmed (read {actual:?}), resending");
            }
        }
        Ok(actual)
    }

    pub(super) fn send_write(&mut self, route: Route, value: u16) -> io::Result<()> {
        if self.needs_settle {
            self.settle_ddc();
        }
        if route.access != Access::Plain {
            self.write_vcp_frame(VCP_DDCCI_INIT, value)?;
        }
        self.write_vcp_frame(route.code, value)
    }

    /// Reads a setting from the register it lives in (see [`route`]), with validation and retries.
    pub fn get_vcp_reply(&mut self, key: u8) -> io::Result<VcpReply> {
        let route = route(key);
        self.read_register(route, READ_TRIES)
            .map_err(|error| io::Error::other(format!("VCP 0x{:02X}: {}", route.code, error.describe())))
    }

    pub fn get_vcp(&mut self, key: u8) -> io::Result<u16> {
        let route = route(key);
        self.get_vcp_reply(key).map(|reply| route.value(reply))
    }

    /// Plain register of 0x52 (MCCS Active control): code of the setting changed last. The register after
    /// `GET 99` is the audio source ([`VCP_AUDIO_SOURCE`]).
    pub(super) fn get_active_control(&mut self) -> io::Result<u8> {
        let route = Route { code: 0x52, access: Access::Plain, verify: false };
        self.read_register(route, READ_TRIES)
            .map(|reply| reply.current as u8)
            .map_err(|error| io::Error::other(format!("VCP 0x52: {}", error.describe())))
    }

    pub(super) fn read_register(&mut self, route: Route, tries: usize) -> Result<VcpReply, FrameError> {
        let mut last = FrameError::Garbage;
        for _ in 0..tries {
            if self.needs_settle {
                self.settle_ddc();
            }
            match self.read_register_once(route) {
                Ok(reply) => return Ok(reply),
                // "Unsupported" is sometimes transient; it does not disturb the bus.
                Err(FrameError::Unsupported) => {
                    last = FrameError::Unsupported;
                    thread::sleep(UNSUPPORTED_WAIT);
                }
                Err(error) => {
                    last = error;
                    self.needs_settle = true;
                    if self.gone {
                        break;
                    }
                    thread::sleep(RETRY_WAIT);
                }
            }
        }
        Err(last)
    }

    pub(super) fn read_register_once(&mut self, route: Route) -> Result<VcpReply, FrameError> {
        if route.access != Access::Vendor {
            return self.get_vcp_frame(route.code);
        }
        let init = self.get_vcp_frame(VCP_DDCCI_INIT)?;
        if init.maximum != 0xFAFA || init.current != 0xFAFA {
            return Err(FrameError::Garbage);
        }
        let reply = self.get_vcp_frame(route.code)?;
        if route.code == VCP_MODE {
            // GET 99 -> GET 22 -> GET 00..1E would read a profile table: close the sequence.
            let _ = self.get_vcp_frame(VCP_VERSION);
        }
        Ok(reply)
    }

    /// One request/reply exchange with strict validation (header, checksum, code, status).
    pub(super) fn get_vcp_frame(&mut self, code: u8) -> Result<VcpReply, FrameError> {
        self.pace();
        let result = self.transport.get_frame(code);
        match &result {
            Err(FrameError::Disconnected) => self.gone = true,
            Ok(_) => self.gone = false,
            Err(_) => {}
        }
        result
    }

    /// The transport reported that the monitor no longer exists (until a reply arrives again).
    pub fn is_gone(&self) -> bool {
        self.gone
    }

    /// Plain GET 0xDF until two consecutive valid replies. Leaves a wedged stream or an outage.
    /// DF (VCP version) is used because codes 00-1E right after GET 99 -> GET 22 start a table stream.
    pub(super) fn settle_ddc(&mut self) -> bool {
        let mut good = 0;
        for _ in 0..30 {
            if self.gone {
                return false;
            }
            match self.get_vcp_frame(VCP_VERSION) {
                Ok(_) => {
                    good += 1;
                    if good == 2 {
                        self.needs_settle = false;
                        return true;
                    }
                }
                Err(_) => good = 0,
            }
        }
        false
    }

    /// Reads the active profile. A Custom profile comes with its table, returned only after
    /// `TABLE_CONFIRMATIONS` identical reads. The profile is the one the monitor reports in the GET 0x22 reply
    /// of the table sequence.
    pub fn read_active_profile(&mut self, confirmations: usize) -> io::Result<ActiveProfile> {
        let deadline = Instant::now() + TABLE_BUDGET;
        let mut previous: Option<(u8, [u8; PROFILE_TABLE_CHUNKS * 4])> = None;
        let mut matches = 1;
        let mut attempts = 0;
        let mut last_error = String::from("no attempt made");

        while Instant::now() < deadline {
            if self.gone {
                return Err(io::Error::other("monitor disconnected"));
            }
            attempts += 1;
            match self.read_profile_table_once() {
                Ok(TableRead::Default(scene_id)) => return Ok(ActiveProfile::Default(scene_id)),
                Ok(TableRead::Table(scene_id, data)) if confirmations <= 1 || previous == Some((scene_id, data)) => {
                    matches += 1;
                    if matches >= confirmations {
                        let mut bytes = [0u8; PROFILE_TABLE_LEN];
                        for (slot, value) in bytes.iter_mut().skip(2).zip(data.iter()) {
                            *slot = *value;
                        }
                        return Ok(ActiveProfile::Custom(ProfileTable { scene_id, bytes }));
                    }
                }
                Ok(TableRead::Table(scene_id, data)) => {
                    previous = Some((scene_id, data));
                    matches = 1;
                }
                Err(error) => {
                    previous = None;
                    matches = 1;
                    last_error = error;
                }
            }
            thread::sleep(Duration::from_millis(500));
        }

        Err(io::Error::other(format!(
            "profile table read failed after {attempts} attempts: {last_error}"
        )))
    }

    /// GET 99 -> GET 22 -> GET profile -> GET FE ... -> 0000/5101, with recovery of lost replies:
    /// NULL leaves the fragment unread (repeat the request), garbage consumes it (FF resends it).
    /// Only Custom profiles (even codes) are read as tables: the manufacturer's program reads only those, and the
    /// stream of a Default profile has a different length (9 fragments for Standard Default) and an unknown layout.
    pub(super) fn read_profile_table_once(&mut self) -> Result<TableRead, String> {
        let mut entered = false;
        for _ in 0..8 {
            if !self.settle_ddc() {
                continue;
            }
            match self.get_vcp_frame(VCP_DDCCI_INIT) {
                Ok(reply) if reply.maximum == 0xFAFA && reply.current == 0xFAFA => {
                    entered = true;
                    break;
                }
                _ => {}
            }
        }
        if !entered {
            return Err("GET 0x99 did not return FAFA/FAFA".into());
        }

        let mode = self
            .get_vcp_frame(VCP_MODE)
            .map_err(|error| format!("GET 0x22: {}", error.describe()))?;
        if mode.maximum != 0x0026 {
            return Err(format!("GET 0x22: unexpected maximum 0x{:04X}", mode.maximum));
        }

        let scene_id = u8::try_from(mode.current)
            .ok()
            .filter(|id| (1..=30).contains(id))
            .ok_or_else(|| format!("GET 0x22: unknown profile {}", mode.current))?;
        if !is_custom_profile(scene_id) {
            // Close the sequence: GET 00..1E right after GET 99 -> GET 22 would start a table stream.
            let _ = self.get_vcp_frame(VCP_VERSION);
            return Ok(TableRead::Default(scene_id));
        }

        let mut chunks: Vec<[u8; 4]> = Vec::with_capacity(PROFILE_TABLE_CHUNKS);
        let mut code = scene_id;
        loop {
            let reply = self.next_table_chunk(code, chunks.len())?;
            let chunk = [
                (reply.maximum >> 8) as u8,
                reply.maximum as u8,
                (reply.current >> 8) as u8,
                reply.current as u8,
            ];
            code = VCP_FIRMWARE;
            if !chunks.is_empty() && chunk == [0, 0, 0x51, 0x01] {
                break;
            }
            if chunks.len() >= PROFILE_TABLE_CHUNKS {
                return Err("table stream did not end".into());
            }
            chunks.push(chunk);
        }
        if chunks.len() != PROFILE_TABLE_CHUNKS {
            return Err(format!("table has {} fragments instead of {}", chunks.len(), PROFILE_TABLE_CHUNKS));
        }

        let mut data = [0u8; PROFILE_TABLE_CHUNKS * 4];
        for (index, chunk) in chunks.iter().enumerate() {
            data[index * 4..index * 4 + 4].copy_from_slice(chunk);
        }
        Ok(TableRead::Table(scene_id, data))
    }

    pub(super) fn next_table_chunk(&mut self, code: u8, received: usize) -> Result<VcpReply, String> {
        let mut request = code;
        let mut error = match self.get_vcp_frame(request) {
            Ok(reply) => return Ok(reply),
            Err(error) => error,
        };
        for _ in 0..TABLE_RESEND_TRIES {
            thread::sleep(TABLE_RESEND_WAIT);
            if !matches!(error, FrameError::Null) {
                request = VCP_RESEND;
            }
            match self.get_vcp_frame(request) {
                // FF outside a stream answers 0000/0200: the lost reply was the 0000/5101 terminator.
                Ok(reply)
                    if request == VCP_RESEND
                        && reply.maximum == 0
                        && reply.current == 0x0200 =>
                {
                    if received == PROFILE_TABLE_CHUNKS {
                        return Ok(VcpReply { maximum: 0, current: 0x5101 });
                    }
                    return Err(format!("stream ended after {received} fragments"));
                }
                Ok(reply) => return Ok(reply),
                Err(next) => error = next,
            }
        }
        Err(format!("fragment {received}: {}", error.describe()))
    }
}

/// Reads one setting (validated, with retries inside [`MsiDdc::get_vcp`]); `None` when it cannot be read.
pub(super) fn read_vcp(ddc: &mut MsiDdc, key: u8) -> Option<u16> {
    match ddc.get_vcp(key) {
        Ok(value) => Some(value),
        Err(error) => {
            log!("[DDC] {error}");
            None
        }
    }
}

/// Waits until the monitor answers plain requests again (start of a session, after an outage).
pub(super) fn wake_ddc(ddc: &mut MsiDdc) -> bool {
    ddc.settle_ddc()
}
