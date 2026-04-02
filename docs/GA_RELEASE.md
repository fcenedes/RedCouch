# RedCouch GA Release Documentation

**Module**: `redcouch` (crate name `red_couch`, library `libred_couch`)
**Version**: 0.1.0
**Target runtime**: Redis Open Source 8.x (verified on Redis 8.4.0)
**Protocol scope**: Memcached **binary + ASCII text protocol over TCP** (port 11210)
**Date**: 2026-04-02

---

## 1. Supported Feature / Compatibility Table

| Opcode Family | Opcodes | Status | Notes |
|---|---|---|---|
| GET | `GET`, `GETQ`, `GETK`, `GETKQ` | ✅ Implemented | Returns value, flags, CAS. Quiet variants suppress success responses. Key-inclusive variants echo key in response. |
| SET/ADD/REPLACE | `SET`, `SETQ`, `ADD`, `ADDQ`, `REPLACE`, `REPLACEQ` | ✅ Implemented | CAS-checked mutations. Flags, expiry, and binary-safe values preserved. Quiet variants suppress success. |
| DELETE | `DELETE`, `DELETEQ` | ✅ Implemented | CAS-checked. Returns item CAS on success. |
| INCREMENT/DECREMENT | `INCR`, `INCRQ`, `DECR`, `DECRQ` | ✅ Implemented | Unsigned 64-bit semantics with initial-value and miss rules. See counter-precision caveat below. |
| APPEND/PREPEND | `APPEND`, `APPENDQ`, `PREPEND`, `PREPENDQ` | ✅ Implemented | Requires existing item. See append-duration caveat below. |
| TOUCH | `TOUCH` | ✅ Implemented | Updates TTL on existing items. |
| GAT/GATQ | `GAT`, `GATQ` | ✅ Implemented | Get-and-touch with TTL update. Key included in response. |
| FLUSH | `FLUSH`, `FLUSHQ` | ✅ Implemented | Namespace-isolated: flushes only `rc:*` keys, never `FLUSHDB`. |
| NOOP | `NOOP` | ✅ Implemented | Returns OK. Used as pipeline terminator. |
| QUIT | `QUIT`, `QUITQ` | ✅ Implemented | Graceful connection close. |
| VERSION | `VERSION` | ✅ Implemented | Returns `RedCouch 0.1.0`. |
| STAT | `STAT` | ✅ Implemented | Returns general stats (pid, uptime, version, cmd_get, cmd_set, curr_items, etc.). Settings/items/slabs/conns groups: unsupported (returns empty terminator). |
| VERBOSITY | `VERBOSITY` | ✅ Implemented | Accepted, returns OK. Logging controlled by Redis module logging, not dynamic verbosity levels. |
| SASL AUTH | `SASL_LIST_MECHS`, `SASL_AUTH`, `SASL_STEP` | ✅ Stub | Lists "PLAIN". Auth always succeeds. No credential enforcement. Allows SASL-requiring clients to connect. |
| Unknown opcodes | Any unrecognized opcode byte | ✅ Handled | Returns `Unknown command` (status 0x0081) with raw opcode byte echoed. |

### ASCII Text Protocol

| Command | Status | Notes |
|---|---|---|
| `set`, `add`, `replace` | ✅ Implemented | Full `<key> <flags> <exptime> <bytes> [noreply]` syntax. |
| `cas` | ✅ Implemented | `<key> <flags> <exptime> <bytes> <cas_unique> [noreply]` |
| `append`, `prepend` | ✅ Implemented | Correct syntax: `<key> <bytes> [noreply]` (no flags/exptime). |
| `get`, `gets` | ✅ Implemented | Multi-key. `gets` returns CAS. |
| `gat`, `gats` | ✅ Implemented | Get-and-touch. `gats` returns CAS. |
| `delete` | ✅ Implemented | `<key> [noreply]` |
| `incr`, `decr` | ✅ Implemented | Returns NOT_FOUND for missing keys (no auto-create in ASCII). |
| `touch` | ✅ Implemented | `<key> <exptime> [noreply]` |
| `flush_all` | ✅ Implemented | Delay parameter accepted but not honored. |
| `version` | ✅ Implemented | Returns `VERSION RedCouch 0.1.0`. |
| `stats` | ✅ Implemented | Bare `stats` returns general stats. Unsupported groups (items, slabs, etc.) return empty `END`. |
| `verbosity` | ✅ Implemented | Accepted and returns OK; no effect. |
| `quit` | ✅ Implemented | Closes connection. |
| **Auth** | ❌ Not supported | No SASL/auth in ASCII text mode (per memcached spec). |
| **noreply** | ✅ Implemented | Suppresses responses on successfully parsed commands. On malformed input, `CLIENT_ERROR` may still be emitted because `noreply` cannot be reliably inferred before successful parse. |
| **Protocol detection** | Automatic | First byte `0x80` → binary; printable ASCII → text; `\r`/`\n` skipped. |
| **Meta commands** | Stub | `mg`/`ms`/`md`/`ma`/`mn`/`me` prefixes detected via text-path prefix router and return `SERVER_ERROR meta protocol not supported`. |

