#!/usr/bin/env bash
# repo-wiki CI benchmark 阈值断言脚本(P1-7)
#
# 对应任务:P1-7 实施 repo-wiki CI benchmark 阈值断言
# 对应 SLO:wiki_knn @100K p95 < 50ms(spec.md 索引 SLO 红线)
# 对应基线:hnsw_100k_p95_search_latency = 152.84 µs(2026-07-27 Phase 4 实测)
#
# # 设计决策
# - CI 中仅运行 10K entry benchmark(快速验证,~30s):100K 预填充在 debug 模式 >5 分钟,
#   不适合 CI 快速反馈。100K benchmark 在 release 模式或本地验证。
# - 阈值设定基于基线 + CI 环境余量(约 4-10 倍):CI runner 性能波动较大,
#   设宽松阈值避免误报,同时能捕获数量级退化。
# - 解析 criterion stdout 输出:criterion 默认输出人类可读格式,
#   用正则表达式提取 `time: [low mean high]` 中的 mean 值。
#
# # 阈值表
# | benchmark                        | 基线(本地)    | CI 阈值       | 余量  |
# |----------------------------------|--------------|--------------|-------|
# | hnsw_10k_search_latency          | ~50 µs       | < 10 ms      | 200×  |
# | hnsw_10k_p95_search_latency      | ~100 µs      | < 20 ms      | 200×  |
# | single_thread_knn_latency/100    | ~10 µs       | < 5 ms       | 500×  |
# | single_thread_knn_latency/1000   | ~100 µs      | < 20 ms      | 200×  |
#
# # 用法
# bash scripts/check_repo_wiki_benchmark.sh           # 默认:10K 快速模式
# bash scripts/check_repo_wiki_benchmark.sh --full    # 完整模式:含 100K(需 release)
# bash scripts/check_repo_wiki_benchmark.sh --quick   # criterion --quick 模式(最快速)
#
# # 退出码
# 0 = 全部 benchmark 通过阈值
# 1 = 至少一个 benchmark 超过阈值
# 2 = 脚本错误(cargo bench 失败 / 解析失败)

set -euo pipefail

# ============================================================
# 配置
# ============================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

# 颜色输出(CI 兼容)
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# benchmark 阈值表(单位:毫秒)
# 格式:"benchmark_name_regex=threshold_ms"
# WHY 按 Name 长度降序排列:避免短前缀误匹配长 benchmark 名。
# 例如 'single_thread_knn_latency/100' 会正则匹配 'single_thread_knn_latency/1000'
# (因 =~ 是 "contains" 语义),导致 1000-entry benchmark 误用 100-entry 阈值。
# 降序排列确保更具体的模式(长名)优先匹配。
declare -a THRESHOLDS=(
    "hnsw_10k_p95_search_latency=20"
    "hnsw_10k_search_latency=10"
    "single_thread_knn_latency/1000=20"
    "single_thread_knn_latency/100=5"
)

# 完整模式额外阈值
declare -a THRESHOLDS_FULL=(
    "hnsw_100k_search_latency=50"
    "hnsw_100k_p95_search_latency=200"
)

# 模式选择
MODE="${1:-default}"
QUICK_FLAG=""
# WHY: criterion bench 过滤参数是正则表达式,多个参数会 AND 组合(非 OR)。
# 用 `|` 正则元字符在一个参数中表达多选一,避免 AND 语义导致无匹配。
BENCH_FILTER="hnsw_10k|single_thread_knn"

if [[ "$MODE" == "--full" ]]; then
    BENCH_FILTER="hnsw|single_thread_knn"
    THRESHOLDS+=("${THRESHOLDS_FULL[@]}")
    echo -e "${YELLOW}[INFO] 完整模式:含 100K benchmark(预计 5-10 分钟)${NC}"
elif [[ "$MODE" == "--quick" ]]; then
    QUICK_FLAG="--quick"
    echo -e "${YELLOW}[INFO] Quick 模式:criterion --quick(最快速,不精确)${NC}"
else
    echo -e "${YELLOW}[INFO] 默认模式:仅 10K benchmark(预计 ~30s)${NC}"
fi

# ============================================================
# 运行 benchmark
# ============================================================

echo ""
echo "=== 运行 repo-wiki benchmark ==="
echo "命令: cargo bench -p repo-wiki --bench vector_bench -- \"$BENCH_FILTER\" $QUICK_FLAG"
echo ""

# 捕获 criterion 输出(BENCH_FILTER 作为单个正则参数传递,避免 AND 语义)
BENCH_OUTPUT=$(cargo bench -p repo-wiki --bench vector_bench -- "$BENCH_FILTER" $QUICK_FLAG 2>&1) || {
    echo -e "${RED}[ERROR] cargo bench 执行失败${NC}"
    echo "$BENCH_OUTPUT" | tail -30
    exit 2
}

echo "$BENCH_OUTPUT"
echo ""

# ============================================================
# 解析输出并断言阈值
# ============================================================

echo "=== 阈值断言 ==="

FAILURES=0
CHECKS=0

