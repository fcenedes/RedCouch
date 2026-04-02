#!/usr/bin/env python3
"""
End-to-end integration tests for the RedCouch memcached binary protocol bridge.
Requires: Redis 8+ with redcouch module loaded, listener on port 11210.

The module now uses a hash-per-item model (no JSON dependency):
  - Each memcached key → Redis Hash at `rc:<key>` with fields v, f, c
  - CAS values are real, from a Redis-backed monotonic counter
  - Values are stored as raw bytes (binary-safe)
  - Flags are preserved from SET extras

Environment variables:
  REDIS_PORT  - Redis port for direct verification (default 16379)
"""
import os, socket, struct, subprocess, sys, time

MAGIC_REQ, MAGIC_RES, HDR = 0x80, 0x81, 24
OP_GET, OP_SET, OP_ADD, OP_DELETE = 0x00, 0x01, 0x02, 0x04
OP_INCR, OP_DECR = 0x05, 0x06
OP_NOOP, OP_GETK = 0x0A, 0x0C
OP_SETQ, OP_ADDQ, OP_DELETEQ = 0x11, 0x12, 0x14
ST_OK, ST_NF, ST_IX, ST_ARGS, ST_UNK = 0, 1, 2, 4, 0x81
CAS_ZERO = 0
HOST, PORT, TMO = "127.0.0.1", 11210, 3.0
REDIS_PORT = int(os.environ.get("REDIS_PORT", "16379"))
results = []
known_gaps = []

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
    return {"magic": mg, "opcode": op, "key_len": kl, "extras_len": el,
            "status": st, "body_len": bl, "opaque": oq, "cas": cs,
            "body": data[off + HDR:off + tot]}, off + tot

def conn():
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.settimeout(TMO)
    s.connect((HOST, PORT))
    return s

def recv_min(s, n=HDR):
    buf = b""
    deadline = time.time() + TMO
    while len(buf) < n and time.time() < deadline:
        try:
            c = s.recv(4096)
            if not c:
                break
            buf += c
        except socket.timeout:
            break
    return buf

def chk(name, ok, detail=""):
    results.append((name, ok, detail))
    tag = "PASS" if ok else "FAIL"
    print(f"  [{tag}] {name}" + (f" -- {detail}" if detail and not ok else ""))

def gap(name, detail):
    known_gaps.append((name, detail))
    print(f"  [GAP ] {name} -- {detail}")

def set_extras(flags=0, expiry=0):
    return struct.pack(">II", flags, expiry)

def incr_extras(delta=1, initial=0, expiry=0):
    return struct.pack(">QQI", delta, initial, expiry)

_TS = str(int(time.time()))
def tkey(name):
    return f"t:{_TS}:{name}".encode()

def redis_key(client_key):
    """Return the namespaced Redis key for a client key."""
    if isinstance(client_key, bytes):
        return "rc:" + client_key.decode()
    return "rc:" + client_key

def redis_cli(*args):
    """Run a redis-cli command against the test Redis instance."""
    try:
        r = subprocess.run(
            ["redis-cli", "-p", str(REDIS_PORT)] + list(args),
            capture_output=True, text=True, timeout=5)
        return r.stdout.strip()
    except Exception as e:
        return f"ERROR: {e}"


