# Quick Start

This page walks you through building RedCouch, loading it into Redis, and running your first commands — in about five minutes.

## Prerequisites

- **Redis 8.x** installed and available as `redis-server` (verified on 8.4.0)
- **Rust 1.85+** toolchain (for building from source)

If you don't have Redis 8, see [Installation](./installation.md) for download options.

## 1. Build the Module

```bash
git clone https://github.com/fcenedes/RedCouch.git
cd RedCouch
cargo build --release
```

This produces the module library:
- **macOS**: `target/release/libred_couch.dylib`
- **Linux**: `target/release/libred_couch.so`

## 2. Start Redis with RedCouch

```bash
# macOS
redis-server --loadmodule ./target/release/libred_couch.dylib

# Linux
redis-server --loadmodule ./target/release/libred_couch.so
```

You should see Redis start with a log line indicating the module loaded. RedCouch opens a TCP listener on `127.0.0.1:11210`.

**Verify it's running:**

```bash
# Check the module is loaded
redis-cli MODULE LIST
# Should show "redcouch"

# Check the memcached endpoint is listening
nc -z 127.0.0.1 11210 && echo "RedCouch is ready"
```

## 3. Your First Commands

Connect using telnet (ASCII protocol):

```bash
telnet 127.0.0.1 11210
```

### Store and retrieve a value

```
set greeting 0 0 12
Hello World!
STORED

get greeting
VALUE greeting 0 12
Hello World!
END
```

The `set` command syntax is: `set <key> <flags> <exptime> <bytes>`, followed by the value data on the next line. Here we're setting key `greeting` with flags=0, no expiry (0), and 12 bytes of data.

### Use CAS for safe updates

```
gets greeting
VALUE greeting 0 12 1
Hello World!
END

cas greeting 0 0 8 1
Hey you!
STORED
```

The `gets` command returns the CAS token (the `1` after the byte count). The `cas` command uses this token to ensure no one else modified the value between the read and write.

### Counters

```
set visits 0 0 1
0
STORED

incr visits 1
1

incr visits 1
2
```

### Delete and verify

```
delete greeting
DELETED

get greeting
END
```

### Check version and stats

```
version
VERSION RedCouch 0.1.0

stats
STAT pid 12345
STAT uptime 42
STAT version RedCouch 0.1.0
...
END

quit
```

## 4. Inspect Data via Redis

While RedCouch is running, open another terminal and use `redis-cli` to see the data:

```bash
# See all RedCouch keys
redis-cli KEYS 'rc:*'

# Inspect a specific item
redis-cli HGETALL rc:visits
# Returns: v (hex-encoded value), f (flags), c (CAS token)
```

This dual-access is one of RedCouch's key features: data is simultaneously accessible through the memcached protocol and native Redis commands.

## 5. Run the Tests

```bash
# Unit and protocol tests (221 tests — no Redis required)
cargo test

# Integration tests (requires Redis 8+ with module loaded)
cd tests/integration && bash run_e2e.sh
```

## Next Steps

- **[Python Client Tutorial](../tutorials/python-client.md)** — Step-by-step Python walkthrough with pymemcache
- **[Multi-Language Examples](../tutorials/multi-language.md)** — Node.js, Go, PHP, and CLI examples
- **[Migration Guide](../tutorials/migration-guide.md)** — Three-phase migration from memcached to native Redis
- **[ASCII Protocol](../guide/ascii-protocol.md)** — Full walkthrough of all 19 ASCII commands
- **[Meta Protocol](../guide/meta-protocol.md)** — Flag-based meta commands with fine-grained control
- **[Binary Protocol](../guide/binary-protocol.md)** — Machine-oriented protocol for SDK clients
- **[Architecture](../reference/architecture.md)** — How RedCouch works under the hood
- **[Configuration](../reference/configuration.md)** — Runtime defaults and tuning guidance
