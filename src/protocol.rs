//! Pure protocol types, parsing, and response building for the memcached
//! binary protocol.  This module has **no** dependency on `redis-module` and
//! can be tested in a normal `cargo test` process.

use byteorder::{BigEndian, ByteOrder};
use std::io::{self, Write};

// ── Magic bytes ──────────────────────────────────────────────────────
pub const MAGIC_REQ: u8 = 0x80;
pub const MAGIC_RES: u8 = 0x81;

// ── Status codes ─────────────────────────────────────────────────────
pub const ST_OK: u16 = 0x0000;
pub const ST_NF: u16 = 0x0001;
pub const ST_IX: u16 = 0x0002;
pub const ST_ARGS: u16 = 0x0004;
pub const ST_UNK: u16 = 0x0081;

// ── Opcodes ──────────────────────────────────────────────────────────
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Opcode {
    Get = 0x00,
    Set = 0x01,
    Add = 0x02,
    Replace = 0x03,
    Delete = 0x04,
    Increment = 0x05,
    Decrement = 0x06,
    Quit = 0x07,
    GetQ = 0x09,
    Noop = 0x0a,
    GetK = 0x0c,
    GetKQ = 0x0d,
}

impl Opcode {
    pub fn parse(b: u8) -> Option<Self> {
        use Opcode::*;
        Some(match b {
            0x00 => Get,
            0x01 => Set,
            0x02 => Add,
            0x03 => Replace,
            0x04 => Delete,
            0x05 => Increment,
            0x06 => Decrement,
            0x07 => Quit,
            0x09 => GetQ,
            0x0a => Noop,
            0x0c => GetK,
            0x0d => GetKQ,
            _ => return None,
        })
    }

    /// Returns `true` for quiet variants that suppress miss responses.
    pub fn is_quiet(self) -> bool {
        matches!(self, Opcode::GetQ | Opcode::GetKQ)
    }

    /// Returns `true` for GETK/GETKQ which echo the key in the response.
    pub fn includes_key(self) -> bool {
        matches!(self, Opcode::GetK | Opcode::GetKQ)
    }
}

// ── Request header ───────────────────────────────────────────────────
pub const HEADER_LEN: usize = 24;

#[derive(Debug)]
pub struct Header {
    pub opcode: Opcode,
    pub key_len: u16,
    pub extras_len: u8,
    pub body_len: u32,
    pub opaque: u32,
    pub cas: u64,
}

impl Header {
    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < HEADER_LEN || buf[0] != MAGIC_REQ {
            return None;
        }
        Some(Self {
            opcode: Opcode::parse(buf[1])?,
            key_len: BigEndian::read_u16(&buf[2..4]),
            extras_len: buf[4],
            body_len: BigEndian::read_u32(&buf[8..12]),
            opaque: BigEndian::read_u32(&buf[12..16]),
            cas: BigEndian::read_u64(&buf[16..24]),
        })
    }
}

// ── Parsed request ───────────────────────────────────────────────────
pub struct Request<'a> {
    pub hdr: Header,
    pub extras: &'a [u8],
    pub key: &'a [u8],
    pub value: &'a [u8],
}

/// Try to parse one complete request from `buf`.
/// Returns `(request, bytes_consumed)` on success.
pub fn parse_request(buf: &[u8]) -> Option<(Request<'_>, usize)> {
    let hdr = Header::parse(buf)?;
    let total = HEADER_LEN + hdr.body_len as usize;
    if buf.len() < total {
        return None;
    }
    let extras_end = HEADER_LEN + hdr.extras_len as usize;
    let key_end = extras_end + hdr.key_len as usize;
    if key_end > total || extras_end > total {
        return None;
    }
    Some((
        Request {
            hdr,
            extras: &buf[HEADER_LEN..extras_end],
            key: &buf[extras_end..key_end],
            value: &buf[key_end..total],
        },
        total,
    ))
}

// ── Response building ────────────────────────────────────────────────