# ═════════════════════════════════════════════════════════════════
# 0. Backing store preflight — hash-per-item model
# ═════════════════════════════════════════════════════════════════
def test_backing_store():
    """Verify that SET creates a Redis Hash with v/f/c fields."""
    s = conn()
    k = tkey("preflight")
    s.sendall(build_req(OP_SET, opaque=50, extras=set_extras(flags=42),
                        key=k, value=b"preflight_value"))
    d = recv_min(s, HDR)
    s.close()
    r, _ = parse_resp(d)

    rk = redis_key(k)
    exists = redis_cli("EXISTS", rk)
    key_type = redis_cli("TYPE", rk)

    chk("set_returns_st_ok", r and r["status"] == ST_OK, f"got {r}")
    chk("set_creates_hash_key", exists == "1",
        f"EXISTS={exists} for {rk}")
    chk("set_key_type_hash", key_type == "hash",
        f"expected hash, got {key_type!r}")

    if exists == "1":
        val = redis_cli("HGET", rk, "v")
        flags = redis_cli("HGET", rk, "f")
        cas = redis_cli("HGET", rk, "c")
        chk("set_stores_value", val == "preflight_value",
            f"expected 'preflight_value', got {val!r}")
        chk("set_stores_flags", flags == "42",
            f"expected '42', got {flags!r}")
        chk("set_stores_cas", cas and int(cas) > 0,
            f"expected non-zero CAS, got {cas!r}")
    chk("set_returns_real_cas", r and r["cas"] > 0,
        f"cas={r and r['cas']}, expected > 0")


# ── 1. NOOP baseline ────────────────────────────────────────────
def test_noop():
    s = conn()
    s.sendall(build_req(OP_NOOP, opaque=900))
    d = recv_min(s, HDR)
    s.close()
    r, _ = parse_resp(d)
    chk("noop_ok", r and r["status"] == ST_OK and r["opaque"] == 900, f"got {r}")
    chk("noop_cas_zero", r and r["cas"] == CAS_ZERO, f"cas={r and r['cas']}")


# ── 2. Bad magic → connection close ─────────────────────────────
def test_bad_magic():
    s = conn()
    bad = bytearray(build_req(OP_NOOP, opaque=1))
    bad[0] = 0x42
    s.sendall(bytes(bad))
    time.sleep(0.3)
    try:
        d = s.recv(4096)
        chk("bad_magic_closes_conn", len(d) == 0,
            f"got {len(d)} bytes instead of conn close")
    except (ConnectionResetError, BrokenPipeError):
        chk("bad_magic_closes_conn", True)
    except socket.timeout:
        chk("bad_magic_closes_conn", False, "timeout, conn not closed")
    finally:
        s.close()


# ── 3. Malformed frame → ST_ARGS + continued parsing ────────────
def test_malformed_frame():
    s = conn()
    mf = bytearray(HDR + 2)
    mf[0] = MAGIC_REQ
    mf[1] = OP_SET
    struct.pack_into(">H", mf, 2, 2)     # key_len=2
    mf[4] = 1                             # extras_len=1
    struct.pack_into(">I", mf, 8, 2)     # body_len=2 → 1+2=3 > 2
    struct.pack_into(">I", mf, 12, 100)  # opaque
    noop = build_req(OP_NOOP, opaque=200)
    s.sendall(bytes(mf) + noop)
    d = recv_min(s, HDR * 2)
    s.close()
    r1, off = parse_resp(d)
    chk("malformed_returns_st_args", r1 and r1["status"] == ST_ARGS, f"got {r1}")
    chk("malformed_echoes_opaque", r1 and r1["opaque"] == 100, f"got {r1}")
    r2, _ = parse_resp(d, off)
    chk("malformed_continues_parsing",
        r2 and r2["status"] == ST_OK and r2["opaque"] == 200, f"got {r2}")


# ── 4. Unknown opcode → ST_UNK + continue ───────────────────────
def test_unknown_opcode():
    s = conn()
    unk = bytearray(build_req(OP_NOOP, opaque=300))
    unk[1] = 0xFE
    noop = build_req(OP_NOOP, opaque=301)
    s.sendall(bytes(unk) + noop)
    d = recv_min(s, HDR * 2)
    s.close()
    r1, off = parse_resp(d)
    chk("unk_opcode_st_unk", r1 and r1["status"] == ST_UNK, f"got {r1}")
    chk("unk_opcode_echoes_0xFE", r1 and r1["opcode"] == 0xFE, f"got {r1}")
    chk("unk_opcode_cas_zero", r1 and r1["cas"] == CAS_ZERO, f"got {r1}")
    r2, _ = parse_resp(d, off)
    chk("unk_opcode_continues",
        r2 and r2["status"] == ST_OK and r2["opaque"] == 301, f"got {r2}")


