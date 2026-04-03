# Configuration

All runtime parameters in this release are **compile-time constants** defined in `src/lib.rs`. There are no dynamic configuration options, no config file, and no Redis module arguments. To change a parameter, modify the constant in the source code and rebuild.

This is a deliberate simplification for the initial release. Future versions may introduce `MODULE LOAD` arguments or Redis config directives.

## Runtime Defaults

| Parameter | Value | Constant | Rationale |
|---|---|---|---|
| Bind address | `127.0.0.1:11210` | `DEFAULT_BIND_ADDR` | Loopback-only prevents accidental network exposure |
| Max connections | 1,024 | `MAX_CONNECTIONS` | Caps thread count; each connection uses one OS thread |
| Read timeout | 30 seconds | `SOCKET_READ_TIMEOUT` | Prevents idle connections from consuming threads indefinitely |
| Write timeout | 10 seconds | `SOCKET_WRITE_TIMEOUT` | Detects unresponsive clients |
| Max frame body | 20 MiB | `MAX_BODY_LEN` | Prevents memory exhaustion from oversized requests |
| Max key length | 250 bytes | `MAX_KEY_LEN` | Matches memcached specification limit |
| Max command line (ASCII) | 2,048 bytes | — | Prevents unbounded line reads |
| Key prefix | `rc:` | `KEY_PREFIX` | Namespaces RedCouch data within Redis |
| CAS counter key | `redcouch:sys:cas_counter` | `CAS_COUNTER_KEY` | Global monotonic counter for CAS tokens |

### Changing defaults

To change a parameter, edit the corresponding constant in `src/lib.rs` and rebuild:

```bash
# Example: change bind address to all interfaces
# Edit src/lib.rs: const DEFAULT_BIND_ADDR: &str = "0.0.0.0:11210";
cargo build --release
```

> **Warning:** Binding to `0.0.0.0` exposes the memcached endpoint to the network. RedCouch has no authentication enforcement (SASL is stub-only). Use Redis ACLs, firewall rules, or a reverse proxy if you need network-accessible memcached protocol access.

## Storage Keys

| Key Pattern | Purpose | Example |
|---|---|---|
| `rc:<key>` | User data items (hash with `v`, `f`, `c` fields) | `rc:session:abc` |
| `redcouch:sys:cas_counter` | Monotonic CAS counter | Value: `42` |
| `redcouch:sys:*` | Reserved system namespace | — |

### Inspecting data via redis-cli

RedCouch data is standard Redis data. You can inspect it directly:

```bash
# List all RedCouch keys
redis-cli KEYS 'rc:*'

# Inspect a specific item
redis-cli HGETALL rc:mykey
# Returns: v <hex-value> f <flags> c <cas>

# Check the CAS counter
redis-cli GET redcouch:sys:cas_counter

# Count RedCouch items
redis-cli EVAL "return #redis.call('KEYS', 'rc:*')" 0
```

> **Note:** The value field (`v`) is hex-encoded. To see the actual value, decode it: a value of `48656c6c6f` is the hex encoding of `Hello`.

## Security Considerations

RedCouch's security model relies on **network-level access control**, not application-level authentication:

| Layer | Status | Recommendation |
|---|---|---|
| **Bind address** | Loopback only by default | Safe for single-host deployments. Change only if you have network-level controls. |
| **SASL authentication** | Stub — always succeeds | Do not rely on SASL for access control. Any client that can reach port 11210 can read and write data. |
| **TLS/SSL** | Not supported | Use a TLS-terminating proxy (e.g., `stunnel`) if you need encrypted transport. |
| **Redis ACLs** | Not applicable | RedCouch uses `ThreadSafeContext` to execute commands, bypassing Redis ACLs. |
| **Connection limit** | 1,024 max | Protects against resource exhaustion but is not rate limiting. |

### Production deployment recommendations

1. **Keep the loopback bind** unless your clients run on separate hosts.
2. **Use firewall rules** (iptables, security groups) to restrict access to port 11210 if binding to `0.0.0.0`.
3. **Monitor connection count** — approaching 1,024 connections indicates you may need to scale out or optimize client connection pooling.
4. **Use Redis persistence** (RDB/AOF) if RedCouch data needs to survive restarts. RedCouch data is standard Redis data and is included in Redis persistence snapshots.

## Tuning Guidance

### Connection limits

The 1,024 connection limit is per-module-instance. If your workload exceeds this, consider:

- **Client-side connection pooling** — Most memcached client libraries support connection pools. A pool of 10-50 connections per application instance is typical.
- **Reducing idle connections** — The 30-second read timeout automatically closes idle connections. Clients should reconnect transparently.

### Timeout tuning

| Scenario | Recommendation |
|---|---|
| Low-latency workloads | Default timeouts (30s/10s) are conservative. Most operations complete in <1ms. |
| Long-running bulk loads | Default timeouts are fine — they apply per-read, not per-connection. |
| Unreliable network | Consider increasing read timeout if clients have intermittent connectivity. |

### Memory considerations

RedCouch's memory usage has two components:

1. **Redis data** — Hash-per-item storage with hex-encoded values. Hex encoding doubles the storage cost of values compared to raw bytes. A 1 KB value consumes ~2 KB of Redis memory.
2. **Thread stacks** — Each connection thread uses ~8 MiB of virtual memory (OS default). At 1,024 connections, this is ~8 GiB virtual but typically <100 MiB resident.

Use Redis's `maxmemory` setting to bound data storage. RedCouch respects Redis's eviction policies for `rc:*` keys.
