#!/usr/bin/env python3
"""
RedCouch load / stress / soak validation suite.

Extends the benchmark harness with adversarial and long-running scenarios
required before GA:
  - quiet-pipeline batches (SETQ/GETQ + NOOP)
  - connection churn / reconnect storms
  - malformed-request / error-path traffic
  - high client counts (8 / 16 / 32 / 64)
  - bounded-append growth isolation
  - soak observation with periodic memory sampling
  - ThreadSafeContext / GIL pressure (high-rate INCR)

Environment variables:
  BENCH_HOST, BENCH_PORT, REDIS_PORT, BENCH_TAG
  STRESS_DURATION  - seconds per stress workload (default 15)
  SOAK_DURATION    - seconds for the soak phase   (default 120)
  STRESS_CLIENTS   - CSV client counts (default "1,4,8,16,32,64")
  STRESS_OUTPUT    - JSON results path
"""

import json, math, os, socket, statistics, struct, subprocess, sys
import threading, time, datetime, platform, traceback

# ── Config ──────────────────────────────────────────────────────────
HOST = os.environ.get("BENCH_HOST", "127.0.0.1")
PORT = int(os.environ.get("BENCH_PORT", "11210"))
REDIS_PORT = int(os.environ.get("REDIS_PORT", "16379"))
STRESS_DUR = int(os.environ.get("STRESS_DURATION", "15"))
SOAK_DUR = int(os.environ.get("SOAK_DURATION", "120"))
CLIENTS = [int(c) for c in os.environ.get("STRESS_CLIENTS", "1,4,8,16,32,64").split(",")]
OUTPUT = os.environ.get("STRESS_OUTPUT", "benchmarks/results/stress_latest.json")
TAG = os.environ.get("BENCH_TAG", "")
TMO = 5.0

# ── Binary protocol constants ──────────────────────────────────────
MAGIC_REQ, MAGIC_RES, HDR = 0x80, 0x81, 24
OP_GET, OP_SET, OP_DELETE = 0x00, 0x01, 0x04
OP_INCR = 0x05
OP_NOOP = 0x0A
OP_GETQ, OP_SETQ = 0x09, 0x11
OP_APPEND = 0x0E
OP_FLUSH = 0x08
EXPECTED_STATUSES = {0x0000, 0x0001, 0x0005}
SET_EXTRAS = struct.pack(">II", 0, 0)
COUNTER_EXTRAS = struct.pack(">QQI", 1, 0, 0)
SMALL_VAL = b"x" * 64

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

def recv_all_responses(sock, count):
    """Receive exactly count responses from the socket."""
    data = b""
    results = []
    while len(results) < count:
        chunk = sock.recv(8192)
        if not chunk:
            break
        data += chunk
        off = 0
        while len(results) < count:
            resp, new_off = parse_resp(data, off)
            if resp is None:
                break
            results.append(resp)
            off = new_off
        data = data[off:]
    return results

def make_conn():
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.settimeout(TMO)
    s.connect((HOST, PORT))
    return s

def percentiles(vals, ps=(50, 95, 99)):
    if not vals:
        return {f"p{p}": 0 for p in ps}
    s = sorted(vals)
    out = {}
    for p in ps:
        k = (len(s) - 1) * p / 100
        f, c = math.floor(k), math.ceil(k)
        out[f"p{p}"] = s[f] + (s[c] - s[f]) * (k - f) if f != c else s[f]
    return out

_seq = [0]
def next_key(prefix=b"sk:"):
    _seq[0] += 1
    return prefix + str(_seq[0] % 10000).encode()

# ── Resource capture ───────────────────────────────────────────────
def capture_resources():
    info = {"timestamp": time.time()}
    try:
        for section, keys in [
            ("memory", [("used_memory:", "used_memory_bytes", int),
                         ("used_memory_rss:", "rss_bytes", int)]),
            ("cpu", [("used_cpu_sys:", "cpu_sys", float),
                     ("used_cpu_user:", "cpu_user", float)]),
            ("clients", [("connected_clients:", "connected_clients", int)]),
        ]:
            r = subprocess.run(["redis-cli", "-p", str(REDIS_PORT), "INFO", section],
                               capture_output=True, text=True, timeout=5)
            for line in r.stdout.splitlines():
                for prefix, field, conv in keys:
                    if line.startswith(prefix):
                        info[field] = conv(line.split(":")[1].strip())
    except Exception as e:
        info["error"] = str(e)
    return info

