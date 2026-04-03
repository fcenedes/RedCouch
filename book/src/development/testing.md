# Test Architecture

RedCouch has a layered test strategy: fast unit tests that run without Redis, integration tests that exercise the full protocol stack against a live Redis instance, and benchmark/stress suites for performance validation.

## Test Summary

| Category | Count | Location | Requires Redis? |
|---|---|---|---|
| Binary protocol unit tests | 76 | `src/protocol.rs` | No |
| ASCII protocol unit tests | 97 | `src/ascii.rs` | No |
| Meta protocol unit tests | 48 | `src/meta.rs` | No |
| Integration/E2E tests | Suite | `tests/integration/test_binary_protocol.py` | Yes (Redis 8+) |
| Benchmark workloads | 10+ profiles | `benchmarks/bench_binary_protocol.py` | Yes (Redis 8+) |
| Stress/soak workloads | 7 phases | `benchmarks/stress_soak_validation.py` | Yes (Redis 8+) |

**Total unit test count:** 221 tests (76 + 97 + 48) — all run via `cargo test` without Redis.

## Host-Process Testing (No Redis Required)

The key design decision in RedCouch's test architecture is that **protocol parsing and encoding tests run without Redis**. This is achieved through `#[cfg(not(test))]` guards that exclude Redis allocator and module dependencies during `cargo test`. The test binary runs as a normal host process, not inside Redis.

```bash
# Run all 221 unit tests (no Redis needed)
cargo test
```

### What the unit tests cover

| Area | Examples |
|---|---|
| **Parser round-trips** | Encode a request → parse it → verify fields match |
| **Opcode coverage** | Every supported opcode has at least one test |
| **Quiet/base mapping** | Quiet opcodes map to correct base opcodes |
| **Frame building** | Response frames have correct header fields, body length, CAS |
| **Malformed handling** | Truncated headers, oversized bodies, invalid opcodes |
| **Binary-safe payloads** | Non-UTF-8 bytes preserved through encode/decode |
| **CAS preservation** | CAS tokens round-trip through request/response |
| **Key validation** | Empty keys, oversized keys, boundary lengths |
| **Meta flag validation** | Unsupported flags rejected, proxy hints accepted |
| **Property-based sweeps** | Exhaustive opcode and size combinations |

### Writing new unit tests

New protocol tests follow a consistent pattern:

1. **Construct a request** using the protocol builder functions
2. **Parse it** using the protocol parser
3. **Assert** the parsed fields match expectations

Tests live in `#[cfg(test)] mod tests` blocks within each protocol module. Example pattern from `src/protocol.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_set_request() {
        // Build a SET request with known key, value, flags, expiry
        let request = build_request(Opcode::Set, key, extras, value);
        // Parse it back
        let parsed = parse_request(&request).unwrap();
        // Verify all fields
        assert_eq!(parsed.opcode, Opcode::Set);
        assert_eq!(parsed.key, key);
        // ...
    }
}
```

## Integration Testing (Requires Redis 8+)

Integration tests exercise the full stack: TCP connection → protocol parsing → Redis execution → response encoding.

### Setup

```bash
# Build the module
cargo build --release

# Start Redis with the module loaded
redis-server --loadmodule ./target/release/libred_couch.dylib  # macOS
redis-server --loadmodule ./target/release/libred_couch.so     # Linux

# Verify the module is loaded and listening
redis-cli MODULE LIST
nc -z 127.0.0.1 11210 && echo "RedCouch listening"
```

### Running E2E tests

```bash
# In a separate terminal (Redis must be running with module)
cd tests/integration && bash run_e2e.sh
```

The E2E test suite (`tests/integration/test_binary_protocol.py`) uses raw socket connections to send binary protocol frames and verify responses. It covers all 34 binary opcodes with correct and error cases.

## Benchmark and Stress Testing

Benchmarks and stress tests are separate from the correctness test suite. They require Redis 8+ with the module loaded and produce JSON result files.

```bash
# Performance benchmark (10 workload profiles, 2 concurrency levels)
cd benchmarks && bash run_benchmarks.sh

# Stress/soak validation (7 phases — concurrency scaling, soak, connection churn)
cd benchmarks && bash run_stress_soak.sh

# Three-way cross-system comparison (requires Docker)
bash benchmarks/run_cross_system.sh
```

Results are stored in `benchmarks/results/` as timestamped JSON files. See [Benchmarks & Performance](../operations/benchmarks.md) for interpretation and measured data.

## CI Integration

The CI pipeline runs unit tests automatically on every push and PR:

```bash
# What CI runs (Ubuntu + macOS):
cargo check --all-targets
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

Integration tests and benchmarks are **not** run in CI — they require a live Redis instance with the module loaded. Run them locally before submitting performance-sensitive changes.
