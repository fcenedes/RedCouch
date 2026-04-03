# ASCII Protocol

The ASCII text protocol is the simplest way to interact with RedCouch. It uses human-readable commands over a plain TCP connection, making it easy to test and debug with standard tools like `telnet` or `nc`.

RedCouch supports all 19 standard memcached ASCII commands. For the complete compatibility table with syntax details, see [Protocol Compatibility Reference](../reference/protocol-compatibility.md#ascii-text-protocol).

## Connecting

```bash
telnet 127.0.0.1 11210
# or
nc 127.0.0.1 11210
```

RedCouch auto-detects ASCII protocol when the first byte is a printable ASCII character (not `0x80`, which routes to binary protocol).

## Basic Key-Value Operations

```
# Store a value
set mykey 0 0 5
hello
STORED

# Retrieve it
get mykey
VALUE mykey 0 5
hello
END

# Store with flags and TTL (60 seconds)
set session:abc 42 60 11
session_data
STORED

# Retrieve with CAS token
gets mykey
VALUE mykey 0 5 1
hello
END

# Compare-and-swap (CAS) update
cas mykey 0 0 5 1
world
STORED

# Delete
delete mykey
DELETED
```

## Counters

```
# Create a counter via set (store numeric string)
set counter 0 0 1
0
STORED

# Increment
incr counter 1
1

# Increment by 10
incr counter 10
11

# Decrement
decr counter 3
8
```

> **Note:** In ASCII protocol, `incr`/`decr` return `NOT_FOUND` for missing keys. Use `set` to initialize counters first.

## Append and Prepend

```
set log 0 0 6
line-1
STORED

append log 7
,line-2
STORED

get log
VALUE log 0 13
line-1,line-2
END

prepend log 8
header: 
STORED
```

## Touch and Get-and-Touch

```
# Update TTL without fetching value
touch mykey 120
TOUCHED

# Get value and update TTL simultaneously
gat 300 mykey
VALUE mykey 0 5
world
END
```

## Stats and Version

```
version
VERSION RedCouch 0.1.0

stats
STAT pid 12345
STAT uptime 42
STAT version RedCouch 0.1.0
STAT cmd_get 5
STAT cmd_set 3
STAT curr_items 2
...
END
```

## Flush

```
# Flush all RedCouch items (only rc:* keys, not entire Redis DB)
flush_all
OK
```

## Noreply Mode

Most commands accept a `noreply` suffix that suppresses the server response. This is useful for fire-and-forget writes:

```
set background-job 0 60 4 noreply
data
```

No `STORED` response is sent. If the command is malformed, a `CLIENT_ERROR` may still be emitted because `noreply` cannot always be reliably parsed before the error is detected.

## Next Steps

- **[Meta Protocol](./meta-protocol.md)** — More powerful flag-based commands with richer control
- **[Binary Protocol](./binary-protocol.md)** — Machine-oriented protocol for high-throughput clients
- **[Protocol Compatibility Reference](../reference/protocol-compatibility.md)** — Complete syntax and status tables for all commands
- **[Known Limitations](../reference/limitations.md)** — Counter precision, append growth, and other caveats

## Quick Reference

| Command | Syntax |
|---|---|
| `set` | `set <key> <flags> <exptime> <bytes> [noreply]\r\n<data>\r\n` |
| `add` | `add <key> <flags> <exptime> <bytes> [noreply]\r\n<data>\r\n` |
| `replace` | `replace <key> <flags> <exptime> <bytes> [noreply]\r\n<data>\r\n` |
| `cas` | `cas <key> <flags> <exptime> <bytes> <cas_unique> [noreply]\r\n<data>\r\n` |
| `append` | `append <key> <bytes> [noreply]\r\n<data>\r\n` |
| `prepend` | `prepend <key> <bytes> [noreply]\r\n<data>\r\n` |
| `get` | `get <key> [<key> ...]` |
| `gets` | `gets <key> [<key> ...]` |
| `gat` | `gat <exptime> <key> [<key> ...]` |
| `gats` | `gats <exptime> <key> [<key> ...]` |
| `delete` | `delete <key> [noreply]` |
| `incr` | `incr <key> <value> [noreply]` |
| `decr` | `decr <key> <value> [noreply]` |
| `touch` | `touch <key> <exptime> [noreply]` |
| `flush_all` | `flush_all [delay] [noreply]` |
| `version` | `version` |
| `stats` | `stats [group]` |
| `verbosity` | `verbosity <level> [noreply]` |
| `quit` | `quit` |
