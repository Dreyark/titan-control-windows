//! Protocol layer ported from `src/monitor.rs` (`MsiDdc`): pace, settle, vendor-register reads, profile table.
//! Only the transport differs (Dxva2 instead of raw I2C). READ-ONLY: no write path exists here.
//!
//! Timing constants are copied unchanged. `REPLY_DELAY` (40 ms between request and reply) cannot be applied:
//! Dxva2 performs the whole exchange inside one call (UNVERIFIED-ON-HW what delay it uses).

use crate::transport::{FrameError, VcpReply, VcpTransport};
use std::collections::BTreeMap;
use std::thread;
use std::time::{Duration, Instant};

pub const VCP_VERSION: u8 = 0xDF;
pub const VCP_FIRMWARE: u8 = 0xFE;
pub const VCP_DDCCI_INIT: u8 = 0x99;
pub const VCP_MODE: u8 = 0x22;
const VCP_RESEND: u8 = 0xFF;
pub const PROFILE_TABLE_LEN: usize = 38;
const PROFILE_TABLE_CHUNKS: usize = 10;

/// Minimum start-to-start interval of requests.
const REQUEST_GAP: Duration = Duration::from_millis(100);
/// Pause after a failed reply (as the manufacturer's program).
const RETRY_WAIT: Duration = Duration::from_millis(324);
const UNSUPPORTED_WAIT: Duration = Duration::from_millis(200);
pub const READ_TRIES: usize = 4;
const TABLE_RESEND_WAIT: Duration = Duration::from_millis(150);
const TABLE_RESEND_TRIES: usize = 12;
const TRACE_LIMIT: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Manufacturer register: `GET 99` -> `GET code`.
    Vendor,
    /// Standard MCCS register: plain GET.
    Plain,
}

pub enum TableRead {
    Table(u8, [u8; PROFILE_TABLE_CHUNKS * 4]),
    Default(u8),
}

pub struct TraceEntry {
    pub t_ms: u128,
    pub code: u8,
    pub result: String,
    pub dur_ms: u128,
}

pub struct Ddc<T: VcpTransport> {
    transport: T,
    start: Instant,
    last_request: Option<Instant>,
    needs_settle: bool,
    pub frames: u32,
    pub errors: BTreeMap<String, u32>,
    pub resends: u32,
    pub trace: Vec<TraceEntry>,
}

impl<T: VcpTransport> Ddc<T> {
    pub fn new(transport: T) -> Self {
        Ddc {
            transport,
            start: Instant::now(),
            last_request: None,
            needs_settle: true,
            frames: 0,
            errors: BTreeMap::new(),
            resends: 0,
            trace: Vec::new(),
        }
    }

    pub fn describe(&self) -> String {
        self.transport.describe()
    }

    /// Waits until `REQUEST_GAP` has passed since the previous request.
    fn pace(&mut self) {
        if let Some(last) = self.last_request
            && let Some(wait) = REQUEST_GAP.checked_sub(last.elapsed())
        {
            thread::sleep(wait);
        }
        self.last_request = Some(Instant::now());
    }

    /// One exchange with pacing, statistics and trace.
    pub fn get_frame(&mut self, code: u8) -> Result<VcpReply, FrameError> {
        self.pace();
        let began = Instant::now();
        let result = self.transport.get_frame(code);
        let dur_ms = began.elapsed().as_millis();
        self.frames += 1;
        let text = match &result {
            Ok(reply) => format!("{:04X}/{:04X}", reply.maximum, reply.current),
            Err(error) => {
                *self.errors.entry(error.describe()).or_insert(0) += 1;
                format!("ERR {}", error.describe())
            }
        };
        crate::log::file(&format!("GET {code:02X} -> {text} ({dur_ms} ms)"));
        if self.trace.len() < TRACE_LIMIT {
            self.trace.push(TraceEntry { t_ms: self.start.elapsed().as_millis(), code, result: text, dur_ms });
        }
        result
    }

