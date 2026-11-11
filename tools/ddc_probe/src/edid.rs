//! EDID parsing (copied from `src/monitor.rs::parse_edid_name/parse_edid_serial`, extended with ids).

const EDID_HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

pub fn parse_edid_name(data: &[u8]) -> Option<String> {
    if data.len() < 128 || data[0..8] != EDID_HEADER {
        return None;
    }
    let mfg = manufacturer(data);
    for b in 0..4 {
        let o = 54 + (b * 18);
        if data[o..o + 4] == [0, 0, 0, 0xFC] {
            let n: String = data[o + 5..o + 18]
                .iter()
                .filter(|&&c| (32..=126).contains(&c))
                .map(|&c| c as char)
                .collect();
            return Some(normalize_monitor_name(&format!("{} {}", mfg, n.trim())));
        }
    }
    Some(normalize_monitor_name(&mfg))
}

pub fn parse_edid_serial(data: &[u8]) -> Option<String> {
    if data.len() < 128 || data[0..8] != EDID_HEADER {
        return None;
    }
    for b in 0..4 {
        let o = 54 + (b * 18);
        if data[o..o + 4] == [0, 0, 0, 0xFF] {
            let serial: String = data[o + 5..o + 18]
                .iter()
                .filter(|&&c| (32..=126).contains(&c))
                .map(|&c| c as char)
                .collect();
            let serial = serial.trim().to_string();
            if !serial.is_empty() {
                return Some(serial);
            }
        }
    }
    None
}

fn manufacturer(data: &[u8]) -> String {
    format!(
        "{}{}{}",
        (((data[8] >> 2) & 0x1F) + 64) as char,
        ((((data[8] & 0x03) << 3) | ((data[9] >> 5) & 0x07)) + 64) as char,
        ((data[9] & 0x1F) + 64) as char
    )
}

fn normalize_monitor_name(name: &str) -> String {
    let trimmed = name.trim();
    trimmed
        .strip_prefix("LHC ")
        .or_else(|| trimmed.strip_prefix("LHC"))
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

/// Identity fields of an EDID.
#[derive(Clone, Debug, Default)]
pub struct EdidIdentity {
    pub name: String,
    /// Manufacturer letters + product code, e.g. `LHC1234`.
    pub edid_id: String,
    /// Serial descriptor text, or the numeric serial as `#XXXXXXXX`, or empty.
    pub serial: String,
    pub week: u8,
    pub year: u16,
}

pub fn identity(data: &[u8]) -> Option<EdidIdentity> {
    let name = parse_edid_name(data)?;
    let product = u16::from_le_bytes([data[10], data[11]]);
    let numeric = u32::from_le_bytes([data[12], data[13], data[14], data[15]]);
    let serial = parse_edid_serial(data).unwrap_or_else(|| {
        if numeric != 0 { format!("#{numeric:08X}") } else { String::new() }
    });
    Some(EdidIdentity {
        name,
        edid_id: format!("{}{:04X}", manufacturer(data), product),
        serial,
        week: data[16],
        year: 1990 + data[17] as u16,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut d = vec![0u8; 128];
        d[0..8].copy_from_slice(&EDID_HEADER);
        // "LHC": L=12, H=8, C=3 -> 0b0_01100_01000_00011
        d[8] = 0b0011_0001;
        d[9] = 0b0000_0011;
        d[10] = 0x34;
        d[11] = 0x12;
        d[12..16].copy_from_slice(&[0x78, 0x56, 0x34, 0x12]);
        d[16] = 12;
        d[17] = 34;
        d[54..58].copy_from_slice(&[0, 0, 0, 0xFC]);
        d[59..59 + 11].copy_from_slice(b"P275MV PLUS");
        d[59 + 11] = 0x0A;
        d
    }

    #[test]
    fn parses_name_and_ids() {
        let id = identity(&sample()).unwrap();
        assert_eq!(id.name, "P275MV PLUS");
        assert_eq!(id.edid_id, "LHC1234");
        assert_eq!(id.serial, "#12345678");
        assert_eq!(id.year, 2024);
    }

    #[test]
    fn serial_descriptor_wins() {
        let mut d = sample();
        d[72..76].copy_from_slice(&[0, 0, 0, 0xFF]);
        d[77..77 + 6].copy_from_slice(b"ABC123");
        d[77 + 6] = 0x0A;
        assert_eq!(identity(&d).unwrap().serial, "ABC123");
    }

    #[test]
    fn rejects_bad_header() {
        assert!(identity(&[0u8; 128]).is_none());
    }
}
