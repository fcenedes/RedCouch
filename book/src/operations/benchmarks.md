# Benchmarks & Performance

This chapter presents RedCouch's measured performance characteristics, explains what the numbers mean for your workload, and documents how to reproduce the benchmarks.

## How to Read These Numbers

All benchmarks measure **end-to-end operation latency** — the time from sending a memcached protocol request to receiving the complete response, including network round-trip, protocol parsing, Lua script execution, and response encoding.

- **ops/sec** — Operations per second (higher is better)
- **p50 µs** — Median latency in microseconds (50th percentile)
- **p95/p99 µs** — Tail latency (95th/99th percentile)
- **Errors** — Number of failed operations (should be 0)

The benchmark harness uses Python asyncio clients sending individual operations in a tight loop. Throughput numbers reflect single-system capacity, not network-bound scenarios.

## Throughput Baselines (Single Client)

Source: `benchmarks/results/bench_20260402_144029.json` (Redis 8.4.0, macOS arm64, local — no Docker).

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

**What this tells you:** A single client can sustain ~30k ops/s for SET and ~27k ops/s for GET at sub-100µs p99 latency. GET misses are faster than hits because misses skip the Lua hex-decode step. APPEND is the slowest mutation because it reads, concatenates, and re-encodes the existing value.

## Concurrency Scaling (4 Clients)

| Workload | ops/sec | p50 µs | p95 µs |
|---|---|---|---|
| SET 64B | 62,574 | 60 | 100 |
| SET 1KB | 60,439 | 63 | 104 |
| GET (hit) | 50,929 | 76 | 120 |
| Mixed R/W | 30,138 | 129 | 184 |

**What this tells you:** Throughput roughly doubles from 1 to 4 clients, but latency also increases due to `ThreadSafeContext` lock contention. The optimal concurrency is ~4 clients; beyond that, the lock becomes the bottleneck and throughput plateaus (see Stress/Soak below).

## Cross-System Comparison

To answer "how does RedCouch compare to Couchbase and native Redis?", all three systems were benchmarked under identical conditions using Docker containers with the same resource limits (256 MB maxmemory, no persistence).

Source: `benchmarks/results/cross_system_20260403_092550.json` (symmetric Docker topology).

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

### Interpretation

- **All three systems are closely matched** — at c=1, all systems fall within ±10% of each other for SET, DELETE, and GET miss. The Docker networking layer dominates single-client latency.
- **GET hit is RedCouch's most expensive operation** — ~18% slower than Redis native at c=1 due to the Lua hex-decode overhead. This is the cost of binary-safe value storage.
- **RedCouch is a viable bridge** — The protocol translation layer does not introduce order-of-magnitude penalties. Migration from memcached protocol to native Redis removes the bridge overhead entirely.

> **Note:** The absolute numbers in the Docker comparison (~8k ops/s at c=1) are lower than the local baselines (~30k ops/s) because Docker networking adds latency. The relative comparisons between systems are what matter.

## Stress/Soak Validation

A 7-phase stress suite validated RedCouch's behavior under sustained load:

| Finding | Value | What it means |
|---|---|---|
| Performance sweet spot | 4 clients (~61k ops/s SET) | Optimal concurrency for throughput |
| Post-saturation ceiling | ~35k ops/s at c≥16 | `ThreadSafeContext` lock serialization caps throughput |
| Soak stability | 175,036 ops / 5s, 0 errors | Stable under sustained mixed workload |
| Memory growth (soak) | 742 KB over 175k ops | No memory leaks detected |
| Connection churn | 169 conn/s, 0 failures | Thread-per-connection model handles rapid connect/disconnect |

## Running Benchmarks

### Prerequisites

- Redis 8+ with RedCouch module loaded (for single-system benchmarks)
- Python 3.10+ with `asyncio` support
- Docker (for cross-system comparison only)

### Commands

```bash
# Single-system benchmark (requires Redis 8+ with module loaded)
cd benchmarks && bash run_benchmarks.sh

# Stress/soak validation
cd benchmarks && bash run_stress_soak.sh

# Cross-system three-way comparison (requires Docker)
bash benchmarks/run_cross_system.sh
```

### Benchmark artifacts

Results are stored as JSON files in `benchmarks/results/`. The latest results are symlinked:

| Symlink | Points to |
|---|---|
| `latest.json` | Most recent single-system benchmark |
| `stress_latest.json` | Most recent stress/soak result |

### Reproducing the cross-system comparison

The cross-system benchmark uses Docker Compose to run all three systems:

```bash
# Start all containers (Couchbase, Redis OSS, Redis + RedCouch)
cd benchmarks && docker compose up --build --wait

# Run the benchmark
python3 bench_cross_system.py

# Tear down
docker compose down
```

See `benchmarks/docker-compose.yml` for container configuration and `benchmarks/Dockerfile.redcouch` for the RedCouch container build.
