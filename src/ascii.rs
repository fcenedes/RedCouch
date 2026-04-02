//! ASCII (text) protocol handler for the memcached text protocol.
//!
//! Implements all 19 supported ASCII commands.  Entered from
//! `handle_conn()` in `lib.rs` after first-byte protocol detection.

// ── Constants ────────────────────────────────────────────────────────

/// Maximum command line length (bytes before \r\n).
const MAX_LINE_LEN: usize = 2048;

/// Maximum key length (same as binary protocol).
const MAX_KEY_LEN: usize = 250;

/// Version string returned by the `version` command.
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

fn validate_key(key: &[u8]) -> Result<(), String> {
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

/// Encode raw bytes as lowercase hex pairs (for Lua script value arg).
#[cfg(not(test))]
fn hex_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for &b in data {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", b);
    }
    s
}