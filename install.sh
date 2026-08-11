#!/usr/bin/env sh
# ============================================================
# Chimera CLI (NEXUS-OMEGA) �?一键安装脚�?(Linux / macOS)
#
# 用法:
#   curl -fsSL https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.sh | sh
#   sh install.sh [--version <ver>] [--install-dir <path>] [--skip-verify]
#
# 私有仓库安装(需 GITHUB_TOKEN 环境变量):
#   WHY: raw.githubusercontent.com 对私有仓�?raw 内容拒绝匿名访问,
#        必须显式�?HTTP header 中传�?Authorization: Bearer <token>�?
#        仅设置环境变量不会自动被 curl 加入 header�?
#
#   Linux / macOS:
#     export GITHUB_TOKEN=ghp_xxx
#     curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" \
#       https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.sh | sh
#
#   如果 curl 持续 404,建议直接克隆仓库后本地执�?
#     git clone https://github.com/Yoloccyt/Chimera-CLI-.git
#     cd Chimera-CLI-
#     export GITHUB_TOKEN=ghp_xxx
#     sh install.sh
#
# 功能:
#   - 自动检测平�?(Linux/macOS) 与架�?(x86_64/aarch64)
#   - �?GitHub Release 下载对应 binary
#   - 可�?SHA256 校验 (�?Release 附带 checksums.txt)
#   - 安装�?~/.local/bin/chimela (默认) �?/usr/local/bin (需 sudo)
#   - 自动追加 PATH 到当�?shell �?rc 文件(~/.zshrc / ~/.bashrc / ~/.profile)
#   - 验证安装: chimela --version
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
# 检测是否为 TTY,非交互模式禁用颜�?(适配 CI / curl | sh)
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
            [ $# -ge 2 ] || die "--version 需要参�?
            VERSION="$2"
            shift 2
            ;;
        --install-dir)
            [ $# -ge 2 ] || die "--install-dir 需要参�?
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
  sh install.sh --version v2.26.0-omega
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

# ------------------ 前置依赖检�?------------------
command -v curl >/dev/null 2>&1 || command -v wget >/dev/null 2>&1 || die "需�?curl �?wget"
command -v uname >/dev/null 2>&1 || die "需�?uname (核心工具缺失)"

# ------------------ 带鉴权的 HTTP 请求封装 ------------------
# WHY: 私有仓库 Release 资源需 Bearer token 鉴权;公有仓库无需�?
#      封装统一入口,避免 6 �?curl/wget 调用重复鉴权逻辑,
#      规避 POSIX sh �?${var:+-H "$header"} word splitting
#      会拆开带空�?header value 的陷�?POSIX sh 无数�?
#      只能�?set -- 构造位置参�?+ "$@" 展开传�?�?
# 参数:
#   $1 - URL
#   $2 - 输出文件路径(可�?省略则输出到 stdout)
# 返回:0 成功,�?0 失败(传�?curl/wget 退出码)
http_get() {
    _hg_url="$1"
    _hg_out="${2:-}"

    if command -v curl >/dev/null 2>&1; then
        # curl 鉴权:�?-H "Authorization: Bearer <token>" 传�?
        # �?set -- 构造位置参�?"$@" 展开时引号保�?header value 不被拆开
        if [ -n "${GITHUB_TOKEN:-}" ]; then
            set -- -H "Authorization: Bearer ${GITHUB_TOKEN}"
        else
            set --
        fi
        if [ -n "$_hg_out" ]; then
            # 下载到文�?显示进度(-fSL,不加 -s),与原脚本行为一�?
            curl -fSL --retry 3 --retry-delay 2 "$@" -o "$_hg_out" "$_hg_url"
        else
            # 输出�?stdout:silent(-fsSL),API 调用无需进度
            curl -fsSL --retry 3 --retry-delay 2 "$@" "$_hg_url"
        fi
    elif command -v wget >/dev/null 2>&1; then
        # wget 鉴权:�?--header="Authorization: Bearer <token>" 传�?
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

# ------------------ 平台/架构检�?------------------
OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}" in
    Linux*)  PLATFORM="linux";;
    Darwin*) PLATFORM="macos";;
    *)       die "不支持的操作系统: ${OS} (仅支�?Linux / macOS)";;
