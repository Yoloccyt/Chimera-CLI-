#!/usr/bin/env bash
# ============================================================
# Chimera CLI 本地 Docker 验证替代脚本 (Linux / macOS Bash)
#
# 功能: 发布前检查清单 (CLAUDE.md S5 第 10-12 项 / nuxus规则.md S7.2 第 7 项)
#       要求 Docker 镜像验证。本脚本提供三级降级策略:
#         1. Docker 可用  -> 完整镜像构建验证 (与 CI release.yml docker job 等价)
#         2. Podman 可用  -> Podman 构建验证 (Podman 兼容 Docker CLI, 无许可证限制)
#         3. 均不可用     -> 降级验证:
#              a) Dockerfile 静态验证 (关键指令/基础镜像/安全配置存在性检查)
#              b) Release binary 验证 (--version 格式 + 体积 < 50MB, 镜像体积代理指标)
#              c) CI Docker 验证状态查询命令引导
#
# 用法:
#   ./scripts/verify_docker_locally.sh
#   ./scripts/verify_docker_locally.sh --skip-build
#   ./scripts/verify_docker_locally.sh --help
# ============================================================
set -euo pipefail

# ============================================================
# 全局配置
# ============================================================

# 仓库根目录 (脚本位于 scripts/ 下, 根目录是上一级)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DOCKERFILE="$REPO_ROOT/Dockerfile"
BINARY_PATH="$REPO_ROOT/target/release/aether"
IMAGE_NAME="chimera-cli:local-verify"

# 体积红线 (与 release.yml / nuxus规则.md S7.2 一致)
BINARY_SIZE_LIMIT=52428800   # 50MB
IMAGE_SIZE_LIMIT=104857600   # 100MB

# 验证结果计数
PASS_COUNT=0
FAIL_COUNT=0
WARNINGS=()

# 颜色输出 (非 TTY 时禁用, 避免日志文件中出现转义序列)
if [ -t 1 ]; then
    GREEN='\033[0;32m'; RED='\033[0;31m'; CYAN='\033[0;36m'
    YELLOW='\033[1;33m'; GRAY='\033[0;90m'; WHITE='\033[0;37m'; NC='\033[0m'
else
    GREEN=''; RED=''; CYAN=''; YELLOW=''; GRAY=''; WHITE=''; NC=''
fi

# ============================================================
# 辅助函数
# ============================================================

# 输出单项检查结果并累加计数
# 参数: $1=检查项名称  $2=0(pass)/1(fail)  $3=详情(可选)
print_check() {
    local name="$1" passed="$2" detail="${3:-}"
    if [ "$passed" -eq 0 ]; then
        printf "${GREEN}[OK]${NC}   %s\n" "$name"
        PASS_COUNT=$((PASS_COUNT + 1))
    else
        printf "${RED}[FAIL]${NC} %s\n" "$name"
        FAIL_COUNT=$((FAIL_COUNT + 1))
    fi
    if [ -n "$detail" ]; then
        printf "${GRAY}       %s${NC}\n" "$detail"
    fi
}

# 检测命令是否在 PATH 中可用
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# ============================================================
# 路径 1/2: Docker / Podman 完整镜像构建验证
# ============================================================

