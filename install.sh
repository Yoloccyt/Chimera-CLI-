#!/usr/bin/env sh
# ============================================================
# Chimera CLI (NEXUS-OMEGA) — 一键安装脚本 (Linux / macOS)
#
# 用法:
#   curl -fsSL https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/master/install.sh | sh
#   sh install.sh [--version <ver>] [--install-dir <path>] [--skip-verify]
#
# 私有仓库安装(需 GITHUB_TOKEN 环境变量):
#   GITHUB_TOKEN=ghp_xxx curl -fsSL .../install.sh | sh
#   GITHUB_TOKEN=ghp_xxx sh install.sh
#
# 功能:
#   - 自动检测平台 (Linux/macOS) 与架构 (x86_64/aarch64)
#   - 从 GitHub Release 下载对应 binary
#   - 可选 SHA256 校验 (若 Release 附带 checksums.txt)
#   - 安装到 ~/.local/bin/chimera (默认) 或 /usr/local/bin (需 sudo)
#   - 自动追加 PATH 到当前 shell 的 rc 文件(~/.zshrc / ~/.bashrc / ~/.profile)
#   - 验证安装: chimera --version
# ============================================================

set -euo pipefail

# ------------------ 配置常量 ------------------
REPO_OWNER="Yoloccyt"
REPO_NAME="Chimera-CLI-"
GITHUB_API="https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}"
GITHUB_RELEASES="https://github.com/${REPO_OWNER}/${REPO_NAME}/releases"
DEFAULT_INSTALL_DIR="${HOME}/.local/bin"
BIN_NAME="chimera"

# ------------------ 颜色输出 ------------------
# 检测是否为 TTY,非交互模式禁用颜色 (适配 CI / curl | sh)
if [ -t 1 ] && command -v tput >/dev/null 2>&1; then
    COLOR_RED=$(tput setaf 1)
    COLOR_GREEN=$(tput setaf 2)
    COLOR_YELLOW=$(tput setaf 3)
    COLOR_BLUE=$(tput setaf 4)
    COLOR_RESET=$(tput sgr0)
else
    COLOR_RED=""
    COLOR_GREEN=""
    COLOR_YELLOW=""
    COLOR_BLUE=""
    COLOR_RESET=""
fi

info()    { printf "%s[INFO]%s %s\n"  "${COLOR_BLUE}"   "${COLOR_RESET}" "$*"; }
success() { printf "%s[OK]%s %s\n"    "${COLOR_GREEN}"  "${COLOR_RESET}" "$*"; }
warn()    { printf "%s[WARN]%s %s\n"  "${COLOR_YELLOW}" "${COLOR_RESET}" "$*"; }
error()   { printf "%s[ERROR]%s %s\n" "${COLOR_RED}"    "${COLOR_RESET}" "$*" >&2; }
die()     { error "$*"; exit 1; }

# ------------------ 参数解析 ------------------
VERSION=""
INSTALL_DIR="${DEFAULT_INSTALL_DIR}"
SKIP_VERIFY="false"

while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            [ $# -ge 2 ] || die "--version 需要参数"
            VERSION="$2"
            shift 2
            ;;
        --install-dir)
            [ $# -ge 2 ] || die "--install-dir 需要参数"
            INSTALL_DIR="$2"
            shift 2
            ;;
        --skip-verify)
            SKIP_VERIFY="true"
            shift
            ;;
        -h|--help)
            cat <<EOF
Chimera CLI 安装脚本

用法:
  sh install.sh [选项]

选项:
  --version <ver>      指定版本 (默认: latest)
  --install-dir <path> 安装目录 (默认: ${DEFAULT_INSTALL_DIR})
  --skip-verify        跳过 SHA256 校验
  -h, --help           显示帮助

示例:
  sh install.sh --version v1.0.1-omega
  sh install.sh --install-dir /usr/local/bin
  sudo sh install.sh --install-dir /usr/local/bin
EOF
            exit 0
            ;;
        *)
            die "未知参数: $1 (使用 -h 查看帮助)"
            ;;
    esac
done

# ------------------ 前置依赖检查 ------------------
command -v curl >/dev/null 2>&1 || command -v wget >/dev/null 2>&1 || die "需要 curl 或 wget"
command -v uname >/dev/null 2>&1 || die "需要 uname (核心工具缺失)"