esac

case "${ARCH}" in
    x86_64|amd64)  ARCH_NORM="x86_64";;
    aarch64|arm64) ARCH_NORM="aarch64";;
    *)             die "不支持的架构: ${ARCH} (仅支�?x86_64 / aarch64)";;
esac

ARTIFACT_NAME="${BIN_NAME}-${PLATFORM}-${ARCH_NORM}"
info "检测到平台: ${PLATFORM} / ${ARCH_NORM}"
info "目标产物: ${ARTIFACT_NAME}"

# ------------------ 版本解析 ------------------
# 若未指定版本,通过 GitHub API 获取 latest
if [ -z "${VERSION}" ]; then
    info "未指定版�?正在获取最新版本号..."
    # WHY die 在命令替换外�?POSIX sh �?$(...) 创建�?shell,
    # �?shell 内的 exit 仅退出子 shell 不影响父脚本�?
    # 若写 `API_RESPONSE=$(http_get ... || die "...")`,die 在子 shell
    # 执行,exit 1 被吞�?父脚本继续运行导致后�?${VERSION} 为空触发
    # 兜底 die,虽功能正确但可读性误导。此处将 || die 移到命令替换外层,
    # 确保 die 在父 shell 执行,set -e 下命令替换退出码会传递到外层�?
    API_RESPONSE=$(http_get "${GITHUB_API}/releases/latest" 2>/dev/null) || die "无法访问 GitHub API (网络/权限错误)"
    # �?API 响应提取 tag_name (兼容 grep / sed)
    VERSION=$(printf "%s" "${API_RESPONSE}" | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -n1 | sed -E 's/.*"([^"]+)"$/\1/')
    [ -n "${VERSION}" ] || die "无法解析最新版本号 (仓库可能未发�?Release)"
    info "最新版�? ${VERSION}"
else
    info "指定版本: ${VERSION}"
fi

# ------------------ 下载链接构�?------------------
DOWNLOAD_URL="${GITHUB_RELEASES}/download/${VERSION}/${ARTIFACT_NAME}"
info "下载链接: ${DOWNLOAD_URL}"

# ------------------ 创建临时目录 ------------------
TMP_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t chimela-install)"
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
  1) 版本不存�?(检�?--version 参数)
  2) 仓库为私�?(需 GITHUB_TOKEN 环境变量)
  3) 网络连接问题"
fi

# 防御性检�?http_get 已在下载时注�?GITHUB_TOKEN 鉴权,若文件仍为空,
# 说明 token 无效/过期,或网络中断导致下载不完整
if [ ! -s "${DOWNLOADED_FILE}" ]; then
    die "下载文件为空 (鉴权失败? 请检�?GITHUB_TOKEN 是否有效)"
fi

success "下载完成: $(ls -lh "${DOWNLOADED_FILE}" | awk '{print $5}')"

# ------------------ SHA256 校验辅助函数 ------------------
# 计算 SHA256 哈希(stdout 输出 hash)
# WHY 抽函�?sha256sum (Linux) �?shasum -a 256 (macOS) 输出格式相同,
# 仅命令名不同,原代码两分支�?15 行几乎完全重�?DRY 违规)�?
# 参数:
#   $1 - 文件路径
# 返回:0(总是),stdout 输出 hash;工具不可用时 stdout 为空,
#       调用方通过 [ -z "${ACTUAL_HASH}" ] 判定
compute_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        # macOS 自带 shasum
        shasum -a 256 "$1" | awk '{print $1}'
    fi
    # 工具不可用时 stdout 为空,return 0 避免触发 set -e
    return 0
}

# �?checksums.txt 提取期望 hash(stdout 输出 hash)
# 参数:
#   $1 - checksums 文件路径
#   $2 - artifact 名称(grep 模式)
# 返回:0,stdout 输出 hash;未找到匹配时 stdout 为空
extract_expected_hash() {
    grep "$2" "$1" 2>/dev/null | awk '{print $1}' || true
}

