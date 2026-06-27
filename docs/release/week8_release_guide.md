# Chimera CLI (NEXUS-OMEGA) — Week 8 发布指南

> **版本**:1.0.0-omega
> **阶段**:Week 8 Task 4 跨平台发布
> **约束**:`#![forbid(unsafe_code)]` 全覆盖 · Docker 镜像 < 100MB · 单 binary < 50MB

---

## 1. 平台支持矩阵

| 平台 | Target Triple | Runner | 构建方式 | 产物文件名 |
|------|---------------|--------|----------|-----------|
| Windows x86_64 | `x86_64-pc-windows-gnu` | windows-latest | cargo 原生 | `chimera-windows-x86_64.exe` |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | ubuntu-latest | cargo 原生 | `chimera-linux-x86_64` |
| Linux aarch64 | `aarch64-unknown-linux-gnu` | ubuntu-latest | `cross` 交叉编译 | `chimera-linux-aarch64` |
| macOS x86_64 | `x86_64-apple-darwin` | macos-latest | cargo 原生 | `chimera-macos-x86_64` |
| macOS aarch64 | `aarch64-apple-darwin` | macos-latest | cargo 原生 | `chimera-macos-aarch64` |

**Binary 内部产物名**:`aether`(由 `crates/chimera-cli/Cargo.toml` 的 `[[bin]] name = "aether"` 决定)。CI/Docker 中重命名为 `chimera` 以保持对外品牌一致。

---

## 2. 本地构建

### 2.1 环境要求(Windows 主机)

```powershell
# 工具链环境变量(每次新 shell 需设置)
$env:CARGO_HOME = 'D:\Chimera CLI\.toolchain\cargo'
$env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'
$env:TMP = 'D:\Chimera CLI\tmp'
$env:TEMP = 'D:\Chimera CLI\tmp'
$env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"
$env:CARGO_TARGET_DIR = 'D:\Chimera CLI\target'
$env:CARGO_INCREMENTAL = '0'
```

### 2.2 本机构建(Windows x86_64)

```powershell
# Release 构建(应用 [profile.release] 优化)
cargo build --release -p chimera-cli

# 产物位置
ls D:\Chimera CLI\target\release\aether.exe

# 验证
.\target\release\aether.exe --version
```

### 2.3 本机交叉编译(可选,需额外工具)

Windows 主机缺 Linux/macOS 链接器,本机交叉编译受限。**推荐方案**:

- **方案 A(推荐)**:用 GitHub Actions CI 构建(见第 3 节)
- **方案 B**:`cargo-zigbuild` + zig 工具链
  ```powershell
  cargo install cargo-zigbuild
  # 需单独安装 zig,然后:
  cargo zigbuild --release --target x86_64-unknown-linux-gnu -p chimera-cli
  ```
- **方案 C**:`cross`(需 Docker Desktop)
  ```powershell
  cargo install cross
  cross build --release --target aarch64-unknown-linux-gnu -p chimera-cli
  ```

> **当前环境状态**:Windows 主机已就绪 `x86_64-pc-windows-gnu` 原生构建;Linux/macOS 交叉编译依赖外部工具链,建议走 CI。

---

## 3. CI/CD 构建(GitHub Actions)

### 3.1 触发方式

Workflow 文件:`.github/workflows/release.yml`

```yaml
on:
  push:
    tags:
      - 'v1.0.0-omega'
      - 'v*.*.*-omega'
  workflow_dispatch:  # 手动触发
```

### 3.2 发布流程

```bash
# 1. 确保所有测试通过
cargo test --workspace

# 2. 打 tag
git tag v1.0.0-omega
git push origin v1.0.0-omega

# 3. GitHub Actions 自动触发:
#    - 5 平台 matrix 构建
#    - workspace 测试
#    - 创建 GitHub Release 并上传 5 个 binary
```

### 3.3 CI 产物

