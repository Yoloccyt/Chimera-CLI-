﻿﻿﻿<#
.SYNOPSIS
    Chimera CLI 本地 Docker 验证替代脚本 (Windows PowerShell)

.DESCRIPTION
    发布前检查清单 (CLAUDE.md S5 第 10-12 项 / nuxus规则.md S7.2 第 7 项) 要求 Docker
    镜像验证: build + run --version + image size < 100MB。本地 Windows 环境若未安装
    Docker Desktop (企业防火墙/许可证/WSL2 不可用等), 本脚本提供三级降级策略:

      1. Docker 可用  -> 完整镜像构建验证 (与 CI release.yml docker job 等价)
      2. Podman 可用  -> Podman 构建验证 (Podman 兼容 Docker CLI, 无许可证限制)
      3. 均不可用     -> 降级验证:
           a) Dockerfile 静态验证 (关键指令/基础镜像/安全配置存在性检查)
           b) Release binary 验证 (--version 格式 + 体积 < 50MB, 镜像体积代理指标)
           c) CI Docker 验证状态查询命令引导

    降级验证的代理逻辑: Dockerfile 静态检查确保镜像构建配方正确, binary 体积 < 50MB
    是镜像体积 < 100MB 的必要条件 (distroless 基础镜像约 20MB + binary), 完整的镜像
    验证由 CI release.yml docker job 在 tag 推送时自动执行。

.PARAMETER SkipBuild
    跳过 release binary 构建 (假定 target/release/aether.exe 已存在)。
    适用于已执行过 cargo build --workspace --release 的增量验证场景。

.EXAMPLE
    .\scripts\verify_docker_locally.ps1
.EXAMPLE
    .\scripts\verify_docker_locally.ps1 -SkipBuild
#>
param(
    [switch]$SkipBuild
)

# ============================================================
# 全局配置
# ============================================================

# 仓库根目录 (脚本位于 scripts/ 下, 根目录是上一级)
$RepoRoot = Split-Path -Parent $PSScriptRoot
$DockerfilePath = Join-Path $RepoRoot "Dockerfile"
$BinaryPath = Join-Path $RepoRoot "target\release\aether.exe"
$ImageName = "chimera-cli:local-verify"

# 体积红线 (与 release.yml / nuxus规则.md S7.2 一致)
$BinarySizeLimit = 52428800    # 50MB = 50 * 1024 * 1024
$ImageSizeLimit = 104857600    # 100MB = 100 * 1024 * 1024

# 验证结果计数
$script:PassCount = 0
$script:FailCount = 0
$script:Warnings = @()

# ============================================================
# 辅助函数
# ============================================================

<#
  输出单项检查结果并累加计数。
  参数:
    $Name    - 检查项名称
    $Passed  - $true 通过 / $false 失败
    $Detail  - 额外详情 (可选)
#>
function Write-Check {
    param([string]$Name, [bool]$Passed, [string]$Detail = "")
    if ($Passed) {
        Write-Host "[OK]   $Name" -ForegroundColor Green
        $script:PassCount++
    } else {
        Write-Host "[FAIL] $Name" -ForegroundColor Red
        $script:FailCount++
    }
    if ($Detail) {
        Write-Host "       $Detail" -ForegroundColor DarkGray
    }
}