# 解析 criterion 输出格式(多行格式:name 和 time: 在不同行):
#   benchmark_name/subpath
#                           time:   [88.123 µs 95.456 µs 105.789 µs]
#
# WHY 多行解析:criterion 对长 benchmark 名会换行,name 和 time: 分别在不同行。
# 旧版假设同行(awk '{print $1}' 提取 time: 而非 name),导致匹配失败。
# 修复:遍历所有行,记录最近的非缩进行作为 benchmark name,遇到 time: 行时关联。
#
# 阈值匹配:按阈值表降序排列(已在 THRESHOLDS 声明时排序),首次匹配即 break,
# 避免短前缀(如 '.../100')误匹配长名(如 '.../1000')。

CURRENT_BENCH_NAME=""

while IFS= read -r line; do
    # 跳过空行
    [[ -z "${line// }" ]] && continue

    # 检查是否是 benchmark name 行(不以空格/Tab 开头,且不含 time:/change:/Found 等关键词)
    if [[ ! "$line" =~ ^[[:space:]] ]] && \
       [[ "$line" != *"time:"* ]] && \
       [[ "$line" != *"change:"* ]] && \
       [[ "$line" != *"Performance"* ]] && \
       [[ "$line" != *"Found"* ]] && \
       [[ "$line" != *"setting"* ]] && \
       [[ "$line" != *"Benchmarking"* ]] && \
       [[ "$line" != *"Gnuplot"* ]] && \
       [[ "$line" != *"Running"* ]] && \
       [[ "$line" != *"Compiling"* ]] && \
       [[ "$line" != *"Finished"* ]] && \
       [[ ! "$line" =~ ^\[ ]]; then
        CURRENT_BENCH_NAME="$(echo "$line" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
        continue
    fi

    # 检查是否是 time: 行
    if [[ "$line" == *"time:"* ]] && [[ -n "$CURRENT_BENCH_NAME" ]]; then
        bench_name="$CURRENT_BENCH_NAME"

        # 按降序遍历阈值表,首次匹配即使用(避免短前缀误匹配)
        matched_threshold=""
        matched_name_regex=""
        for entry in "${THRESHOLDS[@]}"; do
            entry_threshold="${entry##*=}"
            entry_regex="${entry%=*}"
            if [[ "$bench_name" =~ $entry_regex ]]; then
                matched_threshold="$entry_threshold"
                matched_name_regex="$entry_regex"
                break
            fi
        done

        [[ -z "$matched_threshold" ]] && continue

        # 提取时间值:criterion 输出 `time:   [low mean high]`
        time_part=$(echo "$line" | sed -n 's/.*time:\s*\[\(.*\)\]/\1/p')
        if [[ -z "$time_part" ]]; then
            echo -e "${YELLOW}[WARN] 无法解析 $bench_name 的时间值,跳过${NC}"
            continue
        fi

        # 提取 mean(三个值中的第二个)和单位
        mean_str=$(echo "$time_part" | awk '{print $2}')
        unit=$(echo "$time_part" | awk '{print $3}')

        # 转换为毫秒
        case "$unit" in
            ns)   mean_ms=$(echo "$mean_str" | awk '{printf "%.9f", $1 / 1000000}') ;;
            µs|us) mean_ms=$(echo "$mean_str" | awk '{printf "%.6f", $1 / 1000}') ;;
            ms)   mean_ms="$mean_str" ;;
            s)    mean_ms=$(echo "$mean_str" | awk '{printf "%.3f", $1 * 1000}') ;;
            *)    echo -e "${YELLOW}[WARN] 未知单位 '$unit' for $bench_name,跳过${NC}"; continue ;;
        esac

        # 断言
        CHECKS=$((CHECKS + 1))
        passed=$(awk -v m="$mean_ms" -v t="$matched_threshold" 'BEGIN { print (m < t) ? 1 : 0 }')

        if [[ "$passed" == "1" ]]; then
            echo -e "${GREEN}[PASS]${NC} $bench_name: ${mean_ms} ms < ${matched_threshold} ms"
        else
            echo -e "${RED}[FAIL]${NC} $bench_name: ${mean_ms} ms >= ${matched_threshold} ms"
            FAILURES=$((FAILURES + 1))
        fi
    fi
done <<< "$BENCH_OUTPUT"

# ============================================================
# 汇总
# ============================================================

echo ""
echo "=== 汇总 ==="
echo "检查数: $CHECKS"
echo "通过:   $((CHECKS - FAILURES))"
echo "失败:   $FAILURES"

if [[ $FAILURES -gt 0 ]]; then
    echo ""
    echo -e "${RED}[FAIL] ${FAILURES} 个 benchmark 超过阈值,请检查性能退化${NC}"
    exit 1
elif [[ $CHECKS -eq 0 ]]; then
    echo -e "${YELLOW}[WARN] 未匹配到任何 benchmark,请检查 benchmark 名称${NC}"
    exit 2
else
    echo ""
    echo -e "${GREEN}[PASS] 全部 ${CHECKS} 个 benchmark 通过阈值断言${NC}"
    exit 0
fi