### Item Model

| Property | Implementation |
|---|---|
| Storage | Hash-per-item in Redis (`HSET key v <value> f <flags> c <cas>`) |
| Key namespace | Client key `foo` → Redis key `rc:foo`. Collision-free, reserved-key-safe. |
| Reserved keys | System keys under `redcouch:sys:*` (e.g., `redcouch:sys:cas_counter`) |
| CAS | Redis-backed monotonic counter (`INCR redcouch:sys:cas_counter`). Every mutation generates a new CAS. |
| Binary-safe values | Full binary round-trip via Lua hex encode/decode. Non-UTF8 payloads preserved. |
| Flags | 32-bit unsigned, stored as decimal string in hash field `f`. |
| Expiry | `0` = no expiry (persists), `≤2592000` = relative seconds (`EXPIRE`), `>2592000` = absolute Unix timestamp (`EXPIREAT`). |
| Atomic mutations | All CAS-sensitive and read-modify-write operations use server-side Lua scripts. |

---

## 2. Validated Operating Envelope

Source: `benchmarks/results/bench_20260402_144029.json` and `stress_20260402_150543.json` (Redis 8.4.0, macOS arm64).

### Throughput Baselines (1-second runs, single client)

| Workload | ops/sec | p50 µs | p95 µs | p99 µs | Errors |
|---|---|---|---|---|---|
| SET 64B | 31,694 | 29 | 39 | 77 | 0 |
| SET 1KB | 29,899 | 31 | 41 | 58 | 0 |
| SET 64KB | 8,074 | 118 | 151 | 181 | 0 |
| GET (hit) | 26,814 | 35 | 45 | 57 | 0 |
| GET (miss) | 36,782 | 26 | 33 | 41 | 0 |
| DELETE | 40,038 | 24 | 31 | 40 | 0 |
| INCREMENT | 31,883 | 30 | 39 | 56 | 0 |
| Mixed R/W | 14,635 | 65 | 80 | 95 | 0 |
| APPEND | 19,428 | 51 | 73 | 86 | 0 |
| TOUCH | 33,856 | 28 | 36 | 50 | 0 |

### Concurrency Scaling (4-client runs)

| Workload | ops/sec | p50 µs | p95 µs |
|---|---|---|---|
| SET 64B | 62,574 | 60 | 100 |
| SET 1KB | 60,439 | 63 | 104 |
| GET (hit) | 50,929 | 76 | 120 |
| Mixed R/W | 30,138 | 129 | 184 |

### Stress/Soak Findings

| Finding | Value | Source |
|---|---|---|
| **Performance sweet spot** | 4 clients | Peak throughput ~61k ops/s SET at c=4 |
| **Post-saturation ceiling** | ~35k ops/s | At c≥16 with contended workloads (INCR pressure c=16: 32,172 ops/s; c=32: 32,629; c=64: 31,794) |
| **Soak stability** | 175,036 ops / 5s, 0 errors | 8-client mixed soak, ~34.5k ops/s sustained |
| **Memory growth (soak)** | 742 KB over 175k ops | 1.14 MB → 1.88 MB used_memory |
| **Connection churn** | 169 conn/s, 0 failures | p50=637µs, p99=1621µs |
| **Quiet pipeline** | ~3.5k ops/s at c=1 | Expected: quiet-pipeline overhead is per-batch, not per-op |

### Benchmark Artifact Provenance

- **Baseline benchmark**: `benchmarks/results/bench_20260402_144029.json` (tag: `verifier-wave9b`)
- **Stress/soak**: `benchmarks/results/stress_20260402_150543.json` (git ref: `891847b`)
- **Symlinks**: `latest.json` → `bench_20260402_144029.json`, `stress_latest.json` → `stress_20260402_150543.json`
- **Platform**: macOS 15.7.4, arm64, Python 3.13.5, Redis 8.4.0

---

## 3. Known Limitations

### 3.1 Counter Precision (post-2^53)

