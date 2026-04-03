# Quick Start

This page walks you through building RedCouch, loading it into Redis, and running your first commands.

## 1. Build and Load

```bash
# Build the module
cargo build --release

# Start Redis with the module
redis-server --loadmodule ./target/release/libred_couch.dylib   # macOS
redis-server --loadmodule ./target/release/libred_couch.so      # Linux
```

The module starts a TCP listener on `127.0.0.1:11210`.

## 2. Connect with a Client

Any memcached-compatible client can connect on port 11210. RedCouch automatically detects whether the client speaks binary or ASCII protocol.

### Using telnet (ASCII protocol)

```bash
telnet 127.0.0.1 11210
```

### Store and retrieve a value

```
set mykey 0 0 5
hello
STORED

get mykey
VALUE mykey 0 5
hello
END
```

### Delete a value

```
delete mykey
DELETED
```

### Check the version

```
version
VERSION RedCouch 0.1.0
```

## 3. Run the Tests

```bash
# Unit and protocol tests (221 tests — binary, ASCII, meta)
cargo test

# Integration tests (requires Redis 8+ with module loaded)
cd tests/integration && bash run_e2e.sh
```

## Next Steps

- [ASCII Protocol Examples](../guide/ascii-protocol.md) — full ASCII command walkthrough
- [Meta Protocol Examples](../guide/meta-protocol.md) — flag-based meta commands
- [Binary Protocol Examples](../guide/binary-protocol.md) — raw binary protocol usage
- [Protocol Compatibility Reference](../reference/protocol-compatibility.md) — complete command tables
