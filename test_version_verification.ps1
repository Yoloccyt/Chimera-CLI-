#Requires -Version 5.1
<#
.SYNOPSIS
    Local simulation of CI pipeline "Verify image functionality (--version)" step.

.DESCRIPTION
    This script does NOT require Docker. It uses the local release binary to
    simulate the three-stage verification from CI:
    1. Image pullable (simulated by binary existence check)
    2. docker run --version (simulated by direct binary execution)
    3. grep regex validation of output format

    Uses the SAME regex as release.yml line 219:
        ^(aether|chimera) [0-9]+\.[0-9]+\.[0-9]+

    Test coverage:
    - Real binary output (happy path)
    - Simulated normal outputs (aether/chimera x various version formats)
    - Simulated abnormal outputs (empty, no version, wrong prefix, etc.)
    - Multi-line outputs (binary emitting extra info)
    - Regex consistency between grep and PowerShell

.EXAMPLE
    .\test_version_verification.ps1
    .\test_version_verification.ps1 -BinaryPath "D:\Chimera CLI\target\release\aether.exe"
#>

# ============================================================
# param MUST be the first non-comment statement in the script
# ============================================================
param(
    [string]$BinaryPath = "D:\Chimera CLI\target\release\aether.exe"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ============================================================
# Regex - IDENTICAL to release.yml line 219
# grep -qE '^(aether|chimera) [0-9]+\.[0-9]+\.[0-9]+'
# PowerShell equivalent: -match '^(aether|chimera) \d+\.\d+\.\d+'
# Note: grep [0-9]+ and PowerShell \d+ are semantically identical
# (both match one or more digits)
# ============================================================
$script:VersionRegex = '^(aether|chimera) \d+\.\d+\.\d+'

# ============================================================
# Test counters
# ============================================================
$script:PassCount = 0
$script:FailCount = 0
$script:SkipCount = 0

# ============================================================
# Helpers: colored output
# ============================================================
function Write-Section([string]$Title) {
    Write-Host ""
    Write-Host ("=" * 70) -ForegroundColor Cyan
    Write-Host "  $Title" -ForegroundColor Cyan
    Write-Host ("=" * 70) -ForegroundColor Cyan
}

function Write-Pass([string]$Description, [string]$Detail = "") {
    $script:PassCount++
    Write-Host "  [PASS] $Description" -ForegroundColor Green
    if ($Detail) { Write-Host "         $Detail" -ForegroundColor DarkGray }
}

function Write-Fail([string]$Description, [string]$Detail = "") {
    $script:FailCount++
    Write-Host "  [FAIL] $Description" -ForegroundColor Red
    if ($Detail) { Write-Host "         $Detail" -ForegroundColor DarkGray }
}

function Write-Skip([string]$Description, [string]$Detail = "") {
    $script:SkipCount++
    Write-Host "  [SKIP] $Description" -ForegroundColor Yellow
    if ($Detail) { Write-Host "         $Detail" -ForegroundColor DarkGray }
}

# ============================================================
# Core validation function: simulates CI grep logic
# ============================================================
# Returns $true if any line matches the regex (simulating grep -qE
# which exits 0 if any line matches), $false otherwise.
function Test-VersionOutput([string]$Output) {
    # grep -qE semantics: pass if ANY line matches
    # PowerShell -match checks the whole string by default,
    # so we split by newline and match line-by-line.
    #
    # WHY -cmatch instead of -match:
    #   grep -E is case-sensitive by default, but PowerShell -match is
    #   case-INSENSITIVE by default. Using -cmatch ensures the test
    #   script behaves identically to the CI grep command.
    $lines = $Output -split "`n" | ForEach-Object { $_.TrimEnd("`r") }
    foreach ($line in $lines) {
        if ($line -cmatch $script:VersionRegex) {
            return $true
        }
    }
    return $false
}

# ============================================================
# Test 1: Real binary --version output (happy path)
# ============================================================
Write-Section "Test 1: Real binary --version output (happy path)"

if (-not (Test-Path $BinaryPath)) {
    Write-Skip "Binary not found, skipping real-path test" "Path: $BinaryPath"
    Write-Host ""
    Write-Host "Hint: run 'cargo build --release -p chimera-cli' first" -ForegroundColor Yellow
} else {
    Write-Host "  Binary: $BinaryPath" -ForegroundColor DarkGray
    Write-Host "  Executing: --version" -ForegroundColor DarkGray

    try {
        $realOutput = & $BinaryPath --version 2>&1 | Out-String
        $realOutput = $realOutput.Trim()
        Write-Host "  Output: $realOutput" -ForegroundColor DarkGray

        if (Test-VersionOutput $realOutput) {
            Write-Pass "Real binary --version validation passed" "Output matches regex: $realOutput"
        } else {
            Write-Fail "Real binary --version validation failed" "Output does not match regex: $realOutput"
        }
    } catch {
        Write-Fail "Binary execution failed" $_.Exception.Message
    }
}

# ============================================================
# Test 2: Simulated normal outputs (all should PASS)
# ============================================================
Write-Section "Test 2: Simulated normal outputs (all should PASS)"

$normalCases = @(
    @{ Output = "aether 1.0.0-omega";       Desc = "aether + omega version" }
    @{ Output = "chimera 1.0.0-omega";      Desc = "chimera + omega version" }
    @{ Output = "aether 1.0.0";             Desc = "aether + plain semver" }
    @{ Output = "chimera 2.5.3";            Desc = "chimera + multi-digit version" }
    @{ Output = "aether 10.20.30";          Desc = "aether + two-digit version parts" }
    @{ Output = "aether 1.0.0-alpha";       Desc = "aether + alpha prerelease" }
    @{ Output = "aether 1.0.0-rc.1";        Desc = "aether + rc prerelease" }
    # WHY aether 1.0.0.0 passes: CI regex has no $ anchor (intentional, to allow
    # -omega/-alpha prerelease suffixes), so four-part versions also match.
    # This is harmless because real binary output format is fixed.
    @{ Output = "aether 1.0.0.0";           Desc = "aether + four-part version (passes due to no $ anchor, design choice)" }
)

foreach ($case in $normalCases) {
    if (Test-VersionOutput $case.Output) {
        Write-Pass $case.Desc "Input: '$($case.Output)'"
    } else {
        Write-Fail $case.Desc "Input: '$($case.Output)' (should pass but did not)"
    }
}

# ============================================================
# Test 3: Simulated abnormal outputs (all should be REJECTED)
# ============================================================
Write-Section "Test 3: Simulated abnormal outputs (all should be REJECTED)"

$abnormalCases = @(
    @{ Output = "";                         Desc = "empty output" }
    @{ Output = "unknown 1.0.0";            Desc = "unknown program name" }
    @{ Output = "aether";                   Desc = "program name only, no version" }
    @{ Output = "1.0.0";                    Desc = "version only, no program name" }
    @{ Output = "aether v1.0.0";            Desc = "version with v prefix (regex does not match)" }
    @{ Output = "aether 1.0";               Desc = "two-part version (missing patch)" }
    @{ Output = "  aether 1.0.0";           Desc = "leading spaces (^ anchors to line start)" }
    @{ Output = "aether  1.0.0";            Desc = "double space between name and version" }
    @{ Output = "Aether 1.0.0";             Desc = "uppercase A (grep is case-sensitive, -cmatch enforces this)" }
    @{ Output = "aether 1.0.x";             Desc = "non-numeric patch" }
    @{ Output = "error: binary not found";  Desc = "error message" }
)

foreach ($case in $abnormalCases) {
    if (-not (Test-VersionOutput $case.Output)) {
        Write-Pass $case.Desc "Input: '$($case.Output)' (correctly rejected)"
    } else {
        Write-Fail $case.Desc "Input: '$($case.Output)' (should be rejected but passed)"
    }
}

# ============================================================
# Test 4: Edge cases - multi-line output
# ============================================================
Write-Section "Test 4: Edge cases - multi-line output"

$multilineCases = @(
    @{
        Output = "Chimera CLI (NEXUS-OMEGA)`naether 1.0.0-omega`nBuild: 2026-06-28"
        Desc = "multi-line, version line in middle"
        Expected = $true
    }
    @{
        Output = "Loading...`nReady`naether 1.0.0-omega"
        Desc = "multi-line, version line at end"
        Expected = $true
    }
    @{
        Output = "Loading...`nReady`nDone"
        Desc = "multi-line, no version line"
        Expected = $false
    }
)

foreach ($case in $multilineCases) {
    $actual = Test-VersionOutput $case.Output
    if ($actual -eq $case.Expected) {
        Write-Pass $case.Desc "Expected: $($case.Expected), Actual: $actual"
    } else {
        Write-Fail $case.Desc "Expected: $($case.Expected), Actual: $actual"
    }
}

# ============================================================
# Test 5: Regex consistency (CI grep vs PowerShell -match)
# ============================================================
Write-Section "Test 5: Regex consistency (CI grep vs PowerShell)"

$ciRegex = '^(aether|chimera) [0-9]+\.[0-9]+\.[0-9]+'
$psRegex = $script:VersionRegex

Write-Host "  CI regex (grep):  $ciRegex" -ForegroundColor DarkGray
Write-Host "  PS regex (-match): $psRegex" -ForegroundColor DarkGray

# Verify both regexes are semantically equivalent on the same input set
$testInputs = @("aether 1.0.0", "chimera 2.0.0", "unknown 1.0.0", "aether v1.0.0")
$allConsistent = $true

foreach ($input in $testInputs) {
    # grep semantics: case-sensitive line match
    $grepResult = $input -cmatch $ciRegex
    # PowerShell semantics: case-sensitive line match
    $psResult = $input -cmatch $psRegex

    if ($grepResult -ne $psResult) {
        Write-Fail "Regex inconsistency" "Input: '$input', grep: $grepResult, PS: $psResult"
        $allConsistent = $false
    }
}

if ($allConsistent) {
    Write-Pass "CI and PowerShell regexes are semantically equivalent" "All test inputs produced identical results"
}

# ============================================================
# Test report
# ============================================================
Write-Section "Test Report"

$total = $script:PassCount + $script:FailCount + $script:SkipCount
Write-Host ""
Write-Host "  Total:  $total" -ForegroundColor White
Write-Host "  Passed: $script:PassCount" -ForegroundColor Green
Write-Host "  Failed: $script:FailCount" -ForegroundColor $(if ($script:FailCount -gt 0) { "Red" } else { "White" })
Write-Host "  Skipped: $script:SkipCount" -ForegroundColor Yellow
Write-Host ""

if ($script:FailCount -eq 0) {
    Write-Host "  Conclusion: All tests passed, CI verification logic works correctly" -ForegroundColor Green
    Write-Host ""
    Write-Host "  Next step: Push code to trigger CI, verify docker job on GitHub Actions" -ForegroundColor Cyan
    exit 0
} else {
    Write-Host "  Conclusion: Some tests failed, review regex or validation logic" -ForegroundColor Red
    exit 1
}