# 使用指定容器引擎 (docker 或 podman) 执行完整镜像验证。
# 验证项与 release.yml docker job 完全对齐:
#   - build 镜像
#   - run --version (grep ^(aether|chimera) X.Y.Z)
#   - image inspect --format {{.Size}} (< 100MB)
# 参数: $1 = "docker" 或 "podman"
# 返回: 0=全部通过, 1=任一失败
run_engine_verification() {
    local engine="$1"
    echo -e "${CYAN}=== 检测到 $engine, 执行完整镜像构建验证 ===${NC}"

    # --- 构建 ---
    echo "-> $engine build -t $IMAGE_NAME (可能需要数分钟)..."
    # tee 同时输出到终端; set -o pipefail 确保捕获 build 的退出码而非 tee 的
    if ! "$engine" build -t "$IMAGE_NAME" "$REPO_ROOT" 2>&1 | tail -5; then
        print_check "$engine build 成功" 1 "构建失败"
        return 1
    fi
    print_check "$engine build 成功" 0

    # --- --version 验证 ---
    # distroless 无 shell, 直接执行 binary; 输出必须匹配 aether|chimera X.Y.Z
    local version_output
    version_output=$("$engine" run --rm "$IMAGE_NAME" --version 2>&1) || true
    if echo "$version_output" | grep -qE '^(aether|chimera) [0-9]+\.[0-9]+\.[0-9]+'; then
        print_check "$engine run --version 格式校验" 0 "输出: $version_output"
    else
        print_check "$engine run --version 格式校验" 1 "输出: $version_output"
        return 1
    fi

    # --- 镜像体积验证 ---
    local size_raw image_size size_mb
    size_raw=$("$engine" image inspect "$IMAGE_NAME" --format '{{.Size}}' 2>/dev/null) || size_raw=""
    if [[ "$size_raw" =~ ^[0-9]+$ ]]; then
        image_size="$size_raw"
        size_mb=$(awk "BEGIN {printf \"%.2f\", $image_size / 1048576}")
        if [ "$image_size" -lt "$IMAGE_SIZE_LIMIT" ]; then
            print_check "镜像体积 < 100MB" 0 "实际: ${size_mb}MB ($image_size bytes)"
        else
            print_check "镜像体积 < 100MB" 1 "实际: ${size_mb}MB ($image_size bytes)"
            return 1
        fi
    else
        print_check "镜像体积 < 100MB" 1 "无法获取镜像体积: $size_raw"
        return 1
    fi

    # --- 清理临时镜像 ---
    "$engine" rmi "$IMAGE_NAME" --force >/dev/null 2>&1 || true
    return 0
}

# ============================================================
# 路径 3a: Dockerfile 静态验证
# ============================================================

# 检查 Dockerfile 中是否包含指定 pattern
# 参数: $1=正则 pattern  $2=检查项描述
check_pattern() {
    local pattern="$1" desc="$2"
    # `--` 确保 pattern 不被 grep 解析为选项 (如 --chown)
    if grep -qE -- "$pattern" "$DOCKERFILE"; then
        print_check "$desc" 0
    else
        print_check "$desc" 1
    fi
}

# 对 Dockerfile 执行静态结构检查, 确保镜像构建配方关键指令完整。
# 不执行实际构建, 仅验证文本层面的一致性。
run_dockerfile_check() {
    echo -e "${CYAN}=== 降级验证 1/3: Dockerfile 静态检查 ===${NC}"

    if [ ! -f "$DOCKERFILE" ]; then
        print_check "Dockerfile 存在" 1 "路径: $DOCKERFILE"
        return
    fi
    print_check "Dockerfile 存在" 0

    # 关键指令检查清单: 每项对应 Dockerfile 中的一条安全/功能约束
    # 缺失任一项意味着 Dockerfile 被意外篡改或降级, 需要人工排查
    check_pattern 'FROM rust:1\.82-slim AS builder'  "Builder 阶段 (rust:1.82-slim)"
    check_pattern 'FROM gcr\.io/distroless/cc-debian12' "Runtime 阶段 (distroless/cc-debian12)"
    check_pattern 'COPY --from=builder'              "多阶段 COPY --from=builder"
    check_pattern '--chown=nonroot:nonroot'          "文件归属 --chown=nonroot:nonroot"
    check_pattern 'USER nonroot:nonroot'             "USER nonroot:nonroot (最小权限)"
    check_pattern 'ENTRYPOINT \["chimera"\]'         "ENTRYPOINT exec form (无 shell)"
    check_pattern 'HEALTHCHECK'                      "HEALTHCHECK 定义"
    check_pattern 'ARG VERSION'                      "ARG VERSION (CI 版本注入)"
    check_pattern 'ENV RUST_BACKTRACE=1'             "ENV RUST_BACKTRACE=1 (panic 栈回溯)"
    check_pattern 'org\.opencontainers\.image\.title' "OCI LABEL (镜像元数据)"
}

