# Chimera CLI R2 Shadow 红线静态检查（P2-T10 门禁）
# ============================================================
# v4.0 §17 禁止回退项 + 2026-08-15 治理决策：
#   WI-30 RTL Shadow 必须满足——零 Python 进程、零网络外发、零自动转正路径。
# 检查目标（crates/gsoe-evolution/src/rtl_shadow.rs 及同类 Shadow 模块）：
#   1. 无 Python 依赖：禁止 `python`/`py` 子进程调用、`.py` 文件引用
#   2. 无网络外发：禁止 std::net / reqwest / tokio::net 等网络 IO
#   3. 无自动转正：禁止 apply/promote/activate 等"策略生效"方法名
#      （API 面只允许 record/prior/report/shadow 前缀）
#   4. 无梯度/权重更新：禁止 nalgebra/ndarray 的梯度类符号（简化：检查
#      "gradient|backprop|weight_update" 关键词）
#
# 定位：静态近似的纪律扫描（精确保证靠既有测试与代码审查）；
#      零命中是波次收口门禁（v4.0 WI-30 验收）。
# 用法：pwsh -File scripts/check_r2_shadow_redlines.ps1
# 退出码：0 = 全部通过；1 = 命中（打印违规清单）

$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
$violations = [System.Collections.Generic.List[string]]::new()

# 扫描 Shadow 相关模块（rtl_shadow + 未来同类）
$targets = @(
    (Join-Path $root 'crates/gsoe-evolution/src/rtl_shadow.rs')
    (Join-Path $root 'crates/omega-learner/src/rtl_seam.rs')
)

foreach ($f in $targets) {
    if (-not (Test-Path $f)) { continue }
    $rel = $f.Substring($root.Length + 1)
    $lines = Get-Content $f
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        $trimmed = $line.Trim()
        # 跳过注释行
        if ($trimmed.StartsWith('//') -or $trimmed.StartsWith('#![')) { continue }

        # R1：Python 进程/文件
        if ($trimmed -match 'Command::new\("python|Command::new\("py|\.py\b' -or $trimmed -match 'python\s*:\s*(true|")') {
            $violations.Add("${rel}:$($i+1) [R1 Python 依赖] $trimmed")
        }
        # R2：网络外发（std::net / reqwest / tokio::net）
        if ($trimmed -match 'std::net::|reqwest|tokio::net::|TcpStream|UdpSocket') {
            $violations.Add("${rel}:$($i+1) [R2 网络外发] $trimmed")
        }
        # R3：自动转正路径（apply/promote/activate 方法定义）
        if ($trimmed -match 'pub fn (apply|promote|activate|deploy|enable)\b') {
            $violations.Add("${rel}:$($i+1) [R3 自动转正路径] $trimmed")
        }
        # R4：梯度/权重更新
        if ($trimmed -match 'gradient|backprop|weight_update|\.backward\(') {
            $violations.Add("${rel}:$($i+1) [R4 梯度/权重更新] $trimmed")
        }
    }
}

if ($violations.Count -eq 0) {
    Write-Host "[PASS] R2 Shadow 红线零命中（$($targets.Count) 目标）"
    exit 0
} else {
    Write-Host "[FAIL] R2 Shadow 红线命中 $($violations.Count) 处："
    $violations | ForEach-Object { Write-Host "  $_" }
    exit 1
}