# ── Generic workload runner ────────────────────────────────────────
def run_workload(name, op_fn, num_clients, duration_s):
    latencies, errors, ops, bad = [], [0], [0], [0]
    lock = threading.Lock()
    stop = threading.Event()

    def worker():
        ll, lo, le, lb = [], 0, 0, 0
        try:
            s = make_conn()
        except Exception:
            with lock: errors[0] += 1
            return
        try:
            while not stop.is_set():
                t0 = time.monotonic()
                try:
                    resp = op_fn(s)
                    lo += 1
                    if resp and resp.get("status") not in EXPECTED_STATUSES:
                        lb += 1
                except Exception:
                    le += 1
                ll.append((time.monotonic() - t0) * 1_000_000)
        finally:
            try: s.close()
            except: pass
            with lock:
                latencies.extend(ll); errors[0] += le; ops[0] += lo; bad[0] += lb

    threads = [threading.Thread(target=worker, daemon=True) for _ in range(num_clients)]
    t0 = time.monotonic()
    for t in threads: t.start()
    time.sleep(duration_s)
    stop.set()
    for t in threads: t.join(timeout=5)
    wall = time.monotonic() - t0
    pcts = percentiles(latencies)
    total = ops[0] + errors[0]
    return {
        "workload": name, "clients": num_clients, "duration_s": round(wall, 2),
        "total_ops": ops[0],
        "throughput_ops_sec": round(ops[0] / wall, 1) if wall else 0,
        "latency_us": {**pcts, "max": max(latencies) if latencies else 0,
                       "mean": round(statistics.mean(latencies), 1) if latencies else 0},
        "errors": errors[0], "unexpected_statuses": bad[0],
        "error_rate": round(errors[0] / max(total, 1) * 100, 2),
    }

# ── Seed / flush helpers ──────────────────────────────────────────
def seed_keys(n=100):
    try:
        s = make_conn()
        for i in range(n):
            s.sendall(build_req(OP_SET, extras=SET_EXTRAS,
                                key=f"seed:{i}".encode(), value=SMALL_VAL))
            recv_resp(s)
        s.close()
    except Exception as e:
        print(f"  seed warning: {e}", file=sys.stderr)

def flush():
    try:
        s = make_conn()
        s.sendall(build_req(OP_FLUSH))
        recv_resp(s)
        s.close()
    except: pass

# ════════════════════════════════════════════════════════════════════
# VALIDATION WORKLOADS
# ════════════════════════════════════════════════════════════════════

# 1. Quiet-pipeline: batch SETQ+GETQ terminated by NOOP
#    This is a traffic-generation and stability phase. It verifies that the
#    server survives pipelined quiet batches without crash or protocol
#    corruption and that the terminating NOOP is always received. It does
#    NOT assert per-GETQ hit/miss semantics (that is covered by the
#    integration/E2E test suite).
def op_quiet_pipeline(sock):
    """Send 10 SETQ + 10 GETQ + NOOP; drain all responses and assert NOOP terminator."""
    buf = b""
    for i in range(10):
        k = next_key(b"qp:")
        buf += build_req(OP_SETQ, opaque=i, extras=SET_EXTRAS, key=k, value=SMALL_VAL)
    for i in range(10):
        k = f"seed:{i % 100}".encode()
        buf += build_req(OP_GETQ, opaque=100 + i, key=k)
    buf += build_req(OP_NOOP, opaque=999)
    sock.sendall(buf)
    # Drain all responses until we see the terminating NOOP.
    # SETQ suppresses responses on success; GETQ returns on hit; NOOP always responds.
    data = b""
    responses = []
    noop_seen = False
    deadline = time.monotonic() + TMO
    while time.monotonic() < deadline:
        remaining = max(0.1, deadline - time.monotonic())
        sock.settimeout(remaining)
        try:
            chunk = sock.recv(8192)
        except socket.timeout:
            break
        if not chunk:
            break
        data += chunk
        off = 0
        while True:
            resp, new_off = parse_resp(data, off)
            if resp is None:
                break
            responses.append(resp)
            if resp["opcode"] == OP_NOOP:
                noop_seen = True
            off = new_off
        data = data[off:]
        if noop_seen:
            break
    sock.settimeout(TMO)
    # Assert the terminating NOOP was received (protocol integrity check)
    if not noop_seen:
        raise RuntimeError("quiet_pipeline: terminating NOOP not received")
    # The last response must be the NOOP
    if responses and responses[-1]["opcode"] != OP_NOOP:
        raise RuntimeError(f"quiet_pipeline: last response was opcode "
                           f"0x{responses[-1]['opcode']:02x}, expected NOOP")
    return responses[-1] if responses else None