# ============================================================
# 路径 3b: Release binary 验证
# ============================================================

# 验证 release binary 可执行性 + 体积。
# binary 体积 < 50MB 是镜像体积 < 100MB 的必要条件 (distroless 基础约 20MB + binary),
# 作为镜像体积的代理指标。
run_binary_check() {
    echo -e "${CYAN}=== 降级验证 2/3: Release binary 验证 ===${NC}"

    # 构建检查 (--skip-build 时跳过)
    if [ "$SKIP_BUILD" -eq 0 ]; then
        # WHY cargo 可用性预检:CI/容器环境可能未安装 Rust 工具链,
        # 直接调用 cargo 会触发 "command not found",开发者难以区分
        # 是环境问题还是构建失败。提前检测并给出明确指引。
        if ! command_exists cargo; then
            print_check "cargo build --release" 1 "cargo 不在 PATH 中"
            echo -e "${YELLOW}       提示: Rust 工具链未安装或不在 PATH 中。请运行:${NC}"
            echo -e "${WHITE}         curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh${NC}"
            echo -e "${YELLOW}       安装后重开终端或运行 source ~/.cargo/env${NC}"
            return
        fi
        echo "-> cargo build --workspace --release (可能需要数分钟)..."
        if ! cargo build --workspace --release 2>&1 | tail -3; then
            print_check "cargo build --release" 1 "构建失败"
            return
        fi
    fi

    # binary 存在性
    if [ ! -f "$BINARY_PATH" ]; then
        print_check "Binary 存在" 1 "路径: $BINARY_PATH (请先运行 cargo build --workspace --release)"
        return
    fi
    print_check "Binary 存在" 0

    # --version 执行 + 格式校验 (与 release.yml Verify binary runs 步骤对齐)
    local version_output
    version_output=$("$BINARY_PATH" --version 2>&1) || true
    if echo "$version_output" | grep -qE '^(aether|chimera) [0-9]+\.[0-9]+\.[0-9]+'; then
        print_check "binary --version 格式校验" 0 "输出: $version_output"
    else
        print_check "binary --version 格式校验" 1 "输出: $version_output"
    fi

    # 体积验证 (与 release.yml Verify binary size < 50MB 步骤对齐)
    # stat 跨平台: Linux 用 -c%s, macOS 用 -f%z, 兜底用 wc -c
    local binary_size size_mb
    binary_size=$(stat -c%s "$BINARY_PATH" 2>/dev/null || stat -f%z "$BINARY_PATH" 2>/dev/null || wc -c < "$BINARY_PATH" | tr -d ' ')
    size_mb=$(awk "BEGIN {printf \"%.2f\", $binary_size / 1048576}")
    if [ "$binary_size" -lt "$BINARY_SIZE_LIMIT" ]; then
        print_check "binary 体积 < 50MB" 0 "实际: ${size_mb}MB ($binary_size bytes)"
    else
        print_check "binary 体积 < 50MB" 1 "实际: ${size_mb}MB ($binary_size bytes)"
        echo -e "${YELLOW}       提示: binary 体积超限通常说明引入了重量级依赖或 strip/LTO 配置失效${NC}"
    fi
}

# ============================================================
# 路径 3c: CI Docker 验证状态查询引导
# ============================================================

