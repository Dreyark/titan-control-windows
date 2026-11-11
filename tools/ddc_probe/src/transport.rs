//! Transport abstraction (prototype of `src/monitor/transport.rs`).
//!
//! READ-ONLY: this probe's trait has no write method on purpose. The main app adds `write_frame`.

/// Reply of one GET VCP exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VcpReply {
    pub maximum: u16,
    pub current: u16,
}

/// Outcome of a failed single GET exchange. `u32` values are raw Win32 error codes from `GetLastError()`.
#[derive(Clone, Debug)]
pub enum FrameError {
    /// `6e 80 be` busy reply. Dxva2 cannot report this distinctly (UNVERIFIED-ON-HW); kept for the Linux protocol.
    #[allow(dead_code)]
    Null,
    /// Bad checksum / header / data: not a valid reply.
    Garbage(u32),
    /// Valid reply with status "unsupported".
    Unsupported,
    /// Everything else (I2C errors, timing status byte, ...).
    Io(u32),
}

impl FrameError {
    pub fn kind(&self) -> &'static str {
        match self {
            FrameError::Null => "null",
            FrameError::Garbage(_) => "garbage",
            FrameError::Unsupported => "unsupported",
            FrameError::Io(_) => "io",
        }
    }

    pub fn describe(&self) -> String {
        match self {
            FrameError::Null => "NULL reply".into(),
            FrameError::Garbage(raw) => format!("invalid reply ({})", crate::win_error_name(*raw)),
            FrameError::Unsupported => "unsupported".into(),
            FrameError::Io(raw) => format!("I/O ({})", crate::win_error_name(*raw)),
        }
    }
}

pub trait VcpTransport {
    /// One GET exchange for `code`.
    fn get_frame(&mut self, code: u8) -> Result<VcpReply, FrameError>;
    /// Short description for logs.
    fn describe(&self) -> String;
}
