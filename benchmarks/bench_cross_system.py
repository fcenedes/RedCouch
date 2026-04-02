#!/usr/bin/env python3
"""
Cross-system benchmark harness: Couchbase OSS vs Redis OSS native vs Redis + RedCouch.

Produces an apples-to-apples comparison across three systems for equivalent
key-value operations (SET, GET hit, GET miss, DELETE) with identical workload
parameters, client counts, and duration.

Environment variables:
  REDCOUCH_HOST      - RedCouch memcached listener host (default 127.0.0.1)
  REDCOUCH_PORT      - RedCouch memcached listener port (default 11210)
  REDIS_NATIVE_HOST  - Redis OSS native host (default 127.0.0.1)
  REDIS_NATIVE_PORT  - Redis OSS native port (default 16380, Docker container)
  COUCHBASE_HOST     - Couchbase KV host (default 127.0.0.1)
  COUCHBASE_PORT     - Couchbase KV port (default 11211, mapped from container 11210)
  BENCH_DURATION     - seconds per workload (default 5)
  BENCH_CLIENTS      - comma-separated concurrency levels (default "1,4")
  BENCH_OUTPUT       - JSON results file path
  BENCH_TAG          - optional run tag
  SKIP_COUCHBASE     - set to "1" to skip Couchbase benchmarks
  SKIP_REDIS_NATIVE  - set to "1" to skip Redis native benchmarks
  SKIP_REDCOUCH      - set to "1" to skip RedCouch benchmarks
"""

import json, math, os, socket, statistics, struct, sys, threading, time

# ── Config ──────────────────────────────────────────────────────────
REDCOUCH_HOST = os.environ.get("REDCOUCH_HOST", "127.0.0.1")
REDCOUCH_PORT = int(os.environ.get("REDCOUCH_PORT", "11210"))
REDIS_NATIVE_HOST = os.environ.get("REDIS_NATIVE_HOST", "127.0.0.1")
REDIS_NATIVE_PORT = int(os.environ.get("REDIS_NATIVE_PORT", "16380"))
COUCHBASE_HOST = os.environ.get("COUCHBASE_HOST", "127.0.0.1")
COUCHBASE_PORT = int(os.environ.get("COUCHBASE_PORT", "11211"))
DURATION = int(os.environ.get("BENCH_DURATION", "5"))
CLIENTS = [int(c) for c in os.environ.get("BENCH_CLIENTS", "1,4").split(",")]
OUTPUT = os.environ.get("BENCH_OUTPUT", "benchmarks/results/cross_system_latest.json")
TAG = os.environ.get("BENCH_TAG", "")
SKIP_COUCHBASE = os.environ.get("SKIP_COUCHBASE", "0") == "1"
SKIP_REDIS_NATIVE = os.environ.get("SKIP_REDIS_NATIVE", "0") == "1"
SKIP_REDCOUCH = os.environ.get("SKIP_REDCOUCH", "0") == "1"
TMO = 5.0

# ── Binary protocol helpers (memcached) ────────────────────────────
MAGIC_REQ, MAGIC_RES, HDR = 0x80, 0x81, 24
OP_GET, OP_SET, OP_DELETE = 0x00, 0x01, 0x04
OP_NOOP, OP_FLUSH = 0x0A, 0x08

def build_req(opcode, opaque=0, cas=0, extras=b"", key=b"", value=b""):
    bl = len(extras) + len(key) + len(value)
    h = struct.pack(">BBHBBHI", MAGIC_REQ, opcode, len(key), len(extras), 0, 0, bl)
    h += struct.pack(">I", opaque) + struct.pack(">Q", cas)
    return h + extras + key + value

def parse_resp(data, off=0):
    if len(data) - off < HDR:
        return None, off
    mg, op, kl, el, _, st, bl = struct.unpack_from(">BBHBBHI", data, off)
    tot = HDR + bl
    if len(data) - off < tot:
        return None, off
    return {"status": st, "opcode": op, "body_len": bl}, off + tot

def recv_resp(sock):
    data = b""
    while True:
        chunk = sock.recv(4096)
        if not chunk:
            return None
        data += chunk
        resp, _ = parse_resp(data)
        if resp is not None:
            return resp

def make_mc_conn(host, port):
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.settimeout(TMO)
    s.connect((host, port))
    return s

# ── RESP (Redis protocol) helpers ──────────────────────────────────
def make_redis_conn(host, port):
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.settimeout(TMO)
    s.connect((host, port))
    return s

