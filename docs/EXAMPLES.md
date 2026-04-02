# Examples and Tutorials

This guide demonstrates common usage patterns with RedCouch using standard memcached clients.

## Quick Start

### 1. Build and Load

```bash
# Build the module
cargo build --release

# Start Redis with the module
redis-server --loadmodule ./target/release/libred_couch.dylib   # macOS
redis-server --loadmodule ./target/release/libred_couch.so      # Linux
```

The module starts a TCP listener on `127.0.0.1:11210`.

### 2. Connect with a Memcached Client

Any memcached-compatible client can connect on port 11210. RedCouch automatically detects whether the client speaks binary or ASCII protocol.

---

## ASCII Protocol Examples

Connect using `telnet` or `nc` (netcat):

```bash
telnet 127.0.0.1 11210
```

### Basic Key-Value Operations

```
# Store a value
set mykey 0 0 5
hello
STORED

# Retrieve it
get mykey
VALUE mykey 0 5
hello
END

# Store with flags and TTL (60 seconds)
set session:abc 42 60 11
session_data
STORED

# Retrieve with CAS token
gets mykey
VALUE mykey 0 5 1
hello
END

# Compare-and-swap (CAS) update
cas mykey 0 0 5 1
world
STORED

# Delete
delete mykey
DELETED
```

### Counters

```
# Create a counter via set (store numeric string)
set counter 0 0 1
0
STORED

# Increment
incr counter 1
1

# Increment by 10
incr counter 10
11

# Decrement
decr counter 3
8
```

Note: In ASCII protocol, `incr`/`decr` return `NOT_FOUND` for missing keys. Use `set` to initialize counters.

### Append and Prepend

```
set log 0 0 6
line-1
STORED

append log 7
,line-2
STORED

get log
VALUE log 0 13
line-1,line-2
END

prepend log 8
header: 
STORED
```

### Touch and Get-and-Touch

```
# Update TTL without fetching value
touch mykey 120
TOUCHED

# Get value and update TTL simultaneously
gat 300 mykey
VALUE mykey 0 5
world
END
```

### Stats and Version

```
version
VERSION RedCouch 0.1.0

stats
STAT pid 12345
STAT uptime 42
STAT version RedCouch 0.1.0
STAT cmd_get 5
STAT cmd_set 3
STAT curr_items 2
...
END
```

### Flush

```
# Flush all RedCouch items (only rc:* keys, not entire Redis DB)
flush_all
OK
```

---

## Meta Protocol Examples

Meta commands use two-letter prefixes and flags. Connect via telnet:

```bash
telnet 127.0.0.1 11210
```

### Meta Set and Get

```
# Set a value (ms = meta set, 5 = data length)
ms mykey 5
hello
HD

# Get with value and CAS (mg = meta get, v = value, c = CAS)
mg mykey v c
VA 5 c1
hello

# Get with key echo, flags, size
mg mykey k f s
HD kbXlrZXk= f0 s5
```

### Meta Set Modes

```
# Add (only if not exists): M flag with E mode
ms newkey 3 ME
foo
HD

# Replace (only if exists): M flag with R mode
ms mykey 3 MR
bar
HD

# Append: M flag with A mode
ms mykey 4 MA
_end
HD

# Prepend: M flag with P mode
ms mykey 6 MP
start_
HD
```

### Meta Delete

```
md mykey
HD
```

### Meta Arithmetic

```
# Create counter with initial value (J = initial, N = TTL for auto-create)
ma counter J0 N0
HD

# Increment by 5 (D = delta, v = return value)
ma counter D5 v
VA 1
5

# Decrement (MI = decrement mode not available — use MD mode)
ma counter MD D2 v
VA 1
3
```

### Meta Noop (Pipeline Terminator)

```
mn
MN
```

### Opaque Token (Request Correlation)

```
# O flag echoes an opaque token in the response
mg mykey v Oreq-42
VA 5 Oreq-42
hello

mn Oping
MN Oping
```

---

## Binary Protocol

Binary protocol clients connect to the same port (11210). The protocol is auto-detected from the first byte (`0x80` = binary request magic).

Use any memcached binary-protocol client library. For example, with Python's `pymemcache`:

```python
from pymemcache.client.base import Client

client = Client(('127.0.0.1', 11210))

# Basic operations
client.set('key1', b'value1')
result = client.get('key1')  # b'value1'

# With flags
client.set('key2', b'data', flags=0x1234)

# Counters (binary protocol supports auto-create with initial value)
client.incr('counter', 1)

# Delete
client.delete('key1')

client.close()
```
