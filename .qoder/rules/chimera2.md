---
trigger: always_on
---
> **[CONFIG-REFACTOR-20260920] 本文件为派生副本（配置收口）**
> 版本、crate 数、测试基线、NexusEvent 变体数等**计数不再在此登记**，避免与代码库漂移。
> 唯一权威源：`Cargo.toml`（`workspace.package.version` / `workspace.members`）、`Cargo.lock`（依赖**解析后版本**唯一权威源）、`CHANGELOG.md`（版本演进）、
> `docs/architecture/CODE_WIKI.md §11` + `docs/architecture/adr_index.md`（ADR）、
> `docs/reports/`（最新回归报告）、`crates/event-bus/src/types.rs`（事件清单）。
> 需要具体数值时按权威源现取；本文件的历史数值段落不可作为决策依据。

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Chimera CLI (NEXUS-OMEGA) 项目指引

> 本文件面向未来操作本仓库的 Claude Code 实例。
>
> - **全局协作偏好**（语言、代码风格、Git 约束、工具使用偏好）见用户级 `C:\Users\30324\.claude\CLAUDE.md`。
> - **架构硬约束、依赖铁律、async/SQLite/安全红线**见 `.trae/rules/nuxus规则.md` §2、§4、§5、§6(速查版)+ `AGENTS.md` §2/§4/§6(完整版)。
> - **版本演进权威源**见 `CHANGELOG.md`;**架构权威源**见 `docs/architecture/CODE_WIKI.md`(完整 crate 索引/核心类型/事件系统/依赖铁律,已与 `Cargo.toml` 对齐;MCA 体系 L10 `mca-gateway`,ADR-065~068,经 ADR-177 为长期 GATED;Concord TUI 重构 **W0~W11 波次收口**——v2.26 完成首批波次、整体于 v2.27 工程收口,10 份 ADR-074~083;部分子项未接线,逐项现状以 `docs/tui/README.md` 为准)。`CHANGELOG.md` 含 v1.0.0 起完整汇总(尾号以文件现取,2026-10-11 在途 [2.29.0-omega]) + ADR 摘要(ADR 登记数以 `docs/architecture/adr_index.md` 与 CODE_WIKI §11 为准,**不写编号上限,需要时现取**)。
> - **架构文档索引**见 `docs/architecture/README.md` 与 `docs/architecture/INDEX.md`(分类索引,条目与分类数以该文件自身为准,链接指向现存的 CODE_WIKI / ADR 等文档)。
> - **协作流程权威源**:分支/提交/PR/标签发布(含 GPG 条件签名与 tag 重指补救)以仓库根 [`CONTRIBUTING.md`](../../CONTRIBUTING.md)(2026-10-11 立)为唯一权威源。
>
> **当前基线(2026-10-11 核验)**:`Cargo.toml` `workspace.package.version` = **2.29.0-omega**(2026-09-27 本地发布、tag 未 push;上一已发 tag v2.28.2-omega = 2026-09-08);`CHANGELOG.md` 最新条目为 2.29.0-omega;crate 数 **39**(以 `Cargo.toml` `workspace.members` 为准——`router-traits` 经 ADR-193 整 crate 退役、`lsct-tiering` 整并入 `cmt-tiering`,更早批次 A 物理删除 `auto-dpo`/`model-router`);**ADR-222(2026-10-20)后上值不再为现势:`nexus-contracts` 拆为 1+6 + 新建 `nexus-telemetry`,现取 46**;`NexusEvent` 变体 **152**(`crates/event-bus/src/types.rs`,基线 `scripts/doc_count_freeze.txt`,旧值 144/145/146/151 均已过期);测试基线见 `docs/reports/` 最新回归报告;项目当前焦点:**R2 解冻影子期(ADR-053 rev4 + ADR-054 治理签署)+ MCA 双轨验证(`--features mca`,`mca-gateway` 长期 GATED)+ 可达性棘轮(2026-10-02 实测 30 生产可达 / 0 冻结孤岛 / 9 GATED)+ ★ RL 开发闸门(Rust-First:Python 训练服务仅规划、禁止实施)**

## 1. 环境设置

仓库使用项目内 Rust 工具链。Windows 下首次配置：

```powershell
$env:CARGO_HOME = 'D:\Chimera CLI\.toolchain\cargo'
$env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'
$env:TMP = 'D:\Chimera CLI\tmp'
$env:TEMP = 'D:\Chimera CLI\tmp'
$env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"
```

或一次性写入用户环境变量：运行 `install.ps1 -SetupEnv`（执行后需重启终端）。

