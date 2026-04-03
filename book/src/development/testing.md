# Test Architecture

Tests are structured to run without a live Redis instance wherever possible.

## Test Categories

| Category | Count | Location |
|---|---|---|
| Binary protocol unit tests | 76 | `src/protocol.rs` |
| ASCII protocol unit tests | 97 | `src/ascii.rs` |
| Meta protocol unit tests | 48 | `src/meta.rs` |
| Integration/E2E tests | Suite | `tests/integration/test_binary_protocol.py` |
| Benchmark workloads | 10+ profiles | `benchmarks/bench_binary_protocol.py` |
| Stress/soak workloads | 7 phases | `benchmarks/stress_soak_validation.py` |

## Host-Process Testing

All protocol/parser modules use `#[cfg(not(test))]` guards to exclude Redis allocator dependencies during `cargo test`, enabling host-process testing without Redis.

```bash
# Run all 221 protocol/parser tests (no Redis required)
cargo test
```

Test categories cover: parser round-trips, opcode coverage, quiet/base mapping, frame building, malformed frame handling, oversized frame rejection, binary-safe payloads, CAS preservation, response encoding, unknown opcode echoing, key-length limits, and property-based exhaustive opcode/size sweeps.

## Integration Testing

Integration tests require a live Redis 8+ instance with the module loaded:

```bash
# Start Redis with module
redis-server --loadmodule ./target/release/libred_couch.dylib  # macOS

# Run E2E tests
cd tests/integration && bash run_e2e.sh
```

## Benchmark Testing

```bash
# Single-system benchmark
cd benchmarks && bash run_benchmarks.sh

# Stress/soak validation
cd benchmarks && bash run_stress_soak.sh

# Cross-system comparison (requires Docker)
bash benchmarks/run_cross_system.sh
```

See [Benchmarks & Performance](../operations/benchmarks.md) for results and methodology.
