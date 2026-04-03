#!/usr/bin/env python3
"""
RedCouch basic operations example — pymemcache (ASCII protocol).

Prerequisites:
  - Redis 8+ with RedCouch loaded: redis-server --loadmodule ./target/release/libred_couch.so
  - pymemcache installed: pip install pymemcache
  - RedCouch listening on 127.0.0.1:11210 (default)

Usage:
  python examples/python/basic_operations.py
"""
import json
import sys

try:
    from pymemcache.client.base import Client
except ImportError:
    print("ERROR: pymemcache not installed. Run: pip install pymemcache")
    sys.exit(1)

HOST = "127.0.0.1"
PORT = 11210


def main():
    client = Client((HOST, PORT), connect_timeout=3, timeout=3)

    print("=== RedCouch Python Example (pymemcache) ===\n")

    # --- Basic set/get ---
    print("1. Basic set/get")
    client.set("demo:greeting", "Hello from RedCouch!")
    value = client.get("demo:greeting")
    print(f"   Stored and retrieved: {value}\n")

    # --- Flags and TTL ---
    print("2. Flags and expiration")
    data = {"user": "alice", "role": "admin"}
    client.set("demo:session", json.dumps(data).encode(), expire=300, flags=1)
    result = client.get("demo:session")
    print(f"   Session data: {json.loads(result)}\n")

    # --- Add / Replace ---
    print("3. Add and Replace (conditional stores)")
    added = client.add("demo:unique", "first-write")
    print(f"   add() new key: {added}")
    added_again = client.add("demo:unique", "second-write")
    print(f"   add() existing key: {added_again}")
    replaced = client.replace("demo:unique", "updated-value")
    print(f"   replace() existing key: {replaced}\n")

    # --- CAS (Compare-and-Swap) ---
    print("4. Compare-and-Swap (CAS)")
    client.set("demo:cas-key", "original")
    value, cas_token = client.gets("demo:cas-key")
    print(f"   Initial value: {value}, CAS: {cas_token}")
    success = client.cas("demo:cas-key", "updated-by-cas", cas_token)
    print(f"   CAS update with correct token: {success}")
    stale = client.cas("demo:cas-key", "stale-update", cas_token)
    print(f"   CAS update with stale token: {stale}\n")

    # --- Counters ---
    print("5. Counters (incr/decr)")
    client.set("demo:counter", "0")
    val = client.incr("demo:counter", 1)
    print(f"   After incr(1): {val}")
    val = client.incr("demo:counter", 10)
    print(f"   After incr(10): {val}")
    val = client.decr("demo:counter", 3)
    print(f"   After decr(3): {val}\n")

    # --- Append / Prepend ---
    print("6. Append and Prepend")
    client.set("demo:log", "entry-1")
    client.append("demo:log", ",entry-2")
    client.append("demo:log", ",entry-3")
    log_value = client.get("demo:log")
    print(f"   After appends: {log_value}")
    client.prepend("demo:log", "header:")
    log_value = client.get("demo:log")
    print(f"   After prepend: {log_value}\n")

    # --- Multi-get ---
    print("7. Multi-get")
    for i in range(5):
        client.set(f"demo:item:{i}", f"value-{i}")
    results = client.get_many([f"demo:item:{i}" for i in range(5)])
    for key, val in sorted(results.items()):
        print(f"   {key}: {val}")
    print()

    # --- Touch / Get-and-Touch ---
    print("8. Touch and Get-and-Touch")
    client.set("demo:ttl-key", "will-expire", expire=30)
    client.touch("demo:ttl-key", 300)
    print("   Extended TTL to 300s via touch()")
    value = client.gat("demo:ttl-key", 600)
    print(f"   get-and-touch: {value} (TTL now 600s)\n")

    # --- Version and Stats ---
    print("9. Version")
    version = client.version()
    print(f"   Server version: {version}\n")

    # --- Cleanup ---
    print("10. Cleanup")
    for key in ["demo:greeting", "demo:session", "demo:unique",
                 "demo:cas-key", "demo:counter", "demo:log",
                 "demo:ttl-key"] + [f"demo:item:{i}" for i in range(5)]:
        client.delete(key)
    print("   Deleted all demo keys\n")

    client.close()
    print("=== Done! ===")


if __name__ == "__main__":
    try:
        main()
    except ConnectionRefusedError:
        print(f"ERROR: Cannot connect to {HOST}:{PORT}")
        print("Make sure Redis is running with RedCouch loaded:")
        print("  redis-server --loadmodule ./target/release/libred_couch.so")
        sys.exit(1)
