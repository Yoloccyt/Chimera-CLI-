#Requires -Version 5.1
# ============================================================
# chimela CLI (NEXUS-OMEGA) — 一键安装脚本 (Windows PowerShell)
#
# 用法:
#   一行命令(PS 5.1 / PS 7+ 均兼容):
#     & ([scriptblock]::Create((Invoke-RestMethod https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.ps1)))
#
#   多行(推荐,适用于所有版本):
#     $f="$env:TEMP\chimela-install.ps1";irm https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.ps1 -OutFile $f;& $f;ri $f -Force
#
#   本地执行:
#     .\install.ps1 [-Version <ver>] [-InstallDir <path>] [-Proxy <url>] [-LocalFile <path>] [-SkipVerify] [-SkipVersionCheck] [-SetupEnv]
#
# 企业网络/代理场景:
#   .\install.ps1 -Proxy 'http://proxy.company.com:8080'
#
# 离线/手动下载场景:
#   .\install.ps1 -LocalFile 'C:\Users\$env:USERNAME\Downloads\chimela-windows-x86_64.exe' -Version v1.5.8-omega
#
# 私有仓库安装(需 $env:GITHUB_TOKEN 环境变量鉴权):
#   WHY: raw.githubusercontent.com 对私有仓库 raw 内容拒绝匿名访问,
#        必须显式在 HTTP header 中传递 Authorization: Bearer <token>。
#        仅设置环境变量不会自动被 irm 加入 header。
#
#   PowerShell 5.1+:
#     $env:GITHUB_TOKEN='ghp_xxx'
#     $headers = @{ Authorization = "Bearer $env:GITHUB_TOKEN" }
#     $tempFile = Join-Path $env:TEMP "chimela-install.ps1"; irm https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.ps1 -Headers $headers | Out-File -FilePath $tempFile -Encoding utf8; & $tempFile; Remove-Item $tempFile -Force
#
#   如果 irm 持续 404,建议直接克隆仓库后本地执行:
#     git clone https://github.com/Yoloccyt/Chimera-CLI-.git
#     cd Chimera-CLI-
#     $env:GITHUB_TOKEN='ghp_xxx'; .\install.ps1
#
# 参数说明:
#   -Version <ver>      指定版本 (默认: latest,如 v1.5.8-omega)
#   -InstallDir <path>  安装目录 (默认: $env:LOCALAPPDATA\Programs\chimela)
#   -Proxy <url>        HTTP/HTTPS 代理地址 (如 http://proxy.company.com:8080)
#   -LocalFile <path>   使用预先下载的本地 binary 安装,跳过所有网络请求
#   -SkipVerify         跳过 SHA256 校验
#   -SkipVersionCheck   跳过 --version 验证(企业安全策略拦截首次运行等场景)
#   -SetupEnv           仅设置工具链环境变量后退出,不下载 binary
#                       (用于源码构建场景,覆盖 §10.5 "环境变量手动设置"短板)
#
# 功能:
#   - 自动检测架构 (x86_64/aarch64,ARM64 降级 x86_64 兼容层)
#   - 从 GitHub Release 下载 chimela-windows-x86_64.exe
#   - 可选 SHA256 校验 (若 Release 附带 checksums.txt)
#   - 安装到 $env:LOCALAPPDATA\Programs\chimela\ (默认)
#   - 同时生成 chimela.exe / aether.exe / chimera.exe 三个入口
#     (chimela 为新品牌名,aether 为 cargo 二进制名,chimera 为兼容别名)
#   - 添加到用户 PATH (通过 [Environment]::SetEnvironmentVariable)
#   - 验证安装: chimela --version / aether --version / chimera --version (正则 ^(aether|chimera|chimela) \d+\.\d+\.\d+)
#   - (-SetupEnv) 自动注入 CARGO_HOME/RUSTUP_HOME/PATH 到用户级
#
# 退出码:
#   0  安装成功(或 -SetupEnv 模式下环境变量设置成功)
#   1  安装失败(网络/鉴权/校验/权限错误,见 [ERROR] 输出)
#
# 与 release.yml 一致性:
#   - artifact 命名: chimela-windows-x86_64.exe (匹配 release.yml matrix)
#   - --version 正则: ^(aether|chimera|chimela) \d+\.\d+\.\d+ (匹配 docker job grep)
#   - checksums.txt 格式: 兼容 "HASH  file" / "HASH *file" (匹配 printf '%s  %s\n')
# ============================================================

