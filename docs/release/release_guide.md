# Chimera CLI (NEXUS-OMEGA) — 发布指南

> **版本**:1.0.0-omega
> **范围**:本地构建 · CI 5 平台发布 · Docker 镜像 · 交叉编译选项
> **关联**:`.github/workflows/release.yml` · `Dockerfile` · `crates/chimera-cli/Cargo.toml`
> **约束**:`#![forbid(unsafe_code)]` 全覆盖 · Docker 镜像 < 100MB · 单 binary < 50MB

---

## 1. 本地构建(Windows x86_64)

本地主机仅能可靠构建 Windows x86_64 GNU 目标(项目默认工具链 `stable-x86_64-pc-windows-gnu`)。

### 1.1 环境变量(每次新 shell 需设置)

```powershell
$env:CARGO_HOME = 'D:\Chimera CLI\.toolchain\cargo'
$env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'
$env:TMP = 'D:\Chimera CLI\tmp'
$env:TEMP = 'D:\Chimera CLI\tmp'
$env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"
```

### 1.2 Release 构建

```powershell
cargo build --release -p chimera-cli
```

- 产物路径:`target\release\aether.exe`(由 `crates/chimera-cli/Cargo.toml` 的 `[[bin]] name = "aether"` 决定)
- workspace 级 `[profile.release]`(根 `Cargo.toml` 第 224 行)已启用 `strip=true` / `panic="abort"` / `opt-level="z"` / `lto=true` / `codegen-units=1`
- 验证:`.\target\release\aether.exe --version`

> **品牌一致性**:`aether` 为内部 binary 名,CI/Docker 中重命名为 `chimera` 对外发布。

---

## 2. CI 构建(5 平台)

Workflow:`.github/workflows/release.yml`(已静态验证 10/10 项通过)。

### 2.1 触发方式

```yaml
on:
  push:
    tags:
      - 'v1.0.0-omega'      # 首发固定标签
      - 'v*.*.*-omega'      # 后续版本通配
  workflow_dispatch:        # 手动触发(调试用)
```

### 2.2 发布流程

```bash
# 1. 本地预检
cargo test --workspace
cargo build --release -p chimera-cli

# 2. 打标签并推送
git tag v1.0.0-omega
git push origin v1.0.0-omega

# 3. GitHub Actions 自动执行:
#    - build job:5 平台 matrix 并行构建(fail-fast: false)
#    - test  job:workspace 测试(独立,不阻塞 build)
#    - release job:needs [build, test] → 创建 GitHub Release + 上传 5 产物
```

### 2.3 平台矩阵与产物

| 平台 | Target Triple | Runner | 构建方式 | Release 资产名 |
|------|---------------|--------|----------|----------------|
| Windows x86_64 | `x86_64-pc-windows-gnu` | windows-latest | cargo 原生 | `chimera-windows-x86_64.exe` |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | ubuntu-latest | cargo 原生 | `chimera-linux-x86_64` |
| Linux aarch64 | `aarch64-unknown-linux-gnu` | ubuntu-latest | `cross` 交叉编译 | `chimera-linux-aarch64` |
| macOS x86_64 | `x86_64-apple-darwin` | macos-latest | cargo 原生 | `chimera-macos-x86_64` |
| macOS aarch64 | `aarch64-apple-darwin` | macos-latest | cargo 原生 | `chimera-macos-aarch64` |

### 2.4 CI 关键设计

- **权限**:`permissions: contents: write`(创建 Release 必需)
- **缓存**:`actions/cache@v4`,key = `${{ runner.os }}-${{ matrix.target }}-cargo-${{ hashFiles('**/Cargo.lock') }}`
- **产物保留**:`retention-days: 30`
- **Release 创建**:`softprops/action-gh-release@v2`,`generate_release_notes: true`
- **prerelease 判定**:标签名含 `-alpha` / `-beta` / `-rc` 即标记为 prerelease
- **aarch64-linux**:matrix 标记 `cross: true`,通过 `cargo install cross --git` 安装后用 `cross build`

### 2.5 配套 Fuzz Workflow(Week 8 限制深度攻坚新增)

独立 workflow:`.github/workflows/fuzz.yml`,与 `release.yml` 并行运行,职责为模糊测试(解除本地 Windows GNU 无法运行 libFuzzer 的限制 1)。

**触发方式**

- 推送 `v*.*.*-omega` 标签(与 release.yml 同触发条件,GitHub Actions 会各自独立运行,互不冲突)
- 手动触发(workflow_dispatch,便于调试)

**运行环境与时长**

