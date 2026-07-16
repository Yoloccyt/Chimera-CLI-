#Requires -Version 5.1
<#
.SYNOPSIS
    Chimera CLI 磁盘空间诊断与清理脚本
.DESCRIPTION
    提供 Diagnose/SafeClean/ProjectClean 三种模式：
    - Diagnose: 只读扫描磁盘占用，输出报告
    - SafeClean: P0 级安全清理（回收站/Temp/npm cache/chimera-target）
    - ProjectClean: 项目内可回收产物清理（tmp_podman/fuzz target/%SystemDrive%）
.NOTES
    适用于 Windows 10/11 + PowerShell 5.1+
    管理员权限仅 powercfg /hibernate off 需要，本脚本不强制要求
.LINK
    Spec: .trae/specs/disk-space-cleanup-and-prevention/spec.md
#>

[CmdletBinding()]
param(
    [ValidateSet('Diagnose','SafeClean','ProjectClean')]
    [string]$Mode = 'Diagnose',

    [switch]$Force,

    [int]$TopN = 15
)

$ErrorActionPreference = 'Continue'

# ============================================================
# 辅助函数
# ============================================================

# 格式化字节为 GB（保留 3 位小数）
function Format-GB {
    param([long]$Bytes)
    if ($Bytes -le 0) { return [double]0.0 }
    return [math]::Round($Bytes / 1GB, 3)
}

# 打印章节标题
function Write-Section {
    param([string]$Title)
    Write-Host "`n=== $Title ===" -ForegroundColor Cyan
}

# 打印子步骤信息
function Write-SubStep {
    param([string]$Msg)
    Write-Host "  [>] $Msg" -ForegroundColor Gray
}

# 打印成功信息
function Write-Ok {
    param([string]$Msg)
    Write-Host "  [OK] $Msg" -ForegroundColor Green
}

# 计算文件夹大小（递归扫描，忽略权限拒绝错误）
function Get-FolderSize {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) {
        return @{ Size = [long]0; Exists = $false }
    }
    try {
        # -Force 包含隐藏/系统文件，-ErrorAction SilentlyContinue 忽略权限拒绝
        $measure = Get-ChildItem -LiteralPath $Path -Recurse -Force -File -ErrorAction SilentlyContinue |
                   Measure-Object -Property Length -Sum
        $size = if ($measure.Sum) { [long]$measure.Sum } else { [long]0 }
        return @{ Size = $size; Exists = $true }
    } catch {
        return @{ Size = [long]0; Exists = $true }
    }
}

# 获取指定盘符的可用空间（GB）
function Get-DiskFreeGB {
    param([string]$DriveLetter)
    try {
        $disk = Get-CimInstance -ClassName Win32_LogicalDisk -Filter "DeviceID='$DriveLetter'" -ErrorAction Stop
        return Format-GB -Bytes $disk.FreeSpace
    } catch {
        return [double]-1
    }
}

# 确认是否执行清理操作（-Force 开关跳过确认）
function Confirm-CleanAction {
    param(
        [string]$Description,
        [string]$Path,
        [double]$EstimatedGB
    )
    Write-Host "`n  [清理项] $Description" -ForegroundColor Yellow
    Write-Host "    路径: $Path" -ForegroundColor Gray
    Write-Host "    预计释放: $EstimatedGB GB" -ForegroundColor Gray

    if ($Force) {
        Write-Host "    (-Force 已指定，自动执行)" -ForegroundColor DarkGray
        return $true
    }
    $response = Read-Host "    确认执行? (Y/N)"
    return ($response -eq 'Y' -or $response -eq 'y')
}

# 清空目录内容（保留目录本身，仅删除其中的文件和子目录）
function Clear-DirectoryContents {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $items = Get-ChildItem -LiteralPath $Path -Force -ErrorAction SilentlyContinue
    foreach ($item in $items) {
        try {
            Remove-Item -LiteralPath $item.FullName -Recurse -Force -ErrorAction Stop
        } catch {
            Write-Warning "    无法删除: $($item.FullName) - $($_.Exception.Message)"
        }
    }
}

# 删除整个目录（含所有内容）
function Remove-FullDirectory {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { return }
    try {
        Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction Stop
    } catch {
        Write-Warning "    无法删除目录: $Path - $($_.Exception.Message)"
    }
}

