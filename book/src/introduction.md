# RedCouch

**A Redis module that bridges memcached protocol clients to Redis 8+**

RedCouch provides a memcached-compatible TCP endpoint backed by Redis data structures. It allows existing memcached clients — including Couchbase SDK clients that speak the memcached binary protocol — to connect to a Redis 8+ server and use it as a drop-in data store, without any application-level code changes.

## Who Is This For?

RedCouch is designed for teams in one of these situations:

- **Migrating from Couchbase to Redis** — Your applications use Couchbase SDKs or memcached clients. RedCouch lets you point them at a Redis 8+ server and keep running while you plan and execute a gradual migration to native Redis clients.
- **Consolidating data infrastructure** — You want to reduce operational overhead by replacing a standalone memcached or Couchbase deployment with Redis, which you may already be running for other workloads.
- **Evaluating Redis as a memcached replacement** — You want to test Redis with your existing memcached workload before committing to a full client migration.

RedCouch is **not** a general-purpose memcached server. It is a protocol-translation bridge: it speaks memcached on the wire but stores data in Redis. The intended end state is migrating your clients to speak Redis natively, at which point RedCouch is no longer needed.

## What RedCouch Does

RedCouch runs inside Redis as a loaded module. On startup, it opens a TCP listener (default `127.0.0.1:11210`) that accepts memcached protocol connections. Each incoming request is parsed, translated into Redis operations, and the response is sent back in the memcached protocol format the client expects.

```text
┌─────────────────────┐     TCP :11210     ┌──────────────────────────────────┐
│  Memcached Client   │ ◄───────────────► │  RedCouch Module (in Redis)      │
│  (binary or ASCII)  │                    │  parse → Redis ops → respond     │
└─────────────────────┘                    └──────────────────────────────────┘
```

The translation is transparent: clients see standard memcached responses, and Redis sees standard hash operations. Data stored through RedCouch is visible via `redis-cli` under the `rc:` key prefix.

## The Migration Path

RedCouch supports a three-phase migration:

1. **Bridge phase** — Deploy RedCouch on your Redis 8+ server. Point your memcached/Couchbase clients at port 11210. Your data now lives in Redis, accessible through both the memcached protocol (via RedCouch) and native Redis commands.

2. **Dual-access phase** — Begin migrating application code from memcached clients to native Redis clients. Both access paths work simultaneously against the same data. You can migrate one service at a time.

3. **Native phase** — Once all clients speak Redis natively, unload the RedCouch module. The memcached protocol endpoint shuts down; Redis continues serving your data directly with no translation overhead.

This migration path is validated by benchmark comparisons showing that RedCouch imposes modest overhead (~18% on GET hits) compared to native Redis, and performs competitively with Couchbase under identical conditions. See [Benchmarks & Performance](./operations/benchmarks.md) for measured data.

## Supported Protocols

RedCouch automatically detects the protocol from the first byte of each connection:

| Protocol | Detection | Coverage |
|---|---|---|
| **Binary** (Couchbase memcached) | First byte `0x80` | All 34 opcodes — GET, SET, DELETE, INCR/DECR, APPEND, TOUCH, FLUSH, SASL, STAT, and more |
| **ASCII text** | Printable ASCII | All 19 standard commands — set, get, delete, incr, cas, flush_all, etc. |
| **Meta** | `mg`/`ms`/`md`/`ma`/`mn`/`me` prefixes | Flag-based meta get/set/delete/arithmetic/noop/debug |

Protocol is detected once per connection and fixed for its lifetime. A single RedCouch listener serves all three protocols concurrently.

## Key Design Points

- **Hash-per-item storage** — Each memcached item is stored as a Redis hash with fields for value (`v`), flags (`f`), and CAS (`c`). This makes items inspectable via standard Redis tools.
- **Namespaced keys** — Client keys are prefixed with `rc:` to avoid collisions with other Redis data. System keys live under `redcouch:sys:*`.
- **Atomic mutations** — All CAS-sensitive operations use server-side Lua scripts, ensuring atomicity without client-side check-then-set races.
- **Binary-safe values** — Full binary round-trip via Lua hex encode/decode. Non-UTF-8 payloads are preserved exactly.
- **Safe defaults** — Loopback-only bind, 1024 connection limit, read/write timeouts, and a 20 MiB frame cap protect against accidental exposure and resource exhaustion.

## Platform Support

| Target | OS | Architecture | Artifact |
|---|---|---|---|
| `x86_64-unknown-linux-gnu` | Linux | x86_64 | `libred_couch.so` |
| `aarch64-unknown-linux-gnu` | Linux | ARM64 | `libred_couch.so` |
| `x86_64-apple-darwin` | macOS | x86_64 | `libred_couch.dylib` |
| `aarch64-apple-darwin` | macOS | ARM64 | `libred_couch.dylib` |

**Windows is not supported.** Redis modules require a Unix-like environment.

## How This Book Is Organized

- **[Getting Started](./getting-started/installation.md)** — Install RedCouch, load it into Redis, and run your first commands.
- **[User Guide](./guide/ascii-protocol.md)** — Walkthrough examples for each protocol: ASCII, meta, and binary.
- **[Reference](./reference/protocol-compatibility.md)** — Complete protocol compatibility tables, architecture details, configuration reference, and known limitations.
- **[Operations](./operations/benchmarks.md)** — Performance benchmarks, release process, and operational guidance.
- **[Development](./development/contributing.md)** — How to contribute, test architecture, and coding standards.
- **[API Reference](./api-reference.md)** — Rust API documentation generated from source.

## License

MIT — see [LICENSE](https://github.com/fcenedes/RedCouch/blob/main/LICENSE) for details.

## Source Code

RedCouch is open source: <https://github.com/fcenedes/RedCouch>
