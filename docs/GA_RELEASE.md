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
| **Meta commands** | ✅ Implemented | `mg`/`ms`/`md`/`ma`/`mn` fully implemented via text-path prefix router. `me` (debug) returns `EN`. Unsupported flags rejected with `CLIENT_ERROR`. See meta matrix below. |

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

## 3. Benchmark Comparison: Couchbase OSS vs Redis OSS vs Redis + RedCouch

This section documents **measured** performance comparisons across three systems, produced by the repository's cross-system benchmark harness under controlled conditions. Architectural expectations are clearly separated from measured facts.

### 3.1 Comparison Dimensions

The comparison covers common key-value operations across three systems:

| Category | Operations | Why It Matters |
|---|---|---|
| **Common key operations** | GET (hit/miss), SET (64B), DELETE | Core data-path throughput and latency for the most frequent operations in a migration path |

### 3.2 Systems Compared (Symmetric Docker Topology)

All three systems run as Docker containers with identical resource constraints (256 MB maxmemory, no persistence) for a true apples-to-apples comparison.

| System | Description | Data Path | Benchmark Transport |
|---|---|---|---|
| **Couchbase OSS** | Couchbase Community 7.2.4, memcached bucket (Docker container) | Native KV engine, memcached binary protocol | memcached binary over TCP (port 11211) |
| **Redis OSS native** | Redis 8 (Docker container) with native `GET`/`SET`/`DEL` commands | Direct Redis data structure access; no protocol translation | RESP protocol over TCP (port 16380) |
| **Redis + RedCouch** | Redis 8 + RedCouch module (Docker container, built from source) | TCP listener → binary parse → Lua hex encode/decode → `HSET`/`HGETALL` → response | memcached binary over TCP (port 11210) |

### 3.3 Measured Three-Way Comparison (Symmetric)

Source: `benchmarks/results/cross_system_20260403_092550.json` (symmetric Docker topology, macOS arm64, Python 3.13.5 harness, 5s per workload, all three systems containerized).

#### Single Client (c=1)

| Operation | RedCouch ops/s | RedCouch p99 µs | Redis OSS ops/s | Redis OSS p99 µs | Couchbase ops/s | Couchbase p99 µs |
|---|---|---|---|---|---|---|
| SET 64B | 7,873 | 199 | 8,039 | 209 | 8,666 | 163 |
| GET (hit) | 6,776 | 226 | 8,294 | 187 | 8,360 | 185 |
| GET (miss) | 8,264 | 190 | 8,265 | 195 | 8,249 | 203 |
| DELETE | 8,421 | 190 | 8,402 | 193 | 8,471 | 187 |

#### Four Clients (c=4)

| Operation | RedCouch ops/s | RedCouch p99 µs | Redis OSS ops/s | Redis OSS p99 µs | Couchbase ops/s | Couchbase p99 µs |
|---|---|---|---|---|---|---|
| SET 64B | 20,710 | 323 | 19,236 | 368 | 19,757 | 528 |
| GET (hit) | 15,098 | 515 | 18,351 | 394 | 21,985 | 314 |
| GET (miss) | 21,992 | 290 | 19,903 | 353 | 22,376 | 274 |
| DELETE | 22,986 | 263 | 21,236 | 321 | 22,090 | 277 |

### 3.4 Measured Findings and Interpretation

**Measured fact**: Under symmetric Docker topology, all three systems perform within the same order of magnitude (~7k–9k ops/s at c=1, ~15k–23k ops/s at c=4). There is no dramatic throughput gap between them; differences are modest and workload-dependent.

**Key findings**:

1. **All three systems are closely matched at c=1** (~7.9k–8.7k ops/s for SET, ~8.2k–8.5k ops/s for DELETE). The Docker networking layer dominates single-client latency for all three, placing them within ±10% of each other for most operations.
2. **GET (hit) is RedCouch's most expensive operation**: at c=1, RedCouch achieves ~6,776 ops/s vs ~8,300 ops/s for Redis OSS and Couchbase, a ~18–19% deficit. This is consistent with the Lua hex-decode overhead on the read path (documented in Section 4.3). At c=4, this gap widens to ~15k vs ~18k–22k ops/s.
3. **SET, GET (miss), and DELETE scale comparably**: at c=4, RedCouch is competitive with or slightly ahead of Redis OSS native for SET (20.7k vs 19.2k), GET miss (22.0k vs 19.9k), and DELETE (23.0k vs 21.2k). Couchbase is generally within the same range.
4. **Zero errors across all systems**: all 24 workload runs (4 operations × 3 systems × 2 concurrency levels) completed with 0 errors.

