//! Simulated P275MV for tests (no hardware): register state, `GET 99 -> GET code` vendor access, the profile table
//! stream (`GET 22 -> GET profile -> GET FE ... -> 0000/5101`, docs/p275mv_plus.md), plus faults: busy (NULL) and
//! garbage replies, and lost writes. Two mocks are two monitors.

use super::*;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stream {
    Idle,
    /// `GET 22` answered: the next GET of the profile code starts the table.
    WaitProfile(u8),
    /// Next table fragment to send.
    Chunk(usize),
}

pub struct MockMonitor {
    pub plain: HashMap<u8, u16>,
    pub vendor: HashMap<u8, u16>,
    /// Active profile (odd = Default, even = Custom).
    pub profile: u8,
    /// Profile table bytes (byte 0 = brightness, 1 = contrast, ...): 10 fragments of 4 bytes.
    pub table: [u8; 40],
    armed: bool,
    vendor_write_armed: bool,
    stream: Stream,
    /// Fragment lost to a garbage reply: `GET FF` sends it again.
    last_chunk: Option<VcpReply>,
    /// Accepted writes, in order.
    pub writes: Vec<(u8, u16)>,
    pub write_attempts: u32,
    pub frames: u32,
    /// Every n-th write is silently lost (the real monitor loses about one in six).
    pub drop_every_nth_write: Option<u32>,
    /// Every n-th GET answers `6e 80 be` (busy) instead of a value.
    pub busy_every_nth_get: Option<u32>,
    /// Every n-th GET answers garbage.
    pub garbage_every_nth_get: Option<u32>,
}

impl MockMonitor {
    pub fn new(profile: u8) -> Self {
        let mut plain = HashMap::new();
        plain.insert(VCP_BRIGHTNESS, 70);
        plain.insert(VCP_CONTRAST, 50);
        plain.insert(TEST_REG, 70);
        let mut vendor = HashMap::new();
        vendor.insert(VCP_VOLUME, 20);
        let mut table = [0u8; 40];
        table[0] = 90; // brightness
        table[1] = 50; // contrast
        Self {
            plain,
            vendor,
            profile,
            table,
            armed: false,
            vendor_write_armed: false,
            stream: Stream::Idle,
            last_chunk: None,
            writes: Vec::new(),
            write_attempts: 0,
            frames: 0,
            drop_every_nth_write: None,
            busy_every_nth_get: None,
            garbage_every_nth_get: None,
        }
    }

    fn chunk(&self, index: usize) -> VcpReply {
        let b = &self.table[index * 4..index * 4 + 4];
        VcpReply { maximum: u16::from_be_bytes([b[0], b[1]]), current: u16::from_be_bytes([b[2], b[3]]) }
    }

    pub fn get(&mut self, code: u8) -> Result<VcpReply, FrameError> {
        self.frames += 1;
        if self.garbage_every_nth_get.is_some_and(|n| self.frames.is_multiple_of(n)) {
            // The fragment is consumed but its reply is lost: `GET FF` repeats it.
            self.armed = false;
            match self.stream {
                Stream::WaitProfile(profile) if code == profile => {
                    self.last_chunk = Some(self.chunk(0));
                    self.stream = Stream::Chunk(1);
                }
                Stream::Chunk(index) if code == VCP_FIRMWARE => {
                    self.last_chunk = Some(if index < 10 { self.chunk(index) } else { VcpReply { maximum: 0, current: 0x5101 } });
                    self.stream = if index < 10 { Stream::Chunk(index + 1) } else { Stream::Idle };
                }
                _ => self.stream = Stream::Idle,
            }
            return Err(FrameError::Garbage);
        }
        if self.busy_every_nth_get.is_some_and(|n| self.frames.is_multiple_of(n)) {
            return Err(FrameError::Null);
        }
        if code == 0xFF {
            return Ok(self.last_chunk.take().unwrap_or(VcpReply { maximum: 0, current: 0x0200 }));
        }
        if code == VCP_DDCCI_INIT {
            self.armed = true;
            self.stream = Stream::Idle;
            return Ok(VcpReply { maximum: 0xFAFA, current: 0xFAFA });
        }
        if self.armed {
            self.armed = false;
            if code == VCP_MODE {
                self.stream = Stream::WaitProfile(self.profile);
                return Ok(VcpReply { maximum: 0x0026, current: self.profile as u16 });
            }
            self.stream = Stream::Idle;
            return self
                .vendor
                .get(&code)
                .map(|&value| VcpReply { maximum: 0x00FF, current: value })
                .ok_or(FrameError::Unsupported);
        }
        match self.stream {
            Stream::WaitProfile(profile) if code == profile && profile % 2 == 0 => {
                self.stream = Stream::Chunk(1);
                return Ok(self.chunk(0));
            }
            Stream::Chunk(index) if code == VCP_FIRMWARE => {
                if index < 10 {
                    self.stream = Stream::Chunk(index + 1);
                    return Ok(self.chunk(index));
                }
                self.stream = Stream::Idle;
                return Ok(VcpReply { maximum: 0, current: 0x5101 });
            }
            _ => self.stream = Stream::Idle,
        }
        match code {
            VCP_VERSION => Ok(VcpReply { maximum: 0x00FF, current: 0x0201 }),
            VCP_FIRMWARE => Ok(VcpReply { maximum: 0, current: 0x5101 }),
            _ => self
                .plain
                .get(&code)
                .map(|&value| VcpReply { maximum: 0x00FF, current: value })
                .ok_or(FrameError::Unsupported),
        }
    }

