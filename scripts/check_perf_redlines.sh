#!/usr/bin/env bash
# =============================================================================
# check_perf_redlines.sh - performance red-line + bench-inventory gate
# =============================================================================
# Purpose: THIN WRAPPER around scripts/check_perf_redlines.py. It contains no
#          rule data and no logic on purpose.
#
# WHY this file was rewritten (2026-08-31, R1-T4 / audit finding F-R4-01):
#   The previous generation of this gate existed twice -- a 429-line .ps1 and a
#   246-line .sh -- and CI called NEITHER (the only reference anywhere in
#   .github/ was a comment in bench_check.yml). Two hand-maintained copies of
#   one rule had already drifted:
#     * this .sh had no Part 3 inventory gate at all
#     * its criterion filter was 'bench_window_select' instead of the group-name
#       prefix 'window_select/' (zero-match -> criterion ran every bench and the
#       parser picked up a 1.39ms artifact for a ~1ns operation)
#     * its unit parser only recognised "us", so criterion's UTF-8 micro-unit
#       output fell through to the ms branch -> a 1000x misjudgement
#     * it used `grep -oP` while claiming macOS support (BSD grep has no -oP ->
#       every SLO was silently skipped on macOS)
#     * its summary denominator was redlines*2 = 22 while 32 sub-checks ran
#   check_dependency_rules.sh/.ps1 produced the same class of incident (layer map
#   frozen at 38 crates -> the iron-law job went red). One implementation, two
#   launchers is now the standing pattern (see check_crate_reachability.sh).
#
# Usage:
#   bash scripts/check_perf_redlines.sh                  # static gate (all)
#   bash scripts/check_perf_redlines.sh --static-only    # same, explicit
#   bash scripts/check_perf_redlines.sh thresholds       # criterion assertions
#   bash scripts/check_perf_redlines.sh --selftest       # proves the gate has teeth
# Exit code: 0 = pass, 1 = violation, 2 = usage/environment error
# Encoding: all-ASCII (project script convention)
# =============================================================================
set -euo pipefail

# PATH may lack coreutils when launched from PowerShell/MSYS; harmless on Linux.
export PATH="/usr/bin:/bin:$PATH"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

core="scripts/check_perf_redlines.py"
if [ ! -f "$core" ]; then
    echo "[FAIL] $core is missing" >&2
    exit 2
fi

# ubuntu-latest provides python3; MSYS may expose python; Windows launcher `py -3`.
if command -v python3 >/dev/null 2>&1; then
    exec python3 "$core" "$@"
elif command -v python >/dev/null 2>&1; then
    exec python "$core" "$@"
elif command -v py >/dev/null 2>&1; then
    exec py -3 "$core" "$@"
else
    echo "[FAIL] no python3/python/py interpreter found (core requires python >= 3.11)" >&2
    exit 2
fi
