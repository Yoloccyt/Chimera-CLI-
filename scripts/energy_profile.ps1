# energy_profile.ps1 — Windows 能耗参考证据采样器（R5-2，ADR-264）
#
# 定位：dev-only 参考证据工具（不入 CI 门禁）——代理指标（分配数/CPU-ms/
# 空闲唤醒率）是硬验收（ADR-264 §2），本脚本只提供 OS 级旁证：进程 CPU 时间、
# 工作集、整包处理器占用率与（可选）电池报告。
#
# 用法（repo 根，PowerShell 5.1+）：
#   .\scripts\energy_profile.ps1 -DurationSec 30 -ProcessName chimera
#
# 前置：无需管理员（powercfg /energy 需要管理员，故以 /batteryreport 替代，
# 且仅在检测到电池时输出）。输出写 stdout（markdown 片段）。

param(
    [int]$DurationSec = 30,
    [string]$ProcessName = "chimera",
    [int]$IntervalSec = 5
)

$ErrorActionPreference = "SilentlyContinue"
Write-Output "# 能耗参考证据采样（$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')）"
Write-Output ""
Write-Output "- 采样时长：${DurationSec}s（间隔 ${IntervalSec}s）；目标进程：$ProcessName"
Write-Output ""

# 1) 整包处理器占用率（Counter 采样）
$cpuSamples = @()
$end = (Get-Date).AddSeconds($DurationSec)
while ((Get-Date) -lt $end) {
    $c = Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 1 -MaxSamples 1
    $cpuSamples += [math]::Round($c.CounterSamples[0].CookedValue, 1)
    Start-Sleep -Seconds ([Math]::Max(1, $IntervalSec - 1))
}
if ($cpuSamples.Count -gt 0) {
    $avg = ($cpuSamples | Measure-Object -Average).Average
    $max = ($cpuSamples | Measure-Object -Maximum).Maximum
    Write-Output "- 整包 CPU%：avg=$avg max=$max（样本 $($cpuSamples -join ',')）"
}

# 2) 目标进程 CPU 时间与工作集
$procs = Get-Process -Name $ProcessName
if ($procs) {
    foreach ($p in $procs) {
        $wsMB = [math]::Round($p.WorkingSet64 / 1MB, 1)
        Write-Output "- 进程 $($p.Id)（$($p.ProcessName)）：CPU 时间 $($p.TotalProcessorTime) / 工作集 ${wsMB} MB"
    }
}
else {
    Write-Output "- 目标进程 '$ProcessName' 未运行（跳过进程级采样）"
}

# 3) 电池报告（笔记本；台式机自动跳过）
$batt = Get-CimInstance Win32_Battery
if ($batt) {
    $report = Join-Path $env:TEMP "chimera-battery-report.html"
    powercfg /batteryreport /output $report | Out-Null
    if (Test-Path $report) {
        Write-Output "- 电池报告：$report（powercfg /batteryreport）"
    }
}
else {
    Write-Output "- 无电池（台式机/交流供电），跳过 battery report"
}

Write-Output ""
Write-Output "> 定位声明：本输出为参考旁证；硬验收 = 代理指标（分配数/CPU-ms/空闲唤醒率，ADR-264）。"
