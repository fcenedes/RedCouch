#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────
# Cross-System Benchmark Runner
#
# Orchestrates a three-way comparison:
#   1. Redis + RedCouch (memcached binary protocol via module)
#   2. Redis OSS native (direct RESP GET/SET/DEL)
#   3. Couchbase OSS (memcached binary protocol, Docker)
#
# Usage:
#   bash benchmarks/run_cross_system.sh
#
# Prerequisites:
#   - docker compose (for Redis OSS + Couchbase containers)
#   - redis-server 8+ in PATH (for RedCouch module)
#   - cargo build --release (module must be built)
#   - python3 in PATH
#
# Environment variables:
#   BENCH_DURATION  - seconds per workload (default 5)
#   BENCH_CLIENTS   - comma-separated concurrency (default "1,4")
#   BENCH_TAG       - optional run tag
#
# This runner requires ALL THREE systems to be operational for a valid
# three-way comparison. It will fail if any system is unreachable.
# ──────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# Detect module
MODULE="$ROOT_DIR/target/release/libred_couch.dylib"
[ -f "$MODULE" ] || MODULE="$ROOT_DIR/target/release/libred_couch.so"
if [ ! -f "$MODULE" ]; then
    echo "ERROR: Module not found. Run 'cargo build --release' first."
    exit 1
fi

REDIS_PORT=16379         # RedCouch's Redis instance
MEMCACHED_PORT=11210     # RedCouch listener
REDIS_NATIVE_PORT=16380  # Docker Redis OSS
COUCHBASE_PORT=11211     # Docker Couchbase KV
REDIS_DIR=$(mktemp -d)
REDIS_PID=""
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
RESULTS_DIR="$SCRIPT_DIR/results"
RESULT_FILE="$RESULTS_DIR/cross_system_${TIMESTAMP}.json"
LATEST_FILE="$RESULTS_DIR/cross_system_latest.json"

cleanup() {
    echo ""
    echo "Cleaning up..."
    if [ -n "$REDIS_PID" ] && kill -0 "$REDIS_PID" 2>/dev/null; then
        kill "$REDIS_PID" 2>/dev/null || true
        wait "$REDIS_PID" 2>/dev/null || true
    fi
    rm -rf "$REDIS_DIR"
    # Stop Docker containers
    docker compose -f "$SCRIPT_DIR/docker-compose.yml" down -v 2>/dev/null || true
}
trap cleanup EXIT

echo "═══════════════════════════════════════════════════════════"
echo "Cross-System Benchmark Runner"
echo "═══════════════════════════════════════════════════════════"
echo ""

# 1. Start Docker containers (Redis OSS + Couchbase)
echo "Starting Docker containers..."
docker compose -f "$SCRIPT_DIR/docker-compose.yml" up -d
echo "  Waiting for containers to be healthy..."
sleep 5

# Verify Redis OSS container
if redis-cli -p "$REDIS_NATIVE_PORT" PING 2>/dev/null | grep -q "PONG"; then
    echo "  ✅ Redis OSS native ready on port $REDIS_NATIVE_PORT"
else
    echo "ERROR: Redis OSS native not responding on port $REDIS_NATIVE_PORT"
    echo "  Docker container may not have started. Check: docker compose -f benchmarks/docker-compose.yml logs redis-bench"
    exit 1
fi

# 2. Setup Couchbase
echo ""
bash "$SCRIPT_DIR/setup_couchbase.sh" || {
    echo "ERROR: Couchbase setup failed. Cannot run three-system comparison."
    exit 1
}

# 3. Start local Redis with RedCouch module
echo ""
echo "Starting Redis + RedCouch module..."
redis-server \
    --port "$REDIS_PORT" \
    --dir "$REDIS_DIR" \
    --daemonize no \
    --loglevel notice \
    --loadmodule "$MODULE" \
    &>"$REDIS_DIR/redis.log" &
REDIS_PID=$!

# Wait for RedCouch listener
echo "Waiting for RedCouch listener on port $MEMCACHED_PORT..."
REDCOUCH_READY=0
for i in $(seq 1 15); do
    if python3 -c "
import socket, sys
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.settimeout(1)
try:
    s.connect(('127.0.0.1', $MEMCACHED_PORT))
    s.close()
    sys.exit(0)
except:
    sys.exit(1)
" 2>/dev/null; then
        echo "  ✅ RedCouch ready after ${i}s"
        REDCOUCH_READY=1
        break
    fi
    if ! kill -0 "$REDIS_PID" 2>/dev/null; then
        echo "ERROR: Redis+RedCouch process exited. Log:"
        tail -20 "$REDIS_DIR/redis.log" 2>/dev/null || true
        exit 1
    fi
    sleep 1
done
if [ "$REDCOUCH_READY" -ne 1 ]; then
    echo "ERROR: RedCouch listener never became ready on port $MEMCACHED_PORT"
    echo "  Redis log tail:"
    tail -20 "$REDIS_DIR/redis.log" 2>/dev/null || true
    exit 1
fi

# 4. Run cross-system benchmarks (all three systems required)
echo ""
mkdir -p "$RESULTS_DIR"

BENCH_EXIT=0
BENCH_OUTPUT="$RESULT_FILE" \
REDCOUCH_HOST="127.0.0.1" \
REDCOUCH_PORT="$MEMCACHED_PORT" \
REDIS_NATIVE_HOST="127.0.0.1" \
REDIS_NATIVE_PORT="$REDIS_NATIVE_PORT" \
COUCHBASE_HOST="127.0.0.1" \
COUCHBASE_PORT="$COUCHBASE_PORT" \
    python3 "$SCRIPT_DIR/bench_cross_system.py" || BENCH_EXIT=$?

if [ $BENCH_EXIT -ne 0 ]; then
    echo "ERROR: Benchmark harness failed with exit code $BENCH_EXIT"
    exit $BENCH_EXIT
fi

# Verify the result file was produced
if [ ! -f "$RESULT_FILE" ]; then
    echo "ERROR: Expected result file not produced: $RESULT_FILE"
    exit 1
fi

# Verify all three systems are present in the results
SYSTEMS_COUNT=$(python3 -c "
import json
with open('$RESULT_FILE') as f:
    data = json.load(f)
print(len(data['meta']['systems_tested']))
" 2>/dev/null || echo "0")

if [ "$SYSTEMS_COUNT" -ne 3 ]; then
    echo "ERROR: Expected 3 systems in results, found $SYSTEMS_COUNT. Incomplete comparison."
    exit 1
fi

# Symlink latest
ln -sf "$(basename "$RESULT_FILE")" "$LATEST_FILE.tmp"
mv -f "$LATEST_FILE.tmp" "$LATEST_FILE"
echo ""
echo "Results: $RESULT_FILE"
echo "Symlink: cross_system_latest.json → $(basename "$RESULT_FILE")"

echo ""
echo "✅ Cross-system benchmark complete (all 3 systems verified)"
