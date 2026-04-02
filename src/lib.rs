//! cbbridge – Redis module that lets Couchbase / Memcached
//!            binary-protocol clients talk to RedisJSON.
//!
//! Build :  cargo build --release
//! Run   :  redis-stack-server --loadmodule ./target/release/libcbbridge.so

#![forbid(unsafe_code)]
#![allow(clippy::needless_return)]

pub mod protocol;

#[cfg(not(test))]
use byteorder::{BigEndian, ByteOrder};
#[cfg(not(test))]
use bytes::{Buf, BytesMut};
#[cfg(not(test))]
use protocol::{
    Opcode, Request, try_parse_request, ParseResult,
    write_response, write_simple_response, write_error_for_raw_opcode,
    ST_OK, ST_NF, ST_IX, ST_ARGS, ST_UNK,
    CAS_ZERO, CAS_PLACEHOLDER,
};
#[cfg(not(test))]
use std::{
    io::Read,
    net::{TcpListener, TcpStream},
    sync::Once,
    thread,
    time::Duration,
};

// Redis-module imports are only needed when building the actual module,
// not during `cargo test`.
#[cfg(not(test))]
use redis_module::{
    DetachedFromClient, redis_module, Context, RedisError, RedisString,
    RedisValue, Status, ThreadSafeContext,
};

/* ============================================================
   TCP listener (started once from module_init)
   ========================================================= */

#[cfg(not(test))]
static LISTENER: Once = Once::new();

#[cfg(not(test))]
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
   Connection state machine
   ========================================================= */

