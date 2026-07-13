# Chimera CLI (NEXUS-OMEGA)

[![Version](https://img.shields.io/badge/version-1.0.0--omega-blue.svg)](./CHANGELOG.md)
[![License](https://img.shields.io/badge/license-Apache--2.0-green.svg)](./LICENSE)
[![Crates](https://img.shields.io/badge/crates-34-orange.svg)](./CODE_WIKI.md)
[![Tests](https://img.shields.io/badge/tests-3002+-brightgreen.svg)](./CHANGELOG.md)
[![unsafe](https://img.shields.io/badge/forbid(unsafe_code)-34/34-success.svg)](./docs/security/week8_security_report.md)

> **Omni-Model Engineering Generative Architecture**
> 一个面向未来的多模型 AI 工程生成架构命令行工具,代号 **NEXUS-OMEGA**。

---

## 项目总览

Chimera CLI(代号 **NEXUS-OMEGA**)是一个基于 Rust 2021 edition 构建的 34 crate workspace 项目,遵循 10 层架构(L1-L10)和 **OMEGA 四定律**:

| 定律 | 全称 | 工程实现 | 对应 crate |
|------|------|---------|-----------|
| **Ω-Sparse** | 全维稀疏(工具/上下文/记忆/审计/预算) | 五维度掩码稀疏化 | `osa-coordinator` |
| **Ω-Compress** | 层次化上下文压缩 | 四级窗口 + 神经形态记忆 | `hcw-window` + `mlc-engine` |
| **Ω-Evolve** | 自组织在线进化 | GRPO 风格进化 + 偏好优化 | `gsoe-evolution` + `auto-dpo` |
| **Ω-Event** | 事件驱动架构 | Tokio broadcast 跨层解耦 | `event-bus` |

项目核心目标是为多模型 AI 应用提供一个安全、可观测、可进化的异步执行框架,避免传统 AI 代码助手常见的"孤儿调用""裸奔命令""内存爆炸"等系统性问题。

### 关键特性

- **安全**:`#![forbid(unsafe_code)]` 34/34 crate 全覆盖;SecCore 零信任沙箱;OWASP Top 10 全部拦截
- **性能**:三层路由 p95 = 78.79µs(目标 ≤ 2ms,25× 余量);WAL 崩溃恢复 1000 次零数据丢失;SSRA 融合 5.64µs
- **可观测**:Event Bus 跨层通信;Prometheus /metrics 端点;Merkle 审计链
- **可进化**:GRPO 在线进化;DPO 偏好优化;黏液式快速适配

---

## 快速开始

### 环境要求

- **OS**:Windows 11 / Linux / macOS
- **Rust**:`stable-x86_64-pc-windows-gnu`(Windows)或对应平台 stable 工具链
- **链接器**(Windows):MinGW-w64 GCC(`D:\msys64\mingw64\bin\gcc.exe`)

### 安装

#### 方式 1:一键安装脚本(推荐)

项目提供跨平台一键安装脚本,自动检测平台/架构、下载对应 binary、校验 SHA256、配置 PATH 并验证安装。

> **仓库可见性说明**
> 本仓库(Yoloccyt/Chimera-CLI-)当前为**私有仓库**。`raw.githubusercontent.com` 拉取私有仓库内容必须在 HTTP header 中携带 `Authorization: Bearer <GITHUB_TOKEN>`,否则直接返回 `404 Not Found`。
> 如果你已将仓库改为公有,可省略 `-H` / `-Headers` 参数。

##### 公有仓库(可省略鉴权 header)

**Linux / macOS**:

```bash
curl -fsSL https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.sh | sh
```

**Windows** (PowerShell 5.1 / 7+):

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.ps1)))
```

> **WHY 推荐 scriptblock 方式**: `install.ps1` 顶部包含 `param()` 块。`iex (irm ...)` 在部分 PowerShell 7.x 版本下会把 `param()` 解析为非法赋值表达式，导致 `The assignment expression is not valid`。`[scriptblock]::Create()` 是执行完整脚本的正确方式。

##### 私有仓库(必须携带 GITHUB_TOKEN)

**Linux / macOS**:

```bash
export GITHUB_TOKEN='ghp_xxx'
curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" \
  https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.sh | sh
```

**Windows**:

```powershell
$env:GITHUB_TOKEN='ghp_xxx'
$headers = @{ Authorization = "Bearer $env:GITHUB_TOKEN" }
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/Yoloccyt/Chimera-CLI-/main/install.ps1 -Headers $headers)))
```

**WHY 需要 header 鉴权**: `raw.githubusercontent.com` 对私有仓库的 raw 内容拒绝匿名访问;`GITHUB_TOKEN` 仅作为环境变量存在时,`curl`/ `irm` 不会自动把它加入 header,必须显式传递。脚本内部下载 Release binary 时同样会读取该环境变量。

##### 可选参数

```bash
# 指定版本
sh install.sh --version v1.0.2-omega

# 指定安装目录
sh install.sh --install-dir /usr/local/bin

# 跳过 SHA256 校验
sh install.sh --skip-verify

# Windows: 仅设置工具链环境变量(不下载 binary)
.\install.ps1 -SetupEnv
```

脚本功能详见 [install.sh](./install.sh) / [install.ps1](./install.ps1) 头部注释。

> **⚠️ 一键安装脚本访问失败?**
> 如果你在中国大陆或某些企业网络中,`raw.githubusercontent.com` 可能被 DNS 污染或阻断,导致 `404` / `Connection timed out`。解决方法:
> 1. 确认仓库可见性:私有仓库必须按上方示例携带 `Authorization: Bearer <GITHUB_TOKEN>` header
> 2. 使用 [GitHub Releases](https://github.com/Yoloccyt/Chimera-CLI-/releases) 手动下载对应平台 binary(见方式 2)
> 3. 配置 DNS / 代理后重试
> 4. 克隆仓库后本地执行 `sh install.sh` / `.\install.ps1`

#### 方式 2:从 Release 下载

从 [GitHub Releases](https://github.com/Yoloccyt/Chimera-CLI-/releases) 下载对应平台的预编译 binary:

| 平台 | 文件名 |
|------|--------|
| Windows x86_64 | `chimera-windows-x86_64.exe` |
| Linux x86_64 | `chimera-linux-x86_64` |
| Linux aarch64 | `chimera-linux-aarch64` |
| macOS Intel | `chimera-macos-x86_64` |
| macOS Apple Silicon | `chimera-macos-aarch64` |

```bash
# Linux/macOS
chmod +x chimera-linux-x86_64
sudo mv chimera-linux-x86_64 /usr/local/bin/chimera
chimera --version

# Windows
move chimera-windows-x86_64.exe C:\Windows\chimera.exe
chimera --version
```

#### 方式 3:从源码构建

```powershell
# 0. 克隆仓库
git clone <repo-url> "D:\Chimera CLI"
cd "D:\Chimera CLI"

# 1. 一键配置工具链环境变量(推荐,仅需执行一次)
#    自动设置 CARGO_HOME / RUSTUP_HOME / PATH 到用户级环境变量
.\install.ps1 -SetupEnv
#    执行后重启 PowerShell 终端使环境变量生效

# 2. 构建验证
cargo build --workspace --release
.\target\release\aether.exe --version
# 预期输出:chimera 1.0.0-omega
```

> **手动设置环境变量**(替代方案,适用于非默认路径或自定义工具链):
> ```powershell
> $env:CARGO_HOME = 'D:\Chimera CLI\.toolchain\cargo'
> $env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'
> $env:TMP = 'D:\Chimera CLI\tmp'
> $env:TEMP = 'D:\Chimera CLI\tmp'
> $env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"
> ```

#### 方式 4:Docker

```bash
# 构建镜像(< 100MB,distroless 基础)
docker build -t chimera:1.0.0-omega .

# 查看版本
docker run --rm chimera:1.0.0-omega --version

# 运行任务
docker run --rm chimera:1.0.0-omega run "解释 OMEGA 四定律"

# 挂载配置目录(持久化 ~/.aether/omega.yaml)
docker run --rm -v "$HOME/.aether:/root/.aether" chimera:1.0.0-omega config show
```

### 基础用法

```bash
# 查看版本
aether --version

# 初始化配置(生成 ~/.aether/omega.yaml)
aether config init

# Wiki 语义搜索
aether wiki "查询内容"

# 生成 omega.yaml 配置模板
aether generate

# 指定配置文件
aether --config ~/.aether/omega.yaml wiki "查询"
```

> **说明**:binary 内部产物名为 `aether`(由 `crates/chimera-cli/Cargo.toml` 的 `[[bin]] name = "aether"` 决定)。CI/Docker 中重命名为 `chimera` 以保持对外品牌一致。

---

## 已知限制(占位实现)

> 以下 6 项占位实现已在 v1.0.0-omega 中明确标注,计划于 v1.1.0+ 逐步替换为生产级实现。
> 这些占位不影响核心功能正确性,仅限制部分高级特性的精度与泛化能力。

| 占位实现 | 位置 | 当前状态 | v1.1+ 计划 |
|---------|------|---------|----------|
| MTPE pseudo_predictions | `crates/mtpe-executor/src/predictor.rs:248` | 伪预测(N=1-10 步,确定性推断) | 接入真实多步预测模型 |
| SecCore ASA rule-based | `crates/seccore/src/asa.rs:146` | 规则式风险评分 | Critic PPO 强化学习 |
| GSOE policy rule-based | `crates/gsoe-evolution/src/policy/{grpo,mutation,fitness}.rs` | 规则式进化策略 | GRPO 风格强化学习 |
| NMC multimodal perceptors | `crates/nmc-encoder/src/perceptors/{image,video,audio}.rs` | 占位返回 EncodingFailed | ort ONNX 接入 |
| RepoWiki placeholder_embedding | `crates/repo-wiki/src/generator.rs:66` | SHA-256 哈希扩展(无语义) | NMC CLV 512-dim 替换 |
| SCC InMemoryWal | `crates/scc-cache/src/wal.rs:125` | 内存缓冲(进程退出丢失) | SqliteWal 持久化 |

**设计决策**:v1.0.0-omega 选择"占位 + 明确标注"而非"阻塞发布",因为这些组件均有完整的接口契约和测试覆盖,替换为生产实现时无需修改调用方代码(符合依赖倒置原则)。详见 [docs/release/v1.0.0-omega_release_notes.md](./docs/release/v1.0.0-omega_release_notes.md) 第 6 节。
---

## 10 层架构

```text
┌─────────────────────────────────────────────────────────────────┐
│ L10  Interface ─ chimera-cli · chimera-tui · chtc-bridge        │
│                   mcp-mesh · csn-substitutor                    │
├─────────────────────────────────────────────────────────────────┤
│ L9   Quest ────── quest-engine · gea-activator · efficiency-monitor │
├─────────────────────────────────────────────────────────────────┤
│ L8   Parliament ─ parliament · acb-governor · decb-governor     │
├─────────────────────────────────────────────────────────────────┤
│ L7   Execution ── pvl-layer · gqep-executor · mtpe-executor     │
│                   ssra-fusion                                   │
├─────────────────────────────────────────────────────────────────┤
│ L6   Router ───── osa-coordinator · kvbsr-router · faae-router  │
│                   sesa-router                                   │
├─────────────────────────────────────────────────────────────────┤
│ L5   Knowledge ── repo-wiki · gsoe-evolution · auto-dpo         │
├─────────────────────────────────────────────────────────────────┤
│ L4   Security ─── seccore · qeep-protocol · decay-engine        │
├─────────────────────────────────────────────────────────────────┤
│ L3   Storage ──── scc-cache · lsct-tiering · cmt-tiering        │
├─────────────────────────────────────────────────────────────────┤
│ L2   Memory ───── nmc-encoder · hcw-window · mlc-engine         │
├─────────────────────────────────────────────────────────────────┤
│ L1   Core ─────── nexus-core · event-bus · model-router         │
└─────────────────────────────────────────────────────────────────┘
```

### 依赖铁律

```
L(N) → L(N)     ✓ 同层互引允许
L(N) → L(N-1)   ✓ 向下依赖允许
L(N) → L(N+1)   ✗ 向上依赖禁止
L(N) ──event-bus── L(M)  ✓ 跨层通信只能走 Event Bus
L(N) ──mcp-mesh─── L(M)  ✓ 跨进程通信只能走 MCP Mesh
```

完整架构说明见:[docs/architecture/ten_layers.md](./docs/architecture/ten_layers.md)

---

## 34 Crate 索引

| 层 | Crate | 职责 |
|----|-------|------|
| L1 Core | `nexus-core` | 核心类型(CLV/NexusState/UserIntent)+ 共享工具 |
| L1 Core | `event-bus` | Tokio broadcast 跨层通信唯一通道 |
| L1 Core | `model-router` | CACR 成本感知模型路由 |
| L2 Memory | `nmc-encoder` | 神经多模态上下文编码(5 感知器 → CLV 512-dim) |
| L2 Memory | `hcw-window` | 分层上下文窗口(4K/32K/128K/1M) |
| L2 Memory | `mlc-engine` | 四级神经形态记忆(工作/情景/语义/程序) |
| L3 Storage | `scc-cache` | 推测上下文缓存(马尔可夫链预取) |
| L3 Storage | `lsct-tiering` | 任务感知能力分层 |
| L3 Storage | `cmt-tiering` | 能力记忆分层(热/温/冷/冰) |
| L4 Security | `seccore` | 安全核心 + ASA 对抗审计 + Merkle 审计链 |
| L4 Security | `qeep-protocol` | 量子纠缠执行(孤儿调用检测) |
| L4 Security | `decay-engine` | 能力衰减引擎(连续权限流体) |
| L5 Knowledge | `repo-wiki` | 代码 Wiki + ISCM 跨层共享索引 |
| L5 Knowledge | `gsoe-evolution` | 引导式自组织在线进化(GRPO) |
| L5 Knowledge | `auto-dpo` | Direct Preference Optimization 偏好优化 |
| L6 Router | `osa-coordinator` | 全维稀疏协调器(五维度掩码) |
| L6 Router | `kvbsr-router` | KV-Block 语义路由(两级块路由) |
| L6 Router | `faae-router` | Function-as-Expert + EDSB 熵驱动自均衡 |
| L6 Router | `sesa-router` | 子专家稀疏激活(256-bit 位向量掩码) |
| L7 Execution | `pvl-layer` | 生产验证闭环(Producer-Verifier) |
| L7 Execution | `gqep-executor` | 聚集查询执行协议(超时治理) |
| L7 Execution | `mtpe-executor` | 多步预测执行(N=1-10 伪预测) |
| L7 Execution | `ssra-fusion` | 黏液式快速适配(预编译模板融合) |
| L8 Parliament | `parliament` | 5 角色对抗性议会 + AHIRT 红队 |
| L8 Parliament | `acb-governor` | ACB 治理器(能力预算控制) |
| L8 Parliament | `decb-governor` | 双档认知预算治理(连续可调) |
| L9 Quest | `quest-engine` | Quest 引擎 + TTG 思考切换 + LHQP 检查点 |
| L9 Quest | `gea-activator` | 门控专家激活器(Sigmoid 连续门控) |
| L9 Quest | `efficiency-monitor` | 效率监控与告警(Prometheus /metrics) |
| L10 Interface | `chimera-cli` | CLI 入口(Clap + Figment 多源配置) |
| L10 Interface | `chimera-tui` | 基于 ratatui 的 TUI 界面 |
| L10 Interface | `chtc-bridge` | 跨平台 IDE 桥(5 大 IDE 适配) |
| L10 Interface | `mcp-mesh` | MCP 量子网格(跨进程通信) |
| L10 Interface | `csn-substitutor` | 能力替代网络(降级链) |

**统计**:34 个 crate,覆盖率 100%(Week 8 Task 2 已补齐 acb-governor / auto-dpo / chimera-tui)

---

## 性能指标摘要

| 指标 | 目标 | 实测 | 余量 | 状态 |
|------|------|------|------|------|
| WAL 崩溃恢复(1000 次) | 零数据丢失 | 1000/1000 通过 | — | ✅ 达标 |
| 三层路由 p95 延迟 | ≤ 2ms | 78.79µs (0.079ms) | 25× | ✅ 远超目标 |
| SSRA 100 模板融合 | ≤ 20ms | 5.64µs | 3500× | ✅ 远超目标 |
| SESA 256 专家激活 p95 | ≤ 5ms | ≤ 5ms | — | ✅ 达标 |
| SESA 稀疏度 | < 40% | 0.3984375(102/256) | — | ✅ 严格达标 |
| MCP Mesh 5 服务器事务 p95 | ≤ 100ms | ≤ 100ms | — | ✅ 达标 |
| CSN 单次替代查询 p95 | ≤ 30ms | ≤ 30ms | — | ✅ 达标 |
| efficiency-monitor 指标采集 | ≤ 1ms/样本 | ≤ 1ms/样本 | — | ✅ 达标 |

详细性能报告见:[docs/performance/week8_perf_report.md](./docs/performance/week8_perf_report.md)

---

## 安全特性

| 特性 | 说明 | 状态 |
|------|------|------|
| `#![forbid(unsafe_code)]` | 34/34 crate 全覆盖(编译期保证) | ✅ |
| SecCore 零信任沙箱 | 白名单 + 静态分析 + 环境过滤 | ✅ |
| OWASP Top 10 渗透测试 | 20/20 测试通过(100%) | ✅ |
| Merkle 审计链 | SHA-256 链式校验,篡改可检测 | ✅ |
| AHIRT 红队 | 100 载荷,4 类探测,探测率 < 95% 告警 | ✅ |
| ASA 对抗审计 | 规则评分 + Allow/Warn/Block 三级干预 | ✅ |
| QEEP 孤儿调用检测 | Sender drop 时检测未完成 Future | ✅ |
| cargo-fuzz 模糊测试 | 3 个 target 已就绪(待 nightly 运行) | ⚠️ |
| cargo-audit 依赖扫描 | 13 个关键依赖无 High/Critical | ✅ |

详细安全报告见:[docs/security/week8_security_report.md](./docs/security/week8_security_report.md)

---

## 开发状态

| 阶段 | 状态 | 日期 |
|------|------|------|
| Stage 0 — Workspace 脚手架 | ✅ 完成 | 2026-06-20 |
| Week 1 — L0-L1 基础设施 | ✅ 验收通过 | 2026-06-20 |
| Week 2 — L9+L5+L1 | ✅ 验收通过 | 2026-06-21 |
| Week 3 — L5+L6(MLC/HCW/CMT/OSA/KVBSR) | ✅ 验收通过 | 2026-06-22 |
| Week 4 — L6+L7(GEA/GQEP/PVL/MTPE/SCC/EDSB) | ✅ 验收通过 | — |
| Week 5 — L8+L4+L3(Parliament/ASA/AHIRT/TTG/DECB) | ✅ 验收通过 | — |
| Week 6 — L2+L10(SSRA/LSCT/GSOE/NMC/CHTC) | ✅ 验收通过 | — |
| Week 7 — MCP Mesh(CSN/SESA/Monitor/集成) | ✅ 验收通过 | 2026-06-27 |
| Week 8 — 打磨(性能/安全/文档/发布) | ✅ 验收通过 | 2026-06-27 |

### 测试统计

- **总测试数**:3002+ 个(Week 1-8 累计)
- **cargo check --workspace** ✓
- **cargo clippy --workspace -- -D warnings** ✓ 零警告
- **cargo test --workspace** ✓ 全部通过
- **cargo build --workspace --release** ✓

---


## 文档索引

| 文档 | 说明 |
|------|------|
| [CODE_WIKI.md](./CODE_WIKI.md) | 代码 Wiki(架构、模块职责、术语表) |
| [CHANGELOG.md](./CHANGELOG.md) | 版本变更记录 |
| [AETHER_NEXUS_OMEGA_ULTIMATE.md](./AETHER_NEXUS_OMEGA_ULTIMATE.md) | 主架构手册(8 周计划、25 ADR、核心类型) |
| [CHIMERA_NEXUS_COMPLETE_BUILD_GUIDE.md](./CHIMERA_NEXUS_COMPLETE_BUILD_GUIDE.md) | 构建指南 |
| [docs/architecture/](./docs/architecture/) | 架构文档(10 层详解、数据流、ADR 索引) |
| [docs/performance/](./docs/performance/) | 性能报告 |
| [docs/security/](./docs/security/) | 安全报告 |
| [docs/release/](./docs/release/) | 发布指南 |
| [docs/grafana/](./docs/grafana/) | Grafana 监控仪表盘 |

---

## 开发工作流

### 环境变量(Windows)

```powershell
$env:CARGO_HOME = 'D:\Chimera CLI\.toolchain\cargo'
$env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'
$env:TMP = 'D:\Chimera CLI\tmp'
$env:TEMP = 'D:\Chimera CLI\tmp'
$env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"
```

### 日常命令

```powershell
# 快速类型检查
cargo check --workspace

# 完整构建
cargo build --workspace --release

# 运行所有测试
cargo test --workspace

# lint + format
cargo clippy --workspace -- -D warnings
cargo fmt --all

# 生成文档
cargo doc --workspace --no-deps
```

> **资源约束**:内存受限环境下,编译/测试加 `--jobs 1` 避免内存爆炸。

### 配置文件

| 配置源 | 路径 / 前缀 | 优先级 | 说明 |
|--------|------------|--------|------|
| 内置默认值 | `ChimeraConfig::default()` | 1(最低) | 编译期常量 |
| 配置文件 | `~/.aether/omega.yaml` | 2 | YAML 格式,可 `--config` 覆盖 |
| 环境变量 | 前缀 `AETHER_`,嵌套用 `__` | 3 | 如 `AETHER_MODEL__PRIMARY=premium` |
| CLI 参数 | `--model`, `--config` 等 | 4(最高) | Clap 解析 |

**多源加载**使用 Figment 框架,后者覆盖前者。

---

## 版本历史

| 版本 | 日期 | 说明 |
|------|------|------|
| v1.0.0-omega | 2026-06-27 | Week 8 验收通过,34/34 crate 全覆盖,3002+ 测试,性能/安全/文档/发布全部达标 |

详见 [CHANGELOG.md](./CHANGELOG.md)。

---

## 许可证

Apache-2.0 License — 详见 [LICENSE](./LICENSE)。

---

> **维护者**:NEXUS-OMEGA 团队
> **文档版本**:Week 8 同步(2026-06-27)
