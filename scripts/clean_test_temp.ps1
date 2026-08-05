#!/usr/bin/env pwsh
# P9-T4: Clean accumulated test temporary files to prevent I/O degradation
# Usage: powershell -File scripts/clean_test_temp.ps1
# Safe: only removes .tmp* directories under tmp/ and target/tmp/

param(
    [switch]$DryRun,
    [switch]$Force
)

$projectRoot = Split-Path -Parent $PSScriptRoot
$cleaned = 0
$freedBytes = 0

$tempDirs = @(
    "$projectRoot\tmp",
    "$projectRoot\target\tmp"
)

foreach ($dir in $tempDirs) {
    if (Test-Path $dir) {
        $items = Get-ChildItem $dir -Directory -Filter ".tmp*" -ErrorAction SilentlyContinue
        foreach ($item in $items) {
            $size = (Get-ChildItem $item.FullName -Recurse -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
            if ($DryRun) {
                Write-Host "[DRY-RUN] Would remove: $($item.FullName) ($([math]::Round($size/1MB, 2)) MB)"
            } else {
                Remove-Item $item.FullName -Recurse -Force -ErrorAction SilentlyContinue
                Write-Host "[CLEANED] $($item.FullName) ($([math]::Round($size/1MB, 2)) MB)"
            }
            $cleaned++
            $freedBytes += $size
        }
    }
}

Write-Host ""
Write-Host "Summary: $cleaned directories cleaned, $([math]::Round($freedBytes/1MB, 2)) MB freed"
if ($DryRun) { Write-Host "(DRY-RUN mode — no files were actually deleted)" }
