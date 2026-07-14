# scripts/setup-gpg-signing.ps1
# ============================================================
# Release GPG 签名 — 一次性密钥生成 + 配置指引
# ============================================================
# 前置:需要 MSYS2 的 gpg (D:\msys64\usr\bin\gpg.exe)
# 运行此脚本后,按提示完成交互操作。
# ============================================================

$GPG = "D:\msys64\usr\bin\gpg.exe"

if (-not (Test-Path $GPG)) {
    Write-Output "ERROR: MSYS2 GPG not found at $GPG"
    Write-Output "请安装 MSYS2 或 GnuPG (winget install GnuPG.GnuPG)"
    exit 1
}

Write-Output "============================================"
Write-Output " Step 1: 生成 GPG 签名密钥"
Write-Output "============================================"
Write-Output ""
Write-Output "将打开交互式向导,请选择:"
Write-Output "  - 密钥类型: RSA and RSA (default)"
Write-Output "  - 密钥长度: 4096"
Write-Output "  - 过期时间: 2y (2年)"
Write-Output "  - 姓名: NEXUS-OMEGA Team"
Write-Output "  - 邮箱: team@chimera-cli.dev"
Write-Output "  - 注释: (留空)"
Write-Output "  - 密码: (推荐设置,但 CI 自动化签名需无密码,见下方说明)"
Write-Output ""
Write-Output "⚠️  CI 自动化签名需要无密码私钥。建议:"
Write-Output "  1. 先生成一个带密码的主密钥(用于本地手动签名)"
Write-Output "  2. 再生成一个无密码的子密钥(仅用于 CI 自动签名)"
Write-Output "  生成后此脚本会自动导出所需格式。"
Write-Output "============================================"
Write-Output ""
Write-Host "按 Enter 开始生成密钥..." -NoNewline
$null = Read-Host

& $GPG --full-generate-key

Write-Output "`n============================================"
Write-Output " Step 2: 列出已生成的密钥"
Write-Output "============================================"
& $GPG --list-keys

Write-Output "`n============================================"
Write-Output " Step 3: 导出公钥 (上传到 GitHub)"
Write-Output "============================================"
Write-Output "复制下面的公钥,添加到:"
Write-Output "  https://github.com/settings/keys → New GPG Key"
Write-Output ""

$PubKey = & $GPG --armor --export "team@chimera-cli.dev"
Write-Output $PubKey

$PubKey | Out-File -FilePath "$env:USERPROFILE\.aether\release-public.key" -Encoding ascii
Write-Output "(已保存到 $env:USERPROFILE\.aether\release-public.key)"

Write-Output "`n============================================"
Write-Output " Step 4: 导出 CI 私钥 (添加到 GitHub Secrets)"
Write-Output "============================================"
Write-Output ""
Write-Output "⚠️  此操作将显示您的私钥!请确保在安全环境中执行。"
Write-Output "将以下输出添加到: https://github.com/Yoloccyt/Chimera-CLI-/settings/secrets/actions"
Write-Output "  Secret name: GPG_KEY_SIGNING"
Write-Output ""
Write-Output "是否导出私钥? (y/N): " -NoNewline
$confirm = Read-Host
if ($confirm -eq 'y') {
    $PrivKey = & $GPG --armor --export-secret-key "team@chimera-cli.dev"
    Write-Output $PrivKey
    Write-Output ""
    Write-Output "⚠️  请确保输出内容完整复制到 GitHub Secrets 中!"
    Write-Output "   添加后可在浏览器关闭此窗口。"
}

Write-Output "`n============================================"
Write-Output " Step 5: 验证配置"
Write-Output "============================================"
Write-Output "运行以下命令验证签名是否生效:"
Write-Output ""
Write-Output '  echo "test" > /tmp/test.txt'
Write-Output '  gpg --batch --yes --detach-sign --armor /tmp/test.txt'
Write-Output '  gpg --verify /tmp/test.txt.asc /tmp/test.txt'
Write-Output ""
Write-Output "注意: 首次提交后 GitHub 会自动将 commit 标记为 Verified"
Write-Output "如果未显示 Verified,请检查:"
Write-Output "  1. 公钥是否上传到 GitHub Profile → GPG Keys"
Write-Output "  2. 邮箱是否与 git commit 使用的邮箱一致"