| 项 | 值 |
|----|----|
| Runner | `ubuntu-latest`(委托 Linux CI,本地 Windows GNU 不兼容 libFuzzer C++) |
| 工具链 | nightly + `llvm-tools-preview`(`-Zsanitizer=address` 需要) |
| 工具 | `cargo install cargo-fuzz` |
| Matrix | 3 target 并行(`fail-fast: false`,独立失败互不阻塞) |
| 单 target 时长 | 300s(`-max_total_time=300`)+ 编译 |
| 总时长 | ≈ 15-20min |

**Fuzz Target**(对应 `fuzz/Cargo.toml` 的 `[[bin]]` 声明)

| Target | 被测模块 | 说明 |
|--------|----------|------|
| `quest_parse` | `quest-engine` | Quest 解析路径鲁棒性 |
| `seccore_sandbox` | `seccore` | 安全沙箱边界输入 |
| `event_serialize` | `event-bus` | 事件序列化/反序列化(MessagePack + JSON) |

**结果查看与产物**

- GitHub 仓库 → Actions → `Fuzz` workflow → 选择对应运行
- 日志产物:`fuzz-log-<target>` artifact(retention 30 天,`if: always()` 始终上传)
- 崩溃输入:`fuzz-crash-<target>` artifact(retention 90 天,`if: failure()` 仅失败时上传,`if-no-files-found: ignore` 无崩溃不报错)
- 崩溃输入路径:`fuzz/artifacts/<target>/crash-*`(可用于本地复现)

**关键设计点**

- `fail-fast: false`:单 target 崩溃不阻塞其他 target,完整暴露所有问题
- `cache` key = `fuzz-<target>-cargo-<Cargo.lock hash>`,按 target 隔离缓存避免污染
- `working-directory: ./fuzz`:fuzz crate 是独立 package,不在主 workspace 内
- `cargo +nightly fuzz run`:显式指定 nightly 工具链,避免 stable 默认工具链报错

---

## 3. Docker 镜像构建

Dockerfile 已静态验证 10/10 项通过。**本机无 Docker 环境,镜像构建委托 CI 或具备 Docker 的主机执行。**

### 3.1 构建与推送

```powershell
# 本地构建(需 Docker Desktop / Docker Engine)
docker build -t chimera:latest .

# 打版本标签 + 推送 GHCR
docker tag chimera:latest ghcr.io/<org>/chimera:v1.0.0-omega
docker login ghcr.io -u <username> -p <PAT>
docker push ghcr.io/<org>/chimera:v1.0.0-omega
```

> `<org>` 替换为 GitHub 组织/用户名;`<PAT>` 需 `write:packages` 权限。

### 3.2 运行

```bash
# 版本验证
docker run --rm chimera:latest --version

# 运行任务
docker run --rm chimera:latest run "解释 OMEGA 四定律"

# 交互式 TUI
docker run --rm -it chimera:latest tui

# 持久化配置
docker run --rm -v "$HOME/.aether:/root/.aether" chimera:latest config show
```

### 3.3 Dockerfile 设计要点

| 项 | 说明 |
|----|------|
| 多阶段 | Stage1 `rust:1.82-slim` 编译 → Stage2 `gcr.io/distroless/cc-debian12` 运行 |
| 层缓存 | 先 `COPY Cargo.toml Cargo.lock` + `crates/`,再 `cargo build`,源码变更不触发依赖重编译 |
| 体积 | distroless ~20MB + binary ~7MB(strip+lto+opt-level=z)≈ 27MB < 100MB |
| 安全 | distroless 无 shell / 无包管理器,契合 `#![forbid(unsafe_code)]` |
| ENTRYPOINT | exec form `["chimera"]`(distroless 无 shell,shell form 会失败) |
| binary 重命名 | `COPY --from=builder .../aether /usr/local/bin/chimera` |

### 3.4 CI Docker Job(Week 8 限制深度攻坚新增)

`release.yml` 新增 `docker` job(第 146-200 行),在推 tag 时自动构建镜像并推送 GHCR,解除本地无 Docker 的限制 3。位于 `test` job 之后、`release` job 之前,`release` job 的 `needs` 已更新为 `[build, test, docker]`。

**Job 依赖与触发**

- `needs: build`(等待 5 平台 binary 构建完成,保持依赖顺序)
- `if: startsWith(github.ref, 'refs/tags/v')`(仅 tag 推送触发,手动触发不跑 Docker)
- `permissions: contents: read` + `packages: write`(GHCR 推送必需)

**GHCR 镜像地址与 pull 命令**

镜像仓库:`ghcr.io/<org>/<repo>`(`<org>/<repo>` 由 `${{ github.repository }}` 自动填充)

