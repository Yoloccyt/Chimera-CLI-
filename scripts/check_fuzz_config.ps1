<#
.SYNOPSIS
  Chimera CLI fuzz 配置静态验证脚本(Windows / PowerShell)

.DESCRIPTION
  静态验证 fuzz/Cargo.toml 配置完整性,无需 nightly 工具链或实际运行 fuzz。
  检查项:
  1. fuzz/Cargo.toml 存在且 [package.metadata] cargo-fuzz = true
  2. fuzz/fuzz_targets/ 目录下的 .rs 文件数量与 [[bin]] 声明一致
  3. 每个 [[bin]] 的 path 指向实际存在的文件
  4. fuzz/src/lib.rs stub 宏存在(Windows-GNU 兼容方案)
  5. fuzz/Cargo.toml 有 [target.'cfg(not(windows))'.dependencies] 条目

.NOTES
  退出码:0=全部通过, 1=有检查项失败
  使用方式:pwsh scripts/check_fuzz_config.ps1
#>

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$script:FailCount = 0

function Write-Check {
    param([string]$Name, [bool]$Pass, [string]$Detail = '')
    $status = if ($Pass) { 'PASS' } else { 'FAIL' }
    $color = if ($Pass) { 'Green' } else { 'Red' }
    Write-Host "  [$status] $Name" -ForegroundColor $color
    if ($Detail) { Write-Host "         $Detail" -ForegroundColor Gray }
    if (-not $Pass) { $script:FailCount++ }
}

Write-Host "`n=== Chimera CLI Fuzz Config Static Check ===" -ForegroundColor Cyan

$fuzzDir = Join-Path $PSScriptRoot '..' | Join-Path -ChildPath 'fuzz'
$fuzzToml = Join-Path $fuzzDir 'Cargo.toml'

# --- 检查 1: fuzz/Cargo.toml 存在 ---
$tomlExists = Test-Path $fuzzToml
Write-Check 'fuzz/Cargo.toml 存在' $tomlExists
if (-not $tomlExists) {
    Write-Host "`n  fuzz/Cargo.toml 不存在,无法继续检查" -ForegroundColor Red
    exit 1
}

$tomlContent = Get-Content $fuzzToml -Raw

# --- 检查 2: cargo-fuzz metadata ---
$hasMetadata = $tomlContent -match 'cargo-fuzz\s*=\s*true'
Write-Check '[package.metadata] cargo-fuzz = true' $hasMetadata

# --- 检查 3: [lib] stub 宏声明 ---
$hasLib = $tomlContent -match '\[lib\]' -and $tomlContent -match 'name\s*=\s*"chimera_fuzz"'
Write-Check '[lib] chimera_fuzz stub 宏声明' $hasLib

# --- 检查 4: target-specific 依赖 ---
$hasTargetDep = $tomlContent -match "target\.'cfg\(not\(windows\)\)'\.dependencies"
Write-Check "target.'cfg(not(windows))'.dependencies 条目" $hasTargetDep

# --- 检查 5: fuzz_targets/ 文件数 vs [[bin]] 声明数 ---
$targetsDir = Join-Path $fuzzDir 'fuzz_targets'
$rsFiles = @(Get-ChildItem -Path $targetsDir -Filter '*.rs' -ErrorAction SilentlyContinue)
$binCount = ([regex]::Matches($tomlContent, '\[\[bin\]\]')).Count

Write-Check "fuzz_targets/ .rs 文件数 ($($rsFiles.Count)) = [[bin]] 声明数 ($binCount)" ($rsFiles.Count -eq $binCount)

# --- 检查 6: 每个 [[bin]] path 指向实际存在的文件 ---
foreach ($file in $rsFiles) {
    $binName = $file.BaseName
    $pathPattern = "name\s*=\s*`"$binName`""
    $hasBin = $tomlContent -match $pathPattern
    Write-Check "  [[bin]] $binName 声明存在" $hasBin
}

# --- 检查 7: fuzz/src/lib.rs 存在 ---
$libRs = Join-Path $fuzzDir 'src' | Join-Path -ChildPath 'lib.rs'
$libExists = Test-Path $libRs
Write-Check 'fuzz/src/lib.rs stub 宏文件存在' $libExists

# --- 检查 8: 每个 fuzz_target 文件有条件编译 import ---
$allHaveCondImport = $true
foreach ($file in $rsFiles) {
    $content = Get-Content $file.FullName -Raw
    $hasCond = $content -match '#\[cfg\(not\(windows\)\)\]' -and $content -match '#\[cfg\(windows\)\]'
    if (-not $hasCond) {
        $allHaveCondImport = $false
        Write-Check "  $($file.Name) 条件编译 import" $false
    }
}
if ($allHaveCondImport) {
    Write-Check '所有 fuzz target 文件有条件编译 import' $true
}

# --- 汇总 ---
Write-Host "`n=== 检查结果 ===" -ForegroundColor Cyan
if ($script:FailCount -eq 0) {
    Write-Host "  全部通过 (0 failures)" -ForegroundColor Green
    exit 0
} else {
    Write-Host "  $script:FailCount 项检查失败" -ForegroundColor Red
    exit 1
}
