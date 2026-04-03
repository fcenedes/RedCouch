# RedCouch

**A Redis module that bridges memcached protocol clients to Redis 8+**

RedCouch provides a memcached-compatible TCP endpoint backed by Redis data structures. It allows existing memcached clients — including Couchbase SDK clients that speak the memcached binary protocol — to connect to a Redis 8+ server and use it as a drop-in data store.

## What RedCouch Does

RedCouch runs inside Redis as a loaded module. On startup, it opens a TCP listener (default `127.0.0.1:11210`) that accepts memcached protocol connections. Each incoming request is parsed, translated into Redis operations, and the response is sent back in the memcached protocol format the client expects.

```text
┌─────────────────────┐     TCP :11210     ┌──────────────────────────────────┐
│  Memcached Client   │ ◄───────────────► │  RedCouch Module (in Redis)      │
│  (binary or ASCII)  │                    │  parse → Redis ops → respond     │
└─────────────────────┘                    └──────────────────────────────────┘
```

## Supported Protocols

RedCouch automatically detects the protocol from the first byte of each connection:

| Protocol | Detection | Commands |
|---|---|---|
| **Binary** (Couchbase memcached) | First byte `0x80` | All 34 opcodes (GET, SET, DELETE, INCR/DECR, APPEND, TOUCH, FLUSH, SASL, STAT, etc.) |
| **ASCII text** | Printable ASCII | All 19 standard commands (set, get, delete, incr, cas, flush_all, etc.) |
| **Meta** | `mg`/`ms`/`md`/`ma`/`mn`/`me` prefixes | Flag-based meta get/set/delete/arithmetic/noop/debug |

## Key Design Points

- **Hash-per-item storage**: each item stored as a Redis hash with fields for value (`v`), flags (`f`), and CAS (`c`)
- **Namespaced keys**: client keys prefixed with `rc:`, system keys under `redcouch:sys:*`
- **Atomic mutations**: all CAS-sensitive operations use server-side Lua scripts
- **Binary-safe values**: full binary round-trip via Lua hex encode/decode
- **Safe defaults**: loopback-only bind, 1024 connection limit, read/write timeouts, 20 MiB frame cap

## Platform Support

| Target | OS | Architecture |
|---|---|---|
| `x86_64-unknown-linux-gnu` | Linux | x86_64 |
| `aarch64-unknown-linux-gnu` | Linux | ARM64 |
| `x86_64-apple-darwin` | macOS | x86_64 |
| `aarch64-apple-darwin` | macOS | ARM64 |

**Windows is not supported.** Redis modules require a Unix-like environment.

## License

MIT — see [LICENSE](https://github.com/fcenedes/RedCouch/blob/main/LICENSE) for details.

## Source Code

RedCouch is open source: <https://github.com/fcenedes/RedCouch>