# param 块必须紧跟注释区,在任何可执行代码之前
param(
    [string]$Version = '',
    [string]$InstallDir = '',
    [string]$Proxy = '',
    [string]$LocalFile = '',
    [switch]$SkipVerify,
    [switch]$SkipVersionCheck,
    [switch]$SetupEnv
)

# 严格模式: 捕获未定义变量、强制类型转换失败
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
# WHY: PS 5.1 的 Invoke-WebRequest 进度条更新会严重拖慢大文件下载,
#      在企业网络场景下可能导致超时。静默进度条可显著改善下载性能。
$ProgressPreference = 'SilentlyContinue'

# ------------------ 网络层健壮性初始化 ------------------
# WHY: PowerShell 5.1 默认仅启用 TLS 1.0 / SSL3,而 GitHub 等现代 CDN 要求 TLS 1.2+。
#      企业 DPI 防火墙对旧 TLS 握手进行深度检测并阻断,强制 TLS 1.2 可绕过此类检测,
#      与 Podman 安装经验一致([Net.SecurityProtocolType]::Tls12 可成功下载 GitHub 资源)。
#      此设置必须在任何 Invoke-WebRequest/Invoke-RestMethod 调用之前生效。
try {
    $currentProtocols = [Net.ServicePointManager]::SecurityProtocol
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13
    # 仅在成功设置后才输出,避免在 -SetupEnv 等静默模式下产生噪音
    if (-not $SetupEnv) {
        Write-Host "[INFO] 已强制启用 TLS 1.2/1.3 (原始协议: $currentProtocols)" -ForegroundColor Cyan
    }
} catch {
    Write-Warning "无法强制 TLS 1.2/1.3: $($_.Exception.Message)"
}

# 代理支持: 构造统一的 Invoke-WebRequest 通用参数 splat
$script:CommonWebRequestParams = @{
    UseBasicParsing = $true
    ErrorAction     = 'Stop'
}
if (-not [string]::IsNullOrEmpty($Proxy)) {
    $script:CommonWebRequestParams['Proxy'] = $Proxy
    Write-Host "[INFO] 使用代理: $Proxy" -ForegroundColor Cyan
}

