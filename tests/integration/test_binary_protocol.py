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
OP_GET, OP_SET, OP_ADD, OP_REPLACE, OP_DELETE = 0x00, 0x01, 0x02, 0x03, 0x04
OP_INCR, OP_DECR = 0x05, 0x06
OP_FLUSH = 0x08
OP_NOOP, OP_VERSION, OP_GETK = 0x0A, 0x0B, 0x0C
OP_APPEND, OP_PREPEND = 0x0E, 0x0F
OP_SETQ, OP_ADDQ, OP_DELETEQ = 0x11, 0x12, 0x14
OP_APPENDQ, OP_PREPENDQ = 0x19, 0x1A
OP_STAT = 0x10
OP_VERBOSITY = 0x1B
OP_TOUCH, OP_GAT, OP_GATQ = 0x1C, 0x1D, 0x1E
OP_SASL_LIST_MECHS, OP_SASL_AUTH, OP_SASL_STEP = 0x20, 0x21, 0x22
ST_OK, ST_NF, ST_IX, ST_ARGS, ST_NOT_STORED, ST_UNK = 0, 1, 2, 4, 5, 0x81
ST_AUTH_ERROR = 0x20
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

def touch_extras(expiry=0):
    return struct.pack(">I", expiry)

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


# ── 13. TOUCH operation ──────────────────────────────────────────
def test_touch():
    """TOUCH on existing key succeeds; TOUCH on missing key → ST_NF."""
    s = conn()
    k = tkey("touch1")
    # Create key
    s.sendall(build_req(OP_SET, opaque=1100, extras=set_extras(),
                        key=k, value=b"touchval"))
    rd = recv_min(s, HDR)
    r0, _ = parse_resp(rd)
    chk("touch_set_prerequisite", r0 and r0["status"] == ST_OK, f"{r0}")

    # TOUCH existing key with 10s TTL
    s.sendall(build_req(OP_TOUCH, opaque=1101, extras=touch_extras(expiry=10),
                        key=k))
    rd = recv_min(s, HDR)
    r1, _ = parse_resp(rd)
    chk("touch_existing_ok", r1 and r1["status"] == ST_OK,
        f"expected ST_OK, got {r1}")
    chk("touch_returns_cas", r1 and r1["cas"] > 0,
        f"cas={r1 and r1['cas']}")

    # Verify TTL was set via redis-cli
    rk = redis_key(k)
    ttl = redis_cli("TTL", rk)
    chk("touch_sets_ttl", ttl and int(ttl) > 0,
        f"expected TTL > 0, got {ttl}")

    # TOUCH missing key → ST_NF
    k2 = tkey("touch_miss")
    s.sendall(build_req(OP_TOUCH, opaque=1102, extras=touch_extras(expiry=10),
                        key=k2))
    rd = recv_min(s, HDR)
    r2, _ = parse_resp(rd)
    chk("touch_missing_nf", r2 and r2["status"] == ST_NF,
        f"expected ST_NF, got {r2}")
    s.close()


# ── 14. GAT (Get And Touch) ─────────────────────────────────────
def test_gat():
    """GAT on existing key returns value and updates TTL; miss → ST_NF."""
    s = conn()
    k = tkey("gat1")
    FLAGS = 0xBEEF
    # Create key
    s.sendall(build_req(OP_SET, opaque=1200, extras=set_extras(flags=FLAGS),
                        key=k, value=b"gatval"))
    rd = recv_min(s, HDR)
    r0, _ = parse_resp(rd)
    chk("gat_set_prerequisite", r0 and r0["status"] == ST_OK, f"{r0}")

    # GAT with 15s TTL
    s.sendall(build_req(OP_GAT, opaque=1201, extras=touch_extras(expiry=15),
                        key=k))
    rd = recv_min(s, HDR + 100)
    r1, _ = parse_resp(rd)
    chk("gat_existing_ok", r1 and r1["status"] == ST_OK,
        f"expected ST_OK, got {r1}")
    if r1 and r1["status"] == ST_OK:
        got_extras = r1["body"][:r1["extras_len"]]
        # GAT includes key in response
        key_start = r1["extras_len"]
        key_end = key_start + r1["key_len"]
        got_key = r1["body"][key_start:key_end]
        got_val = r1["body"][key_end:]
        got_flags = struct.unpack(">I", got_extras)[0] if len(got_extras) == 4 else None
        chk("gat_returns_value", got_val == b"gatval",
            f"expected b'gatval', got {got_val!r}")
        chk("gat_returns_flags", got_flags == FLAGS,
            f"expected 0x{FLAGS:04x}, got {got_flags}")
        chk("gat_returns_key", got_key == k,
            f"expected {k!r}, got {got_key!r}")
        chk("gat_returns_cas", r1["cas"] > 0,
            f"cas={r1['cas']}")

    # Verify TTL
    rk = redis_key(k)
    ttl = redis_cli("TTL", rk)
    chk("gat_updates_ttl", ttl and int(ttl) > 0,
        f"expected TTL > 0, got {ttl}")

    # GAT miss → ST_NF
    k2 = tkey("gat_miss")
    s.sendall(build_req(OP_GAT, opaque=1202, extras=touch_extras(expiry=10),
                        key=k2))
    rd = recv_min(s, HDR)
    r2, _ = parse_resp(rd)
    chk("gat_missing_nf", r2 and r2["status"] == ST_NF,
        f"expected ST_NF, got {r2}")
    s.close()


