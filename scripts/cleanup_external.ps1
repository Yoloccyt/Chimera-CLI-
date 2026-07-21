#Requires -Version 5.1
<#
.SYNOPSIS
    沙箱外临时文件清理脚本(需系统 PowerShell 管理员执行)
.DESCRIPTION
    清理 D 盘非项目根目录下的可回收空间(联想系/电脑管家迁移/微信缓存等)。
    每个清理项会先列大小再确认。
.NOTES
    必须以管理员权限运行(回收站操作需要)
    路径已用单引号包裹,避免 $ 和 % 被 PowerShell 解释
#>
[CmdletBinding()]
param([switch]$Force)

$ErrorActionPreference = 'Continue'

function Show-Size($path) {
    if (-not (Test-Path -LiteralPath $path)) { return $null }
    $sum = (Get-ChildItem -LiteralPath $path -Recurse -Force -File -ErrorAction SilentlyContinue |
            Measure-Object -Property Length -Sum).Sum
    if ($sum) { [math]::Round($sum/1GB, 3) } else { 0 }
}

function Confirm-Act($desc, $path) {
    Write-Host "`n[$desc]" -ForegroundColor Yellow
    Write-Host "  Path: $path"
    $sz = Show-Size $path
    if ($null -eq $sz) { Write-Host "  (does not exist)"; return $false }
    Write-Host "  Size: $sz GB"
    if ($Force) { return $true }
    $r = Read-Host "  Confirm delete? (Y/N)"
    return ($r -eq 'Y' -or $r -eq 'y')
}

function Remove-Path($path) {
    if (Test-Path -LiteralPath $path) {
        try {
            Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction Stop
            Write-Host "  [OK] removed" -ForegroundColor Green
        } catch {
            Write-Warning "  [FAIL] $($_.Exception.Message)"
        }
    }
}

$freeBefore = [math]::Round((Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='D:'").FreeSpace/1GB, 3)
Write-Host "D free before: $freeBefore GB" -ForegroundColor Cyan

# 1. 联想应用商店下载缓存
if (Confirm-Act "Lenovo App Store downloads" 'D:\LeStoreDownload') {
    Remove-Path 'D:\LeStoreDownload'
}

# 2. 腾讯电脑管家迁移文件
if (Confirm-Act "Tencent PC Manager migration files" 'D:\电脑管家迁移文件') {
    Remove-Path 'D:\电脑管家迁移文件'
}

# 3. 联想软件商店
if (Confirm-Act "Lenovo Softstore" 'D:\LenovoSoftstore') {
    Remove-Path 'D:\LenovoSoftstore'
}

# 4. 微信缓存(只清 Cache,保留 Files/聊天记录)
$wxCache = 'D:\WeChat Files\Cache'
if (Confirm-Act "WeChat cache only" $wxCache) {
    Remove-Path $wxCache
}

# 5. Steam 游戏(不玩可删)
$steamGames = @('D:\群星', 'D:\群星(重生版)')
foreach ($g in $steamGames) {
    if (Confirm-Act "Steam game" $g) { Remove-Path $g }
}

# 6. C 盘 AppData\Temp 被占用的临时文件(关闭关联进程后可删)
#    IS-* = InstallShield, SLB = Service Layer Bus(联想)
#    先 Close-Process 再清
$procNames = @('ISBEW64', 'SLBService', 'TVTInstaller', 'LISFService')
foreach ($p in $procNames) {
    $procs = Get-Process -Name $p -ErrorAction SilentlyContinue
    if ($procs) {
        Write-Host "`n  Found $($procs.Count) '$p' process(es)" -ForegroundColor Yellow
        if ($Force -or ((Read-Host "  Kill processes? (Y/N)") -eq 'Y')) {
            $procs | Stop-Process -Force -ErrorAction SilentlyContinue
            Write-Host "  [OK] killed" -ForegroundColor Green
        }
    }
}

# 7. 清 Temp 中残留(进程关闭后)
$temp = 'C:\Users\30324\AppData\Local\Temp'
if (Confirm-Act "C:\Users\30324\AppData\Local\Temp" $temp) {
    Get-ChildItem -LiteralPath $temp -Force -ErrorAction SilentlyContinue | ForEach-Object {
        try { Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction Stop }
        catch { Write-Warning "  skip: $($_.Name)" }
    }
    Write-Host "  [OK] Temp cleared" -ForegroundColor Green
}

$freeAfter = [math]::Round((Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='D:'").FreeSpace/1GB, 3)
Write-Host "`nD free: $freeBefore -> $freeAfter GB (freed $([math]::Round($freeAfter-$freeBefore,3)) GB)" -ForegroundColor Green
