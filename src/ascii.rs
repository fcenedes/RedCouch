//! ASCII (text) protocol handler for the memcached text protocol.
//!
//! Implements all 19 supported ASCII commands.  Entered from
//! `handle_conn()` in `lib.rs` after first-byte protocol detection.

// ── Constants ────────────────────────────────────────────────────────

/// Maximum command line length (bytes before \r\n).
#[cfg(not(test))]
const MAX_LINE_LEN: usize = 2048;

/// Maximum key length (same as binary protocol).
const MAX_KEY_LEN: usize = 250;

/// Version string returned by the `version` command.
#[cfg(not(test))]
const VERSION_STRING: &str = "VERSION RedCouch 0.1.0";

// ── Pure parser types ───────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum AsciiCmd<'a> {
    Store { cmd: StoreOp, key: &'a [u8], flags: u32, exptime: u32, bytes: u32, noreply: bool },
    Cas { key: &'a [u8], flags: u32, exptime: u32, bytes: u32, cas_unique: u64, noreply: bool },
    AppendPrepend { is_prepend: bool, key: &'a [u8], bytes: u32, noreply: bool },
    Retrieval { cmd: RetrievalOp, exptime: Option<u32>, keys: Vec<&'a [u8]> },
    Delete { key: &'a [u8], noreply: bool },
    Counter { is_decr: bool, key: &'a [u8], value: u64, noreply: bool },
    Touch { key: &'a [u8], exptime: u32, noreply: bool },
    FlushAll { _delay: u32, noreply: bool },
    Version,
    Stats { args: Option<&'a str> },
    Verbosity { noreply: bool },
    Quit,
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum StoreOp { Set, Add, Replace }

#[derive(Debug, PartialEq, Clone, Copy)]
enum RetrievalOp { Get, Gets, Gat, Gats }

#[derive(Debug)]
enum CmdParseResult<'a> {
    Ok(AsciiCmd<'a>),
    UnknownCommand,
    ClientError(String),
}

// ── Line extraction ─────────────────────────────────────────────────

/// Find line end in buffer. Returns `(line_end_idx, skip_past_terminator)`.
/// Accepts both `\r\n` and bare `\n`.
fn find_line_end(buf: &[u8]) -> Option<(usize, usize)> {
    for i in 0..buf.len() {
        if buf[i] == b'\n' {
            if i > 0 && buf[i - 1] == b'\r' {
                return Some((i - 1, i + 1));
            }
            return Some((i, i + 1));
        }
    }
    None
}

// ── Validation / parsing helpers ────────────────────────────────────

pub(crate) fn validate_key(key: &[u8]) -> Result<(), String> {
    if key.is_empty() || key.len() > MAX_KEY_LEN {
        return Err("bad command line format".into());
    }
    for &b in key {
        if b <= 0x20 || b == 0x7F {
            return Err("bad command line format".into());
        }
    }
    Ok(())
}

fn parse_u32(s: &str) -> Result<u32, String> {
    s.parse::<u32>().map_err(|_| "bad command line format".to_string())
}

fn parse_u64(s: &str) -> Result<u64, String> {
    s.parse::<u64>().map_err(|_| "bad command line format".to_string())
}

// ── Individual command parsers ──────────────────────────────────────

/// set/add/replace <key> <flags> <exptime> <bytes> [noreply]\r\n
fn parse_store_cmd<'a>(cmd_name: &str, args: &[&'a str], _line: &'a [u8]) -> CmdParseResult<'a> {
    // Expected: key flags exptime bytes [noreply]
    if args.len() < 4 || args.len() > 5 {
        return CmdParseResult::ClientError("bad command line format".into());
    }
    let key = args[0].as_bytes();
    if let Err(e) = validate_key(key) { return CmdParseResult::ClientError(e); }
    let flags = match parse_u32(args[1]) { Ok(v) => v, Err(e) => return CmdParseResult::ClientError(e) };
    let exptime = match parse_u32(args[2]) { Ok(v) => v, Err(e) => return CmdParseResult::ClientError(e) };
    let bytes = match parse_u32(args[3]) { Ok(v) => v, Err(e) => return CmdParseResult::ClientError(e) };
    let noreply = args.len() == 5 && args[4] == "noreply";
    if args.len() == 5 && args[4] != "noreply" {
        return CmdParseResult::ClientError("bad command line format".into());
    }
    let cmd = match cmd_name {
        "set" => StoreOp::Set,
        "add" => StoreOp::Add,
        "replace" => StoreOp::Replace,
        _ => unreachable!(),
    };
    CmdParseResult::Ok(AsciiCmd::Store { cmd, key, flags, exptime, bytes, noreply })
}

/// cas <key> <flags> <exptime> <bytes> <cas_unique> [noreply]\r\n
fn parse_cas_cmd<'a>(args: &[&'a str], _line: &'a [u8]) -> CmdParseResult<'a> {
    if args.len() < 5 || args.len() > 6 {
        return CmdParseResult::ClientError("bad command line format".into());
    }
    let key = args[0].as_bytes();
    if let Err(e) = validate_key(key) { return CmdParseResult::ClientError(e); }
    let flags = match parse_u32(args[1]) { Ok(v) => v, Err(e) => return CmdParseResult::ClientError(e) };
    let exptime = match parse_u32(args[2]) { Ok(v) => v, Err(e) => return CmdParseResult::ClientError(e) };
    let bytes = match parse_u32(args[3]) { Ok(v) => v, Err(e) => return CmdParseResult::ClientError(e) };
    let cas_unique = match parse_u64(args[4]) { Ok(v) => v, Err(e) => return CmdParseResult::ClientError(e) };
    let noreply = args.len() == 6 && args[5] == "noreply";
    if args.len() == 6 && args[5] != "noreply" {
        return CmdParseResult::ClientError("bad command line format".into());
    }
    CmdParseResult::Ok(AsciiCmd::Cas { key, flags, exptime, bytes, cas_unique, noreply })
}


/// append/prepend <key> <bytes> [noreply]\r\n
fn parse_append_prepend_cmd<'a>(cmd_name: &str, args: &[&'a str], _line: &'a [u8]) -> CmdParseResult<'a> {
    if args.len() < 2 || args.len() > 3 {
        return CmdParseResult::ClientError("bad command line format".into());
    }
    let key = args[0].as_bytes();
    if let Err(e) = validate_key(key) { return CmdParseResult::ClientError(e); }
    let bytes = match parse_u32(args[1]) { Ok(v) => v, Err(e) => return CmdParseResult::ClientError(e) };
    let noreply = args.len() == 3 && args[2] == "noreply";
    if args.len() == 3 && args[2] != "noreply" {
        return CmdParseResult::ClientError("bad command line format".into());
    }
    let is_prepend = cmd_name == "prepend";
    CmdParseResult::Ok(AsciiCmd::AppendPrepend { is_prepend, key, bytes, noreply })
}

/// get/gets <key>*\r\n
fn parse_retrieval_cmd<'a>(cmd_name: &str, args: &[&'a str], _line: &'a [u8]) -> CmdParseResult<'a> {
    if args.is_empty() {
        return CmdParseResult::ClientError("bad command line format".into());
    }
    let mut keys = Vec::with_capacity(args.len());
    for &k in args {
        let kb = k.as_bytes();
        if let Err(e) = validate_key(kb) { return CmdParseResult::ClientError(e); }
        keys.push(kb);
    }
    let cmd = if cmd_name == "gets" { RetrievalOp::Gets } else { RetrievalOp::Get };
    CmdParseResult::Ok(AsciiCmd::Retrieval { cmd, exptime: None, keys })
}

/// gat/gats <exptime> <key>*\r\n
fn parse_gat_cmd<'a>(cmd_name: &str, args: &[&'a str], _line: &'a [u8]) -> CmdParseResult<'a> {
    if args.len() < 2 {
        return CmdParseResult::ClientError("bad command line format".into());
    }
    let exptime = match parse_u32(args[0]) { Ok(v) => v, Err(e) => return CmdParseResult::ClientError(e) };
    let mut keys = Vec::with_capacity(args.len() - 1);
    for &k in &args[1..] {
        let kb = k.as_bytes();
        if let Err(e) = validate_key(kb) { return CmdParseResult::ClientError(e); }
        keys.push(kb);
    }
    let cmd = if cmd_name == "gats" { RetrievalOp::Gats } else { RetrievalOp::Gat };
    CmdParseResult::Ok(AsciiCmd::Retrieval { cmd, exptime: Some(exptime), keys })
}

/// delete <key> [noreply]\r\n
fn parse_delete_cmd<'a>(args: &[&'a str], _line: &'a [u8]) -> CmdParseResult<'a> {
    if args.is_empty() || args.len() > 2 {
        return CmdParseResult::ClientError("bad command line format".into());
    }
    let key = args[0].as_bytes();
    if let Err(e) = validate_key(key) { return CmdParseResult::ClientError(e); }
    let noreply = args.len() == 2 && args[1] == "noreply";
    if args.len() == 2 && args[1] != "noreply" {
        return CmdParseResult::ClientError("bad command line format".into());
    }
    CmdParseResult::Ok(AsciiCmd::Delete { key, noreply })
}

/// incr/decr <key> <value> [noreply]\r\n
fn parse_counter_cmd<'a>(cmd_name: &str, args: &[&'a str], _line: &'a [u8]) -> CmdParseResult<'a> {
    if args.len() < 2 || args.len() > 3 {
        return CmdParseResult::ClientError("bad command line format".into());
    }
    let key = args[0].as_bytes();
    if let Err(e) = validate_key(key) { return CmdParseResult::ClientError(e); }
    let value = match parse_u64(args[1]) { Ok(v) => v, Err(e) => return CmdParseResult::ClientError(e) };
    let noreply = args.len() == 3 && args[2] == "noreply";
    if args.len() == 3 && args[2] != "noreply" {
        return CmdParseResult::ClientError("bad command line format".into());
    }
    let is_decr = cmd_name == "decr";
    CmdParseResult::Ok(AsciiCmd::Counter { is_decr, key, value, noreply })
}

/// touch <key> <exptime> [noreply]\r\n
fn parse_touch_cmd<'a>(args: &[&'a str], _line: &'a [u8]) -> CmdParseResult<'a> {
    if args.len() < 2 || args.len() > 3 {
        return CmdParseResult::ClientError("bad command line format".into());
    }
    let key = args[0].as_bytes();
    if let Err(e) = validate_key(key) { return CmdParseResult::ClientError(e); }
    let exptime = match parse_u32(args[1]) { Ok(v) => v, Err(e) => return CmdParseResult::ClientError(e) };
    let noreply = args.len() == 3 && args[2] == "noreply";
    if args.len() == 3 && args[2] != "noreply" {
        return CmdParseResult::ClientError("bad command line format".into());
    }
    CmdParseResult::Ok(AsciiCmd::Touch { key, exptime, noreply })
}

