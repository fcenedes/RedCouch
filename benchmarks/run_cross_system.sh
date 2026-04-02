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
#   SKIP_COUCHBASE  - set to "1" to skip Couchbase
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
    echo "  ⚠ Redis OSS native not responding on port $REDIS_NATIVE_PORT"
fi

# 2. Setup Couchbase (if not skipped)
if [ "${SKIP_COUCHBASE:-0}" != "1" ]; then
    echo ""
    bash "$SCRIPT_DIR/setup_couchbase.sh" || {
        echo "  ⚠ Couchbase setup failed — will skip Couchbase benchmarks"
        export SKIP_COUCHBASE=1
    }
fi

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
for i in $(seq 1 10); do
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
        break
    fi
    sleep 1
done

# 4. Run cross-system benchmarks
echo ""
mkdir -p "$RESULTS_DIR"

BENCH_OUTPUT="$RESULT_FILE" \
REDCOUCH_HOST="127.0.0.1" \
REDCOUCH_PORT="$MEMCACHED_PORT" \
REDIS_NATIVE_HOST="127.0.0.1" \
REDIS_NATIVE_PORT="$REDIS_NATIVE_PORT" \
COUCHBASE_HOST="127.0.0.1" \
COUCHBASE_PORT="$COUCHBASE_PORT" \
SKIP_COUCHBASE="${SKIP_COUCHBASE:-0}" \
    python3 "$SCRIPT_DIR/bench_cross_system.py" || true

# Symlink latest
if [ -f "$RESULT_FILE" ]; then
    ln -sf "$(basename "$RESULT_FILE")" "$LATEST_FILE.tmp"
    mv -f "$LATEST_FILE.tmp" "$LATEST_FILE"
    echo ""
    echo "Results: $RESULT_FILE"
    echo "Symlink: cross_system_latest.json → $(basename "$RESULT_FILE")"
fi

echo ""
echo "✅ Cross-system benchmark complete"
