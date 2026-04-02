# Installation and Release Guide

## Platform Support

| Target | OS | Architecture | Artifact |
|---|---|---|---|
| `x86_64-unknown-linux-gnu` | Linux | x86_64 | `libred_couch.so` |
| `aarch64-unknown-linux-gnu` | Linux | ARM64 | `libred_couch.so` |
| `x86_64-apple-darwin` | macOS | x86_64 | `libred_couch.dylib` |
| `aarch64-apple-darwin` | macOS | ARM64 | `libred_couch.dylib` |

**Windows is not supported.** Redis modules require a Unix-like environment; RedCouch does not build, test, or publish Windows artifacts.

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

## Runtime Configuration

All runtime parameters are compile-time constants. There are no dynamic configuration options in this release.

| Parameter | Value | Description |
|---|---|---|
| Bind address | `127.0.0.1:11210` | Loopback only (safe default) |
| Max connections | 1,024 | Connections beyond this are dropped |
| Read timeout | 30 seconds | Per-connection socket read timeout |
| Write timeout | 10 seconds | Per-connection socket write timeout |
| Max frame body | 20 MiB | Maximum body size per binary protocol frame |
| Max key length | 250 bytes | Maximum memcached key length |

## crates.io

Source publication to crates.io is **policy-gated**. The `Cargo.toml` metadata is configured, but live publication is disabled until maintainers explicitly confirm the MIT license for public distribution. The release workflow includes a gated `publish-crate` job controlled by the `PUBLISH_CRATE` repository variable.

## Release Process

Releases are triggered by pushing a Git tag matching `v*`:

```bash
git tag v0.1.0
git push origin v0.1.0
```

This triggers the release workflow (`.github/workflows/release.yml`) which:

1. Builds release artifacts for all 4 supported targets
2. Runs tests and clippy checks
3. Creates a GitHub Release with auto-generated release notes
4. Attaches `.tar.gz` archives and SHA-256 checksums
5. Optionally publishes to crates.io (if `PUBLISH_CRATE` is set to `true`)

## Unloading the Module

```bash
redis-cli MODULE UNLOAD redcouch
```

**Note:** RedCouch does not implement a module unload/deinit handler. The background TCP listener thread has no graceful shutdown path. Unloading via `MODULE UNLOAD` is **unverified** and may leave the listener thread orphaned. The recommended approach is to restart the Redis process to fully stop the module.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `FATAL: cannot bind 127.0.0.1:11210` | Port already in use | Stop the other process using port 11210 |
| Module loads but port 11210 not reachable | Bind failed silently | Check Redis logs for the bind error message |
| `MODULE LOAD` returns error | Wrong platform artifact | Use `.so` for Linux, `.dylib` for macOS |
| Connection refused after 1024 clients | Connection limit reached | Reduce concurrent connections or wait for existing ones to close |
