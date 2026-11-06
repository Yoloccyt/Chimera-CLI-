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
    [ValidateSet('Diagnose','SafeClean','ProjectClean','DeepClean')]
    [string]$Mode = 'Diagnose',

    [switch]$Force,

    [int]$TopN = 15,

    # DeepClean 专用：只报告不删除（DryRun 与实际执行严格分离）
    [switch]$DryRun,

    # DeepClean 专用：Repo=仓库内可再生缓存；Drive=C/D 盘开发链缓存；All=两者
    [ValidateSet('Repo','Drive','All')]
    [string]$Scope = 'Repo',

    # DeepClean 专用：高风险项显式启用（fuzz-target / tower / rustup-unused /
    # cargo-autoclean / recycle-d / legacy-rustup / c-pkg-caches / d-legacy-ide）
    [string[]]$Include = @()
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

    # --- 步骤 4.5: cargo cache --autoclean（仅在 cargo 可用时执行） ---
    # WHY E6(报告附录 E): .toolchain 下 cargo 缓存(registry/src)会随依赖迭代持续膨胀,
    #      cargo cache --autoclean 仅清理过时/损坏条目,不影响当前构建所需缓存。
    $cargoAvailable = $null -ne (Get-Command cargo -ErrorAction SilentlyContinue)
    if ($cargoAvailable) {
        $cargoCacheSizeGB = [double]0
        $cargoHome = $env:CARGO_HOME
        if (-not $cargoHome) { $cargoHome = Join-Path $env:USERPROFILE '.cargo' }
        if (Test-Path -LiteralPath $cargoHome) {
            $cargoCacheSize = (Get-FolderSize -Path $cargoHome).Size
            $cargoCacheSizeGB = Format-GB -Bytes $cargoCacheSize
        }
        if (Confirm-CleanAction -Description "cargo cache --autoclean" -Path $cargoHome -EstimatedGB $cargoCacheSizeGB) {
            try {
                cargo cache --autoclean 2>&1 | Out-Null
                Write-Ok "cargo 缓存已清理 (autoclean)"
            } catch {
                Write-Warning "cargo 缓存清理失败: $_"
            }
        }
    } else {
        Write-Host "`n  [跳过] cargo 不可用，跳过 cargo cache clean" -ForegroundColor DarkGray
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
# DeepClean 模式函数（幂等 + DryRun 分离 + 白名单守卫）
# 设计依据: dev-chain-safe-cleanup-plan §九（复用而非新造）
# ============================================================

# 脚本所在仓库根（DeepClean 白名单以它为界）
$script:DeepCleanRepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

# 仓库内绝对受保护目录（DeepClean 绝不触碰；注意不包含 scripts/fuzz，因其下的
# __pycache__ 与 target 才是清理对象）
$script:DeepCleanProtected = @(
    '.toolchain','.git','crates','docs','src','tests','examples',
    '.cargo','.config','.github','.trae','.claude',
    'Aegis','dsh-taskboard','archives','assets','canvases'
)

# 校验仓库内路径是否可删（越界/命中保护目录则拒绝）
function Assert-DeepCleanSafePath {
    param([string]$Path)
    $root = [System.IO.Path]::GetFullPath($script:DeepCleanRepoRoot).TrimEnd('\','/')
    $full = [System.IO.Path]::GetFullPath($Path).TrimEnd('\','/')
    if ($full -eq $root) { throw "拒绝：$Path 指向仓库根" }
    if (-not $full.StartsWith($root + [System.IO.Path]::DirectorySeparatorChar)) { throw "拒绝：$Path 不在仓库内" }
    foreach ($pd in $script:DeepCleanProtected) {
        $p = [System.IO.Path]::GetFullPath((Join-Path $root $pd)).TrimEnd('\','/')
        if ($full -eq $p -or $full.StartsWith($p + [System.IO.Path]::DirectorySeparatorChar)) {
            throw "拒绝：$Path 命中受保护目录 $pd"
        }
    }
}

# 统一的单一目标清理：DryRun 只报告；Test-Path 守卫；失败仅告警；幂等
function Clear-DeepCleanTarget {
    param(
        [string]$Path,
        [string]$Category = 'cache',
        [string]$KeepIf = '',
        [switch]$RemoveDir,
        [switch]$SkipWhitelist
    )
    if (-not $SkipWhitelist) { Assert-DeepCleanSafePath -Path $Path }
    if (-not (Test-Path -LiteralPath $Path)) {
        Write-Host "  [SKIP ] $Category $Path (不存在)" -ForegroundColor DarkGray
        return
    }
    $sizeGB = Format-GB -Bytes (Get-FolderSize -Path $Path).Size
    if ($DryRun) {
        Write-Host "  [DRY  ] $Category $Path (~$sizeGB GB) — 将删除" -ForegroundColor Yellow
        return
    }
    Write-Host "  [CLEAN] $Category $Path (~$sizeGB GB)" -ForegroundColor Yellow
    if ($RemoveDir) {
        Remove-FullDirectory -Path $Path
    } else {
        Get-ChildItem -LiteralPath $Path -Force -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -ne $KeepIf } |
            ForEach-Object { Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction SilentlyContinue }
    }
}

function Invoke-DeepClean {
    Write-Host "`n************************************************" -ForegroundColor Cyan
    Write-Host "  Chimera CLI 深度清理 (DeepClean · Scope=$Scope · DryRun=$DryRun)" -ForegroundColor Cyan
    Write-Host "************************************************" -ForegroundColor Cyan

    $freeBeforeC = Get-DiskFreeGB -DriveLetter 'C:'
    $freeBeforeD = Get-DiskFreeGB -DriveLetter 'D:'
    Write-SubStep "清理前 C 盘可用: $freeBeforeC GB | D 盘可用: $freeBeforeD GB"

    # 统一工具链 env（仅影响本脚本进程）
    if (Test-Path -LiteralPath (Join-Path $script:DeepCleanRepoRoot '.toolchain')) {
        $env:CARGO_HOME  = Join-Path $script:DeepCleanRepoRoot '.toolchain\cargo'
        $env:RUSTUP_HOME = Join-Path $script:DeepCleanRepoRoot '.toolchain\rustup'
    }

    $root = $script:DeepCleanRepoRoot

    # 兼容 CLI 传入的逗号分隔单串（-File 下 -Include a,b,c 会作为单个字符串绑定）
    $includeTokens = @()
    foreach ($raw in $Include) {
        foreach ($tok in ($raw -split ',')) {
            $t = $tok.Trim()
            if ($t) { $includeTokens += $t }
        }
    }

    # ---------------- Repo 作用域：仓库内可再生缓存 ----------------
    if ($Scope -in @('Repo','All')) {
        Write-Section 'Repo 作用域（仓库内可再生缓存）'

        # target/debug：优先 cargo clean --profile dev（保留 release/criterion）
        $dbg = Join-Path $root 'target\debug'
        if (Test-Path -LiteralPath $dbg) {
            $dbgGB = Format-GB -Bytes (Get-FolderSize -Path $dbg).Size
            if ($DryRun) {
                Write-Host "  [DRY  ] 构建缓存 target/debug (~$dbgGB GB) — 将执行 cargo clean --profile dev" -ForegroundColor Yellow
            } else {
                Write-Host "  [CLEAN] 构建缓存 target/debug (~$dbgGB GB)" -ForegroundColor Yellow
                if ($null -ne (Get-Command cargo -ErrorAction SilentlyContinue)) { & cargo clean --profile dev 2>&1 | Out-Null }
                # cargo clean 若因锁/占用失败，则回退到直接删除残留（仍受白名单守卫）
                if (Test-Path -LiteralPath $dbg) { Clear-DeepCleanTarget -Path $dbg -Category '构建缓存 target/debug(残留)' -RemoveDir }
            }
        } else {
            Write-Host "  [SKIP ] target/debug (不存在)" -ForegroundColor DarkGray
        }

        Clear-DeepCleanTarget -Path (Join-Path $root 'tmp_podman') -Category '临时 tmp_podman'
        Clear-DeepCleanTarget -Path (Join-Path $root 'tmp')      -Category '临时 tmp'
        Clear-DeepCleanTarget -Path (Join-Path $root 'scripts\__pycache__') -Category 'Python 缓存' -RemoveDir
        Clear-DeepCleanTarget -Path (Join-Path $root 'tmp\__pycache__')     -Category 'Python 缓存' -RemoveDir
        foreach ($s in @('.superpowers','.zcode','.workbuddy')) {
            Clear-DeepCleanTarget -Path (Join-Path $root $s) -Category "Agent scratch $s" -KeepIf '.gitignore'
        }

        # 高风险/需确认项：默认不删，显式 Include 才处理
        if ($includeTokens -contains 'fuzz-target') {
            Clear-DeepCleanTarget -Path (Join-Path $root 'fuzz\target') -Category 'fuzz 构建缓存' -RemoveDir
        }
        if ($includeTokens -contains 'tower') {
            Clear-DeepCleanTarget -Path (Join-Path $root '.tower') -Category '工具态 .tower'
        }
    }

    # ---------------- Drive 作用域：C/D 盘开发链缓存（高风险，逐项 Include 启用） ----------------
    if ($Scope -in @('Drive','All')) {
        Write-Section 'Drive 作用域（C/D 盘开发链缓存，按 -Include 逐项启用）'

        # rustup 未用工具链（保留 stable-x86_64-pc-windows-gnu）
        if ($includeTokens -contains 'rustup-unused') {
            if ($null -eq (Get-Command rustup -ErrorAction SilentlyContinue)) {
                Write-Host '  [SKIP ] rustup 不可用' -ForegroundColor DarkGray
            } else {
                $list = (& rustup toolchain list) 2>$null
                foreach ($t in @('stable-x86_64-pc-windows-msvc','nightly-x86_64-pc-windows-gnu','nightly-x86_64-pc-windows-msvc')) {
                    if ($list -match [regex]::Escape($t)) {
                        if ($DryRun) { Write-Host "  [DRY  ] rustup 卸载 $t" -ForegroundColor Yellow }
                        else { Write-Host "  [CLEAN] rustup 卸载 $t" -ForegroundColor Yellow; & rustup toolchain uninstall $t | Out-Null }
                    }
                }
            }
        }

        # cargo registry autoclean（仅清过时条目）
        if ($includeTokens -contains 'cargo-autoclean') {
            if ($null -eq (Get-Command cargo -ErrorAction SilentlyContinue)) { Write-Host '  [SKIP ] cargo 不可用' -ForegroundColor DarkGray }
            elseif ($DryRun) { Write-Host '  [DRY  ] cargo cache --autoclean' -ForegroundColor Yellow }
            else { Write-Host '  [CLEAN] cargo cache --autoclean' -ForegroundColor Yellow; & cargo cache --autoclean 2>&1 | Out-Null }
        }

        # D 盘回收站（不可逆）
        if ($includeTokens -contains 'recycle-d') {
            if ($DryRun) { Write-Host '  [DRY  ] 清空 D 盘回收站' -ForegroundColor Yellow }
            else { Write-Host '  [CLEAN] 清空 D 盘回收站' -ForegroundColor Yellow; Clear-RecycleBin -DriveLetter D -Force -ErrorAction SilentlyContinue }
        }

        # D:\.rustup 迁移前遗留（先改名观察，确认无影响后再删除）
        if ($includeTokens -contains 'legacy-rustup') {
            $legacy = 'D:\.rustup'
            if (-not (Test-Path -LiteralPath $legacy)) { Write-Host '  [SKIP ] D:\.rustup (不存在)' -ForegroundColor DarkGray }
            elseif ($DryRun) { Write-Host '  [DRY  ] D:\.rustup 改名为 .rustup.bak' -ForegroundColor Yellow }
            else {
                try {
                    if (Test-Path -LiteralPath 'D:\.rustup.bak') { Write-Warning '  D:\.rustup.bak 已存在，跳过改名' }
                    else { Rename-Item -LiteralPath $legacy -NewName '.rustup.bak' -ErrorAction Stop; Write-Host '  [CLEAN] D:\.rustup → .rustup.bak' -ForegroundColor Yellow }
                } catch { Write-Warning "  无法改名 D:\.rustup（需提权/沙箱外执行）: $($_.Exception.Message)" }
            }
        }

        # C 盘包管理/临时缓存（可能服务其他项目，显式 Include 才处理）
        if ($includeTokens -contains 'c-pkg-caches') {
            if ($null -ne (Get-Command npm -ErrorAction SilentlyContinue)) {
                if ($DryRun) { Write-Host '  [DRY  ] npm cache clean --force' -ForegroundColor Yellow }
                else { & npm cache clean --force 2>&1 | Out-Null; Write-Host '  [CLEAN] npm cache' -ForegroundColor Yellow }
            }
            $pipCmd = $null
            if ($null -ne (Get-Command pip -ErrorAction SilentlyContinue)) { $pipCmd = 'pip' }
            elseif ($null -ne (Get-Command py -ErrorAction SilentlyContinue)) { $pipCmd = 'py' }
            if ($pipCmd) {
                if ($DryRun) { Write-Host '  [DRY  ] pip cache purge' -ForegroundColor Yellow }
                elseif ($pipCmd -eq 'pip') { & pip cache purge 2>&1 | Out-Null; Write-Host '  [CLEAN] pip cache' -ForegroundColor Yellow }
                else { & py -m pip cache purge 2>&1 | Out-Null; Write-Host '  [CLEAN] pip cache (py)' -ForegroundColor Yellow }
            }
            Clear-DeepCleanTarget -Path (Join-Path $env:LOCALAPPDATA 'Temp')   -Category 'C 盘 Temp'      -SkipWhitelist
            Clear-DeepCleanTarget -Path (Join-Path $HOME '.m2\repository')    -Category 'Maven .m2'     -SkipWhitelist
            Clear-DeepCleanTarget -Path (Join-Path $HOME '.cargo')             -Category 'C 盘 .cargo 遗留' -SkipWhitelist
        }

        # C 盘 IDE 纯子缓存（仅 Cache/logs 等子目录，不碰设置/扩展本体）
        if ($includeTokens -contains 'c-ide-caches') {
            $ideDirs = @("$HOME\.vscode","$HOME\.qoder","$HOME\.qoder-cn","$HOME\.trae-cn","$HOME\.lingma","$HOME\.workbuddy")
            $cacheSubs = @('Cache','CachedData','Code Cache','GPUCache','logs','CachedExtensionVSIXs','CachedProfilesData')
            foreach ($ide in $ideDirs) {
                if (-not (Test-Path -LiteralPath $ide)) { continue }
                foreach ($sub in $cacheSubs) {
                    Clear-DeepCleanTarget -Path (Join-Path $ide $sub) -Category "IDE 子缓存 $ide\$sub" -SkipWhitelist
                }
            }
        }

        # C 盘通用缓存（.cache / .docker）
        if ($includeTokens -contains 'c-general-caches') {
            Clear-DeepCleanTarget -Path (Join-Path $HOME '.cache')  -Category 'C 盘 .cache'  -SkipWhitelist
            Clear-DeepCleanTarget -Path (Join-Path $HOME '.docker') -Category 'C 盘 .docker' -SkipWhitelist
        }

        # D 盘旧 IDE 安装（需人工确认已弃用）
        if ($includeTokens -contains 'd-legacy-ide') {
            foreach ($d in @('D:\.vscode','D:\.lingma','D:\Qoder（全新）')) {
                Clear-DeepCleanTarget -Path $d -Category "D 盘旧 IDE $d" -RemoveDir -SkipWhitelist
            }
        }
    }

    # ---------------- 结果 ----------------
    $freeAfterC = Get-DiskFreeGB -DriveLetter 'C:'
    $freeAfterD = Get-DiskFreeGB -DriveLetter 'D:'
    Write-Section 'DeepClean 结果'
    Write-Host ("  C 盘: {0} GB → {1} GB" -f $freeBeforeC, $freeAfterC) -ForegroundColor Green
    Write-Host ("  D 盘: {0} GB → {1} GB" -f $freeBeforeD, $freeAfterD) -ForegroundColor Green
    if ($DryRun) { Write-Host "`n  [DRY-RUN] 未删除任何文件；去掉 -DryRun 后按上述清单执行。" -ForegroundColor Cyan }
    Write-Host "`n=== DeepClean 完成 ===" -ForegroundColor Cyan
}

# ============================================================
# 主分发
# ============================================================

switch ($Mode) {
    'Diagnose'     { Invoke-Diagnose }
    'SafeClean'    { Invoke-SafeClean }
    'ProjectClean' { Invoke-ProjectClean }
    'DeepClean'    { Invoke-DeepClean }
}
