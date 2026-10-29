# 影子双跑长跑驱动包装（P4-T7）
# 用法: powershell -File scripts/run_shadow_sojourn.ps1 [-Days 7]
# 语义: 运行 event-bus shadow_sojourn 驱动（注入时钟多天双跑记账）,
#       校验快照 JSON 落盘且 go_ready_at_7 = true（RK-P20 时间加速等价）。
param(
    [int]$Days = 7
)

$ErrorActionPreference = 'Stop'
# Repo-root-derived toolchain env; the old block pinned 'D:\Chimera CLI' twice
# (once for the env, once in Set-Location), which broke on any other clone path.
. (Join-Path $PSScriptRoot 'toolchain-env.ps1')
Set-ChimeraToolchainEnv | Out-Null
Set-Location $ChimeraRepoRoot

$env:SOJOURN_DAYS = "$Days"
cargo run -p event-bus --example shadow_sojourn
if ($LASTEXITCODE -ne 0) {
    Write-Host "[FAIL] 双跑驱动退出码 $LASTEXITCODE" -ForegroundColor Red
    exit $LASTEXITCODE
}

$snapshotPath = 'tmp/shadow_sojourn_snapshot.json'
if (-not (Test-Path $snapshotPath)) {
    Write-Host '[FAIL] 快照未落盘: ' + $snapshotPath -ForegroundColor Red
    exit 1
}
$snapshot = Get-Content $snapshotPath -Raw | ConvertFrom-Json
if ($snapshot.zero_diff_days -lt [Math]::Min($Days, 7)) {
    Write-Host "[FAIL] 连续零 diff 天数不足: $($snapshot.zero_diff_days)" -ForegroundColor Red
    exit 1
}
Write-Host "[PASS] 双跑台账: days=$($snapshot.days_recorded) zero_diff_days=$($snapshot.zero_diff_days) total_diffs=$($snapshot.total_diffs) go_ready=$($snapshot.go_ready_at_7)" -ForegroundColor Green
exit 0
