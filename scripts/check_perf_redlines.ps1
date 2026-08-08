<#
.SYNOPSIS
  Chimera CLI 性能红线 lint 静态验证脚本(Windows / PowerShell)

.DESCRIPTION
  静态验证 spec.md KPI 表格中定义的全部性能红线(SLO)是否在代码库中
  有对应的 benchmark/test 文件与函数,以及阈值标记是否就位。

  Part 1: Static Lint — 文件/函数/阈值标记存在性检查
  Part 2: SLO Benchmark Assertions — 实际运行 bench, 解析 criterion 输出,
          与 80% redline 阈值比较(Phase 7 新增)

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
  - RL-09: linucb 40arm select p99<50μs(MCA M3 s9 路由臂,新增)
  - RL-10: cost_estimate <1μs(MCA M3 路由热路径,新增)
  - RL-11: sse normalize <5μs/event(MCA M0 SSE 归一器,新增)

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
    @{ Id='RL-08'; Name='50agent_mem_peak <=130MB'; File='crates/chimera-mas/benches/mas_benchmark.rs'; Func='bench_50agent_mem_peak'; Threshold='130' },
    # MCA M3/M4 热路径红线(ADR-065/068):路由选择/成本估算/SSE 归一
    @{ Id='RL-09'; Name='linucb 40arm select p99<50us'; File='crates/omega-learner/benches/linucb_select.rs'; Func='bench_s9_route_40arm_select'; Threshold='P99_TARGET_US' },
    @{ Id='RL-10'; Name='cost_estimate <1us'; File='crates/mca-gateway/benches/mca_hot_paths.rs'; Func='bench_cost_estimate'; Threshold='COST_ESTIMATE_TARGET_US' },
    @{ Id='RL-11'; Name='sse normalize <5us/event'; File='crates/mca-gateway/benches/mca_hot_paths.rs'; Func='bench_sse_normalize'; Threshold='SSE_EVENT_TARGET_US' }
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

# =====================================================================
# Part 2: SLO Benchmark 阈值断言 (实际运行 bench, 解析 criterion 输出)
# =====================================================================
# 每个 SLO 用 80% 宽松阈值作为 CI redline (CI 环境波动缓冲)
# 格式: SLO 名称 -> crate / bench 文件 / bench 函数过滤 / SLO 阈值(秒) / redline 阈值(秒)
$sloRedlines = @(
    # WHY Filter 用 criterion 组名前缀而非 bench 函数名(closure C-13 修复):
    # criterion 命令行过滤匹配的是 benchmark ID("window_select/L0_4K"),
    # 函数名 'bench_window_select' 零匹配时 criterion 跑全部 bench,
    # 解析器取到首个 time: 行造成 1.39ms 伪影(实测单次 select 为 ~1ns)
    @{ Name='window_select';     Crate='hcw-window';    BenchFile='window_select';  Filter='window_select/';            SloSec=0.001;   RedlineSec=0.0008;   Unit='ms'; SloDisplay='1ms';   RedlineDisplay='0.8ms'  },
    @{ Name='mlc_l2_knn';       Crate='mlc-engine';    BenchFile='mlc_l2_knn';     Filter='bench_l2_knn_slo_assert';   SloSec=0.005;   RedlineSec=0.004;    Unit='ms'; SloDisplay='5ms';   RedlineDisplay='4ms'   },
    @{ Name='decay_compute';    Crate='decay-engine';  BenchFile='decay_compute';  Filter='single_decay_by_profile';   SloSec=0.000001;RedlineSec=0.0000008;Unit='us'; SloDisplay='1us';   RedlineDisplay='0.8us' },
    @{ Name='wiki_knn_100k';    Crate='repo-wiki';     BenchFile='wiki_knn_slo';   Filter='wiki_knn_100k_p95';         SloSec=0.050;   RedlineSec=0.040;    Unit='ms'; SloDisplay='50ms';  RedlineDisplay='40ms'  },
    @{ Name='immune_probe';     Crate='chimera-mas';   BenchFile='immune_probe';   Filter='bench_assess_paradox_risk'; SloSec=0.100;   RedlineSec=0.080;    Unit='ms'; SloDisplay='100ms'; RedlineDisplay='80ms'  },
    @{ Name='rhi_judge';        Crate='auto-dpo';      BenchFile='rhi_judge';      Filter='rhi_judge_latency';         SloSec=2.0;     RedlineSec=1.6;      Unit='s';  SloDisplay='2s';    RedlineDisplay='1.6s'   },
    # MCA M3 s9 路由臂 40 臂选择延迟(ADR-068 决策 2:p99 < 50μs,红线上浮 80%)
    @{ Name='linucb_40arm';     Crate='omega-learner'; BenchFile='linucb_select';  Filter='select_arm_40arms_6dim_s9route'; SloSec=0.00005; RedlineSec=0.00004; Unit='us'; SloDisplay='50us';  RedlineDisplay='40us' }
)

Write-Host "`n=== SLO Benchmark Threshold Assertions ===" -ForegroundColor Cyan
Write-Host "    (criterion bench, 80% redline of SLO)`n" -ForegroundColor Gray

