//! Packet layout and validated response decoding. See docs/PROTOCOL.md.
use crate::{Error, Result};

pub const SERVICE: u8 = 12;
pub const SERVER: u8 = 100;
pub const CHUNK: usize = 1024;
pub const MAX_PATH: usize = 255;

pub fn crc8(bytes: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &byte in bytes {
        crc ^= byte;
        for _ in 0..8 {
            crc = (crc << 1) ^ if crc & 0x80 != 0 { 7 } else { 0 };
        }
    }
    crc
}

pub fn u16_at(data: &[u8], at: usize) -> Result<u16> {
    let bytes = data
        .get(at..at + 2)
        .ok_or_else(|| Error::Protocol(format!("missing u16 at offset {at}")))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}
pub fn u32_at(data: &[u8], at: usize) -> Result<u32> {
    let bytes = data
        .get(at..at + 4)
        .ok_or_else(|| Error::Protocol(format!("missing u32 at offset {at}")))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}
pub fn put16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}
pub fn put32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn path_body(path: &str) -> Result<Vec<u8>> {
    validate_path(path)?;
    let mut out = Vec::with_capacity(path.len() + 3);
    put16(&mut out, (path.len() + 1) as u16);
    out.extend_from_slice(path.as_bytes());
    out.push(0);
    Ok(out)
}

pub fn validate_path(path: &str) -> Result<()> {
    if !path.starts_with('/')
        || path.len() > MAX_PATH
        || !path.is_ascii()
        || path.bytes().any(|c| c < 32 || c == 127 || c == b'\\')
        || path
            .split('/')
            .skip(1)
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err(Error::Invalid("Remote path must be absolute ASCII, at most 255 bytes, without empty or dot components".into()));
    }
    Ok(())
}

pub fn program_path(directory: &str, name: &str) -> Result<String> {
    let name = name.to_ascii_uppercase();
    let b = name.as_bytes();
    if b.len() != 9 || b[0] != b'O' || !b[1..5].iter().all(u8::is_ascii_digit) || &b[5..] != b".CNC"
    {
        return Err(Error::Invalid(
            "Program name must look like O1234.CNC (letter O, four digits)".into(),
        ));
    }
    let path = format!("{}/{name}", directory.trim_end_matches('/'));
    validate_path(&path)?;
    Ok(path)
}

pub fn request(local: u8, sequence: u16, operation: u8, body: &[u8]) -> Vec<u8> {
    let header = [local, SERVICE, SERVER, SERVICE];
    let mut out = vec![crc8(&header)];
    out.extend_from_slice(&header);
    put16(&mut out, sequence);
    out.extend_from_slice(&[0, operation]);
    out.extend_from_slice(body);
    out
}

/// Ignore foreign and stale datagrams. A matching body is decoded by the operation.
/// The CNC sends 0x01 in byte zero, not the request CRC. MiniSock's receive
/// parser (0x10002e80) also leaves that byte unchecked; routing and sequence
/// fields below, plus UDP's checksum, identify the response.
pub fn response(packet: &[u8], local: u8, sequence: u16, operation: u8) -> Option<&[u8]> {
    if packet.len() < 9
        || packet.len() > 4096
        || packet[1..5] != [SERVER, SERVICE, local, SERVICE]
        || packet[5..7] != sequence.to_le_bytes()
        || packet[7] != 0
        || packet[8] & 0x80 == 0
        || packet[8] & 0x3f != operation
    {
        return None;
    }
    Some(&packet[5..])
}

pub fn check(payload: &[u8], operation: u8, error_offset: usize) -> Result<()> {
    if payload.len() < 4 {
        return Err(Error::Protocol("short response header".into()));
    }
    if payload[3] & 0x40 != 0 {
        let code = u16_at(payload, error_offset)?;
        return Err(Error::Remote { operation, code });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_request_vector() {
        let mut body = path_body("/user/NCPROG/O1234.CNC").unwrap();
        body.extend_from_slice(&[0, 0]);
        let packet = request(1, 0, 6, &body);
        assert_eq!(
            packet
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            "69010c640c0000000617002f757365722f4e4350524f472f4f313233342e434e43000000"
        );
    }
    #[test]
    fn validates_headers_and_lengths() {
        let mut p = vec![0x01, 100, 12, 88, 12, 0, 0, 0, 0xbf, 3, 0, 1, 0, 0];
        assert!(response(&p, 88, 0, 63).is_some());
        assert!(response(&p, 88, 1, 63).is_none());
        p[0] ^= 1;
        assert!(response(&p, 88, 0, 63).is_some());
        p[3] ^= 1;
        assert!(response(&p, 88, 0, 63).is_none());
        for n in 0..9 {
            assert!(response(&p[..n], 88, 0, 63).is_none());
        }
        assert!(u32_at(&[0, 0, 0], 0).is_err());
    }
    #[test]
    fn path_validation() {
        assert_eq!(
            program_path("/user/NCPROG", "o1234.cnc").unwrap(),
            "/user/NCPROG/O1234.CNC"
        );
        for name in [
            "O123.CNC",
            "O12345.CNC",
            "../O1234.CNC",
            "01234.CNC",
            "O1234.CNC\0",
        ] {
            assert!(program_path("/user/NCPROG", name).is_err());
        }
        assert!(validate_path("/user/../NCPROG").is_err());
        assert!(validate_path(&format!("/{}", "a".repeat(255))).is_err());
    }
}
