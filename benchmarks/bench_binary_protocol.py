#!/usr/bin/env python3
"""
RedCouch benchmark harness for the memcached binary protocol bridge.

Measures throughput, latency (p50/p95/p99/max), error rate, and captures
resource-usage snapshots for each workload scenario.

Requires: Redis 8+ with redcouch module loaded, listener on port 11210.

Environment variables:
  BENCH_HOST        - memcached listener host (default 127.0.0.1)
  BENCH_PORT        - memcached listener port (default 11210)
  BENCH_DURATION    - seconds per workload (default 5)
  BENCH_CLIENTS     - comma-separated concurrency levels (default "1,4,16")
  BENCH_OUTPUT      - JSON results file path (default benchmarks/results/latest.json)
  BENCH_TAG         - optional tag for the run (e.g. git sha, description)
  REDIS_PORT        - Redis port for direct checks (default 16379)
"""

import json, math, os, socket, statistics, struct, sys, threading, time

# ── Config ──────────────────────────────────────────────────────────
HOST = os.environ.get("BENCH_HOST", "127.0.0.1")
PORT = int(os.environ.get("BENCH_PORT", "11210"))
DURATION = int(os.environ.get("BENCH_DURATION", "5"))
CLIENTS = [int(c) for c in os.environ.get("BENCH_CLIENTS", "1,4,16").split(",")]
OUTPUT = os.environ.get("BENCH_OUTPUT", "benchmarks/results/latest.json")
TAG = os.environ.get("BENCH_TAG", "")
REDIS_PORT = int(os.environ.get("REDIS_PORT", "16379"))
TMO = 5.0

# ── Binary protocol helpers (matching integration tests) ────────────
MAGIC_REQ, MAGIC_RES, HDR = 0x80, 0x81, 24
OP_GET, OP_SET, OP_DELETE = 0x00, 0x01, 0x04
OP_INCR, OP_DECR = 0x05, 0x06
OP_NOOP, OP_GETQ, OP_SETQ = 0x0A, 0x09, 0x11
OP_APPEND, OP_PREPEND = 0x0E, 0x0F
OP_TOUCH, OP_GAT = 0x1C, 0x1D
OP_FLUSH = 0x08

def build_req(opcode, opaque=0, cas=0, extras=b"", key=b"", value=b""):
    bl = len(extras) + len(key) + len(value)
    h = struct.pack(">BBHBBHI", MAGIC_REQ, opcode, len(key), len(extras), 0, 0, bl)
    h += struct.pack(">I", opaque) + struct.pack(">Q", cas)
    return h + extras + key + value

def parse_resp(data, off=0):
    if len(data) - off < HDR:
        return None, off
    mg, op, kl, el, _, st, bl = struct.unpack_from(">BBHBBHI", data, off)
    oq = struct.unpack_from(">I", data, off + 12)[0]
    cs = struct.unpack_from(">Q", data, off + 16)[0]
    tot = HDR + bl
    if len(data) - off < tot:
        return None, off
    return {"status": st, "opcode": op, "cas": cs, "body_len": bl}, off + tot

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

def make_conn():
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.settimeout(TMO)
    s.connect((HOST, PORT))
    return s

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

# ── Workload runner ─────────────────────────────────────────────────
# Status codes considered "expected" per workload (not protocol errors).
# ST_NF (key not found) is expected for get_miss, delete, etc.
EXPECTED_STATUSES = {0x0000, 0x0001, 0x0005}  # ST_OK, ST_NF, ST_NOT_STORED

def run_workload(name, op_fn, num_clients, duration_s):
    """Run op_fn in a loop for duration_s seconds across num_clients threads.
    Returns dict with throughput, latency percentiles, error count, and
    unexpected protocol status counts."""
    latencies = []
    errors = [0]
    ops = [0]
    unexpected_statuses = [0]
    lock = threading.Lock()
    stop = threading.Event()

    def worker():
        local_lats = []
        local_ops = 0
        local_errs = 0
        local_unexpected = 0
        try:
            s = make_conn()
        except Exception:
            with lock:
                errors[0] += 1
            return
        try:
            while not stop.is_set():
                t0 = time.monotonic()
                try:
                    resp = op_fn(s)
                    local_ops += 1
                    # Check for unexpected protocol-level statuses
                    if resp is not None and resp.get("status") not in EXPECTED_STATUSES:
                        local_unexpected += 1
                except Exception:
                    local_errs += 1
                elapsed = time.monotonic() - t0
                local_lats.append(elapsed * 1_000_000)  # microseconds
        finally:
            try: s.close()
            except: pass
            with lock:
                latencies.extend(local_lats)
                errors[0] += local_errs
                ops[0] += local_ops
                unexpected_statuses[0] += local_unexpected

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
        "clients": num_clients,
        "duration_s": round(wall, 2),
        "total_ops": ops[0],
        "throughput_ops_sec": round(ops[0] / wall, 1) if wall > 0 else 0,
        "latency_us": {**pcts, "max": max(latencies) if latencies else 0,
                       "mean": round(statistics.mean(latencies), 1) if latencies else 0},
        "errors": errors[0],
        "unexpected_statuses": unexpected_statuses[0],
        "error_rate": round(errors[0] / max(total_attempted, 1) * 100, 2),
    }