> **工具链一致性**：`.cargo/config.toml` 已固化以下关键配置，因此日常开发**无需**再手动设置 `CARGO_INCREMENTAL` 或 linker：
>
> - `target.x86_64-pc-windows-gnu` 的 linker 是 `linker = "gcc"`（`.cargo/config.toml:6`）——**刻意用 PATH 解析，不是硬编码绝对路径**：`D:/msys64/mingw64/bin` 由上面 §1 的 env 块或 `install.ps1 -SetupEnv` 注入 PATH。写死绝对路径会让 `release.yml` 的 Windows runner 链接失败（该 runner 无 D 盘；理由逐字见 `.cargo/config.toml:2-5` 的 WHY 注释）
> - `[build] incremental = false`（根治 rustc 1.96.0 + GNU 的 ICE）
> - `[build] jobs = 4`（`.cargo/config.toml:28`，2026-09-04 工具链诊断的根因修复：15GB 机型上默认 jobs=16 会把内存打满并触发 rustc/链接器 fast-fail `0xC0000409`）
> - `[env] SQLITE_ENABLE_FTS5 = "1"`（启用 repo-wiki 的 FTS5 全文索引）
>
> **本仓库刻意不提供 `rust-toolchain.toml`**——它是全局单值配置，在 MSVC 宿主上写 `stable` 会展开成 MSVC 令 `linker=gcc` 失效，写死 `stable-x86_64-pc-windows-gnu` 又会让 `release.yml` 的 Linux/macOS runner CI 失败。GNU channel 由 `install.ps1 -SetupEnv`（项目本地 `rustup default stable-x86_64-pc-windows-gnu`）保证。若出现 channel 不一致，应重跑 `install.ps1 -SetupEnv`（或手动 `rustup default stable-x86_64-pc-windows-gnu`），**不要**新增/恢复 `rust-toolchain.toml`。

若 D 盘空间不足，可重定向构建临时文件：

```powershell
$env:CARGO_TARGET_DIR = 'E:\chimera-target'
$env:TMP = 'E:\chimera-tmp'
$env:TEMP = 'E:\chimera-tmp'
```

## 2. 常用命令

### 构建与运行

```powershell
# 快速类型检查
cargo check --workspace

# 只检查单个 crate
cargo check -p <crate-name>

# Release 构建（产物为 target/release/chimera.exe / chimera,与 CLI 命令入口一致）
cargo build --workspace --release

# 直接运行 CLI
cargo run -p chimera-cli -- --version
cargo run -p chimera-cli -- wiki "查询内容"
cargo run -p chimera-cli -- run "一次性代码生成/问答提示词"   # 注意：无 generate 子命令（旧文档误载，已实测确认）
cargo run -p chimera-cli -- --config ~/.chimera/omega.yaml wiki "查询"
```

### 测试

```powershell
# 全量测试(含全部 crate 的单元/集成测试与根 package `chimera-e2e-tests` 承载的 E2E/安全/压测/控制闭环 target;crate 数与 target 清单以 `Cargo.toml` 为准)
cargo test --workspace --no-fail-fast   # 默认 fail-fast 会截断 target(R2 M56)：67/2327 vs 真全量 447/11375

# 单 crate
cargo test -p <crate-name>

# 单测试用例
cargo test -p <crate-name> <test-name>

# 显示测试输出
cargo test --workspace --no-fail-fast -- --nocapture

# 压力测试（#[ignore] 标记）
cargo test -- --ignored --nocapture

# 全部可单独跑的 target 名以根 `Cargo.toml` 的 [[test]] 注册为唯一权威源(不在本文登记清单与计数)。
# 常用示例见 .claude/CLAUDE.md §2「测试」段;新登记 target 只改 Cargo.toml,勿回填本文。
```

### Lint、格式与文档

```powershell
# clippy（Windows 下建议限制并行 jobs 并关闭增量，避免 OOM/ICE）
$env:RUST_MIN_STACK = '33554432'
$env:CARGO_INCREMENTAL = '0'
cargo clippy --workspace --all-targets --jobs 2 -- -D warnings

# 格式化
cargo fmt --all -- --check   # 检查
cargo fmt --all              # 应用

# 文档
cargo doc --workspace --no-deps
```

### 基准与安全审计

```powershell
# 单个 crate 的 criterion 基准
cargo bench -p <crate-name>

# 安全审计（唯一权威口径 = .github/workflows/audit.yml 的 `Run security audit` 步骤；本地不得另立标准。
# 三处命令串逐字等价由 scripts/audit_cmd_sync.py 自证，见 agents.md §7.2 与 ADR-167 决策 4）
cargo audit --deny unmaintained --deny unsound `
  --ignore RUSTSEC-2026-0190 --ignore RUSTSEC-2026-0002 --ignore RUSTSEC-2026-0253 `
  --ignore RUSTSEC-2024-0436 --ignore RUSTSEC-2025-0141 --ignore RUSTSEC-2025-0119

# fuzz 配置静态检查（Windows GNU 无法实际运行，fuzz 委托 Linux CI）
cargo check --manifest-path fuzz/Cargo.toml
```

### Docker

```powershell
docker build -t chimera-cli:local .
docker run --rm chimera-cli:local --version
```

## 3. 架构总览

项目是基于 Rust 2021 + Tokio 的 workspace。`Cargo.toml` 的 `workspace.members` 为 crate 数权威源(2026-10-11 现取 39;v2.0.0-omega 新增 `chimera-mas` @ L9;v2.4.0 新增 `nexus-contracts` @ L0 + `omega-learner` @ L6;v2.22.0 新增 `mca-gateway` @ L10;v2.28 新增 5 成员至 43;ADR-193 架构减法——批次 A 物理删除 `auto-dpo`/`model-router`,其后整 crate 退役 `router-traits`、`lsct-tiering` 整并入 `cmt-tiering`,41→39;**ADR-222 边界重裁(2026-10-20)——`nexus-contracts` 拆为 1+6 契约 crate + 新建 `nexus-telemetry`,39→46,此值为该时点现取,现值仍须以 `workspace.members` 为准**)。

