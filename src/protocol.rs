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
pub const ST_NOT_STORED: u16 = 0x0005;
pub const ST_UNK: u16 = 0x0081;

// ── CAS policy ──────────────────────────────────────────────────────
// CAS is now tracked per-item via a Redis-backed monotonic counter
// (`redcouch:sys:cas_counter`).  Every mutation generates a new CAS
// value from this counter and stores it in the item's hash field `c`.
// Error and control responses return CAS_ZERO (0).
pub const CAS_ZERO: u64 = 0;

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
    Flush = 0x08,
    GetQ = 0x09,
    Noop = 0x0a,
    Version = 0x0b,
    GetK = 0x0c,
    GetKQ = 0x0d,
    Append = 0x0e,
    Prepend = 0x0f,
    SetQ = 0x11,
    AddQ = 0x12,
    ReplaceQ = 0x13,
    DeleteQ = 0x14,
    IncrementQ = 0x15,
    DecrementQ = 0x16,
    QuitQ = 0x17,
    FlushQ = 0x18,
    AppendQ = 0x19,
    PrependQ = 0x1a,
    Touch = 0x1c,
    GAT = 0x1d,
    GATQ = 0x1e,
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
            0x08 => Flush,
            0x09 => GetQ,
            0x0a => Noop,
            0x0b => Version,
            0x0c => GetK,
            0x0d => GetKQ,
            0x0e => Append,
            0x0f => Prepend,
            0x11 => SetQ,
            0x12 => AddQ,
            0x13 => ReplaceQ,
            0x14 => DeleteQ,
            0x15 => IncrementQ,
            0x16 => DecrementQ,
            0x17 => QuitQ,
            0x18 => FlushQ,
            0x19 => AppendQ,
            0x1a => PrependQ,
            0x1c => Touch,
            0x1d => GAT,
            0x1e => GATQ,
            _ => return None,
        })
    }

    /// Returns `true` for quiet variants that suppress certain responses.
    /// GET quiet variants suppress miss responses; mutation quiet variants
    /// suppress success responses (errors are still sent).
    pub fn is_quiet(self) -> bool {
        use Opcode::*;
        matches!(
            self,
            GetQ | GetKQ | SetQ | AddQ | ReplaceQ | DeleteQ
                | IncrementQ | DecrementQ | QuitQ | FlushQ
                | AppendQ | PrependQ | GATQ
        )
    }

    /// Returns `true` for GETK/GETKQ/GAT/GATQ which echo the key in the response.
    pub fn includes_key(self) -> bool {
        matches!(self, Opcode::GetK | Opcode::GetKQ | Opcode::GAT | Opcode::GATQ)
    }

    /// Returns the "loud" base opcode for a quiet variant, or self if
    /// already loud.  Useful for grouping quiet and loud variants in
    /// match arms.
    pub fn base(self) -> Self {
        use Opcode::*;
        match self {
            SetQ => Set,
            AddQ => Add,
            ReplaceQ => Replace,
            DeleteQ => Delete,
            IncrementQ => Increment,
            DecrementQ => Decrement,
            QuitQ => Quit,
            FlushQ => Flush,
            GetQ => Get,
            GetKQ => GetK,
            AppendQ => Append,
            PrependQ => Prepend,
            GATQ => GAT,
            other => other,
        }
    }
}

// ── Request header ───────────────────────────────────────────────────
pub const HEADER_LEN: usize = 24;

#[derive(Debug)]
pub struct Header {
    /// Parsed opcode, or `None` if the opcode byte is not recognised.
    pub opcode: Option<Opcode>,
    /// Raw opcode byte from the wire — always available even when the
    /// opcode is unknown, so we can echo it in error responses.
    pub opcode_byte: u8,
    pub key_len: u16,
    pub extras_len: u8,
    pub body_len: u32,
    pub opaque: u32,
    pub cas: u64,
}

impl Header {
    /// Parse a request header from the front of `buf`.
    ///
    /// Returns `None` only when the buffer is too short to contain a
    /// header.  A wrong magic byte or unknown opcode is reported via
    /// [`ParseResult`] by [`try_parse_request`].
    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < HEADER_LEN {
            return None;
        }
        Some(Self {
            opcode: Opcode::parse(buf[1]),
            opcode_byte: buf[1],
            key_len: BigEndian::read_u16(&buf[2..4]),
            extras_len: buf[4],
            body_len: BigEndian::read_u32(&buf[8..12]),
            opaque: BigEndian::read_u32(&buf[12..16]),
            cas: BigEndian::read_u64(&buf[16..24]),
        })
    }
}