# ============================================================
# Diagnose 模式函数
# ============================================================

# 显示所有固定磁盘驱动器的总量/已用/可用空间
function Show-DiskOverview {
    Write-Section "磁盘驱动器概览"
    try {
        # DriveType=3 筛选固定磁盘（排除光驱/网络驱动器/可移动磁盘）
        $disks = Get-CimInstance -ClassName Win32_LogicalDisk -Filter "DriveType=3" -ErrorAction Stop
        $diskInfo = $disks | ForEach-Object {
            $freeGB = Format-GB -Bytes $_.FreeSpace
            $totalGB = Format-GB -Bytes $_.Size
            $usedGB = [math]::Round($totalGB - $freeGB, 3)
            $usedPct = if ($totalGB -gt 0) { [math]::Round(($usedGB / $totalGB) * 100, 1) } else { 0 }
            [PSCustomObject]@{
                Drive    = $_.DeviceID
                Total_GB = $totalGB
                Used_GB  = $usedGB
                Free_GB  = $freeGB
                Used_Pct = "$usedPct%"
            }
        }
        $diskInfo | Format-Table -AutoSize
    } catch {
        Write-Warning "获取磁盘信息失败: $_"
    }
}

# 显示指定路径下 Top N 大文件夹（按大小降序）
function Show-TopFolders {
    param(
        [string]$RootPath,
        [string]$Label
    )
    Write-Section "$Label (Top $TopN)"
    Write-SubStep "扫描路径: $RootPath"

    if (-not (Test-Path -LiteralPath $RootPath)) {
        Write-Warning "路径不存在: $RootPath"
        return
    }

    try {
        # -Force 包含隐藏目录（如 AppData），-ErrorAction SilentlyContinue 忽略权限拒绝
        $folders = Get-ChildItem -LiteralPath $RootPath -Directory -Force -ErrorAction SilentlyContinue
        $results = @()
        foreach ($folder in $folders) {
            $sizeInfo = Get-FolderSize -Path $folder.FullName
            $results += [PSCustomObject]@{
                Folder  = $folder.FullName
                Size_GB = Format-GB -Bytes $sizeInfo.Size
            }
        }
        $results | Sort-Object Size_GB -Descending | Select-Object -First $TopN | Format-Table -AutoSize
    } catch {
        Write-Warning "扫描文件夹失败: $_"
    }
}

# 检查关键系统文件大小（页面文件/休眠文件/交换文件）
function Show-SystemFiles {
    Write-Section "关键系统文件大小"
    $systemFiles = @(
        'C:\pagefile.sys',
        'C:\hiberfil.sys',
        'C:\swapfile.sys'
    )
    $results = @()
    foreach ($file in $systemFiles) {
        try {
            # -Force 获取隐藏/系统属性文件
            $item = Get-Item -LiteralPath $file -Force -ErrorAction Stop
            $results += [PSCustomObject]@{
                File    = $file
                Size_GB = Format-GB -Bytes $item.Length
                Exists  = $true
            }
        } catch {
            $results += [PSCustomObject]@{
                File    = $file
                Size_GB = 0
                Exists  = $false
            }
        }
    }
    $results | Format-Table -AutoSize
}

# 检查回收站大小（C 盘和 D 盘）
function Show-RecycleBin {
    Write-Section "回收站大小"
    # 单引号防止 $ 被解释为变量引用
    $recycleBins = @(
        @{ Path = 'C:\$Recycle.Bin'; Label = 'C 盘回收站' },
        @{ Path = 'D:\$Recycle.Bin'; Label = 'D 盘回收站' }
    )
    $results = @()
    foreach ($rb in $recycleBins) {
        $sizeInfo = Get-FolderSize -Path $rb.Path
        $results += [PSCustomObject]@{
            Location = $rb.Label
            Path     = $rb.Path
            Size_GB  = Format-GB -Bytes $sizeInfo.Size
            Exists   = $sizeInfo.Exists
        }
    }
    $results | Format-Table -AutoSize
}