# 本地无法构建镜像时, Docker 镜像的完整验证由 CI release.yml docker job 完成。
# 此函数输出 gh CLI 查询命令, 引导开发者确认 CI 验证状态。
show_ci_guidance() {
    echo -e "${CYAN}=== 降级验证 3/3: CI Docker 验证状态查询 ===${NC}"
    echo -e "${GRAY}本地无法构建镜像时, Docker 镜像的完整验证由 CI release.yml docker job 完成。${NC}"
    echo -e "${GRAY}推送 tag 后, 通过以下命令查询 CI 状态:${NC}"
    echo ""
    echo -e "${WHITE}  # 查看最近的 Release 工作流运行${NC}"
    echo -e "${WHITE}  gh run list --workflow=release.yml --limit 5${NC}"
    echo ""
    echo -e "${WHITE}  # 查看特定运行的详情 (含 docker job 状态)${NC}"
    echo -e "${WHITE}  gh run view --workflow=release.yml <run-id>${NC}"
    echo ""
    echo -e "${WHITE}  # 查看 docker job 日志 (镜像构建 + 体积验证 + --version 验证)${NC}"
    echo -e "${WHITE}  gh run view <run-id> --log --job=<job-id>${NC}"
    echo ""
    echo -e "${GRAY}  CI docker job 验证项:${NC}"
    echo -e "${GRAY}    - docker build + push to GHCR${NC}"
    echo -e "${GRAY}    - 镜像体积 < 100MB 断言${NC}"
    echo -e "${GRAY}    - docker run --rm <image> --version 格式校验${NC}"
    WARNINGS+=("Docker 镜像完整验证依赖 CI (release.yml docker job), 请确认 tag 推送后 CI 通过")
}

# ============================================================
# 参数解析
# ============================================================

SKIP_BUILD=0
for arg in "$@"; do
    case "$arg" in
        --skip-build)
            SKIP_BUILD=1
            ;;
        -h|--help)
            echo "用法: $0 [--skip-build]"
            echo ""
            echo "选项:"
            echo "  --skip-build  跳过 release binary 构建 (假定 target/release/aether 已存在)"
            echo ""
            echo "功能: 本地 Docker 验证的三级降级策略"
            echo "  1. Docker 可用 -> 完整镜像构建验证"
            echo "  2. Podman 可用 -> Podman 构建验证"
            echo "  3. 均不可用    -> Dockerfile 静态 + binary 体积 + CI 引导"
            exit 0
            ;;
        *)
            echo "错误: 未知参数 '$arg' (使用 --help 查看用法)" >&2
            exit 2
            ;;
    esac
done

# ============================================================
# 主流程
# ============================================================

echo -e "${CYAN}========================================${NC}"
echo -e "${CYAN}Chimera CLI 本地 Docker 验证 (替代脚本)${NC}"
echo -e "${CYAN}========================================${NC}"

if command_exists docker; then
    # 路径 1: Docker 完整验证
    if run_engine_verification "docker"; then
        echo ""
        echo -e "${GREEN}>>> Docker 完整验证通过 <<<${NC}"
    fi
elif command_exists podman; then
    # 路径 2: Podman 完整验证 (Docker 不可用时的替代引擎)
    echo -e "${YELLOW}未检测到 Docker, 发现 Podman, 将使用 Podman 执行镜像构建验证。${NC}"
    if run_engine_verification "podman"; then
        echo ""
        echo -e "${GREEN}>>> Podman 完整验证通过 <<<${NC}"
    fi
else
    # 路径 3: 降级验证 (Docker / Podman 均不可用)
    echo -e "${YELLOW}未检测到 Docker / Podman, 执行降级验证 (Dockerfile 静态 + binary 验证 + CI 引导)。${NC}"
    run_dockerfile_check
    run_binary_check
    show_ci_guidance
fi

# ============================================================
# 汇总报告
# ============================================================

echo ""
echo -e "${CYAN}========================================${NC}"
echo -e "${CYAN}验证汇总${NC}"
echo -e "${CYAN}========================================${NC}"
if [ "$FAIL_COUNT" -eq 0 ]; then
    echo -e "${GREEN}通过: $PASS_COUNT  失败: $FAIL_COUNT${NC}"
else
    echo -e "${RED}通过: $PASS_COUNT  失败: $FAIL_COUNT${NC}"
fi

if [ ${#WARNINGS[@]} -gt 0 ]; then
    echo ""
    echo -e "${YELLOW}注意:${NC}"
    for w in "${WARNINGS[@]}"; do
        echo -e "${YELLOW}  - $w${NC}"
    done
fi

# 失败时退出码 1, 便于 CI / 脚本编排集成
[ "$FAIL_COUNT" -eq 0 ]