/// flush_all [delay] [noreply]\r\n
fn parse_flush_cmd<'a>(args: &[&'a str]) -> CmdParseResult<'a> {
    let mut delay = 0u32;
    let mut noreply = false;
    for &a in args {
        if a == "noreply" {
            noreply = true;
        } else if let Ok(d) = a.parse::<u32>() {
            delay = d;
        } else {
            return CmdParseResult::ClientError("bad command line format".into());
        }
    }
    CmdParseResult::Ok(AsciiCmd::FlushAll { _delay: delay, noreply })
}

/// Top-level command line parser.
fn parse_command_line(line: &[u8]) -> CmdParseResult<'_> {
    let line_str = match std::str::from_utf8(line) {
        Ok(s) => s,
        Err(_) => return CmdParseResult::ClientError("bad command line format".into()),
    };
    let tokens: Vec<&str> = line_str.split_whitespace().collect();
    if tokens.is_empty() {
        return CmdParseResult::UnknownCommand;
    }
    match tokens[0] {
        "set" | "add" | "replace" => parse_store_cmd(tokens[0], &tokens[1..], line),
        "cas" => parse_cas_cmd(&tokens[1..], line),
        "append" | "prepend" => parse_append_prepend_cmd(tokens[0], &tokens[1..], line),
        "get" | "gets" => parse_retrieval_cmd(tokens[0], &tokens[1..], line),
        "gat" | "gats" => parse_gat_cmd(tokens[0], &tokens[1..], line),
        "delete" => parse_delete_cmd(&tokens[1..], line),
        "incr" | "decr" => parse_counter_cmd(tokens[0], &tokens[1..], line),
        "touch" => parse_touch_cmd(&tokens[1..], line),
        "flush_all" => parse_flush_cmd(&tokens[1..]),
        "version" => CmdParseResult::Ok(AsciiCmd::Version),
        "stats" => {
            let args_str = if tokens.len() > 1 {
                // Find start of the args portion in the original line.
                let cmd_end = line_str.find(char::is_whitespace).unwrap_or(line_str.len());
                let rest = line_str[cmd_end..].trim();
                if rest.is_empty() { None } else { Some(rest) }
            } else {
                None
            };
            CmdParseResult::Ok(AsciiCmd::Stats { args: args_str })
        }
        "verbosity" => {
            let noreply = tokens.last().map(|t| *t == "noreply").unwrap_or(false);
            CmdParseResult::Ok(AsciiCmd::Verbosity { noreply })
        }
        "quit" => CmdParseResult::Ok(AsciiCmd::Quit),
        _ => CmdParseResult::UnknownCommand,
    }
}

// ── Hex encoding helper ─────────────────────────────────────────────

/// Hex digit lookup table for `hex_encode`.
///
/// NOTE: `hex_encode` is currently unused — the Lua scripts handle
/// hex encoding server-side.  Kept as a utility for potential future
/// callers.  Not on any runtime hot path today.
#[cfg(not(test))]
const HEX_CHARS: [u8; 16] = *b"0123456789abcdef";

/// Encode raw bytes as lowercase hex pairs.
///
/// Uses direct table lookup instead of `fmt::Write` per byte.
///
/// NOTE: This helper is currently unused at runtime — Lua scripts
/// perform hex encoding inside Redis.  It is retained as a utility
/// for potential future callers and is **not** on a hot path today.
#[cfg(not(test))]
fn hex_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for &b in data {
        s.push(HEX_CHARS[(b >> 4) as usize] as char);
        s.push(HEX_CHARS[(b & 0x0f) as usize] as char);
    }
    s
}

// ═══════════════════════════════════════════════════════════════════
// Runtime connection handler (compiled out in unit-test builds)
// ═══════════════════════════════════════════════════════════════════

#[cfg(not(test))]
use std::io::Read;
#[cfg(not(test))]
use std::net::TcpStream;
#[cfg(not(test))]
use std::sync::atomic::Ordering;
#[cfg(not(test))]
use bytes::{Buf, BytesMut};
#[cfg(not(test))]
use crate::{
    Br, BridgeErr, with_ctx, eval_int, eval_str, hex_decode, make_redis_key, eval_lua,
    CAS_COUNTER_KEY,
    SCRIPT_STORE, SCRIPT_GET, SCRIPT_DELETE,
    SCRIPT_COUNTER, SCRIPT_TOUCH, SCRIPT_GAT, SCRIPT_APPEND, SCRIPT_PREPEND,
    SCRIPT_META_GET, SCRIPT_FLUSH, SCRIPT_COUNT_ITEMS,
    MAX_CONNECTIONS,
    STAT_CMD_GET, STAT_CMD_SET, STAT_CMD_FLUSH, STAT_CMD_TOUCH,
    STAT_GET_HITS, STAT_GET_MISSES, STAT_DELETE_HITS, STAT_DELETE_MISSES,
    STAT_INCR_HITS, STAT_INCR_MISSES, STAT_DECR_HITS, STAT_DECR_MISSES,
    STAT_CAS_HITS, STAT_CAS_MISSES, STAT_CAS_BADVAL,
    STAT_CURR_CONNECTIONS, STAT_TOTAL_CONNECTIONS,
    STAT_AUTH_CMDS, STAT_AUTH_ERRORS, STAT_REJECTED_CONNECTIONS,
    STARTUP_INSTANT, is_redis_error,
};
#[cfg(not(test))]
use crate::meta::{
    MetaCmd, MetaFlag, MetaParseResult, parse_meta_command,
    has_flag, get_flag_token, write_meta_flag_echo,
    validate_mg_flags, validate_ms_flags, validate_md_flags, validate_ma_flags,
    validate_mn_flags, validate_me_flags,
};
#[cfg(not(test))]
use redis_module::RedisValue;
#[cfg(not(test))]
use crate::protocol::MAX_BODY_LEN;

