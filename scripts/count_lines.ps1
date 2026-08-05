# count_lines.ps1 - Count total lines of code in the project (fast pipeline)
# Encoding: UTF-8 BOM (ASCII-safe content)

$ErrorActionPreference = 'SilentlyContinue'
$root = "d:\Chimera CLI"
Set-Location $root

$excluded = @('tmp_podman', 'node_modules', 'target', '.git')

function Is-NotExcluded {
    param([string]$Path)
    foreach ($ex in $global:excluded) {
        if ($Path -match "\\$ex\\" -or $Path -match "\\$ex$") { return $false }
    }
    return $true
}

function Count-Fast {
    param([string]$Label, [string]$Pattern)
    $files = Get-ChildItem -Recurse -File -Filter $Pattern -ErrorAction SilentlyContinue | Where-Object { Is-NotExcluded $_.FullName }
    $totalLines = 0
    $fileCount = 0
    foreach ($f in $files) {
        $totalLines += (Get-Content $f.FullName -ReadCount 0).Length
        $fileCount++
    }
    $global:results += @{Label=$Label; Files=$fileCount; Lines=$totalLines}
}

$global:results = @()
$global:excluded = $excluded

Count-Fast -Label "Rust source (*.rs)" -Pattern "*.rs"
Count-Fast -Label "Markdown (*.md)" -Pattern "*.md"
Count-Fast -Label "Cargo.toml" -Pattern "Cargo.toml"

# PowerShell
$psFiles = Get-ChildItem -Recurse -File -Filter "*.ps1" -ErrorAction SilentlyContinue | Where-Object { Is-NotExcluded $_.FullName }
$psmFiles = Get-ChildItem -Recurse -File -Filter "*.psm1" -ErrorAction SilentlyContinue | Where-Object { Is-NotExcluded $_.FullName }
$psCount = 0; $psFCount = 0
foreach ($f in ($psFiles + $psmFiles)) { $psCount += (Get-Content $f.FullName -ReadCount 0).Length; $psFCount++ }
$results += @{Label="PowerShell (*.ps1, *.psm1)"; Files=$psFCount; Lines=$psCount}

# Shell
$shFiles = Get-ChildItem -Recurse -File -Filter "*.sh" -ErrorAction SilentlyContinue | Where-Object { Is-NotExcluded $_.FullName }
$shCount = 0; $shFCount = 0
foreach ($f in $shFiles) { $shCount += (Get-Content $f.FullName -ReadCount 0).Length; $shFCount++ }
$results += @{Label="Shell (*.sh)"; Files=$shFCount; Lines=$shCount}

# Other TOML
$extraToml = Get-ChildItem -Recurse -File -Filter "*.toml" -ErrorAction SilentlyContinue | Where-Object { Is-NotExcluded $_.FullName } | Where-Object { $_.Name -ne "Cargo.toml" -and $_.Name -ne "Cargo.lock" }
$etCount = 0; $etFCount = 0
foreach ($f in $extraToml) { $etCount += (Get-Content $f.FullName -ReadCount 0).Length; $etFCount++ }
$results += @{Label="Other TOML (*.toml excl Cargo)"; Files=$etFCount; Lines=$etCount}

# JSON
$jsonItems = Get-ChildItem -Recurse -File -Filter "*.json" -ErrorAction SilentlyContinue | Where-Object { Is-NotExcluded $_.FullName }
$jCount = 0; $jFCount = 0
foreach ($f in $jsonItems) { $jCount += (Get-Content $f.FullName -ReadCount 0).Length; $jFCount++ }
$results += @{Label="JSON (*.json)"; Files=$jFCount; Lines=$jCount}

# YAML
$yamlItems = Get-ChildItem -Recurse -File -Filter "*.yml" -ErrorAction SilentlyContinue | Where-Object { Is-NotExcluded $_.FullName }
$yamlItems2 = Get-ChildItem -Recurse -File -Filter "*.yaml" -ErrorAction SilentlyContinue | Where-Object { Is-NotExcluded $_.FullName }
$yCount = 0; $yFCount = 0
foreach ($f in ($yamlItems + $yamlItems2)) { $yCount += (Get-Content $f.FullName -ReadCount 0).Length; $yFCount++ }
$results += @{Label="YAML (*.yml, *.yaml)"; Files=$yFCount; Lines=$yCount}

# Config
$cfgCount = 0; $cfgFCount = 0
foreach ($pat in @("Dockerfile*", ".gitignore", ".env*", "*.conf")) {
    $items = Get-ChildItem -Recurse -File -Filter $pat -ErrorAction SilentlyContinue | Where-Object { Is-NotExcluded $_.FullName }
    foreach ($f in $items) { $cfgCount += (Get-Content $f.FullName -ReadCount 0).Length; $cfgFCount++ }
}
$results += @{Label="Config (Dockerfile, .gitignore, .env, .conf)"; Files=$cfgFCount; Lines=$cfgCount}

# Summary
Write-Host ""
Write-Host ("#" * 60)
Write-Host "  NEXUS-OMEGA Project Code Line Count (v2.19.0-omega)"
Write-Host ("#" * 60)
Write-Host ""
Write-Host ("{0,-40} {1,8} {2,12}" -f "Category", "Files", "Lines")
Write-Host ("{0,-40} {1,8} {2,12}" -f ("-" * 40), ("-" * 8), ("-" * 12))
$totalFiles = 0; $totalLines = 0
foreach ($r in $results) {
    Write-Host ("{0,-40} {1,8} {2,12}" -f $r.Label, $r.Files, $r.Lines)
    $totalFiles += $r.Files
    $totalLines += $r.Lines
}
Write-Host ("{0,-40} {1,8} {2,12}" -f ("-" * 40), ("-" * 8), ("-" * 12))
Write-Host ("{0,-40} {1,8} {2,12}" -f "TOTAL", $totalFiles, $totalLines)
Write-Host ""
Write-Host "Date: $((Get-Date).ToString('yyyy-MM-dd HH:mm:ss'))"
Write-Host "Excluded: $($excluded -join ', ')"
Write-Host ""