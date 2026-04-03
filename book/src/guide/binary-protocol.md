# Binary Protocol

The binary protocol is the machine-oriented protocol used by Couchbase SDKs and some memcached client libraries. It uses a fixed-size 24-byte header followed by variable-length extras, key, and value fields.

Binary protocol clients connect to the same port (11210) as ASCII clients. RedCouch auto-detects binary protocol when the first byte is `0x80` (the binary request magic byte).

For the complete opcode table and compatibility details, see [Protocol Compatibility Reference](../reference/protocol-compatibility.md#binary-protocol).

## Overview

The binary protocol is based on the [Couchbase memcached binary protocol](https://github.com/couchbase/memcached/blob/master/docs/BinaryProtocol.md). All 34 opcodes (0x00–0x22, excluding 0x1F) are parsed and dispatched. This includes quiet variants (which suppress success responses for pipelining) and key-returning variants.

## Example: Raw Socket Binary Protocol (Python)

RedCouch's verified binary protocol test suite (`tests/integration/test_binary_protocol.py`) uses raw socket framing to exercise all 34 binary opcodes. Below is a simplified example:

```python
import socket, struct

MAGIC_REQ, MAGIC_RES, HDR = 0x80, 0x81, 24
OP_SET, OP_GET = 0x01, 0x00

def build_req(opcode, extras=b"", key=b"", value=b"", cas=0):
    bl = len(extras) + len(key) + len(value)
    hdr = struct.pack(">BBHBBHIIQ", MAGIC_REQ, opcode, len(key),
                      len(extras), 0, 0, bl, 0, cas)
    return hdr + extras + key + value

def read_resp(sock):
    hdr = sock.recv(HDR)
    magic, op, kl, el, dt, st, bl, opq, cas = struct.unpack(">BBHBBHIIQ", hdr)
    body = b""
    while len(body) < bl:
        body += sock.recv(bl - len(body))
    return st, cas, body[el + kl:]

sock = socket.create_connection(("127.0.0.1", 11210), timeout=3)

# SET key1 = b"hello" with flags=0, expiry=0
extras = struct.pack(">II", 0, 0)  # flags (4 bytes) + expiry (4 bytes)
sock.sendall(build_req(OP_SET, extras=extras, key=b"key1", value=b"hello"))
status, cas, _ = read_resp(sock)
assert status == 0  # success

# GET key1
sock.sendall(build_req(OP_GET, key=b"key1"))
status, cas, value = read_resp(sock)
assert status == 0 and value == b"hello"

sock.close()
```

## Supported Binary Operations

| Opcode Family | Opcodes | Notes |
|---|---|---|
| GET | `GET`, `GETQ`, `GETK`, `GETKQ` | Quiet variants suppress success. Key variants echo key. |
| SET/ADD/REPLACE | `SET`, `SETQ`, `ADD`, `ADDQ`, `REPLACE`, `REPLACEQ` | CAS-checked. Flags, expiry, binary-safe values preserved. |
| DELETE | `DELETE`, `DELETEQ` | CAS-checked. |
| INCREMENT/DECREMENT | `INCR`, `INCRQ`, `DECR`, `DECRQ` | Unsigned 64-bit with initial-value and miss rules. |
| APPEND/PREPEND | `APPEND`, `APPENDQ`, `PREPEND`, `PREPENDQ` | Requires existing item. |
| TOUCH | `TOUCH` | Updates TTL on existing items. |
| GAT/GATQ | `GAT`, `GATQ` | Get-and-touch with TTL update. |
| FLUSH | `FLUSH`, `FLUSHQ` | Namespace-isolated: only `rc:*` keys. |
| NOOP | `NOOP` | Pipeline terminator. |
| QUIT | `QUIT`, `QUITQ` | Graceful close. |
| VERSION | `VERSION` | Returns `RedCouch 0.1.0`. |
| STAT | `STAT` | General stats. |
| VERBOSITY | `VERBOSITY` | Accepted, no effect. |
| SASL AUTH | `SASL_LIST_MECHS`, `SASL_AUTH`, `SASL_STEP` | Stub: auth always succeeds. |

## SASL Authentication

Binary protocol clients (especially Couchbase SDKs) often require SASL authentication before sending data commands. RedCouch supports the SASL handshake but with **stub-only authentication** — all credentials are accepted:

- `SASL_LIST_MECHS` → Returns `PLAIN`
- `SASL_AUTH` → Always succeeds regardless of username/password
- `SASL_STEP` → Always succeeds

This allows SASL-requiring clients to connect without code changes. See [Known Limitations](../reference/limitations.md#sasl-authentication) for security implications.

## Quiet Commands and Pipelining

Quiet variants (e.g., `GETQ`, `SETQ`, `DELETEQ`) suppress success responses, enabling efficient pipelining. Send a batch of quiet operations followed by a `NOOP` — the NOOP response signals that all preceding quiet operations have been processed.

RedCouch batches all responses from a read cycle into a single `write_all()` call for efficiency.

## Next Steps

- **[ASCII Protocol](./ascii-protocol.md)** — Human-readable protocol for debugging and simple clients
- **[Meta Protocol](./meta-protocol.md)** — Flag-based meta commands with fine-grained control
- **[Protocol Compatibility Reference](../reference/protocol-compatibility.md)** — Complete specification-level command tables