# ── 15. APPEND / PREPEND ────────────────────────────────────────
def test_append_prepend():
    """APPEND/PREPEND on existing key; NOT_STORED on missing."""
    s = conn()
    k = tkey("ap1")
    # Create key with initial value
    s.sendall(build_req(OP_SET, opaque=1300, extras=set_extras(),
                        key=k, value=b"hello"))
    rd = recv_min(s, HDR)
    r0, _ = parse_resp(rd)
    chk("append_set_prerequisite", r0 and r0["status"] == ST_OK, f"{r0}")

    # APPEND " world"
    s.sendall(build_req(OP_APPEND, opaque=1301, key=k, value=b" world"))
    rd = recv_min(s, HDR)
    r1, _ = parse_resp(rd)
    chk("append_ok", r1 and r1["status"] == ST_OK,
        f"expected ST_OK, got {r1}")
    chk("append_returns_cas", r1 and r1["cas"] > 0,
        f"cas={r1 and r1['cas']}")

    # Verify via GET
    s.sendall(build_req(OP_GET, opaque=1302, key=k))
    rd = recv_min(s, HDR + 100)
    r2, _ = parse_resp(rd)
    if r2 and r2["status"] == ST_OK:
        got_val = r2["body"][r2["extras_len"]:]
        chk("append_value_correct", got_val == b"hello world",
            f"expected b'hello world', got {got_val!r}")

    # PREPEND "say "
    s.sendall(build_req(OP_PREPEND, opaque=1303, key=k, value=b"say "))
    rd = recv_min(s, HDR)
    r3, _ = parse_resp(rd)
    chk("prepend_ok", r3 and r3["status"] == ST_OK,
        f"expected ST_OK, got {r3}")

    # Verify via GET
    s.sendall(build_req(OP_GET, opaque=1304, key=k))
    rd = recv_min(s, HDR + 100)
    r4, _ = parse_resp(rd)
    if r4 and r4["status"] == ST_OK:
        got_val = r4["body"][r4["extras_len"]:]
        chk("prepend_value_correct", got_val == b"say hello world",
            f"expected b'say hello world', got {got_val!r}")

    # APPEND on missing key → NOT_STORED
    k2 = tkey("ap_miss")
    s.sendall(build_req(OP_APPEND, opaque=1305, key=k2, value=b"x"))
    rd = recv_min(s, HDR)
    r5, _ = parse_resp(rd)
    chk("append_missing_not_stored",
        r5 and r5["status"] == ST_NOT_STORED,
        f"expected ST_NOT_STORED(5), got {r5}")

    # PREPEND on missing key → NOT_STORED
    s.sendall(build_req(OP_PREPEND, opaque=1306, key=k2, value=b"x"))
    rd = recv_min(s, HDR)
    r6, _ = parse_resp(rd)
    chk("prepend_missing_not_stored",
        r6 and r6["status"] == ST_NOT_STORED,
        f"expected ST_NOT_STORED(5), got {r6}")
    s.close()


# ── 16. VERSION ──────────────────────────────────────────────────
def test_version():
    """VERSION returns a version string."""
    s = conn()
    s.sendall(build_req(OP_VERSION, opaque=1400))
    rd = recv_min(s, HDR + 20)
    s.close()
    r, _ = parse_resp(rd)
    chk("version_ok", r and r["status"] == ST_OK,
        f"expected ST_OK, got {r}")
    if r and r["status"] == ST_OK:
        ver = r["body"].decode("utf-8", errors="replace")
        chk("version_string", "RedCouch" in ver,
            f"expected 'RedCouch' in version, got {ver!r}")