Counter values are exact only for the range `[0, 2^53)`. Beyond `2^53` (9,007,199,254,740,992), the behavior is **precision loss / rounding** rather than reliable wraparound. This is inherent to Redis's use of IEEE 754 double-precision floats for numeric storage via Lua scripts. The memcached binary protocol specifies unsigned 64-bit counter semantics; RedCouch cannot provide bit-exact behavior above 2^53.

### 3.2 Append/Prepend Duration Caveat

APPEND and PREPEND operations retrieve the existing value via Lua hex encode, concatenate, and store back. For items with large accumulated values, each append incurs cost proportional to the existing value size. In the stress suite, 10 keys reached ~61 KB each after ~950 appends of 64B chunks. **For append-heavy workloads with large values, monitor value sizes and consider periodic key rotation.**

### 3.3 Remaining Hot Paths

The following are identified performance costs that remain in the GA release:

1. **Lua hex encode/decode**: Every GET and binary-value mutation passes through Lua `string.format('%02x')` / manual hex decode in Rust. This is the correctness-first approach to avoid redis-module UTF-8 panics on binary payloads.
2. **Per-request `ThreadSafeContext` / GIL**: Each Redis command acquires a `ThreadSafeContext` lock. This serializes Redis access across all connection threads and is the primary concurrency bottleneck above ~4 clients.
3. **Smaller allocation costs**: Per-request `Vec` allocations for key namespacing, hex conversion buffers, and response assembly.

### 3.4 Startup / Bind Caveat

The background TCP listener thread may log readiness (`listening on 127.0.0.1:11210`) before the bind attempt has definitively succeeded. If another process holds port 11210, the module logs a `FATAL: cannot bind` error and the listener thread exits, but Redis itself continues running. **Check for the bind-success log line and verify port 11210 is reachable after module load.**

### 3.5 SASL Authentication

SASL auth is stub-only: `SASL_LIST_MECHS` returns "PLAIN", `SASL_AUTH` always succeeds regardless of credentials. This allows SASL-requiring clients (e.g., Couchbase SDKs) to complete the auth handshake. **No actual credential enforcement exists in this release.**

### 3.6 Malformed Traffic Behavior

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

### 3.7 Deferred Surfaces

The following are **explicitly not in GA scope**:
- Meta protocol (text-path prefix router dispatches `mg`/`ms`/`md`/`ma`/`mn`/`me` to a stub that returns `SERVER_ERROR meta protocol not supported`; actual meta command handling is deferred)
- UDP transport
- Couchbase bucket/vbucket management
- Dynamic STAT groups (settings, items, slabs, conns)
- CI/CD pipeline (no workflows in repo)


---

## 4. Run and Configuration Reference

### Build

```bash
cargo build --release
```

Produces `target/release/libred_couch.dylib` (macOS) or `libred_couch.so` (Linux).

### Load into Redis 8+

```bash
redis-server --loadmodule ./target/release/libred_couch.dylib
```

The module registers as `redcouch` and starts a TCP listener on `127.0.0.1:11210`.

### Runtime Constants

| Parameter | Value | Source |
|---|---|---|
| Bind address | `127.0.0.1:11210` | `DEFAULT_BIND_ADDR` in `src/lib.rs` |
| Max connections | 1,024 | `MAX_CONNECTIONS` |
| Socket read timeout | 30 seconds | `SOCKET_READ_TIMEOUT` |
| Socket write timeout | 10 seconds | `SOCKET_WRITE_TIMEOUT` |
| Max body length per frame | 20 MiB | `MAX_BODY_LEN` in `src/protocol.rs` |
| Max key length | 250 bytes | `MAX_KEY_LEN` in `src/protocol.rs` |
| Max read buffer per connection | ~20 MiB + header + 4 KB | `MAX_READ_BUF` |
| Key prefix | `rc:` | `KEY_PREFIX` |
| CAS counter key | `redcouch:sys:cas_counter` | `CAS_COUNTER_KEY` |

### Verification Commands

```bash
# Build check
cargo check

# Run unit/protocol tests (118 tests — 60 binary + 58 ASCII)
cargo test

# Run E2E integration tests (requires running Redis 8+ with module loaded)
cd tests/integration && bash run_e2e.sh

# Run benchmark suite
cd benchmarks && bash run_benchmarks.sh

# Run stress/soak suite
cd benchmarks && bash run_stress_soak.sh
```

### Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `redis-module` | 2.0.7 | Redis module API bindings |
| `bytes` | 1 | Byte buffer management |
| `byteorder` | 1 | Big-endian integer parsing |
| `thiserror` | 2.0.12 | Error type derivation |

---

## 5. Stress/Soak Validation Statement

