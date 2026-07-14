# Chimera CLI (NEXUS-OMEGA) — 发布与安装指南

> **版本**: 1.7.0-omega
> **代号**: NEXUS-OMEGA (Omni-Model Engineering Generative Architecture)
> **技术栈**: Rust 2021 edition · Tokio async · Workspace 35 crates
> **安全红线**: `#![forbid(unsafe_code)]` 全覆盖
> **许可**: Apache-2.0

---

## 项目简介

Chimera CLI 是一个面向未来的多模型 AI 工程生成架构命令行工具，遵循 OMEGA 四定律构建：

| 定律 | 工程实现 | 说明 |
|------|---------|------|
| Ω-Sparse | 全维稀疏掩码(工具/上下文/记忆/审计/预算) | 按需激活，零浪费 |
| Ω-Compress | 四级窗口 + 神经形态记忆 | 4K/32K/128K/1M 分层上下文 |
| Ω-Evolve | GRPO 风格进化 + DPO 偏好优化 | 运行时自适应 |
| Ω-Event | 事件驱动架构 | Tokio broadcast 跨层解耦 |

核心组件：Event Bus 跨层通信 · SecCore 零信任沙箱 · 三层语义路由 · 14 面板终端仪表盘 · Merkle 审计链 · 能力衰减引擎。

---

## 一键安装

### Windows (PowerShell 5.1+)

```powershell
$f="$env:TEMP\chimela-install.ps1";irm https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.ps1 -OutFile $f;& $f;ri $f -Force
```

### Linux / macOS (Shell)

```bash
curl -fsSL https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.sh | sh
```

### Linux / macOS — 指定版本

```bash
curl -fsSL https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.sh | sh -s -- --version v1.7.0-omega
```

### Docker

```bash
docker pull ghcr.io/yoloccyt/chimera-cli:v1.7.0-omega
docker run --rm ghcr.io/yoloccyt/chimera-cli:v1.7.0-omega --version
```

---

## 验证安装

```bash
# 三个命令入口均可使用(互为别名)
chimela --version   # 期望输出: chimela 1.7.0-omega
aether --version    # 期望输出: aether 1.7.0-omega
chimera --version   # 期望输出: chimera 1.7.0-omega
```

---

## 基本使用

```bash
# 查看帮助
chimela --help

# 知识库检索
chimela wiki "查询关键词"

# 生成代码
chimela generate "任务描述"

# 启动 TUI 仪表盘(14 面板实时监控)
chimela tui

# 配置管理
chimela config init     # 初始化配置 ~/.aether/omega.yaml
chimela config show     # 查看当前配置
```

---

## 源码构建

### 前置条件

- Rust stable 工具链(1.82+)
- (Windows) MinGW-w64 GCC 链接器(MSYS2)

### 克隆与构建

```bash
git clone https://github.com/Yoloccyt/Chimera-CLI-.git
cd Chimera-CLI-

# 快速类型检查
cargo check --workspace

# Release 构建
cargo build --release -p chimera-cli

# 运行验证
./target/release/aether --version
```

---

## 平台支持

| 平台 | 架构 | 安装方式 |
|------|------|---------|
| Windows 10/11 | x86_64 | install.ps1 |
| Linux | x86_64 / aarch64 | install.sh |
| macOS | x86_64 / Apple Silicon | install.sh |
| Docker | 多架构(distroless) | `docker pull ghcr.io/...` |

---

## 发布流程(维护者)

```bash
# 1. 更新版本号
# 编辑 Cargo.toml: [workspace.package] version = "X.Y.Z-omega"
# 编辑 Dockerfile: ARG VERSION=X.Y.Z-omega

# 2. 预检
cargo check --workspace
cargo clippy --workspace --all-targets --jobs 2 -- -D warnings
cargo fmt --all -- --check
cargo test --workspace

# 3. 打标签并推送(触发 release.yml + fuzz.yml CI)
git tag vX.Y.Z-omega
git push origin vX.Y.Z-omega

# 4. CI 自动执行:
#    - build: 5 平台 matrix 并行构建
#    - test: workspace 全量测试
#    - docker: GHCR 镜像构建 + 推送 + 体积验证(< 100MB)
#    - release: 创建 GitHub Release + 上传产物
#    - fuzz: 6 target x 300s 模糊测试
```

### Release 产物清单

| 产物 | 说明 |
|------|------|
| `chimela-windows-x86_64.exe` | Windows binary |
| `chimela-linux-x86_64` | Linux x86_64 binary |
| `chimela-linux-aarch64` | Linux ARM64 binary |
| `chimela-macos-x86_64` | macOS Intel binary |
| `chimela-macos-aarch64` | macOS Apple Silicon binary |
| `checksums.txt` | SHA256 校验文件 |
| `ghcr.io/.../chimera-cli:vX.Y.Z-omega` | Docker 镜像(distroless, < 100MB, 非 root) |

---

## 故障排除

| 问题 | 原因 | 解决 |
|------|------|------|
| 下载失败 | 网络/DNS/防火墙 | 使用 `-Proxy` 参数或手动下载 Release binary 后用 `-LocalFile` 安装 |
| `--version` 无输出(Windows) | 缺少 VC++ 运行时 | 安装 [VC++ Redistributable](https://aka.ms/vs/17/release/vc_redist.x64.exe) |
| GitHub API 404 | 私有仓库匿名访问 | 设置 `GITHUB_TOKEN` 环境变量鉴权 |
| `cargo build` 链接失败 | MinGW gcc 未安装 | 安装 MSYS2 + `mingw-w64-x86_64-gcc` |
| 镜像体积 > 100MB | 构建缓存膨胀 | `docker build --no-cache` |

---

## 版本号规则

| 标签示例 | 语义 | CI prerelease |
|---------|------|---------------|
| `v1.7.0-omega` | 正式发布 | false |
| `v1.7.1-omega` | Bug 修复 | false |
| `v2.0.0-omega` | Breaking Change | false |
| `v1.7.0-omega-alpha.1` | Alpha 预发布 | true |
| `v1.7.0-omega-beta.1` | Beta 预发布 | true |
| `v1.7.0-omega-rc.1` | 发布候选 | true |

---

**NEXUS-OMEGA — Ω-Sparse · Ω-Compress · Ω-Evolve · Ω-Event**
