#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────
# RedCouch E2E integration test runner
#
# Usage:  ./tests/integration/run_e2e.sh
#
# Requirements:
#   - redis-server 8.0+ in PATH
#   - python3 in PATH
#   - Module already built: cargo build --release
#
# The script:
#   1. Starts a private Redis instance with the module loaded
#   2. Waits for the binary-protocol listener on port 11210
#   3. Runs the Python test suite
#   4. Tears down the Redis instance
# ──────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

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
REDIS_PORT=16379
MEMCACHED_PORT=11210
REDIS_DIR=$(mktemp -d)
REDIS_PID=""

cleanup() {
    if [ -n "$REDIS_PID" ] && kill -0 "$REDIS_PID" 2>/dev/null; then
        echo "Stopping Redis (PID $REDIS_PID)..."
        kill "$REDIS_PID" 2>/dev/null || true
        wait "$REDIS_PID" 2>/dev/null || true
    fi
    rm -rf "$REDIS_DIR"
}
trap cleanup EXIT

echo "═══════════════════════════════════════════════════════════"
echo "RedCouch E2E Integration Tests"
echo "═══════════════════════════════════════════════════════════"
echo "  Redis server:  $(redis-server --version 2>/dev/null | head -1)"
echo "  Module:        $MODULE"
echo "  Redis port:    $REDIS_PORT"
echo "  Memcached port: $MEMCACHED_PORT"
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

# ── Preflight: verify Redis is responding ────────────────────────
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
echo "Running tests (hash-per-item data model, no JSON dependency)..."
echo ""

# Run the Python test suite, passing Redis port for optional direct checks
EXIT_CODE=0
REDIS_PORT=$REDIS_PORT python3 "$SCRIPT_DIR/test_binary_protocol.py" || EXIT_CODE=$?

echo ""
if [ $EXIT_CODE -eq 0 ]; then
    echo "✅ All tests passed"
else
    echo "❌ Some tests failed (exit code: $EXIT_CODE)"
    echo ""
    echo "Redis log tail:"
    tail -20 "$REDIS_DIR/redis.log" 2>/dev/null || true
fi

exit $EXIT_CODE
