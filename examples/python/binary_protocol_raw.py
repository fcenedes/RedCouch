#!/usr/bin/env python3
"""
RedCouch binary protocol example — raw sockets (no dependencies).

Demonstrates the memcached binary protocol wire format using only Python
standard library. This is the same approach used by RedCouch's integration
test suite (tests/integration/test_binary_protocol.py).

Prerequisites:
  - Redis 8+ with RedCouch loaded and listening on 127.0.0.1:11210

Usage:
  python examples/python/binary_protocol_raw.py
"""
import socket
import struct
import sys

HOST, PORT = "127.0.0.1", 11210
MAGIC_REQ = 0x80
MAGIC_RES = 0x81
HDR_SIZE = 24

# Opcodes
OP_GET = 0x00
OP_SET = 0x01
OP_ADD = 0x02
OP_DELETE = 0x04
OP_INCR = 0x05
OP_NOOP = 0x0A
OP_VERSION = 0x0B
OP_APPEND = 0x0E


def build_request(opcode, key=b"", value=b"", extras=b"", cas=0, opaque=0):
    """Build a memcached binary protocol request packet."""
    body_len = len(extras) + len(key) + len(value)
    header = struct.pack(
        ">BBHBBHIIQ",
        MAGIC_REQ, opcode, len(key), len(extras),
        0,  # data type
        0,  # reserved / vbucket
        body_len, opaque, cas,
    )
    return header + extras + key + value


def read_response(sock):
    """Read and parse a single binary protocol response."""
    header = b""
    while len(header) < HDR_SIZE:
        chunk = sock.recv(HDR_SIZE - len(header))
        if not chunk:
            raise ConnectionError("Connection closed")
        header += chunk

    magic, opcode, key_len, extras_len, data_type, status, body_len, opaque, cas = \
        struct.unpack(">BBHBBHIIQ", header)

    body = b""
    while len(body) < body_len:
        chunk = sock.recv(body_len - len(body))
        if not chunk:
            raise ConnectionError("Connection closed")
        body += chunk

    value = body[extras_len + key_len:]
    return {
        "status": status,
        "opcode": opcode,
        "cas": cas,
        "opaque": opaque,
        "extras": body[:extras_len],
        "key": body[extras_len:extras_len + key_len],
        "value": value,
    }


def main():
    print("=== RedCouch Binary Protocol Example (raw sockets) ===\n")

    sock = socket.create_connection((HOST, PORT), timeout=3)

    # --- VERSION ---
    print("1. VERSION")
    sock.sendall(build_request(OP_VERSION))
    resp = read_response(sock)
    print(f"   Status: {resp['status']}, Version: {resp['value'].decode()}\n")

    # --- SET ---
    print("2. SET 'bin-key' = 'hello-binary'")
    extras = struct.pack(">II", 0, 0)  # flags=0, expiry=0
    sock.sendall(build_request(OP_SET, key=b"bin-key", value=b"hello-binary", extras=extras))
    resp = read_response(sock)
    print(f"   Status: {resp['status']} (0=success), CAS: {resp['cas']}\n")
    set_cas = resp["cas"]

    # --- GET ---
    print("3. GET 'bin-key'")
    sock.sendall(build_request(OP_GET, key=b"bin-key"))
    resp = read_response(sock)
    print(f"   Status: {resp['status']}, Value: {resp['value'].decode()}, CAS: {resp['cas']}\n")

    # --- ADD (should fail — key exists) ---
    print("4. ADD 'bin-key' (should fail — key exists)")
    sock.sendall(build_request(OP_ADD, key=b"bin-key", value=b"dup", extras=extras))
    resp = read_response(sock)
    print(f"   Status: {resp['status']} (2=key exists)\n")

    # --- INCREMENT ---
    print("5. SET + INCREMENT counter")
    sock.sendall(build_request(OP_SET, key=b"bin-counter", value=b"0", extras=extras))
    read_response(sock)
    incr_extras = struct.pack(">QQI", 5, 0, 0)  # delta=5, initial=0, expiry=0
    sock.sendall(build_request(OP_INCR, key=b"bin-counter", extras=incr_extras))
    resp = read_response(sock)
    counter_val = struct.unpack(">Q", resp["value"])[0] if resp["value"] else 0
    print(f"   Status: {resp['status']}, Counter value: {counter_val}\n")

    # --- APPEND ---
    print("6. APPEND to 'bin-key'")
    sock.sendall(build_request(OP_APPEND, key=b"bin-key", value=b"-appended"))
    resp = read_response(sock)
    sock.sendall(build_request(OP_GET, key=b"bin-key"))
    resp = read_response(sock)
    print(f"   Value after append: {resp['value'].decode()}\n")

    # --- DELETE ---
    print("7. DELETE 'bin-key' and 'bin-counter'")
    sock.sendall(build_request(OP_DELETE, key=b"bin-key"))
    read_response(sock)
    sock.sendall(build_request(OP_DELETE, key=b"bin-counter"))
    read_response(sock)
    print("   Deleted both keys\n")

    # --- NOOP (pipeline terminator) ---
    print("8. NOOP")
    sock.sendall(build_request(OP_NOOP))
    resp = read_response(sock)
    print(f"   Status: {resp['status']} (pipeline flushed)\n")

    sock.close()
    print("=== Done! ===")


if __name__ == "__main__":
    try:
        main()
    except ConnectionRefusedError:
        print(f"ERROR: Cannot connect to {HOST}:{PORT}")
        print("Make sure Redis is running with RedCouch loaded.")
        sys.exit(1)
