# RedCouch Examples

Runnable example scripts demonstrating RedCouch usage across languages and protocols.

## Prerequisites

- Redis 8+ running with RedCouch loaded:
  ```bash
  redis-server --loadmodule ./target/release/libred_couch.so      # Linux
  redis-server --loadmodule ./target/release/libred_couch.dylib    # macOS
  ```
- RedCouch listening on `127.0.0.1:11210` (default)

## Python Examples

Requires Python 3.10+.

### Basic Operations (pymemcache — ASCII protocol)

```bash
pip install pymemcache
python examples/python/basic_operations.py
```

Covers: set/get, flags, TTL, add/replace, CAS, counters, append/prepend, multi-get, touch, version.

### Binary Protocol (raw sockets — no dependencies)

```bash
python examples/python/binary_protocol_raw.py
```

Covers: binary SET/GET/ADD/DELETE/INCREMENT/APPEND/NOOP/VERSION using raw socket framing. No external dependencies needed.

## Shell Examples

### ASCII Protocol Session (netcat)

```bash
bash examples/shell/ascii_session.sh
```

Covers: version, set/get, counters, delete, meta protocol set/get/delete. Uses `nc` (netcat).

## Other Languages

See the [Multi-Language Examples](https://fcenedes.github.io/RedCouch/tutorials/multi-language.html) chapter in the documentation for Node.js, Go, and PHP examples with the exact client library code.

## Notes

- All examples use port **11210** (RedCouch default), not 11211 (standard memcached).
- Examples clean up after themselves — they delete any keys they create.
- The binary protocol example uses no external dependencies (only Python stdlib).
- For the full documentation site, run `mdbook build` from the repo root and open `book/output/index.html`.
