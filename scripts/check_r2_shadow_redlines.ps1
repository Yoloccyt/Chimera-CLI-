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
#
# ⚠ 门内身份诚实声明（2026-10-09 v3.5 W6-2）：本脚本**不在** `scripts/gate_manifest.toml`
#   的门清单内（无 G-号；条数以 scripts/gate_manifest.toml 现值为准，不在本注里写死），
#   因此 `run_gate_manifest.py` 与 CI check job **不执行它**；本脚本是人工/专项纪律扫描，
#   不应当被引用为“CI 已拦”的证据。文档侧同源订正见
#   `docs/architecture/CODE_WIKI.md` 与 `docs/architecture/adr_index.md` 的 R2 影子期段
#   （“五关键词无独立 CI 扫描 job；真守卫是 tests/e2e/r2_freeze_violation_e2e.rs”）。
#   另：本扫描只覆盖下面 $targets 列出的文件（现 2 个），**不是**全仓扫描——把它当
#   全仓红线扫描引用就是夸大（v3.5 取证时文档正是这么错的）。
#
# 退出码方言（与 F39b / gate_rc 一致）：
#   0 = 目标全存在且零命中；1 = 命中违规（打印清单）；
#   2 = **不可判定**（声明的 $targets 里有文件不存在）——缺目标时必须从“零命中”
#   降为“没查”，而不是静默 `continue` 后报 PASS（先例：`audit_fnlen.py` 缺登记册主动退 2、
#   `check_nexus_core_dep_freeze.py` manifest 缺失 exit 2 fail-closed）。
# 用法：pwsh -File scripts/check_r2_shadow_redlines.ps1

$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
$violations = [System.Collections.Generic.List[string]]::new()
$missing = [System.Collections.Generic.List[string]]::new()
$scanned = 0

# 扫描 Shadow 相关模块（rtl_shadow + 未来同类）
$targets = @(
    (Join-Path $root 'crates/gsoe-evolution/src/rtl_shadow.rs')
    (Join-Path $root 'crates/omega-learner/src/rtl_seam.rs')
)

foreach ($f in $targets) {
    if (-not (Test-Path $f)) {
        # 不再 `continue`：缺目标 = 本次判定覆盖不全，必须计入不可判定集。
        $missing.Add((Split-Path -Leaf $f))
        continue
    }
    $scanned += 1
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

if ($missing.Count -gt 0) {
    # 先判覆盖再判结果：一个没扫到的目标不得被“零命中”洗成绿。
    Write-Host "[UNDECIDABLE] 声明的扫描目标有 $($missing.Count) 个不存在（$($missing -join ', ')）；已扫描 $scanned/$($targets.Count) 个。"
    Write-Host "  修法是：把已退役模块从 targets 列表删掉（并记 DELETED-BY-ADR），或补回目标文件；不得让缺目标以 PASS 形态进入收口判据。"
    exit 2
}
if ($violations.Count -eq 0) {
    Write-Host "[PASS] R2 Shadow 红线零命中（$scanned/$($targets.Count) 目标已扫描；本脚本不在门清单内，不等于 CI 已拦）"
    exit 0
} else {
    Write-Host "[FAIL] R2 Shadow 红线命中 $($violations.Count) 处："
    $violations | ForEach-Object { Write-Host "  $_" }
    exit 1
}