# 检查项目目录各子目录大小（显示全部，不做 Top N 截断）
function Show-ProjectDirs {
    param([string]$ProjectRoot = 'D:\Chimera CLI')
    Write-Section "项目目录 $ProjectRoot 子目录大小"
    Write-SubStep "扫描路径: $ProjectRoot"

    if (-not (Test-Path -LiteralPath $ProjectRoot)) {
        Write-Warning "项目目录不存在: $ProjectRoot"
        return
    }

    try {
        $folders = Get-ChildItem -LiteralPath $ProjectRoot -Directory -Force -ErrorAction SilentlyContinue
        $results = @()
        foreach ($folder in $folders) {
            $sizeInfo = Get-FolderSize -Path $folder.FullName
            $results += [PSCustomObject]@{
                Folder  = $folder.Name
                Size_GB = Format-GB -Bytes $sizeInfo.Size
            }
        }
        # 按 Size_GB 降序排列，显示全部子目录
        $results | Sort-Object Size_GB -Descending | Format-Table -AutoSize
    } catch {
        Write-Warning "扫描项目目录失败: $_"
    }
}

# Diagnose 模式主函数（纯只读，不修改任何文件）
function Invoke-Diagnose {
    Write-Host "`n************************************************" -ForegroundColor Cyan
    Write-Host "  Chimera CLI 磁盘空间诊断 (Diagnose 模式 - 只读)" -ForegroundColor Cyan
    Write-Host "************************************************" -ForegroundColor Cyan

    Show-DiskOverview
    Show-TopFolders -RootPath 'D:\' -Label "D 盘根目录文件夹"
    Show-TopFolders -RootPath 'C:\Users\30324' -Label "C:\Users\30324 子文件夹"
    Show-SystemFiles
    Show-RecycleBin
    Show-ProjectDirs

    Write-Host "`n=== 诊断完成 ===" -ForegroundColor Cyan
    Write-Host "  以上为只读诊断结果，未修改任何文件。" -ForegroundColor Green
}

# ============================================================
# SafeClean 模式函数（C 盘 P0 安全清理）
# ============================================================

