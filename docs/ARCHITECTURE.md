# Architecture

RedCouch is a Redis module (`cdylib`) that exposes a memcached-compatible TCP endpoint backed by Redis data structures. It runs inside the Redis process as a loaded module.

## High-Level Data Flow

```
┌─────────────────────┐     TCP :11210     ┌──────────────────────────────────┐
│  Memcached Client   │ ◄───────────────► │  RedCouch Module (in Redis)      │
│  (binary or ASCII)  │                    │                                  │
└─────────────────────┘                    │  ┌───────────────────────────┐   │
                                           │  │ TCP Listener Thread       │   │
                                           │  │  → accept()              │   │
                                           │  │  → spawn handler thread  │   │
                                           │  └───────────────────────────┘   │
                                           │                                  │
                                           │  ┌───────────────────────────┐   │
                                           │  │ Connection Handler Thread │   │
                                           │  │  → protocol detection    │   │
                                           │  │  → parse request         │   │
                                           │  │  → execute via Redis API │   │
                                           │  │  → encode response       │   │
                                           │  └───────────────────────────┘   │
                                           │              │                   │
                                           │              ▼                   │
                                           │  ┌───────────────────────────┐   │
                                           │  │ Redis Data (hashes)      │   │
                                           │  │  rc:<key> → {v, f, c}   │   │
                                           │  │  redcouch:sys:*          │   │
                                           │  └───────────────────────────┘   │
                                           └──────────────────────────────────┘
```

## Module Structure

| File | Purpose |
|---|---|
| `src/lib.rs` | Module entry point, TCP listener, connection handler, Redis command dispatch, Lua scripts, stats |
| `src/protocol.rs` | Binary protocol types: opcode enum, request parser, response encoder, frame builder |
| `src/ascii.rs` | ASCII text protocol parser and command types (19 commands), meta prefix routing |
| `src/meta.rs` | Meta protocol parser, flag validation, command types (`mg`/`ms`/`md`/`ma`/`mn`/`me`) |

## Threading Model

- **Main thread**: Redis server thread. Module init registers the module and spawns the listener.
- **Listener thread**: Single background thread that `accept()`s TCP connections on `127.0.0.1:11210`.
- **Connection threads**: One thread per accepted connection (up to `MAX_CONNECTIONS = 1024`). Each thread owns its socket and handles requests sequentially.

Connections beyond the limit are immediately dropped. Each connection thread acquires a `ThreadSafeContext` lock to execute Redis commands, which serializes Redis access across all threads.

## Protocol Detection

On each new connection, the first byte determines the protocol:

1. **`0x80`** → Binary protocol path (`handle_binary_conn`)
2. **Printable ASCII** → Text protocol path (`handle_ascii_conn`), which internally routes meta commands (`mg`/`ms`/`md`/`ma`/`mn`/`me` prefixes) to the meta handler
3. **`\r`/`\n`** → Skipped; next byte re-evaluated

Protocol is fixed for the lifetime of the connection.

## Storage Model

Each memcached item is stored as a Redis hash with three fields:

| Field | Content | Example |
|---|---|---|
| `v` | Item value (hex-encoded for binary safety) | `48656c6c6f` |
| `f` | Flags (32-bit unsigned, decimal string) | `0` |
| `c` | CAS token (monotonic counter value) | `42` |

**Key mapping**: Client key `foo` → Redis key `rc:foo`. This prefix-based namespace prevents collisions with other Redis data.

**System keys**: The monotonic CAS counter lives at `redcouch:sys:cas_counter`. Flush operations scan only `rc:*` keys.

## Atomicity

All CAS-sensitive and read-modify-write operations (store with CAS check, counters, append/prepend, delete with CAS) use server-side **Lua scripts** executed via `redis.call()`. This ensures atomicity without client-side check-then-set races.

## Binary Safety

Values are hex-encoded before storage and hex-decoded on retrieval. This avoids redis-module panics on non-UTF-8 binary payloads while preserving full byte-level round-trip fidelity. The hex encode/decode happens in Lua scripts on the Redis side.

## Response Batching

Binary protocol responses are collected in a write buffer and flushed in a single `write_all()` call per read cycle, reducing syscall overhead from O(responses) to O(1) per batch.

## Runtime Defaults

| Parameter | Value | Constant |
|---|---|---|
| Bind address | `127.0.0.1:11210` | `DEFAULT_BIND_ADDR` |
| Max connections | 1,024 | `MAX_CONNECTIONS` |
| Read timeout | 30 seconds | `SOCKET_READ_TIMEOUT` |
| Write timeout | 10 seconds | `SOCKET_WRITE_TIMEOUT` |
| Max frame body | 20 MiB | `MAX_BODY_LEN` |
| Max key length | 250 bytes | `MAX_KEY_LEN` |
| Key prefix | `rc:` | `KEY_PREFIX` |
| CAS counter key | `redcouch:sys:cas_counter` | `CAS_COUNTER_KEY` |

## Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `redis-module` | 2.0.7 | Redis module API bindings (context, commands, module registration) |
| `bytes` | 1 | Byte buffer management for protocol parsing |
| `byteorder` | 1 | Big-endian integer parsing for binary protocol |
| `thiserror` | 2.0.12 | Error type derivation |

## Test Architecture

Tests are structured to run without a live Redis instance:

- **`src/protocol.rs`** — 60 binary protocol unit tests (parser round-trips, opcode coverage, frame building, malformed handling)
- **`src/ascii.rs`** — 58 ASCII protocol tests (47 parser + 11 meta prefix routing)
- **`src/meta.rs`** — 28 meta protocol tests (parser, flag validation, mode validation, numeric tokens, bare-flag rejection)
- **`tests/integration/`** — E2E tests requiring a live Redis 8+ instance with the module loaded

All protocol/parser modules use `#[cfg(not(test))]` guards to exclude Redis allocator dependencies during `cargo test`, enabling host-process testing without Redis.