# ── 5. SETQ success suppression ─────────────────────────────────
def test_quiet_setq_suppressed():
    """SETQ success → no response; trailing NOOP confirms pipeline."""
    s = conn()
    k = tkey("qset")
    setq = build_req(OP_SETQ, opaque=400, extras=set_extras(),
                     key=k, value=b"qval")
    noop = build_req(OP_NOOP, opaque=401)
    s.sendall(setq + noop)
    d = recv_min(s, HDR)
    s.close()
    r1, _ = parse_resp(d)
    chk("setq_success_suppressed",
        r1 and r1["opaque"] == 401 and r1["status"] == ST_OK,
        f"expected NOOP opaque=401, got {r1}")


# ── 6. Quiet error NOT suppressed (ADDQ on existing) ────────────
def test_quiet_addq_error_preserved():
    """ADD on existing key → ST_IX; ADDQ should also return ST_IX."""
    s = conn()
    k = tkey("qadd")
    # First SET the key (loud, consume response)
    s.sendall(build_req(OP_SET, opaque=410, extras=set_extras(),
                        key=k, value=b"v1"))
    r0d = recv_min(s, HDR)
    r0, _ = parse_resp(r0d)
    chk("addq_set_prerequisite", r0 and r0["status"] == ST_OK,
        f"SET prerequisite: {r0}")
    if not r0 or r0["status"] != ST_OK:
        s.close()
        return
    # ADDQ same key (should fail with ST_IX, error NOT suppressed)
    addq = build_req(OP_ADDQ, opaque=411, extras=set_extras(),
                     key=k, value=b"v2")
    noop = build_req(OP_NOOP, opaque=412)
    s.sendall(addq + noop)
    d = recv_min(s, HDR * 2)
    s.close()
    r1, off = parse_resp(d)
    chk("addq_error_not_suppressed",
        r1 and r1["opaque"] == 411 and r1["status"] == ST_IX,
        f"expected ADDQ opaque=411 ST_IX, got {r1}")
    if r1 and r1["opaque"] == 411:
        r2, _ = parse_resp(d, off)
        chk("addq_noop_follows",
            r2 and r2["opaque"] == 412 and r2["status"] == ST_OK,
            f"expected NOOP opaque=412, got {r2}")


# ── 7. DELETEQ success suppressed, miss error sent ──────────────
def test_quiet_deleteq():
    """DELETEQ hit → suppressed; DELETEQ miss → ST_NF sent."""
    s = conn()
    k = tkey("qdel2")
    # Create key and wait for response
    s.sendall(build_req(OP_SET, opaque=500, extras=set_extras(),
                        key=k, value=b"dv"))
    r0d = recv_min(s, HDR)
    r0, _ = parse_resp(r0d)
    chk("deleteq_set_prerequisite", r0 and r0["status"] == ST_OK,
        f"SET prerequisite: {r0}")
    if not r0 or r0["status"] != ST_OK:
        s.close()
        return
    # DELETEQ existing → success suppressed
    delq1 = build_req(OP_DELETEQ, opaque=501, key=k)
    # DELETEQ same key (now gone) → miss → ST_NF
    delq2 = build_req(OP_DELETEQ, opaque=502, key=k)
    noop = build_req(OP_NOOP, opaque=503)
    s.sendall(delq1 + delq2 + noop)
    d = recv_min(s, HDR * 2)
    s.close()
    r1, off = parse_resp(d)
    chk("deleteq_success_suppressed",
        r1 and r1["opaque"] == 502,
        f"expected opaque=502 (first DELETEQ suppressed), got {r1}")
    if r1 and r1["opaque"] == 502:
        chk("deleteq_miss_error_sent", r1["status"] == ST_NF,
            f"expected ST_NF, got 0x{r1['status']:04x}")


