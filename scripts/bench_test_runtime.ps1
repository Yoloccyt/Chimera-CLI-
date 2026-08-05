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
        $data = Get-Content $ReportPath -Raw | ConvertFrom-Json
        $rows = @()
        foreach ($t in $data.'rust-tests') {
            if ($t.status -ne "PASS") { continue }
            if (-not $t.elapsed) { continue }
            $rows += [PSCustomObject]@{
                binary = $t.binary
                test   = $t.name
                elapsed = $t.elapsed
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
        }
        "full" {
            $profile = "default"
            $scale = "1.0"
            $bp = "60"
            $jsonPath = Join-Path $ReportDir "nextest-full.json"
            $extraArgs = @()
        }
        "stress" {
            $profile = "stress"
            $scale = "1.0"
            $bp = "60"
            $jsonPath = Join-Path $ReportDir "nextest-stress.json"
            $extraArgs = @()
        }
    }

    Write-Host "=== 模式: $Mode | profile: $profile | scale: $scale | bp: ${bp}s ==="
    Write-Host "    输出: $jsonPath"

    $env:CHIMERA_TEST_TIMEOUT_SCALE = $scale
    $env:CHIMERA_BACKPRESSURE_SECS = $bp

    $startTs = Get-Date
    $logFile = Join-Path $ReportDir "nextest-$Mode.stdout.log"
    $errFile = Join-Path $ReportDir "nextest-$Mode.stderr.log"
    & cargo nextest run --profile $profile --no-fail-fast --message-format json @extraArgs *>$errFile | Tee-Object -FilePath $logFile | Out-Null
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
