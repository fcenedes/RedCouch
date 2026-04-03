# Known Limitations

This chapter documents the known limitations, behavioral differences from standard memcached, and areas intentionally deferred from the current release. Each section explains the cause, the impact on your workload, and any available workarounds.

## Counter Precision (post-2^53)

**Impact:** Counter values are exact only for the range `[0, 2^53)`. Beyond 2^53 (9,007,199,254,740,992), the behavior is **precision loss / rounding** rather than reliable wraparound.

**Cause:** Redis Lua scripts use IEEE 754 double-precision floats for numeric operations. Double-precision floats can represent integers exactly up to 2^53, but above that threshold, consecutive integers are no longer representable. The memcached binary protocol specifies unsigned 64-bit counter semantics with wraparound at 2^64; RedCouch cannot match that behavior exactly above 2^53.

**Who is affected:** Only workloads that increment counters past ~9 quadrillion. Typical use cases (rate limiters, hit counters, sequence numbers) will never reach this threshold.

**Workaround:** If you need exact 64-bit counter semantics, migrate the counter to a native Redis `INCR` key (which uses native 64-bit integers) and access it via a Redis client instead of through the memcached protocol.

## Append/Prepend Value Growth

**Impact:** APPEND and PREPEND operations have cost proportional to the existing value size. Each operation reads the entire existing value (hex-encoded), concatenates the new data, and writes the result back.

**Cause:** The hex-encoding storage model means an append to a 100 KB value requires reading ~200 KB of hex data, concatenating, and writing ~200 KB + new data. This is inherent to the hash-per-item storage model.

**Measured behavior:** In stress testing, 10 keys reached ~61 KB each after ~950 appends of 64-byte chunks, with no errors or instability.

**Workaround:** For append-heavy workloads with large accumulated values, consider:
- **Periodic key rotation** — Start writing to a new key periodically and merge when needed.
- **Size monitoring** — Track value sizes and set alerts if they grow beyond expected bounds.
- **Native Redis migration** — Use Redis's native `APPEND` command (which operates on raw bytes without hex encoding) by migrating the relevant keys to direct Redis access.

## Performance Hot Paths

The following are identified performance costs that remain in the current release. They are documented here so operators and contributors can understand where time is spent:

1. **Lua hex encode/decode** — Every GET and every binary-value mutation passes through Lua `string.format('%02x')` for encoding and manual hex decode in Rust for decoding. This is the largest single overhead compared to native Redis operations. Benchmark data shows ~18% throughput reduction on GET hits compared to native Redis `GET`. This is the correctness-first design: it avoids `redis-module` UTF-8 panics on arbitrary binary payloads.

2. **Per-request `ThreadSafeContext` lock** — Each Redis command acquires the `ThreadSafeContext` lock, which serializes Redis access across all connection threads. This is the primary concurrency bottleneck. Benchmark data shows throughput plateaus at ~4 clients (~60k ops/s) and hits a ceiling of ~35k ops/s at 16+ clients for contended workloads. See [Benchmarks & Performance](../operations/benchmarks.md) for measured data.

3. **Per-request allocations** — `Vec` allocations for key namespacing (`rc:` prefix), hex conversion buffers, and response assembly. These are small compared to the Lua and lock overhead but contribute to the total per-request cost.

## Startup / Bind Caveat

**Impact:** The background TCP listener thread may log readiness before the bind has definitively succeeded.

**Cause:** The listener thread starts, logs its intent to bind, and then calls `bind()`. If another process holds port 11210, the bind fails and the listener thread exits — but Redis itself continues running normally without the memcached endpoint.

**Detection:**
```bash
# After loading the module, verify the port is reachable
nc -z 127.0.0.1 11210 && echo "OK" || echo "FAILED"

# Check Redis logs for bind errors
redis-cli INFO ALL | grep -i redcouch
```

**Workaround:** Ensure no other process is using port 11210 before loading the module. If you need to change the port, modify `DEFAULT_BIND_ADDR` in `src/lib.rs` and rebuild.

## SASL Authentication

**Impact:** SASL auth is stub-only. Any credentials are accepted. There is **no access control** on the memcached protocol endpoint.

**Cause:** The SASL stub exists solely to allow Couchbase SDK clients (which require a SASL handshake) to connect. Implementing real credential enforcement would require a credential store and a policy for credential management, which is outside the scope of a protocol bridge.

**Who is affected:** Anyone exposing port 11210 to untrusted networks.

**Workaround:** Rely on network-level access control (firewall rules, security groups, loopback-only bind) rather than application-level authentication. See [Configuration — Security Considerations](./configuration.md#security-considerations).

## Malformed Traffic Behavior

RedCouch handles malformed requests with clean disconnects or error responses — not crashes or hangs:

| Scenario | Behavior | Connection |
|---|---|---|
| Bad magic byte | Connection closed (EOF) | Closed |
| Truncated header | Read timeout (30s), then close | Closed |
| Body length mismatch | Read timeout, then close | Closed |
| Zero-key GET | Error response (status 0x0001) | Stays open |
| Garbage then valid | Connection closed (EOF) | Closed |
| Oversized key (>250 bytes) | Error response (status 0x0004) | Stays open |
| Oversized frame (>20 MiB body) | Error response | Closed |

This behavior was validated in stress testing with all six malformed scenarios producing the expected clean handling with zero crashes.

## Maximum Sizes

| Limit | Value | Source |
|---|---|---|
| Max key length | 250 bytes | Memcached spec limit |
| Max frame body | 20 MiB | `MAX_BODY_LEN` constant |
| Max command line (ASCII) | 2,048 bytes | Hardcoded parser limit |
| Max concurrent connections | 1,024 | `MAX_CONNECTIONS` constant |

## Module Unload

RedCouch does not implement a module unload/deinit handler. The `MODULE UNLOAD redcouch` command is **unverified** and may leave the listener thread orphaned. The recommended approach to fully stop the module is to restart the Redis process.

## Deferred Surfaces

The following features are **explicitly not in the current release scope**. They may be added in future versions:

| Feature | Reason for deferral |
|---|---|
| Meta protocol **stale items** (`N`/vivify, `I`/invalidate, `R`/recache, `W`/`X`/`Z` flags) | Requires a stale item concept not in the current item model |
| Base64 keys (`b` flag) | Not implemented; standard ASCII keys cover most use cases |
| UDP transport | TCP only; memcached UDP is rarely used in practice |
| Couchbase bucket/vbucket management | Outside the scope of a protocol bridge |
| Dynamic STAT groups (settings, items, slabs, conns) | Returns empty terminator; general stats are supported |
| Windows support | Redis modules require a Unix-like environment |
| Dynamic configuration | All parameters are compile-time constants |
| TLS/SSL | Use a TLS-terminating proxy for encrypted transport |