### 11 层架构(L0 Contracts + L1-L10)

> 架构硬约束、ADR 权威源与依赖铁律的详细规则见 `.trae/rules/nuxus规则.md` §2 + `AGENTS.md` §2。本节仅作快速导航。
>
> **ADR-222 双层口径**:`L0 = {L0-A, L0-B}` 是**层名**分层,机器层号 0/1 **不重排**(116 份 ADR 的历史引用不失真)。L0-A = 纯契约 7 crate(层号 0,G-99 判据原样保留),L0-B = 核心基础面 3 crate(层号 1,另立较宽门);方向铁律由 G-125/G-126(`scripts/check_l0_sublayer.py`)机器守护:L0-A 不得指向 L0-B、L0-B 不得指向层号 ≥ 2。

```text
L0   Contracts ─ nexus-contracts · nexus-contracts-core · nexus-contracts-sparse · nexus-contracts-safety · nexus-contracts-seams · nexus-contracts-mca · nexus-contracts-memory  (L0-A 纯类型+受控纯逻辑,口径见 ADR-033 追加节 2026-10-09;1+6 拆分见 ADR-222 D2)
L10  Interface ─ chimera-cli · chimera-tui · chtc-bridge · mcp-mesh · csn-substitutor · mca-gateway · nexus-app-server [ADR-065]
L9   Quest ───── quest-engine · gea-activator · efficiency-monitor · chimera-mas · mas-sched · nexus-hook
L8   Parliament ─ parliament · decb-governor
L7   Execution ── pvl-layer · gqep-executor · mtpe-executor · ssra-fusion · nexus-subagent
L6   Router ───── osa-coordinator · kvbsr-router · faae-router · sesa-router · omega-learner [ADR-031]
L5   Knowledge ── repo-wiki · gsoe-evolution
L4   Security ─── seccore · qeep-protocol · decay-engine
L3   Storage ──── scc-cache · cmt-tiering · session-store
L2   Memory ───── nmc-encoder · hcw-window · mlc-engine
L1   Core ─────── nexus-core · event-bus · nexus-telemetry  (L0-B 核心基础面;ADR-222 D1:层名与层号分离,本行仍是机器层号 1)
```

### OMEGA 十一定律(权威定义源:`docs/architecture/CODE_WIKI.md` §1.2 + ADR-170（旧引文件经 2026-09-21 全库检索确认不在盘）)

> Ω₁~Ω₉ 为架构基座与 v2.x 学习/进化体系,Ω₁₀/Ω₁₁ 为经验卡片与按需记忆合成扩展(ADR-170 收录);十一定律全部已有代码落地与 E2E 验证(2026-10-11 核验)。

| 定律         | 工程实现                    | 关键 crate                    |
| ---------- | ----------------------- | --------------------------- |
| Ω₁-Sparse   | 全维稀疏掩码（工具/上下文/记忆/审计/预算） | `osa-coordinator`           |
| Ω₂-Compress | 四级窗口 + Mem-π 生成式记忆       | `hcw-window`、`mlc-engine`   |
| Ω₃-Evolve   | AEGIS 四阶段 + GRPO 风格进化 + 变体隔离 | `gsoe-evolution`、`chimera-mas` |
| Ω₄-Event    | 事件驱动架构(broadcast + Critical mpsc) | `event-bus`            |
| Ω₅-Credit   | SHARP Shapley 值精确信用分配      | `parliament`                 |
| Ω₆-Reuse    | 奖励函数优化技能复用率             | `repo-wiki`、`csn-substitutor` |
| Ω₇-Locate   | L1→L2→L3 行为定位导航修改点        | `parliament`                 |
| Ω₈-Assess   | Runtime Auditor 五维度证据纪律  | `efficiency-monitor`         |
| Ω₉-Preserve | 变体隔离 + 停止策略保留历史最佳 | `parliament`(ADR-051) + `gsoe-evolution` + `chimera-mas`(INV-8/9) |
| Ω₁₀-Card | 经验卡片:不可变 + 版本化 + append-only 事件流 | `event-bus`、`nexus-contracts`、`mlc-engine`、`cmt-tiering` |
| Ω₁₁-Synthesize | 按需记忆合成:懒加载合成 + 不阻塞主流程 + 错误签名级兄弟检索 | `mlc-engine`、`event-bus` |

### 依赖铁律

```text
L(N) → L(N)     ✓ 同层互引允许
L(N) → L(N-1)   ✓ 向下依赖允许
L(N) → L(0)     ✓ L0 Contracts 恒允许(ADR-033)
L(N) → L(N+1)   ✗ 向上依赖禁止
L(N) ──event-bus── L(M)  ✓ 跨层通信只能走 Event Bus
L(N) ──mcp-mesh─── L(M)  ✓ 跨进程通信只能走 MCP Mesh
```

- `nexus-core` 必须保持最小依赖，不能 import 上层任何 crate。
- 所有 crate 顶层均声明 `#![forbid(unsafe_code)]`；依赖库中的 unsafe 不破坏本项目 crate 的编译期保证。
- `dev-dependencies` 中的测试代码可绕过生产依赖方向，但仅限 `tests/` 目录。
- 任何违反依赖方向的 import 必须被拒绝，除非有 ADR 记录特批。

