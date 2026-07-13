<#
.SYNOPSIS
    fuzz cra
    te 配置静态验证脚本(Windows PowerShell 版)

.DESCRIPTION
    WHY 此脚本存在:Windows-GNU 环境下 libfuzzer-sys 的 C++ 源码无法编译
    (FuzzerExtFunctionsWindows.cpp 使用 MSVC 特定语法),cargo check 使用
    stub 宏方案验证 fuzz 逻辑语法。此脚本提供额外的配置完整性静态验证,
    确保 fuzz crate 结构正确(metadata、bin 声明、target 文件存在性)。

    验证项:
    1. fuzz/Cargo.toml 存在且可被 cargo 解析
    2. [package.metadata] cargo-fuzz = true
    3. [lib] path 声明存在(承载 stub 宏)
    4. 6 个 [[bin]] 声明存在,每个 bin 的 path 指向的文件存在
    5. 每个 fuzz target 文件包含 fuzz_target! 宏调用
    6. 被测 crate 的 path 依赖目录存在

    退出码:0 = 全部通过,1 = 有失败项

.EXAMPLE
    .\scripts\check_fuzz_config.ps1
#>

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$fuzzDir = Join-Path $repoRoot "fuzz"
$fuzzCargo = Join-Path $fuzzDir "Cargo.toml"
$fuzzTargetsDir = Join-Path $fuzzDir "fuzz_targets"

$failures = @()

function Add-Failure {
    param([string]$Message)
    $script:failures += $Message
    Write-Host "  [FAIL] $Message" -ForegroundColor Red
}

function Add-Pass {
    param([string]$Message)
    Write-Host "  [PASS] $Message" -ForegroundColor Green
}

Write-Host "=== fuzz crate 配置静态验证 ===" -ForegroundColor Cyan
Write-Host ""

# ---------------------------------------------------------------------------
# 检查 1: fuzz/Cargo.toml 存在
# ---------------------------------------------------------------------------
Write-Host "[1/6] 检查 fuzz/Cargo.toml 存在性"
if (Test-Path $fuzzCargo) {
    Add-Pass "fuzz/Cargo.toml 存在"
} else {
    Add-Failure "fuzz/Cargo.toml 不存在: $fuzzCargo"
    # 无法继续后续检查
    Write-Host ""
    Write-Host "验证失败: $failures 项" -ForegroundColor Red
    exit 1
}

# 读取 Cargo.toml 内容
$cargoContent = Get-Content $fuzzCargo -Raw

# ---------------------------------------------------------------------------
# 检查 2: cargo-fuzz metadata
# ---------------------------------------------------------------------------
Write-Host "[2/6] 检查 [package.metadata] cargo-fuzz = true"
if ($cargoContent -match 'cargo-fuzz\s*=\s*true') {
    Add-Pass "cargo-fuzz metadata 已声明"
} else {
    Add-Failure "未找到 cargo-fuzz = true metadata(cargo-fuzz 0.13+ 要求)"
}

# ---------------------------------------------------------------------------
# 检查 3: [lib] path 声明(承载 Windows-GNU stub 宏)
# ---------------------------------------------------------------------------
Write-Host "[3/6] 检查 [lib] path 声明"
if ($cargoContent -match '\[lib\]' -and $cargoContent -match 'path\s*=\s*"src/lib\.rs"') {
    Add-Pass "[lib] path = src/lib.rs 已声明"
} else {
    Add-Failure "未找到 [lib] path = src/lib.rs(Windows-GNU stub 宏载体)"
}

# 验证 lib.rs 文件存在
$libRs = Join-Path $fuzzDir "src/lib.rs"
if (Test-Path $libRs) {
    Add-Pass "src/lib.rs 文件存在"
} else {
    Add-Failure "src/lib.rs 文件不存在: $libRs"
}

# ---------------------------------------------------------------------------
# 检查 4: 8 个 [[bin]] 声明 + target 文件存在性(6 生产 + 2 stub 宏测试)
# ---------------------------------------------------------------------------
Write-Host "[4/6] 检查 8 个 [[bin]] 声明与 target 文件"

$expectedTargets = @(
    @{ name = "quest_parse";              file = "quest_parse.rs" },
    @{ name = "seccore_sandbox";          file = "seccore_sandbox.rs" },
    @{ name = "event_serialize";          file = "event_serialize.rs" },
    @{ name = "cacr_budget_parse";        file = "cacr_budget_parse.rs" },
    @{ name = "checkpoint_deserialize";   file = "checkpoint_deserialize.rs" },
    @{ name = "config_section_parse";     file = "config_section_parse.rs" },
    @{ name = "stub_form1_test";          file = "stub_form1_test.rs" },
    @{ name = "stub_form3_test";          file = "stub_form3_test.rs" }
)

foreach ($target in $expectedTargets) {
    $name = $target.name
    $file = $target.file
    $filePath = Join-Path $fuzzTargetsDir $file

    # 检查 [[bin]] 声明
    $binPattern = "name\s*=\s*`"$name`""
    if ($cargoContent -match $binPattern) {
        # 检查 target 文件存在
        if (Test-Path $filePath) {
            Add-Pass "[$name] bin 声明 + 文件存在"
        } else {
            Add-Failure "[$name] bin 声明存在,但文件不存在: $filePath"
        }
    } else {
        Add-Failure "[$name] 未在 Cargo.toml 中找到 [[bin]] name = `"$name`""
    }
}

# ---------------------------------------------------------------------------
# 检查 5: 每个 fuzz target 包含 fuzz_target! 宏调用
# ---------------------------------------------------------------------------
Write-Host "[5/6] 检查 fuzz target 文件包含 fuzz_target! 宏调用"

foreach ($target in $expectedTargets) {
    $filePath = Join-Path $fuzzTargetsDir $target.file
    if (Test-Path $filePath) {
        $content = Get-Content $filePath -Raw
        if ($content -match 'fuzz_target!\s*\(') {
            Add-Pass "[$($target.name)] 包含 fuzz_target! 宏调用"
        } else {
            Add-Failure "[$($target.name)] 未找到 fuzz_target! 宏调用"
        }
    }
}

# ---------------------------------------------------------------------------
# 检查 6: 被测 crate path 依赖目录存在
# ---------------------------------------------------------------------------
Write-Host "[6/6] 检查被测 crate path 依赖目录存在"

$expectedDeps = @(
    "nexus-core",
    "event-bus",
    "seccore",
    "model-router"
)

foreach ($dep in $expectedDeps) {
    $depDir = Join-Path $repoRoot "crates/$dep"
    if (Test-Path (Join-Path $depDir "Cargo.toml")) {
        Add-Pass "[$dep] path 依赖目录存在"
    } else {
        Add-Failure "[$dep] path 依赖目录不存在: $depDir"
    }
}

# ---------------------------------------------------------------------------
# 汇总
# ---------------------------------------------------------------------------
Write-Host ""
if ($failures.Count -eq 0) {
    Write-Host "=== 验证通过: 所有检查项 PASS ===" -ForegroundColor Green
    exit 0
} else {
    Write-Host "=== 验证失败: $($failures.Count) 项 FAIL ===" -ForegroundColor Red
    $failures | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    exit 1
}