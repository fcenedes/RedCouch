#!/usr/bin/env bash
# RedCouch ASCII protocol session example.
#
# Demonstrates basic memcached ASCII commands using netcat.
# Requires: Redis 8+ with RedCouch loaded on 127.0.0.1:11210
#
# Usage:
#   bash examples/shell/ascii_session.sh

set -euo pipefail

HOST="127.0.0.1"
PORT=11210

echo "=== RedCouch ASCII Protocol Example (netcat) ==="
echo ""

# Helper: send a command and read the response
send_cmd() {
    local cmd="$1"
    echo ">> $cmd"
    echo -e "${cmd}\r" | nc -q 1 "$HOST" "$PORT" 2>/dev/null || \
    echo -e "${cmd}\r" | nc -w 1 "$HOST" "$PORT" 2>/dev/null
    echo ""
}

# Helper: send a storage command with data block
send_store() {
    local cmd="$1"
    local data="$2"
    echo ">> $cmd"
    echo ">> $data"
    printf "%s\r\n%s\r\n" "$cmd" "$data" | nc -q 1 "$HOST" "$PORT" 2>/dev/null || \
    printf "%s\r\n%s\r\n" "$cmd" "$data" | nc -w 1 "$HOST" "$PORT" 2>/dev/null
    echo ""
}

echo "1. Version check"
send_cmd "version"

echo "2. Store a value (set)"
send_store "set demo:shell 0 60 11" "hello-shell"

echo "3. Retrieve it (get)"
send_cmd "get demo:shell"

echo "4. Store a counter"
send_store "set demo:counter 0 0 1" "0"

echo "5. Increment counter"
send_cmd "incr demo:counter 5"

echo "6. Decrement counter"
send_cmd "decr demo:counter 2"

echo "7. Delete keys"
send_cmd "delete demo:shell"
send_cmd "delete demo:counter"

echo "8. Meta protocol set + get"
send_store "ms demo:meta 5" "hello"

echo "9. Meta get with value and CAS"
send_cmd "mg demo:meta v c"

echo "10. Meta delete"
send_cmd "md demo:meta"

echo "=== Done! ==="