# ------------------ 带鉴权的 HTTP 请求封装 ------------------
# WHY: 私有仓库 Release 资源需 Bearer token 鉴权;公有仓库无需。
#      封装统一入口,避免 6 处 curl/wget 调用重复鉴权逻辑,
#      规避 POSIX sh 中 ${var:+-H "$header"} word splitting
#      会拆开带空格 header value 的陷阱(POSIX sh 无数组,
#      只能用 set -- 构造位置参数 + "$@" 展开传递)。
# 参数:
#   $1 - URL
#   $2 - 输出文件路径(可选,省略则输出到 stdout)
# 返回:0 成功,非 0 失败(传递 curl/wget 退出码)
http_get() {
    _hg_url="$1"
    _hg_out="${2:-}"

    if command -v curl >/dev/null 2>&1; then
        # curl 鉴权:用 -H "Authorization: Bearer <token>" 传递
        # 用 set -- 构造位置参数,"$@" 展开时引号保留,header value 不被拆开
        if [ -n "${GITHUB_TOKEN:-}" ]; then
            set -- -H "Authorization: Bearer ${GITHUB_TOKEN}"
        else
            set --
        fi
        if [ -n "$_hg_out" ]; then
            # 下载到文件:显示进度(-fSL,不加 -s),与原脚本行为一致
            curl -fSL --retry 3 --retry-delay 2 "$@" -o "$_hg_out" "$_hg_url"
        else
            # 输出到 stdout:silent(-fsSL),API 调用无需进度
            curl -fsSL --retry 3 --retry-delay 2 "$@" "$_hg_url"
        fi
    elif command -v wget >/dev/null 2>&1; then
        # wget 鉴权:用 --header="Authorization: Bearer <token>" 传递
        if [ -n "${GITHUB_TOKEN:-}" ]; then
            set -- --header="Authorization: Bearer ${GITHUB_TOKEN}"
        else
            set --
        fi
        if [ -n "$_hg_out" ]; then
            wget -q --tries=3 --waitretry=2 "$@" -O "$_hg_out" "$_hg_url"
        else
            wget -qO- --tries=3 --waitretry=2 "$@" "$_hg_url"
        fi
    else
        return 127
    fi
}

# ------------------ 平台/架构检测 ------------------
OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}" in
    Linux*)  PLATFORM="linux";;
    Darwin*) PLATFORM="macos";;
    *)       die "不支持的操作系统: ${OS} (仅支持 Linux / macOS)";;
esac

case "${ARCH}" in
    x86_64|amd64)  ARCH_NORM="x86_64";;
    aarch64|arm64) ARCH_NORM="aarch64";;
    *)             die "不支持的架构: ${ARCH} (仅支持 x86_64 / aarch64)";;
esac

ARTIFACT_NAME="${BIN_NAME}-${PLATFORM}-${ARCH_NORM}"
info "检测到平台: ${PLATFORM} / ${ARCH_NORM}"
info "目标产物: ${ARTIFACT_NAME}"

# ------------------ 版本解析 ------------------
# 若未指定版本,通过 GitHub API 获取 latest
if [ -z "${VERSION}" ]; then
    info "未指定版本,正在获取最新版本号..."
    API_RESPONSE=$(http_get "${GITHUB_API}/releases/latest" 2>/dev/null || die "无法访问 GitHub API (网络/权限错误)")
    # 从 API 响应提取 tag_name (兼容 grep / sed)
    VERSION=$(printf "%s" "${API_RESPONSE}" | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -n1 | sed -E 's/.*"([^"]+)"$/\1/')
    [ -n "${VERSION}" ] || die "无法解析最新版本号 (仓库可能未发布 Release)"
    info "最新版本: ${VERSION}"
else
    info "指定版本: ${VERSION}"
fi

# ------------------ 下载链接构造 ------------------
DOWNLOAD_URL="${GITHUB_RELEASES}/download/${VERSION}/${ARTIFACT_NAME}"
info "下载链接: ${DOWNLOAD_URL}"

# ------------------ 创建临时目录 ------------------
TMP_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t chimera-install)"
cleanup() {
    rm -rf "${TMP_DIR}"
}
trap cleanup EXIT INT TERM

DOWNLOADED_FILE="${TMP_DIR}/${ARTIFACT_NAME}"

# ------------------ 下载 binary ------------------
info "正在下载 ${ARTIFACT_NAME} ..."
if ! http_get "${DOWNLOAD_URL}" "${DOWNLOADED_FILE}"; then
    die "下载失败 (URL: ${DOWNLOAD_URL})