    pub fn write(&mut self, code: u8, value: u16) {
        self.write_attempts += 1;
        if self.drop_every_nth_write.is_some_and(|n| self.write_attempts.is_multiple_of(n)) {
            return;
        }
        if code == VCP_DDCCI_INIT {
            self.vendor_write_armed = true;
            return;
        }
        if self.vendor_write_armed {
            self.vendor.insert(code, value);
            self.vendor_write_armed = false;
        } else {
            self.plain.insert(code, value);
        }
        self.writes.push((code, value));
    }
}

/// Counters that show whether two transports were ever inside an exchange at the same time.
#[derive(Clone, Default)]
pub struct Overlap {
    in_flight: Arc<AtomicUsize>,
    pub max_in_flight: Arc<AtomicUsize>,
}

pub struct MockTransport {
    pub state: Arc<Mutex<MockMonitor>>,
    /// Time an exchange takes (to make overlaps observable).
    pub exchange_time: Duration,
    pub overlap: Overlap,
}

impl MockTransport {
    pub fn new(state: Arc<Mutex<MockMonitor>>) -> Self {
        Self { state, exchange_time: Duration::ZERO, overlap: Overlap::default() }
    }
}

impl VcpTransport for MockTransport {
    fn get_frame(&mut self, code: u8) -> Result<VcpReply, FrameError> {
        let now = self.overlap.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.overlap.max_in_flight.fetch_max(now, Ordering::SeqCst);
        thread::sleep(self.exchange_time);
        let result = self.state.lock().unwrap().get(code);
        self.overlap.in_flight.fetch_sub(1, Ordering::SeqCst);
        result
    }

    fn write_frame(&mut self, code: u8, value: u16) -> io::Result<()> {
        self.state.lock().unwrap().write(code, value);
        Ok(())
    }

    fn describe(&self) -> String {
        "mock".into()
    }
}

/// A plain (MCCS) register used by the write tests: brightness `10` is a "table" code that is written through `99`.
const TEST_REG: u8 = 0x62;

fn mock(profile: u8) -> (Arc<Mutex<MockMonitor>>, MsiDdc) {
    let state = Arc::new(Mutex::new(MockMonitor::new(profile)));
    let mut ddc = MsiDdc::new(Box::new(MockTransport::new(state.clone())));
    ddc.set_write_level(WriteLevel::Normal);
    (state, ddc)
}

fn reply_bytes(code: u8, status: u8, max: u16, current: u16) -> [u8; 11] {
    let mut r = [0x6E, 0x88, 0x02, status, code, 0x00, (max >> 8) as u8, max as u8, (current >> 8) as u8, current as u8, 0];
    r[10] = r[..10].iter().fold(0x50u8, |acc, b| acc ^ b);
    r
}

