# 架构文档一致性巡检(Windows) —— 以 Cargo.toml 的 workspace.members 为唯一权威(canonical truth)。
# WHY(对应 Better Loop 复盘 F4): README/CODE_WIKI/.cargo 曾长期把 crate 数写成 34,
#   而实际已是 35(v2.0.0-omega 新增 chimera-mas)。发布清单只校验版本号/CHANGELOG,
#   缺少"文档 crate 数 vs 代码"的巡检,导致漂移反复复发。本脚本提供可返回 clean/gap 的巡检。
# 与 scripts/check_doc_consistency.sh 逻辑一致(CI 用 .sh,本地 Windows 用本脚本)。
# 退出码: 0=clean, 1=gap
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$status = 0

# (1) canonical truth: 提取 [workspace].members 数组内以 "crates/ 开头的成员并计数
$cargo = Get-Content 'Cargo.toml' -Raw
$membersBlock = [regex]::Match($cargo, 'members\s*=\s*\[(.*?)\]', 'Singleline').Groups[1].Value
$nMembers = ([regex]::Matches($membersBlock, '"crates/[^"]+"')).Count
$nDirs = @(Get-ChildItem 'crates' -Directory | Where-Object { Test-Path (Join-Path $_.FullName 'Cargo.toml') }).Count
Write-Host "[doc-consistency] canonical crate 数 (Cargo.toml members) = $nMembers"

if ($nMembers -ne $nDirs) {
    Write-Host "[GAP] Cargo.toml members($nMembers) 与磁盘 crates/*/Cargo.toml($nDirs) 不一致"
    $status = 1
}

# (2) 索引文档必须包含当前 crate 计数(任一定值形式命中即可)
#     覆盖 docs/architecture 索引(README + 权威源 CODE_WIKI)与 CLAUDE.md 三份文档;
#     缺失文件走 "[GAP] 缺少文档" 分支,故本项同时承担 CODE_WIKI.md 的存在性校验。
# WHY 用"正向包含"而非"检测旧值":CODE_WIKI.md 的变更历史表含合法历史 crate 数
#     (如 v1.0.0-omega 的 "34 crate"),只需断言当前值($nMembers)存在即可,天然规避历史值误报。
# WHY 用 ${nMembers} 花括号定界:PowerShell 在数组字面量中对 "$nMembers" + "个crate" 会解析为
#     两个独立元素("35" 与 "个crate"),从而混入裸 "35" token 命中任意含 35 的文本而漏报;
#     "${nMembers}个crate" 作为单一字符串字面量可正确得到 "35个crate"。
$tokens = @("$nMembers crate", "${nMembers}个crate", "$nMembers 个 crate", "$nMembers Crate", "$nMembers/$nMembers crate")
foreach ($f in @('docs/architecture/README.md', 'docs/architecture/CODE_WIKI.md', '.claude/CLAUDE.md')) {
    if (-not (Test-Path $f)) { Write-Host "[GAP] 缺少文档: $f"; $status = 1; continue }
    $content = Get-Content $f -Raw
    $hit = $false
    foreach ($t in $tokens) { if ($content.Contains($t)) { $hit = $true; break } }
    if (-not $hit) {
        Write-Host "[GAP] $f 未包含当前 crate 计数($nMembers) —— 可能仍是旧值,请对齐 Cargo.toml"
        $status = 1
    }
}

if ($status -eq 0) { Write-Host "[OK] 文档 crate 计数与 Cargo.toml($nMembers) 一致" }
exit $status