# ── 8. CAS: real per-item CAS on success, ZERO on error/control ──
def test_cas_set_success():
    """SET success → real non-zero CAS from counter."""
    s = conn()
    k = tkey("cas_s")
    s.sendall(build_req(OP_SET, opaque=600, extras=set_extras(),
                        key=k, value=b"cv"))
    d = recv_min(s, HDR)
    s.close()
    r, _ = parse_resp(d)
    chk("cas_set_success_nonzero",
        r and r["status"] == ST_OK and r["cas"] > 0,
        f"got cas={r and r['cas']}")


def test_cas_errors_zero():
    """Error/control responses → CAS_ZERO."""
    # DELETE miss → CAS_ZERO
    s = conn()
    s.sendall(build_req(OP_DELETE, opaque=611, key=tkey("cas_no2")))
    d = recv_min(s, HDR)
    s.close()
    r2, _ = parse_resp(d)
    chk("cas_delete_miss_zero",
        r2 and r2["cas"] == CAS_ZERO,
        f"DEL miss cas={r2 and r2['cas']}")

    # NOOP → CAS_ZERO
    s = conn()
    s.sendall(build_req(OP_NOOP, opaque=612))
    d = recv_min(s, HDR)
    s.close()
    r3, _ = parse_resp(d)
    chk("cas_noop_control_zero",
        r3 and r3["cas"] == CAS_ZERO,
        f"NOOP cas={r3 and r3['cas']}")

    # GET miss → CAS_ZERO (no more connection kill — HMGET works)
    s = conn()
    s.sendall(build_req(OP_GET, opaque=610, key=tkey("cas_no")))
    d = recv_min(s, HDR)
    s.close()
    r1, _ = parse_resp(d)
    chk("cas_get_miss_zero",
        r1 and r1["status"] == ST_NF and r1["cas"] == CAS_ZERO,
        f"GET miss: {r1}")


def test_cas_delete_success():
    """DELETE success → non-zero CAS."""
    s = conn()
    k = tkey("cas_d")
    s.sendall(build_req(OP_SET, opaque=620, extras=set_extras(),
                        key=k, value=b"x"))
    r0d = recv_min(s, HDR)
    r0, _ = parse_resp(r0d)
    chk("cas_del_set_prerequisite", r0 and r0["status"] == ST_OK,
        f"SET prerequisite: {r0}")
    if not r0 or r0["status"] != ST_OK:
        s.close()
        return
    s.sendall(build_req(OP_DELETE, opaque=621, key=k))
    d = recv_min(s, HDR)
    s.close()
    r, _ = parse_resp(d)
    chk("cas_delete_success_nonzero",
        r and r["status"] == ST_OK and r["cas"] > 0,
        f"CAS={r and r['cas']}")


# ── 9. SET/GET round-trip ──────────────────────────────────────────
def test_set_get_roundtrip():
    """SET then GET — values, flags, CAS round-trip correctly."""
    s = conn()
    k = tkey("rt")
    val = b"hello world"
    FLAGS = 0xDEAD
    s.sendall(build_req(OP_SET, opaque=700, extras=set_extras(flags=FLAGS),
                        key=k, value=val))
    rd = recv_min(s, HDR)
    r1, _ = parse_resp(rd)

    rk = redis_key(k)
    exists = redis_cli("EXISTS", rk)
    chk("roundtrip_set_ok",
        r1 and r1["status"] == ST_OK and exists == "1",
        f"SET status={r1 and r1['status']}, EXISTS={exists}")
    set_cas = r1["cas"] if r1 else 0

    # GET
    s.sendall(build_req(OP_GET, opaque=701, key=k))
    gd = recv_min(s, HDR + 4 + len(val) + 10)
    s.close()
    r2, _ = parse_resp(gd)
    chk("roundtrip_get_ok", r2 and r2["status"] == ST_OK,
        f"GET: {r2}")
    if r2 and r2["status"] == ST_OK:
        # Extras = 4 bytes flags
        got_extras = r2["body"][:r2["extras_len"]]
        got_val = r2["body"][r2["extras_len"]:]
        got_flags = struct.unpack(">I", got_extras)[0] if len(got_extras) == 4 else None
        chk("roundtrip_value_match", got_val == val,
            f"expected {val!r}, got {got_val!r}")
        chk("roundtrip_flags_match", got_flags == FLAGS,
            f"expected 0x{FLAGS:04x}, got {got_flags}")
        chk("roundtrip_cas_match", r2["cas"] == set_cas,
            f"GET CAS={r2['cas']}, SET CAS={set_cas}")


