//! FL Studio project (`.flp`) and preset (`.fst`) container.
//!
//! A file is two chunks. `FLhd` carries three little-endian u16 fields:
//! format, channel count and PPQ. `FLdt` carries a flat stream of events.
//! Each event starts with one ID byte, and the ID's range fixes the payload:
//!
//! | ID        | payload                                   |
//! |-----------|-------------------------------------------|
//! | 0..=63    | 1 byte                                    |
//! | 64..=127  | 2 bytes                                   |
//! | 128..=191 | 4 bytes                                   |
//! | 192..=255 | LEB128 length, then that many bytes       |
//!
//! ID byte 0xAC is an escape: the real ID is the next little-endian u32 and
//! bits 28..31 of it pick the payload class (0 one byte, 4 two, 8 four,
//! 0xC LEB128 length). FL 25.2 started writing `0xC0000101` this way. So an
//! event ID is a `u32`; plain byte IDs are the values below 256.
//!
//! Everything here is observed from the 165 project files FL Studio 26.1.7
//! ships in `Data/` (versions 10.9 through 26.1), all of which round-trip
//! byte for byte through [`Project::parse`] and [`Project::write`].
//! Event meaning lives in [`event`], one level up from this container.

pub mod event;

use thiserror::Error;

const HEADER_TAG: &[u8; 4] = b"FLhd";
const DATA_TAG: &[u8; 4] = b"FLdt";
const HEADER_LEN: u32 = 6;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    #[error("expected {expected:?} chunk tag at offset {offset}")]
    BadTag {
        expected: &'static str,
        offset: usize,
    },
    #[error("FLhd chunk is {0} bytes, expected 6")]
    BadHeaderLen(u32),
    #[error("file ends inside {what} at offset {offset}")]
    Truncated { what: &'static str, offset: usize },
    #[error("event length varint at offset {0} is longer than 5 bytes")]
    BadVarint(usize),
    #[error("{0} trailing bytes after the FLdt chunk")]
    Trailing(usize),
    #[error("extended event id {id:#x} at offset {offset} has an unknown payload class")]
    BadClass { id: u32, offset: usize },
}

/// The `FLhd` chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// 0 for a full project; presets and other containers use other values.
    pub format: i16,
    pub channel_count: u16,
    /// Pulses per quarter note. 96 in most shipped projects; 13 use 48 and
    /// one uses 72, so never assume it.
    pub ppq: u16,
}

/// ID byte that announces a 32-bit event ID.
pub const ESCAPE: u8 = 0xAC;

/// One raw event: its ID and payload bytes, exactly as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEvent {
    pub id: u32,
    pub data: Vec<u8>,
}

impl RawEvent {
    /// Whether the ID needs the 0xAC escape on disk.
    pub fn is_extended(&self) -> bool {
        self.id >= 256
    }

    /// Whether the payload is length-prefixed (class 0xC).
    pub fn is_var(&self) -> bool {
        event::kind(self.id) == event::Kind::Var
    }
}

/// A parsed file: header plus the raw event stream in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub header: Header,
    pub events: Vec<RawEvent>,
}

impl Project {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader { b: bytes, pos: 0 };
        r.tag(HEADER_TAG, "FLhd")?;
        let hlen = r.u32("FLhd length")?;
        if hlen != HEADER_LEN {
            return Err(Error::BadHeaderLen(hlen));
        }
        let header = Header {
            format: r.u16("FLhd format")? as i16,
            channel_count: r.u16("FLhd channel count")?,
            ppq: r.u16("FLhd ppq")?,
        };
        r.tag(DATA_TAG, "FLdt")?;
        let dlen = r.u32("FLdt length")? as usize;
        let end = r.pos + dlen;
        if end > bytes.len() {
            return Err(Error::Truncated {
                what: "FLdt chunk",
                offset: bytes.len(),
            });
        }
        let mut events = Vec::new();
        while r.pos < end {
            let first = r.u8("event id")?;
            let (id, class) = if first == ESCAPE {
                let id = r.u32("extended event id")?;
                (id, (id >> 28) as u8)
            } else {
                (u32::from(first), first >> 6 << 2)
            };
            let len = match class {
                0 => 1,
                4 => 2,
                8 => 4,
                0xC => r.varint()?,
                _ => return Err(Error::BadClass { id, offset: r.pos }),
            };
            let data = r.bytes(len, "event payload")?.to_vec();
            events.push(RawEvent { id, data });
        }
        if r.pos != bytes.len() {
            return Err(Error::Trailing(bytes.len() - r.pos));
        }
        Ok(Project { header, events })
    }

    pub fn write(&self) -> Vec<u8> {
        let body: usize = self
            .events
            .iter()
            .map(|e| {
                let id_len = if e.is_extended() { 5 } else { 1 };
                let len_len = if e.is_var() {
                    varint_len(e.data.len())
                } else {
                    0
                };
                id_len + len_len + e.data.len()
            })
            .sum();
        let mut out = Vec::with_capacity(8 + HEADER_LEN as usize + 8 + body);
        out.extend_from_slice(HEADER_TAG);
        out.extend_from_slice(&HEADER_LEN.to_le_bytes());
        out.extend_from_slice(&(self.header.format as u16).to_le_bytes());
        out.extend_from_slice(&self.header.channel_count.to_le_bytes());
        out.extend_from_slice(&self.header.ppq.to_le_bytes());
        out.extend_from_slice(DATA_TAG);
        out.extend_from_slice(&(body as u32).to_le_bytes());
        for e in &self.events {
            if e.is_extended() {
                out.push(ESCAPE);
                out.extend_from_slice(&e.id.to_le_bytes());
            } else {
                out.push(e.id as u8);
            }
            if e.is_var() {
                write_varint(&mut out, e.data.len());
            }
            out.extend_from_slice(&e.data);
        }
        out
    }

    /// The writer's version from event 199 as the number FL's loader
    /// compares against, `major*1_000_000 + minor*1_000 + patch` (see
    /// [`event::version_number`]). `None` when the file has no 199 or its
    /// text is not a version.
    ///
    /// This scans the events on every call. Call it once per file and hand
    /// [`event::strings_are_utf16`] of the result to [`event::text`].
    pub fn writer_version(&self) -> Option<u32> {
        event::version_number(&event::version(&self.events)?)
    }
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.b.len());
        let Some(end) = end else {
            return Err(Error::Truncated {
                what,
                offset: self.pos,
            });
        };
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self, what: &'static str) -> Result<u8, Error> {
        Ok(self.bytes(1, what)?[0])
    }
    fn u16(&mut self, what: &'static str) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.bytes(2, what)?.try_into().unwrap()))
    }
    fn u32(&mut self, what: &'static str) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.bytes(4, what)?.try_into().unwrap()))
    }
    fn tag(&mut self, tag: &[u8; 4], name: &'static str) -> Result<(), Error> {
        let offset = self.pos;
        if self.bytes(4, name)? != tag {
            return Err(Error::BadTag {
                expected: name,
                offset,
            });
        }
        Ok(())
    }
    /// Unsigned LEB128, capped at 5 bytes (u32 range) like FL itself.
    fn varint(&mut self) -> Result<usize, Error> {
        let start = self.pos;
        let mut n: u32 = 0;
        for i in 0..5 {
            let c = self.u8("event length")?;
            n |= u32::from(c & 0x7F) << (7 * i);
            if c & 0x80 == 0 {
                return Ok(n as usize);
            }
        }
        Err(Error::BadVarint(start))
    }
}