#[test]
fn frames_have_valid_checksums() {
    let get = transport::encode_get(0xDF);
    assert_eq!(get[..4], [0x51, 0x82, 0x01, 0xDF]);
    assert_eq!(get.iter().fold(0x6Eu8, |acc, b| acc ^ b), 0);
    let set = transport::encode_set(0x10, 0x0132);
    assert_eq!(set[..6], [0x51, 0x84, 0x03, 0x10, 0x01, 0x32]);
    assert_eq!(set.iter().fold(0x6Eu8, |acc, b| acc ^ b), 0);

    let ok = transport::decode_reply(0xDF, &reply_bytes(0xDF, 0, 0x00FF, 0x0201)).unwrap();
    assert_eq!((ok.maximum, ok.current), (0x00FF, 0x0201));
    assert!(matches!(transport::decode_reply(0xDF, &reply_bytes(0xDF, 1, 0, 0)), Err(FrameError::Unsupported)));
    assert!(matches!(transport::decode_reply(0xDE, &reply_bytes(0xDF, 0, 0, 0)), Err(FrameError::Garbage)));
    let mut bad = reply_bytes(0xDF, 0, 1, 2);
    bad[10] ^= 0xFF;
    assert!(matches!(transport::decode_reply(0xDF, &bad), Err(FrameError::Garbage)));
    let busy = [0x6E, 0x80, 0xBE, 0, 0, 0, 0, 0, 0, 0, 0];
    assert!(matches!(transport::decode_reply(0xDF, &busy), Err(FrameError::Null)));
}

#[test]
fn reads_plain_and_vendor_registers() {
    let (_state, mut ddc) = mock(2);
    assert_eq!(ddc.get_vcp(VCP_BRIGHTNESS).unwrap(), 70);
    assert_eq!(ddc.get_vcp(VCP_VOLUME).unwrap(), 20);
}

#[test]
fn reads_the_profile_table_from_the_mock() {
    let (_state, mut ddc) = mock(2);
    match ddc.read_active_profile(TABLE_CONFIRMATIONS).expect("table") {
        ActiveProfile::Custom(table) => {
            assert_eq!(table.scene_id, 2);
            let settings = table.settings();
            assert_eq!(settings[&VCP_BRIGHTNESS], 90);
            assert_eq!(settings[&VCP_CONTRAST], 50);
        }
        ActiveProfile::Default(_) => panic!("profile 2 is a Custom profile"),
    }
}

#[test]
fn table_survives_busy_and_garbage_replies() {
    let (state, mut ddc) = mock(2);
    state.lock().unwrap().busy_every_nth_get = Some(7);
    state.lock().unwrap().garbage_every_nth_get = Some(11);
    match ddc.read_active_profile(TABLE_CONFIRMATIONS) {
        Ok(ActiveProfile::Custom(table)) => assert_eq!(table.settings()[&VCP_BRIGHTNESS], 90),
        other => panic!("table read failed: {other:?}"),
    }
}

#[test]
fn a_write_to_one_monitor_does_not_touch_the_other() {
    let (a, mut ddc_a) = mock(2);
    let (b, _ddc_b) = mock(2);
    ddc_a.send_vcp(TEST_REG, 40).expect("write");
    assert_eq!(a.lock().unwrap().plain[&TEST_REG], 40);
    assert_eq!(b.lock().unwrap().plain[&TEST_REG], 70);
    assert!(b.lock().unwrap().writes.is_empty());
}

#[test]
fn read_only_mode_sends_nothing() {
    let (state, mut ddc) = mock(2);
    ddc.set_read_only(true);
    let error = ddc.send_vcp(TEST_REG, 40).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(state.lock().unwrap().write_attempts, 0);
}