```bash
# 拉取指定版本(tag 推送时由 docker/metadata-action 自动生成)
docker pull ghcr.io/<org>/chimera-cli:v1.0.0-omega

# 拉取 latest(仅非预发布版本会打 latest 标签)
docker pull ghcr.io/<org>/chimera-cli:latest

# 运行
docker run --rm ghcr.io/<org>/chimera-cli:v1.0.0-omega --version
```

> `<org>` 替换为 GitHub 组织/用户名(取决于仓库归属)。

**Tag 自动生成规则**(由 `docker/metadata-action@v5` 处理)

| Tag 来源 | 示例 | 说明 |
|----------|------|------|
| `type=ref,event=tag` | `v1.0.0-omega` | 与 git tag 同名 |
| `type=raw,value=latest` | `latest` | 仅当 tag 不含 `-alpha`/`-beta`/`-rc` 时启用(预发布不打 latest) |

**CI 关键设计点**

| 项 | 说明 |
|----|------|
| Buildx | `docker/setup-buildx-action@v3`,启用多阶段构建与缓存 |
| 缓存 | `cache-from: type=gha` + `cache-to: type=gha,mode=max`,GitHub Actions 缓存加速二次构建 |
| 镜像体积验证 | CI 内 `docker image inspect` 检查 < 100MB(104857600 字节),超限 fail job |
| 体积估算 | distroless ~20MB + binary ~7MB(strip+lto+opt-level=z)≈ 27MB,远低于 100MB 上限 |
| 登录 | `docker/login-action@v3` + `${{ secrets.GITHUB_TOKEN }}`(无需额外配置 PAT) |

> **与本地构建的关系**:CI Docker job 复用根 `Dockerfile`(不修改),通过 GHA 缓存加速;本地构建流程见 3.1 节,两者产物一致。

---

## 4. 交叉编译选项

Windows 主机缺 Linux/macOS 链接器,本机交叉编译受限。三种方案对比:

| 方案 | 工具依赖 | 支持目标 | 本机可用性 | 推荐度 |
|------|----------|----------|-----------|--------|
| **GitHub Actions CI** | 无(云端 runner) | 全 5 平台 | ✅ 可用(推 tag 触发) | ⭐⭐⭐ 首选 |
| **cargo-zigbuild** | zig + cargo-zigbuild | Linux/macOS(含 aarch64) | ❌ zig 未安装 | ⭐⭐ 备选 |
| **cross-rs** | Docker + cross | Linux(含 aarch64) | ❌ 本机无 Docker | ⭐ 备选 |

### 4.1 方案 A:GitHub Actions CI(推荐)

见第 2 节。推 `v*.*.*-omega` 标签即可,5 平台全覆盖,零本地依赖。

### 4.2 方案 B:cargo-zigbuild

```powershell
# 前置:安装 zig(预编译二进制 ~50MB,下载解压加入 PATH)
#   https://ziglang.org/download/
cargo install cargo-zigbuild

# 交叉编译 aarch64-linux
cargo zigbuild --release --target aarch64-unknown-linux-gnu -p chimera-cli
```

**本机状态(Week 8 Task 4 实测)**:
- `cargo-zigbuild` 已安装 ✓
- `zig` 未安装 ✗ → 运行报错 `Error: Failed to find zig / cannot find binary path`
- 结论:需先安装 zig 才能使用,详见第 6 节"已知限制"

### 4.3 方案 C:cross-rs(需 Docker)

```powershell
cargo install cross --git https://github.com/cross-rs/cross
cross build --release --target aarch64-unknown-linux-gnu -p chimera-cli
```

本机无 Docker,此方案不可用;CI 中 aarch64-linux 已采用此方案。

### 4.4 替代方案评估(本机无 zig/Docker 时)

| 替代 | 可行性 | 说明 |
|------|--------|------|
| 安装 zig | ✅ 可行 | 下载预编译二进制 ~50MB,解压加 PATH 即可,无管理员权限 |
| WSL2 | ✅ 可行 | WSL2 内装 Linux 工具链可原生构建 linux-x86_64;aarch64 仍需 cross |
| 委托 CI | ✅ 推荐 | 推 tag 触发,无需本地任何额外工具 |

---

## 5. 验证清单

发布前/后逐项核对:

### 5.1 Binary 验证

```bash
# 所有平台通用
chimera --version          # 预期:chimera 1.0.0-omega
chimera --help             # 子命令列表
chimera config init        # 生成 ~/.aether/omega.yaml
```

### 5.2 体积验证

```bash
# Linux/macOS binary
ls -lh chimera-linux-x86_64      # 预期:< 50M
ls -lh chimera-linux-aarch64     # 预期:< 50M

# Docker 镜像
docker images chimera:latest     # 预期:< 100MB
```