# 2. Connection churn: rapid connect/disconnect
def run_connection_churn(duration_s, rate_per_sec=50):
    """Open and close connections as fast as possible, measuring failures."""
    successes, failures = 0, 0
    conn_latencies = []
    stop = threading.Event()
    lock = threading.Lock()

    def churner():
        nonlocal successes, failures
        ls, lf, ll = 0, 0, []
        while not stop.is_set():
            t0 = time.monotonic()
            try:
                s = make_conn()
                # Do a quick SET to exercise the connection
                s.sendall(build_req(OP_SET, extras=SET_EXTRAS,
                                    key=b"churn:test", value=b"v"))
                recv_resp(s)
                s.close()
                ls += 1
            except Exception:
                lf += 1
            ll.append((time.monotonic() - t0) * 1_000_000)
            # Throttle slightly to avoid pure CPU spin
            time.sleep(max(0, 1.0 / rate_per_sec - (time.monotonic() - t0)))
        with lock:
            nonlocal successes, failures
            successes += ls
            failures += lf
            conn_latencies.extend(ll)

    # Use 4 churner threads
    threads = [threading.Thread(target=churner, daemon=True) for _ in range(4)]
    t0 = time.monotonic()
    for t in threads: t.start()
    time.sleep(duration_s)
    stop.set()
    for t in threads: t.join(timeout=5)
    wall = time.monotonic() - t0

    return {
        "workload": "connection_churn",
        "duration_s": round(wall, 2),
        "total_connections": successes,
        "failed_connections": failures,
        "connections_per_sec": round(successes / wall, 1) if wall else 0,
        "latency_us": {**percentiles(conn_latencies),
                       "max": max(conn_latencies) if conn_latencies else 0,
                       "mean": round(statistics.mean(conn_latencies), 1) if conn_latencies else 0},
        "failure_rate": round(failures / max(successes + failures, 1) * 100, 2),
    }

# 3. Malformed request traffic
def run_malformed_requests():
    """Send various malformed packets and verify graceful handling."""
    results = []

    def try_malformed(name, data_bytes, expect_disconnect=True):
        try:
            s = make_conn()
            s.sendall(data_bytes)
            time.sleep(0.3)
            # Try to read response or detect disconnect
            try:
                s.settimeout(1.0)
                resp_data = s.recv(4096)
                if resp_data:
                    resp, _ = parse_resp(resp_data)
                    results.append({"name": name, "outcome": "response",
                                    "status": resp["status"] if resp else "parse_fail",
                                    "disconnected": False})
                else:
                    results.append({"name": name, "outcome": "eof",
                                    "disconnected": True})
            except socket.timeout:
                results.append({"name": name, "outcome": "timeout",
                                "disconnected": False})
            except ConnectionResetError:
                results.append({"name": name, "outcome": "reset",
                                "disconnected": True})
            finally:
                try: s.close()
                except: pass
        except Exception as e:
            results.append({"name": name, "outcome": "connect_fail",
                            "error": str(e)})

    # Bad magic byte
    try_malformed("bad_magic", b"\xff" + b"\x00" * 23)
    # Truncated header (less than 24 bytes)
    try_malformed("truncated_header", b"\x80\x00\x00\x03\x00\x00\x00\x00")
    # Body length mismatch (claims 1MB body but sends nothing)
    hdr = struct.pack(">BBHBBHI", MAGIC_REQ, OP_GET, 3, 0, 0, 0, 1048576)
    hdr += struct.pack(">I", 0) + struct.pack(">Q", 0)
    try_malformed("body_length_mismatch", hdr + b"key")
    # Zero-length key GET
    try_malformed("zero_key_get", build_req(OP_GET, key=b""))
    # Valid request after garbage (recovery test)
    garbage = b"\x00" * 50
    valid = build_req(OP_SET, extras=SET_EXTRAS, key=b"malformed:recover", value=b"v")
    try_malformed("garbage_then_valid", garbage + valid, expect_disconnect=True)
    # Oversized key (250+ bytes, memcached limit is 250)
    big_key = b"K" * 300
    try_malformed("oversized_key", build_req(OP_SET, extras=SET_EXTRAS,
                                             key=big_key, value=b"v"))

    return {"workload": "malformed_requests", "scenarios": results}

