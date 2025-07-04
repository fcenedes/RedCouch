//! cbbridge – Redis module that lets Couchbase / Memcached
//!            binary-protocol clients talk to RedisJSON.
//!
//! Build :  cargo build --release
//! Run   :  redis-stack-server --loadmodule ./target/release/libcbbridge.so

#![forbid(unsafe_code)]
#![allow(clippy::needless_return)]

use byteorder::{BigEndian, ByteOrder};
use bytes::{Buf, BytesMut};
use redis_module::{DetachedFromClient, redis_module, Context, RedisString, Status, ThreadSafeContext, RedisValue, RedisError};
use std::{
    convert::TryInto,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::Once,
    thread,
    time::Duration,
};

/* ============================================================
   1.  Binary-protocol constants
   ========================================================= */

const MAGIC_REQ: u8 = 0x80;
const MAGIC_RES: u8 = 0x81;

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq)]
enum Opcode {
    Get = 0x00,
    GetQ = 0x09,
    GetK = 0x0c,
    GetKQ = 0x0d,
    Set = 0x01,
    Add = 0x02,
    Replace = 0x03,
    Delete = 0x04,
    Increment = 0x05,
    Decrement = 0x06,
    Quit = 0x07,
    Noop = 0x0a,
}
impl Opcode {
    fn parse(b: u8) -> Option<Self> {
        use Opcode::*;
        Some(match b {
            0x00 => Get,
            0x09 => GetQ,
            0x0c => GetK,
            0x0d => GetKQ,
            0x01 => Set,
            0x02 => Add,
            0x03 => Replace,
            0x04 => Delete,
            0x05 => Increment,
            0x06 => Decrement,
            0x07 => Quit,
            0x0a => Noop,
            _ => return None,
        })
    }
}

/* status codes we emit */
const ST_OK: u16 = 0x0000;
const ST_NF: u16 = 0x0001;
const ST_IX: u16 = 0x0002;
const ST_ARGS: u16 = 0x0004;
const ST_UNK: u16 = 0x0081;

/* ============================================================
   2.  Header + request parsing helpers
   ========================================================= */

#[derive(Debug)]
struct Header {
    opcode: Opcode,
    key_len: u16,
    extras_len: u8,
    body_len: u32,
    opaque: u32,
}
impl Header {
    fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < 24 || buf[0] != MAGIC_REQ {
            return None;
        }
        Some(Self {
            opcode: Opcode::parse(buf[1])?,
            key_len: BigEndian::read_u16(&buf[2..4]),
            extras_len: buf[4],
            body_len: BigEndian::read_u32(&buf[8..12]),
            opaque: BigEndian::read_u32(&buf[12..16]),
        })
    }
}

struct Request<'a> {
    hdr: Header,
    extras: &'a [u8],
    key: &'a [u8],
    value: &'a [u8],
}

fn parse_req(buf: &[u8]) -> Option<(Request<'_>, usize)> {
    let hdr = Header::parse(buf)?;
    let total = 24 + hdr.body_len as usize;
    if buf.len() < total {
        return None;
    }

    let extras_end = 24 + hdr.extras_len as usize;
    let key_end = extras_end + hdr.key_len as usize;

    Some((
        Request {
            hdr,
            extras: &buf[24..extras_end],
            key: &buf[extras_end..key_end],
            value: &buf[key_end..total],
        },
        total,
    ))
}

/* ============================================================
   3.  TCP listener (started once from module_init)
   ========================================================= */

static LISTENER: Once = Once::new();