function Invoke-SafeClean {
    Write-Host "`n************************************************" -ForegroundColor Cyan
    Write-Host "  Chimera CLI P0 安全清理 (SafeClean 模式)" -ForegroundColor Cyan
    Write-Host "************************************************" -ForegroundColor Cyan

    # 记录清理前的可用空间，用于最终计算释放差值
    $freeBefore_C = Get-DiskFreeGB -DriveLetter 'C:'
    $freeBefore_D = Get-DiskFreeGB -DriveLetter 'D:'
    Write-SubStep "清理前 C 盘可用: $freeBefore_C GB | D 盘可用: $freeBefore_D GB"

    # --- 步骤 1: 清空 C 盘回收站 ---
    $rbCPath = 'C:\$Recycle.Bin'
    $rbCSize = (Get-FolderSize -Path $rbCPath).Size
    $rbCSizeGB = Format-GB -Bytes $rbCSize
    if (Confirm-CleanAction -Description "清空 C 盘回收站" -Path $rbCPath -EstimatedGB $rbCSizeGB) {
        try {
            Clear-RecycleBin -DriveLetter C -Force -ErrorAction SilentlyContinue
            Write-Ok "C 盘回收站已清空"
        } catch {
            Write-Warning "清空 C 盘回收站失败: $_"
        }
    }

    # --- 步骤 2: 清空 D 盘回收站 ---
    $rbDPath = 'D:\$Recycle.Bin'
    $rbDSize = (Get-FolderSize -Path $rbDPath).Size
    $rbDSizeGB = Format-GB -Bytes $rbDSize
    if (Confirm-CleanAction -Description "清空 D 盘回收站" -Path $rbDPath -EstimatedGB $rbDSizeGB) {
        try {
            Clear-RecycleBin -DriveLetter D -Force -ErrorAction SilentlyContinue
            Write-Ok "D 盘回收站已清空"
        } catch {
            Write-Warning "清空 D 盘回收站失败: $_"
        }
    }

    # --- 步骤 3: 清空 Temp 目录内容（保留目录本身） ---
    $tempPath = 'C:\Users\30324\AppData\Local\Temp'
    $tempSize = (Get-FolderSize -Path $tempPath).Size
    $tempSizeGB = Format-GB -Bytes $tempSize
    if (Confirm-CleanAction -Description "清空 Temp 目录内容" -Path $tempPath -EstimatedGB $tempSizeGB) {
        Clear-DirectoryContents -Path $tempPath
        Write-Ok "Temp 目录内容已清空（保留目录）"
    }

    # --- 步骤 4: npm cache clean --force（仅在 npm 可用时执行） ---
    $npmAvailable = $null -ne (Get-Command npm -ErrorAction SilentlyContinue)
    if ($npmAvailable) {
        # 尝试获取 npm 缓存路径以估算释放空间
        $npmCachePath = $null
        try {
            $npmCachePath = (npm config get cache 2>$null).Trim()
        } catch { }

        $npmCacheSizeGB = [double]0
        if ($npmCachePath -and (Test-Path -LiteralPath $npmCachePath)) {
            $npmCacheSize = (Get-FolderSize -Path $npmCachePath).Size
            $npmCacheSizeGB = Format-GB -Bytes $npmCacheSize
        }

        $npmDisplayPath = if ($npmCachePath) { $npmCachePath } else { "npm cache (路径未获取)" }
        if (Confirm-CleanAction -Description "npm cache clean --force" -Path $npmDisplayPath -EstimatedGB $npmCacheSizeGB) {
            try {
                npm cache clean --force 2>&1 | Out-Null
                Write-Ok "npm 缓存已清理"
            } catch {
                Write-Warning "npm 缓存清理失败: $_"
            }
        }
    } else {
        Write-Host "`n  [跳过] npm 不可用，跳过 npm cache clean" -ForegroundColor DarkGray
    }

    # --- 步骤 5: 删除 C:\chimera-test-target（如果存在） ---
    $testTargetPath = 'C:\chimera-test-target'
    if (Test-Path -LiteralPath $testTargetPath) {
        $ttSize = (Get-FolderSize -Path $testTargetPath).Size
        $ttSizeGB = Format-GB -Bytes $ttSize
        if (Confirm-CleanAction -Description "删除 C:\chimera-test-target" -Path $testTargetPath -EstimatedGB $ttSizeGB) {
            Remove-FullDirectory -Path $testTargetPath
            Write-Ok "C:\chimera-test-target 已删除"
        }
    } else {
        Write-Host "`n  [跳过] $testTargetPath 不存在" -ForegroundColor DarkGray
    }

    # --- 步骤 6: 删除 C:\chimera-target（如果存在） ---
    $targetPath = 'C:\chimera-target'
    if (Test-Path -LiteralPath $targetPath) {
        $tSize = (Get-FolderSize -Path $targetPath).Size
        $tSizeGB = Format-GB -Bytes $tSize
        if (Confirm-CleanAction -Description "删除 C:\chimera-target" -Path $targetPath -EstimatedGB $tSizeGB) {
            Remove-FullDirectory -Path $targetPath
            Write-Ok "C:\chimera-target 已删除"
        }
    } else {
        Write-Host "`n  [跳过] $targetPath 不存在" -ForegroundColor DarkGray
    }

    # --- 步骤 7: 打印实际释放的空间差值 ---
    $freeAfter_C = Get-DiskFreeGB -DriveLetter 'C:'
    $freeAfter_D = Get-DiskFreeGB -DriveLetter 'D:'

    $freed_C = [math]::Round($freeAfter_C - $freeBefore_C, 3)
    $freed_D = [math]::Round($freeAfter_D - $freeBefore_D, 3)

    Write-Section "清理结果"
    Write-Host "  C 盘: $freeBefore_C GB → $freeAfter_C GB (释放 $freed_C GB)" -ForegroundColor Green
    Write-Host "  D 盘: $freeBefore_D GB → $freeAfter_D GB (释放 $freed_D GB)" -ForegroundColor Green
    Write-Host "`n=== SafeClean 完成 ===" -ForegroundColor Cyan
}

# ============================================================
# ProjectClean 模式函数（项目内可回收产物清理）
# ============================================================