# ── 10. CAS-conditional SET ──────────────────────────────────────
def test_cas_conditional_set():
    """CAS-conditional SET: correct CAS succeeds, wrong CAS returns ST_IX."""
    s = conn()
    k = tkey("cas_cond")
    # Initial SET
    s.sendall(build_req(OP_SET, opaque=800, extras=set_extras(),
                        key=k, value=b"v1"))
    rd = recv_min(s, HDR)
    r1, _ = parse_resp(rd)
    chk("cas_cond_initial_set", r1 and r1["status"] == ST_OK, f"{r1}")
    real_cas = r1["cas"] if r1 else 0

    # SET with correct CAS → should succeed
    s.sendall(build_req(OP_SET, opaque=801, cas=real_cas,
                        extras=set_extras(), key=k, value=b"v2"))
    rd = recv_min(s, HDR)
    r2, _ = parse_resp(rd)
    chk("cas_cond_correct_cas_ok",
        r2 and r2["status"] == ST_OK and r2["cas"] > real_cas,
        f"status={r2 and r2['status']}, cas={r2 and r2['cas']}")

    # SET with stale CAS (the one from first SET) → should fail
    s.sendall(build_req(OP_SET, opaque=802, cas=real_cas,
                        extras=set_extras(), key=k, value=b"v3"))
    rd = recv_min(s, HDR)
    r3, _ = parse_resp(rd)
    chk("cas_cond_stale_cas_ix",
        r3 and r3["status"] == ST_IX,
        f"expected ST_IX, got {r3}")
    s.close()


# ── 11. INCR / DECR counter operations ───────────────────────────
def test_incr_decr():
    """INCR/DECR with initial values, delta, and underflow clamping."""
    s = conn()
    k = tkey("ctr")

    # INCR on non-existent key with initial=100, delta=1
    s.sendall(build_req(OP_INCR, opaque=900,
                        extras=incr_extras(delta=1, initial=100, expiry=0),
                        key=k))
    rd = recv_min(s, HDR + 8)
    r1, _ = parse_resp(rd)
    chk("incr_initial", r1 and r1["status"] == ST_OK, f"{r1}")
    if r1 and r1["status"] == ST_OK:
        body = r1["body"]
        val = struct.unpack(">Q", body)[0] if len(body) == 8 else None
        chk("incr_initial_value", val == 100,
            f"expected 100, got {val}")

    # INCR existing key with delta=5
    s.sendall(build_req(OP_INCR, opaque=901,
                        extras=incr_extras(delta=5, initial=0, expiry=0),
                        key=k))
    rd = recv_min(s, HDR + 8)
    r2, _ = parse_resp(rd)
    if r2 and r2["status"] == ST_OK:
        val = struct.unpack(">Q", r2["body"])[0] if len(r2["body"]) == 8 else None
        chk("incr_delta", val == 105, f"expected 105, got {val}")

    # DECR with delta=200 (underflow → clamp to 0)
    s.sendall(build_req(OP_DECR, opaque=902,
                        extras=incr_extras(delta=200, initial=0, expiry=0),
                        key=k))
    rd = recv_min(s, HDR + 8)
    r3, _ = parse_resp(rd)
    if r3 and r3["status"] == ST_OK:
        val = struct.unpack(">Q", r3["body"])[0] if len(r3["body"]) == 8 else None
        chk("decr_underflow_clamp", val == 0, f"expected 0, got {val}")

    # INCR with 0xFFFFFFFF expiry on missing key → NOT_FOUND
    k2 = tkey("ctr_miss")
    s.sendall(build_req(OP_INCR, opaque=903,
                        extras=incr_extras(delta=1, initial=0, expiry=0xFFFFFFFF),
                        key=k2))
    rd = recv_min(s, HDR)
    r4, _ = parse_resp(rd)
    chk("incr_miss_0xFFFFFFFF",
        r4 and r4["status"] == ST_NF,
        f"expected ST_NF, got {r4}")
    s.close()