| 平台 | Release 资产名 |
|------|----------------|
| Windows | `chimera-windows-x86_64.exe` |
| Linux x86_64 | `chimera-linux-x86_64` |
| Linux aarch64 | `chimera-linux-aarch64` |
| macOS Intel | `chimera-macos-x86_64` |
| macOS Apple Silicon | `chimera-macos-aarch64` |

---

## 4. Docker 镜像

### 4.1 构建

```bash
# 在项目根目录
docker build -t chimera:1.0.0-omega .

# 验证镜像大小(< 100MB)
docker images chimera:1.0.0-omega
```

### 4.2 运行

```bash
# 查看版本
docker run --rm chimera:1.0.0-omega --version

# 运行任务
docker run --rm chimera:1.0.0-omega run "解释 OMEGA 四定律"

# 启动 TUI(需 -it 交互终端)
docker run --rm -it chimera:1.0.0-omega tui

# 挂载配置目录(持久化 ~/.aether/omega.yaml)
docker run --rm -v "$HOME/.aether:/root/.aether" chimera:1.0.0-omega config show
```

### 4.3 Dockerfile 设计要点

- **多阶段构建**:Stage 1 `rust:1.82-slim` 编译,Stage 2 `distroless/cc-debian12` 运行
- **distroless 基础镜像**:无 shell、无包管理器,攻击面最小化(契合 `#![forbid(unsafe_code)]` 安全哲学)
- **层缓存优化**:先 `COPY Cargo.toml Cargo.lock` 和 `crates/`,再 `cargo build`,源码变更不触发依赖重编译
- **体积优化**:workspace 级 `[profile.release]` 配置 strip/lto/opt-level=z/panic=abort
- **ENTRYPOINT exec form**:distroless 无 shell,必须用 `["chimera"]` 而非 `"chimera"`

---

## 5. Release Profile 优化说明

在根 `Cargo.toml` 末尾添加(workspace 级,所有 crate 共享):

```toml
[profile.release]
strip = true          # 移除调试符号
panic = "abort"       # 避免 unwind 表(代价:panic 直接退出)
opt-level = "z"       # 体积最小化优化
lto = true            # 跨 crate 链接时优化
codegen-units = 1     # 单 codegen unit,最大化优化空间
```

**预期体积**:`aether` binary 经 strip + lto + opt-level=z 后应 < 50MB(满足约束)。

**代价**:
- `panic = "abort"`:panic 不 unwind,直接退出进程(不影响 `Result` 错误处理)
- `codegen-units = 1`:release 编译时间增加约 2-3x(可接受,release 不频繁)
- `opt-level = "z"`:运行速度比 `3` 略低(约 5-10%),体积优先场景可接受

---

## 6. 验证方法

### 6.1 Binary 验证

```bash
# 版本号(所有平台)
chimera --version
# 预期输出:chimera 1.0.0-omega

# 帮助
chimera --help

# 配置初始化(生成 ~/.aether/omega.yaml)
chimera config init

# 简单任务
chimera run "hello"
```

### 6.2 Docker 验证

```bash
docker run --rm chimera:1.0.0-omega --version
# 预期:chimera 1.0.0-omega
```

### 6.3 体积验证

```bash
# Linux/macOS
ls -lh chimera-linux-x86_64
# 预期:< 50M

# Docker 镜像
docker images chimera:1.0.0-omega
# 预估:< 100MB
```

---

## 7. 升级路径

### 7.1 从源码升级

```bash
git pull origin main
cargo build --release -p chimera-cli
# 替换旧 binary
cp target/release/aether /usr/local/bin/chimera
```

### 7.2 Docker 升级

```bash
docker pull chimera:1.0.0-omega
# 停止旧容器,启动新容器
docker stop <old-container> && docker rm <old-container>
docker run -d --name chimera chimera:1.0.0-omega
```

### 7.3 版本号规则