# ------------------ SHA256 校验 (可�? ------------------
if [ "${SKIP_VERIFY}" = "false" ]; then
    CHECKSUM_URL="${GITHUB_RELEASES}/download/${VERSION}/checksums.txt"
    info "尝试下载 checksums.txt 进行 SHA256 校验..."
    CHECKSUM_FILE="${TMP_DIR}/checksums.txt"
    http_get "${CHECKSUM_URL}" "${CHECKSUM_FILE}" 2>/dev/null || true

    if [ -s "${CHECKSUM_FILE}" ]; then
        EXPECTED_HASH=$(extract_expected_hash "${CHECKSUM_FILE}" "${ARTIFACT_NAME}")
        if [ -n "${EXPECTED_HASH}" ]; then
            ACTUAL_HASH=$(compute_sha256 "${DOWNLOADED_FILE}")
            if [ -z "${ACTUAL_HASH}" ]; then
                warn "未找�?sha256sum / shasum,跳过校验"
            elif [ "${EXPECTED_HASH}" = "${ACTUAL_HASH}" ]; then
                success "SHA256 校验通过"
            else
                die "SHA256 校验失败
  期望: ${EXPECTED_HASH}
  实际: ${ACTUAL_HASH}"
            fi
        else
            warn "checksums.txt 中未找到 ${ARTIFACT_NAME},跳过校验"
        fi
    else
        warn "Release 未附�?checksums.txt,跳过 SHA256 校验"
    fi
else
    warn "已通过 --skip-verify 跳过校验"
fi

