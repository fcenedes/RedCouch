#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────
# RedCouch Benchmark Runner
#
# Usage:
#   ./benchmarks/run_benchmarks.sh                    # default: 5s, 1/4/16 clients
#   BENCH_DURATION=10 ./benchmarks/run_benchmarks.sh  # 10s per workload
#   BENCH_CLIENTS="1,8,32" ./benchmarks/run_benchmarks.sh
#   BENCH_TAG="v0.1-baseline" ./benchmarks/run_benchmarks.sh
#
# Requirements:
#   - redis-server 8.0+ in PATH
#   - python3 in PATH
#   - Module already built: cargo build --release
#
# The script:
#   1. Starts a private Redis instance with the module loaded
#   2. Waits for the binary-protocol listener on port 11210
#   3. Runs the benchmark suite
#   4. Stores results in benchmarks/results/ with timestamp
#   5. Tears down the Redis instance
#
# Environment variables passed through to bench_binary_protocol.py:
#   BENCH_HOST, BENCH_PORT, BENCH_DURATION, BENCH_CLIENTS, BENCH_TAG
# ──────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# Detect module path (macOS .dylib vs Linux .so)
MODULE="$ROOT_DIR/target/release/libred_couch.dylib"
if [ ! -f "$MODULE" ]; then
    MODULE="$ROOT_DIR/target/release/libred_couch.so"
fi
if [ ! -f "$MODULE" ]; then
    echo "ERROR: Module not found. Run 'cargo build --release' first."
    exit 1
fi

# Use a non-default Redis port to avoid conflicts
REDIS_PORT="${REDIS_PORT:-16379}"
MEMCACHED_PORT="${BENCH_PORT:-11210}"
REDIS_DIR=$(mktemp -d)
REDIS_PID=""
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
RESULTS_DIR="$SCRIPT_DIR/results"
RESULT_FILE="$RESULTS_DIR/bench_${TIMESTAMP}.json"
LATEST_FILE="$RESULTS_DIR/latest.json"

cleanup() {
    if [ -n "$REDIS_PID" ] && kill -0 "$REDIS_PID" 2>/dev/null; then
        echo ""
        echo "Stopping Redis (PID $REDIS_PID)..."
        kill "$REDIS_PID" 2>/dev/null || true
        wait "$REDIS_PID" 2>/dev/null || true
    fi
    rm -rf "$REDIS_DIR"
}
trap cleanup EXIT

echo "═══════════════════════════════════════════════════════════"
echo "RedCouch Benchmark Runner"
echo "═══════════════════════════════════════════════════════════"
echo "  Redis server:   $(redis-server --version 2>/dev/null | head -1)"
echo "  Module:         $MODULE"
echo "  Redis port:     $REDIS_PORT"
echo "  Memcached port: $MEMCACHED_PORT"
echo "  Results file:   $RESULT_FILE"
echo ""

# Start Redis with the module
echo "Starting Redis with module..."
redis-server \
    --port "$REDIS_PORT" \
    --dir "$REDIS_DIR" \
    --daemonize no \
    --loglevel notice \
    --loadmodule "$MODULE" \
    &>"$REDIS_DIR/redis.log" &
REDIS_PID=$!

# Wait for memcached listener to be ready
echo "Waiting for binary-protocol listener on port $MEMCACHED_PORT..."
MAX_WAIT=10
for i in $(seq 1 $MAX_WAIT); do
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
        echo "  Listener ready after ${i}s"
        break
    fi
    if ! kill -0 "$REDIS_PID" 2>/dev/null; then
        echo "ERROR: Redis exited prematurely. Log:"
        cat "$REDIS_DIR/redis.log"
        exit 1
    fi
    sleep 1
done

# Verify Redis is responding
if ! kill -0 "$REDIS_PID" 2>/dev/null; then
    echo "ERROR: Redis not running. Log:"
    cat "$REDIS_DIR/redis.log"
    exit 1
fi

echo ""
echo "Verifying Redis connectivity..."
PING_RESULT=$(redis-cli -p "$REDIS_PORT" PING 2>&1)
if echo "$PING_RESULT" | grep -q "PONG"; then
    echo "  ✅ Redis responding"
else
    echo "  ❌ Redis not responding: $PING_RESULT"
    cat "$REDIS_DIR/redis.log"
    exit 1
fi

echo ""

# Create results directory
mkdir -p "$RESULTS_DIR"

# Run benchmarks
EXIT_CODE=0
BENCH_OUTPUT="$RESULT_FILE" \
REDIS_PORT="$REDIS_PORT" \
BENCH_PORT="$MEMCACHED_PORT" \
    python3 "$SCRIPT_DIR/bench_binary_protocol.py" || EXIT_CODE=$?

# Copy to latest.json for easy access
if [ -f "$RESULT_FILE" ]; then
    cp "$RESULT_FILE" "$LATEST_FILE"
    echo ""
    echo "Results also copied to $LATEST_FILE"
fi

echo ""
if [ $EXIT_CODE -eq 0 ]; then
    echo "✅ Benchmark run complete"
else
    echo "❌ Benchmark run failed (exit code: $EXIT_CODE)"
    echo ""
    echo "Redis log tail:"
    tail -20 "$REDIS_DIR/redis.log" 2>/dev/null || true
fi

exit $EXIT_CODE
