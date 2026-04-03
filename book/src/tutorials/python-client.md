# Tutorial: Python Client

This tutorial walks through using RedCouch from Python, starting with simple key-value operations and building up to CAS workflows, counters, and pipelining. All examples use the standard `pymemcache` library.

## Prerequisites

- Redis 8+ running with RedCouch loaded (see [Installation](../getting-started/installation.md))
- Python 3.10+
- Install pymemcache: `pip install pymemcache`

> **Note:** RedCouch listens on port **11210**, not the default memcached port (11211).

## Step 1: Connect and Store a Value

```python
from pymemcache.client.base import Client

# Connect to RedCouch (port 11210, not 11211)
client = Client(("127.0.0.1", 11210))

# Store a value
client.set("greeting", "Hello from Python!")

# Retrieve it
value = client.get("greeting")
print(value)  # b'Hello from Python!'
```

The `get()` method returns `bytes` by default. If you want strings, use a deserializer:

```python
client = Client(("127.0.0.1", 11210), default_noreply=False)
value = client.get("greeting")
print(value.decode("utf-8"))  # 'Hello from Python!'
```

## Step 2: Flags and Expiration

Memcached flags are a 32-bit integer stored alongside the value. They're commonly used to indicate serialization format. Expiration is in seconds (up to 30 days) or a Unix timestamp (for longer durations).

```python
import json

# Store JSON data with a flags marker and 60-second TTL
data = {"user": "alice", "role": "admin"}
client.set("session:abc", json.dumps(data).encode(), expire=60, flags=1)

# Retrieve with flags
result = client.get("session:abc", return_flags=True)
# result is (b'{"user": "alice", "role": "admin"}', 1)
value, flags = result
if flags == 1:
    parsed = json.loads(value)
    print(parsed["user"])  # 'alice'
```

## Step 3: Add and Replace (Conditional Stores)

```python
# add() only succeeds if the key does NOT exist
client.add("new-key", "first-write")   # True — key created
client.add("new-key", "second-write")  # False — key already exists

# replace() only succeeds if the key DOES exist
client.replace("new-key", "updated")   # True
client.replace("missing", "value")     # False — key not found
```

## Step 4: Compare-and-Swap (CAS)

CAS prevents lost updates when multiple clients write to the same key. The workflow is: read the current CAS token, then write only if the token hasn't changed.

```python
# gets() returns (value, cas_token)
value, cas = client.gets("greeting")
print(f"Value: {value}, CAS: {cas}")

# cas() writes only if the CAS token matches
success = client.cas("greeting", "Updated value", cas)
print(f"CAS update succeeded: {success}")  # True

# A second CAS with the old token fails
success = client.cas("greeting", "Stale update", cas)
print(f"Stale CAS update: {success}")  # False — token changed
```

## Step 5: Counters

```python
# Initialize a counter
client.set("page-views", "0")

# Increment
new_val = client.incr("page-views", 1)
print(f"Views: {new_val}")  # 1

# Increment by 10
new_val = client.incr("page-views", 10)
print(f"Views: {new_val}")  # 11

# Decrement
new_val = client.decr("page-views", 3)
print(f"Views: {new_val}")  # 8
```

> **Note:** ASCII protocol `incr`/`decr` return `NOT_FOUND` for missing keys. Always initialize counters with `set` first.

## Step 6: Append and Prepend

```python
client.set("log", "entry-1")
client.append("log", ",entry-2")
client.append("log", ",entry-3")

print(client.get("log"))  # b'entry-1,entry-2,entry-3'

client.prepend("log", "header:")
print(client.get("log"))  # b'header:entry-1,entry-2,entry-3'
```

## Step 7: Multi-Get

```python
# Store several keys
for i in range(5):
    client.set(f"item:{i}", f"value-{i}")

# Fetch multiple keys in one round trip
results = client.get_many([f"item:{i}" for i in range(5)])
for key, value in results.items():
    print(f"{key}: {value}")
```

## Step 8: Touch and Get-and-Touch

```python
# Extend TTL without fetching the value
client.touch("session:abc", 300)  # Reset to 5 minutes

# Get value AND reset TTL in one operation
value = client.gat("session:abc", 600)  # Get + set TTL to 10 minutes
```

## Step 9: Delete and Flush

```python
# Delete a single key
client.delete("greeting")

# Flush all RedCouch keys (only rc:* keys, not entire Redis DB)
client.flush_all()
```

## Step 10: Verify Data in Redis

Because RedCouch stores data in Redis hashes under the `rc:` prefix, you can inspect the data directly:

```bash
# From redis-cli (on the Redis port, not 11210)
redis-cli

# List RedCouch keys
KEYS rc:*

# Inspect a specific item
HGETALL rc:greeting
# Returns: v (hex-encoded value), f (flags), c (CAS token)
```

This dual-access capability is the foundation of RedCouch's migration story — see the [Migration Guide](./migration-guide.md).

## Runnable Example

A complete, runnable version of this tutorial is available at [`examples/python/basic_operations.py`](https://github.com/fcenedes/RedCouch/blob/main/examples/python/basic_operations.py).

## Next Steps

- **[Multi-Language Examples](./multi-language.md)** — Node.js, Go, and CLI examples
- **[Migration Guide](./migration-guide.md)** — Step-by-step migration from memcached to Redis
- **[Use Cases](./use-cases.md)** — Real-world scenarios and patterns
- **[Binary Protocol](../guide/binary-protocol.md)** — Machine-oriented protocol for SDK clients
