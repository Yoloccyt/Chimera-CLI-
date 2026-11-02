# =============================================================================
# scripts/measure_startup.ps1 — 工具链启动/装配分段测量（性能重构 Task 0）
# =============================================================================
# Purpose: 为 toolchain 性能重构提供**可复现**的启动税证据。用 RUST_LOG=info 抓
#          `startup phase: parse+init|config|runtime|shard|assemble` 五段（shard 桩在
#          composition.rs），并对一组代表
#          命令各跑 N 次，报告 median/p95 墙钟（PowerShell Measure-Command）。
# 非门禁：本脚本产出的数字是观测/趋势，不写死阈值（release 门禁仍由
#          scripts/check_cli_startup.ps1 的 median<100ms 负责，.sh/CI 同阈值）。
# 用法：
#   pwsh -File scripts/measure_startup.ps1                 # 用 debug 二进制
#   pwsh -File scripts/measure_startup.ps1 -Release        # 先 cargo build --release 再测
#   pwsh -File scripts/measure_startup.ps1 -Iterations 50  # 自定义迭代次数
# =============================================================================
[CmdletBinding()]
param(
    [switch]$Release,
    [int]$Iterations = 20
)

$ErrorActionPreference = 'Stop'
# Toolchain env comes from the shared, repo-root-derived helper (see
# scripts/toolchain-env.ps1) instead of a pinned absolute path copy.
. (Join-Path $PSScriptRoot 'toolchain-env.ps1')
Set-ChimeraToolchainEnv | Out-Null

$profile = if ($Release) { 'release' } else { 'debug' }
Write-Host "building chimera ($profile) ..." -ForegroundColor Cyan
if ($Release) {
    cargo build --release -p chimera-cli --bin chimera
} else {
    cargo build -p chimera-cli --bin chimera
}
$bin = "target\$profile\chimera.exe"
if (-not (Test-Path $bin)) { throw "binary not found: $bin" }

# 代表性命令：覆盖 NoContext(零配置/零装配)、one-shot FullContext(Off 分片)、快速退出
$cases = @{
    'version'   = @('--version')
    'help'      = @('--no-banner', 'help')
    'configpath'= @('--no-banner', 'config', 'path')
    'completions'= @('--no-banner', 'completions', 'bash')
    'cost'      = @('--no-banner', '--json', 'cost')
    'questlist' = @('--no-banner', 'quest', 'list')
    'doctor'    = @('--no-banner', 'doctor')
}

function Get-Median($arr) {
    $s = $arr | Sort-Object
    $n = $s.Count
    if ($n -eq 0) { return 0 }
    if ($n % 2 -eq 1) { return $s[[int](($n - 1) / 2)] }
    return ($s[$n / 2 - 1] + $s[$n / 2]) / 2
}
function Get-P95($arr) {
    $s = $arr | Sort-Object
    $idx = [Math]::Ceiling(0.95 * $s.Count) - 1
    if ($idx -lt 0) { $idx = 0 }
    return $s[$idx]
}

$results = @()
foreach ($name in $cases.Keys) {
    $args = $cases[$name]
    $times = @()
    $env:RUST_LOG = 'error'   # 测墙钟时静音日志，减少 stderr 干扰
    for ($i = 0; $i -lt $Iterations; $i++) {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $bin @args 2>$null 1>$null
        $sw.Stop()
        $times += $sw.Elapsed.TotalMilliseconds
    }
    $results += [pscustomobject]@{
        Command = ($args -join ' ')
        Median_ms = [math]::Round((Get-Median $times), 2)
        P95_ms    = [math]::Round((Get-P95 $times), 2)
    }
}

Write-Host "`n=== 启动墙钟（$profile, $Iterations iter, RUST_LOG=error）===" -ForegroundColor Green
$results | Format-Table -AutoSize

Write-Host "=== 分段（RUST_LOG=info, quest list / help 各一次）===" -ForegroundColor Green
$env:RUST_LOG = 'info'
& $bin --no-banner quest list 2>&1 | Select-String 'startup phase'
& $bin --no-banner help 2>&1 | Select-String 'startup phase'
$env:RUST_LOG = $null