/// ASCII connection handler — entered after protocol detection.
/// `buf` already contains the first bytes read by `handle_conn`.
#[cfg(not(test))]
pub(crate) fn handle_ascii_conn(sock: &mut TcpStream, buf: &mut BytesMut) -> Br<()> {
    let mut out = Vec::with_capacity(4096);

    loop {
        // Try to extract a complete line from the buffer.
        if let Some((line_end, skip)) = find_line_end(buf) {
            if line_end > MAX_LINE_LEN {
                out.extend_from_slice(b"CLIENT_ERROR line too long\r\n");
                buf.advance(skip);
                flush_out(sock, &mut out)?;
                continue;
            }
            // Copy line bytes so we release the immutable borrow on buf
            // before calling buf.advance() or read_data_block().
            let line_bytes = buf[..line_end].to_vec();
            buf.advance(skip);
            // Skip empty lines (blank lines between commands).
            if line_bytes.is_empty() || line_bytes.iter().all(|b| b.is_ascii_whitespace()) {
                continue;
            }

            // ── Text-path prefix routing ──────────────────────────
            // Meta protocol commands use two-letter prefixes (mg, ms,
            // md, ma, mn, me) followed by a space.  Route them to the
            // meta protocol handlers; classic ASCII commands fall through.
            if is_meta_command(&line_bytes) {
                match parse_meta_command(&line_bytes) {
                    MetaParseResult::Ok(cmd) => {
                        dispatch_meta_cmd(cmd, None, &mut out)?;
                    }
                    MetaParseResult::NeedData(cmd, datalen) => {
                        if datalen > MAX_BODY_LEN as u32 {
                            out.extend_from_slice(b"CLIENT_ERROR object too large for cache\r\n");
                            drain_data_block(sock, buf, datalen)?;
                        } else {
                            match read_data_block(sock, buf, datalen) {
                                Ok(data) => {
                                    dispatch_meta_cmd(cmd, Some(&data), &mut out)?;
                                }
                                Err(_) => {
                                    out.extend_from_slice(b"CLIENT_ERROR bad data chunk\r\n");
                                }
                            }
                        }
                    }
                    MetaParseResult::ClientError(msg) => {
                        out.extend_from_slice(b"CLIENT_ERROR ");
                        out.extend_from_slice(msg.as_bytes());
                        out.extend_from_slice(b"\r\n");
                    }
                }
                flush_out(sock, &mut out)?;
                continue;
            }

            let cmd = parse_command_line(&line_bytes);

            match cmd {
                CmdParseResult::Ok(ascii_cmd) => {
                    if needs_data_block(&ascii_cmd) {
                        let byte_count = data_block_len(&ascii_cmd);
                        if byte_count > MAX_BODY_LEN {
                            let nr = is_noreply(&ascii_cmd);
                            if !nr { out.extend_from_slice(b"CLIENT_ERROR object too large for cache\r\n"); }
                            drain_data_block(sock, buf, byte_count)?;
                            flush_out(sock, &mut out)?;
                            continue;
                        }
                        let data = match read_data_block(sock, buf, byte_count) {
                            Ok(d) => d,
                            Err(_) => {
                                let nr = is_noreply(&ascii_cmd);
                                if !nr { out.extend_from_slice(b"CLIENT_ERROR bad data chunk\r\n"); }
                                flush_out(sock, &mut out)?;
                                continue;
                            }
                        };
                        dispatch_cmd(ascii_cmd, Some(&data), &mut out)?;
                    } else {
                        dispatch_cmd(ascii_cmd, None, &mut out)?;
                    }
                }
                CmdParseResult::UnknownCommand => {
                    out.extend_from_slice(b"ERROR\r\n");
                }
                CmdParseResult::ClientError(msg) => {
                    out.extend_from_slice(b"CLIENT_ERROR ");
                    out.extend_from_slice(msg.as_bytes());
                    out.extend_from_slice(b"\r\n");
                }
            }

            flush_out(sock, &mut out)?;
            continue;
        }

        // No complete line yet — check buffer size guard.
        if buf.len() > MAX_LINE_LEN + 2 {
            // Line too long — cannot find \n within limit.
            out.extend_from_slice(b"CLIENT_ERROR line too long\r\n");
            flush_out(sock, &mut out)?;
            return Ok(());
        }

        // Read more data.
        let mut tmp = [0u8; 16384];
        match sock.read(&mut tmp) {
            Ok(0) => return Ok(()),
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                return Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into());
            }
            Err(e) => return Err(e.into()),
        }
    }
}

// ── Helper functions for the connection handler ─────────────────────

#[cfg(not(test))]
fn flush_out(sock: &mut TcpStream, out: &mut Vec<u8>) -> Br<()> {
    if !out.is_empty() {
        use std::io::Write;
        sock.write_all(out)?;
        out.clear();
    }
    Ok(())
}

/// Check if a line is a meta protocol command.
/// Meta commands use two-letter prefixes: mg, ms, md, ma, mn, me
/// followed by a space (or end of line for mn/me which can be bare).
fn is_meta_command(line: &[u8]) -> bool {
    if line.len() < 2 {
        return false;
    }
    let prefix = &line[..2];
    let is_meta_prefix = prefix == b"mg" || prefix == b"ms" || prefix == b"md"
        || prefix == b"ma" || prefix == b"mn" || prefix == b"me";
    if !is_meta_prefix {
        return false;
    }
    // Must be followed by space, \t, or end of line (for bare mn/me).
    line.len() == 2 || line[2] == b' ' || line[2] == b'\t'
}

/// Does this command need a data block after the command line?
fn needs_data_block(cmd: &AsciiCmd<'_>) -> bool {
    matches!(cmd, AsciiCmd::Store { .. } | AsciiCmd::Cas { .. } | AsciiCmd::AppendPrepend { .. })
}

/// Get the data block length declared by the command.
fn data_block_len(cmd: &AsciiCmd<'_>) -> u32 {
    match cmd {
        AsciiCmd::Store { bytes, .. } => *bytes,
        AsciiCmd::Cas { bytes, .. } => *bytes,
        AsciiCmd::AppendPrepend { bytes, .. } => *bytes,
        _ => 0,
    }
}

/// Check if noreply is set on this command.
fn is_noreply(cmd: &AsciiCmd<'_>) -> bool {
    match cmd {
        AsciiCmd::Store { noreply, .. } => *noreply,
        AsciiCmd::Cas { noreply, .. } => *noreply,
        AsciiCmd::AppendPrepend { noreply, .. } => *noreply,
        AsciiCmd::Delete { noreply, .. } => *noreply,
        AsciiCmd::Counter { noreply, .. } => *noreply,
        AsciiCmd::Touch { noreply, .. } => *noreply,
        AsciiCmd::FlushAll { noreply, .. } => *noreply,
        AsciiCmd::Verbosity { noreply, .. } => *noreply,
        _ => false,
    }
}

/// Read exactly `byte_count` bytes of data plus trailing \r\n from
/// the socket/buffer.  Returns the data bytes (without the \r\n).
#[cfg(not(test))]
fn read_data_block(sock: &mut TcpStream, buf: &mut BytesMut, byte_count: u32) -> Result<Vec<u8>, ()> {
    let need = byte_count as usize + 2; // data + \r\n
    // Read until we have enough.
    while buf.len() < need {
        let mut tmp = [0u8; 16384];
        match sock.read(&mut tmp) {
            Ok(0) => return Err(()),
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => return Err(()),
        }
    }
    let data = buf[..byte_count as usize].to_vec();
    // Validate trailing \r\n (or bare \n).
    let trail = &buf[byte_count as usize..byte_count as usize + 2];
    let valid_terminator = trail == b"\r\n"
        || (trail[0] == b'\n'); // bare \n + whatever follows
    if !valid_terminator {
        buf.advance(need);
        return Err(());
    }
    let skip = if trail[0] == b'\r' { 2 } else { 1 };
    buf.advance(byte_count as usize + skip);
    Ok(data)
}

/// Drain a data block we don't intend to use (e.g., oversized).
#[cfg(not(test))]
fn drain_data_block(sock: &mut TcpStream, buf: &mut BytesMut, byte_count: u32) -> Br<()> {
    let need = byte_count as usize + 2;
    while buf.len() < need {
        let mut tmp = [0u8; 16384];
        match sock.read(&mut tmp) {
            Ok(0) => return Ok(()),
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(e) => return Err(e.into()),
        }
    }
    buf.advance(need);
    Ok(())
}

// ── Command dispatch ────────────────────────────────────────────────

#[cfg(not(test))]
fn dispatch_cmd(cmd: AsciiCmd<'_>, data: Option<&[u8]>, out: &mut Vec<u8>) -> Br<()> {
    match cmd {
        AsciiCmd::Store { cmd, key, flags, exptime, noreply, .. } => {
            ascii_store(cmd, key, flags, exptime, 0, data.unwrap_or(&[]), noreply, out)
        }
        AsciiCmd::Cas { key, flags, exptime, cas_unique, noreply, .. } => {
            ascii_store(StoreOp::Set, key, flags, exptime, cas_unique, data.unwrap_or(&[]), noreply, out)
        }
        AsciiCmd::AppendPrepend { is_prepend, key, noreply, .. } => {
            ascii_append_prepend(is_prepend, key, data.unwrap_or(&[]), noreply, out)
        }
        AsciiCmd::Retrieval { cmd, exptime, keys } => {
            ascii_retrieval(cmd, exptime, &keys, out)
        }
        AsciiCmd::Delete { key, noreply } => {
            ascii_delete(key, noreply, out)
        }
        AsciiCmd::Counter { is_decr, key, value, noreply } => {
            ascii_counter(is_decr, key, value, noreply, out)
        }
        AsciiCmd::Touch { key, exptime, noreply } => {
            ascii_touch(key, exptime, noreply, out)
        }
        AsciiCmd::FlushAll { noreply, .. } => {
            ascii_flush(noreply, out)
        }
        AsciiCmd::Version => {
            out.extend_from_slice(VERSION_STRING.as_bytes());
            out.extend_from_slice(b"\r\n");
            Ok(())
        }
        AsciiCmd::Stats { args } => {
            ascii_stats(args, out)
        }
        AsciiCmd::Verbosity { noreply } => {
            if !noreply { out.extend_from_slice(b"OK\r\n"); }
            Ok(())
        }
        AsciiCmd::Quit => {
            // Return an error to signal connection close.
            Err(BridgeErr::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "quit",
            )))
        }
    }
}

// ── Meta protocol dispatch ─────────────────────────────────────────