#[test]
fn swapped_handles_block_the_write() {
    let (state, mut ddc) = mock(2);
    // The identity of the handle changed between connecting and the first write (displays renumbered).
    ddc.set_identity_guard(Box::new(|| Err("DISPLAY6 is now another monitor".into())));
    let error = ddc.send_vcp(TEST_REG, 40).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains("identity"));
    assert_eq!(state.lock().unwrap().write_attempts, 0);

    // The same identity: the first write goes through and the check is not repeated.
    let (state, mut ddc) = mock(2);
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    ddc.set_identity_guard(Box::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    ddc.send_vcp(TEST_REG, 41).unwrap();
    ddc.send_vcp(TEST_REG, 42).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.lock().unwrap().plain[&TEST_REG], 42);
    // After a reconnect the check runs again.
    ddc.require_identity_check();
    ddc.send_vcp(TEST_REG, 43).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn dangerous_writes_need_the_advanced_level() {
    let dangerous: &[(u8, u16)] = &[
        (VCP_MODE, 0),
        (0xF0, 1),
        (0xF0, 2),
        (0xD6, 3),
        (0xD6, 4),
        (0xD6, 5),
        (VCP_BUTTON_LOCK, 1),
        (0x6C, 10),
        (0x6E, 10),
        (0x70, 10),
        (VCP_VENDOR_FACTORY_RESET, 0),
        (0x04, 1),
        (0x05, 1),
        (0x06, 1),
        (0x08, 1),
        (0x0A, 1),
    ];
    for &(key, value) in dangerous {
        assert!(guard_write(key, value, WriteLevel::Normal).is_err(), "0x{key:02X} = {value}");
        assert!(guard_write(key, value, WriteLevel::Advanced).is_ok(), "0x{key:02X} = {value} (advanced)");
        let (state, mut ddc) = mock(2);
        assert!(ddc.send_vcp(key, value).is_err(), "0x{key:02X} = {value} must not reach the monitor");
        assert_eq!(state.lock().unwrap().write_attempts, 0);
    }
    // `22 = 0` and `F0` stay blocked at every level (`check_write`).
    for (key, value) in [(VCP_MODE, 0), (0xF0, 1)] {
        assert!(check_write(key, value).is_err());
        let mut ddc = mock(2).1;
        ddc.set_write_level(WriteLevel::Advanced);
        assert!(ddc.send_vcp(key, value).is_err());
    }
    // Ordinary writes pass.
    assert!(guard_write(VCP_BRIGHTNESS, 50, WriteLevel::Normal).is_ok());
    assert!(guard_write(0xD6, 1, WriteLevel::Normal).is_ok());
    assert!(guard_write(VCP_BUTTON_LOCK, 2, WriteLevel::Normal).is_ok());
}

