#!/usr/bin/env bash
set -euo pipefail

# spall Wave 1.5 E2E smoke test
# Usage: ./e2e/smoke.sh [path/to/spall-binary]
# Requires: petstore.json fixture at spall-core/tests/fixtures/petstore.json

SPALL="${1:-./target/release/spall}"
FIXTURE="$(cd "$(dirname "$0")/.." && pwd)/spall-core/tests/fixtures/petstore.json"

if [[ ! -x "$SPALL" ]]; then
    echo "ERROR: spall binary not found or not executable: $SPALL"
    echo "Build with: cargo build --release"
    exit 1
fi

if [[ ! -f "$FIXTURE" ]]; then
    echo "ERROR: petstore fixture missing: $FIXTURE"
    echo "Download with:"
    echo "  curl -sL 'https://petstore3.swagger.io/api/v3/openapi.json' > '$FIXTURE'"
    exit 1
fi

export XDG_CONFIG_HOME=/tmp/spall-e2e-smoke/config
export XDG_CACHE_HOME=/tmp/spall-e2e-smoke/cache
rm -rf "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME"

CACHE_DIR="$XDG_CACHE_HOME/spall"
PASS=0
FAIL=0

assert() {
    local msg="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "  PASS: $msg"
        PASS=$((PASS + 1))
    else
        echo "  FAIL: $msg"
        FAIL=$((FAIL + 1))
    fi
}

# Run command, capture stderr to file
run_stderr() {
    local out="$1"
    shift
    "$@" 2>"$out" >/dev/null || true
}

echo "=== 1. Register petstore API ==="
"$SPALL" api add petstore "$FIXTURE"
assert "api registered" test -d "$XDG_CONFIG_HOME/spall"

echo ""
echo "=== 2. Cache cold run ==="
STDERR=/tmp/spall-smoke-stderr-2
run_stderr "$STDERR" "$SPALL" petstore getpetbyid 1 --spall-dry-run
assert "no postcard warnings on cold run" bash -c '! grep -q "Warning:" "'"$STDERR"'"'
assert "no postcard WontImplement on cold run" bash -c '! grep -q "WontImplement" "'"$STDERR"'"'
assert "cache .ir written" test -f "$CACHE_DIR"/*.ir
assert "cache .idx written" test -f "$CACHE_DIR"/*.idx
assert "cache .meta written" test -f "$CACHE_DIR"/*.meta

echo ""
echo "=== 3. Cache warm run ==="
STDERR=/tmp/spall-smoke-stderr-3
run_stderr "$STDERR" "$SPALL" petstore getpetbyid 1 --spall-dry-run
assert "no Warning on warm run" bash -c '! grep -q "Warning:" "'"$STDERR"'"'
assert "dry-run message present" grep -q "Dry run:" "$STDERR"

echo ""
echo "=== 4. Server URL override ==="
STDERR=/tmp/spall-smoke-stderr-4
run_stderr "$STDERR" "$SPALL" petstore getpetbyid 1 \
    --spall-server "https://mock.example.com" \
    --spall-dry-run
assert "override URL in dry-run" grep -q "https://mock.example.com/pet/1" "$STDERR"

echo ""
echo "=== 5. Degraded --help when spec missing ==="
# Prime cache first
run_stderr /dev/null "$SPALL" petstore getpetbyid 1 --spall-dry-run
mv "$FIXTURE" /tmp/petstore-bak.json
HELP_OUT=/tmp/spall-smoke-help-5
STDERR=/tmp/spall-smoke-stderr-5
"$SPALL" petstore --help >"$HELP_OUT" 2>"$STDERR" || true
mv /tmp/petstore-bak.json "$FIXTURE"
assert "degraded help shows stale warning" grep -q "⚠" "$STDERR"
assert "degraded help shows API title" grep -q "Swagger Petstore" "$HELP_OUT"
assert "degraded help shows operation list" grep -q "getpetbyid" "$HELP_OUT"

echo ""
echo "=== 6. Output mode (pretty TTY vs raw piped) ==="
# TTY path: just verify no panic on dry-run
run_stderr /dev/null "$SPALL" petstore getpetbyid 1 --spall-dry-run
assert "TTY dry-run succeeds" test $? -eq 0
# Piped path: same command piped through cat
run_stderr /dev/null sh -c '"'"$SPALL"' petstore getpetbyid 1 --spall-dry-run | cat'
assert "piped dry-run succeeds" test $? -eq 0

echo ""
echo "=== Summary ==="
echo "$PASS passed, $FAIL failed"

if [[ $FAIL -gt 0 ]]; then
    exit 1
fi
