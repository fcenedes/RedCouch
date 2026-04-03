# Use Cases

This chapter describes real-world scenarios where RedCouch solves a concrete problem. Each case explains the setup, why RedCouch fits, and links to relevant reference material.

## Session Store Migration

**Scenario:** Your web application stores sessions in memcached. You want to move to Redis for persistence and replication, but can't change all services at once.

**Solution:** Deploy RedCouch on Redis 8+. Repoint your memcached clients to port 11210. Sessions are now stored in Redis hashes — surviving restarts and replicating to replicas — while your application code stays unchanged.

**Key operations used:** `set` (store session), `get` (retrieve session), `touch` (extend TTL), `delete` (logout)

**Why RedCouch fits:**
- Zero application code changes during Phase 1
- Redis persistence (RDB/AOF) eliminates cold-cache risk after restarts
- Gradual migration to native Redis clients is possible per-service

**See:** [Migration Guide](./migration-guide.md) for the step-by-step process.

---

## Couchbase-to-Redis Migration

**Scenario:** You're migrating from Couchbase Server to Redis. Your applications use Couchbase SDKs that speak the memcached binary protocol for key-value operations.

**Solution:** RedCouch implements the Couchbase memcached binary protocol (all 34 opcodes, including SASL handshake). Point your Couchbase SDKs at RedCouch on port 11210. The SASL stub accepts any credentials, so no auth configuration changes are needed.

**Key operations used:** Binary GET/SET/DELETE with CAS, SASL auth handshake, quiet variants for pipelining

**Why RedCouch fits:**
- Full binary protocol compatibility with Couchbase SDK wire format
- SASL authentication stub lets SDKs connect without credential changes
- CAS tokens are real and atomically enforced via Redis Lua scripts

**Caveat:** Couchbase-specific features (buckets, vbuckets, views, N1QL) are not supported — only key-value operations. See [Known Limitations](../reference/limitations.md#deferred-surfaces).

---

## Rate Limiter with Dual Access

**Scenario:** You have a rate limiter using memcached counters (`incr`/`decr`). You want to add Redis-based analytics that reads the same counter data in real time.

**Solution:** Use RedCouch as the bridge. Your rate limiter writes through memcached protocol (`incr`/`decr` on port 11210). Your analytics service reads the same counters via native Redis commands on port 6379.

```python
# Rate limiter (memcached client, unchanged)
from pymemcache.client.base import Client
mc = Client(("redis-host", 11210))

def check_rate(client_ip, limit=100):
    key = f"rate:{client_ip}"
    try:
        count = mc.incr(key, 1)
    except Exception:
        mc.set(key, "1", expire=60)
        count = 1
    return int(count) <= limit
```

```python
# Analytics service (native Redis, new)
import redis
r = redis.Redis(host='redis-host', port=6379)

def get_rate_counts():
    keys = r.keys("rc:rate:*")
    counts = {}
    for key in keys:
        hex_val = r.hget(key, "v")
        if hex_val:
            counts[key.decode()] = bytes.fromhex(hex_val.decode()).decode()
    return counts
```

**Why RedCouch fits:**
- Counters work correctly through the memcached protocol
- Same data is readable via native Redis for analytics
- No changes needed to the rate limiter code

---

## Cache Warm-Up Testing

**Scenario:** You want to validate that your application handles cache misses correctly after a restart, but your memcached setup doesn't support scripted warm-up.

**Solution:** Use RedCouch with Redis persistence. Load test data via `redis-cli` or a script, then verify your application reads it correctly through the memcached protocol.

```bash
# Pre-load test data directly into Redis
redis-cli HSET rc:config:feature-flags v "$(echo -n '{"dark_mode":true}' | xxd -p)" f 0 c 1

# Verify through memcached protocol
echo -e "get config:feature-flags\r" | nc 127.0.0.1 11210
```

**Why RedCouch fits:**
- Redis persistence means cache data survives restarts
- Data can be loaded via Redis commands or memcached protocol
- Useful for integration testing and staging environments

---

## Protocol Debugging and Monitoring

**Scenario:** You need to debug what your memcached clients are actually sending and receiving, or monitor cache hit rates.

**Solution:** RedCouch exposes statistics through the memcached `stats` command. You can also inspect the underlying Redis data directly.

```bash
# Check stats via ASCII protocol
echo "stats" | nc 127.0.0.1 11210
# STAT cmd_get 1523
# STAT cmd_set 847
# STAT curr_items 312

# Inspect specific keys via redis-cli
redis-cli HGETALL rc:problematic-key

# Count total cached items
redis-cli --scan --pattern 'rc:*' | wc -l
```

**Why RedCouch fits:**
- Memcached stats are available through standard protocol
- Redis gives you additional visibility (key inspection, memory usage, slow log)
- Dual access makes debugging transparent

---

## Summary: When to Use RedCouch

| Scenario | Fit | Key Benefit |
|---|---|---|
| Migrating from memcached to Redis | ✅ Ideal | Zero-downtime, gradual migration |
| Migrating from Couchbase KV to Redis | ✅ Ideal | Binary protocol + SASL compatibility |
| Adding persistence to a memcached cache | ✅ Good | Redis RDB/AOF protects cache data |
| Dual-access (memcached + Redis) during migration | ✅ Good | Both protocols hit the same data |
| Long-term production memcached proxy | ⚠️ Acceptable | Works, but native Redis is faster |
| High-throughput, latency-critical cache | ❌ Not ideal | Translation overhead adds ~18% latency |

## Next Steps

- **[Python Client Tutorial](./python-client.md)** — Hands-on Python walkthrough
- **[Multi-Language Examples](./multi-language.md)** — Node.js, Go, PHP, CLI
- **[Benchmarks & Performance](../operations/benchmarks.md)** — Measured throughput data
- **[Architecture](../reference/architecture.md)** — How the translation layer works