fn varint_len(mut n: usize) -> usize {
    let mut len = 1;
    while n >= 0x80 {
        n >>= 7;
        len += 1;
    }
    len
}

fn write_varint(out: &mut Vec<u8>, mut n: usize) {
    while n >= 0x80 {
        out.push((n as u8 & 0x7F) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"FLhd");
        v.extend_from_slice(&6u32.to_le_bytes());
        v.extend_from_slice(&[0, 0, 1, 0, 96, 0]);
        v.extend_from_slice(b"FLdt");
        // u8 event, u16 event, u32 event, var event with 2-byte length
        let mut body = vec![9, 1, 64, 2, 0, 128, 3, 0, 0, 0, 199];
        body.extend_from_slice(&[0x80, 0x01]); // 128 as LEB128
        body.extend(std::iter::repeat_n(b'v', 128));
        // escaped var event 0xC0000101 with a 2-byte payload, then escaped u8 0x100
        body.extend_from_slice(&[0xAC, 0x01, 0x01, 0x00, 0xC0, 2, b'h', b'i']);
        body.extend_from_slice(&[0xAC, 0x00, 0x01, 0x00, 0x00, 7]);
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(&body);
        v
    }

    #[test]
    fn round_trips() {
        let bytes = minimal();
        let p = Project::parse(&bytes).unwrap();
        assert_eq!(
            p.header,
            Header {
                format: 0,
                channel_count: 1,
                ppq: 96
            }
        );
        assert_eq!(p.events.len(), 6);
        assert_eq!(p.events[3].data.len(), 128);
        assert_eq!(
            p.events[4],
            RawEvent {
                id: 0xC000_0101,
                data: b"hi".to_vec()
            }
        );
        assert_eq!(
            p.events[5],
            RawEvent {
                id: 0x100,
                data: vec![7]
            }
        );
        assert_eq!(p.write(), bytes);
    }

    #[test]
    fn writer_version_comes_from_event_199() {
        let mut p = Project {
            header: Header {
                format: 0,
                channel_count: 0,
                ppq: 96,
            },
            events: vec![RawEvent {
                id: 9,
                data: vec![1],
            }],
        };
        assert_eq!(p.writer_version(), None);
        p.events.push(RawEvent {
            id: event::VERSION,
            data: b"24.2.99.4720\0".to_vec(),
        });
        assert_eq!(p.writer_version(), Some(24_002_099));
        assert_eq!(Project::parse(&minimal()).unwrap().writer_version(), None);
    }

    #[test]
    fn truncated_payload_is_an_error() {
        let mut bytes = minimal();
        bytes.truncate(bytes.len() - 1);
        // FLdt length now claims more than is present
        assert_eq!(
            Project::parse(&bytes),
            Err(Error::Truncated {
                what: "FLdt chunk",
                offset: bytes.len()
            })
        );
    }

    #[test]
    fn wrong_tag_is_an_error() {
        let mut bytes = minimal();
        bytes[0] = b'X';
        assert_eq!(
            Project::parse(&bytes),
            Err(Error::BadTag {
                expected: "FLhd",
                offset: 0
            })
        );
    }

    #[test]
    fn trailing_bytes_are_an_error() {
        let mut bytes = minimal();
        bytes.push(0);
        assert_eq!(Project::parse(&bytes), Err(Error::Trailing(1)));
    }
}
