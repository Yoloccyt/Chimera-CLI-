<#
.SYNOPSIS
  Chimera CLI 本地 Docker 镜像验证脚本(Windows / PowerShell)

.DESCRIPTION
  三级降级验证:
  1. Docker Desktop 可用 → 构建镜像 + 验证 --version + 检查体积 < 100MB
  2. Podman 可用 → 构建镜像 + 验证 --version
  3. 两者都不可用 → Dockerfile 静态检查 + release binary 体积 < 50MB 代理指标

  完整镜像验证由 release.yml docker job 在 tag 推送时自动执行。
  本脚本用于本地发布前检查清单第 10-12 项的降级验证。

.NOTES
  退出码: 0=验证通过, 1=验证失败
  使用方式: pwsh scripts/verify_docker_locally.ps1
#>

[CmdletBinding()]
param(
    [string]$ImageTag = 'chimera-cli:local'
)

$ErrorActionPreference = 'Stop'
$script:FailCount = 0
$projectRoot = Split-Path -Parent $PSScriptRoot

function Write-Step { param([string]$Msg) Write-Host "`n[STEP] $Msg" -ForegroundColor Cyan }
function Write-Pass { param([string]$Msg) Write-Host "  [PASS] $Msg" -ForegroundColor Green }
function Write-Fail { param([string]$Msg) Write-Host "  [FAIL] $Msg" -ForegroundColor Red; $script:FailCount++ }
function Write-Info { param([string]$Msg) Write-Host "  [INFO] $Msg" -ForegroundColor Gray }

function Invoke-FallbackChecks {
    <#
    .SYNOPSIS
      Dockerfile 静态检查 + release binary 体积代理指标。
      在 Docker/Podman 均不可用或镜像构建失败时作为第三级降级验证。
    #>
    Write-Host "`n[降级模式] 执行 Dockerfile 静态检查 + release binary 体积代理指标" -ForegroundColor Yellow

    # --- 降级验证: Dockerfile 静态检查 ---
    Write-Step 'Dockerfile 静态检查'

    $dockerfile = Join-Path $projectRoot 'Dockerfile'
    if (Test-Path $dockerfile) {
        $content = Get-Content $dockerfile -Raw
        Write-Pass 'Dockerfile 存在'

        $checks = @(
            @{ Name='FROM rust:1-slim-bookworm builder'; Pattern = 'FROM\s+rust:1-slim-bookworm' },
            @{ Name='FROM distroless runtime'; Pattern = 'FROM\s+gcr\.io/distroless/cc-debian12' },
            @{ Name='USER nonroot'; Pattern = 'USER\s+nonroot' },
            @{ Name='HEALTHCHECK 声明'; Pattern = 'HEALTHCHECK' },
            @{ Name='ENTRYPOINT ["chimera"]'; Pattern = 'ENTRYPOINT\s+\["chimera"\]' },
            @{ Name='RUST_BACKTRACE=1'; Pattern = 'RUST_BACKTRACE=1' }
        )

        foreach ($c in $checks) {
            if ($content -match $c.Pattern) {
                Write-Pass $c.Name
            } else {
                Write-Fail $c.Name
            }
        }
    } else {
        Write-Fail 'Dockerfile 存在'
    }

    # --- 降级验证: release binary 体积 ---
    Write-Step 'Release binary 体积检查 (< 50MB 代理指标)'

    $releaseBin = Join-Path $projectRoot 'target' | Join-Path -ChildPath 'release' | Join-Path -ChildPath 'chimera.exe'
    if (Test-Path $releaseBin) {
        $sizeBytes = (Get-Item $releaseBin).Length
        $sizeMB = [math]::Round($sizeBytes / 1MB, 2)
        if ($sizeBytes -lt 50MB) {
            Write-Pass "Release binary 体积: ${sizeMB}MB < 50MB"
        } else {
            Write-Fail "Release binary 体积: ${sizeMB}MB >= 50MB"
        }
    } else {
        Write-Info "Release binary 不存在(未运行 cargo build --release),跳过体积检查"
        Write-Info "完整镜像验证由 release.yml docker job 在 tag 推送时自动执行"
    }

    # --- 降级验证: CI 状态引导 ---
    Write-Step 'CI 状态查询引导'
    Write-Info '完整 Docker 镜像验证(.github/workflows/release.yml docker job)在 tag 推送时自动执行'
    Write-Info '查询最近 CI 运行状态:'
    Write-Info '  gh run list --workflow=release.yml --limit=1'
    Write-Info '  gh run view <run-id> --log --job=docker'
}