### 核心类型

- `CLV`（`nexus-core`）：512 维 f32 上下文潜在向量，NMC 编码器输出，被路由/记忆/激活组件消费。
- `NexusEvent`（`event-bus`）：跨层事件枚举，Critical 级事件无订阅者时记录 warn。
- `UserIntent` / `Quest` / `NexusState` / `OmniSparseMasks`：核心领域类型，修改需 ADR 记录。

### 端到端数据流示例

```text
用户输入 → NMC 编码(L2)
  → Quest 分解 / TTG 切换(L9)
  → Parliament 审议(L8) → Skeptic 否决检查
  → PVL 生产验证(L7)
  → OSA 协调(L6) → KVBSR/SESA 路由(L6)
  → GEA 激活(L9)
  → MTPE 多步预测 / GQEP 聚集执行(L7)
  → QEEP 纠缠(L4)
  → Wiki 沉淀 / GSOE 进化 / Auto-DPO(L5)
  → Event Bus 广播(L1)
```

> ⚠ 上表 `Auto-DPO(L5)` 的原 crate `auto-dpo` 已于批次 A（`38c72e6`，2026-09-20）物理删除（唯一登记表：`docs/architecture/CODE_WIKI.md §3.12`）；`DpoPairGenerated` 事件契约仍在（`crates/event-bus/src/types.rs`），故事件面未断，但不要再按 L5 crate 去找它。

### 前瞻性架构参考(第三阶段演进方向)

> 以下 5 份设计文档(1 份在仓库根,4 份在 `docs/architecture/_blueprints/`,落点权威源 `docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md` §4.3)为第三阶段模块级优化的**前瞻性参考**,完整规则见 `.trae/rules/nuxus规则.md` §3.4.5 + `AGENTS.md` §3.4.5。

| 文档                                         | 核心价值                            | 落地状态(2026-10-11 更新)             |
| ------------------------------------------ | ------------------------------- | -------------------------------- |
| `CHIMERA_NEXUS_OMEGA_完整对话摘要.md`            | 项目演进全景记录(6 轮 × 427,000+ 字)      | ✅ 元文档,记录已发生过程 |
| `docs/architecture/_blueprints/mas-quadrant/CHIMERA_MULTI_AGENT_四象限协同工作系统_系统性设计文档.md` | MAS-Q 设计蓝图(四象限 + 三层委托 + PDCA)   | ✅ **`chimera-mas` 全栈已实现**(v2.0.0~v2.2.0 ADR-026~028 闭环);INV-7/8/9 已生效 |
| `docs/architecture/_blueprints/caf-router/CHIMERA_全球大模型亲和系统_终极设计文档.md`              | CAF 渠道亲和(4 渠道 × 60+ 模型)         | ✅ **MCA 体系已落地**(v2.21.0 `chimera llm` + v2.22.0 token 优化 + v2.25.0 `chimera grep`;ADR-065~068);`mca-gateway` 部署档类型 `DeploymentProfile`(`crates/mca-gateway/src/spec_loader.rs:88`)与 `ProfileMeta`(`:59`);旧述 8 个 affinity profile 所指 AffinityProfile 枚举在码内不存在。**`mca-gateway` 自 2026-09-03 起经 ADR-177 重分类为长期 GATED**,经 chimera-cli `optional` 边 + `--features mca` 装配,不是孤岛 |
| `docs/architecture/_blueprints/triple-paradox/AI_Agent_三重悖论_x_Chimera_深度映射分析.md`        | 三重悖论免疫(记忆/推理/进化)                | ✅ **形式化验证器 M0/M1/M2 全部落地**(v2.13.0~v2.20.0,9 属性),L4 验证器跃迁完成;R2 解冻前置就绪,**影子期等待治理签署**(ADR-053 rev4);★ RL 开发闸门持续有效——Python 训练服务仅规划、禁止实施 |
| `docs/architecture/_blueprints/three-ring-reorg/三环循环_十层接口_元架构重组深度分析.md`                   | 内环(9 crate) + 外环 crate 重组方案 | ✅ **Phase 9 全部交付**(v2.24.0 P9-T1~T12),7 份报告归档;ADR-054 落档;依赖铁律**双源现为绿**——ADR-187(2026-09-24)已从 `check_dependency_rules.{sh,ps1}` 双侧退役内环 Check A,`gsoe-evolution→decay-engine` 为 L5→L4 合法向下边(`scripts/dep_edge_freeze.txt`);环分区为 **NOT-ENFORCED 设计意图**(2026-09-21 曾测 EXIT=1 的 [GAP-A] 结论已过期),外环→内环棘轮门 G-75/G-76(`scripts/check_outer_inner_ratchet.py`)只防增长 |

**三环重组核心约束(前瞻)**:

> ⚠ **STATUS: NOT-ENFORCED (ADR-187, 2026-09-24)**:以下环分区为设计意图,非可执行门。原唯一强制点 Check A(`is_inner_ring()`/`is_inner_base()`)已从 `check_dependency_rules.{sh,ps1}` 双侧退役删除;「内环 9 / 外环 25」为 v2.20 设计时点数字(现值不追),外环→内环方向由棘轮门 G-75/G-76 只防增长。

