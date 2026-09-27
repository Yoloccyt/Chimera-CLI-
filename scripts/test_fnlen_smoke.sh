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
# Exit code: 0 = pass, 1 = assertion failed (a real red verdict), 2 = undecidable
#          (interpreter missing / the audited tool itself crashed -- the check
#           never ran, so it must not borrow 1 to impersonate "the red line broke".
#           2026-09-21 F39b: previously a missing interpreter exited 1, so every
#           host without python3 read it as "the >200-line rule regressed".)
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
    echo "[UNDECIDABLE] no python3/python interpreter found - gate did not judge" >&2
    exit 2
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
# A non-zero exit from the tool under test means the TOOL died, not that the
# function-length rule passed. Both tools are pure reporters (no sys.exit/exit()
# anywhere in their source), so non-zero can only be a crash => exit 2, never
# borrow 1 to impersonate a red verdict.
out="$("$py" scripts/audit_fnlen.py "$tmp")" || {
    echo "[UNDECIDABLE] audit_fnlen.py exited non-zero (tool crash, not a verdict)" >&2
    exit 2
}
if ! grep -q "fn oversized" <<<"$out"; then
    fail="$fail
[FAIL] audit_fnlen.py missed the oversized fn (fixture $tmp/bad/oversized.rs)"
fi
if grep -q "fn small" <<<"$out"; then
    fail="$fail
[FAIL] audit_fnlen.py false-positived the clean fn (fixture $tmp/ok/clean.rs)"
fi

# --- fn_scan.py: coarse line-gap scan -----------------------------------------
out2="$("$py" scripts/fn_scan.py "$tmp")" || {
    echo "[UNDECIDABLE] fn_scan.py exited non-zero (tool crash, not a verdict)" >&2
    exit 2
}
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

# --- the ratchet itself: "single fn <= 200 lines" as a judgement ---------------
# WHY the verdict lives here and not under a new gate id: this script is ALREADY
# wired into CI (ci.yml: "Fn-length tool smoke") and into the manifest as G-48, so
# the judgement inherits CI teeth without minting an unwired gate. rc: 0 honoured,
# 1 the red line moved the wrong way, 2 register unreadable (never borrow 1).
# HARD ORDERING PRECONDITION: scripts/fnlen_baseline.txt must be committed WITH the
# --check code path in audit_fnlen.py; a missing register exits 2 on purpose, so an
# incomplete commit turns this gate red instead of silently skipping the rule.
rc=0
"$py" scripts/audit_fnlen.py --check || rc=$?
if [ "$rc" -eq 1 ]; then
    echo "[FAIL] fn-length ratchet broken ([NEW]/[GROWTH]/[STALE] lines above)" >&2
    exit 1
elif [ "$rc" -gt 1 ]; then
    echo "[UNDECIDABLE] audit_fnlen.py --check could not judge (rc=$rc)" >&2
    exit 2
fi

# Teeth: a ratchet that cannot go red is decoration. Plant a register row for a
# function that does not exist -- the same --check must answer 1 with [STALE].
base="$tmp/baseline_with_ghost.txt"
cp scripts/fnlen_baseline.txt "$base"
printf 'crates/phantom/src/ghost.rs::gone_fn|999  # planted STALE control\n' >> "$base"
rc=0
"$py" scripts/audit_fnlen.py --check --baseline "$base" >"$tmp/ghost.out" 2>&1 || rc=$?
if [ "$rc" -ne 1 ] || ! grep -q '\[STALE\]' "$tmp/ghost.out"; then
    echo "[FAIL] ratchet has no teeth: planted ghost row was not judged red (rc=$rc)" >&2
    cat "$tmp/ghost.out" >&2
    exit 1
fi

# Teeth 3: an EMPTY SCAN SURFACE must also be UNDECIDABLE (2). A mistyped root -- or
# the register path passed positionally, which is exactly how this tool was mis-run
# once -- makes os.walk yield zero .rs files. "0 offenders" from 0 files scanned is
# not a verdict; before this guard it posed as 10 [STALE] reds (looks judged, saw none).
rc=0
"$py" scripts/audit_fnlen.py --check scripts/fnlen_baseline.txt >"$tmp/emptyscan.out" 2>&1 || rc=$?
if [ "$rc" -ne 2 ] || ! grep -q '\[UNDECIDABLE\] scanned 0' "$tmp/emptyscan.out"; then
    echo "[FAIL] empty scan surface gave rc=$rc, expected 2 with [UNDECIDABLE]" >&2
    cat "$tmp/emptyscan.out" >&2
    exit 1
fi

# Teeth 2: an allowance without a reason must be UNDECIDABLE (2), not a silent pass
# and not a borrowed 1 -- that is how registers quietly become dumping grounds.
grep -v '^crates/osa-coordinator' scripts/fnlen_baseline.txt > "$tmp/base_noreason.txt"
printf 'crates/x/src/y.rs::f|300\n' >> "$tmp/base_noreason.txt"
rc=0
"$py" scripts/audit_fnlen.py --check --baseline "$tmp/base_noreason.txt" >/dev/null 2>&1 || rc=$?
if [ "$rc" -ne 2 ]; then
    echo "[FAIL] reason-less register row gave rc=$rc, expected 2 (undecidable)" >&2
    exit 1
fi

echo "[OK] fnlen smoke + ratchet: tools detect >200-line fn (no false positive);" \
     "register honoured; teeth proven (STALE -> 1, reason-less -> 2)"

