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
pub const ST_AUTH_ERROR: u16 = 0x0020;
pub const ST_AUTH_CONTINUE: u16 = 0x0021;
pub const ST_UNK: u16 = 0x0081;

// ── Size limits ─────────────────────────────────────────────────────
/// Maximum allowed body length per frame.  Memcached's default
/// `item_size_max` is 1 MiB; we allow up to 20 MiB to be generous
/// while still preventing multi-gigabyte allocations from a single
/// malicious or buggy frame.
pub const MAX_BODY_LEN: u32 = 20 * 1024 * 1024; // 20 MiB

/// Maximum allowed key length.  The memcached binary protocol uses a
/// u16 for key_len (max 65535), but the traditional memcached limit is
/// 250 bytes.  We enforce the 250-byte limit for compatibility.
pub const MAX_KEY_LEN: u16 = 250;

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
    Stat = 0x10,
    Verbosity = 0x1b,
    Touch = 0x1c,
    GAT = 0x1d,
    GATQ = 0x1e,
    SaslListMechs = 0x20,
    SaslAuth = 0x21,
    SaslStep = 0x22,
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
            0x10 => Stat,
            0x1b => Verbosity,
            0x1c => Touch,
            0x1d => GAT,
            0x1e => GATQ,
            0x20 => SaslListMechs,
            0x21 => SaslAuth,
            0x22 => SaslStep,
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
    /// The frame header declares a body_len that exceeds MAX_BODY_LEN
    /// or a key_len that exceeds MAX_KEY_LEN.  The connection should be
    /// closed because we cannot safely skip past a potentially huge body
    /// without reading and discarding it.
    OversizedFrame {
        opaque: u32,
        opcode_byte: u8,
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
    // Reject oversized frames before attempting to buffer them.
    if hdr.body_len > MAX_BODY_LEN || hdr.key_len > MAX_KEY_LEN {
        return ParseResult::OversizedFrame {
            opaque: hdr.opaque,
            opcode_byte: hdr.opcode_byte,
        };
    }
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
#[allow(clippy::too_many_arguments)]
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
#[allow(clippy::too_many_arguments)]
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
            (0x10, Opcode::Stat),
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
            (0x1b, Opcode::Verbosity),
            (0x1c, Opcode::Touch),
            (0x1d, Opcode::GAT),
            (0x1e, Opcode::GATQ),
            (0x20, Opcode::SaslListMechs),
            (0x21, Opcode::SaslAuth),
            (0x22, Opcode::SaslStep),
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
        assert!(!Opcode::Stat.is_quiet());
        assert!(!Opcode::Verbosity.is_quiet());
        assert!(!Opcode::SaslListMechs.is_quiet());
        assert!(!Opcode::SaslAuth.is_quiet());
        assert!(!Opcode::SaslStep.is_quiet());
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
        assert_eq!(Opcode::Stat.base(), Opcode::Stat);
        assert_eq!(Opcode::Verbosity.base(), Opcode::Verbosity);
        assert_eq!(Opcode::SaslListMechs.base(), Opcode::SaslListMechs);
        assert_eq!(Opcode::SaslAuth.base(), Opcode::SaslAuth);
        assert_eq!(Opcode::SaslStep.base(), Opcode::SaslStep);
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
    fn try_parse_oversized_body() {
        // Craft a header that claims body_len > MAX_BODY_LEN.
        let mut frame = vec![0u8; HEADER_LEN];
        frame[0] = MAGIC_REQ;
        frame[1] = 0x01; // Set
        BigEndian::write_u32(&mut frame[8..12], MAX_BODY_LEN + 1);
        BigEndian::write_u32(&mut frame[12..16], 55); // opaque
        match try_parse_request(&frame) {
            ParseResult::OversizedFrame { opaque, opcode_byte } => {
                assert_eq!(opaque, 55);
                assert_eq!(opcode_byte, 0x01);
            }
            other => panic!("expected OversizedFrame, got {other:?}"),
        }
    }

    #[test]
    fn try_parse_oversized_key() {
        // Craft a header with key_len > MAX_KEY_LEN.
        let key_len = MAX_KEY_LEN + 1;
        let body_len = key_len as u32;
        let mut frame = vec![0u8; HEADER_LEN + body_len as usize];
        frame[0] = MAGIC_REQ;
        frame[1] = 0x00; // Get
        BigEndian::write_u16(&mut frame[2..4], key_len);
        BigEndian::write_u32(&mut frame[8..12], body_len);
        BigEndian::write_u32(&mut frame[12..16], 66);
        match try_parse_request(&frame) {
            ParseResult::OversizedFrame { opaque, opcode_byte } => {
                assert_eq!(opaque, 66);
                assert_eq!(opcode_byte, 0x00);
            }
            other => panic!("expected OversizedFrame, got {other:?}"),
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

    // ================================================================
    // Regression tests — previously fixed wire-visible behaviors
    // ================================================================

    /// Regression: BadMagic must be detected on the very first byte,
    /// even when the rest of the buffer looks like a valid frame.
    #[test]
    fn regression_bad_magic_with_valid_body() {
        let mut frame = build_request_frame(Opcode::Get, 1, 0, &[], b"key", &[]);
        // Corrupt only the magic byte.
        frame[0] = 0x81; // response magic, not request
        assert!(matches!(try_parse_request(&frame), ParseResult::BadMagic));
        frame[0] = 0x00;
        assert!(matches!(try_parse_request(&frame), ParseResult::BadMagic));
    }

    /// Regression: MalformedFrame when extras_len + key_len > body_len.
    /// The parser must return MalformedFrame rather than panicking on
    /// slice bounds.
    #[test]
    fn regression_malformed_extras_key_overflow() {
        // extras_len=4, key_len=4, body_len=6 → 4+4=8 > 6
        let mut frame = vec![0u8; HEADER_LEN + 6];
        frame[0] = MAGIC_REQ;
        frame[1] = Opcode::Set as u8;
        BigEndian::write_u16(&mut frame[2..4], 4); // key_len
        frame[4] = 4; // extras_len
        BigEndian::write_u32(&mut frame[8..12], 6); // body_len
        BigEndian::write_u32(&mut frame[12..16], 123);
        match try_parse_request(&frame) {
            ParseResult::MalformedFrame { opaque, bytes_to_skip, .. } => {
                assert_eq!(opaque, 123);
                assert_eq!(bytes_to_skip, HEADER_LEN + 6);
            }
            other => panic!("expected MalformedFrame, got {other:?}"),
        }
    }

    /// Regression: OversizedFrame at the exact boundary (MAX_BODY_LEN + 1)
    /// and exactly at MAX_BODY_LEN (should be accepted, not rejected).
    #[test]
    fn regression_oversized_at_exact_boundary() {
        // body_len == MAX_BODY_LEN should be accepted
        let mut ok_frame = vec![0u8; HEADER_LEN];
        ok_frame[0] = MAGIC_REQ;
        ok_frame[1] = Opcode::Set as u8;
        BigEndian::write_u32(&mut ok_frame[8..12], MAX_BODY_LEN);
        // Not enough bytes to complete the frame, so we get Incomplete
        assert!(matches!(try_parse_request(&ok_frame), ParseResult::Incomplete));

        // body_len == MAX_BODY_LEN + 1 → OversizedFrame
        let mut bad_frame = vec![0u8; HEADER_LEN];
        bad_frame[0] = MAGIC_REQ;
        bad_frame[1] = Opcode::Set as u8;
        BigEndian::write_u32(&mut bad_frame[8..12], MAX_BODY_LEN + 1);
        assert!(matches!(try_parse_request(&bad_frame), ParseResult::OversizedFrame { .. }));
    }

    /// Regression: key_len == MAX_KEY_LEN (250) should be accepted;
    /// key_len == 251 should be OversizedFrame.
    #[test]
    fn regression_key_len_at_exact_boundary() {
        // key_len = 250, body_len = 250 — valid but incomplete
        let mut ok_hdr = vec![0u8; HEADER_LEN];
        ok_hdr[0] = MAGIC_REQ;
        ok_hdr[1] = Opcode::Get as u8;
        BigEndian::write_u16(&mut ok_hdr[2..4], MAX_KEY_LEN); // 250
        BigEndian::write_u32(&mut ok_hdr[8..12], MAX_KEY_LEN as u32);
        assert!(matches!(try_parse_request(&ok_hdr), ParseResult::Incomplete));

        // key_len = 251 → OversizedFrame
        let mut bad_hdr = vec![0u8; HEADER_LEN];
        bad_hdr[0] = MAGIC_REQ;
        bad_hdr[1] = Opcode::Get as u8;
        BigEndian::write_u16(&mut bad_hdr[2..4], MAX_KEY_LEN + 1);
        BigEndian::write_u32(&mut bad_hdr[8..12], (MAX_KEY_LEN + 1) as u32);
        assert!(matches!(try_parse_request(&bad_hdr), ParseResult::OversizedFrame { .. }));
    }

    /// Regression: DELETE success CAS — the response builder must put
    /// a non-zero CAS in the response for successful delete.  This is a
    /// wire-format invariant: field offset [16..24] in the response.
    #[test]
    fn regression_delete_success_cas_nonzero_in_response() {
        let mut out = Vec::new();
        let delete_cas: u64 = 42;
        write_simple_response(&mut out, Opcode::Delete, ST_OK, 1, delete_cas, &[])
            .expect("write");
        let response_cas = BigEndian::read_u64(&out[16..24]);
        assert_ne!(response_cas, 0, "DELETE success must carry a non-zero CAS");
        assert_eq!(response_cas, 42);
    }

    /// Regression: unknown opcode should still parse successfully and
    /// preserve the raw opcode byte for error echoing.
    #[test]
    fn regression_unknown_opcode_preserves_raw_byte() {
        for raw_byte in [0x30, 0x7F, 0xAA, 0xFF] {
            let frame = build_raw_request_frame(raw_byte, 99, 0, &[], b"k", &[]);
            match try_parse_request(&frame) {
                ParseResult::Ok((req, _)) => {
                    assert!(req.hdr.opcode.is_none());
                    assert_eq!(req.hdr.opcode_byte, raw_byte);
                }
                other => panic!("byte 0x{raw_byte:02x}: expected Ok, got {other:?}"),
            }
        }
    }

    // ================================================================
    // Binary-safe value and key edge cases
    // ================================================================

    /// Null bytes in keys and values must survive round-trip.
    #[test]
    fn binary_safe_null_bytes_in_key_and_value() {
        let key = b"key\x00with\x00nulls";
        let value = b"\x00\x00\x00";
        let frame = build_request_frame(Opcode::Set, 1, 0, &[0u8; 8], key, value);
        let (req, consumed) = parse_request(&frame).expect("should parse");
        assert_eq!(consumed, HEADER_LEN + 8 + key.len() + value.len());
        assert_eq!(req.key, key);
        assert_eq!(req.value, value);
    }

    /// High bytes (0xFF) in keys and values.
    #[test]
    fn binary_safe_high_bytes() {
        let key = b"\xff\xfe\xfd";
        let value = b"\xff\xff\xff\xff";
        let frame = build_request_frame(Opcode::Set, 1, 0, &[0u8; 8], key, value);
        let (req, _) = parse_request(&frame).expect("should parse");
        assert_eq!(req.key, key);
        assert_eq!(req.value, value);
    }

    /// Empty key and empty value — valid for NOOP-like opcodes.
    #[test]
    fn binary_safe_empty_key_and_value() {
        let frame = build_request_frame(Opcode::Noop, 1, 0, &[], &[], &[]);
        let (req, consumed) = parse_request(&frame).expect("should parse");
        assert_eq!(consumed, HEADER_LEN);
        assert!(req.key.is_empty());
        assert!(req.value.is_empty());
        assert!(req.extras.is_empty());
    }

    /// Maximum-length key (250 bytes) must be accepted.
    #[test]
    fn binary_safe_max_key_length() {
        let key = vec![b'A'; MAX_KEY_LEN as usize];
        let frame = build_request_frame(Opcode::Get, 1, 0, &[], &key, &[]);
        let (req, _) = parse_request(&frame).expect("should parse");
        assert_eq!(req.key.len(), 250);
    }

    /// Value with all 256 byte values present.
    #[test]
    fn binary_safe_all_byte_values() {
        let value: Vec<u8> = (0..=255u8).collect();
        let frame = build_request_frame(Opcode::Set, 1, 0, &[0u8; 8], b"k", &value);
        let (req, _) = parse_request(&frame).expect("should parse");
        assert_eq!(req.value.len(), 256);
        assert_eq!(req.value, value.as_slice());
    }

    // ================================================================
    // Response builder round-trip and invariant tests
    // ================================================================

    /// Every response must start with MAGIC_RES (0x81).
    #[test]
    fn response_magic_byte() {
        for opcode in [Opcode::Get, Opcode::Set, Opcode::Noop, Opcode::Quit] {
            let mut out = Vec::new();
            write_simple_response(&mut out, opcode, ST_OK, 0, 0, &[]).unwrap();
            assert_eq!(out[0], MAGIC_RES, "opcode {opcode:?} response must start with 0x81");
        }
    }

    /// Error responses must carry CAS_ZERO.
    #[test]
    fn response_error_carries_cas_zero() {
        for status in [ST_NF, ST_IX, ST_ARGS, ST_NOT_STORED, ST_UNK] {
            let mut out = Vec::new();
            write_simple_response(&mut out, Opcode::Get, status, 0, CAS_ZERO, &[]).unwrap();
            let cas = BigEndian::read_u64(&out[16..24]);
            assert_eq!(cas, CAS_ZERO, "status 0x{status:04x} must carry CAS_ZERO");
        }
    }

    /// Response body_len field must equal extras.len() + key.len() + value.len().
    #[test]
    fn response_body_len_field_correct() {
        let extras = [1u8, 2, 3, 4];
        let key = b"mykey";
        let value = b"myvalue";
        let mut out = Vec::new();
        write_response(&mut out, Opcode::GetK, ST_OK, 0, 1, &extras, key, value).unwrap();
        let body_len = BigEndian::read_u32(&out[8..12]);
        assert_eq!(body_len as usize, extras.len() + key.len() + value.len());
    }

    /// Response opaque must echo the request opaque.
    #[test]
    fn response_opaque_echo() {
        for opaque in [0u32, 1, 0x12345678, u32::MAX] {
            let mut out = Vec::new();
            write_simple_response(&mut out, Opcode::Noop, ST_OK, opaque, 0, &[]).unwrap();
            assert_eq!(BigEndian::read_u32(&out[12..16]), opaque);
        }
    }

    /// Response key_len field must match actual key length.
    #[test]
    fn response_key_len_field_correct() {
        let key = b"testkey";
        let mut out = Vec::new();
        write_response(&mut out, Opcode::GetK, ST_OK, 0, 1, &[0u8; 4], key, b"val").unwrap();
        let key_len = BigEndian::read_u16(&out[2..4]);
        assert_eq!(key_len as usize, key.len());
    }

    /// Response extras_len field must match actual extras length.
    #[test]
    fn response_extras_len_field_correct() {
        let extras = [0u8; 4];
        let mut out = Vec::new();
        write_response(&mut out, Opcode::Get, ST_OK, 0, 1, &extras, &[], b"val").unwrap();
        assert_eq!(out[4], 4);
    }

    /// write_raw_response echoes the exact opcode byte provided.
    #[test]
    fn response_raw_opcode_echo() {
        for raw in [0x00u8, 0xFE, 0xFF, 0x42] {
            let mut out = Vec::new();
            write_raw_response(&mut out, raw, ST_OK, 0, 0, &[], &[], &[]).unwrap();
            assert_eq!(out[1], raw);
        }
    }

    // ================================================================
    // Property-style framing checks
    // ================================================================

    /// For any valid opcode with any key/value/extras, consumed bytes
    /// must equal HEADER_LEN + body_len.
    #[test]
    fn property_consumed_equals_header_plus_body() {
        let opcodes = [
            Opcode::Get, Opcode::Set, Opcode::Delete, Opcode::Noop,
            Opcode::Increment, Opcode::Append, Opcode::Touch, Opcode::GAT,
            Opcode::Flush, Opcode::Version, Opcode::Stat,
        ];
        let extras_sizes = [0, 4, 8, 20];
        let key_sizes = [0, 1, 5, 50];
        let value_sizes = [0, 1, 10, 100];

        for &op in &opcodes {
            for &elen in &extras_sizes {
                for &klen in &key_sizes {
                    for &vlen in &value_sizes {
                        let extras = vec![0u8; elen];
                        let key = vec![b'k'; klen];
                        let value = vec![b'v'; vlen];
                        let frame = build_request_frame(op, 0, 0, &extras, &key, &value);

                        let expected_body = elen + klen + vlen;
                        let expected_total = HEADER_LEN + expected_body;
                        assert_eq!(frame.len(), expected_total,
                            "opcode={op:?} e={elen} k={klen} v={vlen}");

                        match try_parse_request(&frame) {
                            ParseResult::Ok((req, consumed)) => {
                                assert_eq!(consumed, expected_total,
                                    "opcode={op:?} consumed mismatch");
                                assert_eq!(req.extras.len(), elen);
                                assert_eq!(req.key.len(), klen);
                                assert_eq!(req.value.len(), vlen);
                            }
                            other => panic!("opcode={op:?} e={elen} k={klen} v={vlen}: {other:?}"),
                        }
                    }
                }
            }
        }
    }

    /// Extras + key must never exceed body_len — the builder enforces
    /// this invariant.  Verify that build_request_frame always produces
    /// parseable frames.
    #[test]
    fn property_build_always_parseable() {
        for op_byte in 0..=0x22u8 {
            let op = match Opcode::parse(op_byte) {
                Some(o) => o,
                None => continue,
            };
            let frame = build_request_frame(op, 0xDEAD, 0x1234, &[1, 2], b"abc", b"xyz");
            match try_parse_request(&frame) {
                ParseResult::Ok((req, consumed)) => {
                    assert_eq!(consumed, HEADER_LEN + 2 + 3 + 3);
                    assert_eq!(req.hdr.opaque, 0xDEAD);
                    assert_eq!(req.hdr.cas, 0x1234);
                    assert_eq!(req.extras, &[1, 2]);
                    assert_eq!(req.key, b"abc");
                    assert_eq!(req.value, b"xyz");
                }
                other => panic!("opcode {op:?}: expected Ok, got {other:?}"),
            }
        }
    }

    /// Multi-frame extraction: parsing two concatenated frames must
    /// yield both correctly and consume the entire buffer.
    #[test]
    fn property_multi_frame_extraction() {
        let f1 = build_request_frame(Opcode::Set, 1, 100, &[0u8; 8], b"k1", b"v1");
        let f2 = build_request_frame(Opcode::Get, 2, 0, &[], b"k2", &[]);
        let mut buf = f1.clone();
        buf.extend_from_slice(&f2);

        let (r1, c1) = parse_request(&buf).expect("first frame");
        assert_eq!(r1.hdr.opaque, 1);
        assert_eq!(r1.hdr.cas, 100);
        assert_eq!(r1.key, b"k1");
        assert_eq!(r1.value, b"v1");

        let (r2, c2) = parse_request(&buf[c1..]).expect("second frame");
        assert_eq!(r2.hdr.opaque, 2);
        assert_eq!(r2.key, b"k2");

        assert_eq!(c1 + c2, buf.len(), "total consumed must equal buffer length");
    }

    /// Trailing bytes after a valid frame don't affect parsing of
    /// the first frame — they remain as residual for the next parse.
    #[test]
    fn property_trailing_bytes_ignored() {
        let frame = build_request_frame(Opcode::Noop, 7, 0, &[], &[], &[]);
        let mut buf = frame.clone();
        buf.extend_from_slice(&[0xDE, 0xAD]); // trailing garbage

        let (req, consumed) = parse_request(&buf).expect("should parse first frame");
        assert_eq!(consumed, HEADER_LEN);
        assert_eq!(req.hdr.opaque, 7);
        // Remaining 2 bytes are not consumed.
        assert_eq!(buf.len() - consumed, 2);
    }

    // ================================================================
    // Counter / numeric precision limitation tests
    // ================================================================

    /// The LUA_COUNTER script documentation states that counter values
    /// are exact for [0, 2^53).  This test verifies the Rust-side u64
    /// parse round-trip for values near the precision boundary.
    ///
    /// Precision note: Lua 5.1 uses IEEE 754 doubles.  Integers above
    /// 2^53 (9007199254740992) may lose precision and round.  This is
    /// NOT wraparound — it is floating-point precision loss.  The GA
    /// explicitly documents this as a known limitation.
    #[test]
    fn counter_u64_parse_exact_below_2_53() {
        // Values below 2^53 should round-trip exactly through
        // string formatting.
        let exact_values: &[u64] = &[
            0,
            1,
            u32::MAX as u64,
            (1u64 << 53) - 1, // 9007199254740991 — largest exact integer
        ];
        for &val in exact_values {
            let s = format!("{val}");
            let parsed: u64 = s.parse().expect("should parse");
            assert_eq!(parsed, val, "value {val} must round-trip exactly");
        }
    }

    /// Values at or above 2^53 may lose precision when processed
    /// through Lua's f64 representation.  This test documents the
    /// precision loss behavior (NOT wraparound).
    #[test]
    fn counter_precision_loss_above_2_53() {
        let boundary = 1u64 << 53; // 9007199254740992
        // At exactly 2^53, f64 can still represent it.
        let f = boundary as f64;
        assert_eq!(f as u64, boundary, "2^53 itself is exact in f64");

        // 2^53 + 1 loses precision in f64.
        let above = boundary + 1;
        let f_above = above as f64;
        // This is the precision loss: f64 rounds 2^53+1 back to 2^53.
        assert_ne!(f_above as u64, above,
            "2^53+1 should NOT round-trip exactly through f64 — \
             this is precision loss, not wraparound");
        assert_eq!(f_above as u64, boundary,
            "2^53+1 rounds to 2^53 in f64 — precision loss");
    }

    /// The string format "%.0f" used by the Lua counter script
    /// produces precision-lossy (rounded) output for large values,
    /// not wrapped values.
    #[test]
    fn counter_lua_format_is_rounding_not_wraparound() {
        // Simulating what Lua's string.format('%.0f', num) does:
        // it converts the f64 to a string with no decimal places.
        let val: u64 = (1u64 << 53) + 1;
        let as_f64 = val as f64;
        let formatted = format!("{:.0}", as_f64);
        let back: u64 = formatted.parse().unwrap();
        // The result is 2^53, not some wrapped value.
        assert_eq!(back, 1u64 << 53);
        // Crucially, it's NOT 0 or some negative number — it's a
        // nearby value, demonstrating precision loss not wraparound.
        assert!(back > 0, "large counter values round, they don't wrap to zero");
    }

    /// Counter response is an 8-byte big-endian u64 in the value field.
    /// Verify the encoding for representative values.
    #[test]
    fn counter_response_encoding() {
        let test_values: &[u64] = &[0, 1, 255, 256, u32::MAX as u64, u64::MAX];
        for &val in test_values {
            let encoded = val.to_be_bytes();
            assert_eq!(encoded.len(), 8);
            let decoded = u64::from_be_bytes(encoded);
            assert_eq!(decoded, val);
        }
    }

    // ================================================================
    // Additional edge-case tests
    // ================================================================

    /// A single 0x80 byte (just the magic) should be Incomplete,
    /// not a parse error.
    #[test]
    fn edge_single_magic_byte_is_incomplete() {
        assert!(matches!(try_parse_request(&[MAGIC_REQ]), ParseResult::Incomplete));
    }

    /// 23 bytes (one short of a header) should be Incomplete.
    #[test]
    fn edge_one_byte_short_of_header() {
        let mut buf = vec![MAGIC_REQ; 23];
        buf[0] = MAGIC_REQ;
        assert!(matches!(try_parse_request(&buf), ParseResult::Incomplete));
    }

    /// Exactly HEADER_LEN bytes with body_len=0 should parse as a
    /// complete frame (e.g. NOOP).
    #[test]
    fn edge_exact_header_zero_body() {
        let frame = build_request_frame(Opcode::Noop, 0, 0, &[], &[], &[]);
        assert_eq!(frame.len(), HEADER_LEN);
        match try_parse_request(&frame) {
            ParseResult::Ok((req, consumed)) => {
                assert_eq!(consumed, HEADER_LEN);
                assert_eq!(req.hdr.body_len, 0);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    /// CAS field in request header must be preserved exactly.
    #[test]
    fn edge_cas_preserved_in_header() {
        let cas_values: &[u64] = &[0, 1, u64::MAX, 0xDEADBEEFCAFEBABE];
        for &cas in cas_values {
            let frame = build_request_frame(Opcode::Set, 0, cas, &[0u8; 8], b"k", b"v");
            let (req, _) = parse_request(&frame).expect("should parse");
            assert_eq!(req.hdr.cas, cas, "CAS 0x{cas:016x} must be preserved");
        }
    }

    /// Opaque field must be preserved for all u32 values.
    #[test]
    fn edge_opaque_preserved() {
        for opaque in [0u32, 1, u32::MAX, 0x12345678] {
            let frame = build_request_frame(Opcode::Get, opaque, 0, &[], b"k", &[]);
            let (req, _) = parse_request(&frame).expect("should parse");
            assert_eq!(req.hdr.opaque, opaque);
        }
    }

    /// Verify that the response status field is at bytes [6..8].
    #[test]
    fn response_status_field_position() {
        let statuses = [ST_OK, ST_NF, ST_IX, ST_ARGS, ST_NOT_STORED, ST_AUTH_ERROR, ST_AUTH_CONTINUE, ST_UNK];
        for &status in &statuses {
            let mut out = Vec::new();
            write_simple_response(&mut out, Opcode::Get, status, 0, 0, &[]).unwrap();
            assert_eq!(BigEndian::read_u16(&out[6..8]), status,
                "status 0x{status:04x} at wrong position");
        }
    }

    /// Quiet variants of GET suppress miss responses (not hits).
    /// Quiet variants of mutations suppress success responses (not errors).
    /// This verifies the is_quiet classification is consistent with base().
    #[test]
    fn quiet_variants_have_loud_base() {
        let quiet_loud_pairs = [
            (Opcode::GetQ, Opcode::Get),
            (Opcode::GetKQ, Opcode::GetK),
            (Opcode::SetQ, Opcode::Set),
            (Opcode::AddQ, Opcode::Add),
            (Opcode::ReplaceQ, Opcode::Replace),
            (Opcode::DeleteQ, Opcode::Delete),
            (Opcode::IncrementQ, Opcode::Increment),
            (Opcode::DecrementQ, Opcode::Decrement),
            (Opcode::QuitQ, Opcode::Quit),
            (Opcode::FlushQ, Opcode::Flush),
            (Opcode::AppendQ, Opcode::Append),
            (Opcode::PrependQ, Opcode::Prepend),
            (Opcode::GATQ, Opcode::GAT),
        ];
        for (quiet, loud) in quiet_loud_pairs {
            assert!(quiet.is_quiet(), "{quiet:?} must be quiet");
            assert!(!loud.is_quiet(), "{loud:?} must not be quiet");
            assert_eq!(quiet.base(), loud, "{quiet:?}.base() must be {loud:?}");
            assert_eq!(loud.base(), loud, "{loud:?}.base() must be self");
        }
    }

    /// Verify that no opcode byte in the valid range 0x00..=0x22 produces
    /// a gap — every valid byte maps to Some(opcode).
    #[test]
    fn opcode_coverage_no_gaps_in_valid_range() {
        let valid_bytes: Vec<u8> = vec![
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
            0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
            0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
            0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
            0x20, 0x21, 0x22,
        ];
        for b in valid_bytes {
            assert!(Opcode::parse(b).is_some(),
                "byte 0x{b:02x} must map to a known opcode");
        }
        // Gap at 0x1f must be None.
        assert!(Opcode::parse(0x1f).is_none(), "0x1f must be unknown");
    }

    /// Verify that response CAS field is at bytes [16..24] and can
    /// hold the full u64 range.
    #[test]
    fn response_cas_field_full_range() {
        for cas in [0u64, 1, u64::MAX, 0xCAFEBABEDEADBEEF] {
            let mut out = Vec::new();
            write_simple_response(&mut out, Opcode::Get, ST_OK, 0, cas, &[]).unwrap();
            assert_eq!(BigEndian::read_u64(&out[16..24]), cas);
        }
    }

    /// Verify build_raw_request_frame with opcode byte 0x00 produces the
    /// same result as build_request_frame with Opcode::Get.
    #[test]
    fn build_raw_matches_build_typed() {
        let typed = build_request_frame(Opcode::Get, 42, 100, &[1, 2], b"key", b"val");
        let raw = build_raw_request_frame(0x00, 42, 100, &[1, 2], b"key", b"val");
        assert_eq!(typed, raw);
    }
}