# ── 17. Expiry behavior ─────────────────────────────────────────
def test_expiry_set_ttl():
    """SET with TTL → key has TTL and eventually expires."""
    s = conn()

    # Test 1: SET with long TTL to verify TTL is set (no race with redis-cli)
    k_ttl = tkey("exp_ttl")
    s.sendall(build_req(OP_SET, opaque=1500, extras=set_extras(expiry=300),
                        key=k_ttl, value=b"expval_long"))
    rd = recv_min(s, HDR)
    r0, _ = parse_resp(rd)
    chk("expiry_set_ok", r0 and r0["status"] == ST_OK, f"{r0}")

    rk = redis_key(k_ttl)
    ttl = redis_cli("TTL", rk)
    chk("expiry_ttl_set", ttl and int(ttl) > 0,
        f"expected TTL > 0, got {ttl}")

    # Test 2: SET with short TTL to verify key actually expires
    k_exp = tkey("exp_short")
    s.sendall(build_req(OP_SET, opaque=1501, extras=set_extras(expiry=2),
                        key=k_exp, value=b"expval_short"))
    rd = recv_min(s, HDR)
    r1, _ = parse_resp(rd)
    chk("expiry_short_set_ok", r1 and r1["status"] == ST_OK, f"{r1}")

    # Immediately GET → should work
    s.sendall(build_req(OP_GET, opaque=1502, key=k_exp))
    rd = recv_min(s, HDR + 50)
    r2, _ = parse_resp(rd)
    chk("expiry_get_before", r2 and r2["status"] == ST_OK,
        f"expected ST_OK before expiry, got {r2}")

    # Wait for expiry
    time.sleep(3)

    # GET after expiry → should be NOT_FOUND
    s.sendall(build_req(OP_GET, opaque=1503, key=k_exp))
    rd = recv_min(s, HDR)
    r3, _ = parse_resp(rd)
    chk("expiry_get_after", r3 and r3["status"] == ST_NF,
        f"expected ST_NF after expiry, got {r3}")
    s.close()


# ── 18. Counter edge cases ──────────────────────────────────────
def test_counter_edge_cases():
    """Counter: INCR on non-numeric, DECR underflow, large values."""
    s = conn()

    # INCR on non-numeric stored value → ST_ARGS (non-numeric)
    k_nn = tkey("ctr_nn")
    s.sendall(build_req(OP_SET, opaque=1600, extras=set_extras(),
                        key=k_nn, value=b"notanumber"))
    rd = recv_min(s, HDR)
    r0, _ = parse_resp(rd)
    chk("counter_nn_set", r0 and r0["status"] == ST_OK, f"{r0}")

    s.sendall(build_req(OP_INCR, opaque=1601,
                        extras=incr_extras(delta=1, initial=0, expiry=0),
                        key=k_nn))
    rd = recv_min(s, HDR + 20)
    r1, _ = parse_resp(rd)
    chk("counter_non_numeric_error",
        r1 and r1["status"] == ST_ARGS,
        f"expected ST_ARGS for non-numeric, got {r1}")

    # INCR with large initial value
    k_lg = tkey("ctr_lg")
    large_init = 2**52  # safe within Lua double precision
    s.sendall(build_req(OP_INCR, opaque=1602,
                        extras=incr_extras(delta=1, initial=large_init, expiry=0),
                        key=k_lg))
    rd = recv_min(s, HDR + 8)
    r2, _ = parse_resp(rd)
    chk("counter_large_init_ok", r2 and r2["status"] == ST_OK, f"{r2}")
    if r2 and r2["status"] == ST_OK and len(r2["body"]) == 8:
        val = struct.unpack(">Q", r2["body"])[0]
        chk("counter_large_init_value", val == large_init,
            f"expected {large_init}, got {val}")

    # INCR existing large value by 1
    s.sendall(build_req(OP_INCR, opaque=1603,
                        extras=incr_extras(delta=1, initial=0, expiry=0),
                        key=k_lg))
    rd = recv_min(s, HDR + 8)
    r3, _ = parse_resp(rd)
    if r3 and r3["status"] == ST_OK and len(r3["body"]) == 8:
        val = struct.unpack(">Q", r3["body"])[0]
        chk("counter_large_incr", val == large_init + 1,
            f"expected {large_init + 1}, got {val}")

    # DECR to underflow (clamp to 0)
    k_uf = tkey("ctr_uf")
    s.sendall(build_req(OP_INCR, opaque=1604,
                        extras=incr_extras(delta=1, initial=5, expiry=0),
                        key=k_uf))
    rd = recv_min(s, HDR + 8)
    parse_resp(rd)  # consume

    s.sendall(build_req(OP_DECR, opaque=1605,
                        extras=incr_extras(delta=10, initial=0, expiry=0),
                        key=k_uf))
    rd = recv_min(s, HDR + 8)
    r4, _ = parse_resp(rd)
    if r4 and r4["status"] == ST_OK and len(r4["body"]) == 8:
        val = struct.unpack(">Q", r4["body"])[0]
        chk("counter_underflow_zero", val == 0,
            f"expected 0, got {val}")
    s.close()


