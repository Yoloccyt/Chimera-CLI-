#!/usr/bin/env pwsh
<#
.SYNOPSIS
    用 git-cliff 生成 CHANGELOG 初稿（辅助手写，不替代）

.DESCRIPTION
    调用 git-cliff 从 conventional commits 生成 CHANGELOG 初稿。
    生成的内容为原始素材，需要人工润色后合并到 CHANGELOG.md。
    项目 CHANGELOG.md 手写质量高（含 ADR/测试计数/架构上下文），
    此脚本仅用于减少新版本发布时的遗漏。

.PARAMETER Latest
    只生成最新 tag 的 CHANGELOG（默认行为）

.PARAMETER Unreleased
    生成最新 tag 之后到 HEAD 的未发布变更

.PARAMETER Output
    输出文件路径（默认 CHANGELOG_DRAFT.md）

.EXAMPLE
    .\generate_changelog.ps1
    生成最新 tag 的 CHANGELOG 初稿到 CHANGELOG_DRAFT.md

.EXAMPLE
    .\generate_changelog.ps1 -Unreleased
    生成未发布变更的 CHANGELOG 初稿

.NOTES
    依赖: git-cliff (cargo install git-cliff)
    配置: cliff.toml (项目根目录)
#>
[CmdletBinding()]
param(
    [switch]$Latest,
    [switch]$Unreleased,
    [string]$Output = "CHANGELOG_DRAFT.md"
)

$ErrorActionPreference = "Stop"

$repoRoot = $PSScriptRoot | Split-Path -Parent
$configPath = Join-Path $repoRoot "cliff.toml"
$outputPath = if ([System.IO.Path]::IsPathRooted($Output)) { $Output } else { Join-Path $repoRoot $Output }

if (-not (Test-Path $configPath)) {
    Write-Error "配置文件不存在: $configPath"
    exit 1
}

# 检查 git-cliff 可用
$cliffCmd = Get-Command git-cliff -ErrorAction SilentlyContinue
if (-not $cliffCmd) {
    Write-Error "git-cliff 未安装。请运行: cargo install git-cliff"
    exit 1
}

$env:CARGO_HOME = "D:\Chimera CLI\.toolchain\cargo"
$env:PATH = "$env:CARGO_HOME\bin;$env:PATH"

Write-Host "=== git-cliff CHANGELOG 生成 ===" -ForegroundColor Cyan

$args = @("--config", $configPath, "--output", $outputPath)

if ($Unreleased) {
    Write-Host "模式: 未发布变更（最新 tag → HEAD）"
    $args += "--unreleased"
} else {
    Write-Host "模式: 最新 tag"
    $args += "--latest"
}

& git-cliff @args

if ($LASTEXITCODE -eq 0 -and (Test-Path $outputPath)) {
    $lines = (Get-Content $outputPath).Count
    Write-Host "生成成功: $outputPath ($lines 行)" -ForegroundColor Green
    Write-Host "下一步: 人工润色后合并到 CHANGELOG.md" -ForegroundColor Yellow
} else {
    Write-Error "git-cliff 执行失败"
    exit 1
}
