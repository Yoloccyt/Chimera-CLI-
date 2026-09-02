# =============================================================================
# check_perf_redlines.ps1 - performance red-line + bench-inventory gate
# =============================================================================
# Purpose: THIN WRAPPER around scripts/check_perf_redlines.py for local
#          Windows/PowerShell use. CI runs the .sh sibling (ubuntu-latest);
#          both delegate to the SAME python core so the rule cannot drift the
#          way the two hand-written copies did before 2026-08-31 (see the .sh
#          header for the concrete drift list, and audit finding F-R4-01).
#
# ENCODING NOTE: the previous 429-line implementation carried non-ASCII string
#   literals and therefore REQUIRED a UTF-8 BOM -- without it, Windows PowerShell
#   5.1 decoded the script as GBK/936, a CJK lead byte swallowed an adjacent
#   quote, and ~70 ParserErrors made the gate unable to even start. This wrapper
#   is pure ASCII, so the BOM trap is structurally gone rather than merely
#   documented. Keep it ASCII.
#
# Usage:
#   powershell -NoProfile -File scripts/check_perf_redlines.ps1
#   powershell -NoProfile -File scripts/check_perf_redlines.ps1 -SelfTest
#   powershell -NoProfile -File scripts/check_perf_redlines.ps1 -Mode thresholds
#   powershell -NoProfile -File scripts/check_perf_redlines.ps1 -Mode slo
# Exit code: 0 = pass, 1 = violation, 2 = usage/environment error
# =============================================================================
param(
    [switch]$SelfTest,
    # Mode / ExtraArgs mirror the python core CLI: all | lint | inventory |
    # invariants | thresholds | slo | ignored | emit-ignored.
    [string]$Mode = '',
    [Parameter(ValueFromRemainingArguments = $true)][string[]]$ExtraArgs
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

$core = Join-Path $PSScriptRoot 'check_perf_redlines.py'
if (-not (Test-Path $core)) {
    Write-Host '[FAIL] scripts/check_perf_redlines.py is missing'
    exit 2
}

$coreArgs = @()
if ($SelfTest) { $coreArgs += '--selftest' }
if ($Mode) { $coreArgs += $Mode }
if ($ExtraArgs) { $coreArgs += $ExtraArgs }

# Interpreter probe order mirrors the .sh sibling: python3 -> python -> py -3.
# Requires python >= 3.11 for tomllib; the core prints its own [FAIL] and exits 2.
# An explicit FAIL (not a silent skip) is deliberate: a gate that "isn't there"
# must look like a failure, otherwise local verification silently disappears --
# which is exactly how this gate lost its only executor for months.
$candidates = @(
    @{ Exe = 'python3'; Pre = @() },
    @{ Exe = 'python';  Pre = @() },
    @{ Exe = 'py';      Pre = @('-3') }
)

foreach ($cand in $candidates) {
    if (-not (Get-Command $cand.Exe -ErrorAction SilentlyContinue)) { continue }
    & $cand.Exe @($cand.Pre) $core @($coreArgs)
    exit $LASTEXITCODE
}

Write-Host '[FAIL] no python3/python/py interpreter found (core requires python >= 3.11)'
exit 2
