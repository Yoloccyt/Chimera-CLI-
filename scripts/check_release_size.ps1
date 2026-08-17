#!/usr/bin/env pwsh
#Requires -Version 7.0
<#
.SYNOPSIS
    分析 chimera CLI release 构建的体积大头，输出 top N crate 体积占比。

.DESCRIPTION
    调用 cargo-bloat 分析 chimera.exe 的 release 构建，提取：
    - binary 总大小（与 50MB 约束对比）
    - top N 体积大头 crate（默认 10）
    - .text section 大小
    支持基线保存与对比，用于体积回归监控。

.PARAMETER Top
    显示体积大头 crate 数量（默认 10）

.PARAMETER SaveBaseline
    保存当前体积数据到指定 JSON 文件（路径相对于脚本目录）

.PARAMETER CompareBaseline
    与指定基线文件对比（输出体积差异与新增 crate）

.PARAMETER ThresholdMB
    体积约束阈值（默认 50 MB）

.EXAMPLE
    .\check_release_size.ps1
    显示当前体积大头 top 10

.EXAMPLE
    .\check_release_size.ps1 -SaveBaseline docs/reports/baselines/chimera-bloat-2026-08.json
    保存当前基线

.EXAMPLE
    .\check_release_size.ps1 -CompareBaseline docs/reports/baselines/chimera-bloat-2026-08.json
    与基线对比，输出差异

.NOTES
    依赖: cargo-bloat (cargo install cargo-bloat)
    约束: release.yml 已断言 chimera.exe < 50MB
    已知体积大头: hnsw_rs (HNSW 索引), mel_spec (频谱特征), libsqlite3_sys (SQLite)
#>
[CmdletBinding()]
param(
    [int]$Top = 10,
    [string]$SaveBaseline,
    [string]$CompareBaseline,
    [double]$ThresholdMB = 50.0
)

$ErrorActionPreference = "Stop"

# 环境设置
$env:CARGO_HOME = "D:\Chimera CLI\.toolchain\cargo"
$env:RUSTUP_HOME = "D:\Chimera CLI\.toolchain\rustup"
$env:PATH = "$env:CARGO_HOME\bin;D:\msys64\mingw64\bin;$env:PATH"

# 路径
$scriptDir = $PSScriptRoot
$repoRoot = Split-Path $scriptDir -Parent
$binPath = Join-Path $repoRoot "target\release\chimera.exe"

Write-Host "=== cargo-bloat 体积分析 ===" -ForegroundColor Cyan
Write-Host "目标: $binPath"

# 检查 binary 存在
if (-not (Test-Path $binPath)) {
    Write-Error "release binary 不存在: $binPath`n请先运行: cargo build --release --package chimera-cli --bin chimera"
    exit 1
}

# 读取 binary 大小
$binSize = (Get-Item $binPath).Length
$binSizeMB = $binSize / 1MB
Write-Host "Binary 总大小: $([math]::Round($binSizeMB, 2)) MB" -ForegroundColor $(if ($binSizeMB -ge $ThresholdMB) { "Red" } else { "Green" })

# 约束检查
if ($binSizeMB -ge $ThresholdMB) {
    Write-Error "Binary 大小 $([math]::Round($binSizeMB, 2)) MB 超过约束 $ThresholdMB MB"
    exit 1
}

# 运行 cargo-bloat
Write-Host "运行 cargo-bloat --release --crates..." -ForegroundColor Yellow
$bloatOutput = cargo bloat --release --crates --package chimera-cli --bin chimera 2>&1
$bloatLines = $bloatOutput -split "`n"

# 解析输出
$crates = @()
$unknownSize = 0
$textSectionSize = 0
$parseMode = "header"

foreach ($line in $bloatLines) {
    if ($line -match "^\s*File\s+\.text\s+Size\s+Crate") {
        $parseMode = "data"
        continue
    }
    if ($parseMode -eq "data") {
        if ($line -match "^\s*([\d.]+)%\s+([\d.]+)%\s+([\d.]+\w+)\s+\[Unknown\]") {
            $unknownSize = [double]($matches[3] -replace "[^\d.]", "")
        }
        elseif ($line -match "^\s*([\d.]+)%\s+([\d.]+)%\s+([\d.]+)(\w+)\s+(\S+)") {
            $pct = [double]$matches[1]
            $textPct = [double]$matches[2]
            $sizeStr = $matches[3]
            $unit = $matches[4]
            $crate = $matches[5]

            $sizeKB = switch ($unit) {
                "MiB" { [double]$sizeStr * 1024 }
                "KiB" { [double]$sizeStr }
                "B"   { [double]$sizeStr / 1024 }
                default { 0 }
            }

            $crates += [PSCustomObject]@{
                Crate = $crate
                SizeKB = $sizeKB
                FilePct = $pct
                TextPct = $textPct
            }
        }
    }
}

# 排序并取 top N
$topCrates = $crates | Sort-Object SizeKB -Descending | Select-Object -First $Top

# 从 crates 列表提取 .text section 大小（cargo-bloat 将其作为特殊 crate 条目输出）
$textEntry = $crates | Where-Object { $_.Crate -eq ".text" } | Select-Object -First 1
if ($textEntry) {
    $textSectionSize = $textEntry.SizeKB
} else {
    $textSectionSize = 0
}