# ------------------ 工具链环境变量自动化 ------------------
# WHY: 新克隆者需手动设置 CARGO_HOME/RUSTUP_HOME/PATH 才能编译,
#      此函数将工具链 env 注入用户级环境变量,持久化避免每次新 shell 重复设置。
#      路径基于 $PSScriptRoot 推导,支持仓库位于任意盘符;MINGW 通过 gcc 探测。
#      仅 Windows 平台适用。
function Set-Environment {
    <#
    .SYNOPSIS
        自动设置 Chimera CLI 工具链环境变量(CARGO_HOME/RUSTUP_HOME/PATH/TMP/TEMP)
    .DESCRIPTION
        基于脚本所在目录推导项目根目录,将 .toolchain\cargo\bin 与 MSYS2 mingw64\bin
        注入用户级环境变量,持久化保存。同时同步当前会话,避免必须重启终端。
        覆盖 §10.5 已知基建短板"环境变量仍需手动设置"。
    .EXAMPLE
        .\install.ps1 -SetupEnv
    #>
    # WHY: 仓库可能在 D 盘以外的位置,不能写死路径。优先使用 $PSScriptRoot;
    #      若通过 iex(irm...) 执行导致为空,则回退到历史默认路径保持兼容。
    $projectRoot = if ($PSScriptRoot) { $PSScriptRoot } else { 'D:\Chimera CLI' }
    $toolchainRoot = Join-Path $projectRoot '.toolchain'
    $toolchainCargo = Join-Path $toolchainRoot 'cargo\bin'
    $toolchainCargoHome = Join-Path $toolchainRoot 'cargo'
    $toolchainRustupHome = Join-Path $toolchainRoot 'rustup'

    # WHY: MSYS2 也可能安装在其他位置,优先用 where.exe gcc 探测真实路径;
    #      探测失败再回退到默认 D:\msys64\mingw64\bin。
    $mingwBin = 'D:\msys64\mingw64\bin'
    $gccCommand = Get-Command gcc -ErrorAction SilentlyContinue
    if ($gccCommand -and $gccCommand.Source) {
        $detectedMingwBin = Split-Path -Parent $gccCommand.Source
        if ($detectedMingwBin -and (Test-Path (Join-Path $detectedMingwBin 'gcc.exe'))) {
            $mingwBin = $detectedMingwBin
            Write-Info "探测到 MINGW: $mingwBin"
        }
    } elseif (-not (Test-Path (Join-Path $mingwBin 'gcc.exe'))) {
        Write-WarnMsg '未找到 gcc.exe,将使用默认 MINGW 路径;若后续 cargo build 失败请检查 MSYS2 安装'
    }

    # 设置 CARGO_HOME / RUSTUP_HOME(用户级,持久化)
    [Environment]::SetEnvironmentVariable('CARGO_HOME', $toolchainCargoHome, 'User')
    [Environment]::SetEnvironmentVariable('RUSTUP_HOME', $toolchainRustupHome, 'User')
    # 同步当前会话,避免用户必须重启终端才能执行 cargo/rustup
    $env:CARGO_HOME = $toolchainCargoHome
    $env:RUSTUP_HOME = $toolchainRustupHome
    Write-Ok "已设置 CARGO_HOME=$toolchainCargoHome"
    Write-Ok "已设置 RUSTUP_HOME=$toolchainRustupHome"

    # WHY: 项目构建临时文件默认重定向到项目根目录 tmp,避免 C 盘空间不足或权限问题。
    $tmpDir = Join-Path $projectRoot 'tmp'
    if (Test-Path $tmpDir) {
        [Environment]::SetEnvironmentVariable('TMP', $tmpDir, 'User')
        [Environment]::SetEnvironmentVariable('TEMP', $tmpDir, 'User')
        $env:TMP = $tmpDir
        $env:TEMP = $tmpDir
        Write-Ok "已设置 TMP/TEMP=$tmpDir"
    } else {
        Write-WarnMsg "未找到项目 tmp 目录 $tmpDir,跳过 TMP/TEMP 重定向"
    }

    # 检查并注入 PATH(避免重复添加)
    $currentPath = [Environment]::GetEnvironmentVariable('PATH', 'User')
    if (-not $currentPath) { $currentPath = '' }
    if ($currentPath -notlike "*$toolchainCargo*") {
        $newPath = "$toolchainCargo;$mingwBin;$currentPath"
        if ($newPath.Length -gt 8191) {
            Write-WarnMsg "用户 PATH 过长 ($($newPath.Length) 字符),超过 8191 安全上限"
            Write-WarnMsg "请手动将以下目录添加到 PATH: $toolchainCargo; $mingwBin"
        } else {
            [Environment]::SetEnvironmentVariable('PATH', $newPath, 'User')
            $env:Path = "$toolchainCargo;$mingwBin;$env:Path"
            Write-Ok "已将 $toolchainCargo 注入用户 PATH"
        }
    } else {
        Write-WarnMsg "$toolchainCargo 已在 PATH 中"
    }

    Write-Host ""
    Write-Host "用户级环境变量已持久化,当前会话也已同步。" -ForegroundColor Cyan
    Write-Host "如仍无法识别 cargo/rustup,请重新打开 PowerShell 终端。" -ForegroundColor Cyan
}

