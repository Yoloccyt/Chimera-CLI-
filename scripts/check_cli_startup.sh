#!/usr/bin/env bash
# ============================================================
# Chimera CLI 启动延迟基线检查脚本（Linux/macOS/CI 版, 与 .ps1 等价）
#
# 用法:
#   ./scripts/check_cli_startup.sh                 # 默认: runs 10, 阈值 100ms
#   ./scripts/check_cli_startup.sh -r 20           # 指定轮数(基线建立建议 20)
#   ./scripts/check_cli_startup.sh -t 150          # 覆盖阈值(ms)
#   ./scripts/check_cli_startup.sh -s              # 仅测量,不断言(观测模式)
#
# WHY: 为 CLI 启动延迟建立跨平台机械基线(--version/--help/help 三命令)。
#      Windows 基线(2026-08-17, runs 20): median 25.9~27.9ms;
#      Linux 进程启动更快,同一阈值 100ms 余量更充足。
#      配套 Windows 版: scripts/check_cli_startup.ps1。
#
# 依赖: hyperfine (taiki-e/install-action tool: hyperfine 或 cargo binstall)
#        release binary (cargo build --release -p chimera-cli)
#        python3 或 python (解析 hyperfine JSON)
#
# 输出: docs/reports/cli-startup-baseline.json (hyperfine 原始数据, 覆盖更新)
# 退出码（本仓治理门共用方言, F40 起）:
#   0 = 通过   1 = 超阈值（真判定为红）   2 = 不可判定（用法错误 / 缺 binary /
#       缺 hyperfine / 缺 python / 基线 JSON 未生成或不可解析 ⇒ 判据根本没跑完）
#   ★ 旧版把上面五类环境/用法故障一律退 1，读起来像"启动延迟回归了"，
#     而 CI 只看到 rc 非 0 —— 修它的人会被派去看 perf，方向完全错。
# ============================================================
set -euo pipefail

RUNS=10
THRESHOLD_MS=100.0
SKIP_ASSERT=false

while getopts "r:t:sh" opt; do
  case "$opt" in
    r) RUNS="$OPTARG" ;;
    t) THRESHOLD_MS="$OPTARG" ;;
    s) SKIP_ASSERT=true ;;
    h) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "[UNDECIDABLE] 用法: $0 [-r runs] [-t threshold_ms] [-s] [-h]" >&2; exit 2 ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
# WHY 支持 CHIMERA_BIN 覆盖: Windows 本地验证时产物为 chimera.exe(Linux/macOS 为 chimera)
BIN_PATH="${CHIMERA_BIN:-$PROJECT_ROOT/target/release/chimera}"
OUT_JSON="$PROJECT_ROOT/docs/reports/cli-startup-baseline.json"

# ---- 前置检查: binary / hyperfine / python ----
# 这三条都是"判据没能跑起来"，不是"启动延迟回归了" ⇒ 必须退 2（F40）。
if [ ! -f "$BIN_PATH" ]; then
  echo "[UNDECIDABLE] release binary 不存在: $BIN_PATH"
  echo "  请先执行: cargo build --release -p chimera-cli"
  exit 2
fi
if ! command -v hyperfine >/dev/null 2>&1; then
  echo "[UNDECIDABLE] hyperfine 未安装,请执行: cargo binstall hyperfine"
  exit 2
fi
PYTHON=""
for p in python3 python; do
  if command -v "$p" >/dev/null 2>&1; then PYTHON="$p"; break; fi
done
if [ -z "$PYTHON" ]; then
  echo "[UNDECIDABLE] python3/python 未安装(解析 hyperfine JSON 需要)"
  exit 2
fi

echo "[INFO] 测量 CLI 启动延迟 (runs=$RUNS, 阈值=$THRESHOLD_MS ms)"
echo "[INFO] binary: $BIN_PATH ($(du -h "$BIN_PATH" | cut -f1))"

# ---- 测量 (命令字符串带引号包裹路径, cmd/sh 通用) ----
# WHY 引号包裹: hyperfine 在 Windows 内部走 cmd.exe,未引号路径含空格会解析失败;
#      Linux/macOS 的 sh 解析引号路径同样正确,故统一加引号。
hyperfine --warmup 3 --runs "$RUNS" --export-json "$OUT_JSON" \
  -n 'version'  "\"$BIN_PATH\" --version" \
  -n 'help'     "\"$BIN_PATH\" --help" \
  -n 'help-cmd' "\"$BIN_PATH\" help" >/dev/null 2>&1 || true
# hyperfine 对单个命令失败返回非零,统一由下方断言处理

if [ ! -f "$OUT_JSON" ]; then
  echo "[UNDECIDABLE] 未生成基线 JSON: $OUT_JSON"
  echo "  hyperfine 未能产出测量数据（测量没跑成 ≠ 启动延迟回归）"
  exit 2
fi

# ---- 解析结果并断言 (median 为准, 对离群点稳健) ----
FAILED=false
echo ""
echo "=== CLI 启动延迟基线 ==="
# ★ 捕获必须写成 `|| PY_RC=$?`：`set -e` 下"命令; PY_RC=$?"的两行式在命令失败时**当场退出**，
#   下一行永远拿不到值 ⇒ 下面的 [FAIL]/[SKIP] 分支全是死代码， regressions 只留一个裸 rc=1
#   （本仓 3 个 .sh 共 14 处 `$?` 捕获点，其余 13 处一律守卫式 `... )" && rc=0 || rc=$?`，
#     只有这里漏了守卫；实测计数见报告 §72 的表）。
#   python 侧另用 exit 3 区分"读不懂数据"（⇒ 退 2 不可判定）与"超阈值"（⇒ 退 1 真红）。
PY_RC=0
"$PYTHON" - "$OUT_JSON" "$THRESHOLD_MS" <<'PYEOF' || PY_RC=$?
import json, sys
out_json, threshold = sys.argv[1], float(sys.argv[2])
try:
    with open(out_json, encoding='utf-8') as f:
        data = json.load(f)
    results = data['results']
except Exception as exc:                      # 读不到/解析不了 = 判据没数据，不是性能红
    print("  [UNDECIDABLE] 基线 JSON 不可用: %s: %s" % (type(exc).__name__, exc))
    sys.exit(3)
failed = False
for r in results:
    median_ms = r['median'] * 1000
    mean_ms = r['mean'] * 1000
    ok = median_ms < threshold
    if not ok:
        failed = True
    cmd = r['command'].split('/')[-1].replace('chimera', 'chimera')
    status = 'OK ' if ok else 'FAIL'
    print(f"  [{status}] {cmd}  median={median_ms:.2f} ms  mean={mean_ms:.2f} ms  (阈值 < {threshold:.0f} ms)")
sys.exit(1 if failed else 0)
PYEOF

if [ "$PY_RC" -eq 3 ]; then
  echo "[UNDECIDABLE] 无法解析测量结果 —— 判据未给出结论" >&2
  exit 2
fi
if [ "$SKIP_ASSERT" = "true" ]; then
  echo ""
  echo "[SKIP] 跳过断言 (观测模式)"
  exit 0
fi
if [ "$PY_RC" -ne 0 ]; then
  echo ""
  echo "[FAIL] 启动延迟超阈值,请检查: ① 是否在静默态测量; ② 是否引入启动路径回归"
  exit 1
fi
echo ""
echo "[OK] 启动延迟基线检查通过 (阈值 < $THRESHOLD_MS ms)"
exit 0
