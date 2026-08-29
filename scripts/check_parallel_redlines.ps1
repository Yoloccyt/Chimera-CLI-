# Chimera CLI 并行红线静态检查（P1-T13 禁令 lint）
# ============================================================
# 检查四条并行禁令（手册 §8.2 禁令表 + ADR-129 + 红线 3）：
#   1. 禁自旋：wait_spin/spin_loop/yield_now 忙轮询（统一 Notify 等待）——测试模块内豁免（测试同步辅助合法）
#   2. 禁 IO-on-rayon：spawn_compute 单行闭包内 .await（闭包只做纯计算）
#   3. 禁 rayon::join 多闭包误用（join 只收 2 闭包，批处理用 par_iter/scope；tokio::join! 合法不查）
#   4. 禁同步锁持锁跨 .await（std Mutex/RwLock 的 lock() 与 .await 同现；tokio 异步锁 lock().await 合法）
#
# 定位：静态近似的纪律扫描（精确保证靠既有测试与代码审查）；
#      零命中是波次收口门禁（手册 Ch12 W8）。
# 用法：pwsh -File scripts/check_parallel_redlines.ps1 [crate名可选]
# 退出码：0 = 全部通过；1 = 命中（打印违规清单）

param(
    [string]$TargetCrate = ''   # 空 = 全 workspace crates
)

$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
$violations = [System.Collections.Generic.List[string]]::new()

# 收集目标 crate 目录
if ($TargetCrate -ne '') {
    $crateDirs = @((Join-Path $root "crates\$TargetCrate"))
} else {
    $crateDirs = Get-ChildItem (Join-Path $root 'crates') -Directory | ForEach-Object { $_.FullName }
}

# 逐文件扫描（仅 src/ 下的 .rs）
$files = foreach ($dir in $crateDirs) {
    if (Test-Path (Join-Path $dir 'src')) {
        Get-ChildItem (Join-Path $dir 'src') -Recurse -Filter '*.rs'
    }
}

foreach ($f in $files) {
    $rel = $f.FullName.Substring($root.Length + 1)
    $lines = Get-Content $f.FullName
    $inTest = $false   # 当前是否位于测试模块（#[cfg(test)] 或 mod tests）内

    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        $trimmed = $line.Trim()

        # 测试模块边界跟踪：#[cfg(test)] 或 mod tests 标记行置 true 并跳过本行（标记行不参与退出判定）
        if ($trimmed -match '#\[cfg\(test\)\]' -or $trimmed -match '^mod tests\b') {
            $inTest = $true
            continue
        }
        # 退出判定：顶格（行首无空白）且非 } 且非空且非注释 → 测试块已结束
        if ($inTest -and $line -notmatch '^\s' -and -not $line.StartsWith('}') -and $trimmed -ne '' -and -not $trimmed.StartsWith('//')) {
            $inTest = $false
        }

        # 跳过注释行（// 或 /// 或 #![）
        if ($trimmed.StartsWith('//') -or $trimmed.StartsWith('#![')) { continue }

        # 规则 1：禁自旋（ADR-129）——wait_spin / spin_loop / yield_now 调用（测试模块豁免）
        if (-not $inTest -and $trimmed -match 'wait_spin\s*\(|spin_loop\s*\(|std::thread::yield_now\s*\(') {
            $violations.Add("${rel}:$($i+1) [R1 禁自旋] $trimmed")
        }

        # 规则 2：IO-on-rayon——spawn_compute 单行闭包内 .await（同行 => 箭头且同行 .await）
        if ($trimmed -match 'spawn_compute(_batch)?\s*\([^)]*=>' -and $trimmed -match '\.await') {
            $violations.Add("${rel}:$($i+1) [R2 IO-on-rayon 疑似] spawn_compute 闭包内 await（$trimmed）")
        }

        # 规则 3：rayon::join 误用——3+ 参数（tokio::join! 合法，不查）
        if ($trimmed -match 'rayon::join\s*\(') {
            $openIdx = $line.IndexOf('(')
            if ($openIdx -ge 0) {
                $inside = $line.Substring($openIdx)
                $commaCount = ([regex]::Matches($inside, ',')).Count
                if ($commaCount -ge 2) {
                    $violations.Add("${rel}:$($i+1) [R3 join 多闭包疑似] $trimmed")
                }
            }
        }

        # 规则 4：同步锁持锁跨 await——lock() 后非 .await（排除 tokio 异步锁 lock().await）且同行另有 .await
        if ($trimmed -match '\.lock\(\)(?!\s*\.await)' -and $trimmed -match '\.await') {
            # 二次确认：不是 lock().await 形式（负向断言已排除），即同步锁 + await 同现
            $violations.Add("${rel}:$($i+1) [R4 持锁跨 await 疑似] $trimmed")
        }
    }
}

# 输出
if ($violations.Count -eq 0) {
    Write-Host "[PASS] 并行红线零命中（$($files.Count) 文件）"
    exit 0
} else {
    Write-Host "[FAIL] 并行红线命中 $($violations.Count) 处："
    $violations | ForEach-Object { Write-Host "  $_" }
    exit 1
}