# ── 12. Binary-safe value round-trip ──────────────────────────────
def test_binary_value_roundtrip():
    """SET/GET round-trips arbitrary non-UTF8 binary data."""
    s = conn()
    k = tkey("binval")
    # Non-UTF8 payload: NUL, 'A', 0xFF, 'B'
    binary_val = b"\x00\x41\xFF\x42"
    FLAGS = 0x12345678
    s.sendall(build_req(OP_SET, opaque=1000, extras=set_extras(flags=FLAGS),
                        key=k, value=binary_val))
    rd = recv_min(s, HDR)
    r1, _ = parse_resp(rd)
    chk("binary_set_ok", r1 and r1["status"] == ST_OK, f"SET: {r1}")
    set_cas = r1["cas"] if r1 else 0

    # GET the same key
    s.sendall(build_req(OP_GET, opaque=1001, key=k))
    gd = recv_min(s, HDR + 4 + len(binary_val) + 20)
    s.close()
    r2, _ = parse_resp(gd)
    chk("binary_get_ok", r2 and r2["status"] == ST_OK,
        f"GET: {r2}")
    if r2 and r2["status"] == ST_OK:
        got_extras = r2["body"][:r2["extras_len"]]
        got_val = r2["body"][r2["extras_len"]:]
        got_flags = struct.unpack(">I", got_extras)[0] if len(got_extras) == 4 else None
        chk("binary_value_match", got_val == binary_val,
            f"expected {binary_val!r}, got {got_val!r}")
        chk("binary_flags_match", got_flags == FLAGS,
            f"expected 0x{FLAGS:08x}, got {got_flags!r}")
        chk("binary_cas_match", r2["cas"] == set_cas,
            f"GET CAS={r2['cas']}, SET CAS={set_cas}")


# ═════════════════════════════════════════════════════════════════
# Runner
# ═════════════════════════════════════════════════════════════════
ALL_TESTS = [
    # Preflight: verify hash-per-item backing store
    test_backing_store,
    # Protocol framing (no data-path dependency)
    test_noop,
    test_bad_magic,
    test_malformed_frame,
    test_unknown_opcode,
    # Quiet suppression (now backed by real data path)
    test_quiet_setq_suppressed,
    test_quiet_addq_error_preserved,
    test_quiet_deleteq,
    # CAS semantics (real per-item CAS)
    test_cas_set_success,
    test_cas_errors_zero,
    test_cas_delete_success,
    # Data path round-trip
    test_set_get_roundtrip,
    # CAS-conditional operations
    test_cas_conditional_set,
    # Counter operations
    test_incr_decr,
    # Binary-safe data path
    test_binary_value_roundtrip,
]

if __name__ == "__main__":
    print(f"RedCouch binary-protocol E2E tests ({HOST}:{PORT})")
    print(f"Data model: hash-per-item (no JSON dependency)")
    print(f"{'=' * 60}")
    for fn in ALL_TESTS:
        print(f"\n▸ {fn.__name__}")
        try:
            fn()
        except Exception as e:
            chk(fn.__name__, False, f"EXCEPTION: {e}")
    print(f"\n{'=' * 60}")
    passed = sum(1 for _, ok, _ in results if ok)
    failed = sum(1 for _, ok, _ in results if not ok)
    print(f"Results: {passed} passed, {failed} failed, {len(results)} total")
    if known_gaps:
        print(f"Known gaps: {len(known_gaps)}")
        for name, detail in known_gaps:
            print(f"  ⚠ {name}: {detail}")
    if failed:
        print("\nFailed tests:")
        for name, ok, detail in results:
            if not ok:
                print(f"  ✗ {name}: {detail}")
    sys.exit(1 if failed else 0)
