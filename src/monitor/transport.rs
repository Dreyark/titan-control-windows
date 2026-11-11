//! Transport between the DDC/CI protocol (`MsiDdc`) and the bus: Linux raw I2C today, Windows Dxva2 and a mock
//! later. The protocol layer keeps pacing, retries, settle and table logic; a transport only moves single frames.

use std::io;

#[derive(Clone, Copy, Debug)]
pub struct VcpReply {
    pub maximum: u16,
    pub current: u16,
}

/// Outcome of a strict single DDC/CI reply read.
#[derive(Debug)]
pub enum FrameError {
    /// `6e 80 be`: the monitor is busy; a stream fragment was not consumed.
    Null,
    /// Bytes that are not a reply: the monitor did not answer in time or is in an outage.
    Garbage,
    /// Valid reply with status "unsupported" (sometimes transient).
    Unsupported,
    /// The monitor is gone (cable pulled, display switched off by Windows): nothing will answer on this handle.
    Disconnected,
    Io(io::Error),
}

impl FrameError {
    pub fn describe(&self) -> String {
        match self {
            FrameError::Null => "NULL reply".into(),
            FrameError::Garbage => "invalid reply".into(),
            FrameError::Unsupported => "unsupported".into(),
            FrameError::Disconnected => "monitor disconnected".into(),
            FrameError::Io(error) => format!("I/O: {error}"),
        }
    }
}

pub trait VcpTransport: Send {
    /// One GET exchange (request, reply delay, reply), validated: `VcpReply` or a `FrameError`.
    fn get_frame(&mut self, code: u8) -> Result<VcpReply, FrameError>;
    /// One SET frame without a reply.
    fn write_frame(&mut self, code: u8, value: u16) -> io::Result<()>;
    /// Short description for logs.
    #[allow(dead_code)]
    fn describe(&self) -> String;
}

// ---- frames ------------------------------------------------------------------------------------------------

pub const DDC_DEST: u8 = 0x6E;
pub const DDC_SRC: u8 = 0x51;

/// GET VCP request: `51 82 01 code chk`.
pub fn encode_get(code: u8) -> [u8; 5] {
    let chk = DDC_DEST ^ DDC_SRC ^ 0x82 ^ 0x01 ^ code;
    [DDC_SRC, 0x82, 0x01, code, chk]
}

/// SET VCP request: `51 84 03 code hi lo chk`.
pub fn encode_set(code: u8, value: u16) -> [u8; 7] {
    let vh = (value >> 8) as u8;
    let vl = (value & 0xFF) as u8;
    let chk = DDC_DEST ^ DDC_SRC ^ 0x84 ^ 0x03 ^ code ^ vh ^ vl;
    [DDC_SRC, 0x84, 0x03, code, vh, vl, chk]
}

/// Strict validation of an 11-byte GET reply (header, checksum, code, status).
pub fn decode_reply(code: u8, resp: &[u8; 11]) -> Result<VcpReply, FrameError> {
    if resp[..3] == [0x6E, 0x80, 0xBE] {
        return Err(FrameError::Null);
    }
    let checksum = resp.iter().fold(0x50u8, |acc, byte| acc ^ byte);
    if resp[..3] != [0x6E, 0x88, 0x02] || checksum != 0 || resp[4] != code {
        return Err(FrameError::Garbage);
    }
    if resp[3] != 0 {
        return Err(FrameError::Unsupported);
    }
    Ok(VcpReply {
        maximum: ((resp[6] as u16) << 8) | resp[7] as u16,
        current: ((resp[8] as u16) << 8) | resp[9] as u16,
    })
}

// ---- bus lock ----------------------------------------------------------------------------------------------

use std::sync::{Arc, Mutex, OnceLock};

/// One lock for the whole DDC bus: exchanges of different monitors never interleave. Every monitor keeps
/// its own request pacing; only the single exchange (request, delay, reply) is serialised.
pub fn global_bus() -> Arc<Mutex<()>> {
    static BUS: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
    BUS.get_or_init(|| Arc::new(Mutex::new(()))).clone()
}

pub struct BusLocked {
    inner: Box<dyn VcpTransport>,
    bus: Arc<Mutex<()>>,
}

impl BusLocked {
    pub fn new(inner: Box<dyn VcpTransport>, bus: Arc<Mutex<()>>) -> Self {
        Self { inner, bus }
    }
}

impl VcpTransport for BusLocked {
    fn get_frame(&mut self, code: u8) -> Result<VcpReply, FrameError> {
        let _guard = self.bus.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.inner.get_frame(code)
    }

    fn write_frame(&mut self, code: u8, value: u16) -> io::Result<()> {
        let _guard = self.bus.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.inner.write_frame(code, value)
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }
}
