//! RedCouch – Redis module that bridges Couchbase / Memcached
//!            binary-protocol clients to Redis using a hash-per-item
//!            data model.
//!
//! Build :  cargo build --release
//! Run   :  redis-server --loadmodule ./target/release/libred_couch.dylib

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
    CAS_ZERO,
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
    DetachedFromClient, redis_module, Context, RedisString,
    RedisValue, Status, ThreadSafeContext,
};

/* ============================================================
   Key namespace and system keys
   ========================================================= */

/// Prefix for user item keys in Redis.  Client key `foo` maps to
/// Redis key `rc:foo`.
#[cfg(not(test))]
const KEY_PREFIX: &[u8] = b"rc:";

/// Redis key for the monotonic CAS counter.
#[cfg(not(test))]
const CAS_COUNTER_KEY: &str = "redcouch:sys:cas_counter";

/// Build the namespaced Redis key for a client key.
#[cfg(not(test))]
fn make_redis_key(client_key: &[u8]) -> Vec<u8> {
    let mut rk = Vec::with_capacity(KEY_PREFIX.len() + client_key.len());
    rk.extend_from_slice(KEY_PREFIX);
    rk.extend_from_slice(client_key);
    rk
}

/* ============================================================
   Lua scripts for atomic operations
   ========================================================= */

/// Lua script for SET/ADD/REPLACE with atomic CAS check.
///
/// KEYS[1] = item key, KEYS[2] = CAS counter key
/// ARGV[1] = op ("set"|"add"|"replace")
/// ARGV[2] = value bytes
/// ARGV[3] = flags (decimal string)
/// ARGV[4] = request CAS (decimal string, "0" = skip check)
/// ARGV[5] = expiry (decimal string)
///
/// Returns: {status, new_cas_string}
///   status: 0=OK, -1=NOT_FOUND, -2=KEY_EXISTS
#[cfg(not(test))]
const LUA_STORE: &str = r#"
local op = ARGV[1]
local exists = redis.call('EXISTS', KEYS[1])
if op == 'add' and exists == 1 then return {-2, ''} end
if op == 'replace' and exists == 0 then return {-1, ''} end
local req_cas = ARGV[4]
if req_cas ~= '0' then
  if exists == 0 then return {-1, ''} end
  local stored_cas = redis.call('HGET', KEYS[1], 'c')
  if stored_cas ~= req_cas then return {-2, ''} end
end
local new_cas = redis.call('INCR', KEYS[2])
redis.call('HSET', KEYS[1], 'v', ARGV[2], 'f', ARGV[3], 'c', tostring(new_cas))
local exp = tonumber(ARGV[5])
if exp ~= nil and exp > 0 then
  if exp <= 2592000 then redis.call('EXPIRE', KEYS[1], exp)
  else redis.call('EXPIREAT', KEYS[1], exp) end
elseif exp == 0 and exists == 1 then
  redis.call('PERSIST', KEYS[1])
end
return {0, tostring(new_cas)}
"#;

/// Lua script for GET — returns value, flags, CAS as an array.
///
/// KEYS[1] = item key
///
/// Returns: {status, value, flags_string, cas_string}
///   status: 0=OK, -1=NOT_FOUND
#[cfg(not(test))]
const LUA_GET: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 0 then return {-1, '', '', ''} end
local v = redis.call('HGET', KEYS[1], 'v')
local f = redis.call('HGET', KEYS[1], 'f')
local c = redis.call('HGET', KEYS[1], 'c')
if v == false then v = '' end
if f == false then f = '0' end
if c == false then c = '0' end
return {0, v, f, c}
"#;

/// Lua script for DELETE with CAS check.
///
/// KEYS[1] = item key
/// ARGV[1] = request CAS (decimal string, "0" = skip check)
///
/// Returns: 0=OK, -1=NOT_FOUND, -2=KEY_EXISTS (CAS mismatch)
#[cfg(not(test))]
const LUA_DELETE: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 0 then return -1 end
local req_cas = ARGV[1]
if req_cas ~= '0' then
  local stored_cas = redis.call('HGET', KEYS[1], 'c')
  if stored_cas ~= req_cas then return -2 end
end
redis.call('DEL', KEYS[1])
return 0
"#;

