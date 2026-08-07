#!/usr/bin/env bash
# =============================================================================
# bench_test_runtime.sh — P9-T2 测试运行时间基准采集与对比
# =============================================================================
# Purpose: 统一封装 nextest 三档 profile (ci-fast / full / stress) 测试运行,
#          产出 JSON 格式基准数据,支撑 P9-T2 优化前后对比。
# Scope:   P9-T2 优化实施,作为 docs/reports/p9-t2-test-runtime-optimization.md
#          数据源。
# Author:  P9-T2 implementation (test-runtime-optimization)
# Refs:    .trae/specs/p9-t2-test-runtime-optimization/{spec,tasks,checklist}.md
#
# Usage:
#   bash scripts/bench_test_runtime.sh fast      # CI fast 档(PR 反馈)
#   bash scripts/bench_test_runtime.sh full      # 全量(开发本地 / Release)
#   bash scripts/bench_test_runtime.sh stress    # 压测 + 1000-iter(夜间)
#   bash scripts/bench_test_runtime.sh all       # 三档全跑(写入 3 个 JSON)
#   bash scripts/bench_test_runtime.sh <mode> report <path>  # 汇总报告
#
# 模式说明:
#   fast   - 跑常规单元测试,使用 CHIMERA_TEST_TIMEOUT_SCALE=0.1,
#            排除根 package E2E/压测 binary。
#   full   - 全量(等同当前 release.yml 验收配置)。
#   stress - 仅 stress profile 目标(1000-iter 压测)。
#
# Environment:
#   CHIMERA_TEST_TIMEOUT_SCALE  - 等待缩放因子(0.01-1.0,默认 1.0)
#   CHIMERA_BACKPRESSURE_SECS   - event-bus 背压测试时长(默认 60s)
#   NEXTEST_PROFILE             - nextest profile(脚本自动设)
#   REPORT_DIR                  - JSON 报告输出目录(默认 docs/reports)
#
# Exit code:
#   0 = 成功(所有测试通过或被忽略)
#   1 = 有测试失败
#   2 = 脚本错误(参数错 / nextest 未装 / 报告解析失败)
# =============================================================================

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

REPORT_DIR="${REPORT_DIR:-${REPO_ROOT}/docs/reports}"
mkdir -p "${REPORT_DIR}"

MODE="${1:-fast}"
ACTION="${1:-fast}"
TARGET="${2:-${MODE}}"
JSON_PATH="${3:-}"

# =============================================================================
# 工具函数
# =============================================================================

now_ts() {
    date +%s
}

now_iso() {
    date -u +%Y-%m-%dT%H:%M:%SZ
}

# 解析 nextest JSON report 顶部耗时;若未生成返回 "N/A"
parse_total_elapsed() {
    local report_path="$1"
    if [[ ! -f "${report_path}" ]]; then
        echo "N/A"
        return
    fi
    # nextest JSON 含 "started-at" / "finished-at" 字段(ISO8601)
    python3 - "$report_path" <<'PY' 2>/dev/null || echo "N/A"
import json
import sys
from datetime import datetime
try:
    with open(sys.argv[1]) as f:
        data = json.load(f)
    started = data.get("started-at")
    finished = data.get("finished-at")
    if not (started and finished):
        print("N/A")
        sys.exit(0)
    s = datetime.fromisoformat(started.replace("Z", "+00:00"))
    f_ = datetime.fromisoformat(finished.replace("Z", "+00:00"))
    print(f"{(f_ - s).total_seconds():.2f}")
except Exception:
    print("N/A")
PY
}