可能原因:
  1) 版本不存在 (检查 --version 参数)
  2) 仓库为私有 (需 GITHUB_TOKEN 环境变量)
  3) 网络连接问题"
fi

# 防御性检查:http_get 已在下载时注入 GITHUB_TOKEN 鉴权,若文件仍为空,
# 说明 token 无效/过期,或网络中断导致下载不完整
if [ ! -s "${DOWNLOADED_FILE}" ]; then
    die "下载文件为空 (鉴权失败? 请检查 GITHUB_TOKEN 是否有效)"
fi

success "下载完成: $(ls -lh "${DOWNLOADED_FILE}" | awk '{print $5}')"

# ------------------ SHA256 校验 (可选) ------------------
if [ "${SKIP_VERIFY}" = "false" ]; then
    CHECKSUM_URL="${GITHUB_RELEASES}/download/${VERSION}/checksums.txt"
    info "尝试下载 checksums.txt 进行 SHA256 校验..."
    CHECKSUM_FILE="${TMP_DIR}/checksums.txt"
    http_get "${CHECKSUM_URL}" "${CHECKSUM_FILE}" 2>/dev/null || true

    if [ -s "${CHECKSUM_FILE}" ]; then
        if command -v sha256sum >/dev/null 2>&1; then
            EXPECTED_HASH=$(grep "${ARTIFACT_NAME}" "${CHECKSUM_FILE}" | awk '{print $1}' || true)
            if [ -n "${EXPECTED_HASH}" ]; then
                ACTUAL_HASH=$(sha256sum "${DOWNLOADED_FILE}" | awk '{print $1}')
                if [ "${EXPECTED_HASH}" = "${ACTUAL_HASH}" ]; then
                    success "SHA256 校验通过"
                else
                    die "SHA256 校验失败
  期望: ${EXPECTED_HASH}
  实际: ${ACTUAL_HASH}"
                fi
            else
                warn "checksums.txt 中未找到 ${ARTIFACT_NAME},跳过校验"
            fi
        elif command -v shasum >/dev/null 2>&1; then
            # macOS 自带 shasum
            EXPECTED_HASH=$(grep "${ARTIFACT_NAME}" "${CHECKSUM_FILE}" | awk '{print $1}' || true)
            if [ -n "${EXPECTED_HASH}" ]; then
                ACTUAL_HASH=$(shasum -a 256 "${DOWNLOADED_FILE}" | awk '{print $1}')
                if [ "${EXPECTED_HASH}" = "${ACTUAL_HASH}" ]; then
                    success "SHA256 校验通过 (shasum)"
                else
                    die "SHA256 校验失败
  期望: ${EXPECTED_HASH}
  实际: ${ACTUAL_HASH}"
                fi
            else
                warn "checksums.txt 中未找到 ${ARTIFACT_NAME},跳过校验"
            fi
        else
            warn "未找到 sha256sum / shasum,跳过校验"
        fi
    else
        warn "Release 未附带 checksums.txt,跳过 SHA256 校验"
    fi
else
    warn "已通过 --skip-verify 跳过校验"
fi