// ── Parsed request ───────────────────────────────────────────────────
#[derive(Debug)]
pub struct Request<'a> {
    pub hdr: Header,
    pub extras: &'a [u8],
    pub key: &'a [u8],
    pub value: &'a [u8],
}

/// Outcome of [`try_parse_request`].
#[derive(Debug, PartialEq, Eq)]
pub enum ParseResult<T> {
    /// A complete, well-formed frame was consumed.
    Ok(T),
    /// Not enough bytes yet — caller should read more data.
    Incomplete,
    /// The first byte is not MAGIC_REQ (0x80).  The frame is
    /// unrecoverable; the caller should close the connection.
    BadMagic,
    /// The header was parseable but `extras_len + key_len > body_len`.
    /// The caller should skip `bytes_to_skip` bytes and respond with
    /// an error.
    MalformedFrame {
        opaque: u32,
        opcode_byte: u8,
        bytes_to_skip: usize,
    },
}

/// Try to parse one complete request from `buf`.
///
/// Returns a [`ParseResult`] that distinguishes "need more data"
/// (`Incomplete`), "unrecoverable framing error" (`BadMagic`), and
/// "parseable header but invalid body layout" (`MalformedFrame`) from
/// a successful parse (`Ok`).
pub fn try_parse_request(buf: &[u8]) -> ParseResult<(Request<'_>, usize)> {
    if buf.is_empty() {
        return ParseResult::Incomplete;
    }
    // Check magic before anything else.
    if buf[0] != MAGIC_REQ {
        return ParseResult::BadMagic;
    }
    let hdr = match Header::parse(buf) {
        Some(h) => h,
        None => return ParseResult::Incomplete,
    };
    let total = HEADER_LEN + hdr.body_len as usize;
    if buf.len() < total {
        return ParseResult::Incomplete;
    }
    let extras_end = HEADER_LEN + hdr.extras_len as usize;
    let key_end = extras_end + hdr.key_len as usize;
    if key_end > total || extras_end > total {
        return ParseResult::MalformedFrame {
            opaque: hdr.opaque,
            opcode_byte: hdr.opcode_byte,
            bytes_to_skip: total,
        };
    }
    ParseResult::Ok((
        Request {
            hdr,
            extras: &buf[HEADER_LEN..extras_end],
            key: &buf[extras_end..key_end],
            value: &buf[key_end..total],
        },
        total,
    ))
}

/// Legacy convenience wrapper — returns `None` for any non-Ok result.
/// Prefer [`try_parse_request`] in new code.
pub fn parse_request(buf: &[u8]) -> Option<(Request<'_>, usize)> {
    match try_parse_request(buf) {
        ParseResult::Ok(pair) => Some(pair),
        _ => None,
    }
}

// ── Response building ────────────────────────────────────────────────

