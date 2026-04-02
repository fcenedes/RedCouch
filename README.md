# RedCouch

A Redis module that bridges **Couchbase memcached binary protocol** clients to Redis 8+, providing a memcached-compatible TCP endpoint backed by Redis data structures.

**Protocol**: Couchbase memcached binary protocol over TCP (port 11210)
**Runtime**: Redis Open Source 8.x (verified on 8.4.0)
**Module name**: `redcouch`

## References

- [Couchbase memcached binary protocol](https://github.com/couchbase/memcached/blob/master/docs/BinaryProtocol.md)
- [redis-module-rs](https://github.com/RedisLabsModules/redismodule-rs)

## Build

```bash
cargo build --release
```

## Run

```bash
redis-server --loadmodule ./target/release/libred_couch.dylib
```

The module starts a TCP listener on `127.0.0.1:11210` accepting memcached binary protocol clients.

## Test

```bash
# Unit and protocol tests (60 tests)
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
- **Safe defaults**: loopback-only bind, 1024 connection limit, 30s read / 10s write timeouts, 20 MiB frame cap

## License

Use at your own risk.