- `v1.0.0-omega`:首个 OMEGA 发布
- `v1.1.0-omega`:新功能(API 兼容)
- `v1.0.1-omega`:bug 修复
- `v2.0.0-omega`:Breaking Change
- `v1.0.0-omega-alpha.1`:预发布

---

## 8. 故障排查

### 8.1 本机构建失败

| 问题 | 原因 | 解决 |
|------|------|------|
| `error: linker 'gcc' not found` | MSYS2 未安装或 PATH 未配置 | 安装 MSYS2,设置 `PATH` 包含 `D:\msys64\mingw64\bin` |
| `error: failed to run custom build command for openssl-sys` | 缺 OpenSSL | 项目用 `rustls-tls`,检查是否有 crate 显式依赖 openssl |
| `cargo build` 卡在 `Updating crates.io index` | 网络问题 | 配置镜像源或离线编译 |
| binary 体积 > 50MB | profile 未生效 | 确认根 `Cargo.toml` 有 `[profile.release]` 且用 `--release` |

### 8.2 CI 构建失败

| 问题 | 原因 | 解决 |
|------|------|------|
| `cross build` 失败 | cross 版本与 Rust 不兼容 | 用 `cargo install cross --git` 安装最新版 |
| aarch64-linux 产物无法运行 | 在非 ARM 环境测试 | 用 `qemu-aarch64` 或 ARM 服务器验证 |
| macOS 签名警告 | 未签名 | 生产环境需 Apple Developer 证书签名 + notarize |
| Release 创建失败 | `permissions: contents: write` 缺失 | 检查 workflow 权限声明 |

### 8.3 Docker 构建失败

| 问题 | 原因 | 解决 |
|------|------|------|
| 构建上下文过大(GB 级) | `target/` 未排除 | 确认 `.dockerignore` 包含 `target/` |
| `COPY --from=builder ... aether` 找不到文件 | binary 名错误 | 确认 `crates/chimera-cli/Cargo.toml` 的 `[[bin]] name = "aether"` |
| distroless 运行报 `exec format error` | 架构不匹配 | 构建时用 `--platform`,或在对应平台 runner 构建 |
| 镜像 > 100MB | 未用多阶段或未 strip | 确认 Stage 2 是 `distroless/cc-debian12` 且 `[profile.release]` 含 `strip=true` |

### 8.4 运行时问题

| 问题 | 原因 | 解决 |
|------|------|------|
| `chimera: command not found` | PATH 未配置 | `export PATH=$PATH:/usr/local/bin` 或 Windows 加入系统 PATH |
| 配置文件找不到 | 默认 `~/.aether/omega.yaml` 不存在 | 运行 `chimera config init` 生成 |
| TUI 乱码 | 终端不支持 UTF-8/颜色 | Windows 用 Windows Terminal,设 `TERM=xterm-256color` |

---

## 9. 文件清单

| 文件 | 用途 |
|------|------|
| `Cargo.toml` `[profile.release]` | workspace 级体积优化 |
| `Dockerfile` | 多阶段 Docker 构建 |
| `.dockerignore` | Docker 构建上下文排除 |
| `.github/workflows/release.yml` | 5 平台 CI/CD 发布流水线 |
| `docs/release/week8_release_guide.md` | 本文档 |

---

## 10. 当前状态(Week 8 Task 4)

- ✅ **SubTask 4.1**:Windows x86_64 本机构建就绪;Linux/macOS 交叉编译需 CI 环境
- ✅ **SubTask 4.2**:Dockerfile(多阶段 distroless)+ `[profile.release]` 优化
- ✅ **SubTask 4.3**:GitHub Actions 5 平台 matrix + Release 自动创建
- ✅ **SubTask 4.4**:发布指南(本文档)

**未完成项**(需 CI 环境验证):
- Linux/macOS binary 实际交叉编译产物(本机缺链接器)
- Docker 镜像实际构建(本机 Docker 可用性未知,CI 中验证)
- GitHub Release 实际创建(需推送 tag 触发)
