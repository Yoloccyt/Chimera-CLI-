<#
.SYNOPSIS
  Chimera CLI 性能红线 lint 静态验证脚本(Windows / PowerShell)

.DESCRIPTION
  静态验证 spec.md KPI 表格中定义的全部性能红线(SLO)是否在代码库中
  有对应的 benchmark/test 文件与函数,以及阈值标记是否就位。

  检查项(每条红线 3 项):
  1. 文件存在 — benchmark/test 文件路径有效
  2. 函数存在 — `fn <function_name>` 在文件中可匹配
  3. 阈值标记 — 阈值字符串(阈值常量名或阈值描述)在文件中可匹配

  红线全集(8 条,来自 spec.md KPI 表格"性能 SLO 分层"):
  - RL-01: window_select <1ms(内环本地,维持)
  - RL-02: mlc_l2_knn <5ms(内环本地,维持)
  - RL-03: decay <1μs(内环本地,维持,阈值仅在 spec 中,代码无断言)
  - RL-04: wiki_knn @1000 <10ms(索引,既有)
  - RL-05: wiki_knn @10K p95<50ms(索引,P2-W8.1.3)
  - RL-06: wiki_knn @100K p95<50ms(索引,P2-W8.3.1 新增)
  - RL-07: 跨膜事件投递 p95<10ms(跨膜,P1-W2 新增)
  - RL-08: 50agent_mem_peak ≤130MB(MAS,维持)

.NOTES
  退出码:0=全部通过, 1=有检查项失败
  使用方式:pwsh scripts/check_perf_redlines.ps1
  对应任务:P2-W8.3.2 红线 lint CI 化
  权威源:spec.md KPI 表格(nexus-omega-v5-implementation-plan/spec.md L29)
#>

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$script:FailCount = 0
$script:WarnCount = 0

function Write-Check {
    param([string]$Name, [bool]$Pass, [string]$Detail = '')
    $status = if ($Pass) { 'PASS' } else { 'FAIL' }
    $color = if ($Pass) { 'Green' } else { 'Red' }
    Write-Host "  [$status] $Name" -ForegroundColor $color
    if ($Detail) { Write-Host "         $Detail" -ForegroundColor Gray }
    if (-not $Pass) { $script:FailCount++ }
}

function Write-Warn {
    param([string]$Name, [string]$Detail = '')
    Write-Host "  [WARN] $Name" -ForegroundColor Yellow
    if ($Detail) { Write-Host "         $Detail" -ForegroundColor Gray }
    $script:WarnCount++
}

Write-Host "`n=== Chimera CLI Performance Red Line Lint ===" -ForegroundColor Cyan
Write-Host "    (spec.md KPI: 性能 SLO 分层, 8 red lines)`n" -ForegroundColor Gray

$projectRoot = Join-Path $PSScriptRoot '..' | Resolve-Path

# --- 红线全集(spec.md KPI 表格"性能 SLO 分层") ---
# 每条红线:Id / Name / File(相对项目根) / Func(bench/test 函数名) / Threshold(阈值标记,空=跳过)
$redlines = @(
    @{ Id='RL-01'; Name='window_select <1ms'; File='crates/chimera-mas/benches/mas_benchmark.rs'; Func='bench_window_select'; Threshold='1ms' },
    @{ Id='RL-02'; Name='mlc_l2_knn <5ms'; File='crates/chimera-mas/benches/mas_benchmark.rs'; Func='bench_mlc_l2_knn_top10_4096'; Threshold='5ms' },
    @{ Id='RL-03'; Name='decay <1us'; File='crates/decay-engine/benches/decay_bench.rs'; Func='single_decay_step_latency'; Threshold='' },
    @{ Id='RL-04'; Name='wiki_knn @1000 <10ms'; File='crates/repo-wiki/benches/vector_bench.rs'; Func='single_thread_knn_latency'; Threshold='LARGE_SIZE' },
    @{ Id='RL-05'; Name='wiki_knn @10K p95<50ms'; File='crates/repo-wiki/tests/hnsw_p95_test.rs'; Func='test_hnsw_10k_p95_below_50ms'; Threshold='P95_THRESHOLD_MS' },
    @{ Id='RL-06'; Name='wiki_knn @100K p95<50ms'; File='crates/repo-wiki/tests/hnsw_p95_test.rs'; Func='test_hnsw_100k_p95_below_50ms'; Threshold='ENTRY_COUNT_100K' },
    @{ Id='RL-07'; Name='membrane delivery p95<10ms'; File='crates/event-bus/benches/membrane_delivery_bench.rs'; Func='membrane_e2e_delivery'; Threshold='10ms' },
    @{ Id='RL-08'; Name='50agent_mem_peak <=130MB'; File='crates/chimera-mas/benches/mas_benchmark.rs'; Func='bench_50agent_mem_peak'; Threshold='130' }
)

foreach ($rl in $redlines) {
    $filePath = Join-Path $projectRoot $rl.File
    $relPath = $rl.File

    Write-Host "`n  [$($rl.Id)] $($rl.Name)" -ForegroundColor Cyan

    # 检查 1: 文件存在
    $fileExists = Test-Path $filePath
    Write-Check "$($rl.Id).1 file exists ($relPath)" $fileExists
    if (-not $fileExists) {
        Write-Host "         跳过函数/阈值检查(文件不存在)" -ForegroundColor Gray
        continue
    }

    $content = Get-Content $filePath -Raw -Encoding UTF8

    # 检查 2: 函数存在
    $funcPattern = "fn $($rl.Func)"
    $funcExists = $content -match [regex]::Escape($funcPattern)
    Write-Check "$($rl.Id).2 function 'fn $($rl.Func)' exists" $funcExists

    # 检查 3: 阈值标记存在(空=跳过,spec-only 红线)
    if ($rl.Threshold -ne '') {
        $thresholdExists = $content -match [regex]::Escape($rl.Threshold)
        Write-Check "$($rl.Id).3 threshold marker '$($rl.Threshold)' exists" $thresholdExists
    } else {
        Write-Warn "$($rl.Id).3 threshold marker" "阈值仅在 spec 中定义,代码无显式断言(spec-only)"
    }
}

# --- 汇总 ---
Write-Host "`n=== Summary ===" -ForegroundColor Cyan
$totalChecks = $redlines.Count * 2  # 每条红线至少 2 项(文件 + 函数)
$passedChecks = $totalChecks - $script:FailCount
Write-Host "  Passed: $passedChecks / $totalChecks" -ForegroundColor $(if ($script:FailCount -eq 0) { 'Green' } else { 'Red' })
Write-Host "  Warnings: $script:WarnCount" -ForegroundColor Yellow
Write-Host "  Failed: $script:FailCount" -ForegroundColor $(if ($script:FailCount -gt 0) { 'Red' } else { 'Gray' })

if ($script:FailCount -gt 0) {
    Write-Host "`n  RESULT: FAIL — 有 $script:FailCount 项检查失败" -ForegroundColor Red
    exit 1
} else {
    Write-Host "`n  RESULT: PASS — 全部性能红线 lint 通过" -ForegroundColor Green
    exit 0
}
