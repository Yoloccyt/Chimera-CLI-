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
# 退出码: 0 = 通过 / 1 = 超阈值或错误
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
    *) echo "用法: $0 [-r runs] [-t threshold_ms] [-s]" >&2; exit 1 ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
# WHY 支持 CHIMERA_BIN 覆盖: Windows 本地验证时产物为 chimera.exe(Linux/macOS 为 chimera)
BIN_PATH="${CHIMERA_BIN:-$PROJECT_ROOT/target/release/chimera}"
OUT_JSON="$PROJECT_ROOT/docs/reports/cli-startup-baseline.json"

# ---- 前置检查: binary / hyperfine / python ----
if [ ! -f "$BIN_PATH" ]; then
  echo "[ERROR] release binary 不存在: $BIN_PATH"
  echo "  请先执行: cargo build --release -p chimera-cli"
  exit 1
fi
if ! command -v hyperfine >/dev/null 2>&1; then
  echo "[ERROR] hyperfine 未安装,请执行: cargo binstall hyperfine"
  exit 1
fi
PYTHON=""
for p in python3 python; do
  if command -v "$p" >/dev/null 2>&1; then PYTHON="$p"; break; fi
done
if [ -z "$PYTHON" ]; then
  echo "[ERROR] python3/python 未安装(解析 hyperfine JSON 需要)"
  exit 1
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
  echo "[ERROR] 未生成基线 JSON: $OUT_JSON"
  exit 1
fi

# ---- 解析结果并断言 (median 为准, 对离群点稳健) ----
FAILED=false
echo ""
echo "=== CLI 启动延迟基线 ==="
"$PYTHON" - "$OUT_JSON" "$THRESHOLD_MS" <<'PYEOF'
import json, sys
out_json, threshold = sys.argv[1], float(sys.argv[2])
with open(out_json, encoding='utf-8') as f:
    data = json.load(f)
failed = False
for r in data['results']:
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
PY_RC=$?

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
