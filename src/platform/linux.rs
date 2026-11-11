//! Linux transport: raw I2C through `/dev/i2c-*` (the code that used to live inside `MsiDdc`).

use crate::monitor::transport::{FrameError, VcpReply, VcpTransport, decode_reply, encode_get, encode_set};
use i2c_linux::I2c;
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    thread,
    time::Duration,
};

const DDC_ADDR: u16 = 0x37;
/// Write -> read delay of a GET request.
const REPLY_DELAY: Duration = Duration::from_millis(40);

pub struct LinuxI2c {
    i2c: I2c<File>,
}

impl LinuxI2c {
    pub fn open(path: &str) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let mut i2c = I2c::new(file);
        i2c.smbus_set_slave_address(DDC_ADDR, false)?;
        Ok(Self { i2c })
    }
}

impl VcpTransport for LinuxI2c {
    /// One request/reply exchange with strict validation (header, checksum, code, status).
    fn get_frame(&mut self, code: u8) -> Result<VcpReply, FrameError> {
        self.i2c.write_all(&encode_get(code)).map_err(FrameError::Io)?;
        thread::sleep(REPLY_DELAY);

        let mut resp = [0u8; 11];
        self.i2c.read_exact(&mut resp).map_err(FrameError::Io)?;

        decode_reply(code, &resp)
    }

    /// One SET VCP frame: `51 84 03 code hi lo chk`.
    fn write_frame(&mut self, code: u8, value: u16) -> io::Result<()> {
        self.i2c.write_all(&encode_set(code, value))
    }

    fn describe(&self) -> String {
        "Linux I2C".into()
    }
}

/// First 128 bytes of the EDID of the display on an I2C bus (slave address 0x50).
pub fn read_edid_block(path: &str) -> Option<[u8; 128]> {
    let file = File::open(path).ok()?;
    let mut i2c = I2c::new(file);
    i2c.smbus_set_slave_address(0x50, false).ok()?;
    let mut data = [0u8; 128];
    i2c.read_exact(&mut data).ok()?;
    Some(data)
}
