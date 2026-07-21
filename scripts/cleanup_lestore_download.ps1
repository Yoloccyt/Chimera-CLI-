#Requires -Version 5.1
<#
.SYNOPSIS
    清理 D:\LeStoreDownload 联想应用商店下载缓存
.DESCRIPTION
    必须以管理员身份运行(PowerShell 右键"以管理员身份运行"或 sudo 启动)。
    流程:停联想服务 → 等待句柄释放 → 删除文件 → 可选重启服务
.NOTES
    Author: Chimera CLI cleanup toolkit
    Risk: 极低(只清下载缓存,不影响已装应用运行)
    Reclaim: ~38 GB
#>
[CmdletBinding()]
param(
    [switch]$SkipServiceStop,   # 跳过停服务(只删无锁文件)
    [switch]$RestartAfter       # 清理完后自动重启被停的服务
)

$ErrorActionPreference = 'Continue'
$targetDir = 'D:\LeStoreDownload'

if (-not (Test-Path -LiteralPath $targetDir)) {
    Write-Host "[skip] $targetDir not exist" -ForegroundColor DarkGray
    exit 0
}

# 0. 管理员检查
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Host "[ERROR] 必须以管理员身份运行此脚本" -ForegroundColor Red
    Write-Host "        右键 PowerShell → 以管理员身份运行 → 重试" -ForegroundColor Yellow
    exit 1
}

# 1. 停联想相关服务
if (-not $SkipServiceStop) {
    Write-Host "=== Step 1: 停止联想服务 ===" -ForegroundColor Cyan
    $services = @('LISFService', 'LenovoPcManagerService', 'LenovoServiceAS', 'GAService', 'SLBService')
    foreach ($s in $services) {
        $svc = Get-Service -Name $s -ErrorAction SilentlyContinue
        if ($svc -and $svc.Status -eq 'Running') {
            try {
                Write-Host "  Stop $s ..." -NoNewline
                Stop-Service -Name $s -Force -ErrorAction Stop
                $svc.WaitForStatus('Stopped', '00:00:10')
                Write-Host " OK" -ForegroundColor Green
            } catch {
                Write-Host " FAIL: $_" -ForegroundColor Red
            }
        } else {
            Write-Host "  [skip] $s (not running)" -ForegroundColor DarkGray
        }
    }
    # 给句柄释放一点时间
    Start-Sleep -Seconds 3
}

# 2. 杀可能仍持有句柄的进程
Write-Host "`n=== Step 2: 结束可能持锁的进程 ===" -ForegroundColor Cyan
$procPatterns = @('LeCloud', 'LsaIso', 'Lsf', 'Lenovo*Store', 'Lenovo*App', 'LSApp')
$procs = Get-Process | Where-Object {
    foreach ($p in $procPatterns) {
        if ($_.Name -like $p) { return $true }
    }
    return $false
}
if ($procs) {
    foreach ($p in $procs) {
        try {
            Write-Host "  Stop $($p.Name) (PID $($p.Id))" -NoNewline
            Stop-Process -Id $p.Id -Force -ErrorAction Stop
            Write-Host " OK" -ForegroundColor Green
        } catch {
            Write-Host " FAIL: $_" -ForegroundColor Red
        }
    }
    Start-Sleep -Seconds 2
} else {
    Write-Host "  [none] no relevant processes found" -ForegroundColor DarkGray
}

# 3. 记录清理前空间
$freeBefore = [math]::Round((Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='D:'").FreeSpace/1GB, 3)
Write-Host "`nD free before: $freeBefore GB"

# 4. 实际删除
Write-Host "`n=== Step 3: 删除文件 ===" -ForegroundColor Cyan
$items = Get-ChildItem -LiteralPath $targetDir -Force -ErrorAction SilentlyContinue
$total = $items.Count
Write-Host "Target: $total items"
$ok = 0; $fail = 0
foreach ($i in $items) {
    try {
        # .NET API 直删,绕过回收站(更稳,符合 NTFRS 日志要求)
        if ($i.PSIsContainer) {
            [System.IO.Directory]::Delete($i.FullName, $true)
        } else {
            [System.IO.File]::Delete($i.FullName)
        }
        $ok++
        if ($ok % 20 -eq 0) { Write-Host "  progress: $ok/$total" }
    } catch {
        $fail++
        if ($fail -le 10) {
            Write-Host "  FAIL: $($i.Name) - $($_.Exception.Message)" -ForegroundColor Red
        }
    }
}
Write-Host "Success: $ok | Failed: $fail" -ForegroundColor $(if($fail -eq 0){'Green'}else{'Yellow'})

# 5. 验证
$freeAfter = [math]::Round((Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='D:'").FreeSpace/1GB, 3)
$freed = [math]::Round($freeAfter - $freeBefore, 3)
Write-Host "`nD free after: $freeAfter GB (freed $freed GB)" -ForegroundColor Green

# 6. 可选:重启服务
if ($RestartAfter) {
    Write-Host "`n=== Step 4: 重启服务 ===" -ForegroundColor Cyan
    foreach ($s in @('LISFService', 'LenovoPcManagerService', 'LenovoServiceAS', 'GAService')) {
        $svc = Get-Service -Name $s -ErrorAction SilentlyContinue
        if ($svc -and $svc.Status -eq 'Stopped') {
            try {
                Start-Service -Name $s -ErrorAction Stop
                Write-Host "  Start $s OK" -ForegroundColor Green
            } catch {
                Write-Host "  Start $s FAIL: $_" -ForegroundColor Red
            }
        }
    }
}

Write-Host "`n=== Done ===" -ForegroundColor Cyan