**What this means for migration**:
- **RedCouch is a viable drop-in bridge**: the Lua hex-encode/decode overhead imposes a measurable but modest cost (~18% on GET hit at c=1), not a performance cliff. For migration scenarios, the protocol translation layer does not introduce order-of-magnitude penalties.
- **The migration path to native Redis removes the bridge overhead**: once clients migrate from memcached binary protocol to native Redis RESP commands, the Lua translation layer is eliminated entirely.
- **GET (hit) is the primary optimization target** if further RedCouch performance tuning is desired (see Section 4.3).

**What the data does not show**:
- Bare-metal performance without Docker networking overhead — all three systems pay the same Docker transport cost.
- Large-payload (1KB, 64KB) comparisons — only 64B values were tested in the cross-system harness.
- Behavior under sustained high load, connection churn, or mixed workloads (see Section 2 and Section 6 for RedCouch-only stress/soak results).

### 3.4.1 Superseded Asymmetric Comparison (Historical Reference)

The prior cross-system comparison (`cross_system_20260402_223529.json`) ran RedCouch on host-native Redis while Redis OSS and Couchbase ran in Docker containers. That topology asymmetry inflated RedCouch's apparent advantage by 2–4× due to Docker networking overhead on macOS. **Those numbers are superseded by the symmetric comparison above.** The asymmetric artifact is retained in the repository for traceability only.

### 3.5 RedCouch-Only Baselines (Local, No Docker)

The following baselines are from the verified single-system benchmark `benchmarks/results/bench_20260402_144029.json` (tag: `verifier-wave9b`, Redis 8.4.0, macOS arm64):

| Operation | 1-Client ops/sec | 1-Client p50 µs | 1-Client p99 µs | 4-Client ops/sec | 4-Client p50 µs |
|---|---|---|---|---|---|
| SET 64B | 31,694 | 29 | 77 | 62,574 | 60 |
| SET 1KB | 29,899 | 31 | 58 | 60,439 | 63 |
| GET (hit) | 26,814 | 35 | 57 | 50,929 | 76 |
| GET (miss) | 36,782 | 26 | 41 | 68,045 | 54 |
| DELETE | 40,038 | 24 | 40 | 66,470 | 55 |
| INCREMENT | 31,883 | 30 | 56 | 66,568 | 56 |
| Mixed R/W | 14,635 | 65 | 95 | 30,138 | 129 |

### 3.6 Architectural Performance Expectations

These are architectural expectations, **not measured cross-system facts**:

1. **RedCouch adds bridge overhead on top of Redis**: each request traverses TCP accept → binary parse → Lua script → hex encode/decode → `HSET`/`HGETALL` → response. This overhead means RedCouch should be slower than raw Redis `GET`/`SET`/`DEL` on the same host.
2. **The dominant cost is the Lua hex encode/decode bridge**: every GET and binary-value mutation passes through Lua `string.format('%02x')` encoding and manual hex decode in Rust (see Section 4.3).
3. **Per-request `ThreadSafeContext` / GIL serialization** limits scaling above ~4 clients to ~35k ops/s for contended workloads (verified in `stress_20260402_150543.json`).
4. **For migration positioning**: RedCouch is designed as a transitional bridge. The migration path is Couchbase → Redis + RedCouch → native Redis. The cross-system data confirms that RedCouch performance is competitive with the other systems under test conditions, and migrating to native Redis commands would remove the bridge overhead entirely.

### 3.7 Measurement Methodology

