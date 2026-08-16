# Concord W2 T2.4 — 斜杠命令主流同义词表抽查脚本(PowerShell 版)
#
# 断言:重构方案 §9.8 与主流五家(Codex/Claude Code/opencode/Qoder/Hermes)
# 同名的 24 条命令均存在于 SlashCommandRegistry 且执行分层(tier)正确。
# 数据源:crates/chimera-tui/src/actions/slash_registry.rs(单一事实源)。
# 用法:pwsh -NoProfile -File scripts/check_slash_vocab.ps1 ; EXIT=1 时列出差异。

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$registryFile = Join-Path $repoRoot 'crates/chimera-tui/src/actions/slash_registry.rs'
if (-not (Test-Path $registryFile)) {
    Write-Output "[FAIL] registry file not found: $registryFile"
    exit 1
}

$utf8 = New-Object System.Text.UTF8Encoding($false, $true)
$text = [System.IO.File]::ReadAllText($registryFile, $utf8)

# 解析命令条目:name + tier + aliases 三元组
$entries = @{}
$pattern = 'SlashCommandDesc\s*\{\s*name:\s*"([^"]+)",\s*aliases:\s*&\[([^\]]*)\],\s*tier:\s*SlashTier::(\w+)'
foreach ($m in [regex]::Matches($text, $pattern)) {
    $name = $m.Groups[1].Value
    $tier = $m.Groups[3].Value
    $entries[$name] = $tier
    foreach ($a in [regex]::Matches($m.Groups[2].Value, '"([^"]+)"')) {
        $entries[$a.Groups[1].Value] = $tier   # 别名与正名同 tier
    }
}
Write-Output "[INFO] parsed $($entries.Count) command words (names + aliases)"

# 主流五家同名 24 条词表(名称 → 预期 tier)
$expected = [ordered]@{
    'new'         = 'Instant'
    'clear'       = 'Instant'
    'compact'     = 'Orchestrated'
    'resume'      = 'Instant'
    'model'       = 'Instant'
    'mode'        = 'Instant'
    'plan'        = 'Instant'
    'permissions' = 'Instant'
    'init'        = 'Agent'
    'diff'        = 'Instant'
    'review'      = 'Agent'
    'mention'     = 'Instant'
    'mcp'         = 'Orchestrated'
    'theme'       = 'Instant'
    'vim'         = 'Instant'
    'config'      = 'Instant'
    'status'      = 'Instant'
    'doctor'      = 'Instant'
    'help'        = 'Instant'
    'quit'        = 'Instant'      # exit 的别名
    'export'      = 'Instant'
    'undo'        = 'Orchestrated'
    'redo'        = 'Orchestrated'
    'focus'       = 'Instant'
    'pace'        = 'Instant'      # Concord W8 T8.3(ADR-080)配速档
    'context'     = 'Instant'      # Concord W9 T9.5(ADR-081)上下文网格
    'recap'       = 'Instant'      # Concord W11 T11.8(ADR-083)会话回顾
    'copy'        = 'Instant'      # Concord W11 T11.9(ADR-083)复制回复
    'notify'      = 'Instant'      # Concord W11 T11.3(ADR-083)通知开关
    'commands'    = 'Instant'      # Concord W11 T11.2(ADR-083)用户命令管理
    'agent tree'  = 'Instant'      # Concord W10 T10.4(ADR-082)Agent 谱系树
}

$diffs = @()
foreach ($cmd in $expected.Keys) {
    if (-not $entries.Contains($cmd)) {
        $diffs += "MISSING  /$cmd"
    } elseif ($entries[$cmd] -ne $expected[$cmd]) {
        $diffs += "TIER-MISMATCH  /$cmd expected=$($expected[$cmd]) actual=$($entries[$cmd])"
    }
}

if ($diffs.Count -gt 0) {
    Write-Output "[FAIL] slash vocab drift detected:"
    $diffs | ForEach-Object { Write-Output "  - $_" }
    exit 1
}
Write-Output "[OK] all $($expected.Count) mainstream-aligned slash commands present with correct tiers"
exit 0
