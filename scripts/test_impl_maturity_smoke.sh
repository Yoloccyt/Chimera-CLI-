#!/usr/bin/env bash
# =============================================================================
# test_impl_maturity_smoke.sh -- proves check_impl_maturity_claims.py has teeth
# =============================================================================
# WHY this exists (repo lesson, agents.md / gate-trust batch): a gate that has
# never been observed to FAIL is only assumed to work. `--selftest` covers the
# classifier and the verdict arithmetic, but not the end-to-end path
# (walk crates/ -> extract comments -> compare against the baseline file ->
# choose an exit code). This script builds a throwaway repository skeleton in
# $(mktemp -d) and drives the REAL gate against it via --root, so nothing is
# ever written into the working tree (which currently carries uncommitted work).
#
# Four assertions, each one the opposite polarity of a possible silent failure:
#   1. an unregistered marker            -> rc=1  (the gate can see a new fake)
#   2. the same marker, registered        -> rc=0  (baseline honoured, no noise)
#   3. a registered marker that vanished  -> rc=1  (allowance must not outlive debt)
#   4. a malformed baseline row           -> rc=2  (config error, never a fake red)
# Exit: 0 all four hold, 1 any assertion fails.
# =============================================================================
set -u
cd "$(dirname "$0")/.." || exit 1
GATE="scripts/check_impl_maturity_claims.py"

if ! command -v python >/dev/null 2>&1; then
  echo "[ENV-FAIL] python not on PATH"
  exit 2
fi

WORK=$(mktemp -d 2>/dev/null) || { echo "[ENV-FAIL] mktemp failed"; exit 2; }
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$WORK/crates/demo/src" "$WORK/scripts" || exit 1
cat > "$WORK/crates/demo/src/lib.rs" <<'RS'
//! demo crate for gate smoke testing
//! MOCK-ONLY: real backend lands later
#![forbid(unsafe_code)]
pub fn f() {}
RS

fail=0
run() { # $1 = expected rc, $2 = label, $3 = substring that MUST appear in output
  local want="$1" label="$2" needle="$3" rc out
  out=$(python "$GATE" --root "$WORK" 2>&1); rc=$?
  # The substring check is not decoration: an earlier revision of this file had
  # a gate bug that made assertion 4 return rc=2 for the WRONG reason (missing
  # file, not malformed row). rc alone cannot tell those apart; the message can.
  if [ "$rc" != "$want" ]; then
    echo "  [FAIL] $label: rc=$rc want=$want"
    echo "$out" | sed 's/^/         /'
    fail=1
  elif [ -n "$needle" ] && ! printf '%s' "$out" | grep -q "$needle"; then
    echo "  [FAIL] $label: rc=$rc but output lacks '$needle' (right code, wrong cause)"
    echo "$out" | sed 's/^/         /'
    fail=1
  else
    echo "  [PASS] $label -> rc=$rc"
  fi
}

printf '%s\n' "impl_maturity gate smoke (synthetic tree, real scan path):"

# 1. marker present, no baseline row yet -> must be RED
: > "$WORK/scripts/impl_maturity_baseline.txt"
run 1 "unregistered MOCK-ONLY is a GAP" "\[GAP\] demo.MOCK-ONLY = 1 (allowed 0)"

# 2. register it -> must be GREEN
printf 'demo.MOCK-ONLY=1  # smoke fixture\n' > "$WORK/scripts/impl_maturity_baseline.txt"
run 0 "registered marker is honoured" ""

# 3. marker removed from code while the row stays -> must be RED (STALE)
cat > "$WORK/crates/demo/src/lib.rs" <<'RS'
//! demo crate, implementation now real
#![forbid(unsafe_code)]
pub fn f() {}
RS
run 1 "a vacated allowance is STALE" "\[STALE\] demo.MOCK-ONLY"

# 4. malformed row -> must be UNDECIDABLE (2), never a silent 0 or a faked 1
printf 'demo.MOCK-ONLY = one\n' > "$WORK/scripts/impl_maturity_baseline.txt"
run 2 "malformed baseline is a config error" "needs .crate.LABEL=<count>"

if [ "$fail" = 0 ]; then
  echo "[OK] all four ratchet behaviours observed on the real scan path"
  exit 0
fi
echo "[FAIL] at least one assertion did not hold -- the gate is not trustworthy"
exit 1
