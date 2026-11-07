# energy_profile.ps1 — Windows 能耗参考证据采样器（R5-2，ADR-264；R5 工单 B 峰值升级，ADR-265）
#
# 定位：dev-only 参考证据工具（不入 CI 门禁）——代理指标（分配数/CPU-ms/
# 空闲唤醒率）是硬验收（ADR-264 §2），本脚本只提供 OS 级旁证：进程 CPU 时间、
# 工作集（窗口峰值 + 均值双口径）、整包处理器占用率与（可选）电池报告。
#
# 内存峰值口径（ADR-265 登记，工程工单 B）：
#   - **窗口峰值** = 采样环内 WorkingSet64 最大值（本工具测量窗口内的峰值，
#     采样粒度 $PollMs，默认 500ms——两次采样间的瞬时尖峰可能漏采）；
#   - **进程生存期峰值** = .NET Process.PeakWorkingSet64（OS 自进程启动起
#     观测的峰值，含启动段，不受本窗口约束）——两者并列输出防误读；
#   - WorkingSet ≈ 物理驻留字节数，**非提交内存（commit charge）**；任务管理器
#     默认显示的「内存（活动专用工作集）」与本口径同族而非 CommitSize。
#
# 用法（repo 根，PowerShell 5.1+）：
#   .\scripts\energy_profile.ps1 -DurationSec 30 -ProcessName chimera
#
# 前置：无需管理员（powercfg /energy 需要管理员，故以 /batteryreport 替代，
# 且仅在检测到电池时输出）。输出写 stdout（markdown 片段）。

param(
    [int]$DurationSec = 30,
    [string]$ProcessName = "chimera",
    [int]$IntervalSec = 5,
    # 窗口峰值采样粒度（毫秒）；越小越不易漏瞬时尖峰，CPU 开销相应增大
    [int]$PollMs = 500
)

$ErrorActionPreference = "SilentlyContinue"
Write-Output "# 能耗参考证据采样（$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')）"
Write-Output ""
Write-Output "- 采样时长：${DurationSec}s（CPU 间隔 ${IntervalSec}s / 工作集轮询 ${PollMs}ms）；目标进程：$ProcessName"
Write-Output ""

# 1) 主采样环：工作集窗口峰值（每 PollMs）+ 整包 CPU%（每 IntervalSec）
# WHY 单环而非两段：CPU 与工作集在同一时间窗内对账（分离两环会失去「同一
# 测量窗口」语义——能耗旁证要求 CPU% 与 RSS 峰值可同帧引用）。
$cpuSamples = @()
# pid -> @{ ws = @(每次采样的 MB)；peakLifetime = 进程生存期峰值 MB }
$wsByPid = @{}
$end = (Get-Date).AddSeconds($DurationSec)
$nextCpuAt = (Get-Date)
while ((Get-Date) -lt $end) {
    $procs = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue
    foreach ($p in $procs) {
        if (-not $wsByPid.ContainsKey($p.Id)) {
            $wsByPid[$p.Id] = @{ ws = @(); peakLifetime = 0.0 }
        }
        $wsByPid[$p.Id].ws += [math]::Round($p.WorkingSet64 / 1MB, 1)
        # PeakWorkingSet64：OS 记账的生存期峰值（进程退出前随 Get-Process 刷新）
        $wsByPid[$p.Id].peakLifetime = [math]::Round($p.PeakWorkingSet64 / 1MB, 1)
    }

    if ((Get-Date) -ge $nextCpuAt) {
        $c = Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 1 -MaxSamples 1
        $cpuSamples += [math]::Round($c.CounterSamples[0].CookedValue, 1)
        $nextCpuAt = (Get-Date).AddSeconds($IntervalSec)
    }
    Start-Sleep -Milliseconds $PollMs
}

# 2) 整包 CPU% 汇总
if ($cpuSamples.Count -gt 0) {
    $avg = ($cpuSamples | Measure-Object -Average).Average
    $max = ($cpuSamples | Measure-Object -Maximum).Maximum
    Write-Output "- 整包 CPU%：avg=$avg max=$max（样本 $($cpuSamples -join ',')）"
}

# 3) 目标进程：CPU 时间 + 窗口峰值/均值 + 生存期峰值（双口径，ADR-265）
$procsNow = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue
if ($wsByPid.Count -gt 0) {
    foreach ($pidKey in $wsByPid.Keys) {
        $samples = $wsByPid[$pidKey].ws
        $peakWindow = ($samples | Measure-Object -Maximum).Maximum
        $avgWindow = [math]::Round(($samples | Measure-Object -Average).Average, 1)
        $lifetime = $wsByPid[$pidKey].peakLifetime
        $cpuLine = ""
        if ($procsNow) {
            $p = $procsNow | Where-Object { $_.Id -eq $pidKey }
            if ($p) { $cpuLine = " / CPU 时间 $($p.TotalProcessorTime)" }
        }
        Write-Output "- 进程 $pidKey（$ProcessName）${cpuLine}：工作集窗口峰值 **${peakWindow} MB** / 窗口均值 ${avgWindow} MB（样本 $($samples.Count)）/ 生存期峰值 ${lifetime} MB"
    }
}
elseif ($procsNow) {
    # 进程存在但采样窗内没采到（理论上不可能，防御输出）
    foreach ($p in $procsNow) {
        $wsMB = [math]::Round($p.WorkingSet64 / 1MB, 1)
        Write-Output "- 进程 $($p.Id)：CPU 时间 $($p.TotalProcessorTime) / 工作集（单点）${wsMB} MB"
    }
}
else {
    Write-Output "- 目标进程 '$ProcessName' 未运行（跳过进程级采样；CPU% 仍有效）"
}

# 4) 电池报告（笔记本；台式机自动跳过）
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
Write-Output "> 峰值口径：窗口峰值 = 本测量窗内 WorkingSet64 最大值（${PollMs}ms 粒度）；生存期峰值 = OS 记账自进程启动（含启动段）；两者均≈物理驻留非提交内存（ADR-265）。"