fn spawn_listener() {
    thread::spawn(|| {
        let listener =
            TcpListener::bind("0.0.0.0:11210").expect("bind :11210 (binary protocol)");
        listener.set_nonblocking(true).ok();
        eprintln!("[cbbridge] listening on 11210");

        for stream in listener.incoming() {
            match stream {
                Ok(mut sock) => {
                    sock.set_nodelay(true).ok();
                    thread::spawn(move || {
                        if let Err(e) = handle_conn(&mut sock) {
                            eprintln!("[cbbridge] connection error: {e}");
                        }
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(20));
                }
                Err(e) => {
                    eprintln!("[cbbridge] accept error: {e}");
                    break;
                }
            }
        }
    });
}

/* ============================================================
   4.  Connection state machine
   ========================================================= */

#[derive(thiserror::Error, Debug)]
enum BridgeErr {
    #[error("io {0}")]
    Io(#[from] std::io::Error),
    #[error("utf8")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("redis {0}")]
    Redis(String),
}
type Br<T> = Result<T, BridgeErr>;

fn handle_conn(sock: &mut TcpStream) -> Br<()> {
    let mut buf = BytesMut::with_capacity(4096);

    loop {
        let mut tmp = [0u8; 4096];
        match sock.read(&mut tmp) {
            Ok(0) => return Ok(()), // EOF
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => return Err(e.into()),
        }

        while let Some((req, used)) = parse_req(&buf) {
            handle(req, sock)?;
            buf.advance(used);
        }
    }
}

/* ============================================================
   5.  Command handlers
   ========================================================= */

fn handle(req: Request<'_>, sock: &mut TcpStream) -> Br<()> {
    use Opcode::*;
    match req.hdr.opcode {
        Get | GetK | GetQ | GetKQ => op_get(req, sock),
        Set | Add | Replace => op_store(req, sock),
        Delete => op_delete(req, sock),
        Increment | Decrement => op_counter(req, sock),
        Noop => send_simple(sock, &req, ST_OK, &[]),
        Quit => {
            send_simple(sock, &req, ST_OK, &[])?;
            Err(std::io::Error::from(std::io::ErrorKind::ConnectionAborted).into())
        }
    }
}

/* ------------ helpers to run a Redis command -------------- */
fn with_ctx<T>(f: impl Fn(&Context) -> T) -> T {
    let tsc = ThreadSafeContext::<DetachedFromClient>::new();
    let guard = tsc.lock();
    f(&guard)
}

/* ------------ GET ------------ */
/* ------------ GET / GETQ / GETK / GETKQ ------------ */
fn op_get(
    req: Request<'_>,
    sock: &mut TcpStream,
) -> Br<()> {
    use Opcode::*;
    let key = std::str::from_utf8(req.key)?;
    let reply = with_ctx(|ctx| ctx.call("JSON.GET", &[key]))
        .map_err(|e| BridgeErr::Redis(e.to_string()))?;

    // -----------------------------------------------------------------
    // 1.  Handle cache-miss + “quiet” variants
    // -----------------------------------------------------------------
    let is_quiet = matches!(req.hdr.opcode, GetQ | GetKQ);
    if matches!(reply, RedisValue::Null) {
        if is_quiet {
            return Ok(());                   // quiet miss ⇒ no reply
        } else {
            return send_simple(sock, &req, ST_NF, &[]);
        }
    }

    // -----------------------------------------------------------------
    // 2.  Hit  → build extras(4B flags=0) + optional key + value
    // -----------------------------------------------------------------
    let json: String = reply.try_into().map_err(|e: RedisError| BridgeErr::Redis(e.to_string()))?;

    let mut body = Vec::with_capacity(4 + req.key.len() + json.len());
    body.extend_from_slice(&0u32.to_be_bytes());          // flags
    if matches!(req.hdr.opcode, GetK | GetKQ) {
        body.extend_from_slice(req.key);                  // key (for *K opcodes)
    }
    body.extend_from_slice(json.as_bytes());              // value

    let extras_len = 4;
    let key_len_hdr = if matches!(req.hdr.opcode, GetK | GetKQ) {
        req.key.len() as u16
    } else {
        0
    };

    send_full(sock, &req, ST_OK, extras_len, key_len_hdr, &body)
}

/* ------ SET / ADD / REPLACE ------ */
fn op_store(req: Request<'_>, sock: &mut TcpStream) -> Br<()> {
    if req.extras.len() != 8 {
        return send_simple(sock, &req, ST_ARGS, &[]);
    }

    let expiry = BigEndian::read_u32(&req.extras[4..8]);
    let key = std::str::from_utf8(req.key)?;
    let val = std::str::from_utf8(req.value)?;

    let exists = match with_ctx(|ctx| ctx.call("EXISTS", &[key])) {
        Ok(RedisValue::Integer(n)) => n,
        Ok(other) => {
            eprintln!("[cbbridge] EXISTS returned unexpected value: {other:?}");
            0
        }
        Err(e) => return Err(BridgeErr::Redis(e.to_string())),
    };

    use Opcode::*;
    let (do_it, err_if_skip) = match req.hdr.opcode {
        Set => (true, ST_OK),
        Add => (exists == 0, ST_IX),
        Replace => (exists != 0, ST_NF),
        _ => (false, ST_UNK),
    };
    if !do_it {
        return send_simple(sock, &req, err_if_skip, &[]);
    }

    with_ctx(|ctx| ctx.call("JSON.SET", &[key, "$", val]))
        .map_err(|e| BridgeErr::Redis(e.to_string()))?;
    if expiry != 0 {
        with_ctx(|ctx| ctx.call("EXPIRE", &[key, &expiry.to_string()])).ok();
    }
    send_full(sock, &req, ST_OK, 0, 0, &[])
}

/* ------------- DELETE ------------- */
fn op_delete(req: Request<'_>, sock: &mut TcpStream) -> Br<()> {
    let key = std::str::from_utf8(req.key)?;
    let deleted = match with_ctx(|ctx| ctx.call("DEL", &[key])) {
        Ok(RedisValue::Integer(n)) => n,
        Ok(other) => {
            eprintln!("[cbbridge] DEL returned unexpected value: {other:?}");
            0
        }
        Err(e) => return Err(BridgeErr::Redis(e.to_string())),
    };
    send_simple(
        sock,
        &req,
        if deleted == 1 { ST_OK } else { ST_NF },
        &[],
    )
}

/* --------- INCR / DECR --------- */
fn op_counter(
    req: Request<'_>,
    sock: &mut TcpStream,
) -> Br<()> {
    // Extras = delta(8) + initial(8) + expiry(4)  ---> 20 bytes
    if req.extras.len() != 20 {
        return send_simple(sock, &req, ST_ARGS, &[]);
    }

    let delta   = BigEndian::read_u64(&req.extras[0..8])  as i64;
    let initial = BigEndian::read_u64(&req.extras[8..16]) as i64;
    let expiry  = BigEndian::read_u32(&req.extras[16..20]);
    let key     = std::str::from_utf8(req.key)?;

    /* decrement == negative delta */
    let signed = if req.hdr.opcode == Opcode::Decrement {
        -(delta as i64)
    } else {
        delta as i64
    };

    /* ------------------------------------------------------------------
       Try JSON.NUMINCRBY.  If the key / path is missing RedisJSON returns
       Null; we then seed the document with the initial value.
       ------------------------------------------------------------------ */
    let new_val: i64 = match with_ctx(|ctx| {
        ctx.call("JSON.NUMINCRBY", &[key, "$", &signed.to_string()])
    }) {
        /* key existed → parse new value */
        Ok(redis_module::RedisValue::Null) => {
            // seed and (optionally) set TTL
            with_ctx(|ctx| ctx.call("JSON.SET", &[key, "$", &initial.to_string()])).ok();
            if expiry != 0 {
                with_ctx(|ctx| ctx.call("EXPIRE", &[key, &expiry.to_string()])).ok();
            }
            initial
        }
        Ok(val) => {
            // NUMINCRBY returns the number as a JSON string (e.g. "42")
            let s: String = val.try_into().map_err(|e:RedisError| BridgeErr::Redis(e.to_string()))?;
            s.parse().unwrap_or(0)
        }
        Err(e) => return Err(BridgeErr::Redis(e.to_string())),
    };

    /* build and send response: extras=0, keylen=0, body=8-byte counter */
    send_full(sock, &req, ST_OK, 0, 0, &new_val.to_be_bytes())
}

/* ============================================================
   6.  Reply helpers
   ========================================================= */

fn send_simple(
    sock: &mut TcpStream,
    req: &Request<'_>,
    status: u16,
    body: &[u8],
) -> Br<()> {
    send_full(sock, req, status, 0, 0, body)
}

fn send_full(
    sock: &mut TcpStream,
    req: &Request<'_>,
    status: u16,
    extras_len: u8,
    key_len: u16,
    body: &[u8],
) -> Br<()> {
    let total_body = extras_len as u32 + key_len as u32 + body.len() as u32;

    let mut hdr = [0u8; 24];
    hdr[0] = MAGIC_RES;
    hdr[1] = req.hdr.opcode as u8;
    BigEndian::write_u16(&mut hdr[2..4], key_len);
    hdr[4] = extras_len;
    BigEndian::write_u16(&mut hdr[6..8], status);
    BigEndian::write_u32(&mut hdr[8..12], total_body);
    BigEndian::write_u32(&mut hdr[12..16], req.hdr.opaque);
    BigEndian::write_u64(&mut hdr[16..24], 1); // dummy CAS

    sock.write_all(&hdr)?;
    sock.write_all(body)?;
    Ok(())
}

/* ============================================================
   7.  Module declaration  (allocator + init)
   ========================================================= */

fn module_init(ctx: &Context, _args: &[RedisString]) -> Status {
    LISTENER.call_once(spawn_listener);
    ctx.log_notice("cbbridge: listener started on 11210");
    Status::Ok
}

redis_module! {
    name: "cbbridge",
    version: 1,
    allocator: (
        redis_module::alloc::RedisAlloc,
        redis_module::alloc::RedisAlloc
    ),
    data_types: [],
    init: module_init,
    commands: [],
}