# Benchmarks & Performance

## Throughput Baselines (Single Client)

Source: `benchmarks/results/bench_20260402_144029.json` (Redis 8.4.0, macOS arm64).

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

## Concurrency Scaling (4 Clients)

| Workload | ops/sec | p50 µs | p95 µs |
|---|---|---|---|
| SET 64B | 62,574 | 60 | 100 |
| SET 1KB | 60,439 | 63 | 104 |
| GET (hit) | 50,929 | 76 | 120 |
| Mixed R/W | 30,138 | 129 | 184 |

## Cross-System Comparison (Symmetric Docker)

Source: `benchmarks/results/cross_system_20260403_092550.json`. All three systems containerized with identical resource constraints (256 MB maxmemory, no persistence).

### Single Client (c=1)

| Operation | RedCouch ops/s | Redis OSS ops/s | Couchbase ops/s |
|---|---|---|---|
| SET 64B | 7,873 | 8,039 | 8,666 |
| GET (hit) | 6,776 | 8,294 | 8,360 |
| GET (miss) | 8,264 | 8,265 | 8,249 |
| DELETE | 8,421 | 8,402 | 8,471 |

### Four Clients (c=4)

| Operation | RedCouch ops/s | Redis OSS ops/s | Couchbase ops/s |
|---|---|---|---|
| SET 64B | 20,710 | 19,236 | 19,757 |
| GET (hit) | 15,098 | 18,351 | 21,985 |
| GET (miss) | 21,992 | 19,903 | 22,376 |
| DELETE | 22,986 | 21,236 | 22,090 |

**Key finding**: Under symmetric Docker topology, all three systems perform within the same order of magnitude. RedCouch is a viable drop-in bridge — the Lua hex-encode/decode overhead imposes a measurable but modest cost (~18% on GET hit at c=1), not a performance cliff.

## Stress/Soak Summary

| Finding | Value |
|---|---|
| Performance sweet spot | 4 clients (~61k ops/s SET) |
| Post-saturation ceiling | ~35k ops/s at c≥16 |
| Soak stability | 175,036 ops / 5s, 0 errors |
| Memory growth (soak) | 742 KB over 175k ops |
| Connection churn | 169 conn/s, 0 failures |

## Running Benchmarks

```bash
# Single-system benchmark (requires Redis 8+ with module loaded)
cd benchmarks && bash run_benchmarks.sh

# Stress/soak validation
cd benchmarks && bash run_stress_soak.sh

# Cross-system comparison (requires Docker)
bash benchmarks/run_cross_system.sh
```

For full benchmark methodology, artifact provenance, and interpretation, see the [GA Release Documentation](https://github.com/fcenedes/RedCouch/blob/main/docs/GA_RELEASE.md).