# 4. Bounded-append growth isolation
def run_append_growth(duration_s=15, num_keys=10, append_size=64):
    """Append to a bounded set of keys, measure value growth over time."""
    # Create initial keys
    s = make_conn()
    for i in range(num_keys):
        k = f"append:bounded:{i}".encode()
        s.sendall(build_req(OP_SET, extras=SET_EXTRAS, key=k, value=b"INIT"))
        recv_resp(s)

    growth_samples = []
    stop = threading.Event()
    append_count = [0]
    append_val = b"A" * append_size

    def appender():
        try:
            cs = make_conn()
            while not stop.is_set():
                for i in range(num_keys):
                    if stop.is_set():
                        break
                    k = f"append:bounded:{i}".encode()
                    cs.sendall(build_req(OP_APPEND, key=k, value=append_val))
                    resp = recv_resp(cs)
                    append_count[0] += 1
            cs.close()
        except: pass

    def sampler():
        while not stop.is_set():
            time.sleep(2)
            res = capture_resources()
            res["append_count"] = append_count[0]
            growth_samples.append(res)

    t_app = threading.Thread(target=appender, daemon=True)
    t_samp = threading.Thread(target=sampler, daemon=True)
    t0 = time.monotonic()
    t_app.start()
    t_samp.start()
    time.sleep(duration_s)
    stop.set()
    t_app.join(timeout=5)
    t_samp.join(timeout=3)
    wall = time.monotonic() - t0

    # Measure final sizes via GET
    final_sizes = []
    try:
        for i in range(num_keys):
            k = f"append:bounded:{i}".encode()
            s.sendall(build_req(OP_GET, key=k))
            resp_data = b""
            while True:
                chunk = s.recv(65536)
                if not chunk: break
                resp_data += chunk
                r, _ = parse_resp(resp_data)
                if r:
                    final_sizes.append(r["body_len"])
                    break
        s.close()
    except: pass

    return {
        "workload": "append_growth",
        "duration_s": round(wall, 2),
        "num_keys": num_keys,
        "total_appends": append_count[0],
        "append_size_bytes": append_size,
        "final_value_sizes": final_sizes,
        "memory_samples": growth_samples,
    }

