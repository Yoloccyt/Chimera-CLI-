#Requires -Version 5.1
# ============================================================
# Chimera CLI (NEXUS-OMEGA) — 一键安装脚本 (Windows PowerShell)
#
# 用法:
#   iex (irm https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/master/install.ps1)
#   .\install.ps1 [-Version <ver>] [-InstallDir <path>] [-SkipVerify]
#
# 功能:
#   - 自动检测架构 (x86_64/aarch64)
#   - 从 GitHub Release 下载 chimera-windows-x86_64.exe
#   - 安装到 $env:LOCALAPPDATA\Programs\chimera\ (默认)
#   - 添加到用户 PATH (通过 [Environment]::SetEnvironmentVariable)
#   - 验证安装: chimera --version
# ============================================================

# param 块必须紧跟注释区,在任何可执行代码之前
param(
    [string]$Version = '',
    [string]$InstallDir = '',
    [switch]$SkipVerify
)

# 严格模式: 捕获未定义变量、强制类型转换失败
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ------------------ 配置常量 ------------------
# InstallDir 默认值需要延迟到运行时计算 (依赖 $env:LOCALAPPDATA)
if ([string]::IsNullOrEmpty($InstallDir)) {
    $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\chimera'
}

$script:RepoOwner = 'Yoloccyt'
$script:RepoName = 'Chimera-CLI-'
$script:GitHubApi = "https://api.github.com/repos/$($script:RepoOwner)/$($script:RepoName)"
$script:GitHubReleases = "https://github.com/$($script:RepoOwner)/$($script:RepoName)/releases"
$script:BinName = 'chimera'

# ------------------ 颜色输出函数 ------------------
function Write-Info    { param([string]$Msg) Write-Host "[INFO] $Msg" -ForegroundColor Cyan }
function Write-Ok      { param([string]$Msg) Write-Host "[OK] $Msg" -ForegroundColor Green }
function Write-WarnMsg { param([string]$Msg) Write-Host "[WARN] $Msg" -ForegroundColor Yellow }
function Write-ErrMsg  { param([string]$Msg) Write-Host "[ERROR] $Msg" -ForegroundColor Red }
function Die {
    param([string]$Msg)
    Write-ErrMsg $Msg
    exit 1
}

# ------------------ 平台/架构检测 ------------------
if (-not [System.Environment]::Is64BitOperatingSystem) {
    Die '不支持 32 位操作系统 (仅支持 x86_64 / aarch64)'
}

# 检测处理器架构 (AMD64 即 x86_64,ARM64 即 aarch64)
$processorArch = $env:PROCESSOR_ARCHITECTURE
if (-not $processorArch) {
    $processorArch = [System.Environment]::GetEnvironmentVariable('PROCESSOR_ARCHITECTURE')
}

$archNorm = $null
if ($processorArch -match 'AMD64|X64') {
    $archNorm = 'x86_64'
} elseif ($processorArch -eq 'ARM64') {
    $archNorm = 'aarch64'
} elseif ($processorArch -eq 'ARM') {
    Die '不支持 32 位 ARM (仅支持 x86_64 / aarch64)'
} else {
    Die "不支持的架构: $processorArch (仅支持 x86_64 / aarch64)"
}

# Windows 当前 Release 仅有 x86_64 binary (release.yml matrix)
# Windows 11 ARM 可通过 x86_64 兼容层运行
if ($archNorm -ne 'x86_64') {
    Write-WarnMsg "检测到架构 $archNorm,但当前 Release 仅有 Windows x86_64 binary"
    Write-WarnMsg '将使用 x86_64 binary (Windows 11 ARM 可通过 x86_64 兼容层运行)'
    $archNorm = 'x86_64'
}

$artifactName = "$($script:BinName)-windows-$archNorm.exe"
Write-Info "检测到平台: windows / $archNorm"
Write-Info "目标产物: $artifactName"

