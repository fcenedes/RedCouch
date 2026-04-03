# Architecture

RedCouch is a Redis module (`cdylib`) that exposes a memcached-compatible TCP endpoint backed by Redis data structures. It runs inside the Redis process as a loaded module, sharing the same address space and data access as Redis itself.

This chapter explains the architectural decisions behind RedCouch, how requests flow through the system, and why the design makes the trade-offs it does.

## High-Level Data Flow

```text
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

## Request Lifecycle

A typical SET request follows this path:

1. **Accept** — The listener thread accepts the TCP connection and spawns a handler thread.
2. **Detect** — The handler reads the first byte: `0x80` routes to binary, printable ASCII routes to text/meta.
3. **Parse** — The protocol-specific parser decodes the request into an internal representation (opcode, key, value, flags, extras).
4. **Namespace** — The client key is prefixed with `rc:` to form the Redis key.
5. **Execute** — A Lua script runs atomically on the Redis side: it increments the CAS counter, hex-encodes the value, and stores the hash fields (`v`, `f`, `c`). If the request has a CAS token, the script checks it before mutating.
6. **Respond** — The handler builds a protocol-specific response (with the new CAS token) and writes it to the socket.
7. **Batch** — For binary protocol, multiple responses from a single read cycle are buffered and flushed in one `write_all()` call.

## Module Structure

| File | Purpose |
|---|---|
| `src/lib.rs` | Module entry point, TCP listener, connection handler, Redis command dispatch, Lua scripts, stats tracking |
| `src/protocol.rs` | Binary protocol types: opcode enum, request parser, response encoder, frame builder |
| `src/ascii.rs` | ASCII text protocol parser and command types (19 commands), meta prefix routing |
| `src/meta.rs` | Meta protocol parser, flag validation, command types (`mg`/`ms`/`md`/`ma`/`mn`/`me`) |

The crate compiles as a `cdylib` — a C-compatible dynamic library that Redis loads at runtime. The `#[redis_module]` macro registers the module with Redis and triggers `redcouch_init()`, which spawns the TCP listener.

## Threading Model

RedCouch uses a thread-per-connection model:

- **Main thread** — The Redis server thread. Module init registers the module and spawns the listener. RedCouch does not block or interfere with the main Redis event loop.
- **Listener thread** — A single background thread that calls `accept()` on `127.0.0.1:11210` in a loop. Each accepted connection is handed off to a new thread.
- **Connection threads** — One OS thread per accepted connection (up to `MAX_CONNECTIONS = 1024`). Each thread owns its socket and processes requests sequentially — there is no async I/O or event multiplexing within a connection.

### Why thread-per-connection?

The thread-per-connection model was chosen for simplicity and correctness:

- **Simple ownership** — Each thread owns its socket, read buffer, and write buffer. No shared mutable state between connections.
- **Sequential request processing** — Memcached protocol requests on a single connection are processed in order, which matches the protocol's expectation.
- **Bounded resource usage** — The 1,024 connection limit caps thread count. Connections beyond this limit are immediately dropped with no response.

The trade-off is that each connection consumes an OS thread's stack (~8 MiB default on Linux). At the 1,024 connection limit, this is ~8 GiB of virtual memory (though actual resident memory is much lower). For RedCouch's intended use case as a migration bridge, this is acceptable.

### Redis access serialization

Each connection thread acquires a `ThreadSafeContext` lock to execute Redis commands. This lock serializes access to the Redis data structures across all connection threads — only one thread can execute a Redis command at a time. This is the primary concurrency bottleneck: benchmark data shows throughput plateaus around 4 concurrent clients (~60k ops/s) and reaches a ceiling of ~35k ops/s at 16+ clients for contended workloads.

## Protocol Detection

On each new connection, the first byte determines the protocol:

1. **`0x80`** → Binary protocol path (`handle_binary_conn`)
2. **Printable ASCII** → Text protocol path (`handle_ascii_conn`), which internally routes meta commands (`mg`/`ms`/`md`/`ma`/`mn`/`me` prefixes) to the meta handler
3. **`\r`/`\n`** → Skipped; next byte re-evaluated

Protocol is fixed for the lifetime of the connection. A single connection cannot switch between binary and ASCII mode.

## Storage Model

Each memcached item is stored as a Redis hash with three fields:

| Field | Content | Example |
|---|---|---|
| `v` | Item value (hex-encoded for binary safety) | `48656c6c6f` ("Hello") |
| `f` | Flags (32-bit unsigned, decimal string) | `0` |
| `c` | CAS token (monotonic counter value) | `42` |

**Key mapping**: Client key `foo` → Redis key `rc:foo`. This prefix-based namespace prevents collisions with other Redis data. You can inspect RedCouch items directly:

```bash
redis-cli HGETALL rc:foo
# Returns: v, <hex-encoded value>, f, <flags>, c, <cas>
```

**System keys**: The monotonic CAS counter lives at `redcouch:sys:cas_counter`. Flush operations scan only `rc:*` keys, leaving system keys and all non-RedCouch data untouched.

### Why hashes instead of strings?

A memcached item has three properties: value, flags, and CAS token. Using a Redis hash stores all three atomically under one key. The alternative — separate keys for each property — would require multi-key transactions and complicate expiry handling. The hash approach also makes items self-describing and inspectable via standard Redis tools.

### Why hex encoding?

The `redis-module` crate's `RedisString` type requires valid UTF-8 for string operations. Memcached values are arbitrary bytes — a JPEG, a Protocol Buffer, or a compressed payload may contain any byte sequence. Hex encoding guarantees the value stored in Redis is valid ASCII, avoiding panics on non-UTF-8 data. The cost is 2× storage for values and CPU time for encode/decode, but it ensures correctness for all payloads.

## Atomicity and Lua Scripts

All CAS-sensitive and read-modify-write operations use server-side **Lua scripts** executed via `redis.call()`. This includes:

- **Store with CAS check** — SET/ADD/REPLACE with a CAS token verify the current CAS before mutating
- **Counter operations** — INCREMENT/DECREMENT read the current value, compute the new value, and store it atomically
- **Append/Prepend** — Read the existing value, concatenate, and store back in one script
- **Delete with CAS** — Verify CAS before removing the key

Each Lua script executes atomically on the Redis side — no other command can interleave. This eliminates the class of check-then-set race conditions that would arise from multi-step operations using separate Redis commands.

The CAS counter itself is a simple `INCR redcouch:sys:cas_counter` within each Lua script. Every mutation generates a new, globally unique CAS value.

## Response Batching

Binary protocol clients often send multiple requests before reading responses (pipelining). RedCouch collects all responses from a single read cycle into a write buffer and flushes them in a single `write_all()` call. This reduces syscall overhead from O(responses) to O(1) per batch, which is measurable at high throughput.

ASCII protocol responses are written individually since ASCII clients typically send one command at a time.

## Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `redis-module` | 2.0.7 | Redis module API bindings — provides `Context`, `ThreadSafeContext`, module registration macros, and the `RedisString` type |
| `bytes` | 1 | Byte buffer management for protocol parsing — used for zero-copy request body handling |
| `byteorder` | 1 | Big-endian integer parsing for binary protocol header fields |
| `thiserror` | 2.0.12 | Derive macro for error type definitions |

The dependency set is intentionally minimal. No async runtime (tokio, async-std) is used — the thread-per-connection model with blocking I/O keeps the dependency tree small and the build fast.
