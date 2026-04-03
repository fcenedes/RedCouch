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

The `get()` method returns `bytes` by default. To decode to a string, call `.decode()`:

```python
value = client.get("greeting")
print(value.decode("utf-8"))  # 'Hello from Python!'
```

## Step 2: Flags and Expiration with a Serde

Memcached flags are a 32-bit integer stored alongside the value. They're commonly used to indicate serialization format. In pymemcache, flags are managed by a **serde** (serializer/deserializer) object that you pass to the `Client` constructor.

Expiration is in seconds (up to 30 days) or a Unix timestamp (for longer durations).

```python
import json

class JSONSerde:
    """Serialize non-string values as JSON, using flags to track the format."""
    def serialize(self, key, value):
        if isinstance(value, str):
            return value.encode("utf-8"), 0  # flag 0 = raw string
        return json.dumps(value).encode("utf-8"), 1  # flag 1 = JSON

    def deserialize(self, key, value, flags):
        if flags == 0:
            return value.decode("utf-8")
        if flags == 1:
            return json.loads(value)
        return value  # fallback: return raw bytes

# Create a client with the JSON serde
client = Client(("127.0.0.1", 11210), serde=JSONSerde())

# Store a Python dict — the serde serializes it as JSON with flag=1
data = {"user": "alice", "role": "admin"}
client.set("session:abc", data, expire=60)

# Retrieve — the serde deserializes based on the stored flag
result = client.get("session:abc")
print(result["user"])  # 'alice'

# Store a plain string — the serde uses flag=0
client.set("greeting", "hello")
print(client.get("greeting"))  # 'hello' (str, not bytes)
```

You can also pass `flags` directly to `set()` to override the serde's flag value, but this is only needed for advanced use cases.

> **Note:** The remaining steps use a plain client without a serde (as created in Step 1), so `get()` returns raw `bytes`. If you're using the serde client from this step, the returned values will be deserialized strings/objects instead.

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

## Step 9: Verify Data in Redis

Because RedCouch stores data in Redis hashes under the `rc:` prefix, you can inspect the data directly while it's still live:

```bash
# From redis-cli (on the Redis port, not 11210)
redis-cli

# List RedCouch keys created by the steps above
KEYS rc:*
# rc:greeting, rc:session:abc, rc:log, rc:item:0, ...

# Inspect a specific item's internal structure
HGETALL rc:greeting
# 1) "v"    ← hex-encoded value
# 2) "48656c6c6f2066726f6d20507974686f6e21"
# 3) "f"    ← flags (32-bit integer)
# 4) "0"
# 5) "c"    ← CAS token
# 6) "2"
```

This dual-access capability is the foundation of RedCouch's migration story — see the [Migration Guide](./migration-guide.md).

## Step 10: Delete and Flush

Once you've finished inspecting the data, clean up:

```python
# Delete a single key
client.delete("greeting")

# Flush all RedCouch keys (only rc:* keys, not entire Redis DB)
client.flush_all()
```

## Runnable Example

A complete, runnable version of this tutorial is available at [`examples/python/basic_operations.py`](https://github.com/fcenedes/RedCouch/blob/main/examples/python/basic_operations.py).

## Next Steps

- **[Multi-Language Examples](./multi-language.md)** — Node.js, Go, and CLI examples
- **[Migration Guide](./migration-guide.md)** — Step-by-step migration from memcached to Redis
- **[Use Cases](./use-cases.md)** — Real-world scenarios and patterns
- **[Binary Protocol](../guide/binary-protocol.md)** — Machine-oriented protocol for SDK clients
