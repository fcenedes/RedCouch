#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────
# RedCouch Stress / Soak Validation Runner
#
# Usage:
#   ./benchmarks/run_stress_soak.sh
#   STRESS_DURATION=30 SOAK_DURATION=300 ./benchmarks/run_stress_soak.sh
#
# Requirements:
#   - redis-server 8.0+ in PATH
#   - python3 in PATH
#   - Module built: cargo build --release
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

# Use non-default ports to avoid conflicts
REDIS_PORT="${REDIS_PORT:-16379}"
MEMCACHED_PORT="${BENCH_PORT:-11210}"
REDIS_DIR=$(mktemp -d)
REDIS_PID=""
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
RESULTS_DIR="$SCRIPT_DIR/results"
RESULT_FILE="$RESULTS_DIR/stress_${TIMESTAMP}.json"

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
echo "RedCouch Stress / Soak Validation Runner"
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

# Wait for memcached listener
echo "Waiting for protocol listener on port $MEMCACHED_PORT..."
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

mkdir -p "$RESULTS_DIR"

# Run stress/soak validation
EXIT_CODE=0
STRESS_OUTPUT="$RESULT_FILE" \
REDIS_PORT="$REDIS_PORT" \
BENCH_PORT="$MEMCACHED_PORT" \
    python3 "$SCRIPT_DIR/stress_soak_validation.py" || EXIT_CODE=$?

# Symlink latest
if [ -f "$RESULT_FILE" ]; then
    ln -sf "$(basename "$RESULT_FILE")" "$RESULTS_DIR/stress_latest.json"
    echo "  Symlinked → stress_latest.json"
fi

echo ""
echo "Redis log tail:"
tail -5 "$REDIS_DIR/redis.log" 2>/dev/null || true

exit $EXIT_CODE
