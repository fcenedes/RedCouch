# Meta Protocol

Meta commands use two-letter prefixes and a flag-based system. They are routed through the ASCII text-protocol path and detected by prefix after ASCII protocol detection.

Connect via telnet:

```bash
telnet 127.0.0.1 11210
```

## Meta Set and Get

```
# Set a value (ms = meta set, 5 = data length)
ms mykey 5
hello
HD

# Get with value and CAS (mg = meta get, v = value, c = CAS)
mg mykey v c
VA 5 c1
hello

# Get with key echo, flags, size
mg mykey k f s
HD kmykey f0 s5
```

## Meta Set Modes

The `M` flag controls the set mode:

```
# Add (only if not exists): ME
ms newkey 3 ME
foo
HD

# Replace (only if exists): MR
ms mykey 3 MR
bar
HD

# Append: MA
ms mykey 4 MA
_end
HD

# Prepend: MP
ms mykey 6 MP
start_
HD
```

| Mode | Meaning |
|---|---|
| `S` | Set (default) |
| `E` | Add (set if not exists) |
| `A` | Append |
| `P` | Prepend |
| `R` | Replace |

## Meta Delete

```
md mykey
HD
```

## Meta Arithmetic

```
# Create counter with initial value (J = initial, N = TTL for auto-create)
ma counter J0 N0
HD

# Increment by 5 (D = delta, v = return value)
ma counter D5 v
VA 1
5

# Decrement (MD = decrement mode)
ma counter MD D2 v
VA 1
3
```

## Meta Noop (Pipeline Terminator)

```
mn
MN
```

## Opaque Token (Request Correlation)

The `O` flag echoes an opaque token in the response:

```
mg mykey v Oreq-42
VA 5 Oreq-42
hello

mn Oping
MN Oping
```

## Supported Flags by Command

| Command | Supported Flags |
|---|---|
| `mg` (meta get) | `v` (value), `c` (CAS), `f` (flags), `k` (key), `s` (size), `O` (opaque), `q` (quiet), `t` (TTL remaining), `T` (TTL update) |
| `ms` (meta set) | `F` (flags), `T` (TTL), `C` (CAS), `q` (quiet), `O` (opaque), `k` (key), `M` (mode) |
| `md` (meta delete) | `C` (CAS), `q` (quiet), `O` (opaque), `k` (key) |
| `ma` (meta arithmetic) | `D` (delta), `J` (initial), `N` (auto-create TTL), `q` (quiet), `O` (opaque), `k` (key), `v` (value), `c` (CAS), `M` (mode: I/D) |
| `mn` (meta noop) | `O` (opaque) |
| `me` (meta debug) | `O`, `k`, `q` (stub: always returns `EN`) |

## Unsupported Meta Features

- Stale items (`N`/vivify on mg, `I`/invalidate on md)
- Recache (`R` flag on mg)
- Win/lose/stale flags (`W`, `X`, `Z`)
- Base64 keys (`b` flag)
- `me` debug data (stub only — always returns `EN`)

Any unsupported flag is rejected with `CLIENT_ERROR unsupported meta flag '<flag>'`.

Proxy hint flags `P` and `L` are silently accepted and ignored on all meta commands.
