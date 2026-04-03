# Known Limitations

## Counter Precision (post-2^53)

Counter values are exact only for the range `[0, 2^53)`. Beyond `2^53` (9,007,199,254,740,992), the behavior is **precision loss / rounding** rather than reliable wraparound. This is inherent to Redis's use of IEEE 754 double-precision floats for numeric storage via Lua scripts. The memcached binary protocol specifies unsigned 64-bit counter semantics; RedCouch cannot provide bit-exact behavior above 2^53.

## Append/Prepend Value Growth

APPEND and PREPEND operations retrieve the existing value via Lua hex encode, concatenate, and store back. Cost is proportional to existing value size. In the stress suite, 10 keys reached ~61 KB each after ~950 appends of 64B chunks. **For append-heavy workloads with large values, monitor value sizes and consider periodic key rotation.**

## Remaining Hot Paths

The following are identified performance costs that remain in the GA release:

1. **Lua hex encode/decode**: Every GET and binary-value mutation passes through Lua `string.format('%02x')` / manual hex decode in Rust. This is the correctness-first approach to avoid redis-module UTF-8 panics on binary payloads.
2. **Per-request `ThreadSafeContext` / GIL**: Each Redis command acquires a `ThreadSafeContext` lock. This serializes Redis access across all connection threads and is the primary concurrency bottleneck above ~4 clients.
3. **Smaller allocation costs**: Per-request `Vec` allocations for key namespacing, hex conversion buffers, and response assembly.

## Startup / Bind Caveat

The background TCP listener thread may log readiness (`listening on 127.0.0.1:11210`) before the bind attempt has definitively succeeded. If another process holds port 11210, the module logs a `FATAL: cannot bind` error and the listener thread exits, but Redis itself continues running. **Check for the bind-success log line and verify port 11210 is reachable after module load.**

## SASL Authentication

SASL auth is stub-only: `SASL_LIST_MECHS` returns "PLAIN", `SASL_AUTH` always succeeds regardless of credentials. This allows SASL-requiring clients (e.g., Couchbase SDKs) to complete the auth handshake. **No actual credential enforcement exists in this release.**

## Malformed Traffic Behavior

Malformed requests are handled with clean disconnect or timeout, not crashes:

| Scenario | Behavior |
|---|---|
| Bad magic byte | Connection closed (EOF) |
| Truncated header | Read timeout (30s), then close |
| Body length mismatch | Read timeout, then close |
| Zero-key GET | Error response (status 0x0001), connection stays open |
| Garbage then valid | Connection closed (EOF) |
| Oversized key (>250 bytes) | Error response (status 0x0004), connection stays open |
| Oversized frame (>20 MiB body) | Error response, connection closed |

## Maximum Sizes

| Limit | Value |
|---|---|
| Max key length | 250 bytes |
| Max frame body | 20 MiB |
| Max command line (ASCII) | 2,048 bytes |

## Deferred Surfaces

The following are **explicitly not in GA scope**:

- Meta protocol **stale items** (`N`/vivify on mg, `I`/invalidate on md, `R`/recache, `W`/`X`/`Z` stale flags, `b`/base64 keys)
- UDP transport
- Couchbase bucket/vbucket management
- Dynamic STAT groups (settings, items, slabs, conns)
- Windows support