- **Symmetric topology**: All three systems run as Docker containers via `benchmarks/docker-compose.yml` with identical resource constraints (256 MB maxmemory, no persistence). RedCouch is built from source in a multi-stage Docker build (`benchmarks/Dockerfile.redcouch`) using the same Redis 8 base image as the Redis OSS benchmark container.
- **Cross-system harness**: `benchmarks/bench_cross_system.py` — drives identical workload profiles against all three systems under the same client, duration, and concurrency parameters.
- **Single-system harness**: `benchmarks/bench_binary_protocol.py` — comprehensive RedCouch-only benchmark with 10 workload profiles.
- **Environment setup**: `benchmarks/docker-compose.yml` provisions Couchbase OSS (Community 7.2.4, memcached bucket), Redis OSS (Redis 8), and Redis + RedCouch (Redis 8 + module built from source). `benchmarks/setup_couchbase.sh` initializes the Couchbase cluster and memcached bucket.
- **Reproducibility**: `bash benchmarks/run_cross_system.sh` runs the full setup → benchmark → teardown flow. Uses `docker compose up --build --wait` for health-checked startup.

### 3.8 Benchmark Artifact Provenance

| Artifact | Path | Tag/Ref | Content |
|---|---|---|---|
| **Cross-system comparison (symmetric)** | `benchmarks/results/cross_system_20260403_092550.json` | symmetric rerun | 4 workloads × 3 systems × 2 concurrency levels, all Docker |
| Cross-system comparison (asymmetric, superseded) | `benchmarks/results/cross_system_20260402_223529.json` | `cross-system-v1` | Superseded: RedCouch host-native, others Docker |
| RedCouch-only baseline | `benchmarks/results/bench_20260402_144029.json` | `verifier-wave9b` | 10 workloads × 2 concurrency levels (c=1, c=4) |
| Stress/soak results | `benchmarks/results/stress_20260402_150543.json` | git ref `891847b` | 7-phase stress suite |
| Cross-system harness | `benchmarks/bench_cross_system.py` | — | Three-way comparison driver |
| Cross-system runner | `benchmarks/run_cross_system.sh` | — | Docker Compose orchestration + benchmark flow |
| Docker Compose | `benchmarks/docker-compose.yml` | — | Couchbase Community 7.2.4 + Redis 8 + RedCouch (all containerized) |
| RedCouch Dockerfile | `benchmarks/Dockerfile.redcouch` | — | Multi-stage build: Rust source → Redis 8 module container |
| Couchbase setup | `benchmarks/setup_couchbase.sh` | — | Cluster init + memcached bucket creation |
| Single-system harness | `benchmarks/bench_binary_protocol.py` | — | Python 3.13.5, RedCouch-only benchmark |
| Platform | macOS 15.7.4, arm64 | — | Docker Desktop, Python 3.13.5 |

---

## 4. Known Limitations

### 4.1 Counter Precision (post-2^53)

Counter values are exact only for the range `[0, 2^53)`. Beyond `2^53` (9,007,199,254,740,992), the behavior is **precision loss / rounding** rather than reliable wraparound. This is inherent to Redis's use of IEEE 754 double-precision floats for numeric storage via Lua scripts. The memcached binary protocol specifies unsigned 64-bit counter semantics; RedCouch cannot provide bit-exact behavior above 2^53.

### 4.2 Append/Prepend Duration Caveat

APPEND and PREPEND operations retrieve the existing value via Lua hex encode, concatenate, and store back. For items with large accumulated values, each append incurs cost proportional to the existing value size. In the stress suite, 10 keys reached ~61 KB each after ~950 appends of 64B chunks. **For append-heavy workloads with large values, monitor value sizes and consider periodic key rotation.**

### 4.3 Remaining Hot Paths

The following are identified performance costs that remain in the GA release:

1. **Lua hex encode/decode**: Every GET and binary-value mutation passes through Lua `string.format('%02x')` / manual hex decode in Rust. This is the correctness-first approach to avoid redis-module UTF-8 panics on binary payloads.
2. **Per-request `ThreadSafeContext` / GIL**: Each Redis command acquires a `ThreadSafeContext` lock. This serializes Redis access across all connection threads and is the primary concurrency bottleneck above ~4 clients.
3. **Smaller allocation costs**: Per-request `Vec` allocations for key namespacing, hex conversion buffers, and response assembly.

### 4.4 Startup / Bind Caveat