<# 检测命令是否在 PATH 中可用 #>
function Test-CommandAvailable {
    param([string]$Name)
    return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

# ============================================================
# 路径 1/2: Docker / Podman 完整镜像构建验证
# ============================================================

<#
  使用指定容器引擎 (docker 或 podman) 执行完整镜像验证。
  验证项与 release.yml docker job 完全对齐:
    - build 镜像
    - run --version (grep ^(aether|chimera) X.Y.Z)
    - image inspect --format {{.Size}} (< 100MB)
  参数:
    $Engine - "docker" 或 "podman"
  返回: $true 全部通过, $false 任一失败
#>
function Invoke-EngineVerification {
    param([string]$Engine)

    Write-Host ""
    Write-Host "=== 检测到 $Engine, 执行完整镜像构建验证 ===" -ForegroundColor Cyan

    # --- 构建 ---
    Write-Host "-> $Engine build -t $ImageName (可能需要数分钟)..."
    # 捕获完整输出用于诊断, 同时实时回显最后几行
    $buildOutput = & $Engine build -t $ImageName $RepoRoot 2>&1
    $buildExit = $LASTEXITCODE
    $buildOutput | Select-Object -Last 5 | ForEach-Object { Write-Host "   $_" -ForegroundColor DarkGray }
    if ($buildExit -ne 0) {
        Write-Check "$Engine build 成功" $false "退出码 $buildExit (详见上方输出)"
        return $false
    }
    Write-Check "$Engine build 成功" $true

    # --- --version 验证 ---
    # distroless 无 shell, 直接执行 binary; 输出必须匹配 aether|chimera X.Y.Z
    $versionOutput = (& $Engine run --rm $ImageName --version 2>&1).ToString().Trim()
    # -cmatch = case-sensitive match (与 release.yml grep -qE 等价)
    $versionMatch = $versionOutput -cmatch '^(aether|chimera) \d+\.\d+\.\d+'
    Write-Check "$Engine run --version 格式校验" $versionMatch "输出: $versionOutput"
    if (-not $versionMatch) { return $false }

    # --- 镜像体积验证 ---
    $sizeRaw = (& $Engine image inspect $ImageName --format '{{.Size}}' 2>&1).ToString().Trim()
    if ($LASTEXITCODE -eq 0 -and $sizeRaw -match '^\d+$') {
        $imageSize = [int64]$sizeRaw
        $sizeMB = [math]::Round($imageSize / 1MB, 2)
        $sizeOk = $imageSize -lt $ImageSizeLimit
        Write-Check "镜像体积 < 100MB" $sizeOk "实际: ${sizeMB}MB ($imageSize bytes)"
    } else {
        Write-Check "镜像体积 < 100MB" $false "无法获取镜像体积: $sizeRaw"
        return $false
    }

    # --- 清理临时镜像 ---
    & $Engine rmi $ImageName --force 2>$null | Out-Null

    return ($script:FailCount -eq 0)
}

# ============================================================
# 路径 3a: Dockerfile 静态验证
# ============================================================

<#
  对 Dockerfile 执行静态结构检查, 确保镜像构建配方关键指令完整。
  不执行实际构建, 仅验证文本层面的一致性。
#>
function Invoke-DockerfileStaticCheck {
    Write-Host ""
    Write-Host "=== 降级验证 1/3: Dockerfile 静态检查 ===" -ForegroundColor Cyan

    if (-not (Test-Path $DockerfilePath)) {
        Write-Check "Dockerfile 存在" $false "路径: $DockerfilePath"
        return
    }
    Write-Check "Dockerfile 存在" $true

    $content = Get-Content $DockerfilePath -Raw

    # 关键指令检查清单: 每项对应 Dockerfile 中的一条安全/功能约束
    # 缺失任一项意味着 Dockerfile 被意外篡改或降级, 需要人工排查
    $requiredChecks = @(
        @{ Name = "Builder 阶段 (rust:1.85-slim)";         Pattern = 'FROM rust:1\.85-slim AS builder' },
        @{ Name = "Runtime 阶段 (distroless/cc-debian12)"; Pattern = 'FROM gcr\.io/distroless/cc-debian12' },
        @{ Name = "多阶段 COPY --from=builder";            Pattern = 'COPY --from=builder' },
        @{ Name = "文件归属 --chown=nonroot:nonroot";      Pattern = '--chown=nonroot:nonroot' },
        @{ Name = "USER nonroot:nonroot (最小权限)";       Pattern = 'USER nonroot:nonroot' },
        @{ Name = "ENTRYPOINT exec form (无 shell)";       Pattern = 'ENTRYPOINT \["chimera"\]' },
        @{ Name = "HEALTHCHECK 定义";                      Pattern = 'HEALTHCHECK' },
        @{ Name = "ARG VERSION (CI 版本注入)";             Pattern = 'ARG VERSION' },
        @{ Name = "ENV RUST_BACKTRACE=1 (panic 栈回溯)";   Pattern = 'ENV RUST_BACKTRACE=1' },
        @{ Name = "OCI LABEL (镜像元数据)";                Pattern = 'org\.opencontainers\.image\.title' }
    )

    foreach ($check in $requiredChecks) {
        $found = $content -match $check.Pattern
        Write-Check $check.Name $found
    }
}

# ============================================================
# 路径 3b: Release binary 验证
# ============================================================

<#
  验证 release binary 可执行性 + 体积。
  binary 体积 < 50MB 是镜像体积 < 100MB 的必要条件 (distroless 基础约 20MB + binary),
  作为镜像体积的代理指标。
#>
function Invoke-BinaryVerification {
    Write-Host ""
    Write-Host "=== 降级验证 2/3: Release binary 验证 ===" -ForegroundColor Cyan

    # 构建检查 (-SkipBuild 时跳过)
    if (-not $SkipBuild) {
        # WHY cargo 可用性预检:Shell/CI 环境可能未设置工具链 PATH,
        # 直接调用 cargo 会触发 "cargo not recognized" 错误(exit code 1),
        # 开发者难以区分是环境问题还是构建失败。提前检测并给出明确指引。
        $cargoCmd = Get-Command cargo -ErrorAction SilentlyContinue
        if (-not $cargoCmd) {
            Write-Check "cargo build --release" $false "cargo 不在 PATH 中"
            Write-Host "       提示: 工具链环境变量未配置。请运行以下命令配置:" -ForegroundColor Yellow
            Write-Host "         powershell -ExecutionPolicy Bypass -File install.ps1 -SetupEnv" -ForegroundColor White
            Write-Host "       或手动设置 (当前会话):" -ForegroundColor Yellow
            Write-Host '         $env:CARGO_HOME = ''D:\Chimera CLI\.toolchain\cargo''' -ForegroundColor White
            Write-Host '         $env:RUSTUP_HOME = ''D:\Chimera CLI\.toolchain\rustup''' -ForegroundColor White
            Write-Host '         $env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"' -ForegroundColor White
            Write-Host "       配置后重开终端或重新运行本脚本。" -ForegroundColor Yellow
            return
        }

        Write-Host "-> cargo build --workspace --release (可能需要数分钟)..."
        & cargo build --workspace --release 2>&1 |
            Select-Object -Last 3 |
            ForEach-Object { Write-Host "   $_" -ForegroundColor DarkGray }
        if ($LASTEXITCODE -ne 0) {
            Write-Check "cargo build --release" $false "退出码 $LASTEXITCODE"
            return
        }
    }

    # binary 存在性
    if (-not (Test-Path $BinaryPath)) {
        Write-Check "Binary 存在" $false "路径: $BinaryPath (请先运行 cargo build --workspace --release)"
        return
    }
    Write-Check "Binary 存在" $true

    # --version 执行 + 格式校验 (与 release.yml Verify binary runs 步骤对齐)
    $versionOutput = (& $BinaryPath --version 2>&1).ToString().Trim()
    $versionMatch = $versionOutput -cmatch '^(aether|chimera) \d+\.\d+\.\d+'
    Write-Check "binary --version 格式校验" $versionMatch "输出: $versionOutput"

    # 体积验证 (与 release.yml Verify binary size < 50MB 步骤对齐)
    $binarySize = (Get-Item $BinaryPath).Length
    $sizeMB = [math]::Round($binarySize / 1MB, 2)
    $sizeOk = $binarySize -lt $BinarySizeLimit
    Write-Check "binary 体积 < 50MB" $sizeOk "实际: ${sizeMB}MB ($binarySize bytes)"
    if (-not $sizeOk) {
        Write-Host "       提示: binary 体积超限通常说明引入了重量级依赖或 strip/LTO 配置失效" -ForegroundColor Yellow
    }
}

# ============================================================
# 路径 3c: CI Docker 验证状态查询引导
# ============================================================

<#
  本地无法构建镜像时, Docker 镜像的完整验证由 CI release.yml docker job 完成。
  此函数输出 gh CLI 查询命令, 引导开发者确认 CI 验证状态。
#>
function Show-CIQueryGuidance {
    Write-Host ""
    Write-Host "=== 降级验证 3/3: CI Docker 验证状态查询 ===" -ForegroundColor Cyan
    Write-Host "本地无法构建镜像时, Docker 镜像的完整验证由 CI release.yml docker job 完成。" -ForegroundColor DarkGray
    Write-Host "推送 tag 后, 通过以下命令查询 CI 状态:" -ForegroundColor DarkGray
    Write-Host ""
    Write-Host "  # 查看最近的 Release 工作流运行" -ForegroundColor White
    Write-Host "  gh run list --workflow=release.yml --limit 5" -ForegroundColor White
    Write-Host ""
    Write-Host "  # 查看特定运行的详情 (含 docker job 状态)" -ForegroundColor White
    Write-Host "  gh run view --workflow=release.yml <run-id>" -ForegroundColor White
    Write-Host ""
    Write-Host "  # 查看 docker job 日志 (镜像构建 + 体积验证 + --version 验证)" -ForegroundColor White
    Write-Host "  gh run view <run-id> --log --job=<job-id>" -ForegroundColor White
    Write-Host ""
    Write-Host "  CI docker job 验证项:" -ForegroundColor DarkGray
    Write-Host "    - docker build + push to GHCR" -ForegroundColor DarkGray
    Write-Host "    - 镜像体积 < 100MB 断言" -ForegroundColor DarkGray
    Write-Host "    - docker run --rm <image> --version 格式校验" -ForegroundColor DarkGray

    $script:Warnings += "Docker 镜像完整验证依赖 CI (release.yml docker job), 请确认 tag 推送后 CI 通过"
}

# ============================================================
# 主流程
# ============================================================

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "Chimera CLI 本地 Docker 验证 (替代脚本)" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

$hasDocker = Test-CommandAvailable "docker"
$hasPodman = Test-CommandAvailable "podman"

if ($hasDocker) {
    # 路径 1: Docker 完整验证
    $ok = Invoke-EngineVerification -Engine "docker"
    if ($ok) {
        Write-Host ""
        Write-Host ">>> Docker 完整验证通过 <<<" -ForegroundColor Green
    }
} elseif ($hasPodman) {
    # 路径 2: Podman 完整验证 (Docker 不可用时的替代引擎)
    Write-Host "未检测到 Docker, 发现 Podman, 将使用 Podman 执行镜像构建验证。" -ForegroundColor Yellow
    $ok = Invoke-EngineVerification -Engine "podman"
    if ($ok) {
        Write-Host ""
        Write-Host ">>> Podman 完整验证通过 <<<" -ForegroundColor Green
    }
} else {
    # 路径 3: 降级验证 (Docker / Podman 均不可用)
    Write-Host "未检测到 Docker / Podman, 执行降级验证 (Dockerfile 静态 + binary 验证 + CI 引导)。" -ForegroundColor Yellow
    Invoke-DockerfileStaticCheck
    Invoke-BinaryVerification
    Show-CIQueryGuidance
}

# ============================================================
# 汇总报告
# ============================================================

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "验证汇总" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

if ($script:FailCount -eq 0) {
    Write-Host "通过: $($script:PassCount)  失败: $($script:FailCount)" -ForegroundColor Green
} else {
    Write-Host "通过: $($script:PassCount)  失败: $($script:FailCount)" -ForegroundColor Red
}

if ($script:Warnings.Count -gt 0) {
    Write-Host ""
    Write-Host "注意:" -ForegroundColor Yellow
    foreach ($w in $script:Warnings) {
        Write-Host "  - $w" -ForegroundColor Yellow
    }
}

# 失败时退出码 1, 便于 CI / 脚本编排集成
if ($script:FailCount -gt 0) {
    exit 1
}