**The load/stress/soak validation wave (Wave 10) was validation-only, not a product-behavior change.** No code was modified during the stress/soak wave. The 7-phase suite confirmed the operating envelope of the existing implementation on Redis 8.4.0 and produced the evidence-backed findings documented in Section 2 above. The stress artifacts are stored in `benchmarks/results/stress_20260402_150543.json`.

---

## 6. Test Coverage Summary

| Category | Count | Location |
|---|---|---|
| Binary protocol unit tests | 60 | `src/protocol.rs` (via `cargo test`) |
| ASCII protocol unit tests | 58 | `src/ascii.rs` (via `cargo test`) — 47 parser + 11 meta prefix routing |
| Integration/E2E tests | Suite | `tests/integration/test_binary_protocol.py` |
| Benchmark workloads | 10+ profiles | `benchmarks/bench_binary_protocol.py` |
| Stress/soak workloads | 7 phases | `benchmarks/stress_soak_validation.py` |

Test categories cover: parser round-trips, opcode coverage, quiet/base mapping, frame building, malformed frame handling, oversized frame rejection, binary-safe payloads, CAS preservation, response encoding, unknown opcode echoing, key-length limits, and property-based exhaustive opcode/size sweeps.

---

## 7. GA Release Checklist

### Pre-Release

- [x] **Binary protocol framing**: all 34 opcodes (0x00–0x22, excluding 0x1F) parsed and dispatched
- [x] **ASCII text protocol**: 19 commands (set/add/replace/cas/append/prepend/get/gets/gat/gats/delete/incr/decr/touch/flush_all/version/stats/verbosity/quit)
- [x] **Item model**: hash-per-item with binary-safe value, flags, CAS, and expiry
- [x] **CAS correctness**: monotonic counter, atomic Lua-based mutations, CAS-check on store/delete
- [x] **Expiry semantics**: relative (≤30 days), absolute (>30 days), persist (0), verified via GAT/GATQ
- [x] **Flush isolation**: `FLUSH`/`FLUSHQ` scans and deletes only `rc:*` keys
- [x] **Counter semantics**: unsigned 64-bit with initial value, miss semantics, precision caveat documented
- [x] **Quiet command suppression**: all quiet variants suppress success responses correctly
- [x] **SASL stub**: clients requiring auth handshake can connect
- [x] **STAT support**: general stats with curr_items, uptime, version, hit/miss counters
- [x] **Safe defaults**: loopback-only bind, connection limit (1024), read/write timeouts, body size cap
- [x] **Malformed traffic handling**: no crashes, clean disconnect or error response
- [x] **Binary-safe values**: non-UTF8 payloads round-trip via Lua hex encode/decode
- [x] **Reserved key separation**: user keys under `rc:*`, system keys under `redcouch:sys:*`

### Testing

- [x] **Unit tests pass**: `cargo test` — 118 tests (60 binary + 58 ASCII), 0 failures
- [x] **E2E integration suite**: live Redis 8.4.0 binary-client verification
- [x] **Benchmark baseline captured**: artifact with provenance tag `verifier-wave9b`
- [x] **Stress/soak validation**: 7-phase suite, 0 errors, stable memory, clean malformed handling

### Documentation

- [x] **Feature/compatibility table**: all opcodes with status and notes
- [x] **Operating envelope**: throughput, latency, concurrency scaling, soak stability
- [x] **Known limitations**: counter precision, append growth, hot paths, startup caveat, SASL stub
- [x] **Configuration reference**: all runtime constants with values and sources
- [x] **Benchmark provenance**: artifact filenames, git refs, platform details
- [x] **Deferred surfaces**: meta, UDP, bucket/vbucket explicitly listed as out of scope

### Go / No-Go Decision

| Criterion | Status | Evidence |
|---|---|---|
| All P0 tasks complete | ✅ | Waves 1–9 verified and approved |
| All P1 tasks complete | ✅ | Benchmark, optimization, stress/soak, documentation |
| Zero-error stress/soak run | ✅ | 175k ops, 0 errors, 0 unexpected statuses |
| No crash on malformed traffic | ✅ | 6 malformed scenarios: all clean disconnect/timeout/error |
| Connection churn resilience | ✅ | 169 conn/s, 0 failures |
| Memory stability under soak | ✅ | 742 KB growth over 175k ops |
| Known limitations documented | ✅ | Counter precision, append caveat, hot paths, startup, SASL |
| Deferred work explicitly scoped | ✅ | Meta/UDP/bucket not in GA |

**Recommendation**: **GO** for GA release of the memcached binary + ASCII text protocol over TCP scope on Redis 8+.