/// Write a complete binary-protocol response to `w`.
pub fn write_response(
    w: &mut impl Write,
    opcode: Opcode,
    status: u16,
    opaque: u32,
    cas: u64,
    extras: &[u8],
    key: &[u8],
    value: &[u8],
) -> io::Result<()> {
    let total_body = extras.len() as u32 + key.len() as u32 + value.len() as u32;
    let mut hdr = [0u8; HEADER_LEN];
    hdr[0] = MAGIC_RES;
    hdr[1] = opcode as u8;
    BigEndian::write_u16(&mut hdr[2..4], key.len() as u16);
    hdr[4] = extras.len() as u8;
    BigEndian::write_u16(&mut hdr[6..8], status);
    BigEndian::write_u32(&mut hdr[8..12], total_body);
    BigEndian::write_u32(&mut hdr[12..16], opaque);
    BigEndian::write_u64(&mut hdr[16..24], cas);
    w.write_all(&hdr)?;
    w.write_all(extras)?;
    w.write_all(key)?;
    w.write_all(value)?;
    Ok(())
}

/// Convenience: write a simple response with no extras or key.
pub fn write_simple_response(
    w: &mut impl Write,
    opcode: Opcode,
    status: u16,
    opaque: u32,
    cas: u64,
    body: &[u8],
) -> io::Result<()> {
    write_response(w, opcode, status, opaque, cas, &[], &[], body)
}