/// Lua script for INCR/DECR with proper u64 semantics.
///
/// KEYS[1] = item key, KEYS[2] = CAS counter key
/// ARGV[1] = delta (decimal string)
/// ARGV[2] = is_decrement ("0"|"1")
/// ARGV[3] = initial value (decimal string)
/// ARGV[4] = expiry (decimal string)
///
/// Returns: {status, value_string, cas_string}
///   status: 0=OK, -1=NOT_FOUND, -3=NON_NUMERIC
#[cfg(not(test))]
const LUA_COUNTER: &str = r#"
local exists = redis.call('EXISTS', KEYS[1])
if exists == 0 then
  local exp = tonumber(ARGV[4])
  if exp == 4294967295 then return {-1, '', ''} end
  local new_cas = redis.call('INCR', KEYS[2])
  local init = ARGV[3]
  redis.call('HSET', KEYS[1], 'v', init, 'f', '0', 'c', tostring(new_cas))
  if exp ~= nil and exp > 0 then
    if exp <= 2592000 then redis.call('EXPIRE', KEYS[1], exp)
    else redis.call('EXPIREAT', KEYS[1], exp) end
  end
  return {0, init, tostring(new_cas)}
end
local val = redis.call('HGET', KEYS[1], 'v')
local num = tonumber(val)
if num == nil then return {-3, '', ''} end
local delta = tonumber(ARGV[1])
if ARGV[2] == '1' then
  if num < delta then num = 0 else num = num - delta end
else
  num = num + delta
