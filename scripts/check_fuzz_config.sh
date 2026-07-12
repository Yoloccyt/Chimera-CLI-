#!/usr/bin/env bash
# =============================================================================
# fuzz crate 配置静态验证脚本(Linux/macOS Bash 版)
#
# WHY 此脚本存在:与 check_fuzz_config.ps1 对称,为 Linux CI 提供相同的
# 配置完整性验证。在 Linux CI 上通常直接运行 cargo +nightly fuzz run,
# 此脚本作为可选的预检步骤(如在 fuzz.yml 中添加为 pre-check job)。
#
# 验证项与 .ps1 版完全相同:
# 1. fuzz/Cargo.toml 存在且可被 cargo 解析
# 2. [package.metadata] cargo-fuzz = true
# 3. [lib] path 声明存在(承载 stub 宏)
# 4. 8 个 [[bin]] 声明存在(6 生产 + 2 stub 宏测试),每个 bin 的 path 指向的文件存在
# 5. 每个 fuzz target 文件包含 fuzz_target! 宏调用
# 6. 被测 crate 的 path 依赖目录存在
#
# 退出码:0 = 全部通过,1 = 有失败项
# =============================================================================

set -euo pipefail

# 定位仓库根目录(脚本在 scripts/ 下)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FUZZ_DIR="$REPO_ROOT/fuzz"
FUZZ_CARGO="$FUZZ_DIR/Cargo.toml"
FUZZ_TARGETS_DIR="$FUZZ_DIR/fuzz_targets"

FAILURES=0

pass() {
    echo "  [PASS] $1"
}

fail() {
    echo "  [FAIL] $1" >&2
    FAILURES=$((FAILURES + 1))
}

echo "=== fuzz crate 配置静态验证 ==="
echo ""

# ---------------------------------------------------------------------------
# 检查 1: fuzz/Cargo.toml 存在
# ---------------------------------------------------------------------------
echo "[1/6] 检查 fuzz/Cargo.toml 存在性"
if [[ -f "$FUZZ_CARGO" ]]; then
    pass "fuzz/Cargo.toml 存在"
else
    fail "fuzz/Cargo.toml 不存在: $FUZZ_CARGO"
    echo ""
    echo "验证失败: $FAILURES 项"
    exit 1
fi

# ---------------------------------------------------------------------------
# 检查 2: cargo-fuzz metadata
# ---------------------------------------------------------------------------
echo "[2/6] 检查 [package.metadata] cargo-fuzz = true"
if grep -qE 'cargo-fuzz\s*=\s*true' "$FUZZ_CARGO"; then
    pass "cargo-fuzz metadata 已声明"
else
    fail "未找到 cargo-fuzz = true metadata(cargo-fuzz 0.13+ 要求)"
fi

# ---------------------------------------------------------------------------
# 检查 3: [lib] path 声明(承载 Windows-GNU stub 宏)
# ---------------------------------------------------------------------------
echo "[3/6] 检查 [lib] path 声明"
if grep -qE '\[lib\]' "$FUZZ_CARGO" && grep -qE 'path\s*=\s*"src/lib\.rs"' "$FUZZ_CARGO"; then
    pass "[lib] path = src/lib.rs 已声明"
else
    fail "未找到 [lib] path = src/lib.rs(Windows-GNU stub 宏载体)"
fi

LIB_RS="$FUZZ_DIR/src/lib.rs"
if [[ -f "$LIB_RS" ]]; then
    pass "src/lib.rs 文件存在"
else
    fail "src/lib.rs 文件不存在: $LIB_RS"
fi

# ---------------------------------------------------------------------------
# 检查 4: 8 个 [[bin]] 声明 + target 文件存在性(6 生产 + 2 stub 宏测试)
# ---------------------------------------------------------------------------
echo "[4/6] 检查 8 个 [[bin]] 声明与 target 文件(6 生产 + 2 stub 宏测试)"

EXPECTED_TARGETS=(
    "quest_parse:quest_parse.rs"
    "seccore_sandbox:seccore_sandbox.rs"
    "event_serialize:event_serialize.rs"
    "cacr_budget_parse:cacr_budget_parse.rs"
    "checkpoint_deserialize:checkpoint_deserialize.rs"
    "config_section_parse:config_section_parse.rs"
    "stub_form1_test:stub_form1_test.rs"
    "stub_form3_test:stub_form3_test.rs"
)

for entry in "${EXPECTED_TARGETS[@]}"; do
    name="${entry%%:*}"
    file="${entry##*:}"
    file_path="$FUZZ_TARGETS_DIR/$file"

    if grep -qE "name\s*=\s*\"$name\"" "$FUZZ_CARGO"; then
        if [[ -f "$file_path" ]]; then
            pass "[$name] bin 声明 + 文件存在"
        else
            fail "[$name] bin 声明存在,但文件不存在: $file_path"
        fi
    else
        fail "[$name] 未在 Cargo.toml 中找到 [[bin]] name = \"$name\""
    fi
done

# ---------------------------------------------------------------------------
# 检查 5: 每个 fuzz target 包含 fuzz_target! 宏调用
# ---------------------------------------------------------------------------
echo "[5/6] 检查 fuzz target 文件包含 fuzz_target! 宏调用"

for entry in "${EXPECTED_TARGETS[@]}"; do
    name="${entry%%:*}"
    file="${entry##*:}"
    file_path="$FUZZ_TARGETS_DIR/$file"

    if [[ -f "$file_path" ]]; then
        if grep -qE 'fuzz_target!\s*\(' "$file_path"; then
            pass "[$name] 包含 fuzz_target! 宏调用"
        else
            fail "[$name] 未找到 fuzz_target! 宏调用"
        fi
    fi
done

# ---------------------------------------------------------------------------
# 检查 6: 被测 crate path 依赖目录存在
# ---------------------------------------------------------------------------
echo "[6/6] 检查被测 crate path 依赖目录存在"

EXPECTED_DEPS=(
    "nexus-core"
    "event-bus"
    "seccore"
    "model-router"
)

for dep in "${EXPECTED_DEPS[@]}"; do
    dep_cargo="$REPO_ROOT/crates/$dep/Cargo.toml"
    if [[ -f "$dep_cargo" ]]; then
        pass "[$dep] path 依赖目录存在"
    else
        fail "[$dep] path 依赖目录不存在: $REPO_ROOT/crates/$dep"
    fi
done

# ---------------------------------------------------------------------------
# 汇总
# ---------------------------------------------------------------------------
echo ""
if [[ $FAILURES -eq 0 ]]; then
    echo "=== 验证通过: 所有检查项 PASS ==="
    exit 0
else
    echo "=== 验证失败: $FAILURES 项 FAIL ===" >&2
    exit 1
fi
