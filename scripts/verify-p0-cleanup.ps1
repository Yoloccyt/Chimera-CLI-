# scripts/verify-p0-cleanup.ps1
# ============================================================
# P0 修复验证脚本 — 分支清理 + 密钥扫描确认
# ============================================================
# 用法:
#   .\scripts\verify-p0-cleanup.ps1
#
# 前置:无依赖,纯 PowerShell + git 命令,无需网络。
#       如需完整 gitleaks 扫描,见下方 §4 的手动指引。
# ============================================================

$global:PassCount = 0
$global:FailCount = 0
$global:WarnCount = 0

function Write-Pass  { param([string]$Msg) $global:PassCount++; Write-Host "  ✅ $Msg" -ForegroundColor Green }
function Write-Fail  { param([string]$Msg) $global:FailCount++; Write-Host "  ❌ $Msg" -ForegroundColor Red }
function Write-Warn  { param([string]$Msg) $global:WarnCount++; Write-Host "  ⚠️  $Msg" -ForegroundColor Yellow }

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  P0 修复验证脚本" -ForegroundColor Cyan
Write-Host "  分支清理 + 密钥扫描" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# ==========================================================
# §1 分支清理验证
# ==========================================================
Write-Host "§1 分支清理" -ForegroundColor Cyan
Write-Host "────────────────────────────────────────" -ForegroundColor DarkGray

# 1.1 master 已从本地删除
$localBranches = git branch 2>&1
if ($localBranches -match 'master') {
    Write-Fail "本地仍有 master 分支残留"
} else {
    Write-Pass "本地无 master 分支"
}

# 1.2 master 已从远程删除
git fetch --prune 2>&1 | Out-Null
$remoteBranches = git branch -r 2>&1
if ($remoteBranches -match 'master') {
    Write-Fail "远程仍有 master 分支残留"
} else {
    Write-Pass "远程无 master 分支"
}

# 1.3 main 是当前活跃分支
$currentBranch = git branch --show-current 2>&1
if ($currentBranch -eq 'main') {
    Write-Pass "当前分支 = main"
} else {
    Write-Fail "当前分支不是 main (当前: $currentBranch)"
}

# 1.4 无孤儿分支残留（排除 release/ 分支，它们是已发布版本的历史引用）
$orphanPatterns = @('feat/v')
$branches = git branch 2>&1
foreach ($pattern in $orphanPatterns) {
    if ($branches -match $pattern) {
        $matched = ($branches | Select-String -Pattern $pattern).ToString().Trim()
        Write-Warn "发现可能需清理的孤儿分支: $matched"
    } else {
        Write-Pass "无匹配 '$pattern' 的残留分支"
    }
}

# 1.5 最近的清理提交存在
$recentLog = git log --oneline -5 2>&1
$cleanupCommitFound = $recentLog -match '仓库清理|清理 master|清理.*分支'
if ($cleanupCommitFound) {
    Write-Pass "历史记录中存在清理提交"
} else {
    Write-Warn "未在最近 5 条提交中发现清理记录 (可能是之前已提交)"
}

Write-Host ""

# ==========================================================
# §2 密钥扫描验证
# ==========================================================
Write-Host "§2 密钥泄漏扫描" -ForegroundColor Cyan
Write-Host "────────────────────────────────────────" -ForegroundColor DarkGray

# 2.1 敏感文件类型未被 git 跟踪
$sensitiveFiles = git ls-files '*.env' '*.env.*' '*.pem' '*.key' 2>&1
if ($sensitiveFiles) {
    Write-Fail "敏感文件被 git 跟踪: $sensitiveFiles"
} else {
    Write-Pass "无 .env/.pem/.key 文件被 git 跟踪"
}

# 2.2 AWS Key 泄漏
$leaks = git grep -in "AKIA[0-9A-Z]\{16\}" -- ':!*.lock' 2>&1
if ($leaks) { Write-Fail "发现 AWS Key 泄漏" } else { Write-Pass "AWS Key: 无泄漏" }

# 2.3 GitHub Token 泄漏
$leaks = git grep -in "gh[pousr]_[A-Za-z0-9_]\{36,\}" -- ':!*.lock' 2>&1
if ($leaks) { Write-Fail "发现 GitHub Token 泄漏" } else { Write-Pass "GitHub Token: 无泄漏" }

