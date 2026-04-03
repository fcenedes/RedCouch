# Protocol Compatibility Reference

This is the definitive reference for RedCouch's protocol support. For tutorial-style examples, see the [User Guide](../guide/ascii-protocol.md).

RedCouch implements three memcached protocol surfaces over a single TCP listener (port 11210). Protocol detection is automatic: the first byte of each connection determines the protocol.

| First Byte | Protocol |
|---|---|
| `0x80` | Binary (Couchbase memcached) |
| Printable ASCII | Text (ASCII or meta commands) |
| `\r` or `\n` | Skipped; next byte determines protocol |

---

## Binary Protocol

Based on the [Couchbase memcached binary protocol](https://github.com/couchbase/memcached/blob/master/docs/BinaryProtocol.md). All 34 opcodes (0x00–0x22, excluding 0x1F) are parsed and dispatched.

### Supported Operations

| Opcode Family | Opcodes | Status | Notes |
|---|---|---|---|
| GET | `GET` (0x00), `GETQ` (0x09), `GETK` (0x0C), `GETKQ` (0x0D) | ✅ Supported | Returns value, flags, CAS. Quiet variants suppress success responses. Key-inclusive variants echo key. |
| SET/ADD/REPLACE | `SET` (0x01), `SETQ` (0x11), `ADD` (0x02), `ADDQ` (0x12), `REPLACE` (0x03), `REPLACEQ` (0x13) | ✅ Supported | CAS-checked mutations. Flags, expiry, and binary-safe values preserved. |
| DELETE | `DELETE` (0x04), `DELETEQ` (0x14) | ✅ Supported | CAS-checked. Returns item CAS on success. |
| INCREMENT/DECREMENT | `INCR` (0x05), `INCRQ` (0x15), `DECR` (0x06), `DECRQ` (0x16) | ✅ Supported | Unsigned 64-bit semantics with initial-value and miss rules. See [Limitations](./limitations.md). |
| APPEND/PREPEND | `APPEND` (0x0E), `APPENDQ` (0x19), `PREPEND` (0x0F), `PREPENDQ` (0x1A) | ✅ Supported | Requires existing item. |
| TOUCH | `TOUCH` (0x1C) | ✅ Supported | Updates TTL on existing items. |
| GAT/GATQ | `GAT` (0x1D), `GATQ` (0x1E) | ✅ Supported | Get-and-touch with TTL update. Key included in response. |
| FLUSH | `FLUSH` (0x08), `FLUSHQ` (0x18) | ✅ Supported | Namespace-isolated: flushes only RedCouch keys (`rc:*`), never `FLUSHDB`. |
| NOOP | `NOOP` (0x0A) | ✅ Supported | Pipeline terminator. |
| QUIT | `QUIT` (0x07), `QUITQ` (0x17) | ✅ Supported | Graceful connection close. |
| VERSION | `VERSION` (0x0B) | ✅ Supported | Returns `RedCouch 0.1.0`. |
| STAT | `STAT` (0x10) | ✅ Supported | General stats (pid, uptime, version, cmd_get, cmd_set, curr_items). |
| VERBOSITY | `VERBOSITY` (0x1B) | ✅ Supported | Accepted and acknowledged; no runtime effect. |
| SASL AUTH | `SASL_LIST_MECHS` (0x20), `SASL_AUTH` (0x21), `SASL_STEP` (0x22) | ⚠️ Stub | Lists "PLAIN". Auth always succeeds — no credential enforcement. |
| Unknown | Any unrecognized opcode | ✅ Handled | Returns `Unknown command` (status 0x0081). |

### Unsupported Binary Behaviors

| Feature | Status | Reason |
|---|---|---|
| STAT groups (settings, items, slabs, conns) | ❌ Not supported | Returns empty terminator for sub-groups |
| Dynamic SASL credential enforcement | ❌ Not implemented | Stub-only: auth always succeeds |
| UDP transport | ❌ Not supported | TCP only |
| Couchbase bucket/vbucket management | ❌ Not supported | Outside bridge scope |

---

## ASCII Text Protocol

Based on the [memcached ASCII text protocol](https://github.com/memcached/memcached/blob/master/doc/protocol.txt). All 19 standard commands are implemented.


### Supported Commands

| Command | Syntax | Status |
|---|---|---|
| `set` | `set <key> <flags> <exptime> <bytes> [noreply]\r\n<data>\r\n` | ✅ |
| `add` | `add <key> <flags> <exptime> <bytes> [noreply]\r\n<data>\r\n` | ✅ |
| `replace` | `replace <key> <flags> <exptime> <bytes> [noreply]\r\n<data>\r\n` | ✅ |
| `cas` | `cas <key> <flags> <exptime> <bytes> <cas_unique> [noreply]\r\n<data>\r\n` | ✅ |
| `append` | `append <key> <bytes> [noreply]\r\n<data>\r\n` | ✅ |
| `prepend` | `prepend <key> <bytes> [noreply]\r\n<data>\r\n` | ✅ |
| `get` | `get <key> [<key> ...]` | ✅ |
| `gets` | `gets <key> [<key> ...]` | ✅ |
| `gat` | `gat <exptime> <key> [<key> ...]` | ✅ |
| `gats` | `gats <exptime> <key> [<key> ...]` | ✅ |
| `delete` | `delete <key> [noreply]` | ✅ |
| `incr` | `incr <key> <value> [noreply]` | ✅ |
| `decr` | `decr <key> <value> [noreply]` | ✅ |
| `touch` | `touch <key> <exptime> [noreply]` | ✅ |
| `flush_all` | `flush_all [delay] [noreply]` | ✅ |
| `version` | `version` | ✅ |
| `stats` | `stats [group]` | ✅ |
| `verbosity` | `verbosity <level> [noreply]` | ✅ |
| `quit` | `quit` | ✅ |

### Unsupported ASCII Behaviors

| Feature | Status | Reason |
|---|---|---|
| Authentication | ❌ Not supported | No SASL/auth in ASCII text mode (per memcached spec) |
| `flush_all` delay | ⚠️ Accepted, not honored | Delay parameter parsed but flush is immediate |
| `noreply` on malformed input | ⚠️ Partial | `CLIENT_ERROR` may still be emitted if `noreply` cannot be parsed before the error |

---

## Meta Protocol

Meta commands use two-letter prefixes and a flag-based system, routed through the ASCII text-protocol path.

### Supported Meta Commands

| Command | Syntax | Status | Supported Flags |
|---|---|---|---|
| `mg` (meta get) | `mg <key> [flags]` | ✅ | `v`, `c`, `f`, `k`, `s`, `O`, `q`, `t`, `T` |
| `ms` (meta set) | `ms <key> <datalen> [flags]\r\n<data>\r\n` | ✅ | `F`, `T`, `C`, `q`, `O`, `k`, `M` (mode: S/E/A/P/R) |
| `md` (meta delete) | `md <key> [flags]` | ✅ | `C`, `q`, `O`, `k` |
| `ma` (meta arithmetic) | `ma <key> [flags]` | ✅ | `D`, `J`, `N`, `q`, `O`, `k`, `v`, `c`, `M` (mode: I/D) |
| `mn` (meta noop) | `mn [flags]` | ✅ | `O` |
| `me` (meta debug) | `me <key> [flags]` | ⚠️ Stub | Returns `EN`. Flags `O`, `k`, `q` accepted. |

### Unsupported Meta Behaviors

| Feature | Status | Reason |
|---|---|---|
| Stale items (vivify/invalidate) | ❌ | Requires stale item concept not in item model |
| Recache (`R` flag on mg) | ❌ | Requires stale item concept |
| Win/lose/stale flags (`W`, `X`, `Z`) | ❌ | Requires stale item concept |
| Base64 keys (`b` flag) | ❌ | Not implemented |
| `me` debug data | ❌ Stub | Always returns `EN` (not found) |

---

## Item Model

All three protocols share the same underlying item model stored in Redis:

| Property | Implementation |
|---|---|
| Storage shape | Hash-per-item: `HSET <redis_key> v <value> f <flags> c <cas>` |
| Key namespace | Client key `foo` → Redis key `rc:foo` |
| Reserved keys | System keys under `redcouch:sys:*` (e.g., `redcouch:sys:cas_counter`) |
| CAS | Monotonic counter via `INCR redcouch:sys:cas_counter` |
| Binary-safe values | Full binary round-trip via Lua hex encode/decode |
| Flags | 32-bit unsigned, stored as decimal string |
| Expiry | `0` = no expiry, `≤2592000` = relative seconds, `>2592000` = absolute Unix timestamp |
| Atomic mutations | All CAS-sensitive operations use server-side Lua scripts |
| Flush scope | `FLUSH` operates only on `rc:*` keys, never `FLUSHDB` |