The background TCP listener thread may log readiness (`listening on 127.0.0.1:11210`) before the bind attempt has definitively succeeded. If another process holds port 11210, the module logs a `FATAL: cannot bind` error and the listener thread exits, but Redis itself continues running. **Check for the bind-success log line and verify port 11210 is reachable after module load.**

### 4.5 SASL Authentication

SASL auth is stub-only: `SASL_LIST_MECHS` returns "PLAIN", `SASL_AUTH` always succeeds regardless of credentials. This allows SASL-requiring clients (e.g., Couchbase SDKs) to complete the auth handshake. **No actual credential enforcement exists in this release.**

### 4.6 Malformed Traffic Behavior

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

### 4.7 Deferred Surfaces

The following are **explicitly not in GA scope**:
- Meta protocol **stale items** (`N`/vivify on mg, `I`/invalidate on md, `R`/recache, `W`/`X`/`Z` stale flags, `b`/base64 keys) — these require a stale item concept not in the current item model
- UDP transport
- Couchbase bucket/vbucket management
- Dynamic STAT groups (settings, items, slabs, conns)
- CI/CD pipeline enhancements beyond ci.yml and release.yml


---

## 5. Run, Configuration, and Release Reference

**GitHub Releases is the primary release channel for RedCouch.** For the complete maintainer runbook — including the step-by-step process to cut a GitHub Release, artifact verification, and optional crates.io enablement — see **[`docs/INSTALL.md`](INSTALL.md#release-process-github-release--primary)**.

For installation instructions, building from source, loading into Redis, and troubleshooting, see **[`docs/INSTALL.md`](INSTALL.md)**.

For runtime constants and architecture details, see **[`docs/ARCHITECTURE.md`](ARCHITECTURE.md)**.

### Quick Reference

```bash
# Build
cargo build --release

# Load into Redis 8+
redis-server --loadmodule ./target/release/libred_couch.dylib  # macOS
redis-server --loadmodule ./target/release/libred_couch.so     # Linux

# Verify
redis-cli MODULE LIST    # should show "redcouch"
nc -z 127.0.0.1 11210   # should succeed
```

### Verification Commands

```bash
cargo check                                      # Build check
cargo test                                       # 221 unit/protocol tests
cd tests/integration && bash run_e2e.sh          # E2E (requires Redis 8+)
cd benchmarks && bash run_benchmarks.sh          # Benchmark suite
cd benchmarks && bash run_stress_soak.sh         # Stress/soak suite
bash benchmarks/run_cross_system.sh              # Cross-system comparison (requires Docker)
```

### crates.io Publication (Secondary — Not Required)

Source publication to crates.io is **secondary and policy-gated**. It is not part of the primary GitHub Release path. `Cargo.toml` metadata is configured, but live publication remains disabled until maintainers explicitly confirm the MIT license for public distribution. The release workflow includes a gated `publish-crate` job controlled by the `PUBLISH_CRATE` repository variable. See [`docs/INSTALL.md`](INSTALL.md#cratesio-secondary--policy-gated) for enablement steps.

---

## 6. Stress/Soak Validation Statement

**The load/stress/soak validation wave (Wave 10) was validation-only, not a product-behavior change.** No code was modified during the stress/soak wave. The 7-phase suite confirmed the operating envelope of the existing implementation on Redis 8.4.0 and produced the evidence-backed findings documented in Section 2 above. The stress artifacts are stored in `benchmarks/results/stress_20260402_150543.json`.

---

## 7. Test Coverage Summary

For test architecture details and development workflow, see **[`docs/ARCHITECTURE.md`](ARCHITECTURE.md#test-architecture)** and **[`CONTRIBUTING.md`](../CONTRIBUTING.md)**.

| Category | Count | Location |
|---|---|---|
| Binary protocol unit tests | 76 | `src/protocol.rs` (via `cargo test`) |
| ASCII protocol unit tests | 97 | `src/ascii.rs` (via `cargo test`) — parser, error paths, key validation, meta prefix routing |
| Meta protocol unit tests | 48 | `src/meta.rs` (via `cargo test`) — parser, flag validation, mode validation, numeric token validation, bare-flag rejection, edge cases |
| Integration/E2E tests | Suite | `tests/integration/test_binary_protocol.py` |
| Benchmark workloads | 10+ profiles | `benchmarks/bench_binary_protocol.py` |
| Stress/soak workloads | 7 phases | `benchmarks/stress_soak_validation.py` |

Test categories cover: parser round-trips, opcode coverage, quiet/base mapping, frame building, malformed frame handling, oversized frame rejection, binary-safe payloads, CAS preservation, response encoding, unknown opcode echoing, key-length limits, and property-based exhaustive opcode/size sweeps.

---

## 8. GA Release Checklist

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

- [x] **Unit tests pass**: `cargo test` — 221 tests (76 binary + 97 ASCII + 48 meta), 0 failures
- [x] **E2E integration suite**: live Redis 8.4.0 binary-client verification
- [x] **Benchmark baseline captured**: artifact with provenance tag `verifier-wave9b`
- [x] **Cross-system comparison**: symmetric three-way measured comparison (Couchbase OSS, Redis OSS, RedCouch — all Docker) with artifact `cross_system_20260403_092550.json`
- [x] **Stress/soak validation**: 7-phase suite, 0 errors, stable memory, clean malformed handling

### Documentation

- [x] **Feature/compatibility table**: all opcodes with status and notes
- [x] **Operating envelope**: throughput, latency, concurrency scaling, soak stability
- [x] **Known limitations**: counter precision, append growth, hot paths, startup caveat, SASL stub
- [x] **Configuration reference**: all runtime constants with values and sources
- [x] **Benchmark provenance**: artifact filenames, git refs, platform details
- [x] **Deferred surfaces**: meta stale items, UDP, bucket/vbucket explicitly listed as out of scope

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
| Deferred work explicitly scoped | ✅ | Meta stale items/UDP/bucket not in GA |

**Recommendation**: **GO** for GA release of the memcached binary + ASCII text protocol over TCP scope on Redis 8+.

---

## 9. Release-Readiness and Publish-Enablement Checklist

This section captures all prerequisites for cutting a release and enabling crates.io publication. Items are grouped by category with explicit status and blockers.

### 9.1 License Verification

| Check | Status | Evidence |
|---|---|---|
| `LICENSE` file present at repo root | ✅ | Standard MIT License text, Copyright (c) 2026 RedCouch Contributors |
| `Cargo.toml` `license` field | ✅ | `license = "MIT"` |
| `README.md` License section | ✅ | "MIT — see [LICENSE](LICENSE) for details" |
| `CONTRIBUTING.md` license reference | ✅ | "contributions will be licensed under the MIT License" |
| `docs/GA_RELEASE.md` crates.io section | ✅ | References MIT license and policy gate |
| `docs/INSTALL.md` crates.io section | ✅ | References MIT license and policy gate |
| Source file headers | ℹ️ N/A | No source-header license convention adopted; `LICENSE` file at root is the sole license indicator (standard for Rust crates) |
| License consistency across all references | ✅ | All references say MIT; no conflicting license mentions anywhere in the repository |

### 9.2 Cargo.toml Metadata for crates.io

| Field | Status | Value |
|---|---|---|
| `name` | ✅ | `red_couch` |
| `version` | ✅ | `0.1.0` |
| `edition` | ✅ | `2024` |
| `rust-version` | ✅ | `1.85` |
| `description` | ✅ | "Redis module bridging Couchbase memcached binary protocol clients to Redis 8+" |
| `license` | ✅ | `MIT` |
| `repository` | ✅ | `https://github.com/fcenedes/RedCouch` |
| `homepage` | ✅ | `https://github.com/fcenedes/RedCouch` |
| `readme` | ✅ | `README.md` |
| `keywords` | ✅ | `["redis", "memcached", "couchbase", "module", "protocol"]` |
| `categories` | ✅ | `["database", "network-programming"]` |
| `publish` field | ℹ️ | Not set (defaults to `true`). No code change needed — the workflow gate is the publish control. |

### 9.3 GitHub Release Automation

| Check | Status | Evidence |
|---|---|---|
| CI workflow (`ci.yml`) | ✅ | Runs on push/PR to `main`; Ubuntu + macOS; check, test, clippy, fmt, doc |
| Release workflow (`release.yml`) | ✅ | Triggered by `v*` tags; builds 4 targets (Linux x86_64, Linux ARM64, macOS x86_64, macOS ARM64) |
| Tag/version validation (`validate-tag` job) | ✅ | Fails fast if tag version does not match `Cargo.toml` version |
| Test gate before release | ✅ | `test` job runs `cargo test` + `cargo clippy --all-targets -- -D warnings` + `cargo fmt --check` |
| GitHub Release creation | ✅ | `softprops/action-gh-release@v2` with auto-generated release notes and artifact upload |
| SHA-256 checksums | ✅ | Each target produces `.tar.gz.sha256` alongside `.tar.gz` |
| Windows targets | ✅ Not included | Correctly excluded — Windows is not a supported target |

**GitHub Release is the primary output** of this workflow. No crates.io publication occurs unless explicitly opted in (see Section 9.4).

### 9.4 crates.io Publication Gate (Secondary — Not Required)

crates.io publication is **not part of the primary release path**. It is an optional secondary step, gated by policy.

| Check | Status | Evidence |
|---|---|---|
| `publish-crate` job exists in `release.yml` | ✅ | Runs only after build + test + GitHub Release all succeed |
| Gated by `PUBLISH_CRATE` variable | ✅ | `if: ${{ vars.PUBLISH_CRATE == 'true' }}` — disabled by default |
| `CARGO_REGISTRY_TOKEN` secret required | ✅ | Referenced in the publish step |
| Documentation of gate in `docs/INSTALL.md` | ✅ | Section "crates.io (Secondary)" explains the policy gate |
| Documentation of gate in `docs/GA_RELEASE.md` | ✅ | Section 5 "crates.io Publication (Secondary)" explains the policy gate |

### 9.5 GitHub Release Runbook (Maintainer Steps)

This is the primary release path. crates.io is not required.

1. **Verify CI is green**: confirm the latest `main` commit passes CI (`ci.yml`: check, test, clippy, fmt, doc).
2. **Set the version**: update `version` in `Cargo.toml` and run `cargo check` to update `Cargo.lock`.
3. **Create and push the tag** (the tag version must match `Cargo.toml`):
   ```bash
   git tag v0.1.0
   git push origin v0.1.0
   ```
4. **The release workflow runs automatically**:
   - `validate-tag` — confirms tag matches `Cargo.toml` version.
   - `test` — runs `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
   - `build` — cross-compiles for all 4 supported targets.
   - `publish-github` — creates the GitHub Release with auto-generated notes and attaches `.tar.gz` + `.sha256` artifacts.
5. **Verify the release**: check the GitHub Release page at `https://github.com/fcenedes/RedCouch/releases` for all 4 target artifacts with checksums.

> **Optional**: To also publish to crates.io, set `PUBLISH_CRATE=true` as a repository variable and add `CARGO_REGISTRY_TOKEN` as a secret. The `publish-crate` job will then run automatically after the GitHub Release. See Section 9.4.

### 9.6 Items Blocked on Maintainer/Policy Confirmation

| Item | Blocker | What's Ready | What's Needed |
|---|---|---|---|
| **First GitHub Release** | Maintainer decision on release timing | CI, release workflow, documentation, and test suite all ready | Push a `v*` tag to trigger the release workflow |
| **crates.io publication** (optional) | Explicit maintainer confirmation that MIT is the intended license for public crate distribution | `Cargo.toml` metadata complete, `publish-crate` job exists, `LICENSE` file present | Set `PUBLISH_CRATE=true` as a repository variable and add `CARGO_REGISTRY_TOKEN` secret |

### 9.7 Verified Facts (Carried Forward)

- ✅ Linux/macOS release automation exists and covers 4 targets
- ✅ Windows is unsupported and correctly excluded from all workflows and documentation
- ✅ crates.io publication remains policy-gated pending explicit MIT confirmation
- ✅ `cargo check` passes, `cargo test` passes with 221 tests (76 binary + 97 ASCII + 48 meta)
- ✅ Built-ins (EVALSHA migration + non-CAS DELETE bypass), benchmark comparison docs, and open-source documentation set are complete and reflected in `docs/GA_RELEASE.md`
- ✅ No `package.json` exists (correct: this is a Rust crate, not a Node.js package)