#[cfg(not(test))]
#[derive(thiserror::Error, Debug)]
enum BridgeErr {
    #[error("io {0}")]
    Io(#[from] std::io::Error),
    #[error("utf8")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("redis {0}")]
    Redis(String),
}
#[cfg(not(test))]
type Br<T> = Result<T, BridgeErr>;

#[cfg(not(test))]
fn handle_conn(sock: &mut TcpStream) -> Br<()> {
    let mut buf = BytesMut::with_capacity(4096);

    loop {
        let mut tmp = [0u8; 4096];
        match sock.read(&mut tmp) {
            Ok(0) => return Ok(()),
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => return Err(e.into()),
        }

        loop {
            match try_parse_request(&buf) {
                ParseResult::Ok((req, used)) => {
                    handle(req, sock)?;
                    buf.advance(used);
                }
                ParseResult::Incomplete => break,
                ParseResult::BadMagic => {
                    eprintln!("[cbbridge] bad magic byte, closing connection");
                    return Ok(());
                }
                ParseResult::MalformedFrame { opaque, opcode_byte, bytes_to_skip } => {
                    eprintln!("[cbbridge] malformed frame (opcode 0x{opcode_byte:02x}), skipping {bytes_to_skip} bytes");
                    write_error_for_raw_opcode(sock, opcode_byte, ST_ARGS, opaque, b"Malformed frame")?;
                    buf.advance(bytes_to_skip);
                }
            }
        }
    }
}

/* ============================================================
   Command handlers
   ========================================================= */

#[cfg(not(test))]
fn handle(req: Request<'_>, sock: &mut TcpStream) -> Br<()> {
    let opcode = match req.hdr.opcode {
        Some(op) => op,
        None => {
            // Unknown opcode — respond with ST_UNK and continue.
            write_error_for_raw_opcode(
                sock,
                req.hdr.opcode_byte,
                ST_UNK,
                req.hdr.opaque,
                b"Unknown command",
            )?;
            return Ok(());
        }
    };

    use Opcode::*;
    match opcode.base() {
        Get | GetK => op_get(req, sock),
        Set | Add | Replace => op_store(req, sock),
        Delete => op_delete(req, sock),
        Increment | Decrement => op_counter(req, sock),
        Noop => {
            write_simple_response(sock, opcode, ST_OK, req.hdr.opaque, CAS_ZERO, &[])?;
            Ok(())
        }
        Quit => {
            if !opcode.is_quiet() {
                write_simple_response(sock, opcode, ST_OK, req.hdr.opaque, CAS_ZERO, &[])?;
            }
            Err(std::io::Error::from(std::io::ErrorKind::ConnectionAborted).into())
        }
        // base() maps all quiet variants to their loud base, so the
        // remaining arms are unreachable.
        _ => {
            write_error_for_raw_opcode(
                sock, req.hdr.opcode_byte, ST_UNK, req.hdr.opaque,
                b"Unknown command",
            )?;
            Ok(())
        }
    }
}

/* ------------ helpers to run a Redis command -------------- */
#[cfg(not(test))]
fn with_ctx<T>(f: impl Fn(&redis_module::Context) -> T) -> T {
    let tsc = ThreadSafeContext::<DetachedFromClient>::new();
    let guard = tsc.lock();
    f(&guard)
}

/* ------------ GET / GETQ / GETK / GETKQ ------------ */
#[cfg(not(test))]
fn op_get(req: Request<'_>, sock: &mut TcpStream) -> Br<()> {
    // Safety: callers only reach here when opcode is Some.
    let opcode = req.hdr.opcode.unwrap();
    let key = std::str::from_utf8(req.key)?;
    let reply = with_ctx(|ctx| ctx.call("JSON.GET", &[key]))
        .map_err(|e| BridgeErr::Redis(e.to_string()))?;

    if matches!(reply, RedisValue::Null) {
        if opcode.is_quiet() {
            // Quiet GET variants suppress miss responses.
            return Ok(());
        } else {
            write_simple_response(sock, opcode, ST_NF, req.hdr.opaque, CAS_ZERO, &[])?;
            return Ok(());
        }
    }

    let json: String =
        reply.try_into().map_err(|e: RedisError| BridgeErr::Redis(e.to_string()))?;

    let extras = 0u32.to_be_bytes();
    let key_part = if opcode.includes_key() {
        req.key
    } else {
        &[]
    };

    write_response(
        sock,
        opcode,
        ST_OK,
        req.hdr.opaque,
        CAS_PLACEHOLDER,
        &extras,
        key_part,
        json.as_bytes(),
    )?;
    Ok(())
}

/* ------ SET / ADD / REPLACE (and quiet variants) ------ */
#[cfg(not(test))]
fn op_store(req: Request<'_>, sock: &mut TcpStream) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    let base = opcode.base();

    if req.extras.len() != 8 {
        // Always send error even for quiet variants.
        write_simple_response(sock, opcode, ST_ARGS, req.hdr.opaque, CAS_ZERO, &[])?;
        return Ok(());
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

    let (do_it, err_if_skip) = match base {
        Opcode::Set => (true, ST_OK),
        Opcode::Add => (exists == 0, ST_IX),
        Opcode::Replace => (exists != 0, ST_NF),
        _ => (false, ST_UNK),
    };
    if !do_it {
        // Error responses are always sent, even for quiet variants.
        write_simple_response(sock, opcode, err_if_skip, req.hdr.opaque, CAS_ZERO, &[])?;
        return Ok(());
    }

    with_ctx(|ctx| ctx.call("JSON.SET", &[key, "$", val]))
        .map_err(|e| BridgeErr::Redis(e.to_string()))?;
    if expiry != 0 {
        with_ctx(|ctx| ctx.call("EXPIRE", &[key, &expiry.to_string()])).ok();
    }

    // Quiet variants suppress success responses.
    if !opcode.is_quiet() {
        write_response(sock, opcode, ST_OK, req.hdr.opaque, CAS_PLACEHOLDER, &[], &[], &[])?;
    }
    Ok(())
}

/* ------------- DELETE / DELETEQ ------------- */
#[cfg(not(test))]
fn op_delete(req: Request<'_>, sock: &mut TcpStream) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    let key = std::str::from_utf8(req.key)?;
    let deleted = match with_ctx(|ctx| ctx.call("DEL", &[key])) {
        Ok(RedisValue::Integer(n)) => n,
        Ok(other) => {
            eprintln!("[cbbridge] DEL returned unexpected value: {other:?}");
            0
        }
        Err(e) => return Err(BridgeErr::Redis(e.to_string())),
    };

    let status = if deleted == 1 { ST_OK } else { ST_NF };
    // Quiet variants suppress success; errors are always sent.
    if opcode.is_quiet() && status == ST_OK {
        return Ok(());
    }
    write_simple_response(sock, opcode, status, req.hdr.opaque, CAS_ZERO, &[])?;
    Ok(())
}

/* --------- INCR / DECR (and quiet variants) --------- */
#[cfg(not(test))]
fn op_counter(req: Request<'_>, sock: &mut TcpStream) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    let base = opcode.base();

    if req.extras.len() != 20 {
        write_simple_response(sock, opcode, ST_ARGS, req.hdr.opaque, CAS_ZERO, &[])?;
        return Ok(());
    }

    let delta   = BigEndian::read_u64(&req.extras[0..8]) as i64;
    let initial = BigEndian::read_u64(&req.extras[8..16]) as i64;
    let expiry  = BigEndian::read_u32(&req.extras[16..20]);
    let key     = std::str::from_utf8(req.key)?;

    let signed = if base == Opcode::Decrement {
        -(delta as i64)
    } else {
        delta as i64
    };

    let new_val: i64 = match with_ctx(|ctx| {
        ctx.call("JSON.NUMINCRBY", &[key, "$", &signed.to_string()])
    }) {
        Ok(redis_module::RedisValue::Null) => {
            with_ctx(|ctx| ctx.call("JSON.SET", &[key, "$", &initial.to_string()])).ok();
            if expiry != 0 {
                with_ctx(|ctx| ctx.call("EXPIRE", &[key, &expiry.to_string()])).ok();
            }
            initial
        }
        Ok(val) => {
            let s: String = val.try_into().map_err(|e: RedisError| BridgeErr::Redis(e.to_string()))?;
            s.parse().unwrap_or(0)
        }
        Err(e) => return Err(BridgeErr::Redis(e.to_string())),
    };

    // Quiet variants suppress success responses.
    if !opcode.is_quiet() {
        write_response(
            sock,
            opcode,
            ST_OK,
            req.hdr.opaque,
            CAS_PLACEHOLDER,
            &[],
            &[],
            &new_val.to_be_bytes(),
        )?;
    }
    Ok(())
}

/* ============================================================
   Module declaration  (allocator + init)
   ========================================================= */

#[cfg(not(test))]
fn module_init(ctx: &Context, _args: &[RedisString]) -> Status {
    LISTENER.call_once(spawn_listener);
    ctx.log_notice("cbbridge: listener started on 11210");
    Status::Ok
}

#[cfg(not(test))]
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