- 内环 9 crate(记忆+推理+进化)使用共享内存+直接调用,延迟 <1ms(v2.20 设计时点清单,其中 `auto-dpo` 已物理删除,见 CODE_WIKI §3.12)
- 外环 crate 保持事件驱动,只能通过 `CoreLoopEvent` 与内环通信
  > ⚠ **`CoreLoopEvent` 是设计名,当前代码零载体**(2026-09-22 实测:`crates/**/*.rs` 中 `CoreLoopEvent` 与
  > `inner_ring`/`InnerRing` 均 **0 命中**)。曾作为内环边界唯一强制点的 Check A 治理脚本函数
  > `is_inner_ring()`/`is_inner_base()`(曾位于 `scripts/check_dependency_rules.sh`,据此报 `[GAP-A]`)
  > **已随 ADR-187 从双源删除**,环分区自此无任何机器强制(ENFORCEMENT: none);
  > 复活 Check A 须满足 ADR-187 三前置条件,引入该类型属**待实现**,须另立 ADR。
- **关键约束**:内环 crate 不能依赖外环 crate;外环 crate 只能通过 event-bus 依赖内环公开接口
- (v2.20 诊断时点)架构曾存在 4 类病理:星型耦合(L6 Router 5 crate 都依赖 osa-coordinator)、跨层渗透(L7 gqep-executor 依赖 L4 qeep-protocol)、隐式循环依赖(L5 GSOE ↔ L9 Quest 通过 Event Bus 逻辑循环)、L1 上帝 crate(nexus-core/event-bus 被所有成员 crate 依赖)。**处置状态**:v2.24.0 Phase 9(P9-T1~T12)已完成三环重组收尾——星型耦合经 L0 `nexus-contracts` 解耦、2 条生产违规边消除、D3/D4 病理消解;**残余跨层渗透仅 ADR-048/185 收编的 gqep→qeep 一条合法向下边**,ADR-187 退役 Check A 后双源均绿;crate 总数不在此登记,以 `Cargo.toml` `workspace.members` 现取

## 4. 关键工程约定

- **库层 vs 应用层错误**：库层使用 `thiserror` 自定义 enum(计数不登记,以 `Get-ChildItem crates -Recurse -Filter error.rs` 递归现取：**2026-10-11 实测 37 个 / workspace 成员 39 个**（**2026-10-20 复测仍为 37 个，但成员已 46 个**——ADR-222 新增的 8 个 crate 均为纯契约/遥测层，无 `src/error.rs`，故缺该文件的成员由 4 个变为 10 个）——37 含 `lsct-tiering` 整并入后的 `crates/cmt-tiering/src/lsct/error.rs`;无 src/error.rs 的成员错误类型内嵌或属纯类型契约层,缺员清单不抄写;旧「37/41、缺 router-traits 等 4 个」口径已过期)。全部为 thiserror;`chimera-mas` 的 `MasError` 33 变体 + 2 象限约束变体;CLI/应用层使用 `anyhow`。
- **配置加载**：`chimera-cli` 通过 Figment 合并**三个有效源**，优先级由低到高（权威源 `crates/chimera-cli/src/config.rs:219-221`）：
  1. 内置默认值（`Serialized::defaults(ChimeraConfig::default())`）
  2. YAML 配置文件（`Yaml::file(path)`；默认 `~/.chimera/omega.yaml`，`--config` 只改变该路径）
  3. 环境变量 `CHIMERA_*`（嵌套字段用 `__`,例如 `CHIMERA_MODEL_ROUTER__STRATEGY=CostOptimized`；段名须取自真实 14 段，清单与行号见 `.claude/CLAUDE.md` §4 同条）

  > ⚠ **CLI 参数不进 Figment**（`config.rs:216-218` 注释自陈，2026-09-22 按代码更正）：旧版此列的第 4 源"CLI 参数（最高优先级）"
  > 与实现不符，已删 ⇒ 想覆盖配置现状只有 `CHIMERA_*` 一条路。样例文件的坑（`.yaml` 是 9 字节空壳、`.toml` 无 provider 且落地路径不对）
  > 与模板落盘触发点（`chimera config init` / `chimera doctor`，**非**首次运行自动写）详见 `.claude/CLAUDE.md` §4 同节与 `docs/reports/DOC-AUDIT-FACTS_2026-09-20.md §13`。
- **二进制名称**：`crates/chimera-cli` 的产物名为 `chimera`(与 CLI 命令入口一致);本地直接运行请使用 `cargo run -p chimera-cli` 或 `target/release/chimera`。
- **占位实现**：
  - **v1.0.0-omega 起**:MTPE 伪预测 / SecCore ASA 规则式评分 / GSOE 规则式进化策略 / RepoWiki placeholder embedding / SCC InMemoryWal
  - **v1.x 完善**:NMC image/video/audio 感知器已实现
  - **v2.0.0-omega**:chimera-mas 3 子模块(`orchestrator` / `agent` / `context`)+ AgentTask wrapper(包装 `nexus_core::Task` 不修改核心类型)
  - **v2.1.0-omega**:chimera-mas 四象限 `Quadrant` 模型(Q1-Q4)+ WSJF `PriorityScheduler` + `ExpertRegistry`(E01-E08 内置)
  - **v2.2.0-omega**:chimera-mas Part II 7 子模块(§15-§21,INV-7 上下文预算界 / INV-8 归档单调性)+ 5 项 criterion bench
  - 替换为生产实现时不得破坏既有接口契约。

