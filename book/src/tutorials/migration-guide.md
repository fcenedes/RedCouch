# Tutorial: Migration from Memcached to Redis

RedCouch is designed as a **bridge** — not a permanent proxy. This tutorial walks through the three-phase migration from a memcached-based architecture to native Redis, with RedCouch providing the zero-downtime transition layer.

## The Three Phases

```text
Phase 1: Bridge          Phase 2: Dual-Access      Phase 3: Native
┌──────────┐             ┌──────────┐               ┌──────────┐
│  App     │─memcached──▶│  App     │─memcached──▶  │  App     │
│  (old)   │  protocol   │  (mixed) │  + redis      │  (new)   │─redis──▶ Redis
└──────────┘             └──────────┘               └──────────┘
      │                        │
      ▼                        ▼
  RedCouch ──▶ Redis      RedCouch ──▶ Redis
```

## Phase 1: Bridge — Drop-in Replacement

### Step 1: Deploy RedCouch

Load RedCouch into your Redis 8+ server:

```bash
redis-server --loadmodule /path/to/libred_couch.so
```

RedCouch opens a memcached-compatible endpoint on port 11210.

### Step 2: Repoint Your Clients

Change your memcached client configuration to point at RedCouch instead of your memcached server. The only change needed is the host and port:

**Python (before):**
```python
client = Client(("memcached-host", 11211))
```

**Python (after):**
```python
client = Client(("redis-host", 11210))
```

**Go (before):**
```go
mc := memcache.New("memcached-host:11211")
```

**Go (after):**
```go
mc := memcache.New("redis-host:11210")
```

No other code changes needed. Your application continues using its existing memcached client library.

### Step 3: Verify

```bash
# Check RedCouch is responding
echo "version" | nc redis-host 11210
# VERSION RedCouch 0.1.0

# Check data is flowing through to Redis
redis-cli -h redis-host KEYS 'rc:*'
# Shows your memcached keys with rc: prefix
```

### What You Get in Phase 1

- All memcached operations work transparently
- Data is stored in Redis hashes under the `rc:` key prefix
- You can inspect data via `redis-cli` alongside memcached access
- Redis persistence (RDB/AOF) now protects your cache data
- Redis replication can provide high availability

## Phase 2: Dual-Access — Gradual Migration

In this phase, you migrate application code service-by-service from memcached clients to native Redis clients. Both access paths work simultaneously against the same data.

### Understanding the Storage Model

RedCouch stores each memcached key as a Redis hash:

```bash
redis-cli HGETALL rc:session:abc
# 1) "v"    ← hex-encoded value
# 2) "68656c6c6f"
# 3) "f"    ← flags (32-bit integer)
# 4) "0"
# 5) "c"    ← CAS token
# 6) "42"
```

### Reading Data from Redis

To read RedCouch data natively, decode the hex value from the hash:

```python
import redis

r = redis.Redis(host='redis-host', port=6379)

# Read a value stored by a memcached client
hex_value = r.hget("rc:session:abc", "v")
if hex_value:
    value = bytes.fromhex(hex_value.decode())
    print(value)  # b'hello'
```

### Migration Strategy: Service by Service

1. **Pick a service** to migrate (start with read-heavy, non-critical services)
2. **Add a Redis client** alongside the existing memcached client
3. **Read from Redis** (via `rc:` hashes) while writes still go through memcached
4. **Switch writes to Redis** once reads are verified
5. **Remove the memcached client** from that service
6. **Repeat** for the next service

### Example: Migrating a Session Store

**Before (memcached client):**
```python
from pymemcache.client.base import Client
mc = Client(("redis-host", 11210))

def get_session(session_id):
    return mc.get(f"session:{session_id}")

def set_session(session_id, data, ttl=3600):
    mc.set(f"session:{session_id}", data, expire=ttl)
```

**After (native Redis client):**
```python
import redis
r = redis.Redis(host='redis-host', port=6379)

def get_session(session_id):
    return r.get(f"session:{session_id}")

def set_session(session_id, data, ttl=3600):
    r.setex(f"session:{session_id}", ttl, data)
```

> **Note:** Once you switch to native Redis, you no longer need the `rc:` prefix or hex encoding — you're using Redis directly with full access to all Redis data structures.

## Phase 3: Native — Remove RedCouch

Once all services have migrated to native Redis clients:

1. **Verify** no memcached protocol traffic on port 11210
2. **Unload** the module: restart Redis without the `--loadmodule` argument
3. **Clean up** any remaining `rc:*` keys if desired:

```bash
redis-cli --scan --pattern 'rc:*' | xargs redis-cli DEL
```

### What You Gain

- Full access to all Redis data structures (lists, sets, sorted sets, streams, etc.)
- Native Redis performance without translation overhead
- Redis Cluster support
- Redis pub/sub, Lua scripting, modules
- No memcached protocol parsing overhead

## Timeline Expectations

| Phase | Typical Duration | Risk Level |
|---|---|---|
| Bridge | Hours to days | Low — transparent swap |
| Dual-Access | Weeks to months | Medium — requires code changes |
| Native | Minutes | Low — config change + cleanup |

## Next Steps

- **[Python Client Tutorial](./python-client.md)** — Detailed Python examples
- **[Multi-Language Examples](./multi-language.md)** — Node.js, Go, PHP, CLI
- **[Architecture](../reference/architecture.md)** — How RedCouch translates protocols
- **[Known Limitations](../reference/limitations.md)** — What to watch for during migration