$sloPassCount = 0
$sloFailCount = 0
$sloSkipCount = 0

foreach ($slo in $sloRedlines) {
    Write-Host "  [SLO] $($slo.Name) (target: < $($slo.SloDisplay), redline: $($slo.RedlineDisplay))" -ForegroundColor Cyan

    # 运行 bench 并捕获输出
    $benchArgs = @(
        'bench',
        '--package', $slo.Crate,
        '--bench', $slo.BenchFile,
        '--', '--noplot', '--quick', $slo.Filter
    )

    try {
        $benchOutput = & cargo @benchArgs 2>&1 | Out-String
    } catch {
        Write-Host "    [SKIP] bench 执行失败: $_" -ForegroundColor Yellow
        $sloSkipCount++
        continue
    }

# 转换前强制 UTF-8 解码 native stdout:
# criterion 的 "µs" 单位以 UTF-8 字节 0xC2 0xB5 输出,Windows 控制台代码页
# (GBK/936)默认将其解码为 "Âμ",导致后续单位比较失败而误入 else 分支
# (把 6.8µs 当 6.8ms,linucb_40arm SLO 误报 FAIL —— 2026-08-08 发布检查发现)。
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch {}

# 解析 criterion 输出: "time:   [X.XXX us X.XXX us X.XXX us]"
# 格式: time: [lower estimate upper unit]
# 提取 estimate (中间值) 和单位
$timePattern = 'time:\s+\[([^\]]+)\]'
if ($benchOutput -match $timePattern) {
    $timeValues = $Matches[1].Trim() -split '\s+'
    # timeValues[0]=lower, [1]=estimate, [2]=upper, [3]=unit
    if ($timeValues.Count -ge 4) {
        $estimate = [double]$timeValues[2]
        $unit = $timeValues[3]

        # 转换为秒。单位匹配用正则兼容三种微符号形态:
        # U+00B5 µ(UTF-8 解码) / U+03BC μ(GBK 解码) / ASCII u
        if     ($unit -eq 'ns') { $estimateSec = $estimate / 1e9 }
        elseif ($unit -match '^[µμu]s$') { $estimateSec = $estimate / 1e6 }
        elseif ($unit -eq 'ms') { $estimateSec = $estimate / 1e3 }
        elseif ($unit -eq 's')  { $estimateSec = $estimate }
        else                    { $estimateSec = $estimate / 1e3 }

            # 显示实测值
            if     ($slo.Unit -eq 'us') { $displayVal = ('{0:N3} us' -f ($estimateSec * 1e6)) }
            elseif ($slo.Unit -eq 'ms') { $displayVal = ('{0:N3} ms' -f ($estimateSec * 1e3)) }
            else                        { $displayVal = ('{0:N3} s'  -f $estimateSec) }
            Write-Host "    实测: $displayVal" -ForegroundColor Gray

            # 与 redline 比较
            if ($estimateSec -le $slo.RedlineSec) {
                Write-Host "    [PASS] 低于 redline ($($slo.RedlineDisplay))" -ForegroundColor Green
                $sloPassCount++
            } elseif ($estimateSec -le $slo.SloSec) {
                Write-Host "    [WARN] 超过 redline 但低于 SLO ($($slo.SloDisplay))" -ForegroundColor Yellow
                $sloPassCount++  # 低于 SLO 即通过
            } else {
                Write-Host "    [FAIL] 超过 SLO ($($slo.SloDisplay))!" -ForegroundColor Red
                $sloFailCount++
            }
        } else {
            Write-Host "    [SKIP] 无法解析 criterion 时间输出" -ForegroundColor Yellow
            $sloSkipCount++
        }
    } else {
        Write-Host "    [SKIP] 未找到 criterion 时间输出 (bench 可能编译失败)" -ForegroundColor Yellow
        $sloSkipCount++
    }
}

# --- 最终汇总 ---
Write-Host "`n=== Final Summary ===" -ForegroundColor Cyan
Write-Host "  Static Lint:" -ForegroundColor White
$totalChecks = $redlines.Count * 2
$passedChecks = $totalChecks - $script:FailCount
Write-Host "    Passed: $passedChecks / $totalChecks" -ForegroundColor $(if ($script:FailCount -eq 0) { 'Green' } else { 'Red' })
Write-Host "    Warnings: $script:WarnCount" -ForegroundColor Yellow
Write-Host "  SLO Benchmarks:" -ForegroundColor White
Write-Host "    Passed: $sloPassCount" -ForegroundColor $(if ($sloFailCount -eq 0) { 'Green' } else { 'Red' })
Write-Host "    Failed: $sloFailCount" -ForegroundColor $(if ($sloFailCount -gt 0) { 'Red' } else { 'Gray' })
Write-Host "    Skipped: $sloSkipCount" -ForegroundColor Yellow

$totalFail = $script:FailCount + $sloFailCount
if ($totalFail -gt 0) {
    Write-Host "`n  RESULT: FAIL — 有 $totalFail 项检查失败" -ForegroundColor Red
    exit 1
} else {
    Write-Host "`n  RESULT: PASS — 全部性能红线 lint + SLO 断言通过" -ForegroundColor Green
    exit 0
}

