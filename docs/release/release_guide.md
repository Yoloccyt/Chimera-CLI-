# Chimera CLI (NEXUS-OMEGA) — 发布与安装指南

> **版本**: 以 `Cargo.toml` `workspace.package.version` 为权威源（2026-10-11 现取 = `2.29.0-omega`,2026-09-27 本地发布、tag 未 push;**最新已发布 tag = `v2.28.2-omega`(2026-09-08)**;下方安装示例的 tag 一律以 GitHub Releases 实际存在的最新 `-omega` tag 为准）
> **代号**: NEXUS-OMEGA (Omni-Model Engineering Generative Architecture)
> **技术栈**: Rust 2021 edition · Tokio async · Workspace 多 crate（**crate 数以 `Cargo.toml` `workspace.members` 为准**,2026-10-11 = 39；装配面/可达性以 `scripts/check_crate_reachability.sh` 现测为准）
> **协作流程**: 分支 / 提交 / PR / 标签发布规则的唯一权威源是根目录 [CONTRIBUTING.md](../../CONTRIBUTING.md)（含 GPG 条件签名与 tag 重指流程),本文件只讲安装与发布操作
> **安全红线**: `#![forbid(unsafe_code)]` 全 crate 覆盖（逐 crate 清单见 `docs/architecture/CODE_WIKI.md`）
> **许可**: ⚠️ **待决冲突**——本文件原写 Apache-2.0，`LICENSE` 正文为 **Apache-2.0**，而 `Cargo.toml` `workspace.package.license = "MIT"`。三处不一致，发布前须统一（见 `docs/reports/doc-audit-ledger-P0-2026-09-20.md` §9.3）

> ## ⚠ 2026-09-21 可执行性校正（本指南多处命令/名称与代码不符，以下为实测结论）
>
> 1. **二进制名只有一个：`chimera`**（`crates/chimera-cli/Cargo.toml:156` `[[bin]] name = "chimera"`，本地实测 `./target/release/chimera --version` → `chimera 2.28.2-omega`）。下文出现的 `chimela` 并非可用命令入口：`aether`/`chimela` 只是 CI 版本校验的**兼容前缀**（`.github/workflows/release.yml:451` 接受 `^(aether|chimera|chimela)`）与 `install.ps1` 的**安装目录名**（默认 `…\Programs\chimela`）。请统一用 `chimera` 调用。
> 2. **`generate` 子命令不存在**——`crates/chimera-cli/src/cli.rs` 的 `Commands` enum 无该变体（全文件 `generate` 零命中）。代码生成请走 `chimera run`（或 `llm` / `agent`）。完整清单以该 enum 为准。
> 3. **Release 产物名是 `chimera-*` 而非 `chimela-*`**——`.github/workflows/release.yml:52` `artifact_name: chimera-windows-x86_64.exe`，`:549` 同。下文产物清单已按此订正。
> 4. **默认配置路径是 `~/.chimera/omega.yaml`**（`crates/chimera-cli/src/config.rs:34`、`cli.rs:77`）。下文 `~/.aether/omega.yaml` 为旧值；注：`cli.rs:521` 的一处注释仍残留 `.aether`，属代码注释待修。
> 5. **Docker 镜像名正确无需改**：`ghcr.io/<owner>/chimera-cli`（`release.yml:34` `IMAGE_NAME`；`:31` 注释说明了仓库名尾 `-` 不能作为镜像 reference 的处理原因）。
> 6. **TUI 面板数以代码现取为准,勿抄数字**（权威表述见 `docs/tui/README.md` 顶部「面板口径」;2026-10-11 实测:`PanelId` 枚举 = `ALL` = **30 变体**,其中进入焦点环 `REGISTERED_FOCUS_ORDER` 的 **29 个**可 `Tab` 到达;`InjectionStrategy` 经 FC-05 下线未注册但实现保留。下文「14/27 面板」均为早期时点值）。
> 7. **OMEGA 定律为十一条**：Ω₁₀-Card / Ω₁₁-Synthesize 已由 ADR-170 收录并落地；下方表只列 Ω₁~Ω₉ 九条,是全集的子集。

---

## 项目简介

Chimera CLI 是一个面向未来的多模型 AI 工程生成架构命令行工具，遵循 OMEGA 十一定律构建（下表列 Ω₁~Ω₉,Ω₁₀-Card / Ω₁₁-Synthesize 见 `docs/architecture/CODE_WIKI.md §1.2` 与 ADR-170）：

| 定律 | 工程实现 | 说明 |
|------|---------|------|
| Ω₁-Sparse | 全维稀疏掩码(工具/上下文/记忆/审计/预算) | 按需激活，零浪费 |
| Ω₂-Compress | 四级窗口 + 神经形态记忆 | 4K/32K/128K/1M 分层上下文 |
| Ω₃-Evolve | GRPO 风格进化 + DPO 偏好优化 | 运行时自适应 |
| Ω₄-Event | 事件驱动架构 | Tokio broadcast + Critical mpsc 双通道（变体清单以 `crates/event-bus/src/types.rs` 为准） |
| Ω₅-Credit | SHARP Shapley 值精确信用归因 | 三元分解奖励 |
| Ω₆-Reuse | 复用率优先 | SkillGraph + 能力替代 |
| Ω₇-Locate | L1→L2→L3 行为定位 | 自动导航修改点 |
| Ω₈-Assess | Runtime Auditor 五维证据纪律 | 自我评估 |
| Ω₉-Preserve | 变体隔离 + 停止策略 | 保留历史最佳 |