# ------------------ 安装目录准备 ------------------
# 若安装到 /usr/local/bin 等系统目录,需要 sudo
NEED_SUDO="false"
case "${INSTALL_DIR}" in
    /usr/*|/opt/*|/etc/*)
        if [ "$(id -u)" -ne 0 ]; then
            NEED_SUDO="true"
        fi
        ;;
esac

if [ "${NEED_SUDO}" = "true" ]; then
    info "安装到系统目录 ${INSTALL_DIR},需要 sudo 权限"
    sudo mkdir -p "${INSTALL_DIR}" 2>/dev/null || die "无法创建目录 ${INSTALL_DIR} (sudo 失败)"
else
    mkdir -p "${INSTALL_DIR}" || die "无法创建目录 ${INSTALL_DIR}"
fi

# ------------------ 安装 binary ------------------
INSTALL_PATH="${INSTALL_DIR}/${BIN_NAME}"
info "安装到: ${INSTALL_PATH}"

if [ "${NEED_SUDO}" = "true" ]; then
    sudo install -m 0755 "${DOWNLOADED_FILE}" "${INSTALL_PATH}" || die "安装失败 (权限不足?)"
else
    install -m 0755 "${DOWNLOADED_FILE}" "${INSTALL_PATH}" || die "安装失败"
fi

success "binary 已安装"

# ------------------ PATH 配置 ------------------
# 检查 INSTALL_DIR 是否已在 PATH 中
PATH_UPDATED="false"
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*)
        # 已在 PATH
        ;;
    *)
        # 选择当前 shell 的 rc 文件(精确按 $SHELL 选择,不写 .profile 覆盖)
        # WHY: 不同 shell source 不同启动文件,不能用 .profile 一刀切:
        #   - zsh: source ~/.zshrc(非登录)/~/.zprofile(登录),不 source ~/.profile
        #   - bash 非登录: source ~/.bashrc(不 source ~/.profile)
        #   - bash 登录: source ~/.bash_profile → ~/.bash_login → ~/.profile(按顺序)
        #   - fish: source ~/.config/fish/config.fish
        #   - POSIX sh/dash: source ~/.profile
        # 原逻辑用 .profile 覆盖导致 zsh 用户 PATH 不生效(zsh 不读 .profile),
        # bash 非登录 shell 也不读 .profile(macOS Terminal/VS Code 默认非登录),
        # 此处按 $SHELL 精确选择,确保写入用户当前 shell 会 source 的文件
        SHELL_NAME="$(basename "${SHELL:-/bin/sh}")"
        RC_FILE=""
        case "${SHELL_NAME}" in
            zsh)  RC_FILE="${HOME}/.zshrc";;
            bash) RC_FILE="${HOME}/.bashrc";;
            fish) RC_FILE="${HOME}/.config/fish/config.fish";;
            *)    RC_FILE="${HOME}/.profile";;
        esac

        # 确保 rc 文件所在目录存在(fish 的 ~/.config/fish/ 可能不存在)
        RC_DIR="$(dirname "${RC_FILE}")"
        [ -d "${RC_DIR}" ] || mkdir -p "${RC_DIR}" 2>/dev/null || true

        if [ "${NEED_SUDO}" = "false" ]; then
            # 追加 export 行(marker 防重复,幂等)
            MARKER="# chimera-cli install"
            if ! grep -q "${MARKER}" "${RC_FILE}" 2>/dev/null; then
                printf '\n%s\nexport PATH="%s:$PATH"\n' "${MARKER}" "${INSTALL_DIR}" >> "${RC_FILE}"
                PATH_UPDATED="true"
                info "PATH 已追加到 ${RC_FILE}"
            fi
        fi

        # 当前会话也更新
        PATH="${INSTALL_DIR}:${PATH}"
        ;;
esac

if [ "${PATH_UPDATED}" = "true" ]; then
    warn "请重启终端或执行: source ${RC_FILE}"
fi

# ------------------ 验证安装 ------------------
info "验证安装..."
# WHY 命令替换内 || true 短路:set -e 下 binary 退出码非 0 会触发脚本提前退出
VERSION_OUTPUT=$("${INSTALL_PATH}" --version 2>/dev/null || true)
# 与 release.yml docker job line 229 完全一致: ^(aether|chimera) [0-9]+\.[0-9]+\.[0-9]+
# 避免仅检退出码导致 binary 损坏但退出码 0 的假阳性
VERSION_REGEX='^(aether|chimera) [0-9]+\.[0-9]+\.[0-9]+'

if [ -n "${VERSION_OUTPUT}" ]; then
    if printf '%s\n' "${VERSION_OUTPUT}" | grep -Eq "${VERSION_REGEX}"; then
        success "安装成功!"
        info "版本输出: ${VERSION_OUTPUT}"
    else
        warn "${INSTALL_PATH} --version 输出格式异常"
        warn "期望格式: aether|chimera X.Y.Z[-omega]"
        warn "实际输出: ${VERSION_OUTPUT}"
        warn "请手动执行: ${INSTALL_PATH} --version"
    fi
else
    warn "${INSTALL_PATH} --version 执行失败 (退出码非 0 或无输出,可能缺少运行时依赖)"
    warn "请手动执行: ${INSTALL_PATH} --version"
fi

# ------------------ 总结输出 ------------------
printf "\n"
info "================ 安装总结 ================"
info "  版本:   ${VERSION}"
info "  路径:   ${INSTALL_PATH}"
info "  平台:   ${PLATFORM}/${ARCH_NORM}"
if [ "${PATH_UPDATED}" = "true" ]; then
    info "  PATH:   已更新 ${RC_FILE}"
fi
info "=========================================="
printf "\n"
success "执行 'chimera --help' 开始使用"