/// Dispatch a parsed meta command to the appropriate handler.
#[cfg(not(test))]
fn dispatch_meta_cmd(cmd: MetaCmd<'_>, data: Option<&[u8]>, out: &mut Vec<u8>) -> Br<()> {
    match cmd {
        MetaCmd::Noop { flags } => {
            if let Err(e) = validate_mn_flags(&flags) {
                out.extend_from_slice(b"CLIENT_ERROR ");
                out.extend_from_slice(e.as_bytes());
                out.extend_from_slice(b"\r\n");
                return Ok(());
            }
            meta_noop(&flags, out)
        }
        MetaCmd::Get { key, flags } => {
            if let Err(e) = validate_mg_flags(&flags) {
                out.extend_from_slice(b"CLIENT_ERROR ");
                out.extend_from_slice(e.as_bytes());
                out.extend_from_slice(b"\r\n");
                return Ok(());
            }
            meta_get(key, &flags, out)
        }
        MetaCmd::Set { key, flags, .. } => {
            if let Err(e) = validate_ms_flags(&flags) {
                out.extend_from_slice(b"CLIENT_ERROR ");
                out.extend_from_slice(e.as_bytes());
                out.extend_from_slice(b"\r\n");
                return Ok(());
            }
            meta_set(key, data.unwrap_or(&[]), &flags, out)
        }
        MetaCmd::Delete { key, flags } => {
            if let Err(e) = validate_md_flags(&flags) {
                out.extend_from_slice(b"CLIENT_ERROR ");
                out.extend_from_slice(e.as_bytes());
                out.extend_from_slice(b"\r\n");
                return Ok(());
            }
            meta_delete(key, &flags, out)
        }
        MetaCmd::Arithmetic { key, flags } => {
            if let Err(e) = validate_ma_flags(&flags) {
                out.extend_from_slice(b"CLIENT_ERROR ");
                out.extend_from_slice(e.as_bytes());
                out.extend_from_slice(b"\r\n");
                return Ok(());
            }
            meta_arithmetic(key, &flags, out)
        }
        MetaCmd::Debug { key, flags } => {
            if let Err(e) = validate_me_flags(&flags) {
                out.extend_from_slice(b"CLIENT_ERROR ");
                out.extend_from_slice(e.as_bytes());
                out.extend_from_slice(b"\r\n");
                return Ok(());
            }
            // me is unsupported — always return EN (not found).
            let quiet = has_flag(&flags, b'q');
            if !quiet {
                out.extend_from_slice(b"EN");
                write_meta_flag_echo(out, &flags, key);
                out.extend_from_slice(b"\r\n");
            }
            Ok(())
        }
    }
}

// ── Individual command handlers ─────────────────────────────────────