## 5. CI/CD 与发布

- **协作流程权威源**:分支模型 / Conventional Commits 提交类型 / PR 合并 / 标签发布(含 GPG 条件签名与 tag 重指补救)以仓库根 [`CONTRIBUTING.md`](../../CONTRIBUTING.md)(2026-10-11 立)为唯一权威源;本节仅列 CI/CD 与发布技术要点。
- **Tag 触发**:推送 `v*.*.*-omega` 标签自动触发 `.github/workflows/release.yml`(5 平台构建 + GitHub Release)和 `fuzz.yml`(**8 target × 300s**,已与 `fuzz/Cargo.toml` 同步)。
- **Security Audit**:`.github/workflows/audit.yml` 在改动 `Cargo.lock`/`Cargo.toml` 的 PR 上触发,并每日 UTC 02:00 运行。本地使用与 CI 一致的命令(见 §2"基准与安全审计")。
- **版本号同步**:发布 `vX.Y.Z-omega` 前,`Cargo.toml` 的 `workspace.package.version` 必须同步为 `X.Y.Z`,且 `CHANGELOG.md` 必须存在对应的 `vX.Y.Z-omega` 汇总章节。**v2.3.0 → v2.3.1-omega 教训**:commit message 编码损坏时(见 agents.md §10.3a 或 .qoder/rules/chimera.md §10.3a;速查档 nuxus规则.md 无此节)不要 `--amend` / `push --force`,新 tag 递增 patch 即可。

### 发布前检查清单

1. 确认 `Cargo.toml` 的 `workspace.package.version` 与待发布 tag 一致。
2. 确认 `CHANGELOG.md` 已存在对应的 `vX.Y.Z-omega` 汇总章节。
3. `cargo check --workspace`
4. `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings`
5. `cargo fmt --all -- --check`
6. `cargo test --workspace --no-fail-fast`
7. `cargo test --workspace --release --no-fail-fast -- --ignored --test-threads=1 --nocapture`（**release + 单线程必需**：`#[ignore]` 基准测试含性能阈值，debug 模式开销 + 并行竞争会污染延迟测量导致 flaky 误报;与 `agents.md` §7.2 逐字对齐）
8. `cargo audit --deny unmaintained --deny unsound --ignore RUSTSEC-2026-0190 --ignore RUSTSEC-2026-0002 --ignore RUSTSEC-2026-0253 --ignore RUSTSEC-2024-0436 --ignore RUSTSEC-2025-0141 --ignore RUSTSEC-2025-0119`
9. `cargo check --manifest-path fuzz/Cargo.toml` — Windows-GNU 下靠 `fuzz/src/lib.rs` 的 stub 宏验证 fuzz 逻辑语法：`#[cfg(windows)] use chimera_fuzz::fuzz_target` + `#[cfg(not(windows))] use libfuzzer_sys::fuzz_target`。**✅ 2026-09-23 实测 EXIT=0（`Finished dev profile in 1m 04s`，0 error）**：原阻断项（悬空 path 边 + target 内 `use` 已删类型）已一并消化，`cacr_budget_parse` target 的三个模糊目标整体迁到活体后继面 `mca-gateway`（`spec_loader` 两个 TOML 入口 + `cost_guard::CostGuard`），`[[bin]]` 仍 8 个故 `fuzz.yml` matrix 未动；详见 `docs/reports/SAB-B0-2-impl-maturity-gate-2026-09-23.md §14`，`agents.md` §10.3 同条目已于 2026-09-24 同步。可选再跑 `scripts/check_fuzz_config.ps1`（Windows）/ `scripts/check_fuzz_config.sh`（Linux/macOS）静态验证配置完整性
10. `docker build -t chimera-cli:rc .`
11. `docker run --rm chimera-cli:rc --version` 输出匹配 `chimera [0-9]+\.[0-9]+\.[0-9]+`
12. `docker image inspect chimera-cli:rc --format '{{.Size}}'` 确认镜像体积 < 100 MB
    > **本地无 Docker 时的降级验证**：可运行 `scripts/verify_docker_locally.ps1`（Windows）或 `scripts/verify_docker_locally.sh`（Linux/macOS）。脚本自动检测 Docker → Podman → 降级验证（Dockerfile 静态检查 + release binary 体积 < 50MB 代理指标 + CI 状态查询引导）。完整镜像验证由 `release.yml` docker job 在 tag 推送时自动执行，详见 §6。
13. `cargo build --workspace --release` 确认 binary 体积 < 50 MB
14. `git tag v<x.y.z>-omega && git push origin v<x.y.z>-omega`

> 规则级发布要求与详细说明见根 `agents.md` §7.2(本清单与其逐项对齐);**分支/提交/PR/标签发布的完整流程(第 14 步 tag 的 GPG 条件签名、tag 重指补救等)以 [`CONTRIBUTING.md`](../../CONTRIBUTING.md) §6 为唯一权威源**(2026-10-11 立)。

## 6. 本地无法运行的限制