# ------------------ 版本解析 ------------------
# 若未指定版本,通过 GitHub API 获取 latest
if ([string]::IsNullOrEmpty($Version)) {
    Write-Info '未指定版本,正在获取最新版本号...'
    try {
        $headers = @{ 'User-Agent' = 'chimera-install-script' }
        # 私有仓库支持: 若设置了 GITHUB_TOKEN,使用鉴权
        if ($env:GITHUB_TOKEN) {
            $headers['Authorization'] = "Bearer $($env:GITHUB_TOKEN)"
        }
        $release = Invoke-RestMethod -Uri "$($script:GitHubApi)/releases/latest" -Headers $headers -ErrorAction Stop
        $Version = $release.tag_name
        if ([string]::IsNullOrEmpty($Version)) {
            Die '无法解析最新版本号 (仓库可能未发布 Release)'
        }
        Write-Info "最新版本: $Version"
    } catch {
        Die "无法访问 GitHub API: $($_.Exception.Message)
可能原因:
  1) 网络连接问题
  2) 仓库为私有 (需设置 `$env:GITHUB_TOKEN)
  3) GitHub API 速率限制"
    }
} else {
    Write-Info "指定版本: $Version"
}

# ------------------ 下载链接构造 ------------------
$downloadUrl = "$($script:GitHubReleases)/download/$Version/$artifactName"
Write-Info "下载链接: $downloadUrl"

# ------------------ 创建临时目录 ------------------
$tempDir = Join-Path $env:TEMP "chimera-install-$(Get-Random)"
if (-not (Test-Path $tempDir)) {
    New-Item -ItemType Directory -Path $tempDir -Force | Out-Null
}

try {
    # ------------------ 下载 binary ------------------
    $downloadedFile = Join-Path $tempDir $artifactName
    Write-Info "正在下载 $artifactName ..."

    try {
        $headers = @{ 'User-Agent' = 'chimera-install-script' }
        if ($env:GITHUB_TOKEN) {
            $headers['Authorization'] = "Bearer $($env:GITHUB_TOKEN)"
        }
        # UseBasicParsing 兼容旧 PowerShell,禁用 IE 引擎依赖
        Invoke-WebRequest -Uri $downloadUrl -OutFile $downloadedFile -Headers $headers `
            -UseBasicParsing -ErrorAction Stop
        $fileSize = (Get-Item $downloadedFile).Length
        if ($fileSize -eq 0) {
            Die '下载文件为空 (鉴权失败? 请设置 $env:GITHUB_TOKEN)'
        }
        $fileSizeMB = [math]::Round($fileSize / 1MB, 2)
        Write-Ok "下载完成: $fileSizeMB MB"
    } catch {
        Die "下载失败 (URL: $downloadUrl)
错误: $($_.Exception.Message)
可能原因:
  1) 版本不存在 (检查 -Version 参数)
  2) 仓库为私有 (需设置 `$env:GITHUB_TOKEN)
  3) 网络连接问题"
    }

    # ------------------ SHA256 校验 (可选) ------------------
    if (-not $SkipVerify) {
        $checksumUrl = "$($script:GitHubReleases)/download/$Version/checksums.txt"
        Write-Info '尝试下载 checksums.txt 进行 SHA256 校验...'
        $checksumFile = Join-Path $tempDir 'checksums.txt'
        $checksumAvailable = $false
        try {
            $headers = @{ 'User-Agent' = 'chimera-install-script' }
            if ($env:GITHUB_TOKEN) {
                $headers['Authorization'] = "Bearer $($env:GITHUB_TOKEN)"
            }
            Invoke-WebRequest -Uri $checksumUrl -OutFile $checksumFile -Headers $headers `
                -UseBasicParsing -ErrorAction Stop
            if ((Get-Item $checksumFile).Length -gt 0) {
                $checksumAvailable = $true
            }
        } catch {
            Write-WarnMsg 'Release 未附带 checksums.txt,跳过 SHA256 校验'
        }

        if ($checksumAvailable) {
            $checksumContent = Get-Content $checksumFile -Raw
            $expectedHash = $null
            foreach ($line in ($checksumContent -split "`n")) {
                # 匹配 "HASH  filename" 或 "HASH *filename"
                if ($line -match "^\s*([0-9a-fA-F]{64})\s+\*?$([regex]::Escape($artifactName))\s*$") {
                    $expectedHash = $matches[1].ToLower()
                    break
                }
            }

            if ($expectedHash) {
                $actualHash = (Get-FileHash -Path $downloadedFile -Algorithm SHA256).Hash.ToLower()
                if ($expectedHash -eq $actualHash) {
                    Write-Ok 'SHA256 校验通过'
                } else {
                    Die "SHA256 校验失败
  期望: $expectedHash
  实际: $actualHash"
                }
            } else {
                Write-WarnMsg "checksums.txt 中未找到 $artifactName,跳过校验"
            }
        }
    } else {
        Write-WarnMsg '已通过 -SkipVerify 跳过校验'
    }

    # ------------------ 安装目录准备 ------------------
    if (-not (Test-Path $InstallDir)) {
        try {
            New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
        } catch {
            Die "无法创建目录 $InstallDir : $($_.Exception.Message)"
        }
    }

    # ------------------ 安装 binary ------------------
    $installPath = Join-Path $InstallDir "$($script:BinName).exe"
    Write-Info "安装到: $installPath"

    try {
        Copy-Item -Path $downloadedFile -Destination $installPath -Force -ErrorAction Stop
    } catch {
        Die "安装失败 (权限不足?): $($_.Exception.Message)"
    }

    Write-Ok 'binary 已安装'

    # ------------------ PATH 配置 ------------------
    $pathUpdated = $false
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $userPath) { $userPath = '' }

    # 检查 InstallDir 是否已在用户 PATH 中 (大小写不敏感)
    $alreadyInPath = $false
    $pathParts = $userPath -split ';' | Where-Object { $_ -ne '' }
    foreach ($part in $pathParts) {
        if ($part -ieq $InstallDir) {
            $alreadyInPath = $true
            break
        }
    }

    if (-not $alreadyInPath) {
        try {
            $newPath = if ($userPath) { "$InstallDir;$userPath" } else { $InstallDir }
            [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
            $pathUpdated = $true
            Write-Info 'PATH 已更新 (用户级)'
            # 当前会话也立即生效
            $env:Path = "$InstallDir;$env:Path"
        } catch {
            Write-WarnMsg "无法更新 PATH: $($_.Exception.Message)"
            Write-WarnMsg "请手动将 $InstallDir 添加到 PATH"
        }
    }

    if ($pathUpdated) {
        Write-WarnMsg '请重启 PowerShell 终端以使 PATH 全局生效'
    }

    # ------------------ 验证安装 ------------------
    Write-Info '验证安装...'
    try {
        $versionOutput = & $installPath --version 2>&1
        if ($LASTEXITCODE -eq 0) {
            Write-Ok '安装成功!'
            Write-Host "  $versionOutput" -ForegroundColor DarkGray
        } else {
            Write-WarnMsg "$installPath --version 退出码: $LASTEXITCODE"
            Write-WarnMsg "请手动执行: $installPath --version"
        }
    } catch {
        Write-WarnMsg "$installPath --version 执行失败: $($_.Exception.Message)"
        Write-WarnMsg '可能缺少运行时依赖 (Visual C++ Redistributable)'
    }

    # ------------------ 总结输出 ------------------
    Write-Host ''
    Write-Info '================ 安装总结 ================'
    Write-Info "  版本:   $Version"
    Write-Info "  路径:   $installPath"
    Write-Info "  平台:   windows/$archNorm"
    if ($pathUpdated) {
        Write-Info '  PATH:   已更新 (用户级)'
    }
    Write-Info '=========================================='
    Write-Host ''
    Write-Ok "执行 'chimera --help' 开始使用"

} finally {
    # ------------------ 清理临时目录 ------------------
    if (Test-Path $tempDir) {
        Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