/// Write a complete binary-protocol response to `w` using a raw opcode byte.
/// This is the low-level writer; prefer [`write_response`] when you have
/// a known `Opcode`.
pub fn write_raw_response(
    w: &mut impl Write,
    opcode_byte: u8,
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
    hdr[1] = opcode_byte;
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
    write_raw_response(w, opcode as u8, status, opaque, cas, extras, key, value)
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

/// Write an error response for an unknown or malformed opcode, using
/// the raw opcode byte from the wire.
pub fn write_error_for_raw_opcode(
    w: &mut impl Write,
    opcode_byte: u8,
    status: u16,
    opaque: u32,
    body: &[u8],
) -> io::Result<()> {
    write_raw_response(w, opcode_byte, status, opaque, CAS_ZERO, &[], &[], body)
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
    build_raw_request_frame(opcode as u8, opaque, cas, extras, key, value)
}

/// Build a request frame using a raw opcode byte.  Useful for testing
/// unknown-opcode handling.
pub fn build_raw_request_frame(
    opcode_byte: u8,
    opaque: u32,
    cas: u64,
    extras: &[u8],
    key: &[u8],
    value: &[u8],
) -> Vec<u8> {
    let body_len = extras.len() + key.len() + value.len();
    let mut frame = vec![0u8; HEADER_LEN + body_len];
    frame[0] = MAGIC_REQ;
    frame[1] = opcode_byte;
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
            (0x08, Opcode::Flush),
            (0x09, Opcode::GetQ),
            (0x0a, Opcode::Noop),
            (0x0b, Opcode::Version),
            (0x0c, Opcode::GetK),
            (0x0d, Opcode::GetKQ),
            (0x0e, Opcode::Append),
            (0x0f, Opcode::Prepend),
            (0x11, Opcode::SetQ),
            (0x12, Opcode::AddQ),
            (0x13, Opcode::ReplaceQ),
            (0x14, Opcode::DeleteQ),
            (0x15, Opcode::IncrementQ),
            (0x16, Opcode::DecrementQ),
            (0x17, Opcode::QuitQ),
            (0x18, Opcode::FlushQ),
            (0x19, Opcode::AppendQ),
            (0x1a, Opcode::PrependQ),
            (0x1c, Opcode::Touch),
            (0x1d, Opcode::GAT),
            (0x1e, Opcode::GATQ),
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
        assert!(Opcode::SetQ.is_quiet());
        assert!(Opcode::AddQ.is_quiet());
        assert!(Opcode::ReplaceQ.is_quiet());
        assert!(Opcode::DeleteQ.is_quiet());
        assert!(Opcode::IncrementQ.is_quiet());
        assert!(Opcode::DecrementQ.is_quiet());
        assert!(Opcode::QuitQ.is_quiet());
        assert!(Opcode::FlushQ.is_quiet());
        assert!(Opcode::AppendQ.is_quiet());
        assert!(Opcode::PrependQ.is_quiet());
        assert!(Opcode::GATQ.is_quiet());
        assert!(!Opcode::Get.is_quiet());
        assert!(!Opcode::Set.is_quiet());
        assert!(!Opcode::Touch.is_quiet());
        assert!(!Opcode::Append.is_quiet());
        assert!(Opcode::GetK.includes_key());
        assert!(Opcode::GetKQ.includes_key());
        assert!(Opcode::GAT.includes_key());
        assert!(Opcode::GATQ.includes_key());
        assert!(!Opcode::Get.includes_key());
        assert!(!Opcode::Touch.includes_key());
    }

    #[test]
    fn opcode_base() {
        assert_eq!(Opcode::SetQ.base(), Opcode::Set);
        assert_eq!(Opcode::AddQ.base(), Opcode::Add);
        assert_eq!(Opcode::ReplaceQ.base(), Opcode::Replace);
        assert_eq!(Opcode::DeleteQ.base(), Opcode::Delete);
        assert_eq!(Opcode::IncrementQ.base(), Opcode::Increment);
        assert_eq!(Opcode::DecrementQ.base(), Opcode::Decrement);
        assert_eq!(Opcode::QuitQ.base(), Opcode::Quit);
        assert_eq!(Opcode::FlushQ.base(), Opcode::Flush);
        assert_eq!(Opcode::AppendQ.base(), Opcode::Append);
        assert_eq!(Opcode::PrependQ.base(), Opcode::Prepend);
        assert_eq!(Opcode::GATQ.base(), Opcode::GAT);
        assert_eq!(Opcode::Get.base(), Opcode::Get);
        assert_eq!(Opcode::Noop.base(), Opcode::Noop);
        assert_eq!(Opcode::Touch.base(), Opcode::Touch);
        assert_eq!(Opcode::GAT.base(), Opcode::GAT);
    }

    #[test]
    fn header_parse_valid() {
        let frame = build_request_frame(Opcode::Get, 42, 0, &[], b"mykey", &[]);
        let hdr = Header::parse(&frame).expect("should parse");
        assert_eq!(hdr.opcode, Some(Opcode::Get));
        assert_eq!(hdr.opcode_byte, 0x00);
        assert_eq!(hdr.key_len, 5);
        assert_eq!(hdr.extras_len, 0);
        assert_eq!(hdr.body_len, 5);
        assert_eq!(hdr.opaque, 42);
        assert_eq!(hdr.cas, 0);
    }

    #[test]
    fn header_parse_unknown_opcode() {
        let frame = build_raw_request_frame(0xFE, 99, 0, &[], b"key", &[]);
        let hdr = Header::parse(&frame).expect("should parse even unknown opcode");
        assert_eq!(hdr.opcode, None);
        assert_eq!(hdr.opcode_byte, 0xFE);
        assert_eq!(hdr.opaque, 99);
    }

    #[test]
    fn header_rejects_short_buffer() {
        assert!(Header::parse(&[0x80; 10]).is_none());
    }

    #[test]
    fn header_parses_any_magic() {
        // Header::parse no longer rejects wrong magic — that is
        // try_parse_request's job.
        let mut frame = build_request_frame(Opcode::Noop, 0, 0, &[], &[], &[]);
        frame[0] = 0x00;
        let hdr = Header::parse(&frame);
        assert!(hdr.is_some());
    }

    // ── try_parse_request tests ─────────────────────────────────────

    #[test]
    fn try_parse_empty_is_incomplete() {
        assert!(matches!(try_parse_request(&[]), ParseResult::Incomplete));
    }

    #[test]
    fn try_parse_bad_magic() {
        let mut frame = build_request_frame(Opcode::Get, 0, 0, &[], b"k", &[]);
        frame[0] = 0x42;
        assert!(matches!(try_parse_request(&frame), ParseResult::BadMagic));
    }

    #[test]
    fn try_parse_incomplete_header() {
        assert!(matches!(
            try_parse_request(&[MAGIC_REQ, 0x00, 0x00]),
            ParseResult::Incomplete,
        ));
    }

    #[test]
    fn try_parse_incomplete_body() {
        let frame = build_request_frame(Opcode::Get, 0, 0, &[], b"key", &[]);
        assert!(matches!(
            try_parse_request(&frame[..frame.len() - 1]),
            ParseResult::Incomplete,
        ));
    }

    #[test]
    fn try_parse_malformed_frame() {
        // Manually craft a frame with inconsistent lengths:
        // body_len=2, extras_len=1, key_len=2 → 1+2=3 > 2.
        let mut bad = vec![0u8; HEADER_LEN + 2];
        bad[0] = MAGIC_REQ;
        bad[1] = 0x00; // Get
        BigEndian::write_u16(&mut bad[2..4], 2); // key_len=2
        bad[4] = 1; // extras_len=1
        BigEndian::write_u32(&mut bad[8..12], 2); // body_len=2
        BigEndian::write_u32(&mut bad[12..16], 77); // opaque
        match try_parse_request(&bad) {
            ParseResult::MalformedFrame { opaque, opcode_byte, bytes_to_skip } => {
                assert_eq!(opaque, 77);
                assert_eq!(opcode_byte, 0x00);
                assert_eq!(bytes_to_skip, HEADER_LEN + 2);
            }
            other => panic!("expected MalformedFrame, got {other:?}"),
        }
    }

    #[test]
    fn try_parse_unknown_opcode_still_parses() {
        let frame = build_raw_request_frame(0xFE, 42, 0, &[], b"k", &[]);
        match try_parse_request(&frame) {
            ParseResult::Ok((req, consumed)) => {
                assert_eq!(consumed, HEADER_LEN + 1);
                assert!(req.hdr.opcode.is_none());
                assert_eq!(req.hdr.opcode_byte, 0xFE);
                assert_eq!(req.hdr.opaque, 42);
                assert_eq!(req.key, b"k");
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    // ── Legacy parse_request still works ────────────────────────────

    #[test]
    fn parse_request_get() {
        let frame = build_request_frame(Opcode::Get, 7, 0, &[], b"hello", &[]);
        let (req, consumed) = parse_request(&frame).expect("should parse");
        assert_eq!(consumed, HEADER_LEN + 5);
        assert_eq!(req.hdr.opcode, Some(Opcode::Get));
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
        assert_eq!(req1.hdr.opcode, Some(Opcode::Noop));
        assert_eq!(req1.hdr.opaque, 1);

        let (req2, c2) = parse_request(&buf[c1..]).expect("second");
        assert_eq!(req2.hdr.opcode, Some(Opcode::Quit));
        assert_eq!(req2.hdr.opaque, 2);
        assert_eq!(c1 + c2, buf.len());
    }

    // ── Response writing tests ──────────────────────────────────────

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

    #[test]
    fn write_error_for_unknown_opcode() {
        let mut out = Vec::new();
        let msg = b"Unknown command";
        write_error_for_raw_opcode(&mut out, 0xFE, ST_UNK, 42, msg)
            .expect("write");
        assert_eq!(out[0], MAGIC_RES);
        assert_eq!(out[1], 0xFE);
        assert_eq!(BigEndian::read_u16(&out[6..8]), ST_UNK);
        assert_eq!(BigEndian::read_u32(&out[12..16]), 42);
        assert_eq!(BigEndian::read_u64(&out[16..24]), CAS_ZERO);
        assert_eq!(&out[HEADER_LEN..], msg);
    }
}
