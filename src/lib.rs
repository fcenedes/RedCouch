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
    ST_OK, ST_NF, ST_IX, ST_ARGS, ST_NOT_STORED, ST_UNK,
    CAS_ZERO, MAX_BODY_LEN,
};
#[cfg(not(test))]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(test))]
use std::time::Instant;
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

/* ============================================================
   Runtime stats counters
   ========================================================= */

#[cfg(not(test))]
static STAT_CMD_GET: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_CMD_SET: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_CMD_FLUSH: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_CMD_TOUCH: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_GET_HITS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_GET_MISSES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_DELETE_HITS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_DELETE_MISSES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_INCR_HITS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_INCR_MISSES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_DECR_HITS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_DECR_MISSES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_CAS_HITS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_CAS_MISSES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_CAS_BADVAL: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_CURR_CONNECTIONS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_TOTAL_CONNECTIONS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_AUTH_CMDS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STAT_AUTH_ERRORS: AtomicU64 = AtomicU64::new(0);

/// Module startup time — set in `module_init`.
#[cfg(not(test))]
static STARTUP_INSTANT: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Build the namespaced Redis key for a client key.
#[cfg(not(test))]
fn make_redis_key(client_key: &[u8]) -> Vec<u8> {
    let mut rk = Vec::with_capacity(KEY_PREFIX.len() + client_key.len());
    rk.extend_from_slice(KEY_PREFIX);
    rk.extend_from_slice(client_key);
    rk
}