def redis_cmd(sock, *args):
    """Send a Redis RESP command and read the response."""
    cmd = f"*{len(args)}\r\n"
    for a in args:
        if isinstance(a, bytes):
            cmd_bytes = cmd.encode() + f"${len(a)}\r\n".encode() + a + b"\r\n"
            sock.sendall(cmd_bytes)
            return redis_read_resp(sock)
        cmd += f"${len(str(a))}\r\n{a}\r\n"
    sock.sendall(cmd.encode())
    return redis_read_resp(sock)

def redis_read_resp(sock):
    """Read a single RESP response (simple, error, integer, bulk, or null)."""
    line = b""
    while not line.endswith(b"\r\n"):
        b = sock.recv(1)
        if not b:
            return None
        line += b
    line = line[:-2]  # strip CRLF
    prefix = chr(line[0])
    payload = line[1:]
    if prefix == "+":
        return payload.decode()
    elif prefix == "-":
        return None  # error
    elif prefix == ":":
        return int(payload)
    elif prefix == "$":
        length = int(payload)
        if length == -1:
            return None  # null bulk
        data = b""
        while len(data) < length + 2:
            data += sock.recv(length + 2 - len(data))
        return data[:-2]  # strip trailing CRLF
    return None

# ── Percentile helper ───────────────────────────────────────────────
def percentiles(vals, ps=(50, 95, 99)):
    if not vals:
        return {f"p{p}": 0 for p in ps}
    s = sorted(vals)
    out = {}
    for p in ps:
        k = (len(s) - 1) * p / 100
        f = math.floor(k)
        c = math.ceil(k)
        out[f"p{p}"] = s[f] + (s[c] - s[f]) * (k - f) if f != c else s[f]
    return out

EXPECTED_STATUSES = {0x0000, 0x0001, 0x0005}  # OK, NOT_FOUND, NOT_STORED

# ── Generic workload runner ────────────────────────────────────────
def run_workload(name, system, connect_fn, op_fn, num_clients, duration_s):
    """Run op_fn in a loop for duration_s across num_clients threads."""
    latencies = []
    errors = [0]
    ops = [0]
    lock = threading.Lock()
    stop = threading.Event()

    def worker():
        local_lats = []
        local_ops = 0
        local_errs = 0
        try:
            conn = connect_fn()
        except Exception:
            with lock:
                errors[0] += 1
            return
        try:
            while not stop.is_set():
                t0 = time.monotonic()
                try:
                    op_fn(conn)
                    local_ops += 1
                except Exception:
                    local_errs += 1
                elapsed = time.monotonic() - t0
                local_lats.append(elapsed * 1_000_000)
        finally:
            try:
                conn.close()
            except Exception:
                pass
            with lock:
                latencies.extend(local_lats)
                errors[0] += local_errs
                ops[0] += local_ops

    threads = [threading.Thread(target=worker, daemon=True) for _ in range(num_clients)]
    t_start = time.monotonic()
    for t in threads:
        t.start()
    time.sleep(duration_s)
    stop.set()
    for t in threads:
        t.join(timeout=5)
    wall = time.monotonic() - t_start

    pcts = percentiles(latencies)
    total_attempted = ops[0] + errors[0]
    return {
        "workload": name,
        "system": system,
        "clients": num_clients,
        "duration_s": round(wall, 2),
        "total_ops": ops[0],
        "throughput_ops_sec": round(ops[0] / wall, 1) if wall > 0 else 0,
        "latency_us": {**pcts, "max": max(latencies) if latencies else 0,
                       "mean": round(statistics.mean(latencies), 1) if latencies else 0},
        "errors": errors[0],
        "error_rate": round(errors[0] / max(total_attempted, 1) * 100, 2),
    }

# ── Shared state for key generation ───────────────────────────────
_seq = [0]
SMALL_VAL = b"x" * 64
SET_EXTRAS = struct.pack(">II", 0, 0)  # flags=0, expiry=0

def next_key(prefix="bk:"):
    _seq[0] += 1
    return f"{prefix}{_seq[0] % 10000}"

# ── System-specific operation factories ───────────────────────────

# --- RedCouch (memcached binary protocol) ---
def redcouch_connect():
    return make_mc_conn(REDCOUCH_HOST, REDCOUCH_PORT)

def redcouch_set(sock):
    k = next_key("rc:").encode()
    sock.sendall(build_req(OP_SET, extras=SET_EXTRAS, key=k, value=SMALL_VAL))
    return recv_resp(sock)

def redcouch_get_hit(sock):
    sock.sendall(build_req(OP_GET, key=b"rc:seed:0"))
    return recv_resp(sock)

def redcouch_get_miss(sock):
    sock.sendall(build_req(OP_GET, key=b"rc:miss:nonexistent"))
    return recv_resp(sock)

def redcouch_delete(sock):
    k = next_key("rcdel:").encode()
    sock.sendall(build_req(OP_DELETE, key=k))
    return recv_resp(sock)

