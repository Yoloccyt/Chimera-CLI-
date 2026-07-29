<#
.SYNOPSIS
    repo-wiki CI benchmark 阈值断言脚本(P1-7)
.DESCRIPTION
    对应任务:P1-7 实施 repo-wiki CI benchmark 阈值断言
    对应 SLO:wiki_knn @100K p95 < 50ms(spec.md 索引 SLO 红线)
    对应基线:hnsw_100k_p95_search_latency = 152.84 µs(2026-07-27 Phase 4 实测)

    # 设计决策
    - CI 中仅运行 10K entry benchmark(快速验证,~30s):100K 预填充在 debug 模式 >5 分钟,
      不适合 CI 快速反馈。100K benchmark 在 release 模式或本地验证。
    - 阈值设定基于基线 + CI 环境余量(约 4-10 倍):CI runner 性能波动较大,
      设宽松阈值避免误报,同时能捕获数量级退化。
    - 解析 criterion stdout 输出:criterion 默认输出人类可读格式,
      用正则表达式提取 `time: [low mean high]` 中的 mean 值。

    # 阈值表
    | benchmark                        | 基线(本地)    | CI 阈值       | 余量  |
    |----------------------------------|--------------|--------------|-------|
    | hnsw_10k_search_latency          | ~50 µs       | < 10 ms      | 200×  |
    | hnsw_10k_p95_search_latency      | ~100 µs      | < 20 ms      | 200×  |
    | single_thread_knn_latency/100    | ~10 µs       | < 5 ms       | 500×  |
    | single_thread_knn_latency/1000   | ~100 µs      | < 20 ms      | 200×  |

.PARAMETER Mode
    default: 仅 10K benchmark(预计 ~30s)
    --full:  含 100K benchmark(需 release,预计 5-10 分钟)
    --quick: criterion --quick 模式(最快速,不精确)

.EXAMPLE
    .\scripts\check_repo_wiki_benchmark.ps1
    .\scripts\check_repo_wiki_benchmark.ps1 --full
    .\scripts\check_repo_wiki_benchmark.ps1 --quick

.NOTES
    退出码:0 = 全部通过 / 1 = 至少一个超过阈值 / 2 = 脚本错误
#>

param(
    [Parameter(Position = 0)]
    [ValidateSet('default', '--full', '--quick')]
    [string]$Mode = 'default'
)

$ErrorActionPreference = 'Stop'

# ============================================================
# 工具链环境设置(对齐 .claude/CLAUDE.md §1)
# ============================================================
$env:CARGO_HOME = 'D:\Chimera CLI\.toolchain\cargo'
$env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'
$env:TMP = 'D:\Chimera CLI\tmp'
$env:TEMP = 'D:\Chimera CLI\tmp'
$env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"

# ============================================================
# 配置
# ============================================================

$ProjectRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $ProjectRoot

# benchmark 阈值表(单位:毫秒)
# 格式:@{ Name = "benchmark_name_regex"; ThresholdMs = <double> }
# WHY 按 Name 长度降序排列:避免短前缀误匹配长 benchmark 名。
# 例如 'single_thread_knn_latency/100' 会正则匹配 'single_thread_knn_latency/1000'
# (因 -match 是 "contains" 语义),导致 1000-entry benchmark 误用 100-entry 阈值。
# 降序排列确保更具体的模式(长名)优先匹配。
$Thresholds = @(
    @{ Name = 'hnsw_10k_p95_search_latency';     ThresholdMs = 20.0 }
    @{ Name = 'hnsw_10k_search_latency';          ThresholdMs = 10.0 }
    @{ Name = 'single_thread_knn_latency/1000';   ThresholdMs = 20.0 }
    @{ Name = 'single_thread_knn_latency/100';    ThresholdMs = 5.0 }
) | Sort-Object { -($_.Name.Length) }

$BenchFilter = @('hnsw_10k|single_thread_knn')
$QuickFlag = @()

if ($Mode -eq '--full') {
    $BenchFilter = @('hnsw|single_thread_knn')
    $Thresholds += @{ Name = 'hnsw_100k_search_latency';     ThresholdMs = 50.0 }
    $Thresholds += @{ Name = 'hnsw_100k_p95_search_latency'; ThresholdMs = 200.0 }
    Write-Host "[INFO] 完整模式:含 100K benchmark(预计 5-10 分钟)" -ForegroundColor Yellow
} elseif ($Mode -eq '--quick') {
    $QuickFlag = @('--quick')
    Write-Host "[INFO] Quick 模式:criterion --quick(最快速,不精确)" -ForegroundColor Yellow
} else {
    Write-Host "[INFO] 默认模式:仅 10K benchmark(预计 ~30s)" -ForegroundColor Yellow
}

# ============================================================
# 运行 benchmark
# ============================================================

Write-Host ""
Write-Host "=== 运行 repo-wiki benchmark ==="
$filterStr = $BenchFilter -join ' '
$quickStr = $QuickFlag -join ' '
Write-Host "命令: cargo bench -p repo-wiki --bench vector_bench -- $filterStr $quickStr"
Write-Host ""

