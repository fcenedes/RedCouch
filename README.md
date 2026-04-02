# RedCouch

A Redis module that bridges **memcached protocol** clients (binary and ASCII) to Redis 8+, providing a memcached-compatible TCP endpoint backed by Redis data structures.

**Protocol**: Memcached binary + ASCII text protocol over TCP (port 11210)
**Runtime**: Redis Open Source 8.x (verified on 8.4.0)
**Module name**: `redcouch`

## References

- [Couchbase memcached binary protocol](https://github.com/couchbase/memcached/blob/master/docs/BinaryProtocol.md)
- [Memcached ASCII text protocol](https://github.com/memcached/memcached/blob/master/doc/protocol.txt)
- [redis-module-rs](https://github.com/RedisLabsModules/redismodule-rs)

## Build

```bash
cargo build --release
```

## Run

```bash
redis-server --loadmodule ./target/release/libred_couch.dylib
```

The module starts a TCP listener on `127.0.0.1:11210` accepting both memcached binary and ASCII text protocol clients. Protocol detection is automatic based on the first byte of each connection.

## Test

```bash
# Unit and protocol tests (143 tests — binary, ASCII, meta)
cargo test

# Integration tests (requires Redis 8+ with module loaded)
cd tests/integration && bash run_e2e.sh

# Benchmarks
cd benchmarks && bash run_benchmarks.sh
```

## Supported Operations

GET, GETQ, GETK, GETKQ, SET, SETQ, ADD, ADDQ, REPLACE, REPLACEQ, DELETE, DELETEQ, INCREMENT, INCREMENTQ, DECREMENT, DECREMENTQ, APPEND, APPENDQ, PREPEND, PREPENDQ, TOUCH, GAT, GATQ, FLUSH, FLUSHQ, NOOP, QUIT, QUITQ, VERSION, STAT, VERBOSITY, SASL_LIST_MECHS, SASL_AUTH, SASL_STEP.

## Documentation

See [`docs/GA_RELEASE.md`](docs/GA_RELEASE.md) for the full GA release documentation including feature/compatibility table, operating envelope, known limitations, configuration reference, benchmark baselines, and release checklist.

## Key Design Points

- **Hash-per-item storage**: each item stored as a Redis hash with fields for value (`v`), flags (`f`), and CAS (`c`)
- **Namespaced keys**: client keys prefixed with `rc:`, system keys under `redcouch:sys:*`
- **Atomic mutations**: all CAS-sensitive operations use server-side Lua scripts
- **Binary-safe values**: full binary round-trip via Lua hex encode/decode
- **Dual protocol**: automatic binary/ASCII detection on first byte; ASCII text protocol covers all 19 standard commands (no auth in text mode) plus meta protocol commands (mg/ms/md/ma/mn)
- **Safe defaults**: loopback-only bind, 1024 connection limit, 30s read / 10s write timeouts, 20 MiB frame cap

## Platform Support

Release artifacts are built for the following targets:

| Target | OS | Architecture | Artifact |
|--------|----|--------------| ---------|
| `x86_64-unknown-linux-gnu` | Linux | x86_64 | `libred_couch.so` |
| `aarch64-unknown-linux-gnu` | Linux | ARM64 | `libred_couch.so` |
| `x86_64-apple-darwin` | macOS | x86_64 | `libred_couch.dylib` |
| `aarch64-apple-darwin` | macOS | ARM64 | `libred_couch.dylib` |

**Windows**: RedCouch does not currently support or publish Windows runtime artifacts.

## License

MIT — see [LICENSE](LICENSE) for details.