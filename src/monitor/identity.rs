//! Identity of a physical monitor. The EDID name alone is not enough: the two P275MV of one desk have the
//! same name, the same product code and (as measured) the same serial number. What tells them apart is the Windows
//! device path; the user's alias sits on top.

use super::{parse_edid_name, parse_edid_serial};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MonitorId {
    /// EDID model name, e.g. `P275MV PLUS`.
    pub name: String,
    /// EDID manufacturer letters + product code, e.g. `LHC907D`. Empty on Linux.
    pub edid_id: String,
    /// EDID serial (text descriptor or `#XXXXXXXX`); may be empty or identical on two monitors.
    pub serial: String,
    /// Windows device path part `<hwid>\<instance>`. Empty on Linux.
    pub path: String,
}

impl MonitorId {
    /// Identity as used on Linux: the monitor name only (the cache has always been keyed this way).
    pub fn legacy(name: &str) -> Self {
        Self { name: name.to_string(), edid_id: String::new(), serial: String::new(), path: String::new() }
    }

    pub fn is_legacy(&self) -> bool {
        self.edid_id.is_empty() && self.serial.is_empty() && self.path.is_empty()
    }

    /// Key of the settings cache and of the app settings. For a legacy identity it is the bare name, so Linux
    /// keeps reading its existing cache.
    pub fn cache_key(&self) -> String {
        if self.is_legacy() {
            self.name.clone()
        } else {
            format!("{}|{}|{}|{}", self.name, self.edid_id, self.serial, self.path)
        }
    }

    /// Short text for logs and the monitor switch: name plus the tail of the path.
    pub fn label(&self) -> String {
        if self.is_legacy() {
            return self.name.clone();
        }
        let tail = self.path.rsplit(['\\', '&']).next().unwrap_or("");
        format!("{} [{}]", self.name, tail)
    }
}

/// Manufacturer letters and product code of an EDID, e.g. `LHC907D`.
pub fn edid_product_id(edid: &[u8]) -> Option<String> {
    if edid.len() < 128 || edid[0..8] != [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00] {
        return None;
    }
    let letters = [
        ((edid[8] >> 2) & 0x1F) + 64,
        (((edid[8] & 0x03) << 3) | ((edid[9] >> 5) & 0x07)) + 64,
        (edid[9] & 0x1F) + 64,
    ];
    let product = u16::from_le_bytes([edid[10], edid[11]]);
    Some(format!("{}{}{}{product:04X}", letters[0] as char, letters[1] as char, letters[2] as char))
}

/// Identity from an EDID and the device path part. `None` when the EDID is not valid.
pub fn from_edid(edid: &[u8], path: &str) -> Option<MonitorId> {
    let name = parse_edid_name(edid)?;
    let serial = parse_edid_serial(edid).unwrap_or_else(|| {
        let numeric = u32::from_le_bytes([edid[12], edid[13], edid[14], edid[15]]);
        if numeric != 0 { format!("#{numeric:08X}") } else { String::new() }
    });
    Some(MonitorId { name, edid_id: edid_product_id(edid)?, serial, path: path.to_string() })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample_edid(serial: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 128];
        d[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        d[8] = 0b0011_0001; // "LHC"
        d[9] = 0b0000_0011;
        d[10] = 0x7D;
        d[11] = 0x90;
        d[54..58].copy_from_slice(&[0, 0, 0, 0xFC]);
        d[59..59 + 11].copy_from_slice(b"P275MV PLUS");
        d[59 + 11] = 0x0A;
        d[72..76].copy_from_slice(&[0, 0, 0, 0xFF]);
        d[77..77 + serial.len()].copy_from_slice(serial);
        d[77 + serial.len()] = 0x0A;
        d
    }

    #[test]
    fn same_edid_different_paths_are_different_monitors() {
        let edid = sample_edid(b"0000000000000");
        let a = from_edid(&edid, "LHC907D\\5&1&0&UID4355").unwrap();
        let b = from_edid(&edid, "LHC907D\\5&1&0&UID4353").unwrap();
        assert_eq!(a.name, b.name);
        assert_eq!(a.edid_id, "LHC907D");
        assert_ne!(a, b);
        assert_ne!(a.cache_key(), b.cache_key());
    }

    #[test]
    fn legacy_identity_keeps_the_plain_name_as_key() {
        assert_eq!(MonitorId::legacy("P275MV PLUS").cache_key(), "P275MV PLUS");
    }
}
