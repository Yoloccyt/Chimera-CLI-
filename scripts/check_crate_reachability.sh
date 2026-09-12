#!/usr/bin/env bash
# =============================================================================
# check_crate_reachability.sh - crate production-reachability ratchet (ADR-160)
# =============================================================================
# Purpose: thin wrapper around scripts/check_crate_reachability.py so CI (which
#          runs bash on ubuntu-latest) and local PowerShell/MSYS share ONE
#          implementation. The two-copies-of-one-rule mistake that turned the
#          dependency iron-law job red (check_dependency_rules.sh stuck at 38
#          crates while the .ps1 moved to 43) must not be repeated here.
# Scope:  a workspace crate unreachable from the chimera-cli production
#         dependency graph is only allowed if listed in
#         scripts/crate_reachability_freeze.txt. New islands fail the build.
# Usage:
#   bash scripts/check_crate_reachability.sh
#   bash scripts/check_crate_reachability.sh --selftest
# Exit code: 0 = ratchet holds, 1 = violation, 2 = usage/environment error
# Encoding: all-ASCII to avoid CJK path/locale issues (project script convention)
# =============================================================================
set -euo pipefail

# Ensure coreutils are reachable even when the parent process (e.g. PowerShell
# on Windows/MSYS) passes a PATH without /usr/bin. Harmless no-op on Linux/macOS.
export PATH="/usr/bin:/bin:$PATH"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# Locate a python interpreter (ubuntu-latest: python3; MSYS may expose python,
# Windows launcher may only offer `py -3`).
if command -v python3 >/dev/null 2>&1; then
    exec python3 scripts/check_crate_reachability.py "$@"
elif command -v python >/dev/null 2>&1; then
    exec python scripts/check_crate_reachability.py "$@"
elif command -v py >/dev/null 2>&1; then
    exec py -3 scripts/check_crate_reachability.py "$@"
else
    echo "[FAIL] no python3/python/py interpreter found (script requires python >= 3.11)" >&2
    exit 2
fi