- **cargo-fuzz**：libFuzzer 的 Windows 适配依赖 MSVC，本项目 Windows 环境使用 MinGW，因此 fuzz 只能委托 Linux CI。`fuzz/Cargo.toml` 声明了 8 个 fuzz target（quest\_parse/seccore\_sandbox/event\_serialize/cacr\_budget\_parse/checkpoint\_deserialize/config\_section\_parse/harness\_spec\_parse/sse\_parse），`fuzz.yml` CI 已同步至 8 target。Windows-GNU 下通过 `fuzz/src/lib.rs` stub 宏方案（`chimera_fuzz::fuzz_target` 替代 `libfuzzer_sys::fuzz_target`）使 `cargo check --manifest-path fuzz/Cargo.toml` 能通过语法验证。可选运行 `scripts/check_fuzz_config.{ps1,sh}` 静态验证配置完整性。实际 fuzz 运行委托 Linux CI（`.github/workflows/fuzz.yml`）。
- **Docker 镜像构建**：本地未安装 Docker Desktop 时，发布前检查清单第 10-12 项的镜像验证可降级为 `scripts/verify_docker_locally.ps1`（Windows）/ `scripts/verify_docker_locally.sh`（Linux/macOS）。脚本三级降级：Docker → Podman → Dockerfile 静态检查 + binary 体积代理 + CI 引导。完整镜像验证由 `release.yml` docker job 在 tag 推送时自动执行。
- **CI Windows GNU 构建**：`release.yml` 的 Windows job 使用 `windows-latest` + GNU target，但 runner 默认没有 `D:/msys64/mingw64/bin/gcc.exe`；若 Windows release build 链接失败，需要检查是否已安装 MinGW 或改用与本地一致的环境验证。

## 7. 合并/大改后的自检与 `check_errors*.txt`

仓库根目录可能出现 `check_errors*.txt`（如 `check_errors.txt`、`check_errors2.txt`、`check_errors3.txt`）。这些文件是历次 `cargo check --workspace` 的错误日志，没有元信息说明对应提交。处理原则：

- **提交前**：必须确保 `cargo check --workspace` 通过，不得遗留未归档的 `check_errors*.txt`。
- **出现** **`E0428`（重复定义）或** **`E0252`（重复导入）时**：优先检查是否把同一段代码粘贴了两次（常见于合并/大段移动）。
- **生成规范**：如需本地保存日志，使用带时间戳的文件名，例如 `check_errors_$(date +%Y%m%d_%H%M%S).txt`。

## 8. 必须遵守的编码红线(摘要)

以下规则的权威版与详细出处见 `.trae/rules/nuxus规则.md` §4、§6:

**async / 并发(Week 1-8 实战)**:

- 禁止持锁跨 `.await`;`rusqlite` 调用必须 `spawn_blocking`;`tokio::broadcast` 必须先 `subscribe` 再 `spawn`
- Top-K 必须 `select_nth_unstable` (O(n)),禁止 `sort_by` (O(n log n))
- 并发收集用 `FuturesUnordered` 而非 `join_all`

**内存与依赖**:

- 所有 crate 顶层 `#![forbid(unsafe_code)]`;依赖库 unsafe 不破坏当前 crate 保证
- 风险规则列表为空时返回 `RiskLevel::Unknown`;`PRAGMA journal_mode=WAL` 必须用 `pragma_update`
- u64 大数百分比计算用 `f64` 中间值;stable Rust 无 `AtomicF32`

**多治理器 / 多 Agent(v2.x 新增)**:

- 多治理器协同必须经 `ArbitrationLayer` 取保守值
- **Critical 事件**(2026-10-11:mpsc 旁路 **16 个**,权威源 `crates/event-bus/src/bus.rs` 的 `is_critical_mpsc_event()` matches 臂与常量 `CRITICAL_MPSC_VARIANTS`/`CRITICAL_TOTAL`,**勿引行号,会漂移**;`severity()`=Critical 共 20 个——另 4 个 `CheckpointSaved`/`ConsensusReached`/`SlowConsumerDropped`/`OrphanCallDetected` 为 broadcast-only)必须走 mpsc 旁路通道确保送达:`SkepticVeto` / `RedTeamAudit` / `BudgetExceeded` / `AgentTaskFailed` / `AsaIntervention` / `AffinityQuotaExhausted` / `R2FreezeViolation` / `R2FreezeRollbackFailed` / `FormalViolation` / `VetoOverridden` / `R1ShadowRollbackFailed` / `StopRulingIssued` / `ErrorSignatureMatched` / `FormalVerificationFailed` / `CertifyIssued` / `FailureRegistered`
- **INV-7 上下文预算界**:任意时刻 `m_total ≤ MEMORY_BUDGET_MB × MEMORY_BUDGET_UTILIZATION`(130MB × 0.9 = 117MB)
- **INV-8 归档单调性**:归档沿 Hot→Warm→Cold→Ice 单向降级,禁止逆向升级
- `chimera-mas` 任务委托深度 ≤ 5(`MAX_AGENT_DEPTH`)
- **INV-9 委托图无环**:chimera-mas 任务委托图必须为 DAG,违反即 R2FreezeViolation 事件

**降级路径**:

