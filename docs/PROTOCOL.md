# Protocol Compatibility Reference

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
| INCREMENT/DECREMENT | `INCR` (0x05), `INCRQ` (0x15), `DECR` (0x06), `DECRQ` (0x16) | ✅ Supported | Unsigned 64-bit semantics with initial-value and miss rules. See [Limitations](#limitations). |
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

| Command | Syntax | Status | Notes |
|---|---|---|---|
| `set` | `set <key> <flags> <exptime> <bytes> [noreply]\r\n<data>\r\n` | ✅ Supported | |
| `add` | `add <key> <flags> <exptime> <bytes> [noreply]\r\n<data>\r\n` | ✅ Supported | |
| `replace` | `replace <key> <flags> <exptime> <bytes> [noreply]\r\n<data>\r\n` | ✅ Supported | |
| `cas` | `cas <key> <flags> <exptime> <bytes> <cas_unique> [noreply]\r\n<data>\r\n` | ✅ Supported | |
| `append` | `append <key> <bytes> [noreply]\r\n<data>\r\n` | ✅ Supported | No flags/exptime (per spec) |
| `prepend` | `prepend <key> <bytes> [noreply]\r\n<data>\r\n` | ✅ Supported | No flags/exptime (per spec) |
| `get` | `get <key> [<key> ...]` | ✅ Supported | Multi-key |
| `gets` | `gets <key> [<key> ...]` | ✅ Supported | Multi-key, returns CAS |
| `gat` | `gat <exptime> <key> [<key> ...]` | ✅ Supported | Get-and-touch |
| `gats` | `gats <exptime> <key> [<key> ...]` | ✅ Supported | Get-and-touch, returns CAS |
| `delete` | `delete <key> [noreply]` | ✅ Supported | |
| `incr` | `incr <key> <value> [noreply]` | ✅ Supported | NOT_FOUND for missing keys (no auto-create in ASCII) |
| `decr` | `decr <key> <value> [noreply]` | ✅ Supported | NOT_FOUND for missing keys |
| `touch` | `touch <key> <exptime> [noreply]` | ✅ Supported | |
| `flush_all` | `flush_all [delay] [noreply]` | ✅ Supported | Delay accepted but not honored |
| `version` | `version` | ✅ Supported | Returns `VERSION RedCouch 0.1.0` |
| `stats` | `stats [group]` | ✅ Supported | Bare `stats` returns general stats; unsupported groups return empty `END` |
| `verbosity` | `verbosity <level> [noreply]` | ✅ Supported | Accepted; no runtime effect |
| `quit` | `quit` | ✅ Supported | |

### Unsupported ASCII Behaviors

| Feature | Status | Reason |
|---|---|---|
| Authentication | ❌ Not supported | No SASL/auth in ASCII text mode (per memcached spec) |
| `flush_all` delay | ⚠️ Accepted, not honored | Delay parameter parsed but flush is immediate |
| `noreply` on malformed input | ⚠️ Partial | `CLIENT_ERROR` may still be emitted if `noreply` cannot be parsed before the error |


---

## Meta Protocol

Meta commands use two-letter prefixes and a flag-based system, routed through the ASCII text-protocol path. Meta commands are detected by prefix after ASCII protocol detection.

### Supported Meta Commands

| Command | Syntax | Status | Supported Flags |
|---|---|---|---|
| `mg` (meta get) | `mg <key> [flags]` | ✅ Supported | `v` (value), `c` (CAS), `f` (flags), `k` (key), `s` (size), `O` (opaque), `q` (quiet), `t` (TTL remaining), `T` (TTL update) |
| `ms` (meta set) | `ms <key> <datalen> [flags]\r\n<data>\r\n` | ✅ Supported | `F` (flags), `T` (TTL), `C` (CAS), `q` (quiet), `O` (opaque), `k` (key), `M` (mode: S/E/A/P/R) |
| `md` (meta delete) | `md <key> [flags]` | ✅ Supported | `C` (CAS), `q` (quiet), `O` (opaque), `k` (key) |
| `ma` (meta arithmetic) | `ma <key> [flags]` | ✅ Supported | `D` (delta), `J` (initial), `N` (auto-create TTL), `q` (quiet), `O` (opaque), `k` (key), `v` (value), `c` (CAS), `M` (mode: I/D) |
| `mn` (meta noop) | `mn [flags]` | ✅ Supported | `O` (opaque) |
| `me` (meta debug) | `me <key> [flags]` | ⚠️ Stub | Returns `EN` (not found). Flags `O`, `k`, `q` accepted. |

### Meta Set Modes (`M` flag)

| Mode | Meaning | Status |
|---|---|---|
| `S` | Set (default) | ✅ Supported |
| `E` | Add (set if not exists) | ✅ Supported |
| `A` | Append | ✅ Supported (no `F`/`T` flags with append) |
| `P` | Prepend | ✅ Supported (no `F`/`T` flags with prepend) |
| `R` | Replace | ✅ Supported |

### Meta Arithmetic Modes (`M` flag)

| Mode | Meaning | Status |
|---|---|---|
| `I` | Increment (default) | ✅ Supported |
| `D` | Decrement | ✅ Supported |

### Ignored Flags (Proxy Hints)

The following flags are silently accepted and ignored on all meta commands: `P`, `L`.

### Unsupported Meta Behaviors

| Feature | Status | Reason |
|---|---|---|
| Stale items (`N`/vivify on mg, `I`/invalidate on md) | ❌ Not supported | Requires stale item concept not in item model |
| Recache (`R` flag on mg) | ❌ Not supported | Requires stale item concept |
| Win/lose/stale flags (`W`, `X`, `Z`) | ❌ Not supported | Requires stale item concept |
| Base64 keys (`b` flag) | ❌ Not supported | Not implemented |
| `me` debug data | ❌ Stub only | Always returns `EN` (not found) |

Any unsupported flag is rejected with `CLIENT_ERROR unsupported meta flag '<flag>'`.

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

---

## Limitations

### Counter Precision (post-2^53)

Counter values are exact for `[0, 2^53)`. Above 2^53, behavior is **precision loss / rounding** rather than reliable wraparound. This is inherent to Redis's IEEE 754 double-precision floats in Lua scripts. The memcached binary protocol specifies unsigned 64-bit counter semantics; RedCouch cannot provide bit-exact behavior above 2^53.

### Append/Prepend Value Growth

APPEND/PREPEND operations retrieve existing values via Lua hex encode, concatenate, and store back. Cost is proportional to existing value size. Monitor value sizes for append-heavy workloads and consider periodic key rotation.

### Maximum Sizes

| Limit | Value |
|---|---|
| Max key length | 250 bytes |
| Max frame body | 20 MiB |
| Max command line (ASCII) | 2,048 bytes |