/// Handle set/add/replace/cas.
#[cfg(not(test))]
fn ascii_store(
    op: StoreOp, key: &[u8], flags: u32, exptime: u32, cas: u64,
    value: &[u8], noreply: bool, out: &mut Vec<u8>,
) -> Br<()> {
    STAT_CMD_SET.fetch_add(1, Ordering::Relaxed);
    let rk = make_redis_key(key);
    let op_name = match op {
        StoreOp::Set => "set",
        StoreOp::Add => "add",
        StoreOp::Replace => "replace",
    };
    let cas_str = cas.to_string();
    let flags_str = flags.to_string();
    let expiry_str = exptime.to_string();

    let reply = with_ctx(|ctx| {
        let keys_and_args: &[&[u8]] = &[
            b"2",
            rk.as_slice(), CAS_COUNTER_KEY.as_bytes(),
            op_name.as_bytes(), value,
            flags_str.as_bytes(), cas_str.as_bytes(), expiry_str.as_bytes(),
        ];
        eval_lua(ctx, &SCRIPT_STORE, keys_and_args)
    }).map_err(|e| BridgeErr::Redis(e.to_string()))?;

    if is_redis_error(&reply) {
        if !noreply { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
        return Ok(());
    }

    let (status_code, _new_cas) = match &reply {
        RedisValue::Array(arr) if arr.len() >= 2 => {
            let st = eval_int(&arr[0]);
            let cas_s = eval_str(&arr[1]);
            let cas_val: u64 = cas_s.parse().unwrap_or(0);
            (st, cas_val)
        }
        _ => {
            if !noreply { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
            return Ok(());
        }
    };

    let is_cas_op = cas != 0;
    match status_code {
        0 => {
            if is_cas_op { STAT_CAS_HITS.fetch_add(1, Ordering::Relaxed); }
            if !noreply { out.extend_from_slice(b"STORED\r\n"); }
        }
        -1 => {
            // NOT_FOUND (replace on missing, or CAS on missing)
            if is_cas_op { STAT_CAS_MISSES.fetch_add(1, Ordering::Relaxed); }
            if !noreply {
                if is_cas_op { out.extend_from_slice(b"NOT_FOUND\r\n"); }
                else { out.extend_from_slice(b"NOT_STORED\r\n"); }
            }
        }
        -2 => {
            // KEY_EXISTS (add on existing, or CAS mismatch)
            if is_cas_op { STAT_CAS_BADVAL.fetch_add(1, Ordering::Relaxed); }
            if !noreply {
                if is_cas_op { out.extend_from_slice(b"EXISTS\r\n"); }
                else { out.extend_from_slice(b"NOT_STORED\r\n"); }
            }
        }
        _ => {
            if !noreply { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
        }
    }
    Ok(())
}

/// Handle get/gets/gat/gats (multi-key retrieval).
#[cfg(not(test))]
fn ascii_retrieval(
    cmd: RetrievalOp, exptime: Option<u32>, keys: &[&[u8]], out: &mut Vec<u8>,
) -> Br<()> {
    let include_cas = matches!(cmd, RetrievalOp::Gets | RetrievalOp::Gats);
    let is_gat = matches!(cmd, RetrievalOp::Gat | RetrievalOp::Gats);

    for &key in keys {
        STAT_CMD_GET.fetch_add(1, Ordering::Relaxed);
        if is_gat { STAT_CMD_TOUCH.fetch_add(1, Ordering::Relaxed); }

        let rk = make_redis_key(key);

        let reply = if is_gat {
            let exp_str = exptime.unwrap_or(0).to_string();
            with_ctx(|ctx| {
                let keys_and_args: &[&[u8]] = &[
                    b"2",
                    rk.as_slice(), CAS_COUNTER_KEY.as_bytes(),
                    exp_str.as_bytes(),
                ];
                eval_lua(ctx, &SCRIPT_GAT, keys_and_args)
            }).map_err(|e| BridgeErr::Redis(e.to_string()))?
        } else {
            with_ctx(|ctx| {
                let keys_and_args: &[&[u8]] = &[b"1", rk.as_slice()];
                eval_lua(ctx, &SCRIPT_GET, keys_and_args)
            }).map_err(|e| BridgeErr::Redis(e.to_string()))?
        };

        if is_redis_error(&reply) { continue; }

        let (status, hex_val, flags_str, cas_str) = match &reply {
            RedisValue::Array(arr) if arr.len() >= 4 => {
                (eval_int(&arr[0]), eval_str(&arr[1]), eval_str(&arr[2]), eval_str(&arr[3]))
            }
            _ => continue,
        };

        if status == -1 {
            STAT_GET_MISSES.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        STAT_GET_HITS.fetch_add(1, Ordering::Relaxed);

        let value = hex_decode(&hex_val);
        // VALUE <key> <flags> <bytes> [<cas>]\r\n<data>\r\n
        out.extend_from_slice(b"VALUE ");
        out.extend_from_slice(key);
        out.push(b' ');
        out.extend_from_slice(flags_str.as_bytes());
        out.push(b' ');
        out.extend_from_slice(value.len().to_string().as_bytes());
        if include_cas {
            out.push(b' ');
            out.extend_from_slice(cas_str.as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&value);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"END\r\n");
    Ok(())
}

/// Handle delete.
#[cfg(not(test))]
fn ascii_delete(key: &[u8], noreply: bool, out: &mut Vec<u8>) -> Br<()> {
    let rk = make_redis_key(key);
    // ASCII delete never uses CAS — always non-CAS path.
    // Use direct DEL for the fast non-CAS bypass.
    let reply = with_ctx(|ctx| {
        ctx.call("DEL", &[rk.as_slice()])
    }).map_err(|e| BridgeErr::Redis(e.to_string()))?;

    if is_redis_error(&reply) {
        if !noreply { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
        return Ok(());
    }

    // DEL returns the number of keys deleted: 1 = found+deleted, 0 = not found.
    let deleted_count = match &reply {
        RedisValue::Integer(n) => *n,
        _ => {
            if !noreply { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
            return Ok(());
        }
    };

    if deleted_count > 0 {
        STAT_DELETE_HITS.fetch_add(1, Ordering::Relaxed);
        if !noreply { out.extend_from_slice(b"DELETED\r\n"); }
    } else {
        STAT_DELETE_MISSES.fetch_add(1, Ordering::Relaxed);
        if !noreply { out.extend_from_slice(b"NOT_FOUND\r\n"); }
    }
    Ok(())
}

/// Handle incr/decr.
#[cfg(not(test))]
fn ascii_counter(is_decr: bool, key: &[u8], delta: u64, noreply: bool, out: &mut Vec<u8>) -> Br<()> {
    let rk = make_redis_key(key);
    let delta_str = delta.to_string();
    let is_decr_str = if is_decr { "1" } else { "0" };
    // ASCII incr/decr: if key doesn't exist, return NOT_FOUND.
    // We use expiry=4294967295 (0xFFFFFFFF) to signal "do not create".
    let initial_str = "0";
    let expiry_str = "4294967295";

    let reply = with_ctx(|ctx| {
        let keys_and_args: &[&[u8]] = &[
            b"2",
            rk.as_slice(), CAS_COUNTER_KEY.as_bytes(),
            delta_str.as_bytes(), is_decr_str.as_bytes(),
            initial_str.as_bytes(), expiry_str.as_bytes(),
        ];
        eval_lua(ctx, &SCRIPT_COUNTER, keys_and_args)
    }).map_err(|e| BridgeErr::Redis(e.to_string()))?;

    if is_redis_error(&reply) {
        if !noreply { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
        return Ok(());
    }

    let (status, value_str, _cas) = match &reply {
        RedisValue::Array(arr) if arr.len() >= 3 => {
            (eval_int(&arr[0]), eval_str(&arr[1]), eval_str(&arr[2]))
        }
        _ => {
            if !noreply { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
            return Ok(());
        }
    };

    match status {
        0 => {
            if is_decr { STAT_DECR_HITS.fetch_add(1, Ordering::Relaxed); }
            else { STAT_INCR_HITS.fetch_add(1, Ordering::Relaxed); }
            if !noreply {
                out.extend_from_slice(value_str.as_bytes());
                out.extend_from_slice(b"\r\n");
            }
        }
        -1 => {
            if is_decr { STAT_DECR_MISSES.fetch_add(1, Ordering::Relaxed); }
            else { STAT_INCR_MISSES.fetch_add(1, Ordering::Relaxed); }
            if !noreply { out.extend_from_slice(b"NOT_FOUND\r\n"); }
        }
        -3 => {
            // Non-numeric value.
            if !noreply {
                out.extend_from_slice(b"CLIENT_ERROR cannot increment or decrement non-numeric value\r\n");
            }
        }
        _ => {
            if !noreply { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
        }
    }
    Ok(())
}

/// Handle touch.
#[cfg(not(test))]
fn ascii_touch(key: &[u8], exptime: u32, noreply: bool, out: &mut Vec<u8>) -> Br<()> {
    STAT_CMD_TOUCH.fetch_add(1, Ordering::Relaxed);
    let rk = make_redis_key(key);
    let exp_str = exptime.to_string();

    let reply = with_ctx(|ctx| {
        let keys_and_args: &[&[u8]] = &[
            b"2",
            rk.as_slice(), CAS_COUNTER_KEY.as_bytes(),
            exp_str.as_bytes(),
        ];
        eval_lua(ctx, &SCRIPT_TOUCH, keys_and_args)
    }).map_err(|e| BridgeErr::Redis(e.to_string()))?;

    if is_redis_error(&reply) {
        if !noreply { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
        return Ok(());
    }

    let status = match &reply {
        RedisValue::Array(arr) if !arr.is_empty() => eval_int(&arr[0]),
        _ => {
            if !noreply { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
            return Ok(());
        }
    };

    match status {
        0 => { if !noreply { out.extend_from_slice(b"TOUCHED\r\n"); } }
        _ => { if !noreply { out.extend_from_slice(b"NOT_FOUND\r\n"); } }
    }
    Ok(())
}

/// Handle append/prepend.
#[cfg(not(test))]
fn ascii_append_prepend(
    is_prepend: bool, key: &[u8], value: &[u8], noreply: bool, out: &mut Vec<u8>,
) -> Br<()> {
    STAT_CMD_SET.fetch_add(1, Ordering::Relaxed);
    let rk = make_redis_key(key);
    let script = if is_prepend { &SCRIPT_PREPEND } else { &SCRIPT_APPEND };

    let reply = with_ctx(|ctx| {
        let keys_and_args: &[&[u8]] = &[
            b"2",
            rk.as_slice(), CAS_COUNTER_KEY.as_bytes(),
            value, b"0",
        ];
        eval_lua(ctx, script, keys_and_args)
    }).map_err(|e| BridgeErr::Redis(e.to_string()))?;

    if is_redis_error(&reply) {
        if !noreply { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
        return Ok(());
    }

    let status = match &reply {
        RedisValue::Array(arr) if !arr.is_empty() => eval_int(&arr[0]),
        _ => {
            if !noreply { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
            return Ok(());
        }
    };

    match status {
        0 => { if !noreply { out.extend_from_slice(b"STORED\r\n"); } }
        -5 => {
            // NOT_STORED — key does not exist.
            if !noreply { out.extend_from_slice(b"NOT_STORED\r\n"); }
        }
        _ => { if !noreply { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); } }
    }
    Ok(())
}

/// Handle flush_all.
#[cfg(not(test))]
fn ascii_flush(noreply: bool, out: &mut Vec<u8>) -> Br<()> {
    STAT_CMD_FLUSH.fetch_add(1, Ordering::Relaxed);

    // Flush via EVALSHA (NOSCRIPT fallback) — same as binary flush handler.
    with_ctx(|ctx| {
        let keys_and_args: &[&[u8]] = &[b"0"];
        eval_lua(ctx, &SCRIPT_FLUSH, keys_and_args)
    }).map_err(|e| BridgeErr::Redis(e.to_string()))?;

    if !noreply { out.extend_from_slice(b"OK\r\n"); }
    Ok(())
}

// ── Meta protocol handlers ─────────────────────────────────────────

/// Handle mn (meta noop).
#[cfg(not(test))]
fn meta_noop(flags: &[MetaFlag], out: &mut Vec<u8>) -> Br<()> {
    out.extend_from_slice(b"MN");
    if let Some(opaque) = get_flag_token(flags, b'O') {
        out.push(b' ');
        out.push(b'O');
        out.extend_from_slice(opaque.as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    Ok(())
}

/// Handle mg (meta get).
#[cfg(not(test))]
fn meta_get(key: &[u8], flags: &[MetaFlag], out: &mut Vec<u8>) -> Br<()> {
    STAT_CMD_GET.fetch_add(1, Ordering::Relaxed);

    let rk = make_redis_key(key);
    let want_ttl_update = get_flag_token(flags, b'T');

    let reply = if let Some(ttl_str) = want_ttl_update {
        // Use GAT to update TTL, then we'll get a second call for TTL info.
        // Actually, we need the extended meta get that also returns TTL.
        // Use LUA_META_GET after touch.
        let exp_str = ttl_str.to_string();
        with_ctx(|ctx| {
            // First touch to update TTL.
            let touch_keys_and_args: &[&[u8]] = &[
                b"2",
                rk.as_slice(), CAS_COUNTER_KEY.as_bytes(),
                exp_str.as_bytes(),
            ];
            eval_lua(ctx, &SCRIPT_TOUCH, touch_keys_and_args)
        }).map_err(|e| BridgeErr::Redis(e.to_string()))?;
        // Then get with TTL info.
        with_ctx(|ctx| {
            let keys_and_args: &[&[u8]] = &[b"1", rk.as_slice()];
            eval_lua(ctx, &SCRIPT_META_GET, keys_and_args)
        }).map_err(|e| BridgeErr::Redis(e.to_string()))?
    } else {
        with_ctx(|ctx| {
            let keys_and_args: &[&[u8]] = &[b"1", rk.as_slice()];
            eval_lua(ctx, &SCRIPT_META_GET, keys_and_args)
        }).map_err(|e| BridgeErr::Redis(e.to_string()))?
    };

    if is_redis_error(&reply) {
        out.extend_from_slice(b"SERVER_ERROR internal\r\n");
        return Ok(());
    }

    // Parse: {status, hex_value, flags_string, cas_string, ttl, size}
    let (status, hex_val, item_flags, cas_str, ttl, size) = match &reply {
        RedisValue::Array(arr) if arr.len() >= 6 => {
            let st = eval_int(&arr[0]);
            let hv = eval_str(&arr[1]);
            let fl = eval_str(&arr[2]);
            let cs = eval_str(&arr[3]);
            let t = eval_int(&arr[4]);
            let sz = eval_int(&arr[5]);
            (st, hv, fl, cs, t, sz)
        }
        _ => {
            out.extend_from_slice(b"SERVER_ERROR internal\r\n");
            return Ok(());
        }
    };

    let quiet = has_flag(flags, b'q');
    if status == -1 {
        STAT_GET_MISSES.fetch_add(1, Ordering::Relaxed);
        if !quiet {
            out.extend_from_slice(b"EN");
            write_meta_flag_echo(out, flags, key);
            out.extend_from_slice(b"\r\n");
        }
        return Ok(());
    }

    STAT_GET_HITS.fetch_add(1, Ordering::Relaxed);

    let value = hex_decode(&hex_val);
    let want_value = has_flag(flags, b'v');

    if want_value {
        // VA <size> [flags]\r\n<data>\r\n
        out.extend_from_slice(b"VA ");
        out.extend_from_slice(value.len().to_string().as_bytes());
    } else {
        // HD [flags]\r\n
        out.extend_from_slice(b"HD");
    }

    // Append requested metadata flags.
    if has_flag(flags, b'c') {
        out.push(b' ');
        out.push(b'c');
        out.extend_from_slice(cas_str.as_bytes());
    }
    if has_flag(flags, b'f') {
        out.push(b' ');
        out.push(b'f');
        out.extend_from_slice(item_flags.as_bytes());
    }
    if has_flag(flags, b's') {
        out.push(b' ');
        out.push(b's');
        out.extend_from_slice(size.to_string().as_bytes());
    }
    if has_flag(flags, b't') {
        out.push(b' ');
        out.push(b't');
        // ttl: -1 means no expiry, positive means seconds remaining.
        let ttl_val = if ttl == -1 { -1 } else { ttl };
        out.extend_from_slice(ttl_val.to_string().as_bytes());
    }
    write_meta_flag_echo(out, flags, key);
    out.extend_from_slice(b"\r\n");

    if want_value {
        out.extend_from_slice(&value);
        out.extend_from_slice(b"\r\n");
    }
    Ok(())
}

/// Handle ms (meta set).
#[cfg(not(test))]
fn meta_set(key: &[u8], data: &[u8], flags: &[MetaFlag], out: &mut Vec<u8>) -> Br<()> {
    STAT_CMD_SET.fetch_add(1, Ordering::Relaxed);

    let quiet = has_flag(flags, b'q');
    let mode = get_flag_token(flags, b'M').unwrap_or("S");
    let item_flags = get_flag_token(flags, b'F').unwrap_or("0");
    let ttl = get_flag_token(flags, b'T').unwrap_or("0");
    let cas = get_flag_token(flags, b'C').unwrap_or("0");

    match mode {
        "A" | "P" => {
            // Append/Prepend mode.
            let is_prepend = mode == "P";
            let script = if is_prepend { &SCRIPT_PREPEND } else { &SCRIPT_APPEND };
            let rk = make_redis_key(key);
            let reply = with_ctx(|ctx| {
                let keys_and_args: &[&[u8]] = &[
                    b"2",
                    rk.as_slice(), CAS_COUNTER_KEY.as_bytes(),
                    data, cas.as_bytes(),
                ];
                eval_lua(ctx, script, keys_and_args)
            }).map_err(|e| BridgeErr::Redis(e.to_string()))?;

            if is_redis_error(&reply) {
                if !quiet { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
                return Ok(());
            }

            let (status, cas_val) = match &reply {
                RedisValue::Array(arr) if arr.len() >= 2 => {
                    (eval_int(&arr[0]), eval_str(&arr[1]))
                }
                _ => {
                    if !quiet { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
                    return Ok(());
                }
            };

            if !quiet {
                match status {
                    0 => {
                        out.extend_from_slice(b"HD");
                        if has_flag(flags, b'c') || cas != "0" {
                            out.push(b' ');
                            out.push(b'c');
                            out.extend_from_slice(cas_val.as_bytes());
                        }
                        write_meta_flag_echo(out, flags, key);
                        out.extend_from_slice(b"\r\n");
                    }
                    -5 => {
                        // NOT_FOUND for append/prepend.
                        out.extend_from_slice(b"NS");
                        write_meta_flag_echo(out, flags, key);
                        out.extend_from_slice(b"\r\n");
                    }
                    -2 => {
                        // CAS mismatch.
                        out.extend_from_slice(b"EX");
                        write_meta_flag_echo(out, flags, key);
                        out.extend_from_slice(b"\r\n");
                    }
                    _ => {
                        out.extend_from_slice(b"SERVER_ERROR internal\r\n");
                    }
                }
            }
        }
        "S" | "E" | "R" => {
            // S (set), E (add), R (replace) modes.
            let op_name = match mode {
                "E" => "add",
                "R" => "replace",
                _ => "set",
            };
            let rk = make_redis_key(key);
            let cas_str = cas.to_string();
            let flags_str = item_flags.to_string();
            let expiry_str = ttl.to_string();

            let reply = with_ctx(|ctx| {
                let keys_and_args: &[&[u8]] = &[
                    b"2",
                    rk.as_slice(), CAS_COUNTER_KEY.as_bytes(),
                    op_name.as_bytes(), data,
                    flags_str.as_bytes(), cas_str.as_bytes(), expiry_str.as_bytes(),
                ];
                eval_lua(ctx, &SCRIPT_STORE, keys_and_args)
            }).map_err(|e| BridgeErr::Redis(e.to_string()))?;

            if is_redis_error(&reply) {
                if !quiet { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
                return Ok(());
            }

            let (status, new_cas) = match &reply {
                RedisValue::Array(arr) if arr.len() >= 2 => {
                    (eval_int(&arr[0]), eval_str(&arr[1]))
                }
                _ => {
                    if !quiet { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
                    return Ok(());
                }
            };

            if !quiet {
                match status {
                    0 => {
                        STAT_CAS_HITS.fetch_add(1, Ordering::Relaxed);
                        out.extend_from_slice(b"HD");
                        if has_flag(flags, b'c') || cas != "0" {
                            out.push(b' ');
                            out.push(b'c');
                            out.extend_from_slice(new_cas.as_bytes());
                        }
                        write_meta_flag_echo(out, flags, key);
                        out.extend_from_slice(b"\r\n");
                    }
                    -1 => {
                        // NOT_FOUND (replace on missing key, or CAS on missing key).
                        out.extend_from_slice(b"NF");
                        write_meta_flag_echo(out, flags, key);
                        out.extend_from_slice(b"\r\n");
                    }
                    -2 => {
                        // KEY_EXISTS (add on existing, or CAS mismatch).
                        if cas != "0" {
                            STAT_CAS_BADVAL.fetch_add(1, Ordering::Relaxed);
                            out.extend_from_slice(b"EX");
                        } else {
                            out.extend_from_slice(b"NS");
                        }
                        write_meta_flag_echo(out, flags, key);
                        out.extend_from_slice(b"\r\n");
                    }
                    _ => {
                        out.extend_from_slice(b"SERVER_ERROR internal\r\n");
                    }
                }
            }
        }
        _ => {
            // Unreachable: validate_ms_flags rejects unknown modes before dispatch.
            if !quiet { out.extend_from_slice(b"CLIENT_ERROR unsupported ms mode\r\n"); }
        }
    }
    Ok(())
}

/// Handle md (meta delete).
#[cfg(not(test))]
fn meta_delete(key: &[u8], flags: &[MetaFlag], out: &mut Vec<u8>) -> Br<()> {
    let rk = make_redis_key(key);
    let cas = get_flag_token(flags, b'C').unwrap_or("0");
    let quiet = has_flag(flags, b'q');

    // Non-CAS DELETE bypass: when CAS is "0", use direct DEL instead of Lua.
    let (reply, is_direct_del) = if cas == "0" {
        let r = with_ctx(|ctx| {
            ctx.call("DEL", &[rk.as_slice()])
        }).map_err(|e| BridgeErr::Redis(e.to_string()))?;
        (r, true)
    } else {
        let r = with_ctx(|ctx| {
            let keys_and_args: &[&[u8]] = &[
                b"1", rk.as_slice(), cas.as_bytes(),
            ];
            eval_lua(ctx, &SCRIPT_DELETE, keys_and_args)
        }).map_err(|e| BridgeErr::Redis(e.to_string()))?;
        (r, false)
    };

    if is_redis_error(&reply) {
        if !quiet { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
        return Ok(());
    }

    // Direct DEL returns count (1=deleted, 0=not found).
    // Lua DELETE returns status (0=OK, -1=NOT_FOUND, -2=CAS mismatch).
    let status = match &reply {
        RedisValue::Integer(n) => {
            if is_direct_del {
                // Map DEL count to Lua-compatible status codes.
                if *n > 0 { 0 } else { -1 }
            } else {
                *n
            }
        }
        _ => {
            if !quiet { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
            return Ok(());
        }
    };

    match status {
        0 => {
            STAT_DELETE_HITS.fetch_add(1, Ordering::Relaxed);
            if !quiet {
                out.extend_from_slice(b"HD");
                write_meta_flag_echo(out, flags, key);
                out.extend_from_slice(b"\r\n");
            }
        }
        -1 => {
            STAT_DELETE_MISSES.fetch_add(1, Ordering::Relaxed);
            if !quiet {
                out.extend_from_slice(b"NF");
                write_meta_flag_echo(out, flags, key);
                out.extend_from_slice(b"\r\n");
            }
        }
        -2 => {
            // CAS mismatch.
            if !quiet {
                out.extend_from_slice(b"EX");
                write_meta_flag_echo(out, flags, key);
                out.extend_from_slice(b"\r\n");
            }
        }
        _ => {
            if !quiet { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
        }
    }
    Ok(())
}

/// Handle ma (meta arithmetic).
#[cfg(not(test))]
fn meta_arithmetic(key: &[u8], flags: &[MetaFlag], out: &mut Vec<u8>) -> Br<()> {
    let rk = make_redis_key(key);
    let quiet = has_flag(flags, b'q');

    let delta_str = get_flag_token(flags, b'D').unwrap_or("1");
    let mode = get_flag_token(flags, b'M').unwrap_or("I");
    // Mode validation already done in validate_ma_flags; only I/D reach here.
    let is_decr = mode == "D";
    let is_decr_str = if is_decr { "1" } else { "0" };
    let initial = get_flag_token(flags, b'J').unwrap_or("0");
    // N flag = TTL for auto-vivification. Without N, use 4294967295 to signal "do not create".
    let expiry = get_flag_token(flags, b'N').unwrap_or("4294967295");

    let reply = with_ctx(|ctx| {
        let keys_and_args: &[&[u8]] = &[
            b"2",
            rk.as_slice(), CAS_COUNTER_KEY.as_bytes(),
            delta_str.as_bytes(), is_decr_str.as_bytes(),
            initial.as_bytes(), expiry.as_bytes(),
        ];
        eval_lua(ctx, &SCRIPT_COUNTER, keys_and_args)
    }).map_err(|e| BridgeErr::Redis(e.to_string()))?;

    if is_redis_error(&reply) {
        if !quiet { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
        return Ok(());
    }

    let (status, value_str, cas_str) = match &reply {
        RedisValue::Array(arr) if arr.len() >= 3 => {
            let st = eval_int(&arr[0]);
            let val = eval_str(&arr[1]);
            let cas_s = eval_str(&arr[2]);
            (st, val, cas_s)
        }
        _ => {
            if !quiet { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
            return Ok(());
        }
    };

    if is_decr {
        if status == 0 { STAT_DECR_HITS.fetch_add(1, Ordering::Relaxed); }
        else { STAT_DECR_MISSES.fetch_add(1, Ordering::Relaxed); }
    } else {
        if status == 0 { STAT_INCR_HITS.fetch_add(1, Ordering::Relaxed); }
        else { STAT_INCR_MISSES.fetch_add(1, Ordering::Relaxed); }
    }

    match status {
        0 => {
            let want_value = has_flag(flags, b'v');
            if !quiet {
                if want_value {
                    out.extend_from_slice(b"VA ");
                    out.extend_from_slice(value_str.len().to_string().as_bytes());
                } else {
                    out.extend_from_slice(b"HD");
                }
                if has_flag(flags, b'c') {
                    out.push(b' ');
                    out.push(b'c');
                    out.extend_from_slice(cas_str.as_bytes());
                }
                write_meta_flag_echo(out, flags, key);
                out.extend_from_slice(b"\r\n");
                if want_value {
                    out.extend_from_slice(value_str.as_bytes());
                    out.extend_from_slice(b"\r\n");
                }
            }
        }
        -1 => {
            // NOT_FOUND — key doesn't exist and N flag not present.
            if !quiet {
                out.extend_from_slice(b"NF");
                write_meta_flag_echo(out, flags, key);
                out.extend_from_slice(b"\r\n");
            }
        }
        -3 => {
            // NON_NUMERIC — value is not a number.
            if !quiet {
                out.extend_from_slice(b"CLIENT_ERROR cannot increment or decrement non-numeric value\r\n");
            }
        }
        _ => {
            if !quiet { out.extend_from_slice(b"SERVER_ERROR internal\r\n"); }
        }
    }
    Ok(())
}

/// Handle stats [args].
#[cfg(not(test))]
fn ascii_stats(args: Option<&str>, out: &mut Vec<u8>) -> Br<()> {
    match args {
        None | Some("") => {
            // General stats — same counters as binary stat handler.
            let uptime = STARTUP_INSTANT
                .get()
                .map(|t| t.elapsed().as_secs())
                .unwrap_or(0);
            let pid = std::process::id();

            let curr_items: u64 = match with_ctx(|ctx| {
                let keys_and_args: &[&[u8]] = &[b"0"];
                eval_lua(ctx, &SCRIPT_COUNT_ITEMS, keys_and_args)
            }) {
                Ok(RedisValue::Integer(n)) => n as u64,
                _ => 0,
            };

            let stats: Vec<(&str, String)> = vec![
                ("pid", pid.to_string()),
                ("uptime", uptime.to_string()),
                ("time", std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs()).unwrap_or(0).to_string()),
                ("version", "RedCouch 0.1.0".to_string()),
                ("curr_items", curr_items.to_string()),
                ("curr_connections", STAT_CURR_CONNECTIONS.load(Ordering::Relaxed).to_string()),
                ("total_connections", STAT_TOTAL_CONNECTIONS.load(Ordering::Relaxed).to_string()),
                ("cmd_get", STAT_CMD_GET.load(Ordering::Relaxed).to_string()),
                ("cmd_set", STAT_CMD_SET.load(Ordering::Relaxed).to_string()),
                ("cmd_flush", STAT_CMD_FLUSH.load(Ordering::Relaxed).to_string()),
                ("cmd_touch", STAT_CMD_TOUCH.load(Ordering::Relaxed).to_string()),
                ("get_hits", STAT_GET_HITS.load(Ordering::Relaxed).to_string()),
                ("get_misses", STAT_GET_MISSES.load(Ordering::Relaxed).to_string()),
                ("delete_hits", STAT_DELETE_HITS.load(Ordering::Relaxed).to_string()),
                ("delete_misses", STAT_DELETE_MISSES.load(Ordering::Relaxed).to_string()),
                ("incr_hits", STAT_INCR_HITS.load(Ordering::Relaxed).to_string()),
                ("incr_misses", STAT_INCR_MISSES.load(Ordering::Relaxed).to_string()),
                ("decr_hits", STAT_DECR_HITS.load(Ordering::Relaxed).to_string()),
                ("decr_misses", STAT_DECR_MISSES.load(Ordering::Relaxed).to_string()),
                ("cas_hits", STAT_CAS_HITS.load(Ordering::Relaxed).to_string()),
                ("cas_misses", STAT_CAS_MISSES.load(Ordering::Relaxed).to_string()),
                ("cas_badval", STAT_CAS_BADVAL.load(Ordering::Relaxed).to_string()),
                ("auth_cmds", STAT_AUTH_CMDS.load(Ordering::Relaxed).to_string()),
                ("auth_errors", STAT_AUTH_ERRORS.load(Ordering::Relaxed).to_string()),
                ("rejected_connections", STAT_REJECTED_CONNECTIONS.load(Ordering::Relaxed).to_string()),
                ("max_connections", MAX_CONNECTIONS.to_string()),
            ];

            for (name, value) in &stats {
                out.extend_from_slice(b"STAT ");
                out.extend_from_slice(name.as_bytes());
                out.push(b' ');
                out.extend_from_slice(value.as_bytes());
                out.extend_from_slice(b"\r\n");
            }
        }
        // All unsupported stat groups → empty END.
        Some(_) => {}
    }
    out.extend_from_slice(b"END\r\n");
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════
// Unit tests — pure parser logic only (no Redis)
// ═══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── find_line_end ───────────────────────────────────────────────

    #[test]
    fn line_end_crlf() {
        let buf = b"get foo\r\n";
        assert_eq!(find_line_end(buf), Some((7, 9)));
    }

    #[test]
    fn line_end_bare_lf() {
        let buf = b"get foo\n";
        assert_eq!(find_line_end(buf), Some((7, 8)));
    }

    #[test]
    fn line_end_no_terminator() {
        let buf = b"get foo";
        assert_eq!(find_line_end(buf), None);
    }

    #[test]
    fn line_end_empty() {
        assert_eq!(find_line_end(b""), None);
    }

    #[test]
    fn line_end_just_crlf() {
        assert_eq!(find_line_end(b"\r\n"), Some((0, 2)));
    }

    // ── validate_key ────────────────────────────────────────────────

    #[test]
    fn key_valid() {
        assert!(validate_key(b"mykey").is_ok());
    }

    #[test]
    fn key_empty() {
        assert!(validate_key(b"").is_err());
    }

    #[test]
    fn key_too_long() {
        let long_key = vec![b'a'; MAX_KEY_LEN + 1];
        assert!(validate_key(&long_key).is_err());
    }

    #[test]
    fn key_max_length_ok() {
        let key = vec![b'a'; MAX_KEY_LEN];
        assert!(validate_key(&key).is_ok());
    }

    #[test]
    fn key_with_space() {
        assert!(validate_key(b"has space").is_err());
    }

    #[test]
    fn key_with_control_char() {
        assert!(validate_key(b"has\x01ctrl").is_err());
    }

    // ── parse_command_line: storage ─────────────────────────────────

    #[test]
    fn parse_set_basic() {
        match parse_command_line(b"set mykey 0 60 5") {
            CmdParseResult::Ok(AsciiCmd::Store { cmd, key, flags, exptime, bytes, noreply }) => {
                assert_eq!(cmd, StoreOp::Set);
                assert_eq!(key, b"mykey");
                assert_eq!(flags, 0);
                assert_eq!(exptime, 60);
                assert_eq!(bytes, 5);
                assert!(!noreply);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_set_noreply() {
        match parse_command_line(b"set k 1 0 3 noreply") {
            CmdParseResult::Ok(AsciiCmd::Store { noreply, .. }) => assert!(noreply),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_add() {
        match parse_command_line(b"add k 0 0 1") {
            CmdParseResult::Ok(AsciiCmd::Store { cmd: StoreOp::Add, .. }) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_replace() {
        match parse_command_line(b"replace k 0 0 1") {
            CmdParseResult::Ok(AsciiCmd::Store { cmd: StoreOp::Replace, .. }) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ── parse_command_line: CAS ─────────────────────────────────────

    #[test]
    fn parse_cas_basic() {
        match parse_command_line(b"cas k 0 0 5 12345") {
            CmdParseResult::Ok(AsciiCmd::Cas { key, cas_unique, noreply, .. }) => {
                assert_eq!(key, b"k");
                assert_eq!(cas_unique, 12345);
                assert!(!noreply);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_cas_noreply() {
        match parse_command_line(b"cas k 0 0 5 100 noreply") {
            CmdParseResult::Ok(AsciiCmd::Cas { noreply, .. }) => assert!(noreply),
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ── parse_command_line: append/prepend ──────────────────────────

    #[test]
    fn parse_append() {
        match parse_command_line(b"append k 5") {
            CmdParseResult::Ok(AsciiCmd::AppendPrepend { is_prepend, key, bytes, noreply }) => {
                assert!(!is_prepend);
                assert_eq!(key, b"k");
                assert_eq!(bytes, 5);
                assert!(!noreply);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_prepend_noreply() {
        match parse_command_line(b"prepend k 3 noreply") {
            CmdParseResult::Ok(AsciiCmd::AppendPrepend { is_prepend, noreply, .. }) => {
                assert!(is_prepend);
                assert!(noreply);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ── parse_command_line: retrieval ───────────────────────────────

    #[test]
    fn parse_get_single() {
        match parse_command_line(b"get foo") {
            CmdParseResult::Ok(AsciiCmd::Retrieval { cmd, exptime, keys }) => {
                assert_eq!(cmd, RetrievalOp::Get);
                assert!(exptime.is_none());
                assert_eq!(keys, vec![b"foo".as_slice()]);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_get_multi() {
        match parse_command_line(b"get a b c") {
            CmdParseResult::Ok(AsciiCmd::Retrieval { keys, .. }) => {
                assert_eq!(keys.len(), 3);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_gets() {
        match parse_command_line(b"gets k") {
            CmdParseResult::Ok(AsciiCmd::Retrieval { cmd: RetrievalOp::Gets, .. }) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_gat() {
        match parse_command_line(b"gat 60 k1 k2") {
            CmdParseResult::Ok(AsciiCmd::Retrieval { cmd, exptime, keys }) => {
                assert_eq!(cmd, RetrievalOp::Gat);
                assert_eq!(exptime, Some(60));
                assert_eq!(keys.len(), 2);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_gats() {
        match parse_command_line(b"gats 0 k") {
            CmdParseResult::Ok(AsciiCmd::Retrieval { cmd: RetrievalOp::Gats, .. }) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ── parse_command_line: delete/counter/touch ────────────────────

    #[test]
    fn parse_delete() {
        match parse_command_line(b"delete mykey") {
            CmdParseResult::Ok(AsciiCmd::Delete { key, noreply }) => {
                assert_eq!(key, b"mykey");
                assert!(!noreply);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_delete_noreply() {
        match parse_command_line(b"delete k noreply") {
            CmdParseResult::Ok(AsciiCmd::Delete { noreply, .. }) => assert!(noreply),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_incr() {
        match parse_command_line(b"incr counter 5") {
            CmdParseResult::Ok(AsciiCmd::Counter { is_decr, key, value, noreply }) => {
                assert!(!is_decr);
                assert_eq!(key, b"counter");
                assert_eq!(value, 5);
                assert!(!noreply);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_decr_noreply() {
        match parse_command_line(b"decr c 10 noreply") {
            CmdParseResult::Ok(AsciiCmd::Counter { is_decr, noreply, .. }) => {
                assert!(is_decr);
                assert!(noreply);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_touch() {
        match parse_command_line(b"touch k 300") {
            CmdParseResult::Ok(AsciiCmd::Touch { key, exptime, noreply }) => {
                assert_eq!(key, b"k");
                assert_eq!(exptime, 300);
                assert!(!noreply);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ── parse_command_line: admin commands ──────────────────────────

    #[test]
    fn parse_flush_all() {
        match parse_command_line(b"flush_all") {
            CmdParseResult::Ok(AsciiCmd::FlushAll { _delay, noreply }) => {
                assert_eq!(_delay, 0);
                assert!(!noreply);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_flush_all_delay_noreply() {
        match parse_command_line(b"flush_all 30 noreply") {
            CmdParseResult::Ok(AsciiCmd::FlushAll { _delay, noreply }) => {
                assert_eq!(_delay, 30);
                assert!(noreply);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_version() {
        assert!(matches!(parse_command_line(b"version"), CmdParseResult::Ok(AsciiCmd::Version)));
    }

    #[test]
    fn parse_stats_bare() {
        match parse_command_line(b"stats") {
            CmdParseResult::Ok(AsciiCmd::Stats { args }) => assert!(args.is_none()),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_stats_with_arg() {
        match parse_command_line(b"stats items") {
            CmdParseResult::Ok(AsciiCmd::Stats { args }) => {
                assert_eq!(args, Some("items"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_verbosity() {
        match parse_command_line(b"verbosity 2") {
            CmdParseResult::Ok(AsciiCmd::Verbosity { noreply }) => assert!(!noreply),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_quit() {
        assert!(matches!(parse_command_line(b"quit"), CmdParseResult::Ok(AsciiCmd::Quit)));
    }

    // ── parse_command_line: error cases ─────────────────────────────

    #[test]
    fn parse_unknown_command() {
        assert!(matches!(parse_command_line(b"foobar key"), CmdParseResult::UnknownCommand));
    }

    #[test]
    fn parse_set_missing_args() {
        assert!(matches!(parse_command_line(b"set k 0"), CmdParseResult::ClientError(_)));
    }

    #[test]
    fn parse_set_bad_flags() {
        assert!(matches!(parse_command_line(b"set k notanum 0 5"), CmdParseResult::ClientError(_)));
    }

    #[test]
    fn parse_get_no_keys() {
        assert!(matches!(parse_command_line(b"get"), CmdParseResult::ClientError(_)));
    }

    #[test]
    fn parse_incr_bad_value() {
        assert!(matches!(parse_command_line(b"incr k -5"), CmdParseResult::ClientError(_)));
    }

    #[test]
    fn parse_set_bad_noreply_token() {
        assert!(matches!(parse_command_line(b"set k 0 0 5 garbage"), CmdParseResult::ClientError(_)));
    }

    #[test]
    fn parse_delete_bad_extra() {
        assert!(matches!(parse_command_line(b"delete k extra"), CmdParseResult::ClientError(_)));
    }

    // ── needs_data_block / data_block_len / is_noreply ─────────────

    #[test]
    fn data_block_for_store() {
        let cmd = AsciiCmd::Store {
            cmd: StoreOp::Set, key: b"k", flags: 0, exptime: 0, bytes: 10, noreply: false,
        };
        assert!(needs_data_block(&cmd));
        assert_eq!(data_block_len(&cmd), 10);
        assert!(!is_noreply(&cmd));
    }

    #[test]
    fn no_data_block_for_get() {
        let cmd = AsciiCmd::Retrieval {
            cmd: RetrievalOp::Get, exptime: None, keys: vec![b"k"],
        };
        assert!(!needs_data_block(&cmd));
    }

    // ── append/prepend correct wire format (no flags/exptime) ──────

    #[test]
    fn append_no_flags_exptime() {
        // This must fail: append does NOT take flags/exptime.
        assert!(matches!(
            parse_command_line(b"append k 0 0 5"),
            CmdParseResult::ClientError(_),
        ));
    }

    #[test]
    fn prepend_no_flags_exptime() {
        assert!(matches!(
            parse_command_line(b"prepend k 0 0 5"),
            CmdParseResult::ClientError(_),
        ));
    }

    // ── is_meta_command (prefix-based text-path routing) ───────────

    #[test]
    fn meta_get_is_meta() {
        assert!(is_meta_command(b"mg mykey"));
    }

    #[test]
    fn meta_set_is_meta() {
        assert!(is_meta_command(b"ms mykey 5"));
    }

    #[test]
    fn meta_delete_is_meta() {
        assert!(is_meta_command(b"md mykey"));
    }

    #[test]
    fn meta_arithmetic_is_meta() {
        assert!(is_meta_command(b"ma mykey"));
    }

    #[test]
    fn meta_noop_bare_is_meta() {
        assert!(is_meta_command(b"mn"));
    }

    #[test]
    fn meta_debug_is_meta() {
        assert!(is_meta_command(b"me mykey"));
    }

    #[test]
    fn classic_get_not_meta() {
        assert!(!is_meta_command(b"get foo"));
    }

    #[test]
    fn classic_set_not_meta() {
        assert!(!is_meta_command(b"set foo 0 0 5"));
    }

    #[test]
    fn short_line_not_meta() {
        assert!(!is_meta_command(b"m"));
    }

    #[test]
    fn empty_line_not_meta() {
        assert!(!is_meta_command(b""));
    }

    #[test]
    fn mg_without_space_not_meta() {
        // "mgx" is not a meta command — must be followed by space or end.
        assert!(!is_meta_command(b"mgx foo"));
    }
}