# 2.4 Private Key 泄漏（使用固定字符串搜索，避免 PowerShell 花括号转义问题）
$leaks = git grep -inF "BEGIN PRIVATE KEY" -- ':!*.lock' ':!target/' ':!**/tests/' ':!**/benches/' 2>&1
if ($leaks) { Write-Fail "发现 Private Key 泄漏: $leaks" } else { Write-Pass "Private Key: 无泄漏" }

# 2.5 Slack Token 泄漏
$leaks = git grep -in "xox[baprs]-[A-Za-z0-9\-]\{10,\}" -- ':!*.lock' 2>&1
if ($leaks) { Write-Fail "发现 Slack Token 泄漏" } else { Write-Pass "Slack Token: 无泄漏" }

# 2.6 npm Token 泄漏
$leaks = git grep -in "npm_[A-Za-z0-9]\{36,\}" -- ':!*.lock' 2>&1
if ($leaks) { Write-Fail "发现 npm Token 泄漏" } else { Write-Pass "npm Token: 无泄漏" }

# 2.7 JWT Token 泄漏（跳过 .md 假阳性）
$leaks = git grep -in "eyJ[A-Za-z0-9_\-]\{10,\}\.[A-Za-z0-9_\-]\{10,\}\.[A-Za-z0-9_\-]\{10,\}" -- ':!*.lock' ':!*.md' 2>&1
if ($leaks) { Write-Fail "发现 JWT Token 泄漏" } else { Write-Pass "JWT Token: 无泄漏" }

# 2.8 硬编码密码（常见模式）
$leaks = git grep -inP '(password|secret|api_key|apiKey|auth_token)\s*=\s*[''"][^''"]+[''"]' -- '*.rs' '*.toml' '*.yml' '*.yaml' '*.json' 2>&1 | Select-Object -First 10
if ($leaks) {
    Write-Warn "发现可能的硬编码凭据赋值 (需人工审查):"
    $leaks | ForEach-Object { Write-Host "       $_" -ForegroundColor Yellow }
} else {
    Write-Pass "无硬编码凭据赋值"
}

Write-Host ""

# ==========================================================
# §3 总结
# ==========================================================
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  验证结果汇总" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

$total = $global:PassCount + $global:FailCount + $global:WarnCount
Write-Host "  总计: $total 项检查" -ForegroundColor White
Write-Host "  ✅ 通过: $($global:PassCount)" -ForegroundColor Green
if ($global:WarnCount -gt 0) { Write-Host "  ⚠️  警告: $($global:WarnCount)" -ForegroundColor Yellow }
if ($global:FailCount -gt 0) { Write-Host "  ❌ 失败: $($global:FailCount)" -ForegroundColor Red }

Write-Host ""
if ($global:FailCount -eq 0) {
    Write-Host "  P0 验证全部通过。分支已清理，密钥无泄漏。" -ForegroundColor Green
} else {
    Write-Host "  $($global:FailCount) 项检查未通过，请修复后重试。" -ForegroundColor Red
}

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan

# ==========================================================
# §4 附录：gitleaks 手动执行指引
# ==========================================================
Write-Host ""
Write-Host "§4 附录 — gitleaks 完整扫描指引" -ForegroundColor DarkGray
Write-Host "────────────────────────────────────────" -ForegroundColor DarkGray
Write-Host ""
Write-Host "  本脚本使用 git grep 做基础模式匹配，覆盖常见密钥类型。"
Write-Host "  如需更全面的熵检测 + 自定义规则扫描，建议在有网络的环境安装 gitleaks："
Write-Host ""
Write-Host "  # 安装:"
Write-Host "  winget install gitleaks                    # Windows"
Write-Host "  brew install gitleaks                      # macOS"
Write-Host "  sudo apt install gitleaks                  # Debian/Ubuntu"
Write-Host ""
Write-Host "  # 全历史深度扫描:"
Write-Host "  cd 'D:\Chimera CLI'"
Write-Host '  gitleaks detect --source . --verbose --log-opts="--all"'
Write-Host ""
Write-Host "  # 若发现泄漏，熔断处理:"
Write-Host '  java -jar bfg.jar --replace-text passwords.txt'
Write-Host '  git reflog expire --expire=now --all'
Write-Host '  git gc --prune=now --aggressive'
Write-Host '  git push origin --force --all'
Write-Host ""