def redcouch_seed():
    try:
        s = redcouch_connect()
        for i in range(100):
            k = f"rc:seed:{i}".encode()
            s.sendall(build_req(OP_SET, extras=SET_EXTRAS, key=k, value=SMALL_VAL))
            recv_resp(s)
        s.close()
        return True
    except Exception as e:
        print(f"    WARNING: RedCouch seed failed: {e}", file=sys.stderr)
        return False

def redcouch_flush():
    try:
        s = redcouch_connect()
        s.sendall(build_req(OP_FLUSH))
        recv_resp(s)
        s.close()
    except Exception:
        pass

# --- Redis OSS native (RESP protocol) ---
def redis_native_connect():
    return make_redis_conn(REDIS_NATIVE_HOST, REDIS_NATIVE_PORT)

def redis_native_set(sock):
    k = next_key("rn:")
    return redis_cmd(sock, "SET", k, SMALL_VAL.decode())

def redis_native_get_hit(sock):
    return redis_cmd(sock, "GET", "rn:seed:0")

def redis_native_get_miss(sock):
    return redis_cmd(sock, "GET", "rn:miss:nonexistent")

def redis_native_delete(sock):
    k = next_key("rndel:")
    return redis_cmd(sock, "DEL", k)

def redis_native_seed():
    try:
        s = redis_native_connect()
        for i in range(100):
            redis_cmd(s, "SET", f"rn:seed:{i}", SMALL_VAL.decode())
        s.close()
        return True
    except Exception as e:
        print(f"    WARNING: Redis native seed failed: {e}", file=sys.stderr)
        return False

def redis_native_flush():
    try:
        s = redis_native_connect()
        redis_cmd(s, "FLUSHDB")
        s.close()
    except Exception:
        pass


# --- Couchbase OSS (memcached binary protocol) ---
def couchbase_connect():
    return make_mc_conn(COUCHBASE_HOST, COUCHBASE_PORT)

def couchbase_set(sock):
    k = next_key("cb:").encode()
    sock.sendall(build_req(OP_SET, extras=SET_EXTRAS, key=k, value=SMALL_VAL))
    return recv_resp(sock)

def couchbase_get_hit(sock):
    sock.sendall(build_req(OP_GET, key=b"cb:seed:0"))
    return recv_resp(sock)

def couchbase_get_miss(sock):
    sock.sendall(build_req(OP_GET, key=b"cb:miss:nonexistent"))
    return recv_resp(sock)

def couchbase_delete(sock):
    k = next_key("cbdel:").encode()
    sock.sendall(build_req(OP_DELETE, key=k))
    return recv_resp(sock)

def couchbase_seed():
    try:
        s = couchbase_connect()
        for i in range(100):
            k = f"cb:seed:{i}".encode()
            s.sendall(build_req(OP_SET, extras=SET_EXTRAS, key=k, value=SMALL_VAL))
            recv_resp(s)
        s.close()
        return True
    except Exception as e:
        print(f"    WARNING: Couchbase seed failed: {e}", file=sys.stderr)
        return False

def couchbase_flush():
    try:
        s = couchbase_connect()
        s.sendall(build_req(OP_FLUSH))
        recv_resp(s)
        s.close()
    except Exception:
        pass

# ── System definitions ─────────────────────────────────────────────
SYSTEMS = []

def probe_system(name, connect_fn):
    """Check if a system is reachable."""
    try:
        s = connect_fn()
        s.close()
        return True
    except Exception as e:
        print(f"  ⚠ {name} not reachable: {e}")
        return False