# 5. Soak test with periodic memory sampling
def run_soak(duration_s, num_clients=8):
    """Mixed workload for extended duration with periodic resource snapshots."""
    latencies, errors, ops, bad = [], [0], [0], [0]
    lock = threading.Lock()
    stop = threading.Event()
    memory_timeline = []

    def mixed_worker():
        ll, lo, le, lb = [], 0, 0, 0
        try:
            s = make_conn()
        except Exception:
            with lock: errors[0] += 1
            return
        try:
            cycle = 0
            while not stop.is_set():
                t0 = time.monotonic()
                try:
                    cycle += 1
                    mod = cycle % 5
                    if mod == 0:  # SET
                        s.sendall(build_req(OP_SET, extras=SET_EXTRAS,
                                            key=next_key(b"soak:"), value=SMALL_VAL))
                        recv_resp(s)
                    elif mod == 1:  # GET hit
                        s.sendall(build_req(OP_GET, key=f"seed:{cycle % 100}".encode()))
                        recv_resp(s)
                    elif mod == 2:  # GET miss
                        s.sendall(build_req(OP_GET, key=b"soak:miss:nonexistent"))
                        recv_resp(s)
                    elif mod == 3:  # INCR
                        s.sendall(build_req(OP_INCR, extras=COUNTER_EXTRAS,
                                            key=b"soak:counter"))
                        recv_resp(s)
                    else:  # DELETE
                        s.sendall(build_req(OP_DELETE, key=next_key(b"soak:")))
                        recv_resp(s)
                    lo += 1
                except Exception:
                    le += 1
                ll.append((time.monotonic() - t0) * 1_000_000)
        finally:
            try: s.close()
            except: pass
            with lock:
                latencies.extend(ll); errors[0] += le; ops[0] += lo; bad[0] += lb

    def resource_sampler():
        interval = max(5, duration_s // 20)  # ~20 samples
        while not stop.is_set():
            time.sleep(interval)
            snap = capture_resources()
            snap["elapsed_s"] = round(time.monotonic() - t_start, 1)
            snap["ops_so_far"] = ops[0]
            memory_timeline.append(snap)

    threads = [threading.Thread(target=mixed_worker, daemon=True)
               for _ in range(num_clients)]
    sampler_t = threading.Thread(target=resource_sampler, daemon=True)

    t_start = time.monotonic()
    pre_resources = capture_resources()
    for t in threads: t.start()
    sampler_t.start()
    time.sleep(duration_s)
    stop.set()
    for t in threads: t.join(timeout=5)
    sampler_t.join(timeout=3)
    wall = time.monotonic() - t_start
    post_resources = capture_resources()

    pcts = percentiles(latencies)
    total = ops[0] + errors[0]

    # Compute memory delta
    mem_start = pre_resources.get("used_memory_bytes", 0)
    mem_end = post_resources.get("used_memory_bytes", 0)

    return {
        "workload": "soak_mixed",
        "clients": num_clients,
        "duration_s": round(wall, 2),
        "total_ops": ops[0],
        "throughput_ops_sec": round(ops[0] / wall, 1) if wall else 0,
        "latency_us": {**pcts, "max": max(latencies) if latencies else 0,
                       "mean": round(statistics.mean(latencies), 1) if latencies else 0},
        "errors": errors[0], "unexpected_statuses": bad[0],
        "error_rate": round(errors[0] / max(total, 1) * 100, 2),
        "memory_start_bytes": mem_start,
        "memory_end_bytes": mem_end,
        "memory_delta_bytes": mem_end - mem_start,
        "memory_timeline": memory_timeline,
        "resources_before": pre_resources,
        "resources_after": post_resources,
    }

# 6. High-rate INCR (ThreadSafeContext / GIL pressure)
def op_incr_pressure(sock):
    """Single INCR for GIL pressure testing."""
    sock.sendall(build_req(OP_INCR, extras=COUNTER_EXTRAS, key=b"gil:counter"))
    return recv_resp(sock)

# ════════════════════════════════════════════════════════════════════
# MAIN
# ════════════════════════════════════════════════════════════════════
def print_result_line(r):
    if "latency_us" in r:
        lat = r["latency_us"]
        errs = r.get("errors", 0)
        bad = r.get("unexpected_statuses", 0)
        tput = r.get("throughput_ops_sec", 0)
        print(f"  {r['workload']:<30s} c={r.get('clients','?'):>3}  "
              f"{tput:>10.0f} ops/s  p99={lat.get('p99',0):>8.0f}µs  "
              f"errs={errs}  bad={bad}")

def main():
    print("═══════════════════════════════════════════════════════════")
    print("RedCouch Stress / Soak Validation Suite")
    print("═══════════════════════════════════════════════════════════")
    print(f"  Host:           {HOST}:{PORT}")
    print(f"  Stress dur:     {STRESS_DUR}s per workload")
    print(f"  Soak dur:       {SOAK_DUR}s")
    print(f"  Client counts:  {CLIENTS}")
    print(f"  Output:         {OUTPUT}")
    print()

    # Capture provenance: Redis version/build and module git info
    redis_version = "unknown"
    try:
        rv = subprocess.run(["redis-server", "--version"],
                            capture_output=True, text=True, timeout=5)
        redis_version = rv.stdout.strip()
    except Exception:
        pass
    git_info = "unknown"
    try:
        gi = subprocess.run(["git", "describe", "--always", "--dirty", "--tags"],
                            capture_output=True, text=True, timeout=5)
        git_info = gi.stdout.strip()
    except Exception:
        pass

    run_meta = {
        "timestamp": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "host": HOST, "port": PORT,
        "stress_duration_s": STRESS_DUR, "soak_duration_s": SOAK_DUR,
        "client_counts": CLIENTS, "tag": TAG,
        "platform": platform.platform(), "python": platform.python_version(),
        "redis_version": redis_version,
        "module_git_ref": git_info,
    }

    all_results = []

    # ── Phase 1: Quiet-pipeline stress ─────────────────────────────
    print("\n── Phase 1: Quiet-pipeline coverage ────────────────────")
    flush(); time.sleep(0.3); seed_keys()
    for nc in [1, 4, 8]:
        r = run_workload(f"quiet_pipeline_c{nc}", op_quiet_pipeline, nc, STRESS_DUR)
        all_results.append(r)
        print_result_line(r)

    # ── Phase 2: High-concurrency load ─────────────────────────────
    print("\n── Phase 2: High-concurrency load ──────────────────────")
    def op_set_small(sock):
        sock.sendall(build_req(OP_SET, extras=SET_EXTRAS, key=next_key(), value=SMALL_VAL))
        return recv_resp(sock)
    def op_mixed(sock):
        k = next_key(b"hc:")
        sock.sendall(build_req(OP_SET, extras=SET_EXTRAS, key=k, value=SMALL_VAL))
        recv_resp(sock)
        sock.sendall(build_req(OP_GET, key=k))
        return recv_resp(sock)

    for nc in CLIENTS:
        flush(); time.sleep(0.3); seed_keys()
        r = run_workload(f"set_small_c{nc}", op_set_small, nc, STRESS_DUR)
        all_results.append(r)
        print_result_line(r)
        r = run_workload(f"mixed_rw_c{nc}", op_mixed, nc, STRESS_DUR)
        all_results.append(r)
        print_result_line(r)

    # ── Phase 3: Connection churn ──────────────────────────────────
    print("\n── Phase 3: Connection churn / reconnect storm ────────")
    churn_r = run_connection_churn(STRESS_DUR)
    all_results.append(churn_r)
    print(f"  connection_churn               "
          f"{churn_r['connections_per_sec']:>10.0f} conn/s  "
          f"fails={churn_r['failed_connections']}")

    # ── Phase 4: Malformed requests ────────────────────────────────
    print("\n── Phase 4: Malformed-request / error-path traffic ────")
    malf_r = run_malformed_requests()
    all_results.append(malf_r)
    for sc in malf_r["scenarios"]:
        print(f"  {sc['name']:<30s} → {sc['outcome']}")

    # ── Phase 5: Append growth isolation ───────────────────────────
    print("\n── Phase 5: Bounded-append growth isolation ────────────")
    append_r = run_append_growth(duration_s=STRESS_DUR)
    all_results.append(append_r)
    print(f"  append_growth                  "
          f"appends={append_r['total_appends']}  "
          f"final_sizes={append_r['final_value_sizes'][:3]}...")

    # ── Phase 6: GIL / ThreadSafeContext pressure ──────────────────
    print("\n── Phase 6: INCR pressure (GIL / ThreadSafeContext) ───")
    for nc in [1, 4, 16, 32, 64]:
        flush(); time.sleep(0.3)
        r = run_workload(f"incr_pressure_c{nc}", op_incr_pressure, nc, STRESS_DUR)
        all_results.append(r)
        print_result_line(r)

    # ── Phase 7: Soak observation ──────────────────────────────────
    print(f"\n── Phase 7: Soak observation ({SOAK_DUR}s) ──────────────────")
    flush(); time.sleep(0.3); seed_keys()
    soak_r = run_soak(SOAK_DUR, num_clients=8)
    all_results.append(soak_r)
    print_result_line(soak_r)
    mem_delta_kb = soak_r["memory_delta_bytes"] / 1024
    print(f"  Memory delta: {mem_delta_kb:+.1f} KB over {soak_r['duration_s']:.0f}s")

    # ── Write results ──────────────────────────────────────────────
    output_data = {"meta": run_meta, "results": all_results}
    os.makedirs(os.path.dirname(OUTPUT) or ".", exist_ok=True)
    with open(OUTPUT, "w") as f:
        json.dump(output_data, f, indent=2)
    print(f"\n✅ Results written to {OUTPUT}")

    # ── Summary ────────────────────────────────────────────────────
    print("\n── Summary ─────────────────────────────────────────────")
    total_errors = sum(r.get("errors", 0) + r.get("failed_connections", 0)
                       for r in all_results if isinstance(r, dict))
    total_bad = sum(r.get("unexpected_statuses", 0)
                    for r in all_results if isinstance(r, dict))
    print(f"  Total workload-level errors: {total_errors}")
    print(f"  Total unexpected statuses:   {total_bad}")
    if soak_r["error_rate"] == 0 and abs(mem_delta_kb) < 5120:
        print("  ✅ Soak passed: no errors, memory growth < 5MB")
    else:
        print(f"  ⚠️  Soak review needed: err_rate={soak_r['error_rate']}% "
              f"mem_delta={mem_delta_kb:.1f}KB")

    return 0

if __name__ == "__main__":
    sys.exit(main())