# ------------------ 配置常量 ------------------
# InstallDir 默认值需要延迟到运行时计算 (依赖 $env:LOCALAPPDATA)
if ([string]::IsNullOrEmpty($InstallDir)) {
    $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\chimela'
}

$script:RepoOwner = 'Yoloccyt'
$script:RepoName = 'Chimera-CLI-'
$script:GitHubApi = "https://api.github.com/repos/$($script:RepoOwner)/$($script:RepoName)"
$script:GitHubReleases = "https://github.com/$($script:RepoOwner)/$($script:RepoName)/releases"
$script:BinName = 'chimela'

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

# -SetupEnv 短路: 仅设置环境变量后立即退出,不进入下载安装流程
# WHY: 必须放在 Write-Ok/Write-WarnMsg 等函数定义之后,确保 PowerShell 7 的
#      顺序执行模式下函数已可用。
if ($SetupEnv) {
    Set-Environment
    exit 0
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

# ------------------ 本地文件模式短路 ------------------
# WHY: 企业网络常阻断 GitHub releases 直连,允许用户预先把 binary 下载到本地,
#      然后通过 -LocalFile 参数直接安装,跳过所有网络请求。
$localFileMode = $false
if (-not [string]::IsNullOrEmpty($LocalFile)) {
    $localFileResolved = Resolve-Path $LocalFile -ErrorAction SilentlyContinue
    if (-not $localFileResolved -or -not (Test-Path $localFileResolved.Path)) {
        Die "指定的本地文件不存在: $LocalFile"
    }
    $localFileMode = $true
    if ([string]::IsNullOrEmpty($Version)) {
        $Version = 'local'
    }
    Write-Info "本地文件模式: $($localFileResolved.Path)"
    Write-Info "版本标记: $Version"
}

# ------------------ 版本号归一化 ------------------
# WHY: 用户常输入 1.5.8-omega 而非 v1.5.8-omega,直接拼入 Release URL 会导致 404。
#      此处自动补 v 前缀,同时保留 latest/local 等特殊标记,避免无意义失败。
if (-not [string]::IsNullOrEmpty($Version) -and $Version -ne 'local' -and $Version -notmatch '^v\d') {
    if ($Version -match '^\d+\.\d+\.\d+') {
        $Version = "v$Version"
        Write-Info "版本号已归一化为: $Version"
    }
}

# ------------------ 版本解析 ------------------
# 若未指定版本且非本地文件模式,通过 GitHub API 获取 latest
if (-not $localFileMode -and [string]::IsNullOrEmpty($Version)) {
    Write-Info '未指定版本,正在获取最新版本号...'
    try {
        $headers = @{ 'User-Agent' = 'chimela-install-script' }
        # 私有仓库支持: 若设置了 GITHUB_TOKEN,使用鉴权
        if ($env:GITHUB_TOKEN) {
            $headers['Authorization'] = "Bearer $($env:GITHUB_TOKEN)"
        }
        $requestParams = @{ Uri = "$($script:GitHubApi)/releases/latest"; Headers = $headers; ErrorAction = 'Stop' }
        if ($script:CommonWebRequestParams.ContainsKey('Proxy')) {
            $requestParams['Proxy'] = $script:CommonWebRequestParams['Proxy']
        }
        $release = Invoke-RestMethod @requestParams
        $Version = $release.tag_name
        if ([string]::IsNullOrEmpty($Version)) {
            Die '无法解析最新版本号 (仓库可能未发布 Release)'
        }
        Write-Info "最新版本: $Version"
    } catch {
        Die "无法访问 GitHub API: $($_.Exception.Message)
可能原因与解决方案:
  1) 网络连接问题 → 检查能否访问 https://api.github.com
  2) 企业防火墙/DPI 阻断 → 使用 -Proxy 参数,或先手动下载后用 -LocalFile 安装
     示例: .\install.ps1 -Proxy 'http://proxy.company.com:8080'
     示例: .\install.ps1 -LocalFile 'C:\Users\$env:USERNAME\Downloads\chimela-windows-x86_64.exe' -Version v1.5.8-omega
  3) 仓库为私有 (需设置 `$env:GITHUB_TOKEN)
  4) GitHub API 速率限制"
    }
} elseif (-not $localFileMode) {
    Write-Info "指定版本: $Version"
}

# ------------------ 下载链接构造 ------------------
$downloadUrl = "$($script:GitHubReleases)/download/$Version/$artifactName"
if (-not $localFileMode) {
    Write-Info "下载链接: $downloadUrl"
}

# ------------------ 下载辅助函数 ------------------
# WHY: 企业网络/DPI 防火墙常见阻断策略是在数据传输阶段中断连接,单次重试不足。
#      此函数提供三层降级策略:
#        1. Invoke-WebRequest + 重试(默认,支持代理)
#        2. curl.exe (Win10+ 1803 内置,绕开 PS 5.1 WebRequest 限制)
#        3. Start-BitsTransfer (支持断点续传,适合不稳定网络)
#      每层失败后自动 fallback 到下一层,确保尽可能完成下载。
function Invoke-DownloadWithRetry {
    param(
        [string]$Url,
        [string]$OutFile,
        [hashtable]$Headers,
        [hashtable]$ProxyParams,
        [int]$TimeoutSec = 300,
        [int]$MaxRetries = 3
    )

    # ---- 1. Invoke-WebRequest 模式 (带重试 + 指数退避) ----
    for ($attempt = 1; $attempt -le $MaxRetries; $attempt++) {
        try {
            $requestParams = @{
                Uri             = $Url
                OutFile         = $OutFile
                Headers         = $Headers
                UseBasicParsing = $true
                ErrorAction     = 'Stop'
                TimeoutSec      = $TimeoutSec
            }
            if ($ProxyParams.ContainsKey('Proxy')) {
                $requestParams['Proxy'] = $ProxyParams['Proxy']
            }
            Invoke-WebRequest @requestParams
            $fileSize = (Get-Item $OutFile).Length
            if ($fileSize -eq 0) { throw '下载文件为空' }
            return 'Invoke-WebRequest'
        } catch {
            if ($attempt -lt $MaxRetries) {
                $waitSec = [math]::Pow(2, $attempt)  # 2s, 4s
                Write-WarnMsg "下载第 $attempt/$MaxRetries 次失败,${waitSec}s 后重试: $($_.Exception.Message)"
                Start-Sleep -Seconds $waitSec
            } else {
                Write-WarnMsg "下载第 $attempt/$MaxRetries 次失败: $($_.Exception.Message)"
            }
        }
    }

    # ---- 2. curl.exe 回退 (Win10+ 1803 内置,绕开 PS 5.1 限制) ----
    $curlPath = Get-Command curl.exe -ErrorAction SilentlyContinue
    if ($curlPath) {
        Write-WarnMsg '尝试 curl.exe 作为回退下载方式...'
        try {
            $curlArgs = @(
                '-L', '-o', $OutFile,
                '--max-time', $TimeoutSec,
                '--retry', $MaxRetries,
                '--retry-delay', 2
            )
            if ($Headers.ContainsKey('Authorization')) {
                $curlArgs += '-H'; $curlArgs += "Authorization: $($Headers['Authorization'])"
            }
            if ($ProxyParams.ContainsKey('Proxy')) {
                $curlArgs += '-x'; $curlArgs += $ProxyParams['Proxy']
            }
            $curlArgs += $Url
            & $curlPath @curlArgs 2>&1 | Out-Null
            if ($LASTEXITCODE -eq 0 -and (Get-Item $OutFile).Length -gt 0) {
                Write-Ok 'curl.exe 下载成功'
                return 'curl'
            }
        } catch {
            Write-WarnMsg "curl.exe 回退失败: $($_.Exception.Message)"
        }
    }

    # ---- 3. Start-BitsTransfer 回退 (支持断点续传,适合不稳定网络) ----
    # WHY: BITS 后台智能传输可自动恢复网络中断,但 Windows 11 默认禁用。
    #      不作为首选,仅作为 Invoke-WebRequest 和 curl 均失败时的最后尝试。
    try {
        Write-WarnMsg '尝试 Start-BitsTransfer 作为最终回退方式...'
        $bitsParams = @{
            Source          = $Url
            Destination     = $OutFile
            DisplayName     = 'chimela CLI Download'
            ErrorAction     = 'Stop'
        }
        if ($ProxyParams.ContainsKey('Proxy')) {
            $bitsParams['ProxyUsage'] = 'Override'
            $bitsParams['ProxyList'] = $ProxyParams['Proxy']
        }
        Start-BitsTransfer @bitsParams
        if ((Get-Item $OutFile).Length -gt 0) {
            Write-Ok 'Start-BitsTransfer 下载成功'
            return 'BITS'
        }
    } catch {
        Write-WarnMsg "Start-BitsTransfer 回退失败: $($_.Exception.Message)"
    }

    return $null  # 全部失败
}

# ------------------ 创建临时目录 ------------------
$tempDir = Join-Path $env:TEMP "chimela-install-$(Get-Random)"
if (-not (Test-Path $tempDir)) {
    New-Item -ItemType Directory -Path $tempDir -Force | Out-Null
}

try {
    # ------------------ 获取 binary ------------------
    $downloadedFile = Join-Path $tempDir $artifactName

    if ($localFileMode) {
        # WHY: 本地文件模式完全跳过网络,直接复制到临时目录参与后续安装流程
        Write-Info "正在复制本地 binary ..."
        Copy-Item -Path $localFileResolved.Path -Destination $downloadedFile -Force -ErrorAction Stop
        $fileSize = (Get-Item $downloadedFile).Length
        $fileSizeMB = [math]::Round($fileSize / 1MB, 2)
        Write-Ok "本地 binary 已复制: $fileSizeMB MB"
    } else {
        # ------------------ 下载 binary ------------------
        Write-Info "正在下载 $artifactName ..."

        try {
            $headers = @{ 'User-Agent' = 'chimela-install-script' }
            if ($env:GITHUB_TOKEN) {
                $headers['Authorization'] = "Bearer $($env:GITHUB_TOKEN)"
            }
            # 三层降级下载: Invoke-WebRequest(重试×3) → curl.exe → BITS
            $downloadParams = @{
                Url         = $downloadUrl
                OutFile     = $downloadedFile
                Headers     = $headers
                ProxyParams = $script:CommonWebRequestParams
                TimeoutSec  = 300
                MaxRetries  = 3
            }
            $downloadMethod = Invoke-DownloadWithRetry @downloadParams
            if (-not $downloadMethod) {
                Die "下载失败 (URL: $downloadUrl)
所有下载方式均失败,可能原因:
  1) 版本不存在 (检查 -Version 参数)
  2) 仓库为私有 (需设置 `$env:GITHUB_TOKEN)
  3) 网络连接问题 / 企业 DPI 阻断
     → 使用代理: .\install.ps1 -Proxy 'http://proxy.company.com:8080'
     → 手动下载 Release 中的 $artifactName,然后使用:
       .\install.ps1 -LocalFile '<下载路径>' -Version $Version"
            }
            $fileSize = (Get-Item $downloadedFile).Length
            $fileSizeMB = [math]::Round($fileSize / 1MB, 2)
            Write-Ok "下载完成: $fileSizeMB MB (方式: $downloadMethod)"
        } catch {
            Die "下载失败 (URL: $downloadUrl)
错误: $($_.Exception.Message)
所有下载方式均失败,可能原因:
  1) 版本不存在 (检查 -Version 参数)
  2) 仓库为私有 (需设置 `$env:GITHUB_TOKEN)
  3) 网络连接问题 / 企业 DPI 阻断
     → 使用代理: .\install.ps1 -Proxy 'http://proxy.company.com:8080'
     → 手动下载 Release 中的 $artifactName,然后使用:
       .\install.ps1 -LocalFile '<下载路径>' -Version $Version"
        }
    }

    # ------------------ SHA256 校验 (可选) ------------------
    # WHY: 本地文件模式下网络不可达,跳过 checksum 下载(由用户自行确保本地 binary 来源可信)
    if (-not $SkipVerify -and -not $localFileMode) {
        $checksumUrl = "$($script:GitHubReleases)/download/$Version/checksums.txt"
        Write-Info '尝试下载 checksums.txt 进行 SHA256 校验...'
        $checksumFile = Join-Path $tempDir 'checksums.txt'
        $checksumAvailable = $false
        try {
            $headers = @{ 'User-Agent' = 'chimela-install-script' }
            if ($env:GITHUB_TOKEN) {
                $headers['Authorization'] = "Bearer $($env:GITHUB_TOKEN)"
            }
            $requestParams = $script:CommonWebRequestParams.Clone()
            $requestParams['Uri'] = $checksumUrl
            $requestParams['OutFile'] = $checksumFile
            $requestParams['Headers'] = $headers
            Invoke-WebRequest @requestParams
            if ((Get-Item $checksumFile).Length -gt 0) {
                $checksumAvailable = $true
            }
        } catch {
            Write-WarnMsg 'Release 未附带 checksums.txt 或网络不可达,跳过 SHA256 校验'
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
    # WHY: 项目内部 cargo binary 名为 'aether',但 CI/Docker 外部品牌名为 'chimela'。
    #      为消除用户困惑,同时提供三个命令入口:
    #        - chimela.exe: 新品牌名
    #        - aether.exe:   cargo 内部二进制名
    #        - chimera.exe:  旧品牌兼容别名
    #      Windows 符号链接需要特殊权限且兼容性差,直接复制更可靠(仅 1.3MB)。
    $installPath = Join-Path $InstallDir "$($script:BinName).exe"
    $aetherPath = Join-Path $InstallDir 'aether.exe'
    $chimeraPath = Join-Path $InstallDir 'chimera.exe'
    Write-Info "安装到: $installPath"

    try {
        Copy-Item -Path $downloadedFile -Destination $installPath -Force -ErrorAction Stop
        Copy-Item -Path $installPath -Destination $aetherPath -Force -ErrorAction Stop
        Copy-Item -Path $installPath -Destination $chimeraPath -Force -ErrorAction Stop
    } catch {
        Die "安装失败 (权限不足?): $($_.Exception.Message)"
    }

    # WHY: 从网络下载的 exe 会携带 Internet 区域标记(MOTW),可能触发 Windows Defender/
    #      SmartScreen/企业 AV 的拦截,导致 --version 验证失败。安装后解除标记。
    Unblock-File -Path $installPath -ErrorAction SilentlyContinue
    Unblock-File -Path $aetherPath -ErrorAction SilentlyContinue
    Unblock-File -Path $chimeraPath -ErrorAction SilentlyContinue

    Write-Ok 'binary 已安装 (chimela.exe + aether.exe + chimera.exe 兼容别名)'

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
            # WHY: Windows 用户级 PATH 过长会导致注册表写入被截断或后续进程解析失败。
            #      8191 是现代 Windows API 的安全上限,超过时拒绝自动写入并提示手动添加。
            if ($newPath.Length -gt 8191) {
                Write-WarnMsg "用户 PATH 过长 ($($newPath.Length) 字符),超过 8191 安全上限"
                Write-WarnMsg "请手动将 $InstallDir 添加到 PATH"
            } else {
                [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
                $pathUpdated = $true
                Write-Info 'PATH 已更新 (用户级)'
                # 当前会话也立即生效
                $env:Path = "$InstallDir;$env:Path"
            }
        } catch {
            Write-WarnMsg "无法更新 PATH: $($_.Exception.Message)"
            Write-WarnMsg "请手动将 $InstallDir 添加到 PATH"
        }
    }

    if ($pathUpdated) {
        Write-WarnMsg '请重启 PowerShell 终端以使 PATH 全局生效'
    }

    # ------------------ 验证安装 ------------------
    # WHY: 同时验证 chimela.exe / aether.exe / chimera.exe,确保三个入口都可用。
    #      验证失败默认阻塞安装(exit 1),因为继续返回 0 会让 CI/用户误以为成功。
    #      企业安全策略可能首次运行拦截 exe,此时可用 -SkipVersionCheck 显式绕过。
    Write-Info '验证安装...'
    $versionRegex = '^(aether|chimera|chimela) \d+\.\d+\.\d+'
    $verifiedEntries = @()
    $versionFailed = 0

    foreach ($exePath in @($installPath, $aetherPath, $chimeraPath)) {
        try {
            $versionOutput = (& $exePath --version 2>&1 | Out-String).Trim()
            $matched = $false
            if ($LASTEXITCODE -eq 0 -and $versionOutput) {
                foreach ($line in ($versionOutput -split "`n")) {
                    if ($line.TrimEnd("`r") -cmatch $versionRegex) {
                        $matched = $true
                        break
                    }
                }
            }
            if ($matched) {
                $verifiedEntries += (Split-Path -Leaf $exePath)
                Write-Ok "$exePath 验证通过"
                Write-Host "  $versionOutput" -ForegroundColor DarkGray
            } else {
                $versionFailed++
                Write-WarnMsg "$exePath --version 验证失败"
                Write-WarnMsg "期望格式: aether|chimera|chimela X.Y.Z[-omega]"
                Write-WarnMsg "实际输出: $versionOutput"
                Write-WarnMsg "退出码: $LASTEXITCODE"
                Write-WarnMsg "可能原因: 缺少 VC++ 运行时 / Windows Defender 拦截 / 文件损坏"
                Write-WarnMsg "请手动执行: $exePath --version"
            }
        } catch {
            $versionFailed++
            Write-WarnMsg "$exePath --version 执行失败: $($_.Exception.Message)"
            Write-WarnMsg '可能缺少运行时依赖 (Visual C++ Redistributable)'
        }
    }

    if ($versionFailed -gt 0 -and -not $SkipVersionCheck) {
        Die "安装验证失败 ($versionFailed/3 个入口不可用)。请检查上述警告,或使用 -SkipVersionCheck 跳过验证。"
    }

    # ------------------ 总结输出 ------------------
    Write-Host ''
    Write-Info '================ 安装总结 ================'
    Write-Info "  版本:   $Version"
    Write-Info "  主入口: $installPath"
    Write-Info "  别名:   $aetherPath, $chimeraPath"
    Write-Info "  平台:   windows/$archNorm"
    if ($pathUpdated) {
        Write-Info '  PATH:   已更新 (用户级)'
    }
    if ($verifiedEntries.Count -gt 0) {
        Write-Info "  验证:   $($verifiedEntries -join ', ')"
    }
    Write-Info '=========================================='
    Write-Host ''
    Write-Ok "执行 'chimela --help'、'aether --help' 或 'chimera --help' 开始使用"

} finally {
    # ------------------ 清理临时目录 ------------------
    if (Test-Path $tempDir) {
        Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