# ── 19. FLUSH operation ─────────────────────────────────────────
def test_flush():
    """FLUSH deletes all rc: keys; confirms via GET miss."""
    s = conn()
    k1 = tkey("fl1")
    k2 = tkey("fl2")

    # Create two keys
    s.sendall(build_req(OP_SET, opaque=1700, extras=set_extras(),
                        key=k1, value=b"fv1"))
    rd = recv_min(s, HDR)
    parse_resp(rd)
    s.sendall(build_req(OP_SET, opaque=1701, extras=set_extras(),
                        key=k2, value=b"fv2"))
    rd = recv_min(s, HDR)
    parse_resp(rd)

    # FLUSH
    s.sendall(build_req(OP_FLUSH, opaque=1702))
    rd = recv_min(s, HDR)
    r1, _ = parse_resp(rd)
    chk("flush_ok", r1 and r1["status"] == ST_OK,
        f"expected ST_OK, got {r1}")

    # GET both keys → NOT_FOUND
    s.sendall(build_req(OP_GET, opaque=1703, key=k1))
    rd = recv_min(s, HDR)
    r2, _ = parse_resp(rd)
    chk("flush_key1_gone", r2 and r2["status"] == ST_NF,
        f"expected ST_NF after flush, got {r2}")

    s.sendall(build_req(OP_GET, opaque=1704, key=k2))
    rd = recv_min(s, HDR)
    r3, _ = parse_resp(rd)
    chk("flush_key2_gone", r3 and r3["status"] == ST_NF,
        f"expected ST_NF after flush, got {r3}")
    s.close()


# ── 20. SASL auth ──────────────────────────────────────────────
def test_sasl_list_mechs():
    """SASL_LIST_MECHS returns 'PLAIN'."""
    s = conn()
    s.sendall(build_req(OP_SASL_LIST_MECHS, opaque=2000))
    d = recv_min(s, HDR + 20)
    s.close()
    r, _ = parse_resp(d)
    chk("sasl_list_mechs_ok", r and r["status"] == ST_OK,
        f"expected ST_OK, got {r}")
    if r and r["status"] == ST_OK:
        body = r["body"].decode("utf-8", errors="replace")
        chk("sasl_list_mechs_plain", "PLAIN" in body,
            f"expected 'PLAIN' in body, got {body!r}")


def test_sasl_auth_plain():
    """SASL_AUTH with PLAIN mechanism succeeds (GA: permissive)."""
    s = conn()
    # PLAIN payload: \0username\0password
    plain_payload = b"\x00testuser\x00testpass"
    s.sendall(build_req(OP_SASL_AUTH, opaque=2010,
                        key=b"PLAIN", value=plain_payload))
    d = recv_min(s, HDR + 20)
    s.close()
    r, _ = parse_resp(d)
    chk("sasl_auth_plain_ok", r and r["status"] == ST_OK,
        f"expected ST_OK, got {r}")


def test_sasl_auth_unsupported_mech():
    """SASL_AUTH with unsupported mechanism → ST_AUTH_ERROR."""
    s = conn()
    s.sendall(build_req(OP_SASL_AUTH, opaque=2020,
                        key=b"SCRAM-SHA-1", value=b"data"))
    d = recv_min(s, HDR + 40)
    s.close()
    r, _ = parse_resp(d)
    chk("sasl_auth_bad_mech", r and r["status"] == ST_AUTH_ERROR,
        f"expected ST_AUTH_ERROR(0x20), got {r}")