### 5.3 GitHub Release 产物核对

Release 页面应包含 **5 个 binary 资产**:

- [ ] `chimera-windows-x86_64.exe`
- [ ] `chimera-linux-x86_64`
- [ ] `chimera-linux-aarch64`
- [ ] `chimera-macos-x86_64`
- [ ] `chimera-macos-aarch64`

### 5.4 CI Workflow 静态核对(已完成,Week 8 Task 4)

`release.yml` 10 项验证全部通过:触发条件 · 5 平台 matrix · 依赖方向 · cross 工具 · 缓存策略 · 产物上传 · GitHub Release · 权限 · prerelease 判定 · binary 重命名。

`Dockerfile` 10 项验证全部通过:多阶段 · 基础镜像 · 系统依赖 · 层缓存 · 构建命令 · binary 重命名 · ENTRYPOINT exec form · 体积估算 · 安全哲学 · 镜像标签。

---

## 6. 已知限制(本地环境)

> 本机环境:Windows 11 + PowerShell,GNU 工具链,无 Docker,无 zig。

| 限制编号 | 限制内容 | 本地状态 | 解决策略 |
|----------|----------|----------|----------|
| **限制 2** | 5 平台 binary 交叉编译需 CI 环境 | ❌ 本地不可用 | 委托 GitHub Actions CI(推 tag 触发) |
| **限制 3** | Docker 镜像构建需 Docker 环境 | ❌ 本地无 Docker | 委托 CI 或具备 Docker 的主机 |

### 6.1 本地可验证项

- ✅ Windows x86_64 原生构建(`cargo build --release -p chimera-cli`)
- ✅ workspace 测试(`cargo test --workspace`)
- ✅ release.yml / Dockerfile 静态验证(已完成)
- ✅ `[profile.release]` 体积优化配置已就位

### 6.2 本地不可验证项(委托 CI)

- ❌ Linux x86_64 / aarch64 binary 实际产物(缺链接器)
- ❌ macOS x86_64 / aarch64 binary 实际产物(缺 macOS 工具链)
- ❌ Docker 镜像实际构建与体积(无 Docker)
- ❌ GitHub Release 实际创建(需推 tag 触发)
- ❌ `docker pull ghcr.io/<org>/chimera:<tag>` 实际可用性

### 6.3 提升本地能力的可选路径

1. **安装 zig(~50MB 预编译二进制)**:解锁 cargo-zigbuild,本机即可交叉编译 Linux/macOS(含 aarch64)
2. **启用 WSL2**:在 WSL2 内构建 linux-x86_64 原生产物
3. **安装 Docker Desktop**:解锁本机 Docker 镜像构建 + cross-rs

当前阶段遵循"长期主义 + 不过度配置",上述安装按需进行;Week 8 验收以 CI 委托为准。

---

## 7. 版本号规则

| 标签 | 语义 | CI prerelease |
|------|------|---------------|
| `v1.0.0-omega` | 首个 OMEGA 发布 | false |
| `v1.1.0-omega` | 新功能(API 兼容) | false |
| `v1.0.1-omega` | bug 修复 | false |
| `v2.0.0-omega` | Breaking Change | false |
| `v1.0.0-omega-alpha.1` | 预发布(alpha) | true |
| `v1.0.0-omega-beta.1` | 预发布(beta) | true |
| `v1.0.0-omega-rc.1` | 发布候选 | true |

CI 通过 `contains(github.ref_name, '-alpha'/'-beta'/'-rc')` 自动判定 prerelease 标记。

---

## 8. 相关文件索引

| 文件 | 用途 | 验证状态 |
|------|------|----------|
| `.github/workflows/release.yml` | 5 平台 CI/CD + Docker GHCR 发布流水线 | ✅ 静态验证 10/10(Week 8 新增 docker job) |
| `.github/workflows/fuzz.yml` | cargo-fuzz 模糊测试(3 target × 300s) | ✅ Week 8 限制深度攻坚新增 |
| `Dockerfile` | 多阶段 distroless 镜像 | ✅ 静态验证 10/10 |
| `.dockerignore` | Docker 构建上下文排除 | ✅ 存在 |
| `Cargo.toml` `[profile.release]` | workspace 级体积优化 | ✅ 已配置(line 224) |
| `crates/chimera-cli/Cargo.toml` | `[[bin]] name = "aether"` | ✅ 已声明 |
| `docs/release/release_guide.md` | 本文档 | ✅ 新建 |
| `docs/release/week8_release_guide.md` | Week 8 详细发布指南(补充参考) | ✅ 已存在 |
| `docs/release/v1.0.0-omega_release_notes.md` | v1.0.0-omega 发布说明 | ✅ 已存在 |
