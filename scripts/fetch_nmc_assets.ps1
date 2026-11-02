<#
=============================================================================
 fetch_nmc_assets.ps1 - NMC model asset fetcher (WI-C2; ADR-196 D4 preflight)
=============================================================================
 Companion: scripts/fetch_nmc_assets.sh (parity by design: same manifest,
 same row checks, same exit codes). ASCII-only on purpose: Windows PowerShell
 5.1 runs under a GBK console, and non-ASCII content would require a UTF-8
 BOM (gate G-85) -- pure ASCII keeps both sides trivially in parity.

 Behavior:
   1. Parse assets/nmc/manifest.toml ([[model]] rows).
   2. Per row: status READY (url and sha256 both set) or PENDING (both empty).
      A half-filled row (only one set) is malformed input -> exit 2.
   3. Default is a DRY RUN: print the plan, touch nothing, exit 0.
   4. -Apply downloads READY rows into -TargetDir (default
      assets/nmc/models), verifies SHA-256, fails loudly on mismatch.
      PENDING rows are skipped with an explicit message. Model binaries are
      NEVER committed (see .gitignore: assets/nmc/models/).

 Exit codes: 0 ok / 1 apply failure (download or hash mismatch) / 2 malformed
 input (manifest unreadable, no rows, bad row).
#>
param(
    [string]$Manifest = "assets/nmc/manifest.toml",
    [string]$TargetDir = "assets/nmc/models",
    [switch]$Apply
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $Manifest)) {
    Write-Host "ERROR: manifest not found: $Manifest"
    exit 2
}

# ---- parse [[model]] rows (key = "value" lines) -----------------------------
$rows = @()
$cur = $null
foreach ($raw in Get-Content -LiteralPath $Manifest) {
    $line = $raw.Trim()
    if ($line -eq "[[model]]") {
        if ($cur -ne $null) { $rows += $cur }
        $cur = @{ model_type = ""; file = ""; url = ""; sha256 = "" }
        continue
    }
    if ($cur -eq $null -or $line -eq "" -or $line.StartsWith("#")) { continue }
    if ($line -match '^([A-Za-z_]+)\s*=\s*"(.*)"\s*$') {
        $key = $Matches[1]; $val = $Matches[2]
        if ($cur.ContainsKey($key)) { $cur[$key] = $val }
    }
}
if ($cur -ne $null) { $rows += $cur }

if ($rows.Count -eq 0) {
    Write-Host "ERROR: no [[model]] rows in $Manifest"
    exit 2
}

$types = @("Image", "Video", "Audio", "Text")
$pending = 0
$ready = 0
$plan = @()

foreach ($row in $rows) {
    if (-not ($types -contains $row.model_type)) {
        Write-Host "ERROR: unknown model_type '$($row.model_type)'"
        exit 2
    }
    $hasUrl = $row.url -ne ""
    $hasSha = $row.sha256 -ne ""
    if ($hasUrl -ne $hasSha) {
        Write-Host "ERROR: half-filled row ($($row.model_type)): url and sha256 must be set together"
        exit 2
    }
    if ($hasUrl -and $hasSha) {
        $ready += 1
        $plan += "READY   $($row.model_type) -> $($row.file) <- $($row.url)"
    } else {
        $pending += 1
        $plan += "PENDING $($row.model_type) -> $($row.file) (asset source/license not adjudicated yet)"
    }
}

Write-Host "== NMC asset plan ($Manifest) =="
foreach ($p in $plan) { Write-Host "  $p" }
Write-Host "  ready=$ready pending=$pending apply=$($Apply.IsPresent)"

if (-not $Apply) {
    Write-Host "DRY RUN (pass -Apply to download READY rows)"
    exit 0
}

# ---- apply: download + SHA-256 verify ---------------------------------------
if (-not (Test-Path -LiteralPath $TargetDir)) {
    New-Item -ItemType Directory -Path $TargetDir | Out-Null
}

foreach ($row in $rows) {
    if ($row.url -eq "") {
        Write-Host "SKIP  $($row.model_type): PENDING (no url/sha256 yet)"
        continue
    }
    $dest = Join-Path $TargetDir $row.file
    Write-Host "GET   $($row.model_type): $($row.url) -> $dest"
    try {
        Invoke-WebRequest -Uri $row.url -OutFile $dest
    } catch {
        Write-Host "ERROR: download failed ($($row.model_type)): $($_.Exception.Message)"
        exit 1
    }
    $actual = (Get-FileHash -LiteralPath $dest -Algorithm SHA256).Hash.ToLower()
    $expect = $row.sha256.ToLower()
    if ($actual -ne $expect) {
        Write-Host "ERROR: SHA-256 mismatch ($($row.model_type)): expected $expect got $actual"
        Remove-Item -LiteralPath $dest -Force -ErrorAction SilentlyContinue
        exit 1
    }
    Write-Host "OK    $($row.model_type): verified $actual"
}

Write-Host "DONE: all READY rows fetched and verified"
exit 0