def test_sasl_step():
    """SASL_STEP → ST_AUTH_ERROR (PLAIN is single-step)."""
    s = conn()
    s.sendall(build_req(OP_SASL_STEP, opaque=2030,
                        key=b"PLAIN", value=b"step_data"))
    d = recv_min(s, HDR + 60)
    s.close()
    r, _ = parse_resp(d)
    chk("sasl_step_error", r and r["status"] == ST_AUTH_ERROR,
        f"expected ST_AUTH_ERROR(0x20), got {r}")


# ── 21. STAT ───────────────────────────────────────────────────
def test_stat_general():
    """STAT with empty key returns general stats terminated by empty response."""
    s = conn()
    # First do a SET so we have at least 1 item
    k = tkey("stat_item")
    s.sendall(build_req(OP_SET, opaque=2100, extras=set_extras(),
                        key=k, value=b"statval"))
    rd = recv_min(s, HDR)
    parse_resp(rd)

    # STAT with empty key
    s.sendall(build_req(OP_STAT, opaque=2101))
    # Read enough for multiple stat responses
    d = recv_min(s, HDR * 30)
    s.close()

    # Parse all stat responses
    stats = {}
    off = 0
    while True:
        r, off = parse_resp(d, off)
        if r is None:
            break
        chk_once = r["status"] == ST_OK and r["opcode"] == OP_STAT
        if not chk_once:
            break
        key_start = r["extras_len"]
        key_end = key_start + r["key_len"]
        stat_key = r["body"][key_start:key_end].decode("utf-8", errors="replace")
        stat_val = r["body"][key_end:].decode("utf-8", errors="replace")
        if stat_key == "" and stat_val == "":
            break  # terminator
        stats[stat_key] = stat_val

    chk("stat_has_pid", "pid" in stats, f"stats keys: {list(stats.keys())}")
    chk("stat_has_version", "version" in stats, f"stats keys: {list(stats.keys())}")
    chk("stat_has_uptime", "uptime" in stats, f"stats keys: {list(stats.keys())}")
    chk("stat_has_curr_items", "curr_items" in stats,
        f"stats keys: {list(stats.keys())}")
    chk("stat_has_cmd_get", "cmd_get" in stats,
        f"stats keys: {list(stats.keys())}")
    chk("stat_has_cmd_set", "cmd_set" in stats,
        f"stats keys: {list(stats.keys())}")
    chk("stat_has_curr_connections", "curr_connections" in stats,
        f"stats keys: {list(stats.keys())}")
    if "curr_items" in stats:
        chk("stat_curr_items_positive", int(stats["curr_items"]) > 0,
            f"expected > 0, got {stats['curr_items']}")
    if "version" in stats:
        chk("stat_version_redcouch", "RedCouch" in stats["version"],
            f"expected 'RedCouch', got {stats['version']}")


def test_stat_unsupported_group():
    """STAT with unsupported group key → just terminator."""
    s = conn()
    s.sendall(build_req(OP_STAT, opaque=2200, key=b"slabs"))
    d = recv_min(s, HDR + 10)
    s.close()
    r, _ = parse_resp(d)
    chk("stat_unsupported_terminates",
        r and r["status"] == ST_OK and r["key_len"] == 0 and r["body_len"] == 0,
        f"expected empty terminator, got {r}")


# ── 22. VERBOSITY ─────────────────────────────────────────────
def test_verbosity():
    """VERBOSITY → ST_OK (no-op)."""
    s = conn()
    # Verbosity extras: 4 bytes for verbosity level
    verb_extras = struct.pack(">I", 2)
    s.sendall(build_req(OP_VERBOSITY, opaque=2300, extras=verb_extras))
    d = recv_min(s, HDR)
    s.close()
    r, _ = parse_resp(d)
    chk("verbosity_ok", r and r["status"] == ST_OK,
        f"expected ST_OK, got {r}")


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
    # New binary semantics
    test_touch,
    test_gat,
    test_append_prepend,
    test_version,
    test_expiry_set_ttl,
    test_counter_edge_cases,
    # Auth / Stats / Admin
    test_sasl_list_mechs,
    test_sasl_auth_plain,
    test_sasl_auth_unsupported_mech,
    test_sasl_step,
    test_stat_general,
    test_stat_unsupported_group,
    test_verbosity,
    # Flush (run last — destroys data)
    test_flush,
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