Write-Host "`n=== Top $Top 体积大头 Crate ===" -ForegroundColor Cyan
Write-Host ("{0,-25} {1,10} {2,10} {3,10}" -f "Crate", "Size(KB)", "File%", "Text%")
Write-Host ("-" * 60)
foreach ($c in $topCrates) {
    Write-Host ("{0,-25} {1,10:N1} {2,10:F1} {3,10:F1}" -f $c.Crate, $c.SizeKB, $c.FilePct, $c.TextPct)
}

Write-Host "`n[Unknown] 段: $([math]::Round($unknownSize, 2)) MiB (LTO 后符号丢失)" -ForegroundColor Yellow
Write-Host ".text section: $([math]::Round($textSectionSize / 1024, 2)) MiB" -ForegroundColor Yellow

# 基线保存
if ($SaveBaseline) {
    $baselinePath = if ([System.IO.Path]::IsPathRooted($SaveBaseline)) { $SaveBaseline } else { Join-Path $repoRoot $SaveBaseline }
    $baselineDir = Split-Path $baselinePath -Parent
    if (-not (Test-Path $baselineDir)) { New-Item -ItemType Directory -Path $baselineDir -Force | Out-Null }
    $baselineData = @{
        timestamp = (Get-Date -Format "o")
        binarySizeMB = [math]::Round($binSizeMB, 2)
        textSectionSizeMB = [math]::Round($textSectionSize / 1024, 2)
        unknownSizeMB = [math]::Round($unknownSize, 2)
        topCrates = $topCrates | ForEach-Object {
            @{ Crate = $_.Crate; SizeKB = [math]::Round($_.SizeKB, 1); FilePct = $_.FilePct; TextPct = $_.TextPct }
        }
    }
    $baselineData | ConvertTo-Json -Depth 5 | Set-Content $baselinePath -Encoding UTF8
    Write-Host "`n基线已保存: $baselinePath" -ForegroundColor Green
}

# 基线对比
if ($CompareBaseline) {
    $baselinePath = if ([System.IO.Path]::IsPathRooted($CompareBaseline)) { $CompareBaseline } else { Join-Path $repoRoot $CompareBaseline }
    if (-not (Test-Path $baselinePath)) {
        Write-Error "基线文件不存在: $baselinePath"
        exit 1
    }
    $baseline = Get-Content $baselinePath -Raw | ConvertFrom-Json

    Write-Host "`n=== 与基线对比 ===" -ForegroundColor Cyan
    Write-Host "基线时间: $($baseline.timestamp)"

    $sizeDiffMB = $binSizeMB - $baseline.binarySizeMB
    $sizeDiffPct = ($sizeDiffMB / $baseline.binarySizeMB) * 100
    Write-Host "Binary 大小变化: $([math]::Round($sizeDiffMB, 2)) MB ($([math]::Round($sizeDiffPct, 1))%)" -ForegroundColor $(if ($sizeDiffMB -gt 0.5) { "Yellow" } elseif ($sizeDiffMB -lt -0.5) { "Green" } else { "Gray" })

    # 新增/消失的 crate
    $baselineCrates = $baseline.topCrates | ForEach-Object { $_.Crate }
    $currentCrates = $topCrates.Crate
    $newCrates = $currentCrates | Where-Object { $_ -notin $baselineCrates }
    $removedCrates = $baselineCrates | Where-Object { $_ -notin $currentCrates }

    if ($newCrates) {
        Write-Host "新增体积大头 crate:" -ForegroundColor Yellow
        $newCrates | ForEach-Object { Write-Host "  + $_" }
    }
    if ($removedCrates) {
        Write-Host "移除体积大头 crate:" -ForegroundColor Green
        $removedCrates | ForEach-Object { Write-Host "  - $_" }
    }

    # 异常增长检查（单 crate 增长 > 10% 且绝对增长 > 10KB，过滤微小 crate 的百分比噪声）
    $regressions = @()
    foreach ($c in $topCrates) {
        $baselineCrate = $baseline.topCrates | Where-Object { $_.Crate -eq $c.Crate } | Select-Object -First 1
        if ($baselineCrate) {
            $diff = $c.SizeKB - $baselineCrate.SizeKB
            $pct = if ($baselineCrate.SizeKB -gt 0) { ($diff / $baselineCrate.SizeKB) * 100 } else { 0 }
            if ($pct -gt 10 -and $diff -gt 10) {
                $regressions += [PSCustomObject]@{
                    Crate = $c.Crate
                    DiffKB = [math]::Round($diff, 1)
                    Pct = [math]::Round($pct, 1)
                }
            }
        }
    }

    if ($regressions) {
        Write-Host "`n体积回归 (>10% 增长):" -ForegroundColor Red
        foreach ($r in $regressions) {
            Write-Host "  $($r.Crate): +$($r.DiffKB) KB (+$($r.Pct)%)"
        }
        exit 1
    }
}

Write-Host "`n体积分析完成 ✓" -ForegroundColor Green
exit 0