end
if num < 0 then num = 0 end
local str_val = string.format('%.0f', num)
local new_cas = redis.call('INCR', KEYS[2])
redis.call('HSET', KEYS[1], 'v', str_val, 'c', tostring(new_cas))
return {0, str_val, tostring(new_cas)}
"#;

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
        eprintln!("[redcouch] listening on 11210");

        for stream in listener.incoming() {
            match stream {
                Ok(mut sock) => {
                    sock.set_nodelay(true).ok();
                    thread::spawn(move || {
                        if let Err(e) = handle_conn(&mut sock) {
                            eprintln!("[redcouch] connection error: {e}");
                        }
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(20));
                }
                Err(e) => {
                    eprintln!("[redcouch] accept error: {e}");
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
                    eprintln!("[redcouch] bad magic byte, closing connection");
                    return Ok(());
                }
                ParseResult::MalformedFrame { opaque, opcode_byte, bytes_to_skip } => {
                    eprintln!("[redcouch] malformed frame (opcode 0x{opcode_byte:02x}), skipping {bytes_to_skip} bytes");
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

/// Helper to check if a RedisValue represents an error (including the
/// `Ok(RedisValue::StaticError(...))` pattern from redis-module 2.0.7
/// when `RedisModule_Call` returns NULL).
#[cfg(not(test))]
fn is_redis_error(v: &RedisValue) -> bool {
    matches!(v, RedisValue::StaticError(_))
}

/// Extract an integer from a Redis EVAL array result element.
#[cfg(not(test))]
fn eval_int(v: &RedisValue) -> i64 {
    match v {
        RedisValue::Integer(n) => *n,
        _ => 0,
    }
}

/// Extract a bulk string from a Redis EVAL array result element.
#[cfg(not(test))]
fn eval_str(v: &RedisValue) -> String {
    match v {
        RedisValue::BulkString(s) => s.clone(),
        RedisValue::BulkRedisString(s) => s.to_string_lossy(),
        RedisValue::SimpleString(s) => s.clone(),
        RedisValue::SimpleStringStatic(s) => (*s).to_string(),
        _ => String::new(),
    }
}

/* ------------ GET / GETQ / GETK / GETKQ ------------ */
#[cfg(not(test))]
fn op_get(req: Request<'_>, sock: &mut TcpStream) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    let rk = make_redis_key(req.key);

    // Use Lua script to atomically fetch value, flags, CAS.
    let reply = with_ctx(|ctx| {
        let args: &[&[u8]] = &[LUA_GET.as_bytes(), b"1", rk.as_slice()];
        ctx.call("EVAL", args)
    }).map_err(|e| BridgeErr::Redis(e.to_string()))?;

    if is_redis_error(&reply) {
        return Err(BridgeErr::Redis(format!("EVAL get error: {reply:?}")));
    }

    // Parse result: {status, value, flags_string, cas_string}
    let fields = match &reply {
        RedisValue::Array(arr) if arr.len() >= 4 => arr,
        _ => {
            return Err(BridgeErr::Redis(format!("EVAL get unexpected: {reply:?}")));
        }
    };

    let status_code = eval_int(&fields[0]);
    if status_code == -1 {
        // NOT_FOUND
        if opcode.is_quiet() {
            return Ok(());
        }
        write_simple_response(sock, opcode, ST_NF, req.hdr.opaque, CAS_ZERO, &[])?;
        return Ok(());
    }

    // Extract value (as raw bytes), flags, CAS.
    let value_bytes: Vec<u8> = match &fields[1] {
        RedisValue::BulkString(s) => s.as_bytes().to_vec(),
        RedisValue::BulkRedisString(s) => s.as_slice().to_vec(),
        RedisValue::SimpleString(s) => s.as_bytes().to_vec(),
        RedisValue::SimpleStringStatic(s) => s.as_bytes().to_vec(),
        other => {
            eprintln!("[redcouch] GET value field unexpected type: {other:?}");
            Vec::new()
        }
    };

    let flags: u32 = eval_str(&fields[2]).parse().unwrap_or(0);
    let cas: u64 = eval_str(&fields[3]).parse().unwrap_or(0);

    let extras = flags.to_be_bytes();
    let key_part = if opcode.includes_key() {
        req.key
    } else {
        &[]
    };

    write_response(
        sock, opcode, ST_OK, req.hdr.opaque, cas,
        &extras, key_part, &value_bytes,
    )?;
    Ok(())
}

/* ------ SET / ADD / REPLACE (and quiet variants) ------ */
#[cfg(not(test))]
fn op_store(req: Request<'_>, sock: &mut TcpStream) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    let base = opcode.base();

    if req.extras.len() != 8 {
        write_simple_response(sock, opcode, ST_ARGS, req.hdr.opaque, CAS_ZERO, &[])?;
        return Ok(());
    }

    let flags = BigEndian::read_u32(&req.extras[0..4]);
    let expiry = BigEndian::read_u32(&req.extras[4..8]);
    let rk = make_redis_key(req.key);

    let op_name = match base {
        Opcode::Set => "set",
        Opcode::Add => "add",
        Opcode::Replace => "replace",
        _ => "set",
    };

    let req_cas = req.hdr.cas.to_string();
    let flags_str = flags.to_string();
    let expiry_str = expiry.to_string();

    // EVAL LUA_STORE 2 <key> <cas_counter> <op> <value> <flags> <cas> <expiry>
    let reply = with_ctx(|ctx| {
        let args: &[&[u8]] = &[
            LUA_STORE.as_bytes(),
            b"2",
            rk.as_slice(),
            CAS_COUNTER_KEY.as_bytes(),
            op_name.as_bytes(),
            req.value,
            flags_str.as_bytes(),
            req_cas.as_bytes(),
            expiry_str.as_bytes(),
        ];
        ctx.call("EVAL", args)
    }).map_err(|e| BridgeErr::Redis(e.to_string()))?;

    if is_redis_error(&reply) {
        return Err(BridgeErr::Redis(format!("EVAL store error: {reply:?}")));
    }

    // Parse the Lua result: {status, new_cas_string}
    let (status_code, new_cas) = match &reply {
        RedisValue::Array(arr) if arr.len() >= 2 => {
            let st = eval_int(&arr[0]);
            let cas_s = eval_str(&arr[1]);
            let cas_val: u64 = cas_s.parse().unwrap_or(0);
            (st, cas_val)
        }
        _ => {
            return Err(BridgeErr::Redis(format!("EVAL store unexpected: {reply:?}")));
        }
    };

    match status_code {
        0 => {
            // Success
            if !opcode.is_quiet() {
                write_response(
                    sock, opcode, ST_OK, req.hdr.opaque, new_cas, &[], &[], &[],
                )?;
            }
        }
        -1 => {
            // NOT_FOUND (REPLACE on missing key, or CAS on missing key)
            write_simple_response(sock, opcode, ST_NF, req.hdr.opaque, CAS_ZERO, &[])?;
        }
        -2 => {
            // KEY_EXISTS (ADD on existing key, or CAS mismatch)
            write_simple_response(sock, opcode, ST_IX, req.hdr.opaque, CAS_ZERO, &[])?;
        }
        _ => {
            return Err(BridgeErr::Redis(format!("EVAL store unknown status: {status_code}")));
        }
    }
    Ok(())
}

/* ------------- DELETE / DELETEQ ------------- */
#[cfg(not(test))]
fn op_delete(req: Request<'_>, sock: &mut TcpStream) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    let rk = make_redis_key(req.key);
    let req_cas = req.hdr.cas.to_string();

    let reply = with_ctx(|ctx| {
        let args: &[&[u8]] = &[
            LUA_DELETE.as_bytes(),
            b"1",
            rk.as_slice(),
            req_cas.as_bytes(),
        ];
        ctx.call("EVAL", args)
    }).map_err(|e| BridgeErr::Redis(e.to_string()))?;

    if is_redis_error(&reply) {
        return Err(BridgeErr::Redis(format!("EVAL delete error: {reply:?}")));
    }

    let status_code = match &reply {
        RedisValue::Integer(n) => *n,
        _ => {
            return Err(BridgeErr::Redis(format!("EVAL delete unexpected: {reply:?}")));
        }
    };

    match status_code {
        0 => {
            // Deleted successfully.
            if opcode.is_quiet() {
                return Ok(());
            }
            // For DELETE success, we don't return the old CAS — just a non-zero one.
            // Use a fresh CAS from the counter for the response.
            let new_cas = get_next_cas();
            write_simple_response(sock, opcode, ST_OK, req.hdr.opaque, new_cas, &[])?;
        }
        -1 => {
            // NOT_FOUND
            write_simple_response(sock, opcode, ST_NF, req.hdr.opaque, CAS_ZERO, &[])?;
        }
        -2 => {
            // KEY_EXISTS (CAS mismatch)
            write_simple_response(sock, opcode, ST_IX, req.hdr.opaque, CAS_ZERO, &[])?;
        }
        _ => {
            return Err(BridgeErr::Redis(format!("EVAL delete unknown status: {status_code}")));
        }
    }
    Ok(())
}

/// Get the next CAS value from the Redis counter.
#[cfg(not(test))]
fn get_next_cas() -> u64 {
    match with_ctx(|ctx| ctx.call("INCR", &[CAS_COUNTER_KEY])) {
        Ok(RedisValue::Integer(n)) => n as u64,
        _ => 1,
    }
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

    let delta   = BigEndian::read_u64(&req.extras[0..8]);
    let initial = BigEndian::read_u64(&req.extras[8..16]);
    let expiry  = BigEndian::read_u32(&req.extras[16..20]);
    let rk = make_redis_key(req.key);

    let is_decr = if base == Opcode::Decrement { "1" } else { "0" };
    let delta_str = delta.to_string();
    let initial_str = initial.to_string();
    let expiry_str = expiry.to_string();

    let reply = with_ctx(|ctx| {
        let args: &[&[u8]] = &[
            LUA_COUNTER.as_bytes(),
            b"2",
            rk.as_slice(),
            CAS_COUNTER_KEY.as_bytes(),
            delta_str.as_bytes(),
            is_decr.as_bytes(),
            initial_str.as_bytes(),
            expiry_str.as_bytes(),
        ];
        ctx.call("EVAL", args)
    }).map_err(|e| BridgeErr::Redis(e.to_string()))?;

    if is_redis_error(&reply) {
        return Err(BridgeErr::Redis(format!("EVAL counter error: {reply:?}")));
    }

    // Parse result: {status, value_string, cas_string}
    let (status_code, value_str, new_cas) = match &reply {
        RedisValue::Array(arr) if arr.len() >= 3 => {
            let st = eval_int(&arr[0]);
            let val = eval_str(&arr[1]);
            let cas_s = eval_str(&arr[2]);
            let cas_val: u64 = cas_s.parse().unwrap_or(0);
            (st, val, cas_val)
        }
        _ => {
            return Err(BridgeErr::Redis(format!("EVAL counter unexpected: {reply:?}")));
        }
    };

    match status_code {
        0 => {
            // Parse the counter value as u64.
            let counter_val: u64 = value_str.parse().unwrap_or(0);
            if !opcode.is_quiet() {
                write_response(
                    sock, opcode, ST_OK, req.hdr.opaque, new_cas,
                    &[], &[], &counter_val.to_be_bytes(),
                )?;
            }
        }
        -1 => {
            // NOT_FOUND (0xFFFFFFFF expiry)
            write_simple_response(sock, opcode, ST_NF, req.hdr.opaque, CAS_ZERO, &[])?;
        }
        -3 => {
            // Non-numeric value
            write_simple_response(
                sock, opcode, ST_ARGS, req.hdr.opaque, CAS_ZERO,
                b"Non-numeric value",
            )?;
        }
        _ => {
            return Err(BridgeErr::Redis(format!("EVAL counter unknown status: {status_code}")));
        }
    }
    Ok(())
}

/* ============================================================
   Module declaration  (allocator + init)
   ========================================================= */

#[cfg(not(test))]
fn module_init(ctx: &Context, _args: &[RedisString]) -> Status {
    LISTENER.call_once(spawn_listener);
    ctx.log_notice("redcouch: listener started on 11210");
    Status::Ok
}

#[cfg(not(test))]
redis_module! {
    name: "redcouch",
    version: 1,
    allocator: (
        redis_module::alloc::RedisAlloc,
        redis_module::alloc::RedisAlloc
    ),
    data_types: [],
    init: module_init,
    commands: [],
}
