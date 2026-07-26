#!/usr/bin/env bash
# ============================================================
# Chimera CLI 性能红线 lint 静态验证脚本(Linux/macOS / Bash)
#
# 静态验证 spec.md KPI 表格中定义的全部性能红线(SLO)是否在代码库中
# 有对应的 benchmark/test 文件与函数,以及阈值标记是否就位。
#
# 检查项(每条红线 3 项):
# 1. 文件存在 — benchmark/test 文件路径有效
# 2. 函数存在 — `fn <function_name>` 在文件中可匹配
# 3. 阈值标记 — 阈值字符串(阈值常量名或阈值描述)在文件中可匹配
#
# 红线全集(8 条,来自 spec.md KPI 表格"性能 SLO 分层"):
# - RL-01: window_select <1ms(内环本地,维持)
# - RL-02: mlc_l2_knn <5ms(内环本地,维持)
# - RL-03: decay <1μs(内环本地,维持,阈值仅在 spec 中,代码无断言)
# - RL-04: wiki_knn @1000 <10ms(索引,既有)
# - RL-05: wiki_knn @10K p95<50ms(索引,P2-W8.1.3)
# - RL-06: wiki_knn @100K p95<50ms(索引,P2-W8.3.1 新增)
# - RL-07: 跨膜事件投递 p95<10ms(跨膜,P1-W2 新增)
# - RL-08: 50agent_mem_peak ≤130MB(MAS,维持)
#
# 退出码: 0=全部通过, 1=有检查项失败
# 使用方式: bash scripts/check_perf_redlines.sh
# 对应任务: P2-W8.3.2 红线 lint CI 化
# 权威源: spec.md KPI 表格(nexus-omega-v5-implementation-plan/spec.md L29)
# ============================================================

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FAIL_COUNT=0
WARN_COUNT=0

# 颜色输出(非交互终端禁用颜色)
if [ -t 1 ]; then
    GREEN='\033[0;32m'
    RED='\033[0;31m'
    YELLOW='\033[0;33m'
    CYAN='\033[0;36m'
    GRAY='\033[0;90m'
    NC='\033[0m'
else
    GREEN=''; RED=''; YELLOW=''; CYAN=''; GRAY=''; NC=''
fi

echo -e "\n${CYAN}=== Chimera CLI Performance Red Line Lint ===${NC}"
echo -e "    ${GRAY}(spec.md KPI: 性能 SLO 分层, 8 red lines)${NC}\n"

check() {
    local name="$1"
    local pass="$2"
    local detail="${3:-}"
    if [ "$pass" = "true" ]; then
        echo -e "  ${GREEN}[PASS]${NC} $name"
    else
        echo -e "  ${RED}[FAIL]${NC} $name"
        [ -n "$detail" ] && echo -e "         ${GRAY}$detail${NC}"
        FAIL_COUNT=$((FAIL_COUNT + 1))
    fi
}

warn() {
    local name="$1"
    local detail="${2:-}"
    echo -e "  ${YELLOW}[WARN]${NC} $name"
    [ -n "$detail" ] && echo -e "         ${GRAY}$detail${NC}"
    WARN_COUNT=$((WARN_COUNT + 1))
}

# --- 红线全集(spec.md KPI 表格"性能 SLO 分层") ---
# 每条红线:Id / Name / File(相对项目根) / Func(bench/test 函数名) / Threshold(阈值标记,空=跳过)
# 格式: Id|Name|File|Func|Threshold
REDLINES=(
    "RL-01|window_select <1ms|crates/chimera-mas/benches/mas_benchmark.rs|bench_window_select|1ms"
    "RL-02|mlc_l2_knn <5ms|crates/chimera-mas/benches/mas_benchmark.rs|bench_mlc_l2_knn_top10_4096|5ms"
    "RL-03|decay <1us|crates/decay-engine/benches/decay_bench.rs|single_decay_step_latency|"
    "RL-04|wiki_knn @1000 <10ms|crates/repo-wiki/benches/vector_bench.rs|single_thread_knn_latency|LARGE_SIZE"
    "RL-05|wiki_knn @10K p95<50ms|crates/repo-wiki/tests/hnsw_p95_test.rs|test_hnsw_10k_p95_below_50ms|P95_THRESHOLD_MS"
    "RL-06|wiki_knn @100K p95<50ms|crates/repo-wiki/tests/hnsw_p95_test.rs|test_hnsw_100k_p95_below_50ms|ENTRY_COUNT_100K"
    "RL-07|membrane delivery p95<10ms|crates/event-bus/benches/membrane_delivery_bench.rs|membrane_e2e_delivery|10ms"
    "RL-08|50agent_mem_peak <=130MB|crates/chimera-mas/benches/mas_benchmark.rs|bench_50agent_mem_peak|130"
)

for entry in "${REDLINES[@]}"; do
    IFS='|' read -r rl_id rl_name rl_file rl_func rl_threshold <<< "$entry"

    file_path="$PROJECT_ROOT/$rl_file"

    echo -e "\n  ${CYAN}[$rl_id] $rl_name${NC}"

    # 检查 1: 文件存在
    if [ ! -f "$file_path" ]; then
        check "$rl_id.1 file exists ($rl_file)" false
        echo -e "         ${GRAY}跳过函数/阈值检查(文件不存在)${NC}"
        continue
    fi
    check "$rl_id.1 file exists ($rl_file)" true

    # 检查 2: 函数存在
    if grep -q "fn ${rl_func}" "$file_path"; then
        check "$rl_id.2 function 'fn ${rl_func}' exists" true
    else
        check "$rl_id.2 function 'fn ${rl_func}' exists" false
    fi

    # 检查 3: 阈值标记存在(空=跳过,spec-only 红线)
    if [ -n "$rl_threshold" ]; then
        if grep -qF "$rl_threshold" "$file_path"; then
            check "$rl_id.3 threshold marker '$rl_threshold' exists" true
        else
            check "$rl_id.3 threshold marker '$rl_threshold' exists" false
        fi
    else
        warn "$rl_id.3 threshold marker" "阈值仅在 spec 中定义,代码无显式断言(spec-only)"
    fi
done

# --- 汇总 ---
echo -e "\n${CYAN}=== Summary ===${NC}"
total_checks=$(( ${#REDLINES[@]} * 2 ))
passed_checks=$(( total_checks - FAIL_COUNT ))
if [ "$FAIL_COUNT" -eq 0 ]; then
    summary_color="$GREEN"
else
    summary_color="$RED"
fi
echo -e "  ${summary_color}Passed: $passed_checks / $total_checks${NC}"
echo -e "  ${YELLOW}Warnings: $WARN_COUNT${NC}"
echo -e "  ${summary_color}Failed: $FAIL_COUNT${NC}"

if [ "$FAIL_COUNT" -gt 0 ]; then
    echo -e "\n  ${RED}RESULT: FAIL — 有 $FAIL_COUNT 项检查失败${NC}"
    exit 1
else
    echo -e "\n  ${GREEN}RESULT: PASS — 全部性能红线 lint 通过${NC}"
    exit 0
fi