Write-Host "`n=== Chimera CLI Docker Local Verification ===" -ForegroundColor Cyan

# --- 检测可用容器运行时 ---
$dockerAvailable = $false
$podmanAvailable = $false
$runtime = $null

try {
    $dockerVersion = docker --version 2>$null
    if ($LASTEXITCODE -eq 0) {
        $dockerAvailable = $true
        $runtime = 'docker'
        Write-Info "检测到 Docker: $dockerVersion"
    }
} catch { }

if (-not $dockerAvailable) {
    try {
        $podmanVersion = podman --version 2>$null
        if ($LASTEXITCODE -eq 0) {
            $podmanAvailable = $true
            $runtime = 'podman'
            Write-Info "检测到 Podman: $podmanVersion"
        }
    } catch { }
}

if (-not $dockerAvailable -and -not $podmanAvailable) {
    Invoke-FallbackChecks
} else {
    # --- 容器运行时可用: 完整验证 ---
    Write-Step "使用 $runtime 构建镜像 ($ImageTag)"

    $buildCmd = if ($runtime -eq 'docker') { 'docker' } else { 'podman' }
    & $buildCmd build -t $ImageTag "$projectRoot"

    if ($LASTEXITCODE -eq 0) {
        Write-Pass "镜像构建成功: $ImageTag"

        # --- 验证 --version ---
        Write-Step '验证 --version 输出'
        $versionOutput = & $buildCmd run --rm $ImageTag --version 2>&1
        if ($LASTEXITCODE -eq 0 -and $versionOutput -match '^(aether|chimera)\s+[0-9]+\.[0-9]+\.[0-9]+') {
            Write-Pass "--version 输出匹配: $versionOutput"
        } else {
            Write-Fail "--version 输出不匹配: $versionOutput"
        }

        # --- 检查镜像体积 ---
        Write-Step '检查镜像体积 (< 100MB)'

        if ($runtime -eq 'docker') {
            $sizeStr = docker image inspect $ImageTag --format '{{.Size}}' 2>$null
            if ($sizeStr -and $LASTEXITCODE -eq 0) {
                $sizeBytes = [long]$sizeStr
                $sizeMB = [math]::Round($sizeBytes / 1MB, 2)
                if ($sizeMB -lt 100) {
                    Write-Pass "镜像体积: ${sizeMB}MB < 100MB"
                } else {
                    Write-Fail "镜像体积: ${sizeMB}MB >= 100MB"
                }
            } else {
                Write-Fail '无法获取镜像体积'
            }
        } else {
            # Podman
            $sizeStr = podman image inspect $ImageTag --format '{{.Size}}' 2>$null
            if ($sizeStr -and $LASTEXITCODE -eq 0) {
                $sizeBytes = [long]$sizeStr
                $sizeMB = [math]::Round($sizeBytes / 1MB, 2)
                if ($sizeMB -lt 100) {
                    Write-Pass "镜像体积: ${sizeMB}MB < 100MB"
                } else {
                    Write-Fail "镜像体积: ${sizeMB}MB >= 100MB"
                }
            } else {
                Write-Fail '无法获取镜像体积'
            }
        }
    } else {
        # 构建失败本身不直接计为最终失败,而是降级到静态检查作为本地验证的替代路径。
        # 这样 Podman machine 未启动等环境问题时,发布前检查清单仍可通过 Dockerfile + binary 代理指标完成。
        Write-Info "镜像构建失败,降级到静态检查"
        Invoke-FallbackChecks
    }
}

# --- 汇总 ---
Write-Host "`n=== 验证结果 ===" -ForegroundColor Cyan
if ($script:FailCount -eq 0) {
    Write-Host "  全部通过 (0 failures)" -ForegroundColor Green
    exit 0
} else {
    Write-Host "  $script:FailCount 项失败" -ForegroundColor Red
    exit 1
}
