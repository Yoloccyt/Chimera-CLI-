#!/usr/bin/env bash
# ============================================================
# Chimera CLI fuzz 配置静态验证脚本(Linux/macOS / Bash)
#
# 静态验证 fuzz/Cargo.toml 配置完整性,无需 nightly 工具链或实际运行 fuzz。
# 检查项:
# 1. fuzz/Cargo.toml 存在且 [package.metadata] cargo-fuzz = true
# 2. fuzz/fuzz_targets/ 目录下的 .rs 文件数量与 [[bin]] 声明一致
# 3. 每个 [[bin]] 的 path 指向实际存在的文件
# 4. fuzz/src/lib.rs stub 宏存在(Windows-GNU 兼容方案)
# 5. fuzz/Cargo.toml 有 [target.'cfg(not(windows))'.dependencies] 条目
#
# 退出码: 0=全部通过, 1=有检查项失败
# 使用方式: bash scripts/check_fuzz_config.sh
# ============================================================

set -euo pipefail

FUZZ_DIR="$(cd "$(dirname "$0")/.." && pwd)/fuzz"
FUZZ_TOML="$FUZZ_DIR/Cargo.toml"
FAIL_COUNT=0

# 颜色输出(非交互终端禁用颜色)
if [ -t 1 ]; then
    GREEN='\033[0;32m'
    RED='\033[0;31m'
    CYAN='\033[0;36m'
    GRAY='\033[0;90m'
    NC='\033[0m'
else
    GREEN=''; RED=''; CYAN=''; GRAY=''; NC=''
fi

echo -e "\n${CYAN}=== Chimera CLI Fuzz Config Static Check ===${NC}"

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

# --- 检查 1: fuzz/Cargo.toml 存在 ---
if [ ! -f "$FUZZ_TOML" ]; then
    echo -e "  ${RED}[FAIL]${NC} fuzz/Cargo.toml 不存在"
    echo -e "\n  无法继续检查" >&2
    exit 1
fi
check 'fuzz/Cargo.toml 存在' true

TOML_CONTENT=$(cat "$FUZZ_TOML")

# --- 检查 2: cargo-fuzz metadata ---
check '[package.metadata] cargo-fuzz = true' \
    "$(echo "$TOML_CONTENT" | grep -q 'cargo-fuzz.*=.*true' && echo true || echo false)"

# --- 检查 3: [lib] stub 宏声明 ---
check '[lib] chimera_fuzz stub 宏声明' \
    "$(echo "$TOML_CONTENT" | grep -q '\[lib\]' && echo "$TOML_CONTENT" | grep -q 'chimera_fuzz' && echo true || echo false)"

# --- 检查 4: target-specific 依赖 ---
check "target.'cfg(not(windows))'.dependencies 条目" \
    "$(echo "$TOML_CONTENT" | grep -q "cfg(not(windows))" && echo true || echo false)"

# --- 检查 5: fuzz_targets/ 文件数 vs [[bin]] 声明数 ---
TARGETS_DIR="$FUZZ_DIR/fuzz_targets"
RS_COUNT=$(find "$TARGETS_DIR" -name '*.rs' -type f 2>/dev/null | wc -l)
BIN_COUNT=$(echo "$TOML_CONTENT" | grep -c '\[\[bin\]\]')

check "fuzz_targets/ .rs 文件数 ($RS_COUNT) = [[bin]] 声明数 ($BIN_COUNT)" \
    "$([ "$RS_COUNT" -eq "$BIN_COUNT" ] && echo true || echo false)"

# --- 检查 6: 每个 [[bin]] path 指向实际存在的文件 ---
for f in "$TARGETS_DIR"/*.rs; do
    [ -f "$f" ] || continue
    bin_name=$(basename "$f" .rs)
    check "  [[bin]] $bin_name 声明存在" \
        "$(echo "$TOML_CONTENT" | grep -q "name.*=.*\"$bin_name\"" && echo true || echo false)"
done

# --- 检查 7: fuzz/src/lib.rs 存在 ---
check 'fuzz/src/lib.rs stub 宏文件存在' \
    "$([ -f "$FUZZ_DIR/src/lib.rs" ] && echo true || echo false)"

# --- 检查 8: 每个 fuzz_target 文件有条件编译 import ---
ALL_COND=true
for f in "$TARGETS_DIR"/*.rs; do
    [ -f "$f" ] || continue
    if ! grep -q '#\[cfg(not(windows))\]' "$f" || ! grep -q '#\[cfg(windows)\]' "$f"; then
        ALL_COND=false
        check "  $(basename "$f") 条件编译 import" false
    fi
done
[ "$ALL_COND" = true ] && check '所有 fuzz target 文件有条件编译 import' true

# --- 汇总 ---
echo -e "\n${CYAN}=== 检查结果 ===${NC}"
if [ "$FAIL_COUNT" -eq 0 ]; then
    echo -e "  ${GREEN}全部通过 (0 failures)${NC}"
    exit 0
else
    echo -e "  ${RED}$FAIL_COUNT 项检查失败${NC}"
    exit 1
fi