# ── Main ────────────────────────────────────────────────────────────
def main():
    import platform, datetime
    print("═══════════════════════════════════════════════════════════")
    print("Cross-System Benchmark: Couchbase OSS vs Redis OSS vs RedCouch")
    print("═══════════════════════════════════════════════════════════")
    print(f"  Duration:  {DURATION}s per workload")
    print(f"  Clients:   {CLIENTS}")
    print()

    # Probe each system
    systems = []
    if not SKIP_REDCOUCH:
        if probe_system("RedCouch", redcouch_connect):
            systems.append({
                "name": "redis_redcouch",
                "label": "Redis + RedCouch",
                "connect": redcouch_connect,
                "seed": redcouch_seed,
                "flush": redcouch_flush,
                "workloads": [
                    ("set_64B", redcouch_set),
                    ("get_hit", redcouch_get_hit),
                    ("get_miss", redcouch_get_miss),
                    ("delete", redcouch_delete),
                ],
            })

    if not SKIP_REDIS_NATIVE:
        if probe_system("Redis OSS native", redis_native_connect):
            systems.append({
                "name": "redis_native",
                "label": "Redis OSS native",
                "connect": redis_native_connect,
                "seed": redis_native_seed,
                "flush": redis_native_flush,
                "workloads": [
                    ("set_64B", redis_native_set),
                    ("get_hit", redis_native_get_hit),
                    ("get_miss", redis_native_get_miss),
                    ("delete", redis_native_delete),
                ],
            })

    if not SKIP_COUCHBASE:
        if probe_system("Couchbase OSS", couchbase_connect):
            systems.append({
                "name": "couchbase_oss",
                "label": "Couchbase OSS",
                "connect": couchbase_connect,
                "seed": couchbase_seed,
                "flush": couchbase_flush,
                "workloads": [
                    ("set_64B", couchbase_set),
                    ("get_hit", couchbase_get_hit),
                    ("get_miss", couchbase_get_miss),
                    ("delete", couchbase_delete),
                ],
            })

    if not systems:
        print("\nERROR: No systems reachable. Nothing to benchmark.")
        return 1

    print(f"\n  Systems available: {[s['label'] for s in systems]}")
    print()

    run_meta = {
        "timestamp": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "duration_s": DURATION,
        "client_counts": CLIENTS,
        "tag": TAG,
        "platform": platform.platform(),
        "python": platform.python_version(),
        "systems_tested": [s["name"] for s in systems],
        "systems_config": {
            "redis_redcouch": {"host": REDCOUCH_HOST, "port": REDCOUCH_PORT, "protocol": "memcached_binary"},
            "redis_native": {"host": REDIS_NATIVE_HOST, "port": REDIS_NATIVE_PORT, "protocol": "resp"},
            "couchbase_oss": {"host": COUCHBASE_HOST, "port": COUCHBASE_PORT, "protocol": "memcached_binary"},
        },
    }

    all_results = []

    for sys_info in systems:
        sname = sys_info["name"]
        slabel = sys_info["label"]
        print(f"\n{'═' * 60}")
        print(f"  System: {slabel}")
        print(f"{'═' * 60}")

        for num_clients in CLIENTS:
            print(f"\n  ── {num_clients} client(s) ──")
            sys_info["flush"]()
            time.sleep(0.3)
            sys_info["seed"]()
            time.sleep(0.3)

            for wl_name, wl_fn in sys_info["workloads"]:
                sys.stdout.write(f"    {wl_name:.<30s}")
                sys.stdout.flush()
                result = run_workload(wl_name, sname, sys_info["connect"], wl_fn, num_clients, DURATION)
                all_results.append(result)
                tput = result["throughput_ops_sec"]
                p99 = result["latency_us"]["p99"]
                errs = result["errors"]
                print(f" {tput:>10.0f} ops/s  p99={p99:>8.0f}µs  errs={errs}")

    # Write results
    output_data = {"meta": run_meta, "results": all_results}
    os.makedirs(os.path.dirname(OUTPUT) or ".", exist_ok=True)
    with open(OUTPUT, "w") as f:
        json.dump(output_data, f, indent=2)
    print(f"\n✅ Results written to {OUTPUT}")

    # Comparison summary table
    print_comparison_table(all_results)
    return 0

def print_comparison_table(results):
    """Print a side-by-side comparison table."""
    print("\n" + "═" * 90)
    print("CROSS-SYSTEM COMPARISON SUMMARY")
    print("═" * 90)
    print(f"{'Workload':<12s} {'System':<20s} {'Clients':>7s} {'Ops/s':>10s} "
          f"{'p50µs':>8s} {'p95µs':>8s} {'p99µs':>8s} {'Errs':>5s}")
    print("─" * 90)

    # Group by workload then by client count
    from collections import defaultdict
    by_wl = defaultdict(list)
    for r in results:
        by_wl[(r["workload"], r["clients"])].append(r)

    prev_wl = None
    for (wl, clients), entries in sorted(by_wl.items()):
        if prev_wl is not None and wl != prev_wl:
            print("─" * 90)
        prev_wl = wl
        for r in sorted(entries, key=lambda x: x["system"]):
            lat = r["latency_us"]
            sys_label = {"redis_redcouch": "Redis+RedCouch",
                        "redis_native": "Redis OSS native",
                        "couchbase_oss": "Couchbase OSS"}.get(r["system"], r["system"])
            print(f"{wl:<12s} {sys_label:<20s} {clients:>7d} "
                  f"{r['throughput_ops_sec']:>10.0f} "
                  f"{lat['p50']:>8.0f} {lat['p95']:>8.0f} {lat['p99']:>8.0f} "
                  f"{r['errors']:>5d}")

if __name__ == "__main__":
    sys.exit(main())