#[test]
fn the_bus_lock_serialises_exchanges_of_two_monitors() {
    let run = |locked: bool| -> usize {
        let overlap = Overlap::default();
        let bus = Arc::new(Mutex::new(()));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let state = Arc::new(Mutex::new(MockMonitor::new(2)));
            let mut transport = MockTransport::new(state);
            transport.exchange_time = Duration::from_millis(15);
            transport.overlap = overlap.clone();
            let mut transport: Box<dyn VcpTransport> =
                if locked { Box::new(transport::BusLocked::new(Box::new(transport), bus.clone())) } else { Box::new(transport) };
            handles.push(thread::spawn(move || {
                for _ in 0..10 {
                    let _ = transport.get_frame(VCP_VERSION);
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        overlap.max_in_flight.load(Ordering::SeqCst)
    };
    assert_eq!(run(true), 1, "with the bus lock exchanges never overlap");
    assert!(run(false) >= 2, "sanity check: without the lock they do overlap");
}

#[test]
fn chaos_lost_writes_end_in_a_consistent_state() {
    let (state, mut ddc) = mock(2);
    state.lock().unwrap().drop_every_nth_write = Some(6);
    for value in 30..42u16 {
        // `set_vcp` writes, reads back and sends the value once more when the monitor reports another one.
        let _ = ddc.set_vcp(TEST_REG, value);
        assert_eq!(state.lock().unwrap().plain[&TEST_REG], value, "lost write not repaired for {value}");
    }
}

#[test]
fn identical_monitor_names_get_separate_cache_entries() {
    let edid = identity::tests::sample_edid(b"0000000000000");
    let a = identity::from_edid(&edid, "LHC907D\\5&1&0&UID0002").unwrap();
    let b = identity::from_edid(&edid, "LHC907D\\5&1&0&UID0001").unwrap();
    assert_eq!(a.name, b.name);

    let mut store = CacheStore::default();
    store.put_settings(&a.cache_key(), &HashMap::from([(VCP_BRIGHTNESS, 10)]), None);
    store.put_settings(&b.cache_key(), &HashMap::from([(VCP_BRIGHTNESS, 99)]), None);
    assert_eq!(store.monitors.len(), 2);
    assert_eq!(store.monitors[&a.cache_key()][&VCP_BRIGHTNESS], 10);
    assert_eq!(store.monitors[&b.cache_key()][&VCP_BRIGHTNESS], 99);
}

#[test]
fn old_cache_format_migrates_only_when_unambiguous() {
    let old = r#"{
        "monitors": {"P275MV PLUS": {"16": 55}},
        "color_profiles": {"P275MV PLUS": {"11": {"1": 2}}},
        "snapshots": {"P275MV PLUS": {"resolution": "2560x1440", "hz": 144, "serial_number": "X", "firm": "v 5.1.1", "usage_mins": 7}},
        "last_monitor": "P275MV PLUS",
        "last_i2c_path": "/dev/i2c-14"
    }"#;
    let edid = identity::tests::sample_edid(b"0000000000000");
    let a = identity::from_edid(&edid, "LHC907D\\A").unwrap();
    let b = identity::from_edid(&edid, "LHC907D\\B").unwrap();

    // One monitor of that name: the old entries move to its new key, nothing is lost.
    let mut store: CacheStore = serde_json::from_str(old).unwrap();
    assert!(store.migrate_legacy_keys(std::slice::from_ref(&a)));
    assert!(!store.monitors.contains_key("P275MV PLUS"));
    assert_eq!(store.monitors[&a.cache_key()][&VCP_BRIGHTNESS], 55);
    assert_eq!(store.color_profiles[&a.cache_key()][&11][&1], 2);
    assert_eq!(store.snapshots[&a.cache_key()].usage_mins, 7);
    assert_eq!(store.last_monitor, Some(a.cache_key()));
    assert_eq!(store.last_i2c_path.as_deref(), Some("/dev/i2c-14"));
    // A second run changes nothing.
    assert!(!store.migrate_legacy_keys(std::slice::from_ref(&a)));

    // Two monitors with that name: nobody knows whose settings these were, so they are left alone.
    let mut store: CacheStore = serde_json::from_str(old).unwrap();
    assert!(!store.migrate_legacy_keys(&[a.clone(), b.clone()]));
    assert!(store.monitors.contains_key("P275MV PLUS"));
    assert!(!store.monitors.contains_key(&a.cache_key()) && !store.monitors.contains_key(&b.cache_key()));
}

#[test]
fn a_disconnected_monitor_is_recognised_without_long_retries() {
    struct Gone;
    impl VcpTransport for Gone {
        fn get_frame(&mut self, _code: u8) -> Result<VcpReply, FrameError> {
            Err(FrameError::Disconnected)
        }
        fn write_frame(&mut self, _code: u8, _value: u16) -> io::Result<()> {
            Err(io::Error::other("gone"))
        }
        fn describe(&self) -> String {
            "gone".into()
        }
    }
    let mut ddc = MsiDdc::new(Box::new(Gone));
    let started = Instant::now();
    assert!(ddc.get_vcp(VCP_VERSION).is_err());
    assert!(ddc.is_gone());
    // Four retries with 324 ms pauses would take over a second.
    assert!(started.elapsed() < Duration::from_millis(900), "took {:?}", started.elapsed());

    // A monitor that answers again is no longer reported as gone.
    let (_state, mut ok) = mock(2);
    assert!(ok.get_vcp(VCP_VERSION).is_ok());
    assert!(!ok.is_gone());
}

fn leveled(level: u8) -> (Arc<Mutex<MockMonitor>>, MsiDdc, Arc<AtomicU8>) {
    let state = Arc::new(Mutex::new(MockMonitor::new(2)));
    let mut ddc = MsiDdc::new(Box::new(MockTransport::new(state.clone())));
    ddc.set_write_level(WriteLevel::Normal);
    let source = Arc::new(AtomicU8::new(level));
    ddc.set_level_source(source.clone());
    (state, ddc, source)
}

#[test]
fn write_levels_open_only_their_own_settings() {
    // Level 0: nothing at all.
    let (state, mut ddc, _) = leveled(0);
    assert!(ddc.is_read_only());
    assert!(ddc.send_vcp(VCP_VOLUME, 40).is_err());
    assert_eq!(state.lock().unwrap().write_attempts, 0);

    // Level 1: brightness, contrast, volume (and the OSD flag) - not the other picture settings, not an unknown code.
    let (state, mut ddc, source) = leveled(1);
    for key in [VCP_BRIGHTNESS, VCP_VOLUME, VCP_VOLUME, VCP_NEW_CONTROL_VALUE] {
        assert!(ddc.check_user_write(key).is_ok(), "level 1 must allow 0x{key:02X}");
    }
    for key in [VCP_SHARPNESS, VCP_MODE, VCP_DCR, VCP_FPS_COUNTER, 0x77] {
        assert!(ddc.check_user_write(key).is_err(), "level 1 must refuse 0x{key:02X}");
        assert!(ddc.send_vcp(key, 1).is_err());
    }
    assert_eq!(state.lock().unwrap().write_attempts, 0, "refused writes never reach the monitor");

    // Level 2 adds the other picture settings, level 3 the rest of the settings the UI offers.
    source.store(2, Ordering::SeqCst);
    assert!(ddc.check_user_write(VCP_SHARPNESS).is_ok() && ddc.check_user_write(VCP_MODE).is_ok());
    assert!(ddc.check_user_write(VCP_FPS_COUNTER).is_err());
    source.store(3, Ordering::SeqCst);
    assert!(ddc.check_user_write(VCP_FPS_COUNTER).is_ok() && ddc.check_user_write(VCP_REAR_LED).is_ok());
    // Never by level: the dangerous list and anything unknown.
    assert!(ddc.send_vcp(VCP_VENDOR_FACTORY_RESET, 0).is_err());
    assert!(ddc.send_vcp(0x04, 1).is_err());
    assert!(ddc.send_vcp(VCP_BUTTON_LOCK, 1).is_err());
    assert!(ddc.send_vcp(0x77, 1).is_err());
}

#[test]
fn every_setting_the_ui_offers_has_a_write_level() {
    for &key in PROFILE_SETTING_KEYS.iter().chain(GLOBAL_SETTING_KEYS) {
        // The only offered settings without a level would be the dangerous ones, which are not in these lists.
        assert!(write_class(key).is_some(), "0x{key:02X} has no write level");
        assert!(!is_dangerous_write(key, 1) || key == VCP_BUTTON_LOCK, "0x{key:02X} is dangerous");
    }
}

#[test]
fn level_zero_is_an_immediate_emergency_stop() {
    let (state, mut ddc, source) = leveled(1);
    ddc.send_vcp(VCP_VOLUME, 41).unwrap();
    assert_eq!(state.lock().unwrap().vendor[&VCP_VOLUME], 41);
    // The very next write after the switch is turned off is refused; nothing queued gets through.
    source.store(0, Ordering::SeqCst);
    for value in 42..50 {
        assert!(ddc.send_vcp(VCP_VOLUME, value).is_err());
    }
    assert_eq!(state.lock().unwrap().writes.len(), 1, "only the first write was accepted");
}

#[test]
fn chaos_outage_of_a_second_never_leaves_a_half_applied_write() {
    // Volume is a manufacturer register: written through 99, read after GET 99 - as in the real monitor.
    let (state, mut ddc, _) = leveled(1);
    {
        let mut monitor = state.lock().unwrap();
        monitor.drop_every_nth_write = Some(6);
    }
    // A write lands during a monitor outage (replies are garbage for a second); the app must notice and repair.
    state.lock().unwrap().garbage_every_nth_get = Some(1);
    let outage = Instant::now();
    let _ = ddc.set_vcp(VCP_VOLUME, 33);
    while outage.elapsed() < Duration::from_secs(1) {
        let _ = ddc.get_vcp(VCP_VERSION);
    }
    state.lock().unwrap().garbage_every_nth_get = None;
    // After the outage a write with read-back ends in the requested state.
    for value in [35u16, 36, 37] {
        let _ = ddc.set_vcp(VCP_VOLUME, value);
        let stored = state.lock().unwrap().vendor.get(&VCP_VOLUME).copied();
        assert_eq!(stored, Some(value), "state after the outage differs from the requested value {value}");
    }
}

#[test]
fn reading_the_profile_of_a_vanished_monitor_ends_at_once() {
    struct Gone;
    impl VcpTransport for Gone {
        fn get_frame(&mut self, _code: u8) -> Result<VcpReply, FrameError> {
            Err(FrameError::Disconnected)
        }
        fn write_frame(&mut self, _code: u8, _value: u16) -> io::Result<()> {
            Err(io::Error::other("gone"))
        }
        fn describe(&self) -> String {
            "gone".into()
        }
    }
    let mut ddc = MsiDdc::new(Box::new(Gone));
    let started = Instant::now();
    assert!(ddc.read_active_profile(TABLE_CONFIRMATIONS).is_err());
    // The table read used to keep trying for a minute, which also delayed the report that the monitor was gone.
    assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
    assert!(ddc.is_gone());
}
