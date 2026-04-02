#!/usr/bin/env bash
# Initialize Couchbase Community Edition for benchmark comparison.
# Creates a single-node cluster with a memcached-type bucket for
# apples-to-apples memcached binary protocol comparison.
#
# Usage: bash benchmarks/setup_couchbase.sh
# Requires: curl, docker with couchbase-bench container running
set -euo pipefail

CB_HOST="${CB_HOST:-127.0.0.1}"
CB_PORT="${CB_PORT:-8091}"
CB_USER="${CB_USER:-admin}"
CB_PASS="${CB_PASS:-password}"
CB_BUCKET="${CB_BUCKET:-bench}"
CB_RAM="${CB_RAM:-256}"

BASE="http://${CB_HOST}:${CB_PORT}"

echo "Waiting for Couchbase REST API..."
for i in $(seq 1 60); do
    if curl -sf "${BASE}/pools" >/dev/null 2>&1; then
        echo "  Couchbase API ready after ${i}s"
        break
    fi
    sleep 1
done

# Check if already initialized
if curl -sf -u "${CB_USER}:${CB_PASS}" "${BASE}/pools/default" >/dev/null 2>&1; then
    echo "  Cluster already initialized"
else
    echo "Initializing cluster..."
    # Set memory quotas
    curl -sf -X POST "${BASE}/pools/default" \
        -d "memoryQuota=${CB_RAM}" || true

    # Set up services (kv only for benchmark)
    curl -sf -X POST "${BASE}/node/controller/setupServices" \
        -d "services=kv" || true

    # Set admin credentials
    curl -sf -X POST "${BASE}/settings/web" \
        -d "port=${CB_PORT}" \
        -d "username=${CB_USER}" \
        -d "password=${CB_PASS}" || true

    echo "  Cluster initialized"
fi

# Check if bucket exists
if curl -sf -u "${CB_USER}:${CB_PASS}" "${BASE}/pools/default/buckets/${CB_BUCKET}" >/dev/null 2>&1; then
    echo "  Bucket '${CB_BUCKET}' already exists"
else
    echo "Creating memcached bucket '${CB_BUCKET}'..."
    curl -sf -X POST -u "${CB_USER}:${CB_PASS}" \
        "${BASE}/pools/default/buckets" \
        -d "name=${CB_BUCKET}" \
        -d "bucketType=memcached" \
        -d "ramQuota=${CB_RAM}" \
        -d "authType=sasl" \
        -d "saslPassword=" || {
        echo "  WARNING: memcached bucket creation failed."
        echo "  Couchbase Community 7.2+ may have deprecated memcached bucket type."
        echo "  Trying couchbase bucket type as fallback..."
        curl -sf -X POST -u "${CB_USER}:${CB_PASS}" \
            "${BASE}/pools/default/buckets" \
            -d "name=${CB_BUCKET}" \
            -d "bucketType=couchbase" \
            -d "ramQuota=${CB_RAM}" || {
            echo "  ERROR: Could not create any bucket. Couchbase benchmark will be skipped."
            exit 1
        }
        echo "  Created couchbase-type bucket (not memcached-type)."
        echo "  NOTE: KV binary protocol should still work but is Couchbase KV, not standard memcached."
    }
    echo "  Waiting for bucket to be ready..."
    sleep 5
fi

echo ""
echo "Couchbase setup complete."
echo "  Console: http://${CB_HOST}:${CB_PORT}"
echo "  KV port: 11211 (mapped from container 11210)"
echo "  Bucket:  ${CB_BUCKET}"
