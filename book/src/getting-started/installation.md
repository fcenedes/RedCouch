# Installation

## Prerequisites

- **Redis 8.x** (Open Source). Verified on Redis 8.4.0.
- **Rust 1.85+** (stable) — only needed if building from source.

## Option 1: Install from GitHub Release

Pre-built artifacts are attached to [GitHub Releases](https://github.com/fcenedes/RedCouch/releases) as `.tar.gz` archives with SHA-256 checksums.

```bash
# Download the release for your platform (example: Linux x86_64, version v0.1.0)
curl -LO https://github.com/fcenedes/RedCouch/releases/download/v0.1.0/redcouch-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
curl -LO https://github.com/fcenedes/RedCouch/releases/download/v0.1.0/redcouch-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256

# Verify checksum
sha256sum -c redcouch-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256

# Extract
tar xzf redcouch-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
```

This extracts `libred_couch.so` (Linux) or `libred_couch.dylib` (macOS).

## Option 2: Build from Source

```bash
git clone https://github.com/fcenedes/RedCouch.git
cd RedCouch
cargo build --release
```

The compiled module is at:
- **macOS**: `target/release/libred_couch.dylib`
- **Linux**: `target/release/libred_couch.so`

## Loading the Module

### Command Line

```bash
redis-server --loadmodule /path/to/libred_couch.so      # Linux
redis-server --loadmodule /path/to/libred_couch.dylib    # macOS
```

### redis.conf

Add to your Redis configuration file:

```
loadmodule /path/to/libred_couch.so
```

### Runtime (MODULE LOAD)

```bash
redis-cli MODULE LOAD /absolute/path/to/libred_couch.so
```

### Verify

After loading, check that the module is active:

```bash
redis-cli MODULE LIST
# Should show "redcouch" in the list

# Verify the memcached endpoint is listening
nc -z 127.0.0.1 11210 && echo "RedCouch listening" || echo "Not listening"
```

## Unloading the Module

```bash
redis-cli MODULE UNLOAD redcouch
```

> **Note:** RedCouch does not implement a module unload/deinit handler. The background TCP listener thread has no graceful shutdown path. Unloading via `MODULE UNLOAD` is **unverified** and may leave the listener thread orphaned. The recommended approach is to restart the Redis process to fully stop the module.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `FATAL: cannot bind 127.0.0.1:11210` | Port already in use | Stop the other process using port 11210 |
| Module loads but port 11210 not reachable | Bind failed silently | Check Redis logs for the bind error message |
| `MODULE LOAD` returns error | Wrong platform artifact | Use `.so` for Linux, `.dylib` for macOS |
| Connection refused after 1024 clients | Connection limit reached | Reduce concurrent connections or wait for existing ones to close |