SMALL_VAL = b"x" * 64
MEDIUM_VAL = b"y" * 1024
LARGE_VAL = b"z" * 65536
SET_EXTRAS = struct.pack(">II", 0, 0)  # flags=0, expiry=0
COUNTER_EXTRAS = struct.pack(">QQI", 1, 0, 0)  # delta=1, initial=0, expiry=0

_seq = [0]
def next_key(prefix=b"bk:"):
    _seq[0] += 1
    return prefix + str(_seq[0] % 10000).encode()

# ── Workload definitions ───────────────────────────────────────────
# Each returns a callable(sock) that performs one request/response cycle.

def op_set_small(sock):
    """SET with 64-byte value."""
    sock.sendall(build_req(OP_SET, extras=SET_EXTRAS, key=next_key(), value=SMALL_VAL))
    return recv_resp(sock)

def op_set_medium(sock):
    """SET with 1KB value."""
    sock.sendall(build_req(OP_SET, extras=SET_EXTRAS, key=next_key(), value=MEDIUM_VAL))
    return recv_resp(sock)

def op_set_large(sock):
    """SET with 64KB value."""
    sock.sendall(build_req(OP_SET, extras=SET_EXTRAS, key=next_key(), value=LARGE_VAL))
    return recv_resp(sock)

def op_get_hit(sock):
    """GET an existing key (pre-seeded)."""
    sock.sendall(build_req(OP_GET, key=b"bench:seed:0"))
    return recv_resp(sock)

def op_get_miss(sock):
    """GET a non-existent key."""
    sock.sendall(build_req(OP_GET, key=b"bench:miss:nonexistent"))
    return recv_resp(sock)

def op_delete(sock):
    """DELETE a key (may miss, that's fine for throughput)."""
    sock.sendall(build_req(OP_DELETE, key=next_key(b"del:")))
    return recv_resp(sock)

def op_incr(sock):
    """INCREMENT a counter."""
    sock.sendall(build_req(OP_INCR, extras=COUNTER_EXTRAS, key=b"bench:counter"))
    return recv_resp(sock)

def op_mixed_rw(sock):
    """50/50 SET then GET on the same key."""
    k = next_key(b"mx:")
    sock.sendall(build_req(OP_SET, extras=SET_EXTRAS, key=k, value=SMALL_VAL))
    recv_resp(sock)
    sock.sendall(build_req(OP_GET, key=k))
    return recv_resp(sock)

def op_append(sock):
    """APPEND to a key (pre-seeded)."""
    sock.sendall(build_req(OP_APPEND, key=b"bench:seed:0", value=b"A"))
    return recv_resp(sock)

def op_touch(sock):
    """TOUCH a key with TTL (pre-seeded)."""
    extras = struct.pack(">I", 300)  # 300s TTL
    sock.sendall(build_req(OP_TOUCH, extras=extras, key=b"bench:seed:0"))
    return recv_resp(sock)

# ── Seed data ───────────────────────────────────────────────────────
def seed_data():
    """Pre-populate keys needed for hit/append/touch workloads."""
    try:
        s = make_conn()
        for i in range(100):
            k = f"bench:seed:{i}".encode()
            s.sendall(build_req(OP_SET, extras=SET_EXTRAS, key=k, value=SMALL_VAL))
            recv_resp(s)
        s.close()
        print(f"  Seeded 100 keys")
    except Exception as e:
        print(f"  WARNING: seed failed: {e}", file=sys.stderr)

def flush_data():
    """Flush RedCouch data between runs."""
    try:
        s = make_conn()
        s.sendall(build_req(OP_FLUSH))
        recv_resp(s)
        s.close()
    except Exception:
        pass

# ── Resource capture ────────────────────────────────────────────────
def capture_resources():
    """Capture system-level resource info (best-effort)."""
    info = {"timestamp": time.time()}
    try:
        import subprocess
        # Get Redis memory info
        result = subprocess.run(
            ["redis-cli", "-p", str(REDIS_PORT), "INFO", "memory"],
            capture_output=True, text=True, timeout=5
        )
        for line in result.stdout.splitlines():
            if line.startswith("used_memory:"):
                info["redis_used_memory_bytes"] = int(line.split(":")[1])
            elif line.startswith("used_memory_rss:"):
                info["redis_rss_bytes"] = int(line.split(":")[1])
        # Get Redis CPU info
        result = subprocess.run(
            ["redis-cli", "-p", str(REDIS_PORT), "INFO", "cpu"],
            capture_output=True, text=True, timeout=5
        )
        for line in result.stdout.splitlines():
            if line.startswith("used_cpu_sys:"):
                info["redis_cpu_sys"] = float(line.split(":")[1])
            elif line.startswith("used_cpu_user:"):
                info["redis_cpu_user"] = float(line.split(":")[1])
        # Get Redis connection info
        result = subprocess.run(
            ["redis-cli", "-p", str(REDIS_PORT), "INFO", "clients"],
            capture_output=True, text=True, timeout=5
        )
        for line in result.stdout.splitlines():
            if line.startswith("connected_clients:"):
                info["redis_connected_clients"] = int(line.split(":")[1])
    except Exception as e:
        info["resource_capture_error"] = str(e)
    return info

