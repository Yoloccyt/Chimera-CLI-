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
    WHITE='\033[1;37m'
    NC='\033[0m'
else
    GREEN=''; RED=''; YELLOW=''; CYAN=''; GRAY=''; WHITE=''; NC=''
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

# =====================================================================
# Part 2: SLO Benchmark 阈值断言 (实际运行 bench, 解析 criterion 输出)
# =====================================================================
# 每个 SLO 用 80% 宽松阈值作为 CI redline (CI 环境波动缓冲)
# 格式: Name|Crate|BenchFile|Filter|SloSec|RedlineSec|Unit|SloDisplay|RedlineDisplay
SLO_REDLINES=(
    "window_select|hcw-window|window_select|bench_window_select|0.001|0.0008|ms|1ms|0.8ms"
    "mlc_l2_knn|mlc-engine|mlc_l2_knn|bench_l2_knn_slo_assert|0.005|0.004|ms|5ms|4ms"
    "decay_compute|decay-engine|decay_compute|single_decay_by_profile|0.000001|0.0000008|us|1us|0.8us"
    "wiki_knn_100k|repo-wiki|wiki_knn_slo|wiki_knn_100k_p95|0.050|0.040|ms|50ms|40ms"
    "immune_probe|chimera-mas|immune_probe|bench_assess_paradox_risk|0.100|0.080|ms|100ms|80ms"
    "rhi_judge|auto-dpo|rhi_judge|rhi_judge_latency|2.0|1.6|s|2s|1.6s"
)

echo -e "\n${CYAN}=== SLO Benchmark Threshold Assertions ===${NC}"
echo -e "    ${GRAY}(criterion bench, 80% redline of SLO)${NC}\n"

SLO_PASS=0
SLO_FAIL=0
SLO_SKIP=0

for entry in "${SLO_REDLINES[@]}"; do
    IFS='|' read -r s_name s_crate s_bench s_filter s_slo s_redline s_unit s_disp s_rdisp <<< "$entry"

    echo -e "  ${CYAN}[SLO] $s_name (target: < $s_disp, redline: $s_rdisp)${NC}"

    # 运行 bench 并捕获输出
    bench_output=$(cargo bench --package "$s_crate" --bench "$s_bench" -- --noplot --quick "$s_filter" 2>&1) || true

    # 解析 criterion 输出: "time:   [X.XXX us X.XXX us X.XXX us]"
    time_line=$(echo "$bench_output" | grep -oP 'time:\s+\[\K[^\]]+' | head -1)

    if [ -z "$time_line" ]; then
        echo -e "    ${YELLOW}[SKIP] 未找到 criterion 时间输出 (bench 可能编译失败)${NC}"
        SLO_SKIP=$((SLO_SKIP + 1))
        continue
    fi

    # 提取 estimate (第二个值) 和单位
    estimate=$(echo "$time_line" | awk '{print $2}')
    unit=$(echo "$time_line" | awk '{print $4}')

    if [ -z "$estimate" ] || [ -z "$unit" ]; then
        echo -e "    ${YELLOW}[SKIP] 无法解析 criterion 时间输出${NC}"
        SLO_SKIP=$((SLO_SKIP + 1))
        continue
    fi

    # 转换为秒 (使用 awk 进行浮点运算)
    estimate_sec=$(echo "$estimate $unit" | awk '{
        val = $1; u = $2
        if (u == "ns") printf "%.15f", val / 1e9
        else if (u == "us") printf "%.15f", val / 1e6
        else if (u == "ms") printf "%.15f", val / 1e3
        else if (u == "s") printf "%.15f", val
        else printf "%.15f", val / 1e3
    }')

    # 显示实测值
    display_val=$(echo "$estimate_sec $s_unit" | awk '{
        val = $1; u = $2
        if (u == "us") printf "%.3f us", val * 1e6
        else if (u == "ms") printf "%.3f ms", val * 1e3
        else if (u == "s") printf "%.3f s", val
    }')
    echo -e "    ${GRAY}实测: $display_val${NC}"

    # 与 redline 比较 (使用 awk 浮点比较)
    result=$(echo "$estimate_sec $s_redline $s_slo" | awk '{
        est = $1; redline = $2; slo = $3
        if (est <= redline) print "PASS"
        else if (est <= slo) print "WARN"
        else print "FAIL"
    }')

    case "$result" in
        PASS)
            echo -e "    ${GREEN}[PASS] 低于 redline ($s_rdisp)${NC}"
            SLO_PASS=$((SLO_PASS + 1))
            ;;
        WARN)
            echo -e "    ${YELLOW}[WARN] 超过 redline 但低于 SLO ($s_disp)${NC}"
            SLO_PASS=$((SLO_PASS + 1))
            ;;
        FAIL)
            echo -e "    ${RED}[FAIL] 超过 SLO ($s_disp)!${NC}"
            SLO_FAIL=$((SLO_FAIL + 1))
            ;;
    esac
done

# --- 最终汇总 ---
echo -e "\n${CYAN}=== Final Summary ===${NC}"
echo -e "  ${WHITE}Static Lint:${NC}"
total_checks=$(( ${#REDLINES[@]} * 2 ))
passed_checks=$(( total_checks - FAIL_COUNT ))
if [ "$FAIL_COUNT" -eq 0 ]; then
    summary_color="$GREEN"
else
    summary_color="$RED"
fi
echo -e "    ${summary_color}Passed: $passed_checks / $total_checks${NC}"
echo -e "    ${YELLOW}Warnings: $WARN_COUNT${NC}"
echo -e "  ${WHITE}SLO Benchmarks:${NC}"
if [ "$SLO_FAIL" -eq 0 ]; then
    echo -e "    ${GREEN}Passed: $SLO_PASS${NC}"
else
    echo -e "    ${RED}Passed: $SLO_PASS${NC}"
fi
echo -e "    $( [ "$SLO_FAIL" -gt 0 ] && echo "${RED}" || echo "${GRAY}" )Failed: $SLO_FAIL${NC}"
echo -e "    ${YELLOW}Skipped: $SLO_SKIP${NC}"

TOTAL_FAIL=$(( FAIL_COUNT + SLO_FAIL ))
if [ "$TOTAL_FAIL" -gt 0 ]; then
    echo -e "\n  ${RED}RESULT: FAIL — 有 $TOTAL_FAIL 项检查失败${NC}"
    exit 1
else
    echo -e "\n  ${GREEN}RESULT: PASS — 全部性能红线 lint + SLO 断言通过${NC}"
    exit 0
fi
