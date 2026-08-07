# =============================================================================
# bench_test_runtime.ps1 — P9-T2 测试运行时间基准采集与对比 (Windows)
# =============================================================================
# Purpose: 统一封装 nextest 三档 profile (ci-fast / full / stress) 测试运行,
#          产出 JSON 格式基准数据,支撑 P9-T2 优化前后对比。
# Scope:   P9-T2 优化实施,作为 docs/reports/p9-t2-test-runtime-optimization.md
#          数据源。
# Author:  P9-T2 implementation (test-runtime-optimization)
# Refs:    .trae/specs/p9-t2-test-runtime-optimization/{spec,tasks,checklist}.md
#
# Usage:
#   pwsh scripts/bench_test_runtime.ps1 fast      # CI fast 档
#   pwsh scripts/bench_test_runtime.ps1 full      # 全量
#   pwsh scripts/bench_test_runtime.ps1 stress    # 压测
#   pwsh scripts/bench_test_runtime.ps1 all       # 三档全跑
#
# Environment: 与 .sh 镜像一致,详细见 .sh 头部注释。
# =============================================================================

[CmdletBinding()]
param(
    [Parameter(Position=0)]
    [ValidateSet("fast","full","stress","all","report")]
    [string]$Mode = "fast",

    [Parameter(Position=1)]
    [string]$Target = ""
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot  = Resolve-Path (Join-Path $ScriptDir "..")
Set-Location $RepoRoot

$ReportDir = if ($env:REPORT_DIR) { $env:REPORT_DIR } else { Join-Path $RepoRoot "docs/reports" }
if (-not (Test-Path $ReportDir)) { New-Item -ItemType Directory -Path $ReportDir -Force | Out-Null }

function Now-Iso { (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ") }

function Parse-TotalElapsed {
    param([string]$ReportPath)
    if (-not (Test-Path $ReportPath)) { return "N/A" }
    try {
        $data = Get-Content $ReportPath -Raw | ConvertFrom-Json
        if ($data.'started-at' -and $data.'finished-at') {
            $s = [datetime]::Parse($data.'started-at').ToUniversalTime()
            $f = [datetime]::Parse($data.'finished-at').ToUniversalTime()
            return "{0:N2}" -f ($f - $s).TotalSeconds
        }
    } catch {}
    return "N/A"
}

function Parse-TopTests {
    param([string]$ReportPath, [int]$TopN = 10)
    if (-not (Test-Path $ReportPath)) { return "[]" }
    try {
        # nextest libtest-json-plus 输出为 JSON Lines:
        # {"type":"test","event":"ok","name":"...","exec_time":<secs>}
        # 2026-08-07 ultra-plan:修复原单 JSON 文档假设(实际为 JSONL)。
        $rows = @()
        foreach ($line in Get-Content $ReportPath) {
            if ($line -match '"type":"test","event":"ok"') {
                $t = $line | ConvertFrom-Json
                if ($null -eq $t.exec_time) { continue }
                $rows += [PSCustomObject]@{
                    binary  = ($t.name -split '\$')[0]
                    test    = $t.name
                    elapsed = [double]$t.exec_time
                }
            }
        }
        $top = $rows | Sort-Object elapsed -Descending | Select-Object -First $TopN
        $json = $top | ForEach-Object {
            [PSCustomObject]@{
                binary        = $_.binary
                test          = $_.test
                elapsed_secs  = [math]::Round($_.elapsed, 3)
            }
        } | ConvertTo-Json -Compress
        return $json
    } catch {
        return "[]"
    }
}

function Write-Metadata {
    param(
        [string]$Mode,
        [string]$Elapsed,
        [string]$TopJson
    )
    $ts = Now-Iso
    $hostName = $env:COMPUTERNAME
    $cpuCount = (Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors
    $rustcVersion = (& rustc --version 2>$null) -join ""
    $nextestVersion = (& cargo nextest --version 2>$null | Select-Object -First 1) -join ""

    $scale = if ($env:CHIMERA_TEST_TIMEOUT_SCALE) { $env:CHIMERA_TEST_TIMEOUT_SCALE } else { "1.0" }
    $bp    = if ($env:CHIMERA_BACKPRESSURE_SECS) { $env:CHIMERA_BACKPRESSURE_SECS } else { "60" }

    $metadata = [PSCustomObject]@{
        mode = $Mode
        timestamp = $ts
        host = $hostName
        cpu_count = $cpuCount
        rustc_version = $rustcVersion
        nextest_version = $nextestVersion
        chimera_test_timeout_scale = $scale
        chimera_backpressure_secs = $bp
        total_elapsed_secs = $Elapsed
        top_tests = $TopJson | ConvertFrom-Json
    }

    $outPath = Join-Path $ReportDir "p9-t2-baseline-$Mode.json"
    $metadata | ConvertTo-Json -Depth 6 | Set-Content -Path $outPath -Encoding UTF8
    Write-Host "  -> 写入 $outPath"
}

function Run-Mode {
    param([string]$Mode)
    $profile = ""
    $scale = "1.0"
    $bp = "60"
    $extraArgs = @()
    $jsonPath = ""

    switch ($Mode) {
        "fast" {
            $profile = "ci-fast"
            $scale = "0.1"
            $bp = "5"
            $jsonPath = Join-Path $ReportDir "nextest-fast.json"
            $extraArgs = @("--exclude","chimera-e2e-tests")
            # ultra-plan:与 ci.yml 对齐,mcp-mesh 1000 事务测试在快轨收敛到 100
            # (缺省 1000 spec 语义由 full 档/完整验收档承担)
            $env:CHIMERA_MCP_TXN_COUNT = "100"
        }
        "full" {
            $profile = "default"
            $scale = "1.0"
            $bp = "60"
            $jsonPath = Join-Path $ReportDir "nextest-full.json"
            $extraArgs = @()
            # ultra-plan:full 档恢复 spec 默认(1000 次事务全量验证)
            Remove-Item Env:CHIMERA_MCP_TXN_COUNT -ErrorAction SilentlyContinue
        }
        "stress" {
            $profile = "stress"
            $scale = "1.0"
            $bp = "60"
            $jsonPath = Join-Path $ReportDir "nextest-stress.json"
            # ultra-plan:与 stress.yml 对齐 —— 仅跑 stress binary(
            # 1000-iter 压测标了 #[ignore],需 --run-ignored all 显式包含;
            # 原实现 extraArgs 为空会误跑全 workspace 常规测试)。
            $extraArgs = @("-E", "binary(/stress/)", "--run-ignored", "all")
        }
    }

    Write-Host "=== 模式: $Mode | profile: $profile | scale: $scale | bp: ${bp}s ==="
    Write-Host "    输出: $jsonPath"

    $env:CHIMERA_TEST_TIMEOUT_SCALE = $scale
    $env:CHIMERA_BACKPRESSURE_SECS = $bp
    # ultra-plan:libtest-json 为 nextest 实验特性,需显式开启(2026-08-07 实测缺省报错)
    $env:NEXTEST_EXPERIMENTAL_LIBTEST_JSON = '1'

    $startTs = Get-Date
    $logFile = Join-Path $ReportDir "nextest-$Mode.stdout.log"
    $errFile = Join-Path $ReportDir "nextest-$Mode.stderr.log"
    # ultra-plan 修复:原 `*>` 把所有流重定向后管道为空,Tee-Object 收不到数据;
    # 改为 `2>`(stderr 进文件),stdout(JSON Lines)走管道进 logFile。
    # 另:--exclude 在 nextest 0.9.143 需搭配 --workspace;--message-format json 非法,
    # 修正为 libtest-json-plus。
    & cargo nextest run --workspace --profile $profile --no-fail-fast --message-format libtest-json-plus @extraArgs 2>$errFile | Tee-Object -FilePath $logFile | Out-Null
    $endTs = Get-Date
    $wallSecs = ($endTs - $startTs).TotalSeconds

    # 兜底:把 stdout log 复制为 json 供解析
    if (-not (Test-Path $jsonPath) -or (Get-Item $jsonPath).Length -eq 0) {
        Copy-Item $logFile $jsonPath -Force
    }

    $elapsed = Parse-TotalElapsed $jsonPath
    if ($elapsed -eq "N/A") { $elapsed = "{0:N2}" -f $wallSecs }

    $topJson = Parse-TopTests $jsonPath 10
    Write-Host "  -> 耗时: $elapsed s (wall: $wallSecs s)"
    Write-Metadata -Mode $Mode -Elapsed $elapsed -TopJson $topJson
}

switch ($Mode) {
    "fast"   { Run-Mode "fast" }
    "full"   { Run-Mode "full" }
    "stress" { Run-Mode "stress" }
    "all" {
        Run-Mode "fast"
        Run-Mode "full"
        Run-Mode "stress"
    }
    "report" {
        if (-not $Target) { Write-Error "report 模式需指定 metadata JSON 路径"; exit 2 }
        Get-Content $Target -Raw | ConvertFrom-Json | ConvertTo-Json -Depth 6
    }
}
