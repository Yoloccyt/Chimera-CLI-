#!/usr/bin/env bash
# =============================================================================
# test_fnlen_smoke.sh - Minimal smoke test for the function-length audit scripts
# =============================================================================
# Purpose: keep scripts/audit_fnlen.py + scripts/fn_scan.py executable in clean
#          CI environments (ubuntu-latest ships python3). Builds a temporary
#          fixture with one deliberately oversized (>200-line) function and one
#          clean file, then asserts both scripts detect only the oversized one.
#          Guards the "single function <= 200 lines" red line (nuxus-rules 6.1)
#          against silent tool regression.
# Usage:
#   bash scripts/test_fnlen_smoke.sh
# Exit code: 0 = pass, 1 = any assertion failed (or python missing)
# Encoding: all-ASCII to avoid CJK path/locale issues (project script convention)
# =============================================================================
set -euo pipefail

# Ensure coreutils are reachable even when the parent process (e.g. PowerShell
# on Windows/MSYS) passes a PATH without /usr/bin. Harmless no-op on Linux/macOS.
export PATH="/usr/bin:/bin:$PATH"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# Locate a python interpreter (ubuntu-latest: python3; MSYS may expose python).
py=""
if command -v python3 >/dev/null 2>&1; then
    py="python3"
elif command -v python >/dev/null 2>&1; then
    py="python"
else
    echo "[FAIL] no python3/python interpreter found" >&2
    exit 1
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/ok" "$tmp/bad"

# Fixture 1: clean file with a small function - must never be reported.
cat > "$tmp/ok/clean.rs" <<'EOF'
pub fn small(a: u32) -> u32 {
    a + 1
}
EOF

# Fixture 2: one function spanning > 200 real code lines - must be reported.
# Body lines use real statements (no comment-only padding) so the brace-balance
# scanner in audit_fnlen.py counts them after comment/string sanitization.
{
    echo "pub fn oversized() -> u32 {"
    for ((i = 1; i <= 250; i++)); do
        echo "    let v$i = $i;"
    done
    echo "    v250"
    echo "}"
} > "$tmp/bad/oversized.rs"

fail=""

# --- audit_fnlen.py: precise brace-balance scan ------------------------------
out="$("$py" scripts/audit_fnlen.py "$tmp")"
if ! grep -q "fn oversized" <<<"$out"; then
    fail="$fail
[FAIL] audit_fnlen.py missed the oversized fn (fixture $tmp/bad/oversized.rs)"
fi
if grep -q "fn small" <<<"$out"; then
    fail="$fail
[FAIL] audit_fnlen.py false-positived the clean fn (fixture $tmp/ok/clean.rs)"
fi

# --- fn_scan.py: coarse line-gap scan -----------------------------------------
out2="$("$py" scripts/fn_scan.py "$tmp")"
if ! grep -q "fn oversized" <<<"$out2"; then
    fail="$fail
[FAIL] fn_scan.py missed the oversized fn (fixture $tmp/bad/oversized.rs)"
fi

if [ -n "$fail" ]; then
    echo "$fail" >&2
    echo "--- audit_fnlen.py output ---" >&2
    echo "$out" >&2
    echo "--- fn_scan.py output ---" >&2
    echo "$out2" >&2
    exit 1
fi

echo "[OK] fnlen smoke test pass (audit_fnlen.py + fn_scan.py detect >200-line fn, no false positive)"