def capture_connected_clients():
    """Quick query for current connected_clients count (best-effort)."""
    try:
        import subprocess
        result = subprocess.run(
            ["redis-cli", "-p", str(REDIS_PORT), "INFO", "clients"],
            capture_output=True, text=True, timeout=2
        )
        for line in result.stdout.splitlines():
            if line.startswith("connected_clients:"):
                return int(line.split(":")[1])
    except Exception:
        pass
    return None


# ── Workload registry ───────────────────────────────────────────────
WORKLOADS = [
    ("set_small_64B", op_set_small),
    ("set_medium_1KB", op_set_medium),
    ("set_large_64KB", op_set_large),
    ("get_hit", op_get_hit),
    ("get_miss", op_get_miss),
    ("delete", op_delete),
    ("increment", op_incr),
    ("mixed_read_write", op_mixed_rw),
    ("append", op_append),
    ("touch", op_touch),
]


# ── Main ────────────────────────────────────────────────────────────
def main():
    import platform, datetime
    print("═══════════════════════════════════════════════════════════")
    print("RedCouch Benchmark Suite")
    print("═══════════════════════════════════════════════════════════")
    print(f"  Host:      {HOST}:{PORT}")
    print(f"  Duration:  {DURATION}s per workload")
    print(f"  Clients:   {CLIENTS}")
    print(f"  Workloads: {len(WORKLOADS)}")
    print(f"  Output:    {OUTPUT}")
    print()

    run_meta = {
        "timestamp": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "host": HOST,
        "port": PORT,
        "duration_s": DURATION,
        "client_counts": CLIENTS,
        "tag": TAG,
        "platform": platform.platform(),
        "python": platform.python_version(),
    }

    all_results = []
    total_runs = len(WORKLOADS) * len(CLIENTS)
    run_num = 0

    for num_clients in CLIENTS:
        print(f"\n── {num_clients} client(s) ────────────────────────────────")
        # Seed once per client-count block
        flush_data()
        time.sleep(0.5)
        seed_data()
        time.sleep(0.5)

        for wl_name, wl_fn in WORKLOADS:
            run_num += 1
            label = f"[{run_num}/{total_runs}]"
            sys.stdout.write(f"  {label} {wl_name:.<30s}")
            sys.stdout.flush()

            # Capture resources before this specific workload
            pre_resources = capture_resources()

            result = run_workload(wl_name, wl_fn, num_clients, DURATION)

            # Capture resources after workload (sockets already closed)
            post_resources = capture_resources()
            result["resources_before"] = pre_resources
            result["resources_after"] = post_resources
            # Record configured concurrency (connected_clients from INFO
            # reflects only the sample instant; record intended concurrency)
            result["configured_clients"] = num_clients
            all_results.append(result)

            tput = result["throughput_ops_sec"]
            p99 = result["latency_us"]["p99"]
            errs = result["errors"]
            ustatus = result["unexpected_statuses"]
            print(f" {tput:>10.0f} ops/s  p99={p99:>8.0f}µs  errs={errs}  bad_status={ustatus}")

    # Write results
    output_data = {"meta": run_meta, "results": all_results}
    os.makedirs(os.path.dirname(OUTPUT) or ".", exist_ok=True)
    with open(OUTPUT, "w") as f:
        json.dump(output_data, f, indent=2)
    print(f"\n✅ Results written to {OUTPUT}")

    # Summary table
    print("\n── Summary ─────────────────────────────────────────────────")
    print(f"{'Workload':<25s} {'Clients':>7s} {'Ops/s':>10s} {'p50µs':>8s} "
          f"{'p95µs':>8s} {'p99µs':>8s} {'MaxµS':>8s} {'Errs':>5s} {'Bad':>5s}")
    print("─" * 90)
    for r in all_results:
        lat = r["latency_us"]
        print(f"{r['workload']:<25s} {r['clients']:>7d} {r['throughput_ops_sec']:>10.0f} "
              f"{lat['p50']:>8.0f} {lat['p95']:>8.0f} {lat['p99']:>8.0f} "
              f"{lat['max']:>8.0f} {r['errors']:>5d} {r['unexpected_statuses']:>5d}")

    return 0

if __name__ == "__main__":
    sys.exit(main())