// ── Helper to build a raw request frame ──────────────────────────────
/// Build a binary-protocol request frame from parts.  Useful for tests
/// and for any code that needs to construct wire-format requests.
pub fn build_request_frame(
    opcode: Opcode,
    opaque: u32,
    cas: u64,
    extras: &[u8],
    key: &[u8],
    value: &[u8],
) -> Vec<u8> {
    let body_len = extras.len() + key.len() + value.len();
    let mut frame = vec![0u8; HEADER_LEN + body_len];
    frame[0] = MAGIC_REQ;
    frame[1] = opcode as u8;
    BigEndian::write_u16(&mut frame[2..4], key.len() as u16);
    frame[4] = extras.len() as u8;
    BigEndian::write_u32(&mut frame[8..12], body_len as u32);
    BigEndian::write_u32(&mut frame[12..16], opaque);
    BigEndian::write_u64(&mut frame[16..24], cas);
    frame[HEADER_LEN..HEADER_LEN + extras.len()].copy_from_slice(extras);
    let key_start = HEADER_LEN + extras.len();
    frame[key_start..key_start + key.len()].copy_from_slice(key);
    let val_start = key_start + key.len();
    frame[val_start..val_start + value.len()].copy_from_slice(value);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcode_round_trip() {
        for &(byte, expected) in &[
            (0x00, Opcode::Get),
            (0x01, Opcode::Set),
            (0x02, Opcode::Add),
            (0x03, Opcode::Replace),
            (0x04, Opcode::Delete),
            (0x05, Opcode::Increment),
            (0x06, Opcode::Decrement),
            (0x07, Opcode::Quit),
            (0x09, Opcode::GetQ),
            (0x0a, Opcode::Noop),
            (0x0c, Opcode::GetK),
            (0x0d, Opcode::GetKQ),
        ] {
            assert_eq!(Opcode::parse(byte), Some(expected));
            assert_eq!(expected as u8, byte);
        }
    }

    #[test]
    fn opcode_unknown_returns_none() {
        assert!(Opcode::parse(0xFF).is_none());
        assert!(Opcode::parse(0x80).is_none());
    }

    #[test]
    fn quiet_and_key_flags() {
        assert!(Opcode::GetQ.is_quiet());
        assert!(Opcode::GetKQ.is_quiet());
        assert!(!Opcode::Get.is_quiet());
        assert!(Opcode::GetK.includes_key());
        assert!(Opcode::GetKQ.includes_key());
        assert!(!Opcode::Get.includes_key());
    }

    #[test]
    fn header_parse_valid() {
        let frame = build_request_frame(Opcode::Get, 42, 0, &[], b"mykey", &[]);
        let hdr = Header::parse(&frame).expect("should parse");
        assert_eq!(hdr.opcode, Opcode::Get);
        assert_eq!(hdr.key_len, 5);
        assert_eq!(hdr.extras_len, 0);
        assert_eq!(hdr.body_len, 5);
        assert_eq!(hdr.opaque, 42);
        assert_eq!(hdr.cas, 0);
    }

    #[test]
    fn header_rejects_short_buffer() {
        assert!(Header::parse(&[0x80; 10]).is_none());
    }

    #[test]
    fn header_rejects_wrong_magic() {
        let mut frame = build_request_frame(Opcode::Noop, 0, 0, &[], &[], &[]);
        frame[0] = 0x00;
        assert!(Header::parse(&frame).is_none());
    }

    #[test]
    fn parse_request_get() {
        let frame = build_request_frame(Opcode::Get, 7, 0, &[], b"hello", &[]);
        let (req, consumed) = parse_request(&frame).expect("should parse");
        assert_eq!(consumed, HEADER_LEN + 5);
        assert_eq!(req.hdr.opcode, Opcode::Get);
        assert_eq!(req.key, b"hello");
        assert!(req.extras.is_empty());
        assert!(req.value.is_empty());
    }

    #[test]
    fn parse_request_set_with_extras_and_value() {
        let extras = [0u8; 8];
        let frame = build_request_frame(Opcode::Set, 1, 0, &extras, b"k", b"val");
        let (req, consumed) = parse_request(&frame).expect("should parse");
        assert_eq!(consumed, HEADER_LEN + 8 + 1 + 3);
        assert_eq!(req.extras.len(), 8);
        assert_eq!(req.key, b"k");
        assert_eq!(req.value, b"val");
    }

    #[test]
    fn parse_request_incomplete_returns_none() {
        let frame = build_request_frame(Opcode::Get, 0, 0, &[], b"key", &[]);
        assert!(parse_request(&frame[..frame.len() - 1]).is_none());
    }

    #[test]
    fn parse_request_two_in_buffer() {
        let f1 = build_request_frame(Opcode::Noop, 1, 0, &[], &[], &[]);
        let f2 = build_request_frame(Opcode::Quit, 2, 0, &[], &[], &[]);
        let mut buf = f1.clone();
        buf.extend_from_slice(&f2);

        let (req1, c1) = parse_request(&buf).expect("first");
        assert_eq!(req1.hdr.opcode, Opcode::Noop);
        assert_eq!(req1.hdr.opaque, 1);

        let (req2, c2) = parse_request(&buf[c1..]).expect("second");
        assert_eq!(req2.hdr.opcode, Opcode::Quit);
        assert_eq!(req2.hdr.opaque, 2);
        assert_eq!(c1 + c2, buf.len());
    }

    #[test]
    fn write_response_simple() {
        let mut out = Vec::new();
        write_simple_response(&mut out, Opcode::Noop, ST_OK, 99, 0, &[])
            .expect("write");
        assert_eq!(out.len(), HEADER_LEN);
        assert_eq!(out[0], MAGIC_RES);
        assert_eq!(out[1], Opcode::Noop as u8);
        assert_eq!(BigEndian::read_u16(&out[6..8]), ST_OK);
        assert_eq!(BigEndian::read_u32(&out[12..16]), 99);
    }

    #[test]
    fn write_response_with_body() {
        let mut out = Vec::new();
        let extras = 0u32.to_be_bytes();
        write_response(&mut out, Opcode::Get, ST_OK, 5, 100, &extras, &[], b"val")
            .expect("write");
        assert_eq!(out.len(), HEADER_LEN + 4 + 3);
        assert_eq!(out[4], 4);
        assert_eq!(BigEndian::read_u32(&out[8..12]), 7);
        assert_eq!(BigEndian::read_u64(&out[16..24]), 100);
        assert_eq!(&out[HEADER_LEN + 4..], b"val");
    }

    #[test]
    fn write_response_getk_with_key() {
        let mut out = Vec::new();
        let extras = 0u32.to_be_bytes();
        write_response(
            &mut out, Opcode::GetK, ST_OK, 0, 1,
            &extras, b"mykey", b"myval",
        ).expect("write");
        let key_len = BigEndian::read_u16(&out[2..4]);
        assert_eq!(key_len, 5);
        let key_start = HEADER_LEN + 4;
        assert_eq!(&out[key_start..key_start + 5], b"mykey");
        assert_eq!(&out[key_start + 5..], b"myval");
    }
}