    /// Plain GET 0xDF until two consecutive valid replies. Leaves a wedged stream or an outage.
    pub fn settle(&mut self) -> bool {
        let mut good = 0;
        for _ in 0..30 {
            match self.get_frame(VCP_VERSION) {
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

    /// Reads a register with validation and retries (as `MsiDdc::read_register`).
    pub fn read_register(&mut self, code: u8, access: Access, tries: usize) -> Result<VcpReply, FrameError> {
        let mut last = FrameError::Garbage(0);
        for _ in 0..tries {
            if self.needs_settle {
                self.settle();
            }
            match self.read_register_once(code, access) {
                Ok(reply) => return Ok(reply),
                // "Unsupported" is sometimes transient; it does not disturb the bus.
                Err(FrameError::Unsupported) => {
                    last = FrameError::Unsupported;
                    thread::sleep(UNSUPPORTED_WAIT);
                }
                Err(error) => {
                    last = error;
                    self.needs_settle = true;
                    thread::sleep(RETRY_WAIT);
                }
            }
        }
        Err(last)
    }

    fn read_register_once(&mut self, code: u8, access: Access) -> Result<VcpReply, FrameError> {
        if access != Access::Vendor {
            return self.get_frame(code);
        }
        let init = self.get_frame(VCP_DDCCI_INIT)?;
        if init.maximum != 0xFAFA || init.current != 0xFAFA {
            return Err(FrameError::Garbage(0));
        }
        self.get_frame(code)
    }

    /// `GET 99 -> GET 22 -> GET profile -> GET FE ... -> 0000/5101` (as `read_profile_table_once`).
    /// One attempt only; the caller decides about repetition.
    pub fn read_profile_table_once(&mut self) -> Result<TableRead, String> {
        let mut entered = false;
        for _ in 0..8 {
            if !self.settle() {
                continue;
            }
            match self.get_frame(VCP_DDCCI_INIT) {
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
            .get_frame(VCP_MODE)
            .map_err(|error| format!("GET 0x22: {}", error.describe()))?;
        if mode.maximum != 0x0026 {
            return Err(format!("GET 0x22: unexpected maximum 0x{:04X}", mode.maximum));
        }

        let scene_id = u8::try_from(mode.current)
            .ok()
            .filter(|id| (1..=30).contains(id))
            .ok_or_else(|| format!("GET 0x22: unknown profile {}", mode.current))?;
        if scene_id % 2 != 0 {
            // Close the sequence: GET 00..1E right after GET 99 -> GET 22 would start a table stream.
            let _ = self.get_frame(VCP_VERSION);
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

    fn next_table_chunk(&mut self, code: u8, received: usize) -> Result<VcpReply, String> {
        let mut request = code;
        let mut error = match self.get_frame(request) {
            Ok(reply) => return Ok(reply),
            Err(error) => error,
        };
        for _ in 0..TABLE_RESEND_TRIES {
            thread::sleep(TABLE_RESEND_WAIT);
            if !matches!(error, FrameError::Null) {
                request = VCP_RESEND;
                self.resends += 1;
            }
            match self.get_frame(request) {
                // FF outside a stream answers 0000/0200: the lost reply was the 0000/5101 terminator.
                Ok(reply) if request == VCP_RESEND && reply.maximum == 0 && reply.current == 0x0200 => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Scripted transport: replies are consumed in order; requests are recorded.
    struct Fake {
        replies: VecDeque<Result<VcpReply, FrameError>>,
        requests: Vec<u8>,
    }

    impl VcpTransport for Fake {
        fn get_frame(&mut self, code: u8) -> Result<VcpReply, FrameError> {
            self.requests.push(code);
            self.replies.pop_front().expect("unexpected extra request")
        }
        fn describe(&self) -> String {
            "fake".into()
        }
    }

    fn ok(maximum: u16, current: u16) -> Result<VcpReply, FrameError> {
        Ok(VcpReply { maximum, current })
    }

    #[test]
    fn reads_profile_table_sequence() {
        let mut replies: VecDeque<_> = VecDeque::new();
        replies.push_back(ok(0, 0x0100)); // settle DF
        replies.push_back(ok(0, 0x0100)); // settle DF
        replies.push_back(ok(0xFAFA, 0xFAFA)); // GET 99
        replies.push_back(ok(0x0026, 0x0002)); // GET 22 -> profile 2 (Custom)
        for i in 0..10u16 {
            replies.push_back(ok(i, 0x0101 + i)); // 10 fragments
        }
        replies.push_back(ok(0, 0x5101)); // terminator
        let mut ddc = Ddc::new(Fake { replies, requests: vec![] });
        match ddc.read_profile_table_once().expect("table") {
            TableRead::Table(scene, data) => {
                assert_eq!(scene, 2);
                assert_eq!(&data[0..4], &[0, 0, 0x01, 0x01]);
                assert_eq!(&data[36..40], &[0, 9, 0x01, 0x0A]);
            }
            TableRead::Default(_) => panic!("expected a table"),
        }
        let sent = &ddc.transport.requests;
        // settle x2, 99, 22, profile code, then FE for the remaining 9 fragments and the terminator.
        let mut expected = vec![0xDF, 0xDF, 0x99, 0x22, 0x02];
        expected.extend(std::iter::repeat_n(0xFE, 10));
        assert_eq!(sent, &expected);
    }

    #[test]
    fn default_profile_closes_sequence() {
        let mut replies: VecDeque<_> = VecDeque::new();
        replies.push_back(ok(0, 0x0100));
        replies.push_back(ok(0, 0x0100));
        replies.push_back(ok(0xFAFA, 0xFAFA));
        replies.push_back(ok(0x0026, 0x0003)); // odd = Default
        replies.push_back(ok(0, 0x0100)); // closing GET DF
        let mut ddc = Ddc::new(Fake { replies, requests: vec![] });
        assert!(matches!(ddc.read_profile_table_once(), Ok(TableRead::Default(3))));
        assert_eq!(ddc.transport.requests, vec![0xDF, 0xDF, 0x99, 0x22, 0xDF]);
    }

    #[test]
    fn plain_read_retries_after_garbage() {
        let mut replies: VecDeque<_> = VecDeque::new();
        replies.push_back(ok(0, 0x0100)); // initial settle
        replies.push_back(ok(0, 0x0100));
        replies.push_back(Err(FrameError::Garbage(1))); // first read fails
        replies.push_back(ok(0, 0x0100)); // settle
        replies.push_back(ok(0, 0x0100));
        replies.push_back(ok(0, 0x5101)); // second read
        let mut ddc = Ddc::new(Fake { replies, requests: vec![] });
        let reply = ddc.read_register(VCP_FIRMWARE, Access::Plain, READ_TRIES).unwrap();
        assert_eq!(reply.current, 0x5101);
        assert_eq!(ddc.errors.len(), 1);
    }
}