# 解析测试用例耗时并按耗时降序排序,输出 TopN (binary::test elapsed_secs)
parse_top_tests() {
    local report_path="$1"
    local top_n="${2:-10}"
    if [[ ! -f "${report_path}" ]]; then
        echo "[]"
        return
    fi
    python3 - "$report_path" "$top_n" <<'PY' 2>/dev/null || echo "[]"
import json
import sys
# ultra-plan 2026-08-07:nextest libtest-json-plus 输出为 JSON Lines,
# 单测试行形如 {"type":"test","event":"ok","name":"...","exec_time":<secs>}
try:
    top_n = int(sys.argv[2])
    rows = []
    with open(sys.argv[1], encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.strip()
            if '"type":"test","event":"ok"' not in line:
                continue
            t = json.loads(line)
            elapsed = t.get("exec_time")
            if elapsed is None:
                continue
            name = t.get("name", "?")
            binary = name.split("$")[0]
            rows.append((binary, name, elapsed))
    rows.sort(key=lambda r: r[2], reverse=True)
    top = rows[:top_n]
    print(json.dumps([
        {"binary": b, "test": n, "elapsed_secs": round(e, 3)}
        for (b, n, e) in top
    ]))
except Exception:
    print("[]")
PY
}

# 写 metadata JSON(每档模式独立 .json)
write_metadata() {
    local mode="$1"
    local elapsed="$2"
    local top_json="$3"
    local timestamp; timestamp="$(now_iso)"
    local host; host="$(hostname 2>/dev/null || echo 'unknown')"
    local cpu_count; cpu_count="$(nproc 2>/dev/null || echo 1)"
    local rustc_version; rustc_version="$(rustc --version 2>/dev/null || echo unknown)"
    local nextest_version; nextest_version="$(cargo nextest --version 2>/dev/null | head -1 || echo unknown)"

    cat >"${REPORT_DIR}/p9-t2-baseline-${mode}.json" <<EOF
{
  "mode": "${mode}",
  "timestamp": "${timestamp}",
  "host": "${host}",
  "cpu_count": ${cpu_count},
  "rustc_version": "${rustc_version}",
  "nextest_version": "${nextest_version}",
  "chimera_test_timeout_scale": "${CHIMERA_TEST_TIMEOUT_SCALE:-1.0}",
  "chimera_backpressure_secs": "${CHIMERA_BACKPRESSURE_SECS:-60}",
  "total_elapsed_secs": ${elapsed},
  "top_tests": ${top_json}
}
EOF
    echo "  -> 写入 ${REPORT_DIR}/p9-t2-baseline-${mode}.json"
}

# =============================================================================
# nextest profile 检查
# =============================================================================

if ! command -v cargo-nextest >/dev/null 2>&1; then
    # 兜底:从 .config/nextest.toml 探测
    if [[ ! -f "${REPO_ROOT}/.config/nextest.toml" ]]; then
        echo "[ERROR] cargo-nextest 未安装且 .config/nextest.toml 不存在。" >&2
        echo "  安装: cargo install cargo-nextest --locked" >&2
        exit 2
    fi
fi

# =============================================================================
# 模式分派
# =============================================================================

run_mode() {
    local mode="$1"
    local profile=""
    local scale="1.0"
    local bp_secs="60"
    local extra_args=()
    local json_path=""

    case "${mode}" in
        fast)
            profile="ci-fast"
            scale="0.1"
            bp_secs="5"
            json_path="${REPORT_DIR}/nextest-fast.json"
            # P9-T2: ci-fast 档排除 stress test binary(走 stress profile 集中跑)
            extra_args=(-E 'not binary(/stress/)')
            ;;
        full)
            profile="default"
            scale="1.0"
            bp_secs="60"
            json_path="${REPORT_DIR}/nextest-full.json"
            extra_args=()
            ;;
        stress)
            profile="stress"
            scale="1.0"
            bp_secs="60"
            json_path="${REPORT_DIR}/nextest-stress.json"
            # P9-T2: stress 档仅跑 stress test binary
            # ultra-plan:补 --run-ignored all —— 1000-iter 压测标了 #[ignore],
            # 缺省不包含则压测核心全部被跳过(与 stress.yml 对齐;
            # nextest 语法为 --run-ignored,非 cargo test 的 --include-ignored)。
            extra_args=(-E 'binary(/stress/)' --run-ignored all)
            ;;
        *)
            echo "[ERROR] 未知模式: ${mode} (fast|full|stress)" >&2
            exit 2
            ;;
    esac

    echo "=== 模式: ${mode} | profile: ${profile} | scale: ${scale} | bp: ${bp_secs}s ==="
    echo "    输出: ${json_path}"

    # ultra-plan 2026-08-07 修复:--message-format json 非法(nextest 0.9.x 支持
    # human/libtest-json/libtest-json-plus);libtest-json 为实验特性需 env 开启;
    # --exclude 需搭配 --workspace。
    local start_ts; start_ts="$(now_ts)"

    CHIMERA_TEST_TIMEOUT_SCALE="${scale}" \
    CHIMERA_BACKPRESSURE_SECS="${bp_secs}" \
    NEXTEST_EXPERIMENTAL_LIBTEST_JSON=1 \
    cargo nextest run \
        --workspace \
        --profile "${profile}" \
        --no-fail-fast \
        --message-format libtest-json-plus \
        "${extra_args[@]}" \
        2>"${REPORT_DIR}/nextest-${mode}.stderr.log" \
        | tee "${REPORT_DIR}/nextest-${mode}.stdout.log" \
        > "${json_path}" || true

    local end_ts; end_ts="$(now_ts)"
    local wall_secs=$((end_ts - start_ts))

    # 优先从 JSON 解析;失败则用 wall time 兜底
    local elapsed
    elapsed="$(parse_total_elapsed "${json_path}")"
    if [[ "${elapsed}" == "N/A" ]]; then
        elapsed="${wall_secs}.00"
    fi

    local top_json
    top_json="$(parse_top_tests "${json_path}" 10)"

    echo "  -> 耗时: ${elapsed}s (wall: ${wall_secs}s)"
    write_metadata "${mode}" "${elapsed}" "${top_json}"
}

# =============================================================================
# 报告汇总
# =============================================================================

gen_report() {
    local source_path="$1"
    if [[ ! -f "${source_path}" ]]; then
        echo "[ERROR] 元数据文件不存在: ${source_path}" >&2
        exit 2
    fi
    echo "=== 汇总报告 ==="
    cat "${source_path}" | python3 -m json.tool
}

# =============================================================================
# 入口
# =============================================================================

if [[ "${ACTION}" == "report" ]]; then
    if [[ -z "${TARGET}" ]]; then
        echo "[ERROR] report 模式需指定 metadata JSON 路径" >&2
        exit 2
    fi
    gen_report "${TARGET}"
    exit $?
fi

case "${ACTION}" in
    fast|full|stress)
        run_mode "${ACTION}"
        ;;
    all)
        run_mode fast
        run_mode full
        run_mode stress
        ;;
    *)
        echo "用法: $0 {fast|full|stress|all|report <path>}" >&2
        exit 2
        ;;
esac