- 降级/耗尽路径必须经**已存在的活体事件**发布可观测信号：规则/风险告警走 `EfficiencyAlertTriggered`（`efficiency-monitor` `monitor.rs` 的 `publish_rule_alert`/`publish_paradox_risk_alert`/`publish_critical_alert` 真实发布），预算耗尽走 `BudgetExceeded`（Critical+mpsc）。`MemoryNearBudget`/`ExpertOverload`/`ArchiveIoContention` 是 `chimera-mas/src/stability.rs:157` 的**内部 `enum PressureSource`**（降级链输入分类，非 `NexusEvent` 变体；`event-bus` 无这三名）。**红线经 ADR-210 Option C 与现网对齐、自 2026-10-11 起可满足（由上列活体事件承载）。未来若要把压力源升为一等总线事件，前置=先给 `DegradationChain::apply()` 接生产调用方（现无）**，再按七点位同批（含 `variant_count_test` 计数锁与 `doc_count_freeze.txt`）新增变体——不得先加零生产者装饰变体。**

**Concord TUI 重构 W0~W11(新增红线)**:

- **面板数不在此登记**（旧记「22 面板」为首批波次时点、“27 变体 / 焦点环 26”为 2026-09-21 时点，均已陈旧；**2026-10-08 现测**:`PanelId` 枚举 30(= `ALL`)/ `REGISTERED_FOCUS_ORDER` 焦点环 29,不变量 `ALL == REGISTERED_FOCUS_ORDER ∪ {InjectionStrategy}`——该面板经 FC-05 下线但仍占枚举位;真值 `crates/chimera-tui/src/types.rs`,**勿引行号,会漂移**):插入新面板必须同步 `types.rs`(枚举 + `ALL` + `REGISTERED_FOCUS_ORDER`)+ `app/mod.rs` 工厂 + `app/tests.rs` + `tests/integration.rs` 四处断言
- `PanelId::next/prev` 循环测试断言需同步维护(2026-08-02 实证)
- i18n 硬编码 CJK 入 `KNOWN_VIOLATIONS` 基线,渐进 i18n 化(避免 i18n_completeness_test flaky)
- SlashCommandRegistry 现 **54 条**命令(2026-10-07 `/bench` 接线后;真值 `crates/chimera-tui/src/actions/slash_registry.rs`,旧值 53 已过期),注册项必须包含完整 `title_key` 字段(中英双语断言);W0~W11 逐项接线现状的唯一权威源是 `docs/tui/README.md`

**PowerShell 沙箱**:
- `Select-String` 默认大小写不敏感,"0 failed"会误匹配"FAILED";统计测试失败用 `-CaseSensitive`

**三重悖论免疫(第三阶段架构决策,见 nuxus规则.md §3.4.5)**:

- **记忆悖论**:OSA 静态稀疏掩码不能替代自适应记忆控制;固定 top-k 召回在任务阶段切换时会产生"幽灵记忆";记忆策略必须随任务阶段自适应(MinimalRecall→StandardTopK→QueryReformulation→AggressivePruning)
- **推理悖论**:10 层协调成本存在"推理悖论阈值"——当协调成本超过推理增益时,多 Agent 不如单 Agent;SkepticVeto 可能被策略性利用;需定期测量协调成本/推理增益比值
- **进化悖论**:GSOE/AutoDPO 验证器层级为 L3(执行反馈),存在"奖励黑客"风险;需向 L4(形式化验证)跃迁

**三环重组约束(前瞻,见 nuxus规则.md §3.4.5)**:

- 内环 9 crate 不依赖外环;外环 25 crate 仅通过 event-bus 依赖内环公开接口（**9/25 为 v2.20 设计时点快照,现值不追**;环分区现为 NOT-ENFORCED 设计意图,ADR-187 退役 Check A 后仅余 G-75/G-76 外环→内环只防增长棘轮门,见本文 §3 前瞻段）
- (v2.20 诊断时点)架构曾存在 4 类病理:星型耦合(L6)/跨层渗透(L7→L4)/隐式循环依赖(L5↔L9)/L1 上帝 crate——**v2.24.0 Phase 9(P9-T1~T12,ADR-054)已系统处置**,详见本文 §3.4.5 同条;crate 总数不登记,以 `Cargo.toml` `workspace.members` 现取

**9.工作时必须遵守**：组建一个由多名拥有10年以上行业经验的精英专家级子代理构成的专业化协作团队，该团队需以任务优先级为核心指导原则，对各项任务进度实施系统性的分布式深度分析与监控。团队应通过多轮结构化思考、充分探讨及严谨的验证流程，确保对任务需求、目标及约束条件的理解全面且准确。在执行阶段，必须严格按照既定的任务优先级顺序推进实施工作，同时始终秉持长期主义的工作理念，杜绝任何短期行为和资源过度消耗的情况。特别强调在代码修改过程中，必须先进行多方面、系统性的思考与规划后再实施代码修改，确保产出高质量并且高效的代码成果，具体质量标准包括：清晰且模块化的代码逻辑结构、高度的代码可读性、杜绝冗余代码和技术债务、完善的注释说明（包括功能描述、参数说明及复杂逻辑解释）以及符合行业最佳实践的编码规范。在整个任务执行周期内，授权团队调用所有符合任务要求且系统允许的工具资源，包括但不限于mcp、skills等相关工具，以保障任务的高效完成和卓越质量。团队还应建立定期进度汇报与质量审查机制，确保任务执行过程的透明度和成果的可追溯性。

claude.md文档和nuxus规则.md文档，进行融会贯通式相互参考。
