#Requires -Version 5.1
# ============================================================
# Chimera CLI 启动延迟基线检查脚本（P2-7 hyperfine 落地）
#
# 用法:
#   .\scripts\check_cli_startup.ps1              # 默认: 测量 + 断言 (runs 10)
#   .\scripts\check_cli_startup.ps1 -Runs 20     # 指定轮数(基线建立建议 20)
#   .\scripts\check_cli_startup.ps1 -ThresholdMs 100  # 覆盖阈值
#   .\scripts\check_cli_startup.ps1 -SkipAssert  # 仅测量输出,不断言(观测模式)
#
# WHY: 为 CLI 启动延迟建立可重复执行的机械基线——--version/--help/help
#      三命令的进程启动 + clap 解析 + tokio 初始化延迟(Windows 实测
#      2026-08-17: median 26.8~28.1ms)。阈值 100ms 为基线 ~3.5× 余量,
#      防 CI 机器负载/防病毒扫描导致的假回归(与 bench_check 阈值惯例一致)。
#
# 依赖: hyperfine (cargo binstall hyperfine, 报告附录 D 已装 1.20.0)
#        release binary (cargo build --release -p chimera-cli)
#
# 输出: docs/reports/cli-startup-baseline.json (hyperfine 原始数据)
# 退出码: 0 = 通过 / 1 = 超阈值或错误
# ============================================================

param(
    [int]$Runs = 10,
    [double]$ThresholdMs = 100.0,
    [switch]$SkipAssert
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$projectRoot = Split-Path -Parent $PSScriptRoot
$binPath = Join-Path $projectRoot 'target\release\chimera.exe'
$outJson = Join-Path $projectRoot 'docs\reports\cli-startup-baseline.json'

# ---- 前置检查: binary 与 hyperfine ----
if (-not (Test-Path $binPath)) {
    Write-Host "[ERROR] release binary 不存在: $binPath" -ForegroundColor Red
    Write-Host "  请先执行: cargo build --release -p chimera-cli" -ForegroundColor Yellow
    exit 1
}
$hyperfine = Get-Command hyperfine -ErrorAction SilentlyContinue
if (-not $hyperfine) {
    Write-Host "[ERROR] hyperfine 未安装,请执行: cargo binstall hyperfine" -ForegroundColor Red
    exit 1
}

# ---- 测量: cmd 兼容路径(hyperfine 内部走 cmd.exe) ----
# WHY: hyperfine 在 Windows 用 cmd /C 执行,相对路径 "./chimera.exe" 不被 cmd 识别,
#      必须用带引号的完整路径。
$quotedBin = '"' + $binPath + '"'
Write-Host "[INFO] 测量 CLI 启动延迟 (runs=$Runs, 阈值=$ThresholdMs ms, 静默态优先)" -ForegroundColor Cyan
Write-Host "[INFO] binary: $binPath ($([math]::Round((Get-Item $binPath).Length / 1MB, 1)) MB)" -ForegroundColor Cyan

# 测量并导出 JSON;超阈值时由下方断言逻辑统一处理(hyperfine 自身退出码不阻断)
# WHY 不重定向 stderr: PowerShell 7 对 native 命令 stderr 重定向与
#      StandardOutputEncoding 存在兼容问题(实测 "only supported when standard
#      output is redirected"),直接透出不影响功能。
& $hyperfine.Source --warmup 3 --runs $Runs --export-json $outJson `
    -n 'version'   "$quotedBin --version" `
    -n 'help'      "$quotedBin --help" `
    -n 'help-cmd'  "$quotedBin help"
if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne 1) {
    Write-Host "[ERROR] hyperfine 执行失败 (退出码 $LASTEXITCODE)" -ForegroundColor Red
    exit 1
}
if (-not (Test-Path $outJson)) {
    Write-Host "[ERROR] 未生成基线 JSON: $outJson" -ForegroundColor Red
    exit 1
}

# ---- 解析结果并断言 ----
# WHY: median 比 mean 对离群点(系统负载/防病毒扫描)更稳健,断言以 median 为准;
#      三个命令任一超阈值即失败。
$data = Get-Content $outJson -Raw | ConvertFrom-Json
$failed = $false
Write-Host "`n=== CLI 启动延迟基线 ===" -ForegroundColor Cyan
foreach ($r in $data.results) {
    $medianMs = [math]::Round($r.median * 1000, 2)
    $meanMs = [math]::Round($r.mean * 1000, 2)
    $ok = $medianMs -lt $ThresholdMs
    if (-not $ok) { $failed = $true }
    $status = if ($ok) { 'OK ' } else { 'FAIL' }
    $line = "  [$status] $($r.command -replace '.*chimera\.exe','chimera')  median=$medianMs ms  mean=$meanMs ms  (阈值 < $ThresholdMs ms)"
    if ($ok) { Write-Host $line -ForegroundColor Green } else { Write-Host $line -ForegroundColor Red }
}

if ($SkipAssert) {
    Write-Host "`n[SKIP] 跳过断言 (观测模式)" -ForegroundColor Yellow
    exit 0
}
if ($failed) {
    Write-Host "`n[FAIL] 启动延迟超阈值,请检查: ① 是否在静默态测量; ② 是否引入启动路径回归(如新增重量级初始化)" -ForegroundColor Red
    exit 1
}
Write-Host "`n[OK] 启动延迟基线检查通过 (阈值 < $ThresholdMs ms)" -ForegroundColor Green
exit 0