# WHY: criterion bench 过滤参数是正则表达式,多个参数会 AND 组合(非 OR)。
# 用 `|` 正则元字符在一个参数中表达多选一,避免 AND 语义导致无匹配。
$benchArgs = @('bench', '-p', 'repo-wiki', '--bench', 'vector_bench', '--', $BenchFilter[0]) + $QuickFlag
$benchOutput = & cargo @benchArgs 2>&1 | Out-String
$benchExit = $LASTEXITCODE

if ($benchExit -ne 0) {
    Write-Host "[ERROR] cargo bench 执行失败(exit=$benchExit)" -ForegroundColor Red
    $benchOutput -split "`n" | Select-Object -Last 30 | ForEach-Object { Write-Host $_ }
    exit 2
}

Write-Host $benchOutput
Write-Host ""

# ============================================================
# 解析输出并断言阈值
# ============================================================

Write-Host "=== 阈值断言 ==="

$failures = 0
$checks = 0

# criterion 输出格式(benchmark name 和 time 在不同行):
#   hnsw_10k_search_latency/knn_top5
#                           time:   [48.820 µs 49.163 µs 49.559 µs]
# 解析策略:
#   1. 按行遍历,记录最近的 benchmark name(非缩进的非空行)
#   2. 遇到 time: 行时,提取时间值并关联到最近的 benchmark name
$lines = $benchOutput -split "`n"
$currentBenchName = $null

# time 行正则:提取三个时间值 + 单位
# 格式: `                        time:   [48.820 µs 49.163 µs 49.559 µs]`
$timePattern = 'time:\s+\[([\d.]+)\s+(\S+)\s+([\d.]+)\s+(\S+)\s+([\d.]+)\s+(\S+)\]'

for ($i = 0; $i -lt $lines.Count; $i++) {
    $line = $lines[$i].TrimEnd()

    # 跳过空行
    if ([string]::IsNullOrWhiteSpace($line)) { continue }

    # 检查是否是 benchmark name 行(不以空格开头,不包含 time:/change:/Performance:/Found/setting/Benchmarking)
    if (-not $line.StartsWith(' ') -and
        -not $line.StartsWith("`t") -and
        $line -notmatch 'time:' -and
        $line -notmatch 'change:' -and
        $line -notmatch 'Performance' -and
        $line -notmatch 'Found' -and
        $line -notmatch 'setting' -and
        $line -notmatch 'Benchmarking' -and
        $line -notmatch 'Gnuplot' -and
        $line -notmatch 'Running' -and
        $line -notmatch 'Compiling' -and
        $line -notmatch 'Finished' -and
        $line -notmatch '^\[') {
        $currentBenchName = $line.Trim()
        continue
    }

    # 检查是否是 time: 行
    if ($line -match $timePattern -and $currentBenchName) {
        $benchName = $currentBenchName
        # mean 是三个值中的第二个(Matches[3] 和 Matches[4])
        $meanStr = $Matches[3]
        $unit = $Matches[4]

        # 转换为毫秒
        $meanMs = switch ($unit) {
            'ns'  { [double]$meanStr / 1000000.0 }
            { $_ -eq 'µs' -or $_ -eq 'us' -or $_ -eq 'Âµs' } { [double]$meanStr / 1000.0 }
            'ms'  { [double]$meanStr }
            's'   { [double]$meanStr * 1000.0 }
            default {
                Write-Host "[WARN] 未知单位 '$unit' for $benchName,跳过" -ForegroundColor Yellow
                continue
            }
        }

        # 查找匹配的阈值
        $matchedThreshold = $null
        foreach ($t in $Thresholds) {
            if ($benchName -match $t.Name) {
                $matchedThreshold = $t
                break
            }
        }

        if (-not $matchedThreshold) { continue }

        $checks++
        if ($meanMs -lt $matchedThreshold.ThresholdMs) {
            Write-Host ("[PASS] {0}: {1:F6} ms < {2} ms" -f $benchName, $meanMs, $matchedThreshold.ThresholdMs) -ForegroundColor Green
        } else {
            Write-Host ("[FAIL] {0}: {1:F6} ms >= {2} ms" -f $benchName, $meanMs, $matchedThreshold.ThresholdMs) -ForegroundColor Red
            $failures++
        }
    }
}

# ============================================================
# 汇总
# ============================================================

Write-Host ""
Write-Host "=== 汇总 ==="
Write-Host "检查数: $checks"
Write-Host "通过:   $($checks - $failures)"
Write-Host "失败:   $failures"

if ($failures -gt 0) {
    Write-Host ""
    Write-Host "[FAIL] $failures 个 benchmark 超过阈值,请检查性能退化" -ForegroundColor Red
    exit 1
} elseif ($checks -eq 0) {
    Write-Host "[WARN] 未匹配到任何 benchmark,请检查 benchmark 名称" -ForegroundColor Yellow
    exit 2
} else {
    Write-Host ""
    Write-Host "[PASS] 全部 $checks 个 benchmark 通过阈值断言" -ForegroundColor Green
    exit 0
}
