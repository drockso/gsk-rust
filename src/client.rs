use crate::{
    Error, Result,
    wire::{self, put16, put32, u16_at, u32_at},
};
use std::{
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct Config {
    pub address: Ipv4Addr,
    pub port: u16,
    pub local_port: u16,
    pub timeout: Duration,
    /// Additional attempts only for explicit-offset I/O, stat and mount.
    pub retries: u8,
    pub verbose: bool,
    pub cancelled: Arc<AtomicBool>,
}
impl Config {
    pub fn new(address: Ipv4Addr) -> Self {
        Self {
            address,
            port: 6000,
            local_port: 0,
            timeout: Duration::from_secs(3),
            retries: 2,
            verbose: false,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub size: u32,
    pub mode: u32,
}
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub size: u32,
    pub mode: u32,
}
impl DirEntry {
    pub fn is_directory(&self) -> bool {
        self.mode & 0x170000 == 0x040000
    }
}

pub struct Client {
    socket: UdpSocket,
    config: Config,
    local_id: u8,
    sequence: u16,
}

impl Client {
    pub fn connect(config: Config) -> Result<Self> {
        if config.address.octets()[0] == 0
            || config.address.is_multicast()
            || config.address.is_broadcast()
            || config.port == 0
            || config.timeout.is_zero()
            || config.timeout > Duration::from_secs(60)
        {
            return Err(Error::Invalid(
                "Expected a unicast IPv4 address, nonzero UDP port and timeout in 1..60000 ms"
                    .into(),
            ));
        }
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, config.local_port))?;
        socket.connect((config.address, config.port))?;
        socket.set_write_timeout(Some(config.timeout))?;
        let local_octet = match socket.local_addr()? {
            SocketAddr::V4(addr) => addr.ip().octets()[3],
            _ => unreachable!("IPv4 socket"),
        };
        let local_id = if [0, 100, 254, 255].contains(&local_octet) {
            25
        } else {
            local_octet
        };
        let sequence = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
            ^ std::process::id()) as u16;
        let mut client = Self {
            socket,
            config,
            local_id,
            sequence,
        };
        client.mount()?;
        Ok(client)
    }

    fn exchange(
        &mut self,
        operation: u8,
        body: &[u8],
        retry_safe: bool,
        cleanup: bool,
    ) -> Result<Vec<u8>> {
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        let packet = wire::request(self.local_id, sequence, operation, body);
        let attempts = if retry_safe {
            self.config.retries as usize + 1
        } else {
            1
        };
        let mut buffer = [0u8; 65536];
        for attempt in 0..attempts {
            if !cleanup && self.config.cancelled.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            if self.config.verbose {
                eprintln!(
                    "UDP tx op=0x{operation:02X} seq={sequence} bytes={} attempt={}",
                    packet.len(),
                    attempt + 1
                );
            }
            if self.socket.send(&packet)? != packet.len() {
                return Err(Error::Protocol("short UDP send".into()));
            }
            let deadline = Instant::now() + self.config.timeout;
            loop {
                if !cleanup && self.config.cancelled.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                self.socket
                    .set_read_timeout(Some(remaining.min(Duration::from_millis(100))))?;
                match self.socket.recv(&mut buffer) {
                    Ok(count) => {
                        if let Some(payload) =
                            wire::response(&buffer[..count], self.local_id, sequence, operation)
                        {
                            if self.config.verbose {
                                eprintln!(
                                    "UDP rx op=0x{operation:02X} seq={sequence} bytes={count}"
                                );
                            }
                            return Ok(payload.to_vec());
                        }
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock
                                | io::ErrorKind::TimedOut
                                | io::ErrorKind::Interrupted
                        ) => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Err(Error::Timeout {
            operation,
            uncertain: !retry_safe,
        })
    }

    fn mount(&mut self) -> Result<()> {
        let response = self.exchange(0x3f, &[3, 0, 1, 0, 0], true, false)?;
        wire::check(&response, 0x3f, 6)?;
        if u16_at(&response, 4)? != 3 {
            return Err(Error::Protocol("wrong mount subcommand".into()));
        }
        Ok(())
    }

    pub fn stat(&mut self, path: &str) -> Result<FileInfo> {
        let mut body = wire::path_body(path)?;
        body.extend_from_slice(&[0, 0]); // Native FileStat sends two padding bytes.
        let response = self.exchange(6, &body, true, false)?;
        wire::check(&response, 6, 4)?;
        let length = u16_at(&response, 4)? as usize;
        let data = response
            .get(6..6 + length)
            .ok_or_else(|| Error::Protocol("truncated stat data".into()))?;
        Ok(FileInfo {
            size: u32_at(data, 28)?,
            mode: u32_at(data, 8)?,
        })
    }

    fn open(&mut self, path: &str, mode: u16, directory: bool) -> Result<u16> {
        let mut body = Vec::new();
        put16(&mut body, mode);
        body.extend(wire::path_body(path)?);
        let operation = if directory { 8 } else { 1 };
        let handle_offset = if directory { 6 } else { 8 };
        let response = self.exchange(operation, &body, false, false)?;
        wire::check(&response, operation, handle_offset)?;
        if u16_at(&response, 4)? != mode {
            return Err(Error::Protocol("open mode differs from request".into()));
        }
        u16_at(&response, handle_offset)
    }

    fn close(&mut self, handle: u16, directory: bool) -> Result<()> {
        let operation = if directory { 9 } else { 4 };
        let response = self.exchange(operation, &handle.to_le_bytes(), false, true)?;
        if response.get(3).is_some_and(|flags| flags & 0x40 != 0) {
            return Err(Error::Remote {
                operation,
                code: u16_at(&response, 6).unwrap_or(0x800a),
            });
        }
        if u16_at(&response, 4)? != handle {
            return Err(Error::Protocol("close returned wrong handle".into()));
        }
        Ok(())
    }

    fn write_block(&mut self, handle: u16, offset: u32, data: &[u8]) -> Result<()> {
        let mut body = Vec::with_capacity(data.len() + 8);
        put16(&mut body, handle);
        put16(&mut body, data.len() as u16);
        put32(&mut body, offset);
        body.extend_from_slice(data);
        let response = self.exchange(3, &body, true, false)?;
        wire::check(&response, 3, 12)?;
        if u16_at(&response, 4)? != handle
            || u16_at(&response, 6)? as usize != data.len()
            || u32_at(&response, 8)? != offset
        {
            return Err(Error::Protocol(
                "write acknowledgement has wrong handle, offset or byte count".into(),
            ));
        }
        Ok(())
    }

    fn read_block(&mut self, handle: u16, offset: u32, count: u16) -> Result<Option<Vec<u8>>> {
        let mut body = Vec::new();
        put16(&mut body, handle);
        put16(&mut body, count);
        put32(&mut body, offset);
        let response = self.exchange(2, &body, true, false)?;
        match wire::check(&response, 2, 12) {
            // Real CNC EOF packets contain stale count/offset fields: never consume them.
            Err(Error::Remote { code: 0x8011, .. }) => return Ok(None),
            other => other?,
        }
        let actual = u16_at(&response, 6)? as usize;
        if u16_at(&response, 4)? != handle
            || u32_at(&response, 8)? != offset
            || actual > count as usize
        {
            return Err(Error::Protocol(
                "read response has wrong handle, offset or byte count".into(),
            ));
        }
        if actual == 0 {
            return Err(Error::Protocol(
                "zero-length read without explicit EOF".into(),
            ));
        }
        let data = response
            .get(12..12 + actual)
            .ok_or_else(|| Error::Protocol("truncated read payload".into()))?;
        Ok(Some(data.to_vec()))
    }

    /// Source should be an immutable local snapshot. Always closes the remote handle.
    pub fn send<R: Read>(
        &mut self,
        path: &str,
        source: &mut R,
        length: u32,
        no_clobber: bool,
    ) -> Result<()> {
        match self.stat(path) {
            Ok(_) if no_clobber => {
                return Err(Error::Invalid(format!("Remote file exists: {path}")));
            }
            Ok(_) => {}
            Err(e) if e.is_not_found() => {}
            Err(e) => return Err(e),
        }
        let handle = self.open(path, 0x1a, false)?;
        let result = (|| {
            let mut offset = 0u32;
            let mut buffer = [0u8; wire::CHUNK];
            while offset < length {
                let count = (length - offset).min(wire::CHUNK as u32) as usize;
                source.read_exact(&mut buffer[..count])?;
                self.write_block(handle, offset, &buffer[..count])?;
                offset += count as u32;
            }
            Ok(())
        })();
        let closed = self.close(handle, false);
        result.and(closed)
    }

    pub fn receive<W: Write>(&mut self, path: &str, destination: &mut W) -> Result<u32> {
        let expected = self.stat(path)?.size;
        let handle = self.open(path, 0x11, false)?;
        let result = (|| {
            let mut offset = 0u32;
            loop {
                let Some(data) = self.read_block(handle, offset, wire::CHUNK as u16)? else {
                    if offset != expected {
                        return Err(Error::Protocol(format!(
                            "early EOF: expected {expected} bytes, received {offset}"
                        )));
                    }
                    break;
                };
                let next = offset
                    .checked_add(data.len() as u32)
                    .ok_or_else(|| Error::Protocol("file exceeds 32-bit protocol limit".into()))?;
                if next > expected {
                    return Err(Error::Protocol("remote file grew during download".into()));
                }
                destination.write_all(&data)?;
                offset = next;
            }
            destination.flush()?;
            Ok(offset)
        })();
        let closed = self.close(handle, false);
        match result {
            Ok(n) => closed.map(|()| n),
            Err(e) => Err(e),
        }
    }

    pub fn list(&mut self, path: &str) -> Result<Vec<DirEntry>> {
        let handle = self.open(path, 0x100, true)?;
        let result = (|| {
            let mut entries = Vec::new();
            loop {
                let mut body = Vec::new();
                put16(&mut body, handle);
                put16(&mut body, wire::CHUNK as u16);
                // Directory enumeration advances a server cursor: don't replay a lost reply.
                let response = self.exchange(10, &body, false, false)?;
                wire::check(&response, 10, 8)?;
                if u16_at(&response, 4)? != handle {
                    return Err(Error::Protocol("wrong directory handle".into()));
                }
                let count = u16_at(&response, 6)? as usize;
                let size = u16_at(&response, 8)? as usize;
                if size > wire::CHUNK {
                    return Err(Error::Protocol("oversized directory page".into()));
                }
                let data = response
                    .get(10..10 + size)
                    .ok_or_else(|| Error::Protocol("truncated directory page".into()))?;
                let page = parse_directory(data, count)?;
                if count == 0 {
                    break;
                }
                entries.extend(page);
                if entries.len() > 100_000 {
                    return Err(Error::Protocol("directory entry limit exceeded".into()));
                }
            }
            Ok(entries)
        })();
        let closed = self.close(handle, true);
        match result {
            Ok(entries) => closed.map(|()| entries),
            Err(e) => Err(e),
        }
    }
}

fn parse_directory(data: &[u8], count: usize) -> Result<Vec<DirEntry>> {
    let mut entries = Vec::new();
    let mut position = 0;
    for _ in 0..count {
        let length = u32_at(data, position)? as usize;
        if length < 25 || length > data.len().saturating_sub(position) {
            return Err(Error::Protocol("invalid directory record length".into()));
        }
        let record = &data[position..position + length];
        let name_bytes = &record[24..];
        let nul = name_bytes
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| Error::Protocol("unterminated directory name".into()))?;
        entries.push(DirEntry {
            name: String::from_utf8_lossy(&name_bytes[..nul]).into_owned(),
            size: u32_at(record, 8)?,
            mode: u32_at(record, 4)?,
        });
        position += length;
    }
    if position != data.len() {
        return Err(Error::Protocol(
            "directory count and payload size disagree".into(),
        ));
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captured_directory_record() {
        let mut record = vec![0u8; 36];
        record[..4].copy_from_slice(&36u32.to_le_bytes());
        record[4..8].copy_from_slice(&0x100600u32.to_le_bytes());
        record[8..12].copy_from_slice(&15656u32.to_le_bytes());
        record[24..33].copy_from_slice(b"O3998.CNC");
        let entry = parse_directory(&record, 1).unwrap().remove(0);
        assert_eq!(entry.name, "O3998.CNC");
        assert_eq!(entry.size, 15656);
        assert!(!entry.is_directory());
        assert!(parse_directory(&record, 2).is_err());
        record[..4].fill(0xff);
        assert!(parse_directory(&record, 1).is_err());
    }
}