/// Decode a hex string (pairs of hex digits) into raw bytes.
/// Returns an empty Vec if the input is not valid hex.
#[cfg(not(test))]
fn hex_decode(hex: &str) -> Vec<u8> {
    if hex.len() % 2 != 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    for i in (0..bytes.len()).step_by(2) {
        let hi = match bytes[i] {
            b'0'..=b'9' => bytes[i] - b'0',
            b'a'..=b'f' => bytes[i] - b'a' + 10,
            b'A'..=b'F' => bytes[i] - b'A' + 10,
            _ => return Vec::new(),
        };
        let lo = match bytes[i + 1] {
            b'0'..=b'9' => bytes[i + 1] - b'0',
            b'a'..=b'f' => bytes[i + 1] - b'a' + 10,
            b'A'..=b'F' => bytes[i + 1] - b'A' + 10,
            _ => return Vec::new(),
        };
        out.push((hi << 4) | lo);
    }
    out
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

/// Lua script for GET — returns value as hex-encoded string to avoid
/// redis-module UTF-8 conversion panics on binary payloads.
///
/// KEYS[1] = item key
///
/// Returns: {status, hex_value, flags_string, cas_string}
///   status: 0=OK, -1=NOT_FOUND
///   hex_value: value bytes encoded as lowercase hex pairs
#[cfg(not(test))]
const LUA_GET: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 0 then return {-1, '', '', ''} end
local v = redis.call('HGET', KEYS[1], 'v')
local f = redis.call('HGET', KEYS[1], 'f')
local c = redis.call('HGET', KEYS[1], 'c')
if v == false then v = '' end
if f == false then f = '0' end
if c == false then c = '0' end
local hex = (v:gsub('.', function(ch) return string.format('%02x', string.byte(ch)) end))
return {0, hex, f, c}
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

/// Lua script for INCR/DECR with u64 semantics.
///
/// KEYS[1] = item key, KEYS[2] = CAS counter key
/// ARGV[1] = delta (decimal string)
/// ARGV[2] = is_decrement ("0"|"1")
/// ARGV[3] = initial value (decimal string)
/// ARGV[4] = expiry (decimal string)
///
/// Precision note: Lua 5.1 (embedded in Redis) uses IEEE 754 double-precision
/// floats, so integers above 2^53 (9007199254740992) lose precision.  Counter
/// values within [0, 2^53) are exact.  Values at or above 2^53 may round;
/// the module does NOT attempt string-based big-integer math in Lua because
/// the performance and complexity tradeoffs are not justified for GA.
/// Overflow past the Lua precision boundary wraps modulo the double
/// representation.  Underflow on DECR clamps to 0.
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

/// Lua script for TOUCH — update expiry on an existing key.
///
/// KEYS[1] = item key, KEYS[2] = CAS counter key
/// ARGV[1] = expiry (decimal string)
///
/// Returns: {status, cas_string}
///   status: 0=OK, -1=NOT_FOUND
#[cfg(not(test))]
const LUA_TOUCH: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 0 then return {-1, ''} end
local exp = tonumber(ARGV[1])
if exp ~= nil and exp > 0 then
  if exp <= 2592000 then redis.call('EXPIRE', KEYS[1], exp)
  else redis.call('EXPIREAT', KEYS[1], exp) end
elseif exp == 0 then
  redis.call('PERSIST', KEYS[1])
end
local new_cas = redis.call('INCR', KEYS[2])
redis.call('HSET', KEYS[1], 'c', tostring(new_cas))
return {0, tostring(new_cas)}
"#;

/// Lua script for GAT (Get And Touch) — fetch value and update expiry atomically.
///
/// KEYS[1] = item key, KEYS[2] = CAS counter key
/// ARGV[1] = expiry (decimal string)
///
/// Returns: {status, hex_value, flags_string, cas_string}
///   status: 0=OK, -1=NOT_FOUND
#[cfg(not(test))]
const LUA_GAT: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 0 then return {-1, '', '', ''} end
local exp = tonumber(ARGV[1])
if exp ~= nil and exp > 0 then
  if exp <= 2592000 then redis.call('EXPIRE', KEYS[1], exp)
  else redis.call('EXPIREAT', KEYS[1], exp) end
elseif exp == 0 then
  redis.call('PERSIST', KEYS[1])
end
local new_cas = redis.call('INCR', KEYS[2])
redis.call('HSET', KEYS[1], 'c', tostring(new_cas))
local v = redis.call('HGET', KEYS[1], 'v')
local f = redis.call('HGET', KEYS[1], 'f')
if v == false then v = '' end
if f == false then f = '0' end
local hex = (v:gsub('.', function(ch) return string.format('%02x', string.byte(ch)) end))
return {0, hex, f, tostring(new_cas)}
"#;

/// Lua script for APPEND — append data to existing value.
///
/// KEYS[1] = item key, KEYS[2] = CAS counter key
/// ARGV[1] = data to append
/// ARGV[2] = request CAS (decimal string, "0" = skip check)
///
/// Returns: {status, cas_string}
///   status: 0=OK, -1=NOT_FOUND, -2=KEY_EXISTS (CAS mismatch)
#[cfg(not(test))]
const LUA_APPEND: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 0 then return {-5, ''} end
local req_cas = ARGV[2]
if req_cas ~= '0' then
  local stored_cas = redis.call('HGET', KEYS[1], 'c')
  if stored_cas ~= req_cas then return {-2, ''} end
end
local old_v = redis.call('HGET', KEYS[1], 'v')
if old_v == false then old_v = '' end
local new_v = old_v .. ARGV[1]
local new_cas = redis.call('INCR', KEYS[2])
redis.call('HSET', KEYS[1], 'v', new_v, 'c', tostring(new_cas))
return {0, tostring(new_cas)}
"#;

/// Lua script for PREPEND — prepend data to existing value.
///
/// KEYS[1] = item key, KEYS[2] = CAS counter key
/// ARGV[1] = data to prepend
/// ARGV[2] = request CAS (decimal string, "0" = skip check)
///
/// Returns: {status, cas_string}
///   status: 0=OK, -1=NOT_FOUND, -2=KEY_EXISTS (CAS mismatch)
#[cfg(not(test))]
const LUA_PREPEND: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 0 then return {-5, ''} end
local req_cas = ARGV[2]
if req_cas ~= '0' then
  local stored_cas = redis.call('HGET', KEYS[1], 'c')
  if stored_cas ~= req_cas then return {-2, ''} end
end
local old_v = redis.call('HGET', KEYS[1], 'v')
if old_v == false then old_v = '' end
local new_v = ARGV[1] .. old_v
local new_cas = redis.call('INCR', KEYS[2])
redis.call('HSET', KEYS[1], 'v', new_v, 'c', tostring(new_cas))
return {0, tostring(new_cas)}
"#;


/* ============================================================
   TCP listener (started once from module_init)
   ========================================================= */

/// Default bind address — loopback only to avoid accidental public
/// exposure.  Override via module args if needed in future.
#[cfg(not(test))]
const DEFAULT_BIND_ADDR: &str = "127.0.0.1:11210";

/// Maximum number of concurrent client connections.  Beyond this limit
/// new connections are accepted and immediately closed with an error log.
#[cfg(not(test))]
const MAX_CONNECTIONS: u64 = 1024;

/// Socket read timeout — how long a connection can be idle before being
/// closed.  30 seconds is generous for interactive memcached workloads.
#[cfg(not(test))]
const SOCKET_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Socket write timeout — prevents a stuck client from blocking a thread.
#[cfg(not(test))]
const SOCKET_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum read buffer size per connection.  If the buffer grows beyond
/// this without producing a complete frame, the connection is closed.
/// This prevents a slow-drip attack from consuming unbounded memory.
#[cfg(not(test))]
const MAX_READ_BUF: usize = (MAX_BODY_LEN as usize) + protocol::HEADER_LEN + 4096;

/// Connection rejection counter.
#[cfg(not(test))]
static STAT_REJECTED_CONNECTIONS: AtomicU64 = AtomicU64::new(0);

#[cfg(not(test))]
static LISTENER: Once = Once::new();

#[cfg(not(test))]
fn spawn_listener() {
    thread::spawn(|| {
        let listener = match TcpListener::bind(DEFAULT_BIND_ADDR) {
            Ok(l) => l,
            Err(e) => {
                eprintln!(
                    "[redcouch] FATAL: cannot bind {DEFAULT_BIND_ADDR}: {e}  \
                     (is another instance already running?)"
                );
                return;
            }
        };
        // Use blocking accept — avoids busy-wait polling.
        listener.set_nonblocking(false).ok();
        eprintln!("[redcouch] listening on {DEFAULT_BIND_ADDR} (max_connections={MAX_CONNECTIONS})");

        for stream in listener.incoming() {
            match stream {
                Ok(mut sock) => {
                    // Enforce connection limit.
                    let current = STAT_CURR_CONNECTIONS.load(Ordering::Relaxed);
                    if current >= MAX_CONNECTIONS {
                        STAT_REJECTED_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
                        eprintln!("[redcouch] connection limit reached ({MAX_CONNECTIONS}), rejecting");
                        // Drop the socket immediately — client sees connection reset.
                        drop(sock);
                        continue;
                    }

                    sock.set_nodelay(true).ok();
                    sock.set_read_timeout(Some(SOCKET_READ_TIMEOUT)).ok();
                    sock.set_write_timeout(Some(SOCKET_WRITE_TIMEOUT)).ok();
                    STAT_TOTAL_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
                    STAT_CURR_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
                    thread::spawn(move || {
                        if let Err(e) = handle_conn(&mut sock) {
                            // Don't log read timeouts as errors — they are expected
                            // for idle connections.
                            if let BridgeErr::Io(ref io_err) = e {
                                if io_err.kind() == std::io::ErrorKind::WouldBlock
                                    || io_err.kind() == std::io::ErrorKind::TimedOut
                                {
                                    // Normal idle timeout, suppress log.
                                } else {
                                    eprintln!("[redcouch] connection error: {e}");
                                }
                            } else {
                                eprintln!("[redcouch] connection error: {e}");
                            }
                        }
                        STAT_CURR_CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
                    });
                }
                Err(e) => {
                    eprintln!("[redcouch] accept error: {e}");
                    // Transient accept errors (e.g. fd exhaustion) — sleep
                    // briefly and retry rather than killing the listener.
                    thread::sleep(Duration::from_millis(100));
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
    let mut buf = BytesMut::with_capacity(16384);
    // Response write buffer — all responses for a batch of parsed requests
    // are collected here and flushed in a single write_all() call, reducing
    // the number of syscalls from O(responses * 4) to O(1) per read cycle.
    let mut out = Vec::with_capacity(8192);

    loop {
        // Guard against unbounded buffer growth.
        if buf.len() > MAX_READ_BUF {
            eprintln!("[redcouch] read buffer exceeded {MAX_READ_BUF} bytes, closing connection");
            return Ok(());
        }

        let mut tmp = [0u8; 16384];
        match sock.read(&mut tmp) {
            Ok(0) => return Ok(()),
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // Socket read timeout — close the idle connection.
                return Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into());
            }
            Err(e) => return Err(e.into()),
        }

        loop {
            match try_parse_request(&buf) {
                ParseResult::Ok((req, used)) => {
                    handle(req, &mut out)?;
                    buf.advance(used);
                }
                ParseResult::Incomplete => break,
                ParseResult::BadMagic => {
                    eprintln!("[redcouch] bad magic byte, closing connection");
                    return Ok(());
                }
                ParseResult::MalformedFrame { opaque, opcode_byte, bytes_to_skip } => {
                    eprintln!("[redcouch] malformed frame (opcode 0x{opcode_byte:02x}), skipping {bytes_to_skip} bytes");
                    write_error_for_raw_opcode(&mut out, opcode_byte, ST_ARGS, opaque, b"Malformed frame")?;
                    buf.advance(bytes_to_skip);
                }
                ParseResult::OversizedFrame { opaque, opcode_byte } => {
                    eprintln!("[redcouch] oversized frame (opcode 0x{opcode_byte:02x}), closing connection");
                    write_error_for_raw_opcode(&mut out, opcode_byte, ST_ARGS, opaque, b"Frame too large")?;
                    // Flush the error response before closing.
                    if !out.is_empty() {
                        use std::io::Write;
                        sock.write_all(&out)?;
                        out.clear();
                    }
                    return Ok(());
                }
            }
        }

        // Flush all batched responses in a single write_all() call.
        if !out.is_empty() {
            use std::io::Write;
            sock.write_all(&out)?;
            out.clear();
        }
    }
}

/* ============================================================
   Command handlers
   ========================================================= */

#[cfg(not(test))]
fn handle(req: Request<'_>, out: &mut Vec<u8>) -> Br<()> {
    let opcode = match req.hdr.opcode {
        Some(op) => op,
        None => {
            // Unknown opcode — respond with ST_UNK and continue.
            write_error_for_raw_opcode(
                out,
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
        Get | GetK => op_get(req, out),
        Set | Add | Replace => op_store(req, out),
        Delete => op_delete(req, out),
        Increment | Decrement => op_counter(req, out),
        Touch => op_touch(req, out),
        GAT => op_gat(req, out),
        Append | Prepend => op_append_prepend(req, out),
        Flush => op_flush(req, out),
        Version => op_version(req, out),
        Stat => op_stat(req, out),
        Verbosity => op_verbosity(req, out),
        SaslListMechs => op_sasl_list_mechs(req, out),
        SaslAuth => op_sasl_auth(req, out),
        SaslStep => op_sasl_step(req, out),
        Noop => {
            write_simple_response(out, opcode, ST_OK, req.hdr.opaque, CAS_ZERO, &[])?;
            Ok(())
        }
        Quit => {
            if !opcode.is_quiet() {
                write_simple_response(out, opcode, ST_OK, req.hdr.opaque, CAS_ZERO, &[])?;
            }
            Err(std::io::Error::from(std::io::ErrorKind::ConnectionAborted).into())
        }
        // base() maps all quiet variants to their loud base, so the
        // remaining arms are unreachable.
        _ => {
            write_error_for_raw_opcode(
                out, req.hdr.opcode_byte, ST_UNK, req.hdr.opaque,
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
fn op_get(req: Request<'_>, out: &mut Vec<u8>) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    STAT_CMD_GET.fetch_add(1, Ordering::Relaxed);
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
        STAT_GET_MISSES.fetch_add(1, Ordering::Relaxed);
        if opcode.is_quiet() {
            return Ok(());
        }
        write_simple_response(out, opcode, ST_NF, req.hdr.opaque, CAS_ZERO, &[])?;
        return Ok(());
    }

    STAT_GET_HITS.fetch_add(1, Ordering::Relaxed);

    // Value is hex-encoded by the Lua script to avoid redis-module
    // UTF-8 conversion panics on binary payloads.  Decode it here.
    let hex_str = eval_str(&fields[1]);
    let value_bytes = hex_decode(&hex_str);

    let flags: u32 = eval_str(&fields[2]).parse().unwrap_or(0);
    let cas: u64 = eval_str(&fields[3]).parse().unwrap_or(0);

    let extras = flags.to_be_bytes();
    let key_part = if opcode.includes_key() {
        req.key
    } else {
        &[]
    };

    write_response(
        out, opcode, ST_OK, req.hdr.opaque, cas,
        &extras, key_part, &value_bytes,
    )?;
    Ok(())
}

/* ------ SET / ADD / REPLACE (and quiet variants) ------ */
#[cfg(not(test))]
fn op_store(req: Request<'_>, out: &mut Vec<u8>) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    let base = opcode.base();
    STAT_CMD_SET.fetch_add(1, Ordering::Relaxed);

    if req.extras.len() != 8 {
        write_simple_response(out, opcode, ST_ARGS, req.hdr.opaque, CAS_ZERO, &[])?;
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

    // Track CAS stats for CAS-conditional stores.
    let is_cas_op = req.hdr.cas != 0;
    match status_code {
        0 => {
            // Success
            if is_cas_op {
                STAT_CAS_HITS.fetch_add(1, Ordering::Relaxed);
            }
            if !opcode.is_quiet() {
                write_response(
                    out, opcode, ST_OK, req.hdr.opaque, new_cas, &[], &[], &[],
                )?;
            }
        }
        -1 => {
            // NOT_FOUND (REPLACE on missing key, or CAS on missing key)
            if is_cas_op {
                STAT_CAS_MISSES.fetch_add(1, Ordering::Relaxed);
            }
            write_simple_response(out, opcode, ST_NF, req.hdr.opaque, CAS_ZERO, &[])?;
        }
        -2 => {
            // KEY_EXISTS (ADD on existing key, or CAS mismatch)
            if is_cas_op {
                STAT_CAS_BADVAL.fetch_add(1, Ordering::Relaxed);
            }
            write_simple_response(out, opcode, ST_IX, req.hdr.opaque, CAS_ZERO, &[])?;
        }
        _ => {
            return Err(BridgeErr::Redis(format!("EVAL store unknown status: {status_code}")));
        }
    }
    Ok(())
}

/* ------------- DELETE / DELETEQ ------------- */
#[cfg(not(test))]
fn op_delete(req: Request<'_>, out: &mut Vec<u8>) -> Br<()> {
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
            STAT_DELETE_HITS.fetch_add(1, Ordering::Relaxed);
            // Deleted successfully.
            if opcode.is_quiet() {
                return Ok(());
            }
            // For DELETE success, we don't return the old CAS — just a non-zero one.
            // Use a fresh CAS from the counter for the response.
            let new_cas = get_next_cas();
            write_simple_response(out, opcode, ST_OK, req.hdr.opaque, new_cas, &[])?;
        }
        -1 => {
            STAT_DELETE_MISSES.fetch_add(1, Ordering::Relaxed);
            // NOT_FOUND
            write_simple_response(out, opcode, ST_NF, req.hdr.opaque, CAS_ZERO, &[])?;
        }
        -2 => {
            // KEY_EXISTS (CAS mismatch)
            write_simple_response(out, opcode, ST_IX, req.hdr.opaque, CAS_ZERO, &[])?;
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
fn op_counter(req: Request<'_>, out: &mut Vec<u8>) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    let base = opcode.base();

    if req.extras.len() != 20 {
        write_simple_response(out, opcode, ST_ARGS, req.hdr.opaque, CAS_ZERO, &[])?;
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

    let is_incr = base == Opcode::Increment;
    match status_code {
        0 => {
            if is_incr {
                STAT_INCR_HITS.fetch_add(1, Ordering::Relaxed);
            } else {
                STAT_DECR_HITS.fetch_add(1, Ordering::Relaxed);
            }
            // Parse the counter value as u64.
            let counter_val: u64 = value_str.parse().unwrap_or(0);
            if !opcode.is_quiet() {
                write_response(
                    out, opcode, ST_OK, req.hdr.opaque, new_cas,
                    &[], &[], &counter_val.to_be_bytes(),
                )?;
            }
        }
        -1 => {
            if is_incr {
                STAT_INCR_MISSES.fetch_add(1, Ordering::Relaxed);
            } else {
                STAT_DECR_MISSES.fetch_add(1, Ordering::Relaxed);
            }
            // NOT_FOUND (0xFFFFFFFF expiry)
            write_simple_response(out, opcode, ST_NF, req.hdr.opaque, CAS_ZERO, &[])?;
        }
        -3 => {
            // Non-numeric value
            write_simple_response(
                out, opcode, ST_ARGS, req.hdr.opaque, CAS_ZERO,
                b"Non-numeric value",
            )?;
        }
        _ => {
            return Err(BridgeErr::Redis(format!("EVAL counter unknown status: {status_code}")));
        }
    }
    Ok(())
}

/* ------------- TOUCH ------------- */
#[cfg(not(test))]
fn op_touch(req: Request<'_>, out: &mut Vec<u8>) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    STAT_CMD_TOUCH.fetch_add(1, Ordering::Relaxed);

    if req.extras.len() != 4 {
        write_simple_response(out, opcode, ST_ARGS, req.hdr.opaque, CAS_ZERO, &[])?;
        return Ok(());
    }

    let expiry = BigEndian::read_u32(&req.extras[0..4]);
    let rk = make_redis_key(req.key);
    let expiry_str = expiry.to_string();

    let reply = with_ctx(|ctx| {
        let args: &[&[u8]] = &[
            LUA_TOUCH.as_bytes(),
            b"2",
            rk.as_slice(),
            CAS_COUNTER_KEY.as_bytes(),
            expiry_str.as_bytes(),
        ];
        ctx.call("EVAL", args)
    }).map_err(|e| BridgeErr::Redis(e.to_string()))?;

    if is_redis_error(&reply) {
        return Err(BridgeErr::Redis(format!("EVAL touch error: {reply:?}")));
    }

    let (status_code, new_cas) = match &reply {
        RedisValue::Array(arr) if arr.len() >= 2 => {
            let st = eval_int(&arr[0]);
            let cas_s = eval_str(&arr[1]);
            let cas_val: u64 = cas_s.parse().unwrap_or(0);
            (st, cas_val)
        }
        _ => {
            return Err(BridgeErr::Redis(format!("EVAL touch unexpected: {reply:?}")));
        }
    };

    match status_code {
        0 => {
            if !opcode.is_quiet() {
                write_simple_response(out, opcode, ST_OK, req.hdr.opaque, new_cas, &[])?;
            }
        }
        -1 => {
            write_simple_response(out, opcode, ST_NF, req.hdr.opaque, CAS_ZERO, &[])?;
        }
        _ => {
            return Err(BridgeErr::Redis(format!("EVAL touch unknown status: {status_code}")));
        }
    }
    Ok(())
}

/* ------------- GAT (Get And Touch) ------------- */
#[cfg(not(test))]
fn op_gat(req: Request<'_>, out: &mut Vec<u8>) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    STAT_CMD_GET.fetch_add(1, Ordering::Relaxed);
    STAT_CMD_TOUCH.fetch_add(1, Ordering::Relaxed);

    if req.extras.len() != 4 {
        write_simple_response(out, opcode, ST_ARGS, req.hdr.opaque, CAS_ZERO, &[])?;
        return Ok(());
    }

    let expiry = BigEndian::read_u32(&req.extras[0..4]);
    let rk = make_redis_key(req.key);
    let expiry_str = expiry.to_string();

    let reply = with_ctx(|ctx| {
        let args: &[&[u8]] = &[
            LUA_GAT.as_bytes(),
            b"2",
            rk.as_slice(),
            CAS_COUNTER_KEY.as_bytes(),
            expiry_str.as_bytes(),
        ];
        ctx.call("EVAL", args)
    }).map_err(|e| BridgeErr::Redis(e.to_string()))?;

    if is_redis_error(&reply) {
        return Err(BridgeErr::Redis(format!("EVAL gat error: {reply:?}")));
    }

    let fields = match &reply {
        RedisValue::Array(arr) if arr.len() >= 4 => arr,
        _ => {
            return Err(BridgeErr::Redis(format!("EVAL gat unexpected: {reply:?}")));
        }
    };

    let status_code = eval_int(&fields[0]);
    if status_code == -1 {
        if opcode.is_quiet() {
            return Ok(());
        }
        write_simple_response(out, opcode, ST_NF, req.hdr.opaque, CAS_ZERO, &[])?;
        return Ok(());
    }

    let hex_str = eval_str(&fields[1]);
    let value_bytes = hex_decode(&hex_str);
    let flags: u32 = eval_str(&fields[2]).parse().unwrap_or(0);
    let cas: u64 = eval_str(&fields[3]).parse().unwrap_or(0);

    let extras = flags.to_be_bytes();
    let key_part = if opcode.includes_key() {
        req.key
    } else {
        &[]
    };

    write_response(
        out, opcode, ST_OK, req.hdr.opaque, cas,
        &extras, key_part, &value_bytes,
    )?;
    Ok(())
}

/* ------------- APPEND / PREPEND ------------- */
#[cfg(not(test))]
fn op_append_prepend(req: Request<'_>, out: &mut Vec<u8>) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    let base = opcode.base();
    // Append/Prepend are mutation commands; count under cmd_set
    // (matches standard memcached stat semantics).
    STAT_CMD_SET.fetch_add(1, Ordering::Relaxed);

    // Append/Prepend: no extras expected, key + value in body.
    if !req.extras.is_empty() {
        write_simple_response(out, opcode, ST_ARGS, req.hdr.opaque, CAS_ZERO, &[])?;
        return Ok(());
    }

    let rk = make_redis_key(req.key);
    let req_cas = req.hdr.cas.to_string();

    let lua_script = if base == Opcode::Append { LUA_APPEND } else { LUA_PREPEND };

    let reply = with_ctx(|ctx| {
        let args: &[&[u8]] = &[
            lua_script.as_bytes(),
            b"2",
            rk.as_slice(),
            CAS_COUNTER_KEY.as_bytes(),
            req.value,
            req_cas.as_bytes(),
        ];
        ctx.call("EVAL", args)
    }).map_err(|e| BridgeErr::Redis(e.to_string()))?;

    if is_redis_error(&reply) {
        return Err(BridgeErr::Redis(format!("EVAL append/prepend error: {reply:?}")));
    }

    let (status_code, new_cas) = match &reply {
        RedisValue::Array(arr) if arr.len() >= 2 => {
            let st = eval_int(&arr[0]);
            let cas_s = eval_str(&arr[1]);
            let cas_val: u64 = cas_s.parse().unwrap_or(0);
            (st, cas_val)
        }
        _ => {
            return Err(BridgeErr::Redis(format!("EVAL append/prepend unexpected: {reply:?}")));
        }
    };

    match status_code {
        0 => {
            if !opcode.is_quiet() {
                write_response(
                    out, opcode, ST_OK, req.hdr.opaque, new_cas, &[], &[], &[],
                )?;
            }
        }
        -5 => {
            // NOT_STORED — key does not exist
            write_simple_response(out, opcode, ST_NOT_STORED, req.hdr.opaque, CAS_ZERO, &[])?;
        }
        -2 => {
            // CAS mismatch
            write_simple_response(out, opcode, ST_IX, req.hdr.opaque, CAS_ZERO, &[])?;
        }
        _ => {
            return Err(BridgeErr::Redis(format!("EVAL append/prepend unknown status: {status_code}")));
        }
    }
    Ok(())
}

/* ------------- FLUSH ------------- */
#[cfg(not(test))]
fn op_flush(req: Request<'_>, out: &mut Vec<u8>) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    STAT_CMD_FLUSH.fetch_add(1, Ordering::Relaxed);

    // Flush deletes all rc: prefixed keys.
    // Use SCAN + DEL pattern to avoid blocking on large keyspaces.
    with_ctx(|ctx| {
        // Use Lua to scan and delete all rc:* keys atomically.
        let lua = r#"
local cursor = '0'
local count = 0
repeat
  local result = redis.call('SCAN', cursor, 'MATCH', 'rc:*', 'COUNT', 100)
  cursor = result[1]
  local keys = result[2]
  if #keys > 0 then
    for i, key in ipairs(keys) do
      redis.call('DEL', key)
      count = count + 1
    end
  end
until cursor == '0'
return count
"#;
        let args: &[&[u8]] = &[lua.as_bytes(), b"0"];
        ctx.call("EVAL", args)
    }).map_err(|e| BridgeErr::Redis(e.to_string()))?;

    if !opcode.is_quiet() {
        write_simple_response(out, opcode, ST_OK, req.hdr.opaque, CAS_ZERO, &[])?;
    }
    Ok(())
}

/* ------------- VERSION ------------- */
#[cfg(not(test))]
fn op_version(req: Request<'_>, out: &mut Vec<u8>) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    let version = b"RedCouch 0.1.0";
    write_simple_response(out, opcode, ST_OK, req.hdr.opaque, CAS_ZERO, version)?;
    Ok(())
}

/* ------------- SASL auth (GA: permissive, no auth enforcement) ------------- */

/// SASL_LIST_MECHS — return the list of supported SASL mechanisms.
/// GA behavior: returns "PLAIN" so clients know the mechanism is available.
/// No actual authentication is enforced in this GA release.
#[cfg(not(test))]
fn op_sasl_list_mechs(req: Request<'_>, out: &mut Vec<u8>) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    STAT_AUTH_CMDS.fetch_add(1, Ordering::Relaxed);
    write_simple_response(out, opcode, ST_OK, req.hdr.opaque, CAS_ZERO, b"PLAIN")?;
    Ok(())
}

/// SASL_AUTH — accept PLAIN authentication.
/// GA behavior: accepts any credentials and returns ST_OK.  This lets
/// SASL-requiring clients (e.g. Couchbase SDKs) connect without error.
/// When real auth enforcement is needed post-GA, this handler should
/// validate credentials against a configured source.
#[cfg(not(test))]
fn op_sasl_auth(req: Request<'_>, out: &mut Vec<u8>) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    STAT_AUTH_CMDS.fetch_add(1, Ordering::Relaxed);

    // The mechanism name is in the key field.
    let mechanism = std::str::from_utf8(req.key).unwrap_or("");
    if mechanism != "PLAIN" {
        STAT_AUTH_ERRORS.fetch_add(1, Ordering::Relaxed);
        write_simple_response(
            out, opcode, protocol::ST_AUTH_ERROR, req.hdr.opaque, CAS_ZERO,
            b"Unsupported SASL mechanism",
        )?;
        return Ok(());
    }

    // PLAIN auth: accept any credentials for GA.
    // The value field contains the PLAIN payload: \0<username>\0<password>
    write_simple_response(
        out, opcode, ST_OK, req.hdr.opaque, CAS_ZERO,
        b"Authenticated",
    )?;
    Ok(())
}

/// SASL_STEP — continue a multi-step SASL handshake.
/// GA behavior: PLAIN is single-step, so any SASL_STEP is unexpected.
/// Return ST_AUTH_ERROR to signal the handshake is already complete.
#[cfg(not(test))]
fn op_sasl_step(req: Request<'_>, out: &mut Vec<u8>) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    STAT_AUTH_CMDS.fetch_add(1, Ordering::Relaxed);
    STAT_AUTH_ERRORS.fetch_add(1, Ordering::Relaxed);
    write_simple_response(
        out, opcode, protocol::ST_AUTH_ERROR, req.hdr.opaque, CAS_ZERO,
        b"SASL step not expected for PLAIN mechanism",
    )?;
    Ok(())
}

/* ------------- STAT ------------- */

/// Lua script to count current items (rc:* keys).
#[cfg(not(test))]
const LUA_COUNT_ITEMS: &str = r#"
local cursor = '0'
local count = 0
repeat
  local result = redis.call('SCAN', cursor, 'MATCH', 'rc:*', 'COUNT', 1000)
  cursor = result[1]
  count = count + #result[2]
until cursor == '0'
return count
"#;

/// STAT — return server statistics.
///
/// Binary STAT protocol:
/// - Each stat is a response with opcode=STAT, the stat name in the key
///   field, and the stat value in the value field.
/// - The sequence ends with a response that has empty key and empty value.
/// - If the request key is empty, return general stats.
/// - If the request key names a group we don't support, return empty
///   (just the terminator).
#[cfg(not(test))]
fn op_stat(req: Request<'_>, out: &mut Vec<u8>) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    let stat_key = std::str::from_utf8(req.key).unwrap_or("");

    match stat_key {
        "" => {
            // General stats.
            let uptime = STARTUP_INSTANT
                .get()
                .map(|t| t.elapsed().as_secs())
                .unwrap_or(0);
            let pid = std::process::id();

            // Count current items via Lua SCAN.
            let curr_items: u64 = match with_ctx(|ctx| {
                let args: &[&[u8]] = &[LUA_COUNT_ITEMS.as_bytes(), b"0"];
                ctx.call("EVAL", args)
            }) {
                Ok(RedisValue::Integer(n)) => n as u64,
                _ => 0,
            };

            let stats: Vec<(&str, String)> = vec![
                ("pid", pid.to_string()),
                ("uptime", uptime.to_string()),
                ("time", std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
                    .to_string()),
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
                write_response(
                    out, opcode, ST_OK, req.hdr.opaque, CAS_ZERO,
                    &[], name.as_bytes(), value.as_bytes(),
                )?;
            }
        }
        // Unsupported stat groups — return just the terminator.
        // This includes "settings", "items", "slabs", "conns", etc.
        // These are intentionally unsupported in the GA release.
        _ => {}
    }

    // Terminator: empty key + empty value.
    write_response(
        out, opcode, ST_OK, req.hdr.opaque, CAS_ZERO,
        &[], &[], &[],
    )?;
    Ok(())
}

/* ------------- VERBOSITY ------------- */

/// VERBOSITY — set server verbosity level.
/// GA behavior: accept and return ST_OK as a no-op.  RedCouch does not
/// currently support dynamic verbosity levels; logging is controlled
/// by Redis module logging.
#[cfg(not(test))]
fn op_verbosity(req: Request<'_>, out: &mut Vec<u8>) -> Br<()> {
    let opcode = req.hdr.opcode.unwrap();
    write_simple_response(out, opcode, ST_OK, req.hdr.opaque, CAS_ZERO, &[])?;
    Ok(())
}


/* ============================================================
   Module declaration  (allocator + init)
   ========================================================= */

#[cfg(not(test))]
fn module_init(ctx: &Context, _args: &[RedisString]) -> Status {
    let _ = STARTUP_INSTANT.set(Instant::now());
    LISTENER.call_once(spawn_listener);
    ctx.log_notice(&format!(
        "redcouch: listener started on {} (max_connections={}, read_timeout={}s, write_timeout={}s, max_body={})",
        DEFAULT_BIND_ADDR,
        MAX_CONNECTIONS,
        SOCKET_READ_TIMEOUT.as_secs(),
        SOCKET_WRITE_TIMEOUT.as_secs(),
        MAX_BODY_LEN,
    ));
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