核心组件：Event Bus 跨层通信 · SecCore 零信任沙箱（Linux 走 gVisor/runsc，非 Linux 降级进程隔离）· 三层语义路由 · Concord TUI（面板数以 `PanelId` 为准 + slash 命令数以 `SlashCommandRegistry` 为准，Chat/Quest 双轨） · Merkle 审计链 · 能力衰减引擎 · MCA 多通道亲和网关（feature 门控）。crate 数与装配面口径见本文件顶部校正块（不在此登记数值）。

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
# 将 vX.Y.Z-omega 换成 GitHub Releases 上实际存在的最新 tag（2026-10-11 时点最新已发布 = v2.28.2-omega）
curl -fsSL https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.sh | sh -s -- --version vX.Y.Z-omega
```

### Docker

```bash
docker pull ghcr.io/yoloccyt/chimera-cli:vX.Y.Z-omega
docker run --rm ghcr.io/yoloccyt/chimera-cli:vX.Y.Z-omega --version
```

---

## 验证安装

```bash
# 唯一命令入口（[[bin]] name = "chimera"）
chimera --version   # 期望输出: chimera <workspace.package.version>
```

> CI 的版本校验额外兼容历史前缀 `aether` / `chimela`（`release.yml:451`），但本地构建产物只有 `chimera(.exe)`；不要按三别名并列使用。

---

## 基本使用

```bash
# 查看帮助（子命令清单的权威源 = crates/chimera-cli/src/cli.rs 的 Commands enum）
chimera --help
chimera help                       # 含 EXAMPLES 段

# 知识库检索
chimera wiki "查询关键词"

# 执行任务（注意：无 generate 子命令，代码生成走 run / llm / agent）
chimera run "任务描述"

# 启动 TUI（面板数 = PanelId 枚举现取:2026-10-11 为 30 类型 / 29 个可 Tab 到达,见 docs/tui/README.md）
chimera tui

# 配置管理（默认配置文件 ~/.chimera/omega.yaml）
chimera config init
chimera config show
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
./target/release/chimera --version
```

---

## 回滚与逃生开关（运维）

当某个子系统出现异常时，**先查回滚开关台账**，再考虑改代码或回退版本。

台账权威源：[ADR-242 回滚开关统一台账](../architecture/ADR-242-rollback-switch-registry.md)
—— 每项含「开关 / 作用 / 回滚到的行为 / 影响范围 / 验证命令」五列，可直接照着验证。

清单概览（**完整表与安全语义说明见 ADR-242**，此处不复制，避免双源）：

| 类别 | 示例开关 | 用途 |
|------|---------|------|
| 并行化回滚 | `CHIMERA_NO_PARALLEL_*`、`CHIMERA_MCP_SERIAL_COMMIT` | 把并行路径退回串行 |
| 性能/启动回滚 | `CHIMERA_SYSINFO_FULL_REFRESH`、`CHIMERA_CONFIG_EAGER`、`CHIMERA_RT_FLAVOR` | 退回改造前的启动或采集形态 |
| 兼容通道 | `CHIMERA_NO_V3_ENGINE`、`--legacy-exit-code` | 旧行为过渡（兼容窗口**已过期但当前保留**） |
| 诊断 | `CHIMERA_TEST_TIMEOUT_SCALE`、`CHIMERA_PERF_SCALE`、`CHIMERA_PANIC_TEST` | 仅非生产路径 |

> ⚠ **安全语义不可降级**：沙箱层 / 策略层 / 路由层**不提供**任何关闭开关。
> 排查工具调用被拒时，请读错误文案的前缀（`sandbox:` / `policy:` / `route:`）——
> 它已含具体拦截原因，而非关闭沙箱。详见 ADR-242 §3.5。

现取全部 `CHIMERA_*` 变量（区分回滚开关与普通配置项）：

```powershell
git grep -oh 'CHIMERA_[A-Z0-9_]*' -- "*.rs" "*.ps1" "*.sh" "*.yml" | Sort-Object -Unique
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

> 分支模型、提交约定、PR 合并、tag 重指补救与 GPG 条件签名的**规则层权威源**是 [CONTRIBUTING.md](../../CONTRIBUTING.md) §3/§6;本节是操作步骤。

```bash
# 0. 发布前完整门禁见 agents.md §7.2 与 .claude/CLAUDE.md「发布前检查清单」(check/clippy/fmt/全量测试/audit/perf 门)

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
#    - fuzz: 8 target x 300s 模糊测试
```

### Release 产物清单

| 产物 | 说明 |
|------|------|
| `chimera-windows-x86_64.exe` | Windows binary |
| `chimera-linux-x86_64` | Linux x86_64 binary |
| `chimera-linux-aarch64` | Linux ARM64 binary |
| `chimera-macos-x86_64` | macOS Intel binary |
| `chimera-macos-aarch64` | macOS Apple Silicon binary |
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
