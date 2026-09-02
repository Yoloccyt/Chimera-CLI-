# =============================================================================
# check_crate_reachability.ps1 - crate production-reachability ratchet (ADR-160)
# =============================================================================
# Purpose: thin wrapper around scripts/check_crate_reachability.py for local
#          Windows/PowerShell use. CI runs the .sh sibling (ubuntu-latest);
#          both delegate to the SAME python core so the rule cannot drift the
#          way check_dependency_rules.sh/.ps1 did (see ADR-160 context).
# Scope:   a workspace crate unreachable from the chimera-cli production
#          dependency graph is only allowed if listed in
#          scripts/crate_reachability_freeze.txt. New islands exit non-zero.
# Usage:
#   powershell -NoProfile -File scripts/check_crate_reachability.ps1
#   powershell -NoProfile -File scripts/check_crate_reachability.ps1 -SelfTest
# Exit code: 0 = ratchet holds, 1 = violation, 2 = usage/environment error
# Encoding: all-ASCII to avoid CJK locale issues (project script convention)
# =============================================================================
param(
    [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

$core = Join-Path $PSScriptRoot 'check_crate_reachability.py'
if (-not (Test-Path $core)) {
    Write-Host '[FAIL] scripts/check_crate_reachability.py is missing'
    exit 2
}

# Locate a python interpreter: python3 -> python -> py -3 (Windows launcher).
# Requires python >= 3.11 for tomllib; the python core prints its own [FAIL]
# line and exits 2 when tomllib is unavailable.
$candidates = @(
    @{ Exe = 'python3'; Args = @() },
    @{ Exe = 'python';  Args = @() },
    @{ Exe = 'py';      Args = @('-3') }
)

$psArgs = @()
if ($SelfTest) { $psArgs += '--selftest' }

foreach ($cand in $candidates) {
    if (-not (Get-Command $cand.Exe -ErrorAction SilentlyContinue)) { continue }
    & $cand.Exe @($cand.Args) $core @psArgs
    exit $LASTEXITCODE
}

Write-Host '[FAIL] no python3/python/py interpreter found (script requires python >= 3.11)'
exit 2
