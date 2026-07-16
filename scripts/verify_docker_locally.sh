#!/usr/bin/env bash
# ============================================================
# Chimera CLI 本地 Docker 镜像验证脚本(Linux/macOS / Bash)
#
# 三级降级验证:
# 1. Docker 可用 → 构建镜像 + 验证 --version + 检查体积 < 100MB
# 2. Podman 可用 → 构建镜像 + 验证 --version
# 3. 两者都不可用 → Dockerfile 静态检查 + release binary 体积 < 50MB 代理指标
#
# 完整镜像验证由 release.yml docker job 在 tag 推送时自动执行。
# 本脚本用于本地发布前检查清单第 10-12 项的降级验证。
#
# 退出码: 0=验证通过, 1=验证失败
# 使用方式: bash scripts/verify_docker_locally.sh
# ============================================================

set -euo pipefail

IMAGE_TAG="${1:-chimera-cli:local}"
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FAIL_COUNT=0

# 颜色输出(非交互终端禁用颜色)
if [ -t 1 ]; then
    GREEN='\033[0;32m'; RED='\033[0;31m'; CYAN='\033[0;36m'; GRAY='\033[0;90m'; YELLOW='\033[0;33m'; NC='\033[0m'
else
    GREEN=''; RED=''; CYAN=''; GRAY=''; YELLOW=''; NC=''
fi

step()     { echo -e "\n${CYAN}[STEP]${NC} $1"; }
pass()     { echo -e "  ${GREEN}[PASS]${NC} $1"; }
fail()     { echo -e "  ${RED}[FAIL]${NC} $1"; FAIL_COUNT=$((FAIL_COUNT + 1)); }
info_msg() { echo -e "  ${GRAY}[INFO]${NC} $1"; }

echo -e "\n${CYAN}=== Chimera CLI Docker Local Verification ===${NC}"

# --- 检测可用容器运行时 ---
RUNTIME=""
if command -v docker &>/dev/null; then
    RUNTIME="docker"
    info_msg "检测到 Docker: $(docker --version)"
elif command -v podman &>/dev/null; then
    RUNTIME="podman"
    info_msg "检测到 Podman: $(podman --version)"
fi

if [ -z "$RUNTIME" ]; then
    echo -e "\n${YELLOW}[降级模式]${NC} Docker / Podman 均不可用,执行静态检查"

    # --- 降级验证: Dockerfile 静态检查 ---
    step 'Dockerfile 静态检查'

    DOCKERFILE="$PROJECT_ROOT/Dockerfile"
    if [ -f "$DOCKERFILE" ]; then
        CONTENT=$(cat "$DOCKERFILE")
        pass 'Dockerfile 存在'

        # 检查关键配置
        echo "$CONTENT" | grep -qE 'FROM\s+rust:1-slim-bookworm'    && pass 'FROM rust:1-slim-bookworm builder'    || fail 'FROM rust:1-slim-bookworm builder'
        echo "$CONTENT" | grep -qE 'FROM\s+gcr\.io/distroless/cc-debian12' && pass 'FROM distroless runtime'           || fail 'FROM distroless runtime'
        echo "$CONTENT" | grep -qE 'USER\s+nonroot'                   && pass 'USER nonroot'                          || fail 'USER nonroot'
        echo "$CONTENT" | grep -q 'HEALTHCHECK'                       && pass 'HEALTHCHECK 声明'                      || fail 'HEALTHCHECK 声明'
        echo "$CONTENT" | grep -qE 'ENTRYPOINT\s+\["chimera"\]'      && pass 'ENTRYPOINT ["chimera"]'               || fail 'ENTRYPOINT ["chimera"]'
        echo "$CONTENT" | grep -q 'RUST_BACKTRACE=1'                 && pass 'RUST_BACKTRACE=1'                      || fail 'RUST_BACKTRACE=1'
    else
        fail 'Dockerfile 存在'
    fi

    # --- 降级验证: release binary 体积 ---
    step 'Release binary 体积检查 (< 50MB 代理指标)'

    RELEASE_BIN="$PROJECT_ROOT/target/release/chimera"
    if [ -f "$RELEASE_BIN" ]; then
        SIZE_BYTES=$(stat -c%s "$RELEASE_BIN" 2>/dev/null || stat -f%z "$RELEASE_BIN" 2>/dev/null || echo 0)
        SIZE_MB=$(echo "scale=2; $SIZE_BYTES / 1048576" | bc)
        if [ "$SIZE_BYTES" -lt 52428800 ]; then
            pass "Release binary 体积: ${SIZE_MB}MB < 50MB"
        else
            fail "Release binary 体积: ${SIZE_MB}MB >= 50MB"
        fi
    else
        info_msg "Release binary 不存在(未运行 cargo build --release),跳过体积检查"
        info_msg "完整镜像验证由 release.yml docker job 在 tag 推送时自动执行"
    fi

    # --- 降级验证: CI 状态引导 ---
    step 'CI 状态查询引导'
    info_msg '完整 Docker 镜像验证(.github/workflows/release.yml docker job)在 tag 推送时自动执行'
    info_msg '查询最近 CI 运行状态:'
    info_msg '  gh run list --workflow=release.yml --limit=1'
    info_msg '  gh run view <run-id> --log --job=docker'
else
    # --- 容器运行时可用: 完整验证 ---
    step "使用 $RUNTIME 构建镜像 ($IMAGE_TAG)"

    if "$RUNTIME" build -t "$IMAGE_TAG" "$PROJECT_ROOT"; then
        pass "镜像构建成功: $IMAGE_TAG"
    else
        fail "镜像构建失败"
        echo -e "\n${CYAN}=== 验证结果: $FAIL_COUNT 项失败 ===${NC}"
        exit 1
    fi

    # --- 验证 --version ---
    step '验证 --version 输出'
    VERSION_OUTPUT=$("$RUNTIME" run --rm "$IMAGE_TAG" --version 2>&1) || true
    if echo "$VERSION_OUTPUT" | grep -qE '^(aether|chimera)\s+[0-9]+\.[0-9]+\.[0-9]+'; then
        pass "--version 输出匹配: $VERSION_OUTPUT"
    else
        fail "--version 输出不匹配: $VERSION_OUTPUT"
    fi

    # --- 检查镜像体积 ---
    step '检查镜像体积 (< 100MB)'

    SIZE_BYTES=$("$RUNTIME" image inspect "$IMAGE_TAG" --format '{{.Size}}' 2>/dev/null || echo 0)
    if [ "$SIZE_BYTES" -gt 0 ]; then
        SIZE_MB=$(echo "scale=2; $SIZE_BYTES / 1048576" | bc)
        if [ "$SIZE_BYTES" -lt 104857600 ]; then
            pass "镜像体积: ${SIZE_MB}MB < 100MB"
        else
            fail "镜像体积: ${SIZE_MB}MB >= 100MB"
        fi
    else
        fail '无法获取镜像体积'
    fi
fi

# --- 汇总 ---
echo -e "\n${CYAN}=== 验证结果 ===${NC}"
if [ "$FAIL_COUNT" -eq 0 ]; then
    echo -e "  ${GREEN}全部通过 (0 failures)${NC}"
    exit 0
else
    echo -e "  ${RED}$FAIL_COUNT 项失败${NC}"
    exit 1
fi
