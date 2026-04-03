#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────
# Cross-System Benchmark Runner (Symmetric Topology)
#
# Orchestrates a three-way comparison with ALL targets running
# in Docker containers for environment parity:
#   1. Redis + RedCouch (memcached binary protocol via module, Docker)
#   2. Redis OSS native (direct RESP GET/SET/DEL, Docker)
#   3. Couchbase OSS (memcached binary protocol, Docker)
#
# Usage:
#   bash benchmarks/run_cross_system.sh
#
# Prerequisites:
#   - docker compose (all three systems run as containers)
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

MEMCACHED_PORT=11210     # RedCouch listener (Docker)
REDIS_NATIVE_PORT=16380  # Docker Redis OSS
COUCHBASE_PORT=11211     # Docker Couchbase KV
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
RESULTS_DIR="$SCRIPT_DIR/results"
RESULT_FILE="$RESULTS_DIR/cross_system_${TIMESTAMP}.json"
LATEST_FILE="$RESULTS_DIR/cross_system_latest.json"

cleanup() {
    echo ""
    echo "Cleaning up..."
    # Stop all Docker containers
    docker compose -f "$SCRIPT_DIR/docker-compose.yml" down -v 2>/dev/null || true
}
trap cleanup EXIT

echo "═══════════════════════════════════════════════════════════"
echo "Cross-System Benchmark Runner (Symmetric Docker Topology)"
echo "═══════════════════════════════════════════════════════════"
echo ""

# 1. Build and start all Docker containers (Redis OSS + Couchbase + RedCouch)
#    --wait blocks until every service's healthcheck passes, so we don't need
#    separate sleep/polling loops.  The RedCouch healthcheck verifies both
#    Redis RESP and the memcached listener on port 11210.
echo "Building and starting Docker containers (waiting for health checks)..."
docker compose -f "$SCRIPT_DIR/docker-compose.yml" up -d --build --wait --wait-timeout 120
echo "  All containers healthy."

# Belt-and-suspenders: confirm each target is reachable from the host.
echo ""
if redis-cli -p "$REDIS_NATIVE_PORT" PING 2>/dev/null | grep -q "PONG"; then
    echo "  ✅ Redis OSS native ready on port $REDIS_NATIVE_PORT"
else
    echo "ERROR: Redis OSS native not responding on port $REDIS_NATIVE_PORT"
    echo "  Check: docker compose -f benchmarks/docker-compose.yml logs redis-bench"
    exit 1
fi

# 2. Setup Couchbase
echo ""
bash "$SCRIPT_DIR/setup_couchbase.sh" || {
    echo "ERROR: Couchbase setup failed. Cannot run three-system comparison."
    exit 1
}

# 3. Final RedCouch reachability check (should already be healthy from --wait)
echo ""
REDCOUCH_READY=0
for i in $(seq 1 10); do
    if python3 -c "
import socket, sys
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.settimeout(2)
try:
    s.connect(('127.0.0.1', $MEMCACHED_PORT))
    s.close()
    sys.exit(0)
except:
    sys.exit(1)
" 2>/dev/null; then
        echo "  ✅ RedCouch memcached listener ready on port $MEMCACHED_PORT"
        REDCOUCH_READY=1
        break
    fi
    sleep 1
done
if [ "$REDCOUCH_READY" -ne 1 ]; then
    echo "ERROR: RedCouch listener not reachable on port $MEMCACHED_PORT"
    echo "  Container logs:"
    docker compose -f "$SCRIPT_DIR/docker-compose.yml" logs redcouch-bench 2>/dev/null | tail -20
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