# ------------------ 安装目录准备 ------------------
# 若安装到 /usr/local/bin 等系统目�?需�?sudo
NEED_SUDO="false"
case "${INSTALL_DIR}" in
    /usr/*|/opt/*|/etc/*)
        if [ "$(id -u)" -ne 0 ]; then
            NEED_SUDO="true"
        fi
        ;;
esac

if [ "${NEED_SUDO}" = "true" ]; then
    info "安装到系统目�?${INSTALL_DIR},需�?sudo 权限"
    sudo mkdir -p "${INSTALL_DIR}" 2>/dev/null || die "无法创建目录 ${INSTALL_DIR} (sudo 失败)"
else
    mkdir -p "${INSTALL_DIR}" || die "无法创建目录 ${INSTALL_DIR}"
fi

# ------------------ 安装 binary ------------------
INSTALL_PATH="${INSTALL_DIR}/${BIN_NAME}"
info "安装�? ${INSTALL_PATH}"

if [ "${NEED_SUDO}" = "true" ]; then
    sudo install -m 0755 "${DOWNLOADED_FILE}" "${INSTALL_PATH}" || die "安装失败 (权限不足?)"
else
    install -m 0755 "${DOWNLOADED_FILE}" "${INSTALL_PATH}" || die "安装失败"
fi

success "binary 已安�?

# 兼容别名:�?chimela �?aether 创建符号链接指向 chimera
# WHY: 旧品牌名 chimela 和内部编码名 aether 作为向后兼容别名保留,
#      确保老用户和新用户都能找到命令入�?
for _alias in chimela aether; do
    _alias_path="${INSTALL_DIR}/${_alias}"
    if [ ! -e "${_alias_path}" ]; then
        ln -sf "${BIN_NAME}" "${_alias_path}" 2>/dev/null || true
        info "已创建别�? ${_alias_path} -> ${BIN_NAME}"
    fi
done

# ------------------ PATH 配置 ------------------
# 检�?INSTALL_DIR 是否已在 PATH �?
PATH_UPDATED="false"
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*)
        # 已在 PATH
        ;;
    *)
        # 选择当前 shell �?rc 文件(精确�?$SHELL 选择,不写 .profile 覆盖)
        # WHY: 不同 shell source 不同启动文件,不能�?.profile 一刀�?
        #   - zsh: source ~/.zshrc(非登�?/~/.zprofile(登录),�?source ~/.profile
        #   - bash 非登�? source ~/.bashrc(�?source ~/.profile)
        #   - bash 登录: source ~/.bash_profile �?~/.bash_login �?~/.profile(按顺�?
        #   - fish: source ~/.config/fish/config.fish
        #   - POSIX sh/dash: source ~/.profile
        # 原逻辑�?.profile 覆盖导致 zsh 用户 PATH 不生�?zsh 不读 .profile),
        # bash 非登�?shell 也不�?.profile(macOS Terminal/VS Code 默认非登�?,
        # 此处�?$SHELL 精确选择,确保写入用户当前 shell �?source 的文�?
        SHELL_NAME="$(basename "${SHELL:-/bin/sh}")"
        RC_FILE=""
        case "${SHELL_NAME}" in
            zsh)  RC_FILE="${HOME}/.zshrc";;
            bash) RC_FILE="${HOME}/.bashrc";;
            fish) RC_FILE="${HOME}/.config/fish/config.fish";;
            *)    RC_FILE="${HOME}/.profile";;
        esac

        # 确保 rc 文件所在目录存�?fish �?~/.config/fish/ 可能不存�?
        RC_DIR="$(dirname "${RC_FILE}")"
        [ -d "${RC_DIR}" ] || mkdir -p "${RC_DIR}" 2>/dev/null || true

        if [ "${NEED_SUDO}" = "false" ]; then
            # 追加 export �?marker 防重�?幂等)
            MARKER="# chimela-cli install"
            if ! grep -q "${MARKER}" "${RC_FILE}" 2>/dev/null; then
                printf '\n%s\nexport PATH="%s:$PATH"\n' "${MARKER}" "${INSTALL_DIR}" >> "${RC_FILE}"
                PATH_UPDATED="true"
                info "PATH 已追加到 ${RC_FILE}"
            fi
        fi

        # 当前会话也更�?
        PATH="${INSTALL_DIR}:${PATH}"
        ;;
esac

if [ "${PATH_UPDATED}" = "true" ]; then
    warn "请重启终端或执行: source ${RC_FILE}"
fi

# ------------------ 验证安装 ------------------
info "验证安装..."
# WHY 命令替换�?|| true 短路:set -e �?binary 退出码�?0 会触发脚本提前退�?
VERSION_OUTPUT=$("${INSTALL_PATH}" --version 2>/dev/null || true)
# �?release.yml docker job line 232 完全一�? ^(aether|chimera|chimela) [0-9]+\.[0-9]+\.[0-9]+
# 避免仅检退出码导致 binary 损坏但退出码 0 的假阳�?
VERSION_REGEX='^(aether|chimera|chimela) [0-9]+\.[0-9]+\.[0-9]+'

if [ -n "${VERSION_OUTPUT}" ]; then
    if printf '%s\n' "${VERSION_OUTPUT}" | grep -Eq "${VERSION_REGEX}"; then
        success "安装成功!"
        info "版本输出: ${VERSION_OUTPUT}"
    else
        warn "${INSTALL_PATH} --version 输出格式异常"
        warn "期望格式: aether|chimera|chimela X.Y.Z[-omega]"
        warn "实际输出: ${VERSION_OUTPUT}"
        warn "请手动执�? ${INSTALL_PATH} --version"
    fi
else
    warn "${INSTALL_PATH} --version 执行失败 (退出码�?0 或无输出,可能缺少运行时依�?"
    warn "请手动执�? ${INSTALL_PATH} --version"
fi

# ------------------ 总结输出 ------------------
printf "\n"
info "================ 安装总结 ================"
info "  版本:   ${VERSION}"
info "  路径:   ${INSTALL_PATH}"
info "  平台:   ${PLATFORM}/${ARCH_NORM}"
if [ "${PATH_UPDATED}" = "true" ]; then
    info "  PATH:   已更�?${RC_FILE}"
fi
info "=========================================="
printf "\n"
success "执行 'chimela --help' 开始使�?
