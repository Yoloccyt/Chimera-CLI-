#!/usr/bin/env bash
# =============================================================================
# check_parallel_redlines.sh - 并行红线静态检查（P1-T13 禁令 lint，bash 形态）
# =============================================================================
# 与 check_parallel_redlines.ps1（G-134，Windows 权威执行体）逐条等价的 CI 形态
# （G-133，接 ci.yml check job），沿 G-04/G-05 双源先例。四条禁令与 .ps1 头注一致：
#   1. 禁自旋：wait_spin/spin_loop/yield_now 忙轮询（统一 Notify 等待）——测试模块内豁免
#      （测试同步辅助合法）
#   2. 禁 IO-on-rayon：spawn_compute 单行闭包内 .await（闭包只做纯计算）
#   3. 禁 rayon::join 多闭包误用（join 只收 2 闭包，批处理用 par_iter/scope；
#      tokio::join! 合法不查）
#   4. 禁同步锁持锁跨 .await（std Mutex/RwLock 的 lock() 与 .await 同现；
#      tokio 异步锁 lock().await 合法）
#
# 判定逻辑为 .ps1 的忠实移植（含测试模块边界跟踪与 R3 自原始行首个 "(" 起数逗号的
# 计数口径）；两源唯一差异是执行体（pwsh / bash+awk）。
#
# 定位：静态近似的纪律扫描（精确保证靠既有测试与代码审查）；
#      零命中是波次收口门禁（手册 Ch12 W8）。
# 用法：bash scripts/check_parallel_redlines.sh [crate名可选]
# 退出码：0 = 全部通过；1 = 命中（打印违规清单）
# =============================================================================
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"   # 违规报告用仓库相对路径，与 .ps1 的 rel 口径一致；不依赖调用方 cwd

target_crate="${1:-}"

# 收集目标 crate 目录下 src/ 内的全部 .rs（与 .ps1 同口径：crates/<name>/src 递归）
shopt -s nullglob
files=()
if [ -n "$target_crate" ]; then
    crate_dirs=("$root/crates/$target_crate")
else
    crate_dirs=("$root"/crates/*)
fi
for dir in "${crate_dirs[@]}"; do
    if [ -d "$dir/src" ]; then
        while IFS= read -r -d '' f; do
            files+=("${f#"$root"/}")
        done < <(find "$dir/src" -type f -name '*.rs' -print0)
    fi
done

# 逐文件扫描（单 awk 全量过，per-file 状态在 FNR==1 重置）
scan() {
    awk '
    FNR == 1 { inTest = 0 }
    {
        raw = $0
        trimmed = raw
        sub(/^[ \t\r]+/, "", trimmed)
        sub(/[ \t\r]+$/, "", trimmed)

        # 测试模块入口：#[cfg(test)] 或顶格 mod tests → 标记行本身不参与判定（continue）
        if (trimmed ~ /#\[cfg\(test\)\]/ || trimmed ~ /^mod tests([^_[:alnum:]]|$)/) {
            inTest = 1
            next
        }
        # 测试模块出口：顶格（行首无空白）且非 "}" 且非空且非 "//" 注释 → 测试块结束
        if (inTest && raw !~ /^[ \t\r]/ && substr(raw, 1, 1) != "}" && trimmed != "" && substr(trimmed, 1, 2) != "//") {
            inTest = 0
        }

        # 跳过注释行（// 或 /// 或 #![）
        if (substr(trimmed, 1, 2) == "//" || substr(trimmed, 1, 3) == "#![") next

        # 规则 1：禁自旋（ADR-129）——测试模块豁免
        if (!inTest && trimmed ~ /wait_spin[[:space:]]*\(|spin_loop[[:space:]]*\(|std::thread::yield_now[[:space:]]*\(/) {
            print FILENAME ":" FNR " [R1 禁自旋] " trimmed
        }

        # 规则 2：IO-on-rayon——spawn_compute 单行闭包内 .await（同行 => 箭头且同行 .await）
        if (trimmed ~ /spawn_compute(_batch)?[[:space:]]*\([^)]*=>/ && trimmed ~ /\.await/) {
            print FILENAME ":" FNR " [R2 IO-on-rayon 疑似] spawn_compute 闭包内 await（" trimmed "）"
        }

        # 规则 3：rayon::join 误用——自原始行首个 "(" 起数逗号，>=2 即疑似（tokio::join! 不查）
        if (trimmed ~ /rayon::join[[:space:]]*\(/) {
            openIdx = index(raw, "(")
            if (openIdx > 0) {
                inside = substr(raw, openIdx)
                commaCount = gsub(/,/, "", inside)
                if (commaCount >= 2) {
                    print FILENAME ":" FNR " [R3 join 多闭包疑似] " trimmed
                }
            }
        }

        # 规则 4：同步锁持锁跨 await——存在任一 .lock() 其后不紧随空白+.await，
        # 且同行另有 .await（.ps1 的负向断言 (?!\s*\.await) 展开为逐次出现位置检查；
        # tokio 异步锁 lock().await 合法）
        if (trimmed ~ /\.lock\(\)/ && trimmed ~ /\.await/) {
            rest = trimmed
            while ((pos = index(rest, ".lock()")) > 0) {
                after = substr(rest, pos + 7)
                if (after !~ /^[[:space:]]*\.await/) {
                    print FILENAME ":" FNR " [R4 持锁跨 await 疑似] " trimmed
                    break
                }
                rest = after
            }
        }
    }
    ' "$@"
}

# WHY 经临时文件而非进程替换收结果：`mapfile < <(scan ...)` 会丢失 awk 的非零
# 退出码，awk 打不开文件时将静默变 PASS（假绿）；走重定向则 set -e 直接判红。
tmp_report="$(mktemp)"
trap 'rm -f "$tmp_report"' EXIT
if [ "${#files[@]}" -gt 0 ]; then
    scan "${files[@]}" > "$tmp_report"
    mapfile -t violations < "$tmp_report"
else
    violations=()
fi

if [ "${#violations[@]}" -eq 0 ]; then
    echo "[PASS] 并行红线零命中（${#files[@]} 文件）"
    exit 0
else
    echo "[FAIL] 并行红线命中 ${#violations[@]} 处："
    for v in "${violations[@]}"; do
        echo "  $v"
    done
    exit 1
fi