function Invoke-ProjectClean {
    Write-Host "`n************************************************" -ForegroundColor Cyan
    Write-Host "  Chimera CLI 项目产物清理 (ProjectClean 模式)" -ForegroundColor Cyan
    Write-Host "************************************************" -ForegroundColor Cyan

    $projectRoot = 'D:\Chimera CLI'

    # 受保护目录与文件列表（绝对不清理）
    $protectedDirs = @(
        '.toolchain', '.git', 'crates', 'docs', 'src', 'tests',
        'examples', '.cargo', '.github', '.trae', '.claude'
    )
    $protectedFiles = @('Cargo.toml', 'Cargo.lock')

    Write-SubStep "受保护目录（不清理）: $($protectedDirs -join ', ')"
    Write-SubStep "受保护文件（不清理）: $($protectedFiles -join ', ')"

    # 记录清理前的 D 盘可用空间
    $freeBefore_D = Get-DiskFreeGB -DriveLetter 'D:'
    Write-SubStep "清理前 D 盘可用: $freeBefore_D GB"

    # --- 步骤 1: 清空 tmp_podman 目录内容（保留目录本身） ---
    $tmpPodmanPath = Join-Path $projectRoot 'tmp_podman'
    if (Test-Path -LiteralPath $tmpPodmanPath) {
        $tpSize = (Get-FolderSize -Path $tmpPodmanPath).Size
        $tpSizeGB = Format-GB -Bytes $tpSize
        if (Confirm-CleanAction -Description "清空 tmp_podman 目录内容" -Path $tmpPodmanPath -EstimatedGB $tpSizeGB) {
            Clear-DirectoryContents -Path $tmpPodmanPath
            Write-Ok "tmp_podman 内容已清空（保留目录）"
        }
    } else {
        Write-Host "`n  [跳过] $tmpPodmanPath 不存在" -ForegroundColor DarkGray
    }

    # --- 步骤 2: 删除 fuzz\target 构建产物（保留 fuzz 源码） ---
    $fuzzTargetPath = Join-Path $projectRoot 'fuzz\target'
    if (Test-Path -LiteralPath $fuzzTargetPath) {
        $ftSize = (Get-FolderSize -Path $fuzzTargetPath).Size
        $ftSizeGB = Format-GB -Bytes $ftSize
        if (Confirm-CleanAction -Description "删除 fuzz\target 构建产物" -Path $fuzzTargetPath -EstimatedGB $ftSizeGB) {
            Remove-FullDirectory -Path $fuzzTargetPath
            Write-Ok "fuzz\target 已删除（保留 fuzz 源码）"
        }
    } else {
        Write-Host "`n  [跳过] $fuzzTargetPath 不存在" -ForegroundColor DarkGray
    }

    # --- 步骤 3: 删除 %SystemDrive% 异常目录（如果存在） ---
    # 注意：目录名包含百分号，使用单引号字面量 + -LiteralPath 处理
    # Join-Path 不会展开 %SystemDrive%，因为它不是 PowerShell 环境变量语法
    $systemDrivePath = Join-Path $projectRoot '%SystemDrive%'
    if (Test-Path -LiteralPath $systemDrivePath) {
        $sdSize = (Get-FolderSize -Path $systemDrivePath).Size
        $sdSizeGB = Format-GB -Bytes $sdSize
        if (Confirm-CleanAction -Description "删除异常目录 %SystemDrive%" -Path $systemDrivePath -EstimatedGB $sdSizeGB) {
            Remove-FullDirectory -Path $systemDrivePath
            Write-Ok "%SystemDrive% 异常目录已删除"
        }
    } else {
        Write-Host "`n  [跳过] $systemDrivePath 不存在" -ForegroundColor DarkGray
    }

    # --- 打印实际释放的空间 ---
    $freeAfter_D = Get-DiskFreeGB -DriveLetter 'D:'
    $freed_D = [math]::Round($freeAfter_D - $freeBefore_D, 3)

    Write-Section "清理结果"
    Write-Host "  D 盘: $freeBefore_D GB → $freeAfter_D GB (释放 $freed_D GB)" -ForegroundColor Green
    Write-Host "`n=== ProjectClean 完成 ===" -ForegroundColor Cyan
}

# ============================================================
# 主分发
# ============================================================

switch ($Mode) {
    'Diagnose'     { Invoke-Diagnose }
    'SafeClean'    { Invoke-SafeClean }
    'ProjectClean' { Invoke-ProjectClean }
}
