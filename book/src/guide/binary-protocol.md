# Binary Protocol

Binary protocol clients connect to the same port (11210). The protocol is auto-detected from the first byte (`0x80` = binary request magic).

## Overview

The binary protocol is based on the [Couchbase memcached binary protocol](https://github.com/couchbase/memcached/blob/master/docs/BinaryProtocol.md). All 34 opcodes (0x00–0x22, excluding 0x1F) are parsed and dispatched.

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

See the [Protocol Compatibility Reference](../reference/protocol-compatibility.md) for the complete specification-level command tables.
