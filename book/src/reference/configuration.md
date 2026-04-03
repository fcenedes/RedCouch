# Configuration

All runtime parameters are compile-time constants. There are no dynamic configuration options in this release.

## Runtime Defaults

| Parameter | Value | Constant |
|---|---|---|
| Bind address | `127.0.0.1:11210` | `DEFAULT_BIND_ADDR` |
| Max connections | 1,024 | `MAX_CONNECTIONS` |
| Read timeout | 30 seconds | `SOCKET_READ_TIMEOUT` |
| Write timeout | 10 seconds | `SOCKET_WRITE_TIMEOUT` |
| Max frame body | 20 MiB | `MAX_BODY_LEN` |
| Max key length | 250 bytes | `MAX_KEY_LEN` |
| Max command line (ASCII) | 2,048 bytes | — |
| Key prefix | `rc:` | `KEY_PREFIX` |
| CAS counter key | `redcouch:sys:cas_counter` | `CAS_COUNTER_KEY` |

## Storage Keys

| Key Pattern | Purpose |
|---|---|
| `rc:<key>` | User data items (hash with fields `v`, `f`, `c`) |
| `redcouch:sys:cas_counter` | Monotonic CAS counter |
| `redcouch:sys:*` | Reserved system namespace |

## Security Defaults

- **Bind address**: Loopback only (`127.0.0.1`) by default — not exposed to the network.
- **SASL auth**: Stub only. Auth handshake succeeds for all credentials. No credential enforcement.
- **Connection limit**: 1,024 concurrent connections. Beyond this, new connections are immediately dropped.
- **Timeouts**: Read timeout 30s, write timeout 10s per connection.
- **Frame size cap**: 20 MiB maximum body per binary protocol frame.
