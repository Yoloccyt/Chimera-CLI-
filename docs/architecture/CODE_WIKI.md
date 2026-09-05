# Chimera CLI Code Wiki — NEXUS-OMEGA

> **版本**: v2.28.0-omega (Code Wiki · v2.28.0-omega 基线同步,2026-08-28)
> **基线代码版本**: v2.28.0-omega 在途(迭代链 v2.0.0-omega `chimera-mas` 多 Agent 协同子系统 → v2.4.0-omega P5 进化闭环 37 crate → v2.14.0~~v2.19.0 P2 Sprint 14 项任务全量交付 → v2.20.0-omega PROBE HCW-Sparse 深度优化完整闭环 P-1~~P3 38 crate(PROBE 阶段基线起点) → v2.21.0-omega CLI LLM 统一入口 → v2.22.0-omega MCA token 效率深度优化 38 个 crate 第 38 个 `mca-gateway` 落地 → v2.24.0-omega Phase 9 三环循环元架构重组收尾 P9-T12 + RUSTSEC-2026-0217/0222/0223 修复 → v2.25.0-omega Milestone B 全部交付 B-1~~B-6(Milestone B 终态) → v2.26.0-omega Concord TUI 重构 W0~~W11 全部收尾(SlashCommandRegistry 53 命令注册 + `/` 一级整合 + Chat/Quest 双轨会话模式 + ApprovalMode 动态 Shift+Tab + i18n 中英门户 + 10 份 ADR-074~~083 落档) →~~ **~~v2.27.0-omega Phase 10 §16 跨层协同闭环审计修复正式发布~~**~~(W1-W7 全波次闭环:经验卡片闭环组合根 + Quest 生命周期桥 + 卡片生成触发点 + 事件协议补齐 + mpsc 双清单对齐 + 合成闭环 + 奖励缺口) →~~ **~~v2.27.1-omega GPG 签名补发 + MCA E2E 超时加固~~**~~(无功能性变更) →~~ **~~v2.28.0-omega(在途,未打 tag)Phase 1-5 Ch12 W1-W26 收尾~~**~~(ComputeBridge/ShardedBus/CBMR/CausalGraph + 5 新 crate 至 43 + ADR-095~~160 + ADR-160 可达性棘轮/event\_types 镜像退役);types.rs 单表(metadata() 分类)**144 NexusEvent 变体**)
> **最后更新**: 2026-08-30(v2.28.0-omega 在途基线收编:43 crates · 28 生产可达/15 ADR-160 冻结孤岛 · 11564 tests · 144 NexusEvent · ADR-001\~160;工作区分支 `feat/phase1-w1-w8`,最新已发 tag 为 v2.27.0-omega(注:v2.27.1-omega 为 CHANGELOG-only 补丁,本地与 origin 均无 tag),v2.28 尚未打 tag)
> **权威源**: 本文件是架构决策、模块职责、核心类型的唯一权威参考
> **生成方式**: 8 位资深专家虚拟团队分布式源码深度分析 + 实证验证(Cargo.toml 比对 + `cargo check --workspace` 43/43 crate 全绿)
> **专家签名**: E01 首席架构师 · E02 安全架构师 · E03 记忆系统专家 · E04 路由算法专家 · E05 生产系统专家 · E06 认知科学专家 · E07 任务调度专家 · E08 前端交互专家
> **三方一致性** (2026-08-30 复核): `Cargo.toml` workspace.package.version = `2.28.0-omega`(代码实况,43 members) ⇔ `CHANGELOG.md` 最新条目 = `[2.28.1-omega] 2026-08-28 在途补丁登记(未升 version)` ⇔ 本文档 = **43 crates(28 生产可达 + 15 ADR-160 冻结孤岛)· 144 NexusEvent 变体(types.rs 单表,event\_types.rs 镜像已退役)· 测试规模 11564 passed / 0 failed**(2026-08-31 当前工作树全量重测,485 test target;静态 `#[test]` 计数 11433,差值为 doctest 43 + 宏展开;演进链 v2.20.0 8455 → v2.22.0 9255 → v2.24.0 9590 → v2.25.0 9669 → C 9699 → D 9744 → v2.26.0 9954 → v2.27.0 10836 → v2.28.0 11564)
> **基线变更触发**: 任何 workspace.member / NexusEvent 变体 / `#[test]` 函数增删必须同步更新本文档的"§1.1 身份标识"与"§3 Crate 索引",并触发 `scripts/check_doc_consistency.ps1` 巡检

***

## 目录

1. [项目概览](#1-项目概览)
2. [十层架构详解](#2-十层架构详解)
3. [43 Crate 完整索引](#3-43-crate-完整索引)
4. [核心领域类型](#4-核心领域类型)
5. [事件系统](#5-事件系统)
6. [依赖关系铁律](#6-依赖关系铁律)
7. [端到端数据流](#7-端到端数据流)
8. [关键设计模式](#8-关键设计模式)
9. [工程红线与实战教训](#9-工程红线与实战教训)
10. [构建、测试与运行](#10-构建测试与运行)
11. [架构决策记录 (ADR)](#11-架构决策记录-adr)
12. [目录结构索引](#12-目录结构索引)

***

## 1. 项目概览

### 1.1 身份标识

| 字段       | 值                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 项目名      | Chimera CLI                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| 代号       | NEXUS-OMEGA (Omni-Model Engineering Generative Architecture)                                                                                                                                                                                                                                                                                                                                                                                   |
| 根目录      | `D:\Chimera CLI`                                                                                                                                                                                                                                                                                                                                                                                                                               |
| 技术栈      | Rust 2021 edition · Tokio async · Workspace × **43 crates** (38 基线 + v2.28 新增 L10 `nexus-app-server` / L3 `session-store` / L9 `mas-sched`·`nexus-hook` / L7 `nexus-subagent`)                                                                                                                                                                                                                                                                 |
| 核心哲学     | OMEGA 十一定律: Ω₁-Sparse · Ω₂-Compress · Ω₃-Evolve · Ω₄-Event · Ω₅-Credit · Ω₆-Reuse · Ω₇-Locate · Ω₈-Assess · Ω₉-Preserve · Ω₁₀-Card · Ω₁₁-Synthesize                                                                                                                                                                                                                                                                                                                         |
| 设计来源     | Claude Code 尸检 + Hermes 基因 + Qoder 骨骼 + 五大模型灵魂                                                                                                                                                                                                                                                                                                                                                                                                 |
| **当前版本** | `v2.28.0-omega` (**在途开发,工作区** **`feat/phase1-w1-w8`** **分支,尚未打 tag**;最新已发 tag = v2.27.0-omega(注:v2.27.1-omega 为 CHANGELOG-only 补丁,本地与 origin 均无 tag);Phase 1-5 Ch12 波次 W1-W26 全部收尾 + 5 新 crate 落地 + ADR-095~160 治理 + 三轮冗余收敛;迭代链 v2.8.0 polish-v2.7 → v2.9.0~v2.13.0 L8/L10/MCA → v2.14.0\~v2.19.0 P3 Sprint → v2.20.0 PROBE → v2.21.0 CLI LLM → v2.22.0 MCA token → v2.24.0 Phase 9 → v2.25.0 Milestone B → v2.26.0 Concord TUI → v2.27.0 Phase 10 → v2.27.1 GPG 补发 → v2.28.0 Phase 1-5 治理 + 可达性棘轮) |
| 测试规模     | **11564 passed / 0 failed** (debug 模式全量回归,2026-08-31 当前工作树全量重测,485 test target;含 Concord TUI 27 面板(REGISTERED\_FOCUS\_ORDER 代码实测 = PanelId enum 27 变体) + 53 slash commands(slash\_registry 断言 reg.len()==53) + 双轨会话 + Phase 10 跨层闭环 + Phase 12 Ch12 波次 1-5 + 5 新 crate + doctest;演进 v2.26.0 9954 → v2.27.0 10836 → v2.28.0 11564)                                                                                                             |

### 1.2 OMEGA 十一定律(Ω₁~Ω₉ 基座 + Ω₁₀/Ω₁₁ 扩展;权威定义源:`Chimera CLI 十层架构深度打磨与优化方案 最新版.md` §3;Ω₁₀/Ω₁₁ 见 `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §3.1,收录于 ADR-170)

> ★ Insight: 四定律→九定律→十一定律演进,Ω₁~Ω₄ 架构基座 + Ω₅~Ω₉ v2.x 学习/进化体系补齐 + Ω₁₀/Ω₁₁ 经验卡片与按需记忆合成扩展。全部十一定律已有代码落地与 E2E 验证(2026-08-11 全库核验 Ω₁~Ω₉,2026-09-02 ADR-170 收录 Ω₁₀/Ω₁₁)。

| 定律             | 符号 | 工程实现                                         | 落地 crate                                         |
| -------------- | -- | -------------------------------------------- | ------------------------------------------------ |
| **Ω-Sparse**   | Ω₁ | 全维稀疏掩码 + 按需激活(工具/上下文/记忆/审计/预算)               | `osa-coordinator` + `sesa-router`                |
| **Ω-Compress** | Ω₂ | 四级窗口 + Mem-π 生成式记忆(4K/32K/128K/1M)           | `hcw-window` + `mlc-engine`                      |
| **Ω-Evolve**   | Ω₃ | AEGIS 四阶段引擎 + GRPO 风格进化 + 变体隔离               | `gsoe-evolution` + `chimera-mas`                 |
| **Ω-Event**    | Ω₄ | 事件驱动架构(broadcast + Critical mpsc 双通道,144 事件) | `event-bus`                                      |
| **Ω-Credit**   | Ω₅ | 信用分配:SHARP Shapley 值精确归因                     | `parliament/sharp.rs` + `mappo.rs`               |
| **Ω-Reuse**    | Ω₆ | 复用率优先:奖励函数优化技能复用率                            | `repo-wiki/skill_graph.rs` + `csn-substitutor`   |
| **Ω-Locate**   | Ω₇ | 行为定位:L1→L2→L3 自动导航代码修改点                      | `parliament/critical_path.rs`                    |
| **Ω-Assess**   | Ω₈ | 自我评估:Runtime Auditor 五维度证据纪律                 | `efficiency-monitor/auditor.rs`                  |
| **Ω-Preserve** | Ω₉ | 保留历史最佳:变体隔离 + 停止策略                           | `chimera-mas/variant_pool.rs` + `gsoe-evolution` |
| **Ω-Card**    | Ω₁₀ | 经验卡片数据结构:不可变 + 版本化 + append-only 事件流           | `event-bus/experience_card_bus.rs` + `nexus-contracts/experience_card.rs` + `mlc-engine/experience_card_system.rs` + `cmt-tiering/experience_card_storage.rs` |
| **Ω-Synthesize** | Ω₁₁ | 按需记忆合成算法:懒加载合成 + 非阻塞主流程 + Debug→同错误签名兄弟定向检索 | `mlc-engine/on_demand_synthesizer.rs` + `event-bus` |

### 1.3 核心术语速查

| 缩写       | 全称                                           | 对应 crate          |
| -------- | -------------------------------------------- | ----------------- |
| Ω-Sparse | 全维稀疏(工具/上下文/记忆/审计/预算)                        | `osa-coordinator` |
| Ω-Card  | 经验卡片数据结构(不可变 + 版本化 + append-only 事件流)         | `event-bus` + `mlc-engine` |
| Ω-Synthesize | 按需记忆合成算法(懒加载 + 非阻塞 + 错误签名定向检索)          | `mlc-engine` |
| CLV      | Context Latent Vector (512-dim 潜在语言)         | `nexus-core`      |
| MLC      | Multi-Level Context (四级神经形态记忆)               | `mlc-engine`      |
| HCW      | Hierarchical Context Window (4K/32K/128K/1M) | `hcw-window`      |
| CMT      | Capability Memory Tiering (热/温/冷/冰)          | `cmt-tiering`     |
| OSA      | Omni-Sparse Architecture (全维稀疏协调器)           | `osa-coordinator` |
| KVBSR    | KV-Block Semantic Router (两级块路由)             | `kvbsr-router`    |
| FaaE     | Function-as-Expert (工具即专家,语义路由)              | `faae-router`     |
| PVL      | Producer-Verifier Loop (并行流式生成验证)            | `pvl-layer`       |
| MTPE     | Multi-Token Prediction Execution (多步预测执行)    | `mtpe-executor`   |
| GQEP     | Gather-Query Execution Protocol (聚集执行)       | `gqep-executor`   |
| QEEP     | Quantum-Entangled Execution Protocol (量子纠缠)  | `qeep-protocol`   |
| TTG      | Thinking Toggle Governance (三级思考切换)          | `quest-engine`    |
| SSRA     | Slime-Style Rapid Adaptation (黏液式适配)         | `ssra-fusion`     |
| ISCM     | Inter-Shared Cross Module (跨层共享索引)           | `repo-wiki`       |
| SCC      | Speculative Context Cache (推测缓存)             | `scc-cache`       |
| LHQP     | Long-Horizon Quest Persistence (检查点持久化)      | `quest-engine`    |
| GSOE     | Guided Self-Organizing Evolution (在线进化)      | `gsoe-evolution`  |
| AHIRT    | Anti-Hack Intelligent Red Team (反黑客红队)       | `parliament`      |
| CHTC     | Cross-Harness Tool Compatibility (跨平台适配)     | `chtc-bridge`     |

***

## 2. 十层架构详解

### 2.1 分层映射 (L1→L10)

```
L0   Contracts ── nexus-contracts                            [ADR-033 纯类型零依赖契约层]
L10  Interface ── chimera-cli · chimera-tui · chtc-bridge · mcp-mesh · csn-substitutor · mca-gateway · nexus-app-server [ADR-065 MCA 网关 + WI-01 宿主协议门面]
L9   Quest ───── quest-engine · gea-activator · efficiency-monitor · chimera-mas · mas-sched · nexus-hook [ADR-145 调度控制面 + ADR-146 生命周期 Hook]
L8   Parliament ─ parliament · acb-governor · decb-governor
L7   Execution ── pvl-layer · gqep-executor · mtpe-executor · ssra-fusion · nexus-subagent [ADR-148 类型化子代理 + Task Auction]
L6   Router ───── osa-coordinator · kvbsr-router · faae-router · sesa-router · omega-learner [ADR-031 Bandit 学习]
L5   Knowledge ── repo-wiki · gsoe-evolution · auto-dpo
L4   Security ─── seccore · qeep-protocol · decay-engine
L3   Storage ──── scc-cache · lsct-tiering · cmt-tiering · session-store [ADR-141 append-only 会话事件流]
L2   Memory ───── nmc-encoder · hcw-window · mlc-engine
L1   Core ─────── nexus-core · event-bus · model-router
```

> **v2.4.0-omega 变更** (v5.0 P2 + P4):
>
> 1. **L0 Contracts** 新增: `nexus-contracts` 纯类型零依赖契约层(ADR-033),含 `OmniSparseMasks` / `HarnessSpec` / `TemporalMeta` / `NamespaceQuota` / `SelectorPolicy` 五类共享类型;依赖铁律扩展为 `L(N) → L(0)` 恒允许
> 2. **L6 Router** 新增: `omega-learner` LinUCB Bandit 学习层(ADR-031),异步下发 `SelectorPolicy::Learned` 给调用方,本地 fallback 保证可用性
> 3. 总数 35 → **37 crate**(+1 L0 +1 L6)
>
> **MCA M0 变更** (ADR-065,PANTHEON 计划):
> L10 新增 `mca-gateway` 多通道亲和网关(三协议 Codec + spec 驱动适配器,
> 仅依赖 L0/L1,与 chtc-bridge 同构),总数 37 → **38 crate**
>
> **v2.28.0-omega 变更** (Phase 1-5 Ch12 治理,ADR-141/145/146/148 + WI-01):
> 38 → **43 crate**,新增 5 个生产 crate——L10 `nexus-app-server`(第 39,WI-01 宿主层协议门面,核心-表面分离,JSON-RPC v1 + 每 Thread 一 actor)、L3 `session-store`(第 40,ADR-141,append-only 会话事件流 + CBMR 微批写)、L9 `mas-sched`(第 41,ADR-145,从 chimera-mas strangler 拆出的多代理调度控制面)、L9 `nexus-hook`(第 42,ADR-146,13+ 生命周期 Hook + seccore 沙箱)、L7 `nexus-subagent`(第 43,ADR-148,类型化子代理运行时 + Task Auction,Swarm 上限 8)。**可达性**:ADR-160 棘轮裁定 28 个生产可达 + 15 个冻结孤岛(见 `scripts/crate_reachability_freeze.txt`),dev-dep 不计入装配面。

### 2.2 各层职责概述

| 层级                | 名称  | 核心职责                           |
| ----------------- | --- | ------------------------------ |
| **L1 Core**       | 核心层 | 定义全局共享领域类型、事件总线契约、模型路由策略       |
| **L2 Memory**     | 记忆层 | 多模态编码、分层上下文窗口、四级神经形态记忆         |
| **L3 Storage**    | 存储层 | 推测缓存、存储层级协调、能力内存分层             |
| **L4 Security**   | 安全层 | 零信任沙箱、量子纠缠协议(零孤儿调用)、能力衰减引擎     |
| **L5 Knowledge**  | 知识层 | 代码知识库、自进化引擎、偏好优化               |
| **L6 Router**     | 路由层 | 全维稀疏协调、KV块语义路由、工具即专家路由、子专家稀疏激活 |
| **L7 Execution**  | 执行层 | 生产验证循环、聚集执行、多步预测、黏液式融合         |
| **L8 Parliament** | 议会层 | 多角色辩论表决、自适应预算治理、动态紧急预算治理       |
| **L9 Quest**      | 任务层 | 长期任务引擎、门控专家激活、效率监控仪表盘          |
| **L10 Interface** | 接口层 | CLI入口、TUI仪表盘、跨IDE适配、MCP网格、降级链  |

***

## 3. 43 Crate 完整索引

> **三方一致性原则**: 本节每个 crate 的"层归属"必须与 §2.1 分层映射图严格一致;
> "主要依赖"必须与各 crate 的 `Cargo.toml` 实证一致;任何增删触发 `scripts/check_doc_consistency.ps1` 巡检。
> **v2.4.0-omega 变更**: 35 → 37 crate,新增 L0 `nexus-contracts` (ADR-033) + L6 `omega-learner` (ADR-031)。
> **MCA M0 变更**: 37 → 38 crate,新增 L10 `mca-gateway` (ADR-065)。
> **v2.28.0-omega 变更**: 38 → 43 crate,新增 L10 `nexus-app-server`(WI-01)、L3 `session-store`(ADR-141)、L9 `mas-sched`(ADR-145)、L9 `nexus-hook`(ADR-146)、L7 `nexus-subagent`(ADR-148)。
> **可达性标注(ADR-160)**: 43 crate = 28 生产可达 + 15 冻结孤岛;孤岛完整清单(含阻塞依赖/解除条件)集中见本节末尾 **§3.11 冻结孤岛清单**,权威源为 `scripts/crate_reachability_freeze.txt`(由 `scripts/check_crate_reachability.sh` 生成,dev-dep 不计入装配面)。

### 3.0 L0 Contracts (1 crate,ADR-033)

#### [nexus-contracts](file:///d:/Chimera%20CLI/crates/nexus-contracts)

| 项          | 说明                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**    | L0 Contracts                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| **核心职责**   | 纯类型 + 零逻辑 + 零依赖契约层;承载跨层共享的语义不变量类型(`OmniSparseMasks` / `HarnessSpec` / `TemporalMeta` / `NamespaceQuota` / `SelectorPolicy`,MCA M0 新增 `affinity` 模块:`ProviderId` / `CapabilitySet` / `ModelAffinitySpec`,ADR-065),P9 新增 `budget_tier` / `command_validation` / `domain` / `event_payload` 模块(`BudgetTier` / `Command` / `CommandPolicy` / `AttackType` / `ThinkingMode` / `MultimodalInput` / `UserIntent` / `Quest` / `Task` / `EventSeverity` / `TaskPriority` / `AgentStatus`,ADR-054 决策 3/6,P9-T3/T4/T7 上提/下沉),依赖铁律扩展为 `L(N) → L(0)` 恒允许 |
| **关键文件**   | [lib.rs](file:///d:/Chimera%20CLI/crates/nexus-contracts/src/lib.rs)                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| **关键类型**   | `OmniSparseMasks` · `HarnessSpec` · `TemporalMeta` · `NamespaceQuota` · `SelectorPolicy` · `BudgetTier` · `Command` · `CommandPolicy` · `AttackType` · `ThinkingMode` · `MultimodalInput` · `UserIntent` · `Quest` · `Task` · `EventSeverity` · `TaskPriority` · `AgentStatus`                                                                                                                                                                                                                                                              |
| **关键函数**   | 类型派生(`Serialize`/`Deserialize`/`Clone`/`Debug`/`PartialEq`);`util` 模块三个**无副作用共享纯函数**:`xts_top_k_by`(Top-K,O(n) `select_nth_unstable_by`)、`sigmoid`、`percentile_sorted<T: Copy>`(已排序切片分位,O(1))。三者为 ADR-033"纯类型+零逻辑"约束的**受控例外**(与 `test_scale::scaled_timeout!` 同源),须为零分配/零 I/O/零全局状态                                                                                                                                                                                                                                                         |
| **性能证据**   | `benches/util_micro.rs`(criterion,含 `CountingAlloc` 零堆分配硬断言):`sigmoid scalar` 33.2 ns / `sigmoid map 1024` 33.73 µs / `percentile_sorted p95` 6.5 ns(n=100 与 n=10000 等价 → 实证 O(1));CI 门槛见 `bench_check.yml`                                                                                                                                                                                                                                                                                                                                 |
| **主要依赖**   | **仅** **`serde`** **workspace**,**无其他 workspace crate 依赖**(零依赖契约层)                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| **设计模式**   | Pure Data · 零运行时 · 编译期契约                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| **ADR 来源** | ADR-033 (L0 nexus-contracts)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |

### 3.1 L1 Core (3 crates)

#### [nexus-core](file:///d:/Chimera%20CLI/crates/nexus-core)

| 项        | 说明                                                                                                                                                                                                                                                                            |
| -------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L1 Core                                                                                                                                                                                                                                                                       |
| **核心职责** | 定义所有上层共享的领域类型、CLV向量、错误类型、存储trait                                                                                                                                                                                                                                              |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/nexus-core/src/lib.rs) · [types.rs](file:///d:/Chimera%20CLI/crates/nexus-core/src/types.rs) · [clv.rs](file:///d:/Chimera%20CLI/crates/nexus-core/src/clv.rs) · [state.rs](file:///d:/Chimera%20CLI/crates/nexus-core/src/state.rs) |
| **关键类型** | `UserIntent` · `Quest` · `Task` · `TaskStatus` · `Checkpoint` · `ThinkingMode` · `CLV` · `NexusState` · `MultimodalInput`                                                                                                                                                     |
| **关键函数** | `CLV::basis()` · `CLV::cosine_similarity()` · `cosine_similarity_slices()`(定义已下沉 L0 `nexus-contracts::util`,此处 `pub use` 重导出,调用路径不变) |
| **主要依赖** | ndarray · serde · chrono · uuid · thiserror                                                                                                                                                                                                                                   |

#### [event-bus](file:///d:/Chimera%20CLI/crates/event-bus)

| 项        | 说明                                                                                                                                                                                                                                                                                      |
| -------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L1 Core                                                                                                                                                                                                                                                                                 |
| **核心职责** | 跨层通信唯一通道，基于Tokio broadcast + mpsc双通道，背压控制                                                                                                                                                                                                                                               |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/event-bus/src/lib.rs) · [bus.rs](file:///d:/Chimera%20CLI/crates/event-bus/src/bus.rs) · [types.rs](file:///d:/Chimera%20CLI/crates/event-bus/src/types.rs) · [backpressure.rs](file:///d:/Chimera%20CLI/crates/event-bus/src/backpressure.rs) |
| **关键类型** | `EventBus` · `NexusEvent` · `EventMetadata` · `EventSeverity` · `EventSubscription`                                                                                                                                                                                                     |
| **关键方法** | `EventBus::publish()` · `EventBus::subscribe()` · `EventBus::publish_critical()`                                                                                                                                                                                                        |
| **主要依赖** | tokio · serde · chrono · uuid · tracing · nexus-core                                                                                                                                                                                                                                    |
| **设计模式** | 发布-订阅 · 双通道保障(Normal→broadcast, Critical→mpsc)                                                                                                                                                                                                                                          |

#### [model-router](file:///d:/Chimera%20CLI/crates/model-router)

| 项        | 说明                                                                                                                                                                                                                                                                                                                                                                  |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L1 Core                                                                                                                                                                                                                                                                                                                                                             |
| **核心职责** | 模型注册与策略化路由(MoE混合专家、CACR上下文感知)，调用历史SQLite持久化                                                                                                                                                                                                                                                                                                                         |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/model-router/src/lib.rs) · [router.rs](file:///d:/Chimera%20CLI/crates/model-router/src/router.rs) · [moe.rs](file:///d:/Chimera%20CLI/crates/model-router/src/moe.rs) · [cacr.rs](file:///d:/Chimera%20CLI/crates/model-router/src/cacr.rs) · [registry.rs](file:///d:/Chimera%20CLI/crates/model-router/src/registry.rs) |
| **关键类型** | `ModelRouter` · `ModelRegistry` · `MoERouter` · `CACRController` · `RoutingStrategy`                                                                                                                                                                                                                                                                                |
| **关键方法** | `ModelRouter::route()` · `ModelRegistry::register()`                                                                                                                                                                                                                                                                                                                |
| **主要依赖** | tokio · rusqlite · serde · ndarray · dashmap · rand · nexus-core                                                                                                                                                                                                                                                                                                    |

***

### 3.2 L2 Memory (3 crates)

#### [nmc-encoder](file:///d:/Chimera%20CLI/crates/nmc-encoder)

| 项           | 说明                                                                                                                                                                                                                                                                                                                                                                                                         |
| ----------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**     | L2 Memory                                                                                                                                                                                                                                                                                                                                                                                                  |
| **核心职责**    | 神经多模态编码(NMC)，Text/Image/Video/Audio/Desktop感知器 → 512维CLV融合；Image/Video/Audio 通过 tract-onnx 加载预训练模型(CLIP/VideoMAE/Whisper)实现语义级嵌入推理                                                                                                                                                                                                                                                                         |
| **关键文件**    | [lib.rs](file:///d:/Chimera%20CLI/crates/nmc-encoder/src/lib.rs) · [fusion.rs](file:///d:/Chimera%20CLI/crates/nmc-encoder/src/fusion.rs) · [perceptors/](file:///d:/Chimera%20CLI/crates/nmc-encoder/src/perceptors) · [perceptors/onnx\_backend.rs](file:///d:/Chimera%20CLI/crates/nmc-encoder/src/perceptors/onnx_backend.rs) · [config.rs](file:///d:/Chimera%20CLI/crates/nmc-encoder/src/config.rs) |
| **关键类型**    | `NmcEncoder` · `TextPerceptor` · `ImagePerceptor` · `AudioPerceptor` · `VideoPerceptor` · `DesktopPerceptor` · `OnnxBackend` · `ModelType`                                                                                                                                                                                                                                                                 |
| **关键方法**    | `NmcEncoder::encode()` → `CLV` · `OnnxBackend::load()` / `OnnxBackend::run()`                                                                                                                                                                                                                                                                                                                              |
| **主要依赖**    | tokio · ndarray · serde · tract-onnx · image · sha2 · nexus-core · event-bus                                                                                                                                                                                                                                                                                                                               |
| **ONNX 说明** | P1-1 (2026-07-28): ImagePerceptor/VideoPerceptor/AudioPerceptor 从占位升级为 tract-onnx 推理，模型加载失败时 fallback 返回 `EncodingFailed`（向后兼容）；模型文件通过 `NmcConfig::model_dir` 配置，详见 `docs/onnx-models.md`                                                                                                                                                                                                                  |

#### [hcw-window](file:///d:/Chimera%20CLI/crates/hcw-window)

| 项        | 说明                                                                                                                                                                                                                                                                                                  |
| -------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L2 Memory                                                                                                                                                                                                                                                                                           |
| **核心职责** | 分层上下文窗口(4K/32K/128K/1M)，配合OSA稀疏掩码实现1M等效上下文(实际仅加载128K)                                                                                                                                                                                                                                               |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/hcw-window/src/lib.rs) · [window.rs](file:///d:/Chimera%20CLI/crates/hcw-window/src/window.rs) · [selector.rs](file:///d:/Chimera%20CLI/crates/hcw-window/src/selector.rs) · [compressor.rs](file:///d:/Chimera%20CLI/crates/hcw-window/src/compressor.rs) |
| **关键类型** | `HierarchicalWindow` · `WindowSelector` · `WindowCompressor` · `WindowTier`                                                                                                                                                                                                                         |
| **关键方法** | `HierarchicalWindow::load()` · `HierarchicalWindow::select()`                                                                                                                                                                                                                                       |
| **主要依赖** | tokio · serde · ndarray · dashmap · nexus-core                                                                                                                                                                                                                                                      |

#### [mlc-engine](file:///d:/Chimera%20CLI/crates/mlc-engine)

| 项        | 说明                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L2 Memory                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| **核心职责** | 四级神经形态记忆(L0工作/L1情景/L2语义/L3程序)，自动冷热迁移                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/mlc-engine/src/lib.rs) · [engine.rs](file:///d:/Chimera%20CLI/crates/mlc-engine/src/engine.rs) · [l0\_working.rs](file:///d:/Chimera%20CLI/crates/mlc-engine/src/l0_working.rs) · [l1\_episodic.rs](file:///d:/Chimera%20CLI/crates/mlc-engine/src/l1_episodic.rs) · [l2\_semantic.rs](file:///d:/Chimera%20CLI/crates/mlc-engine/src/l2_semantic.rs) · [l3\_procedural.rs](file:///d:/Chimera%20CLI/crates/mlc-engine/src/l3_procedural.rs) |
| **关键类型** | `MlcEngine` · `WorkingMemory` · `EpisodicMemory` · `SemanticMemory` · `ProceduralMemory` · `MemoryTier`                                                                                                                                                                                                                                                                                                                                                                               |
| **关键方法** | `MlcEngine::store()` · `MlcEngine::recall()` · `MlcEngine::promote()` · `MlcEngine::demote()`                                                                                                                                                                                                                                                                                                                                                                                         |
| **主要依赖** | tokio · serde · ndarray · rusqlite · dashmap · chrono · uuid · nexus-core                                                                                                                                                                                                                                                                                                                                                                                                             |

***

### 3.3 L3 Storage (4 crates,v2.28 新增 session-store)

#### [scc-cache](file:///d:/Chimera%20CLI/crates/scc-cache)

| 项        | 说明                                                                                                                                                                                                                                                                                                                                               |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **架构层**  | L3 Storage                                                                                                                                                                                                                                                                                                                                       |
| **核心职责** | 推测上下文缓存(SCC)，一阶马尔可夫链推测性预取 + Arc引用保护LRU + WAL持久化                                                                                                                                                                                                                                                                                                  |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/scc-cache/src/lib.rs) · [cache.rs](file:///d:/Chimera%20CLI/crates/scc-cache/src/cache.rs) · [lru.rs](file:///d:/Chimera%20CLI/crates/scc-cache/src/lru.rs) · [prefetch.rs](file:///d:/Chimera%20CLI/crates/scc-cache/src/prefetch.rs) · [wal.rs](file:///d:/Chimera%20CLI/crates/scc-cache/src/wal.rs) |
| **关键类型** | `SccCache` · `LruCache` · `Prefetcher` · `Wal` · `CacheEntry`                                                                                                                                                                                                                                                                                    |
| **关键方法** | `SccCache::get()` · `SccCache::put()` · `SccCache::prefetch()`                                                                                                                                                                                                                                                                                   |
| **主要依赖** | tokio · serde · rmp-serde · dashmap · rand · chrono · nexus-core                                                                                                                                                                                                                                                                                 |

#### [lsct-tiering](file:///d:/Chimera%20CLI/crates/lsct-tiering)

| 项        | 说明                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| -------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L3 Storage                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| **核心职责** | 延迟敏感存储分层(LSCT)，根据任务负载画像计算层级切换策略                                                                                                                                                                                                                                                                                                                                                                                                                             |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/lsct-tiering/src/lib.rs) · [tiering/coordinator.rs](file:///d:/Chimera%20CLI/crates/lsct-tiering/src/tiering/coordinator.rs) · [tiering/promoter.rs](file:///d:/Chimera%20CLI/crates/lsct-tiering/src/tiering/promoter.rs) · [tiering/demoter.rs](file:///d:/Chimera%20CLI/crates/lsct-tiering/src/tiering/demoter.rs) · [tiering/profile.rs](file:///d:/Chimera%20CLI/crates/lsct-tiering/src/tiering/profile.rs) |
| **关键类型** | `LsctCoordinator` · `TierPromoter` · `TierDemoter` · `WorkloadProfile` · `StorageTier`                                                                                                                                                                                                                                                                                                                                                                      |
| **关键方法** | `LsctCoordinator::profile()` · `LsctCoordinator::promote()` · `LsctCoordinator::demote()`                                                                                                                                                                                                                                                                                                                                                                   |
| **主要依赖** | tokio · serde · dashmap · chrono · tracing · nexus-core                                                                                                                                                                                                                                                                                                                                                                                                     |

#### [cmt-tiering](file:///d:/Chimera%20CLI/crates/cmt-tiering)

| 项        | 说明                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| -------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L3 Storage                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| **核心职责** | 能力内存分层(CMT)，热/温/冷/冰四级存储 + 指数衰减自动迁移                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/cmt-tiering/src/lib.rs) · [coordinator.rs](file:///d:/Chimera%20CLI/crates/cmt-tiering/src/coordinator.rs) · [hot.rs](file:///d:/Chimera%20CLI/crates/cmt-tiering/src/hot.rs) · [warm.rs](file:///d:/Chimera%20CLI/crates/cmt-tiering/src/warm.rs) · [cold.rs](file:///d:/Chimera%20CLI/crates/cmt-tiering/src/cold.rs) · [ice.rs](file:///d:/Chimera%20CLI/crates/cmt-tiering/src/ice.rs) · [decay.rs](file:///d:/Chimera%20CLI/crates/cmt-tiering/src/decay.rs) · [migrator.rs](file:///d:/Chimera%20CLI/crates/cmt-tiering/src/migrator.rs) |
| **关键类型** | `CmtCoordinator` · `HotTier` · `WarmTier` · `ColdTier` · `IceTier` · `DecayScheduler` · `CapabilityMigrator`                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| **关键方法** | `CmtCoordinator::access()` · `CmtCoordinator::store()` · `CmtCoordinator::decay_tick()`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| **主要依赖** | tokio · serde · rmp-serde · rusqlite · dashmap · rand · chrono · sha2 · hex · nexus-core                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |

#### [session-store](file:///d:/Chimera%20CLI/crates/session-store)

| 项          | 说明                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| ---------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**    | L3 Storage(v2.28 新增,workspace 第 40 个 crate)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| **核心职责**   | 会话事件流存储:把会话持久化从「Checkpoint 线性全量快照」升级为「append-only 事件流 + CBMR 微批写」;JSONL 段文件(每 Thread 一段,长度前缀 WAL 意向,`append` 返回 Ok 前 fsync)+ SQLite 树索引(segments/events 单事务批量);Offset 双键 `{seq 全局单调, row 段内行号}` 供 k-way 归并回放;`fork(session, offset)` 前缀段元数据零拷贝复制                                                                                                                                                                                                                                                                                          |
| **关键文件**   | [lib.rs](file:///d:/Chimera%20CLI/crates/session-store/src/lib.rs) · [segment.rs](file:///d:/Chimera%20CLI/crates/session-store/src/segment.rs) · [writer.rs](file:///d:/Chimera%20CLI/crates/session-store/src/writer.rs) · [tree.rs](file:///d:/Chimera%20CLI/crates/session-store/src/tree.rs) · [replay.rs](file:///d:/Chimera%20CLI/crates/session-store/src/replay.rs) · [model\_view.rs](file:///d:/Chimera%20CLI/crates/session-store/src/model_view.rs) · [error.rs](file:///d:/Chimera%20CLI/crates/session-store/src/error.rs) |
| **关键类型**   | `SessionSegment` · `SegmentOffset{seq,row}` · `MicroBatchWriter`(CBMR,≤64/2ms 自适应窗口) · `EventTreeIndex` · `SessionReplayer` · `ModelView`                                                                                                                                                                                                                                                                                                                                                                                                 |
| **关键方法**   | `append(ev)`(攒批→spawn\_blocking flush) · `read_events()`(SQLite 树索引,与写并发 WAL) · `fork(session, offset)`                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| **主要依赖**   | tokio · serde · serde\_json · rusqlite(spawn\_blocking 包装) · thiserror · nexus-core · nexus-contracts · event-bus                                                                                                                                                                                                                                                                                                                                                                                                                         |
| **设计模式**   | append-only 段 · CBMR 微批写(N 次直写降为 ceil(N/64) 次) · Write-Ahead 长度前缀 · 崩溃尾部截断                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| **ADR 来源** | ADR-141(CSC 会话存储契约)/ ADR-108(CBMR 微批写)/ ADR-109(Offset 双键归并);对应 v4.0 WI-18、九源手册 W9 T-07                                                                                                                                                                                                                                                                                                                                                                                                                                                   |

***

### 3.4 L4 Security (3 crates)

#### [seccore](file:///d:/Chimera%20CLI/crates/seccore)

| 项                     | 说明                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| --------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**               | L4 Security                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| **核心职责**              | 安全核心，零信任沙箱(gVisor内核级隔离/进程隔离)、Merkle审计链、ASA自适应安全审计                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| **关键文件**              | [lib.rs](file:///d:/Chimera%20CLI/crates/seccore/src/lib.rs) · [sandbox.rs](file:///d:/Chimera%20CLI/crates/seccore/src/sandbox.rs) · [audit.rs](file:///d:/Chimera%20CLI/crates/seccore/src/audit.rs) · [asa.rs](file:///d:/Chimera%20CLI/crates/seccore/src/asa.rs) · [policy.rs](file:///d:/Chimera%20CLI/crates/seccore/src/policy.rs) · [gvisor.rs](file:///d:/Chimera%20CLI/crates/seccore/src/gvisor.rs) · [types.rs](file:///d:/Chimera%20CLI/crates/seccore/src/types.rs) |
| **关键类型**              | `SecurityCore` · `Sandbox` · `MerkleAuditChain` · `AsaAuditor` · `SecurityPolicy` · `RiskLevel` · `GvisorRuntime` · `GvisorConfig`                                                                                                                                                                                                                                                                                                                                                 |
| **关键方法**              | `Sandbox::execute()` · `MerkleAuditChain::record()` · `AsaAuditor::audit()` · `GvisorRuntime::detect()` / `GvisorRuntime::spawn()`                                                                                                                                                                                                                                                                                                                                                 |
| **主要依赖**              | tokio · serde · sha2 · hex · chrono · uuid · tracing · thiserror · event-bus                                                                                                                                                                                                                                                                                                                                                                                                       |
| **gVisor 说明**         | P2-9 (2026-07-28): `Sandbox` 新增 `use_gvisor` + `gvisor_runtime` 字段，`execute_in_sandbox()` 在 Linux + runsc 可用时通过 gVisor 内核级隔离执行命令，否则降级为 `tokio::process::Command`；`GvisorRuntime` 封装 runsc 检测与子进程启动；详见 `docs/gvisor-deployment.md`                                                                                                                                                                                                                                                  |
| **FormalVerifier L4** | ADR-047(Proposed, 2026-07-27):L4 形式化验证器层级跃迁路线图,嵌入 seccore 而非新建独立 crate(决策 1);三阶段渐进式实施 M0 骨架(2026-08-15)/ M1 集成(2026-09-15)/ M2 完整(2026-10-15);属性语言采用 Rust 类型系统 + proptest + clippy 三层组合(决策 3,非 SMT-LIB/Lean);复用 `sandbox.rs` 资源限制 + `chimera-mas/invariants.rs` InvariantChecker 模式(决策 4);对齐 ADR-042 R2 解冻前置条件判定标准 2(ADR 落档);落地路径 `crates/seccore/src/formal_verifier.rs` + 4 子模块(append-only)                                                                                     |

#### [decay-engine](file:///d:/Chimera%20CLI/crates/decay-engine)

| 项        | 说明                                                                                                                                          |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L4 Security                                                                                                                                 |
| **核心职责** | 能力衰减引擎，基于连续权限流体模型，能力随时间/风险动态衰减                                                                                                              |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/decay-engine/src/lib.rs) · [engine.rs](file:///d:/Chimera%20CLI/crates/decay-engine/src/engine.rs) |
| **关键类型** | `DecayEngine` · `CapabilityToken` · `DecayProfile`                                                                                          |
| **关键方法** | `DecayEngine::grant()` · `DecayEngine::check()` · `DecayEngine::decay_tick()`                                                               |
| **主要依赖** | tokio · serde · dashmap · chrono · tracing · nexus-core · event-bus                                                                         |

#### [qeep-protocol](file:///d:/Chimera%20CLI/crates/qeep-protocol)

| 项        | 说明                                                                                                                                                                                                                               |
| -------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L4 Security                                                                                                                                                                                                                      |
| **核心职责** | 量子纠缠执行协议(QEEP)，保证所有异步操作都有聚集/超时处理(零孤儿调用)                                                                                                                                                                                          |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/qeep-protocol/src/lib.rs) · [protocol.rs](file:///d:/Chimera%20CLI/crates/qeep-protocol/src/protocol.rs) · [detector.rs](file:///d:/Chimera%20CLI/crates/qeep-protocol/src/detector.rs) |
| **关键类型** | `QeepProtocol` · `OrphanDetector` · `EntangledOperation` · `EntanglementState`                                                                                                                                                   |
| **关键方法** | `QeepProtocol::entangle()` · `QeepProtocol::complete()` · `OrphanDetector::detect()`                                                                                                                                             |
| **主要依赖** | tokio · serde · dashmap · uuid · chrono · tracing · thiserror · nexus-core · event-bus                                                                                                                                           |

***

### 3.5 L5 Knowledge (3 crates)

#### [repo-wiki](file:///d:/Chimera%20CLI/crates/repo-wiki)

| 项        | 说明                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L5 Knowledge                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| **核心职责** | 代码知识库，ISCM跨层共享索引、FTS5全文检索、内存KNN向量检索、知识沉淀与指标                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/repo-wiki/src/lib.rs) · [store.rs](file:///d:/Chimera%20CLI/crates/repo-wiki/src/store.rs) · [iscm.rs](file:///d:/Chimera%20CLI/crates/repo-wiki/src/iscm.rs) · [fts.rs](file:///d:/Chimera%20CLI/crates/repo-wiki/src/fts.rs) · [vector.rs](file:///d:/Chimera%20CLI/crates/repo-wiki/src/vector.rs) · [generator.rs](file:///d:/Chimera%20CLI/crates/repo-wiki/src/generator.rs) · [metrics.rs](file:///d:/Chimera%20CLI/crates/repo-wiki/src/metrics.rs) |
| **关键类型** | `RepoWiki` · `IscmIndex` · `FtsSearcher` · `VectorSearcher` · `WikiEntry` · `WikiMetrics`                                                                                                                                                                                                                                                                                                                                                                                                            |
| **关键方法** | `RepoWiki::search()` · `RepoWiki::store()` · `RepoWiki::generate_index()`                                                                                                                                                                                                                                                                                                                                                                                                                            |
| **主要依赖** | tokio · serde · serde\_json · rmp-serde · rusqlite · ndarray · dashmap · sha2 · hex · chrono · uuid · nexus-core · event-bus                                                                                                                                                                                                                                                                                                                                                                         |

#### [gsoe-evolution](file:///d:/Chimera%20CLI/crates/gsoe-evolution)

| 项             | 说明                                                                                                                                                                                                                                                                                                                                                                                                                              |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**       | L5 Knowledge                                                                                                                                                                                                                                                                                                                                                                                                                    |
| **核心职责**      | 引导式自组织进化(GSOE)，GRPO风格策略进化 + 适应度评估 + 变异                                                                                                                                                                                                                                                                                                                                                                                          |
| **关键文件**      | [lib.rs](file:///d:/Chimera%20CLI/crates/gsoe-evolution/src/lib.rs) · [engine.rs](file:///d:/Chimera%20CLI/crates/gsoe-evolution/src/engine.rs) · [policy/grpo.rs](file:///d:/Chimera%20CLI/crates/gsoe-evolution/src/policy/grpo.rs) · [policy/fitness.rs](file:///d:/Chimera%20CLI/crates/gsoe-evolution/src/policy/fitness.rs) · [policy/mutation.rs](file:///d:/Chimera%20CLI/crates/gsoe-evolution/src/policy/mutation.rs) |
| **关键类型**      | `GsoeEngine` · `GrpoPolicy` · `FitnessEvaluator` · `MutationOperator` · `EvolutionRecord`                                                                                                                                                                                                                                                                                                                                       |
| **关键方法**      | `GsoeEngine::evolve()` · `GsoeEngine::record_feedback()`                                                                                                                                                                                                                                                                                                                                                                        |
| **主要依赖**      | tokio · serde · ndarray · rand · dashmap · chrono · uuid · tracing · nexus-core · event-bus                                                                                                                                                                                                                                                                                                                                     |
| **L4 形式化验证门** | ADR-047(Proposed, 2026-07-27):GSOE 进化主路径将新增 `evolve_with_formal_verification()` 方法(append-only,决策 5),在 L3 执行反馈通过后追加 L4 形式化验证门作为第二道闸;L4 门失败的候选发布 `NexusEvent::FormalVerificationFailed`(Critical 级,走 mpsc 旁路通道)并否决,不进入 AutoDPO 偏好对生成;L4 门通过发布 `NexusEvent::FormalVerificationPassed`(Normal 级);对齐 ADR-042 R2 冻结解冻前置条件 + 三重悖论进化悖论红线(L3→L4 跃迁);落地时间表 M1 集成 2026-09-15                                                              |

#### [auto-dpo](file:///d:/Chimera%20CLI/crates/auto-dpo)

| 项        | 说明                                                                                                                                                                                                                                                             |
| -------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L5 Knowledge                                                                                                                                                                                                                                                   |
| **核心职责** | 自动DPO(直接偏好优化)，从执行轨迹自动生成偏好对；**P1-3: FormalVerifier M1 偏好对一致性验证**(反自偏好 + margin 有界性)                                                                                                                                                                             |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/auto-dpo/src/lib.rs) · [generator.rs](file:///d:/Chimera%20CLI/crates/auto-dpo/src/generator.rs) · [formal/preference\_consistency.rs](file:///d:/Chimera%20CLI/crates/auto-dpo/src/formal/preference_consistency.rs) |
| **关键类型** | `AutoDpoGenerator` · `PreferencePair` · `TrajectorySample` · `PreferenceConsistencyChecker`(M1)                                                                                                                                                                |
| **关键方法** | `AutoDpoGenerator::generate_pair()` · `AutoDpoGenerator::record_trajectory()` · `PreferenceConsistencyChecker::verify_no_self_preference()` · `PreferenceConsistencyChecker::verify_margin_bounded()`                                                          |
| **主要依赖** | event-bus · tokio · serde · serde\_json · thiserror · tracing · nexus-contracts(VerificationResult)                                                                                                                                                            |

***

### 3.6 L6 Router (5 crates,ADR-031 新增 omega-learner)

> **L6 星型耦合已解耦**（ADR-033）：kvbsr-router、faae-router、sesa-router 通过 L0 \[`nexus-contracts`] 共享 `ToolId` 等类型，不再依赖 `osa-coordinator`。`osa-coordinator` 保持为 L6 Router 的协调器，但并非其他路由器的依赖。

#### [osa-coordinator](file:///d:/Chimera%20CLI/crates/osa-coordinator)

| 项        | 说明                                                                                                                                                                                                                                     |
| -------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L6 Router                                                                                                                                                                                                                              |
| **核心职责** | 全维稀疏架构协调器(OSA)，五维度稀疏掩码(路由/上下文/记忆/审计/预算)计算                                                                                                                                                                                              |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/osa-coordinator/src/lib.rs) · [coordinator.rs](file:///d:/Chimera%20CLI/crates/osa-coordinator/src/coordinator.rs) · [masks.rs](file:///d:/Chimera%20CLI/crates/osa-coordinator/src/masks.rs) |
| **关键类型** | `OmniSparseCoordinator` · `OmniSparseMasks` · `RoutingMask` · `ContextMask` · `MemoryMask` · `AuditMask` · `BudgetMask`                                                                                                                |
| **关键方法** | `OmniSparseCoordinator::compute_masks()` → `OmniSparseMasks`                                                                                                                                                                           |
| **主要依赖** | nexus-contracts · nexus-core · event-bus · serde · serde\_json · thiserror · sha2 · hex · tracing                                                                                                                                      |

#### [kvbsr-router](file:///d:/Chimera%20CLI/crates/kvbsr-router)

| 项        | 说明                                                                                                                                                                                                                                                                                                      |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L6 Router                                                                                                                                                                                                                                                                                               |
| **核心职责** | KV块语义路由器(KVBSR)，两级块路由 + 语义块管理 + 自动再平衡                                                                                                                                                                                                                                                                   |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/kvbsr-router/src/lib.rs) · [router.rs](file:///d:/Chimera%20CLI/crates/kvbsr-router/src/router.rs) · [blocks.rs](file:///d:/Chimera%20CLI/crates/kvbsr-router/src/blocks.rs) · [rebalancer.rs](file:///d:/Chimera%20CLI/crates/kvbsr-router/src/rebalancer.rs) |
| **关键类型** | `KvbsrRouter` · `SemanticBlock` · `BlockRebalancer` · `RoutingResult`                                                                                                                                                                                                                                   |
| **关键方法** | `KvbsrRouter::route()` · `KvbsrRouter::register_block()` · `BlockRebalancer::rebalance()`                                                                                                                                                                                                               |
| **主要依赖** | tokio · serde · ndarray · dashmap · uuid · tracing · nexus-core · event-bus · nexus-contracts · thiserror                                                                                                                                                                                               |

#### [faae-router](file:///d:/Chimera%20CLI/crates/faae-router)

| 项        | 说明                                                                                                                                                                                                                                                                                      |
| -------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L6 Router                                                                                                                                                                                                                                                                               |
| **核心职责** | 工具即专家路由器(FaaE)，专家注册/匹配/路由、EDSB概率均衡                                                                                                                                                                                                                                                      |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/faae-router/src/lib.rs) · [router.rs](file:///d:/Chimera%20CLI/crates/faae-router/src/router.rs) · [expert.rs](file:///d:/Chimera%20CLI/crates/faae-router/src/expert.rs) · [edsb.rs](file:///d:/Chimera%20CLI/crates/faae-router/src/edsb.rs) |
| **关键类型** | `FaaeRouter` · `Expert` · `ExpertRegistry` · `EdsBalancer` · `ExpertSelection`                                                                                                                                                                                                          |
| **关键方法** | `FaaeRouter::route()` · `FaaeRouter::register_expert()`                                                                                                                                                                                                                                 |
| **主要依赖** | tokio · serde · rand · dashmap · uuid · chrono · tracing · nexus-core · event-bus · nexus-contracts · thiserror                                                                                                                                                                         |

#### [sesa-router](file:///d:/Chimera%20CLI/crates/sesa-router)

| 项        | 说明                                                                                                                                                                                                                                                                                                                                                                                       |
| -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L6 Router                                                                                                                                                                                                                                                                                                                                                                                |
| **核心职责** | 子专家稀疏激活(SESA)，三层路由(前置条件→掩码→稀疏度→激活)                                                                                                                                                                                                                                                                                                                                                       |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/sesa-router/src/lib.rs) · [sparsity.rs](file:///d:/Chimera%20CLI/crates/sesa-router/src/sparsity.rs) · [prerequisite.rs](file:///d:/Chimera%20CLI/crates/sesa-router/src/prerequisite.rs) · [mask.rs](file:///d:/Chimera%20CLI/crates/sesa-router/src/mask.rs) · [activation.rs](file:///d:/Chimera%20CLI/crates/sesa-router/src/activation.rs) |
| **关键类型** | `SesaRouter` · `SparsityController` · `PrerequisiteChecker` · `MaskApplier` · `ActivationFunc` · `ActivationResult`                                                                                                                                                                                                                                                                      |
| **关键方法** | `SesaRouter::activate()` · `SparsityController::compute_threshold()`                                                                                                                                                                                                                                                                                                                     |
| **主要依赖** | tokio · serde · anyhow · thiserror · dashmap · tracing · nexus-core · event-bus                                                                                                                                                                                                                                                                                                          |

#### [omega-learner](file:///d:/Chimera%20CLI/crates/omega-learner)

| 项          | 说明                                                                                                                                                                                                                                                                                                  |
| ---------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**    | L6 Router                                                                                                                                                                                                                                                                                           |
| **核心职责**   | LinUCB Bandit 学习层;嫁接 gsoe-evolution / auto-dpo 的 RHI-CG 通道;异步下发 `SelectorPolicy::Learned` 给调用方,本地 fallback 保证可用性;R1 召回配额影子模式(CQL/IQL),R2 形式化验证器落地前冻结                                                                                                                                                |
| **关键文件**   | [lib.rs](file:///d:/Chimera%20CLI/crates/omega-learner/src/lib.rs) · [bandit.rs](file:///d:/Chimera%20CLI/crates/omega-learner/src/bandit.rs) · [policy.rs](file:///d:/Chimera%20CLI/crates/omega-learner/src/policy.rs) · [shadow.rs](file:///d:/Chimera%20CLI/crates/omega-learner/src/shadow.rs) |
| **关键类型**   | `OmegaLearner` · `LinUCB` · `SelectorPolicy::{Static,Learned,Hybrid}` · `ShadowObserver` · `RewardSignal`                                                                                                                                                                                           |
| **关键方法**   | `OmegaLearner::recommend()` · `LinUCB::update()` · `ShadowObserver::compare()`                                                                                                                                                                                                                      |
| **主要依赖**   | tokio · serde · ndarray · rand · tracing · **nexus-contracts** (L0) · **event-bus** (L1)                                                                                                                                                                                                            |
| **依赖铁律**   | L6 → L0(ADR-033 扩展恒允许) + L6 → L1(原铁律允许);**不依赖 L7+**(符合 §2.2 铁律)                                                                                                                                                                                                                                     |
| **设计模式**   | Bandit · 影子模式(Shadow Mode)· 异步下发 + 本地 fallback                                                                                                                                                                                                                                                      |
| **ADR 来源** | ADR-031 (Harness-as-Spec + omega-learner 边界) · ADR-043 (R1 影子模式)                                                                                                                                                                                                                                    |

***

### 3.7 L7 Execution (5 crates,v2.28 新增 nexus-subagent)

#### [pvl-layer](file:///d:/Chimera%20CLI/crates/pvl-layer)

| 项        | 说明                                                                                                                                                                                                                                                                                              |
| -------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L7 Execution                                                                                                                                                                                                                                                                                    |
| **核心职责** | 生产者-验证者循环(PVL)，并行流式生成与验证，反馈闭环                                                                                                                                                                                                                                                                   |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/pvl-layer/src/lib.rs) · [producer.rs](file:///d:/Chimera%20CLI/crates/pvl-layer/src/producer.rs) · [verifier.rs](file:///d:/Chimera%20CLI/crates/pvl-layer/src/verifier.rs) · [feedback.rs](file:///d:/Chimera%20CLI/crates/pvl-layer/src/feedback.rs) |
| **关键类型** | `PvlLayer` · `Producer` · `Verifier` · `FeedbackLoop` · `ProduceResult` · `VerifyResult`                                                                                                                                                                                                        |
| **关键方法** | `PvlLayer::produce_and_verify()` · `FeedbackLoop::record()`                                                                                                                                                                                                                                     |
| **主要依赖** | tokio · serde · dashmap · futures · rand · chrono · uuid · tracing · nexus-core · event-bus                                                                                                                                                                                                     |

#### [gqep-executor](file:///d:/Chimera%20CLI/crates/gqep-executor)

| 项        | 说明                                                                                                                                                                                                                                                                                                      |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L7 Execution                                                                                                                                                                                                                                                                                            |
| **核心职责** | 聚集-查询执行协议(GQEP)，异步操作聚集、超时处理、批量执行                                                                                                                                                                                                                                                                        |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/gqep-executor/src/lib.rs) · [gatherer.rs](file:///d:/Chimera%20CLI/crates/gqep-executor/src/gatherer.rs) · [timeout.rs](file:///d:/Chimera%20CLI/crates/gqep-executor/src/timeout.rs) · [batch.rs](file:///d:/Chimera%20CLI/crates/gqep-executor/src/batch.rs) |
| **关键类型** | `GqepExecutor` · `Gatherer` · `TimeoutController` · `BatchExecutor` · `GatherResult`                                                                                                                                                                                                                    |
| **关键方法** | `GqepExecutor::gather()` · `GqepExecutor::execute_batch()`                                                                                                                                                                                                                                              |
| **主要依赖** | tokio · serde · dashmap · futures · rand · chrono · uuid · tracing · nexus-core · event-bus · qeep-protocol                                                                                                                                                                                             |
| **架构例外** | ⚠️ 跨层渗透例外(ADR-048 Accepted):L7→L4 依赖 qeep-protocol,接受现状,推迟至三环重组根治                                                                                                                                                                                                                                       |

#### [mtpe-executor](file:///d:/Chimera%20CLI/crates/mtpe-executor)

| 项        | 说明                                                                                                                                                                                                                                 |
| -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L7 Execution                                                                                                                                                                                                                       |
| **核心职责** | 多Token预测执行(MTPE)，伪预测加速、回退策略                                                                                                                                                                                                        |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/mtpe-executor/src/lib.rs) · [predictor.rs](file:///d:/Chimera%20CLI/crates/mtpe-executor/src/predictor.rs) · [fallback.rs](file:///d:/Chimera%20CLI/crates/mtpe-executor/src/fallback.rs) |
| **关键类型** | `MtpeExecutor` · `Predictor` · `FallbackStrategy` · `PredictionResult`                                                                                                                                                             |
| **关键方法** | `MtpeExecutor::predict()` · `MtpeExecutor::verify_and_fallback()`                                                                                                                                                                  |
| **主要依赖** | tokio · serde · dashmap · rand · tracing · nexus-core · event-bus                                                                                                                                                                  |

#### [ssra-fusion](file:///d:/Chimera%20CLI/crates/ssra-fusion)

| 项        | 说明                                                                                                                                                                                                                                     |
| -------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L7 Execution                                                                                                                                                                                                                           |
| **核心职责** | 黏液式快速适配(SSRA)，多策略融合引擎、模板系统                                                                                                                                                                                                             |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/ssra-fusion/src/lib.rs) · [fusion/engine.rs](file:///d:/Chimera%20CLI/crates/ssra-fusion/src/fusion/engine.rs) · [templates.rs](file:///d:/Chimera%20CLI/crates/ssra-fusion/src/templates.rs) |
| **关键类型** | `SsraFusionEngine` · `FusionStrategy` · `TemplateRegistry` · `FusionResult`                                                                                                                                                            |
| **关键方法** | `SsraFusionEngine::fuse()` · `SsraFusionEngine::adapt()`                                                                                                                                                                               |
| **主要依赖** | tokio · serde · serde\_json · rand · dashmap · chrono · uuid · tracing · nexus-core · event-bus                                                                                                                                        |

#### [nexus-subagent](file:///d:/Chimera%20CLI/crates/nexus-subagent)

| 项          | 说明                                                                                                                                                                                                                                                                                                                                                                                    |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**    | L7 Execution(v2.28 新增,workspace 第 43 个 crate;ADR-148 裁决层归属定案——执行层,同一引擎换参数)                                                                                                                                                                                                                                                                                                            |
| **核心职责**   | 类型化 SubAgent 运行时 + Task Auction 市场:coder/explore/plan 三类子代理(同一执行引擎换模型/工具集/权限/worktree 参数);Arena 竞争 + 竞价(bid → `min_by(cost/match)` 择胜);**禁嵌套**(`NestedSubAgentForbidden` L0 契约,编译期+运行期双断言,Swarm 规模上限 8);取消经 `CancellationToken` 四因传播(用户取消/超时/配额耗尽/父级撤销);与 mas-sched 分工:Auction 管短任务派发,Claim 管长任务租约                                                                                    |
| **关键文件**   | [lib.rs](file:///d:/Chimera%20CLI/crates/nexus-subagent/src/lib.rs) · [runtime.rs](file:///d:/Chimera%20CLI/crates/nexus-subagent/src/runtime.rs) · [auction.rs](file:///d:/Chimera%20CLI/crates/nexus-subagent/src/auction.rs) · [cancel.rs](file:///d:/Chimera%20CLI/crates/nexus-subagent/src/cancel.rs) · [types.rs](file:///d:/Chimera%20CLI/crates/nexus-subagent/src/types.rs) |
| **关键类型**   | `SubAgentRuntime` · `SubAgentHandle` · `SubAgentKind`(coder/explore/plan) · `SubAgentSpec`/`SubAgentProfile` · `TaskAuction` · `TaskOffer` · `Bid` · `CancellationToken`/`CancelReason` · `SWARM_LIMIT=8`                                                                                                                                                                             |
| **关键方法**   | `SubAgentRuntime::spawn()` · `TaskAuction::offer()`/`award()`(min\_by cost/match) · `CancellationToken::cancel()`                                                                                                                                                                                                                                                                     |
| **主要依赖**   | tokio · serde · thiserror · async-trait · nexus-core · nexus-contracts · event-bus(内部依赖 ≤6 门禁)                                                                                                                                                                                                                                                                                        |
| **设计模式**   | 类型化子代理(引擎复用+参数化) · Task Auction 市场 · CancellationToken 协作式取消 · 嵌套禁止双断言                                                                                                                                                                                                                                                                                                                |
| **ADR 来源** | ADR-148(层归属 D-P5 定案);对应 v4.0 WI-25、九源手册 W17-18、Phase 3 T9                                                                                                                                                                                                                                                                                                                             |

***

### 3.8 L8 Parliament (3 crates)

#### [parliament](file:///d:/Chimera%20CLI/crates/parliament)

| 项                      | 说明                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**                | L8 Parliament                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| **核心职责**               | 多模型议会，角色注册(Skeptic/Security/Execution)、辩论、投票、否决权、AHIRT红队;v2.10.0+ 协调度量全路径埋点 + StrategyCapGuard 推理悖论风控;v2.11.0+ 多维共识质量(`ConsensusQualityMetrics` 赞成/弃权/分歧/裕度/Skeptic 立场)+ override 度量盲区修复 + Arc 共享优化                                                                                                                                                                                                                                                                                                                                                                                                                  |
| **关键文件**               | [lib.rs](file:///d:/Chimera%20CLI/crates/parliament/src/lib.rs) · [debate.rs](file:///d:/Chimera%20CLI/crates/parliament/src/debate.rs) · [voting.rs](file:///d:/Chimera%20CLI/crates/parliament/src/voting.rs) · [veto.rs](file:///d:/Chimera%20CLI/crates/parliament/src/veto.rs) · [roles.rs](file:///d:/Chimera%20CLI/crates/parliament/src/roles.rs) · [ahirt.rs](file:///d:/Chimera%20CLI/crates/parliament/src/ahirt.rs) · [strategy\_cap.rs](file:///d:/Chimera%20CLI/crates/parliament/src/strategy_cap.rs)(v2.10.0+) · [override.rs](file:///d:/Chimera%20CLI/crates/parliament/src/override.rs)(v2.11.0+) |
| **关键类型**               | `Parliament` · `DebateChamber` · `VotingSystem` · `VetoPower` · `RoleRegistry` · `AhirtRedTeam` · `ParliamentRole` · `VoteValue` · `ConsensusQualityMetrics`(v2.11.0+) · `StrategyCapGuard`(v2.10.0+) · `StrategyCap`(枚举: Full/Simplified/FastPath)                                                                                                                                                                                                                                                                                                                                                                  |
| **关键方法**               | `Parliament::submit_proposal()` · `Parliament::debate()` · `Parliament::vote()` · `VetoPower::exercise()` · `deliberate_with_policy()`(v2.10.0+ 全路径埋点) · `deliberate_with_override()`(v2.11.0+ 度量盲区修复) · `consensus_quality()`(v2.11.0+ 多维质量派生) · `StrategyCapGuard::update_cap()`(v2.10.0+ 滞后带状态机)                                                                                                                                                                                                                                                                                                                  |
| **主要依赖**               | tokio · serde · dashmap · rand · chrono · uuid · tracing · nexus-core · event-bus · seccore                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| **v2.10.0-omega 关键变化** | (1) `deliberate_with_policy` 全路径 wall-clock 埋点(含 Vetoed 短路),`weighted_approval_rate`/`participation_rate` 随 `DebateCompleted` 事件上报;(2) `StrategyCapGuard` 消费 `CoordinationRatioReported`,滞后带(连续 3 次越阈降档 / 5 次回落升档 / 带内双清零),生效策略 = min(学习策略, 封顶)                                                                                                                                                                                                                                                                                                                                                                      |
| **v2.11.0-omega 关键变化** | (1) `override.rs` 三返回路径全覆盖(`deliberate_with_override`/`reopen_veto`);(2) `voting.rs::consensus_quality()` 派生 5 指标(分歧度 = 加权 position 方差 / 0.25 归一化 Popoviciu 上界);(3) `collect_opinions_filtered` Arc 共享消除 O(R×T) clone(50 任务基准归档);(4) `execute_delegation`/`execute_batch_delegation` 去重抽 `run_delegation_batch` 私有内核                                                                                                                                                                                                                                                                                                 |
| **关联 ADR**             | ADR-046 (ImmuneSystem facade) · ADR-063 (L8 协调度量接线闭环) · ADR-064 (L8 Parliament 深度优化第二轮)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |

#### [acb-governor](file:///d:/Chimera%20CLI/crates/acb-governor)

| 项        | 说明                                                                                                                                              |
| -------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L8 Parliament                                                                                                                                   |
| **核心职责** | 自适应预算治理器(ACB)，根据历史使用模式动态调整预算分配                                                                                                                  |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/acb-governor/src/lib.rs) · [governor.rs](file:///d:/Chimera%20CLI/crates/acb-governor/src/governor.rs) |
| **关键类型** | `AcbGovernor` · `BudgetAllocation` · `UsagePattern`                                                                                             |
| **关键方法** | `AcbGovernor::allocate()` · `AcbGovernor::record_usage()` · `AcbGovernor::adjust_tick()`                                                        |
| **主要依赖** | tokio · serde · dashmap · chrono · tracing · nexus-core · event-bus                                                                             |

#### [decb-governor](file:///d:/Chimera%20CLI/crates/decb-governor)

| 项        | 说明                                                                                                                                                                                                                               |
| -------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L8 Parliament                                                                                                                                                                                                                    |
| **核心职责** | 动态紧急预算治理器(DECB)，预算超限检测、溢出处理、紧急预算调整                                                                                                                                                                                               |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/decb-governor/src/lib.rs) · [governor.rs](file:///d:/Chimera%20CLI/crates/decb-governor/src/governor.rs) · [overflow.rs](file:///d:/Chimera%20CLI/crates/decb-governor/src/overflow.rs) |
| **关键类型** | `DecbGovernor` · `BudgetState` · `OverflowHandler` · `BudgetExceededReason`                                                                                                                                                      |
| **关键方法** | `DecbGovernor::check()` · `DecbGovernor::adjust()` · `OverflowHandler::handle()`                                                                                                                                                 |
| **主要依赖** | tokio · serde · dashmap · chrono · tracing · nexus-core · event-bus                                                                                                                                                              |

> **🔴 红线**: `BudgetExceeded` 事件的 `severity()` 必须返回 `EventSeverity::Critical`，强制走mpsc通道确保送达。

***

### 3.9 L9 Quest (6 crates,v2.28 新增 mas-sched · nexus-hook)

#### [quest-engine](file:///d:/Chimera%20CLI/crates/quest-engine)

| 项                      | 说明                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**                | L9 Quest                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| **核心职责**               | 长期任务引擎，Quest DAG管理、检查点持久化(LHQP)、TTG三级思考切换、仲裁层;v2.10.0+ L8 协调度量接线闭环(`metrics_sync` 订阅合并 + 多维共识质量消费)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| **关键文件**               | [lib.rs](file:///d:/Chimera%20CLI/crates/quest-engine/src/lib.rs) · [engine.rs](file:///d:/Chimera%20CLI/crates/quest-engine/src/engine.rs) · [dag.rs](file:///d:/Chimera%20CLI/crates/quest-engine/src/dag.rs) · [checkpoint.rs](file:///d:/Chimera%20CLI/crates/quest-engine/src/checkpoint.rs) · [ttg.rs](file:///d:/Chimera%20CLI/crates/quest-engine/src/ttg.rs) · [arbitration.rs](file:///d:/Chimera%20CLI/crates/quest-engine/src/arbitration.rs) · [control.rs](file:///d:/Chimera%20CLI/crates/quest-engine/src/control.rs) · [metrics\_sync.rs](file:///d:/Chimera%20CLI/crates/quest-engine/src/metrics_sync.rs)(v2.10.0+) |
| **关键类型**               | `QuestEngine` · `TaskDag` · `CheckpointManager` · `TtgGovernor` · `ArbitrationLayer` · `PendingCoordSample`(v2.10.0+)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| **关键方法**               | `QuestEngine::create_quest()` · `QuestEngine::execute()` · `CheckpointManager::save()` · `CheckpointManager::restore()` · `TtgGovernor::switch_mode()` · `spawn_metrics_subscriber()`(v2.10.0+, subscribe-before-spawn 红线守护)                                                                                                                                                                                                                                                                                                                                                                                                           |
| **主要依赖**               | tokio · serde · rmp-serde · rusqlite · dashmap · sha2 · hex · chrono · uuid · tracing · nexus-core · event-bus                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| **v2.10.0-omega 关键变化** | (1) `complete_quest` 经既有 builder 填充 `parliament_debate_latency_ms` / `delegation_overhead_ms` / `consensus_quality`;(2) `cancel_quest` 清理 `PendingCoordSample` 防泄漏;(3) 修复 TTG 切换延迟硬编码 0.0 缺口;(4) 修复 `with_metrics_config` 静默断开 EventBus 绑定的缺陷                                                                                                                                                                                                                                                                                                                                                                                          |
| **v2.11.0-omega 关键变化** | (1) `PendingCoordSample` 同步消费 `ConsensusQualityMetrics` 多维字段(divergence / abstention\_rate / consensus\_margin),仅观测不影响 `InferenceGainSample` 主 proxy 口径                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |

#### [gea-activator](file:///d:/Chimera%20CLI/crates/gea-activator)

| 项        | 说明                                                                                                                                                                                                                                                                                                            |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L9 Quest                                                                                                                                                                                                                                                                                                      |
| **核心职责** | 门控专家激活(GEA)，冲突检测、门控计算、CSA上下文开关激活                                                                                                                                                                                                                                                                              |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/gea-activator/src/lib.rs) · [activator.rs](file:///d:/Chimera%20CLI/crates/gea-activator/src/activator.rs) · [gating.rs](file:///d:/Chimera%20CLI/crates/gea-activator/src/gating.rs) · [conflict.rs](file:///d:/Chimera%20CLI/crates/gea-activator/src/conflict.rs) |
| **关键类型** | `GeaActivator` · `GatingNetwork` · `ConflictDetector` · `CsaSwitch` · `ActivationGate`                                                                                                                                                                                                                        |
| **关键方法** | `GeaActivator::activate()` · `ConflictDetector::detect()` · `GatingNetwork::compute_gates()`                                                                                                                                                                                                                  |
| **主要依赖** | tokio · serde · ndarray · dashmap · rand · tracing · nexus-core · event-bus · osa-coordinator · sesa-router                                                                                                                                                                                                   |

#### [efficiency-monitor](file:///d:/Chimera%20CLI/crates/efficiency-monitor)

| 项        | 说明                                                                                                                                                                                                                                                                                                                                    |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L9 Quest                                                                                                                                                                                                                                                                                                                              |
| **核心职责** | 效率监控，指标收集、告警、仪表盘数据聚合                                                                                                                                                                                                                                                                                                                  |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/efficiency-monitor/src/lib.rs) · [collectors.rs](file:///d:/Chimera%20CLI/crates/efficiency-monitor/src/collectors.rs) · [alerts.rs](file:///d:/Chimera%20CLI/crates/efficiency-monitor/src/alerts.rs) · [dashboard.rs](file:///d:/Chimera%20CLI/crates/efficiency-monitor/src/dashboard.rs) |
| **关键类型** | `EfficiencyMonitor` · `MetricsCollector` · `AlertManager` · `DashboardData` · `BudgetMetrics` · `RouterStats`                                                                                                                                                                                                                         |
| **关键方法** | `EfficiencyMonitor::collect()` · `EfficiencyMonitor::check_alerts()` · `EfficiencyMonitor::dashboard_snapshot()`                                                                                                                                                                                                                      |
| **主要依赖** | tokio · serde · prometheus-client · dashmap · chrono · tracing · nexus-core · event-bus                                                                                                                                                                                                                                               |

#### [chimera-mas](file:///d:/Chimera%20CLI/crates/chimera-mas)

| 项                      | 说明                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**                | L9 Quest                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| **核心职责**               | 多 Agent 协同工作子系统(MAS),层级化递归委托编排、独立上下文隔离、Agent 生命周期管理、孙代理四象限稳定分工、WSJF 优先级调度、精英专家团队编制、Part II 闭环能力(上下文预算/分块调度/三级归档/知识协同/稳定闭环/PDCA 基准/INV-7/INV-8 不变量)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| **关键文件**               | [lib.rs](file:///d:/Chimera%20CLI/crates/chimera-mas/src/lib.rs) · [orchestrator.rs](file:///d:/Chimera%20CLI/crates/chimera-mas/src/orchestrator.rs) · [quadrant.rs](file:///d:/Chimera%20CLI/crates/chimera-mas/src/quadrant.rs) · [scheduler.rs](file:///d:/Chimera%20CLI/crates/chimera-mas/src/scheduler.rs) · [experts.rs](file:///d:/Chimera%20CLI/crates/chimera-mas/src/experts.rs) · [agent/meta.rs](file:///d:/Chimera%20CLI/crates/chimera-mas/src/agent/meta.rs) · [delegation.rs](file:///d:/Chimera%20CLI/crates/chimera-mas/src/delegation.rs) · [context/budget\_model.rs](file:///d:/Chimera%20CLI/crates/chimera-mas/src/context/budget_model.rs) · [chunker.rs](file:///d:/Chimera%20CLI/crates/chimera-mas/src/chunker.rs) · [archive/mod.rs](file:///d:/Chimera%20CLI/crates/chimera-mas/src/archive/mod.rs) · [knowledge/mod.rs](file:///d:/Chimera%20CLI/crates/chimera-mas/src/knowledge/mod.rs) · [stability.rs](file:///d:/Chimera%20CLI/crates/chimera-mas/src/stability.rs) · [pdca.rs](file:///d:/Chimera%20CLI/crates/chimera-mas/src/pdca.rs) · [invariants.rs](file:///d:/Chimera%20CLI/crates/chimera-mas/src/invariants.rs) |
| **关键类型**               | `RootOrchestrator` · `AgentMeta` · `AgentType` · `AgentContext` · `AgentTask` · `TaskComplexity` · `Quadrant` · `QuadrantPlan` · `PriorityScheduler` · `ExpertRegistry` · `MasError` · `MemoryBudgetModel`(§15) · `AdmissionGate`(§15/INV-7) · `TaskChunker`(§16) · `BatchExecutor`(§16) · `ArchiveTier`(§17/INV-8) · `ExpertConsultant`(§18) · `MutualInquirer`(§18) · `WikiRetriever`(§18) · `StabilityGuard`(§19) · `CircuitBreaker`(§19) · `DegradationChain`(§19) · `PdcaLoop`(§20) · `PdcaMetrics`(§20) · `InvariantChecker`(§21)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| **关键方法**               | `RootOrchestrator::delegate()` · `RootOrchestrator::delegate_quadrants()` · `activated_quadrants()` · `wsjf_score()` · `AgentContext::build_prompt()` · `AdmissionGate::check()`(§15/INV-7) · `TaskChunker::chunk()`(§16) · `InvariantChecker::check_inv8_archive_monotonicity()`(§17/INV-8) · `WikiRetriever::search()`(§18) · `StabilityGuard::apply_degradation()`(§19) · `PdcaLoop::check()`/`act()`/`plan_reflux()`(§20)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| **主要依赖**               | tokio · serde · thiserror · chrono · uuid · tracing · futures · regex · nexus-core · event-bus · hcw-window · osa-coordinator · quest-engine · gqep-executor · qeep-protocol · mlc-engine · cmt-tiering · scc-cache · repo-wiki · faae-router · model-router · parliament · acb-governor · decb-governor · efficiency-monitor                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| **设计模式**               | 层级递归委托 · 四象限稳定分工(INV-3/4) · WSJF 优先级调度 · 包装模式(AgentTask wrapper) · 独立上下文隔离 · 上下文预算 + 派生准入闸(INV-7) · 任务复杂度分块调度 · 三级归档单调性(INV-8) · 专家咨询/互询/Wiki 检索知识协同 · 稳定闭环降级链 + CircuitBreaker · PDCA 端到端闭环 + criterion 基准 · 不变量编码(InvariantChecker)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| **v2.10.0-omega 关键变化** | (1) `AgentTask` 新增 `quest_id` 归因字段(`#[serde(default)]` 兼容)+ `with_quest()` builder;(2) 委托批次 wall-clock 聚合(非 duration 求和,并行不重复计费);(3) `DelegationCompleted` 事件携带真实开销,`quest-engine::metrics_sync` 消费合并到 `delegation_overhead_ms` 字段                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| **v2.11.0-omega 关键变化** | (1) `execute_delegation`/`execute_batch_delegation` 去重:`run_delegation_batch` 私有内核(仅 agent\_id 中缀/文案参数化),公开 API 零变更;(2) `chimera-mas/benches/delegation_bench` 新增 fan-out 1/4/16 三档基准                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| **相关 ADR**             | [ADR-026](file:///d:/Chimera%20CLI/docs/architecture/ADR-026-chimera-mas-subsystem.md) · [ADR-027](file:///d:/Chimera%20CLI/docs/architecture/ADR-027-chimera-mas-quadrant.md) · [ADR-028](file:///d:/Chimera%20CLI/docs/architecture/ADR-028-chimera-mas-part2-closure.md) · [ADR-063](file:///d:/Chimera%20CLI/docs/architecture/ADR-063-l8-coordination-metrics-closure.md) · [ADR-064](file:///d:/Chimera%20CLI/docs/architecture/ADR-064-l8-parliament-deep-polish-round2.md)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |

**Part II §15-§21 子模块清单(ADR-028)**:

| 子模块                       | 章节  | 核心类型                                                                                                              | 职责                                     |
| ------------------------- | --- | ----------------------------------------------------------------------------------------------------------------- | -------------------------------------- |
| `context/budget_model.rs` | §15 | `MemoryBudgetModel` · `AdmissionGate` · `TokenBudget`                                                             | 上下文预算 + HCW 稀疏化 + INV-7 派生准入闸          |
| `chunker.rs`              | §16 | `TaskChunker` · `BatchExecutor` · `BatchConfig` · `ChunkOutput`                                                   | 任务复杂度分块 + 分批调度                         |
| `archive/`                | §17 | `ArchiveTier` · `compressor.rs` · `scheduler.rs` · `tier.rs`                                                      | Agent 记忆三级归档(1mo/3mo/6mo)+ INV-8 单调性   |
| `knowledge/`              | §18 | `ExpertConsultant` · `MutualInquirer` · `WikiRetriever` · `KnowledgeChain`                                        | 知识协同:专家咨询 + 同僚互询 + Wiki FTS5/KNN 检索    |
| `stability.rs`            | §19 | `StabilityGuard` · `CircuitBreaker` · `DegradationChain` · `DegradationStep` · `PressureSource` · `TerminalState` | 稳定闭环 + 三类压力源降级链 + 熔断器(零孤儿)             |
| `pdca.rs`                 | §20 | `PdcaLoop` · `PdcaMetrics` · `PdcaAdjustments` · `PlanReflux` · `PdcaAlert` · `AlertThresholds`                   | PDCA 端到端闭环强化 + 4 条告警规则 + criterion 基准  |
| `invariants.rs`           | §21 | `InvariantChecker` · `ArchiveTier` · `MEMORY_BUDGET_MB` · `MEMORY_BUDGET_UTILIZATION`                             | INV-7/INV-8 不变量编码 + 1000 次 proptest 验证 |

**criterion 基准(5 项,§20)**: `window_select`(< 1ms) · `mlc_l2_knn_top10@4096`(< 5ms) · `wiki_knn@1000`(< 10ms) · `wiki_knn@10`(< 1ms) · `decay_compute`(< 1μs) · `50agent_mem_peak`(≤ 130MB)

#### [mas-sched](file:///d:/Chimera%20CLI/crates/mas-sched)

| 项          | 说明                                                                                                                                                                                                                                                                                                                                                           |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **架构层**    | L9 Quest(v2.28 新增,workspace 第 41 个 crate;ADR-145 裁决从 chimera-mas strangler 拆出,控制面/执行面分离)                                                                                                                                                                                                                                                                     |
| **核心职责**   | 多代理调度器**控制面**(纯调度,不碰工具执行):`PeerScheduler` trait 四原语 claim/renew\_lease/handoff/should\_run;`SimplePeerScheduler` 内存实现(租约表+配额+优先级);`ShadowScheduler` 影子包装(只决策不执行,决策日志 100% 可回放,逐位一致,Ω₂ 确定性);与 nexus-subagent 分工:Claim 管长任务租约(TodoClaim/Lease/Quota/Handoff),Auction 管短任务派发                                                                                    |
| **关键文件**   | [lib.rs](file:///d:/Chimera%20CLI/crates/mas-sched/src/lib.rs) · [scheduler.rs](file:///d:/Chimera%20CLI/crates/mas-sched/src/scheduler.rs) · [shadow.rs](file:///d:/Chimera%20CLI/crates/mas-sched/src/shadow.rs) · [types.rs](file:///d:/Chimera%20CLI/crates/mas-sched/src/types.rs) · [error.rs](file:///d:/Chimera%20CLI/crates/mas-sched/src/error.rs) |
| **关键类型**   | `PeerScheduler`(trait) · `SimplePeerScheduler` · `ShadowScheduler`/`ShadowLog` · `TodoClaim` · `Lease` · `Quota` · `Handoff`                                                                                                                                                                                                                                 |
| **关键方法**   | `claim()` · `renew_lease()` · `handoff()` · `should_run()` · `ShadowLog::replay()`                                                                                                                                                                                                                                                                           |
| **主要依赖**   | tokio · serde · thiserror · **仅 L0/L1**(nexus-contracts/nexus-core/event-bus,内部依赖 ≤3 门禁)                                                                                                                                                                                                                                                                     |
| **设计模式**   | strangler 拆分 · 控制面/执行面分离 · 影子模式可回放 · 无自旋(Instant 时间戳非忙等) · 禁 feature 标志                                                                                                                                                                                                                                                                                      |
| **ADR 来源** | ADR-145(层归属 D-P3 定案);对应 v4.0 WI-29、九源手册 W16、Phase 3 T2                                                                                                                                                                                                                                                                                                       |

#### [nexus-hook](file:///d:/Chimera%20CLI/crates/nexus-hook)

| 项          | 说明                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| ---------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**    | L9 Quest(v2.28 新增,workspace 第 42 个 crate;ADR-146 裁决挂靠 Quest 生命周期)                                                                                                                                                                                                                                                                                                                                                                                                                         |
| **核心职责**   | 用户可编程生命周期 Hook 系统:13+ `LifecycleEvent`(PreToolUse/PostToolUse/PreQuestTurn/PostQuestTurn 等);TOML 配置挂载 shell 命令 + 环境变量注入($TOOL\_NAME/$SESSION\_ID/$GOAL\_ID);PreToolUse 类 hook 非零退出码可拒否该次工具调用;hook 命令执行前经 seccore `ProcessFence` 沙箱校验(逃逸拒绝:写 /etc、越界网络)+ 项目信任提示(TrustLevel)+ 超时熔断(默认 5s,不阻主流程);每条触发记 `HookAudit`(可接 session-store)                                                                                                                                                         |
| **关键文件**   | [lib.rs](file:///d:/Chimera%20CLI/crates/nexus-hook/src/lib.rs) · [lifecycle.rs](file:///d:/Chimera%20CLI/crates/nexus-hook/src/lifecycle.rs) · [config.rs](file:///d:/Chimera%20CLI/crates/nexus-hook/src/config.rs) · [executor.rs](file:///d:/Chimera%20CLI/crates/nexus-hook/src/executor.rs) · [audit.rs](file:///d:/Chimera%20CLI/crates/nexus-hook/src/audit.rs) · [event\_bridge.rs](file:///d:/Chimera%20CLI/crates/nexus-hook/src/event_bridge.rs)(P4-T3 hook.\* 双轨注册,WI-21 联动) |
| **关键类型**   | `LifecycleEvent`(13+) · `HookConfig` · `HookExecutor` · `HookAudit` · `TrustLevel` · `HookEventBridge`                                                                                                                                                                                                                                                                                                                                                                                    |
| **关键方法**   | `HookExecutor::run()`(tokio::process + 超时) · `HookEventBridge::register()` · 非零退出码拒否判定                                                                                                                                                                                                                                                                                                                                                                                                    |
| **主要依赖**   | tokio · serde · toml · thiserror · nexus-core · nexus-contracts · event-bus · seccore(ProcessFence 沙箱)                                                                                                                                                                                                                                                                                                                                                                                    |
| **设计模式**   | 生命周期挂载点 · 同步 shell hook · 沙箱栅栏 + 信任分级 + 超时熔断 · 全量审计                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| **ADR 来源** | ADR-146(层归属 D-P4 定案);对应 v4.0 WI-24、九源手册 W16、Phase 3 T3                                                                                                                                                                                                                                                                                                                                                                                                                                    |

***

### 3.10 L10 Interface (7 crates,v2.28 新增 nexus-app-server)

#### [mca-gateway](file:///d:/Chimera%20CLI/crates/mca-gateway)

| 项          | 说明                                                                                                                                                                                                                 |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **架构层**    | L10 Interface                                                                                                                                                                                                      |
| **核心职责**   | MCA 多通道亲和网关(ADR-065,PANTHEON 计划)：三协议 Codec(OpenAI Chat/Anthropic Messages/OpenAI Responses)、spec 驱动通用厂商适配器、SSE 流式归一、能力协商取代名字嗅探(P1)                                                                                 |
| **关键文件**   | [lib.rs](file:///d:/Chimera%20CLI/crates/mca-gateway/src/lib.rs) · [gateway.rs](file:///d:/Chimera%20CLI/crates/mca-gateway/src/gateway.rs) · [error.rs](file:///d:/Chimera%20CLI/crates/mca-gateway/src/error.rs) |
| **关键类型**   | `McaGateway` · `McaGatewayConfig` · `AffinityError`(5 变体)                                                                                                                                                          |
| **关键方法**   | `McaGateway::register_spec()` · `McaGateway::lookup_spec()`(ArcSwap RCU 无锁读) · `VendorAdapter::invoke()`(非流式全周期)                                                                                                   |
| **主要依赖**   | tokio · serde · thiserror · toml · arc-swap · dashmap · reqwest(rustls) · rusqlite · nexus-contracts · event-bus                                                                                                   |
| **M4 状态**  | 全量落地:三协议 Codec + 七厂商 12 模型 spec 卡(affinity.d)+ SSE 归一器 + transport(熔断/限流/域名白名单)+ session 状态守恒(VerbatimThinking C9)+ 健康探针 EWMA + 能力协商引擎(三态降级);体验对等验收 6 测全绿;配额切换 E2E 3 测全绿                                           |
| **ADR 来源** | ADR-065 (MCA 总纲与 L10 网关) · ADR-066 (能力协商与三态降级)                                                                                                                                                                     |

#### [mcp-mesh](file:///d:/Chimera%20CLI/crates/mcp-mesh)

| 项        | 说明                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **架构层**  | L10 Interface                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| **核心职责** | MCP(Model Context Protocol)量子网格，服务器注册、量子纠缠/叠加/事务、SSRF防护                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/mcp-mesh/src/lib.rs) · [mesh.rs](file:///d:/Chimera%20CLI/crates/mcp-mesh/src/mesh.rs) · [server\_registry.rs](file:///d:/Chimera%20CLI/crates/mcp-mesh/src/server_registry.rs) · [quantum/entanglement.rs](file:///d:/Chimera%20CLI/crates/mcp-mesh/src/quantum/entanglement.rs) · [quantum/superposition.rs](file:///d:/Chimera%20CLI/crates/mcp-mesh/src/quantum/superposition.rs) · [quantum/transaction.rs](file:///d:/Chimera%20CLI/crates/mcp-mesh/src/quantum/transaction.rs) |
| **关键类型** | `McpMesh` · `ServerRegistry` · `QuantumEntanglement` · `QuantumSuperposition` · `QuantumTransaction` · `McpServer`                                                                                                                                                                                                                                                                                                                                                                                                             |
| **关键方法** | `McpMesh::register_server()` · `McpMesh::call_tool()` · `QuantumTransaction::commit()`                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| **主要依赖** | tokio · serde · anyhow · thiserror · tracing · uuid · chrono · dashmap · event-bus                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| **降级说明** | HTTP 传输未启用(无 reqwest/axum 依赖,实际为进程内消息网格 + 事件总线);SSRF 防护为本地白名单而非网络出口拦截                                                                                                                                                                                                                                                                                                                                                                                                                                                          |

#### [csn-substitutor](file:///d:/Chimera%20CLI/crates/csn-substitutor)

| 项        | 说明                                                                                                                                                                                                                                                                                                                                                   |
| -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L10 Interface                                                                                                                                                                                                                                                                                                                                        |
| **核心职责** | 能力替代网络(CSN)，降级链构建、相似度匹配、优雅降级                                                                                                                                                                                                                                                                                                                         |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/csn-substitutor/src/lib.rs) · [substitutor.rs](file:///d:/Chimera%20CLI/crates/csn-substitutor/src/substitutor.rs) · [degradation\_chain.rs](file:///d:/Chimera%20CLI/crates/csn-substitutor/src/degradation_chain.rs) · [similarity.rs](file:///d:/Chimera%20CLI/crates/csn-substitutor/src/similarity.rs) |
| **关键类型** | `CsnSubstitutor` · `DegradationChain` · `SimilarityMatcher` · `SubstitutionResult`                                                                                                                                                                                                                                                                   |
| **关键方法** | `CsnSubstitutor::find_substitute()` · `DegradationChain::next()`                                                                                                                                                                                                                                                                                     |
| **主要依赖** | tokio · serde · ndarray · dashmap · rand · tracing · nexus-core · event-bus                                                                                                                                                                                                                                                                          |

#### [chtc-bridge](file:///d:/Chimera%20CLI/crates/chtc-bridge)

| 项        | 说明                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| -------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L10 Interface                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| **核心职责** | 跨IDE工具兼容桥(CHTC)，VSCode/Vim/Emacs/IntelliJ/Zed适配器，枚举分发而非trait object                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| **关键文件** | [lib.rs](file:///d:/Chimera%20CLI/crates/chtc-bridge/src/lib.rs) · [bridge.rs](file:///d:/Chimera%20CLI/crates/chtc-bridge/src/bridge.rs) · [protocol.rs](file:///d:/Chimera%20CLI/crates/chtc-bridge/src/protocol.rs) · [adapters/vscode.rs](file:///d:/Chimera%20CLI/crates/chtc-bridge/src/adapters/vscode.rs) · [adapters/vim.rs](file:///d:/Chimera%20CLI/crates/chtc-bridge/src/adapters/vim.rs) · [adapters/emacs.rs](file:///d:/Chimera%20CLI/crates/chtc-bridge/src/adapters/emacs.rs) · [adapters/intellij.rs](file:///d:/Chimera%20CLI/crates/chtc-bridge/src/adapters/intellij.rs) · [adapters/zed.rs](file:///d:/Chimera%20CLI/crates/chtc-bridge/src/adapters/zed.rs) |
| **关键类型** | `ChtcBridge` · `IdeAdapter`(enum) · `BridgeProtocol` · `IdeType`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| **关键方法** | `ChtcBridge::connect()` · `ChtcBridge::send_command()`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| **主要依赖** | tokio · serde · serde\_json · dashmap · uuid · tracing · nexus-core · event-bus                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |

#### [chimera-tui](file:///d:/Chimera%20CLI/crates/chimera-tui)

| 项                              | 说明                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| ------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**                        | L10 Interface                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| **核心职责**                       | 终端仪表盘(27 面板循环 = PanelId::REGISTERED_FOCUS_ORDER 长度, 27 PanelId 枚举, 未注册 0（Timeline / Sysinfo 已接线）),基于 ratatui 的实时监控与双向控制;含设计手册权威源 `NEXUS_OMEGA_TUI_DESIGN_BIBLE.md`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| **关键文件**                       | [lib.rs](file:///d:/Chimera%20CLI/crates/chimera-tui/src/lib.rs) · [app.rs](file:///d:/Chimera%20CLI/crates/chimera-tui/src/app.rs) · [render.rs](file:///d:/Chimera%20CLI/crates/chimera-tui/src/render.rs) · [subscriber.rs](file:///d:/Chimera%20CLI/crates/chimera-tui/src/subscriber.rs) · [command\_palette.rs](file:///d:/Chimera%20CLI/crates/chimera-tui/src/command_palette.rs) · [panels/](file:///d:/Chimera%20CLI/crates/chimera-tui/src/panels) (20 个面板) · [viz/](file:///d:/Chimera%20CLI/crates/chimera-tui/src/viz) (5 个可视化组件) · [data/resource\_history.rs](file:///d:/Chimera%20CLI/crates/chimera-tui/src/data/resource_history.rs) · [data/metrics\_history.rs](file:///d:/Chimera%20CLI/crates/chimera-tui/src/data/metrics_history.rs) · [config/tui\_bible.rs](file:///d:/Chimera%20CLI/crates/chimera-tui/src/config/tui_bible.rs) |
| **关键类型**                       | `TuiApp` · `TuiDataSource` · `Panel`(trait) · `DataPipeline` · `EventSubscriber` · `TuiCommand` · `QuestAction` · `SortMode` · `TuiBible` · `LayoutTemplate` · `KeyBinding` · `VizChartKind` · `VizWidget` · `ResourceHistory` · `MetricSample` · `ThresholdLevel` · `gradient_color` · `MetricsHistory`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| **关键面板**                       | Quest · Parliament · Router · Memory · Security · Budget · Decay · MCP Nodes · CHTC · Health · Event Stream · Log · Help · **ResourceMonitor**(v1.7+) · **Timeline**(v1.7+) · **Sysinfo**(v1.8 新增) · **MetricsDashboard**(v1.8 新增) · **TaskManager**(v1.8 新增,`PanelId` 复用 Quest 共享数据源) · **OsaSparse**(v1.7+,ClvVector 关联)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| **v1.8 新增模块**                  | `data/resource_history.rs`(滑动窗口+中位数滤波) · `data/metrics_history.rs`(SQLite 历史持久化) · `config/tui_bible.rs`(Figment 4 源配置加载器) · `viz/`(`line_chart`/`heatmap`/`bar_chart`/`gauge`/`histogram`)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| **🆕 v3.1 引擎骨架** (ADR-029, M0) | `engine/{buffer,diff,style,writer,rect,compat}.rs`(双缓冲 diff + StylePool + DiffEngine + proptest 幂等) · `engine/layout/{presets,flex,constraint,node,engine}.rs`(Flexbox + 四模式 Ide/Chat/VimSplit/Focus) · `actions/{registry,descriptor,codegen,panel_menu}.rs` + `actions/domains/{quest,task,export,view,system,config}.rs`(Registry 单一事实源 + 六域分包 + MAX\_ACTIONS=40 熔断) · `input/router.rs`(5 态 RouterMode + 22 RouteTarget + 14 D 类快照测试) · `i18n/{mod,zh,en}.rs`(AtomicU8 运行时 + t! 宏 + Ctrl+L + zh 120 keys) · `components/{mod,traits}.rs`(L5 组件骨架 LayoutNode 树) · `v3-engine` feature flag(Cargo.toml:10-12,默认 off)                                                                                                                                                                                                                                      |
| **主要依赖**                       | tokio · serde · serde\_yaml · ratatui · crossterm · dashmap · chrono · futures · sysinfo · rusqlite · figment · tracing · nexus-core · event-bus                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| **测试规模**                       | 426 lib 单元测试 + 30+ 集成测试(`color_gradient_test` 11/`task_manager_test` 10/`sysinfo_panel_test` 4/`tui_bible_config_test` 3/`metrics_history_persistence_test` 3/`viz_components_test` 5/`metrics_dashboard_test` 3/`trend_charts_test` 9/`resource_monitor_panel_test` 4)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |

#### [chimera-cli](file:///d:/Chimera%20CLI/crates/chimera-cli)

| 项        | 说明                                                                                                                                                                                                                                                                                                                                                        |
| -------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**  | L10 Interface                                                                                                                                                                                                                                                                                                                                             |
| **核心职责** | CLI主入口，命令解析、配置加载、子命令分发                                                                                                                                                                                                                                                                                                                                    |
| **关键文件** | [main.rs](file:///d:/Chimera%20CLI/crates/chimera-cli/src/main.rs) · [lib.rs](file:///d:/Chimera%20CLI/crates/chimera-cli/src/lib.rs) · [cli.rs](file:///d:/Chimera%20CLI/crates/chimera-cli/src/cli.rs) · [config.rs](file:///d:/Chimera%20CLI/crates/chimera-cli/src/config.rs) · [commands/](file:///d:/Chimera%20CLI/crates/chimera-cli/src/commands) |
| **关键类型** | `Cli` · `Commands`(enum) · `ChimeraConfig`                                                                                                                                                                                                                                                                                                                |
| **子命令**  | `run` · `tui` · `chat` · `quest`(list/show/cancel/checkpoint) · `config`(init/list/show/path) · `wiki` · `parliament` · `mcp`(list/serve/call/inspect) · `audit` · `agent`(list/spawn/inspect/cancel) · `doctor` · `completions`                                                                                                                          |
| **关键方法** | `commands::dispatch()` · `config::load()`                                                                                                                                                                                                                                                                                                                 |
| **主要依赖** | tokio · clap · figment · serde · tracing · tracing-subscriber · anyhow · chimera-tui · nexus-core · event-bus · quest-engine · repo-wiki · parliament                                                                                                                                                                                                     |

#### [nexus-app-server](file:///d:/Chimera%20CLI/crates/nexus-app-server)

| 项          | 说明                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| ---------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **架构层**    | L10 Interface(v2.28 新增,workspace 第 39 个 crate;WI-01 核心-表面分离,预算 48/53)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| **核心职责**   | 宿主层协议门面:对外提供稳定外部协议(**JSON-RPC v1**,协议冻结 ≥3 个月,扩展走 `extras` 逃逸舱),对内以 `CoreOp/CoreEvent` 单向驱动核心;**NexusEvent 永不进外部协议**(内闭:内部事件走 EventBus;外开:外部只经 AppOp/AppEvent,转译在 server 层);每 Thread 一 actor(会话状态归 actor 独占);断线恢复(客户端持 last\_item\_id 重连回放增量,Item 为最小 I/O 单元,状态机 started→in\_progress→completed/failed);设计源对标 Codex CLI app-server / OpenCode serve-attach / DSH headless 五形态                                                                                                                                                                                                                                                                                        |
| **关键文件**   | [lib.rs](file:///d:/Chimera%20CLI/crates/nexus-app-server/src/lib.rs) · [server.rs](file:///d:/Chimera%20CLI/crates/nexus-app-server/src/server.rs) · [protocol.rs](file:///d:/Chimera%20CLI/crates/nexus-app-server/src/protocol.rs) · [transport.rs](file:///d:/Chimera%20CLI/crates/nexus-app-server/src/transport.rs) · [sse.rs](file:///d:/Chimera%20CLI/crates/nexus-app-server/src/sse.rs) · [backend.rs](file:///d:/Chimera%20CLI/crates/nexus-app-server/src/backend.rs) · [approval.rs](file:///d:/Chimera%20CLI/crates/nexus-app-server/src/approval.rs) · [subagent\_engine.rs](file:///d:/Chimera%20CLI/crates/nexus-app-server/src/subagent_engine.rs) |
| **关键类型**   | `AppServer`/`AppServerConfig` · `AppOp`/`AppEvent` · `CoreOp`/`CoreEvent` · `ThreadActor` · `Item`(最小 I/O 单元) · `SseTransport` · `ApprovalGate` · `SubAgentEngine`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| **关键方法**   | `AppServer::new()` · `ThreadActor::handle_op()` · 增量回放 `replay_since(last_item_id)` · SSE 推送                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| **主要依赖**   | tokio · serde · serde\_json · thiserror · axum/tokio-stream(SSE,以 Cargo.toml 为准) · nexus-core · nexus-contracts · event-bus · mas-sched · nexus-subagent                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| **设计模式**   | 核心-表面分离(内闭外开) · Actor 模型(每 Thread 一 actor) · 协议版本冻结 + extras 逃逸舱 · 断线增量回放                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| **ADR 来源** | WI-01(v4.0 §6.1/§6.2/§13)、九源手册 W1;L0 `app.rs` 契约(ADR-054 P9 上提)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |

### 3.11 生产可达性与冻结孤岛清单(ADR-160,2026-08-29 棘轮)

> **口径**:从 `chimera-cli` 生产依赖图反向可达(**dev-dependencies 与 feature-gated/optional 边不计入**)。43 crate = **28 生产可达 + 14 冻结孤岛 + 1 GATED(mca-gateway，仅 `--features mca` 编译，ADR-177 重分类)**。登记为孤岛 = 承认其为"可见技术债",不阻塞合并;新出现的未登记不可达 crate 会让 `scripts/check_crate_reachability.sh` 非零退出。三条偿还路径:① 接入组合根 `chimera-cli/Cargo.toml`;② 在消费方声明 `optional=true` + cargo feature(先例 ADR-065 决策 6 + `--features mca` CI job);③ 保持不接线但在 ADR 记录理由(shadow-first/迁移)。权威清单以 `scripts/crate_reachability_freeze.txt` 为准。

| 孤岛 crate          | 层   | 冻结类别       | 不可达原因(摘要)                                                                                 | 依据      |
| ----------------- | --- | ---------- | ----------------------------------------------------------------------------------------- | ------- |
| `mas-sched`       | L9  | 刻意预留       | shadow-first,从 chimera-mas 拆出待经 L0 契约接管                                                   | ADR-145 |
| `nexus-hook`      | L9  | 刻意预留       | 空配置 = 当前行为,安全回退路径                                                                         | ADR-146 |
| `mca-gateway`     | L10 | feature 门控 | 仅根 `mca` feature + ci mda job 编译;无消费方(连 optional 都未声明),ADR-065 决策 6 装配期注入未落地,默认 binary 不含 | ADR-065 |
| `model-router`    | L1  | 历史孤岛       | 入边仅 auto-dpo(自身孤岛)+ quest-engine dev-dep;规则文档"CAF 已落地"在生产图上不成立                            | ADR-160 |
| `auto-dpo`        | L5  | 历史孤岛       | L5 进化环三 crate 之一,生产入边为零(RL 闸门禁止 Python 服务实体)                                              | ADR-160 |
| `scc-cache`       | L3  | 历史孤岛       | 入边仅 gea-activator/hcw-window dev-dep + mca-gateway(孤岛)                                    | ADR-160 |
| `csn-substitutor` | L10 | 历史孤岛       | 能力降级链,入边仅根 E2E dev-dep                                                                    | ADR-160 |
| `gea-activator`   | L9  | 历史孤岛       | 入边仅根 E2E dev-dep(Phase 6 W0 层归属更正为 L9)                                                    | ADR-160 |
| `sesa-router`     | L6  | 历史孤岛       | 入边仅根 E2E dev-dep                                                                          | ADR-160 |
| `decb-governor`   | L8  | 历史孤岛       | 入边仅 parliament dev-dep                                                                    | ADR-160 |
| `acb-governor`    | L8  | 历史孤岛       | 零入边(连 dev-dep 都没有,仅根 workspace.dependencies 声明)                                           | ADR-160 |
| `chtc-bridge`     | L10 | 历史孤岛       | 5 IDE 适配器,入边仅根 E2E dev-dep                                                                | ADR-160 |
| `lsct-tiering`    | L3  | 历史孤岛       | 入边仅根 E2E dev-dep                                                                          | ADR-160 |
| `mtpe-executor`   | L7  | 历史孤岛       | 入边仅 gea-activator dev-dep                                                                 | ADR-160 |
| `ssra-fusion`     | L7  | 历史孤岛       | 入边仅根 E2E dev-dep(Phase 6 W0 层归属更正为 L7)                                                    | ADR-160 |

> ★ Insight:**"零 Stub / 全部实现" ≠ "已装配"**。14 个冻结孤岛(另 1 个 GATED=mca-gateway, ADR-177)单测与 E2E 全绿、代码完整,但不在 `chimera-cli` 生产二进制的反向依赖图上——这是"实现完成度"与"装配可达性"两个正交维度。ADR-160 用棘轮把这一差异显式化、冻结化,避免文档把"写了"误报成"上线了"。

***

## 4. 核心领域类型

> 权威源: [nexus-core/src/types.rs](file:///d:/Chimera%20CLI/crates/nexus-core/src/types.rs)

### 4.1 UserIntent — 用户意图

```rust
pub struct UserIntent {
    pub intent_id: String,           // UUIDv7
    pub raw_text: String,            // 用户输入原始文本
    pub multimodal_inputs: Vec<MultimodalInput>,
    pub risk_level: u8,              // 0-100 风险等级
}
```

**风险等级语义**:

* 0-30: 低风险，只读操作

* 31-70: 中风险，有副作用但可控

* 71-100: 高风险，需Parliament审议

### 4.2 Quest — 长期任务

```rust
pub struct Quest {
    pub quest_id: String,            // UUIDv7
    pub title: String,               // 人类可读标题
    pub tasks: Vec<Task>,            // DAG任务列表
    pub thinking_mode: ThinkingMode, // Fast/Standard/Deep
    pub checkpoint_id: Option<String>,
}
```

### 4.3 Task — 任务节点

```rust
pub struct Task {
    pub task_id: String,
    pub description: String,
    pub status: TaskStatus,          // Pending/Running/Completed/Failed
    pub dependencies: Vec<String>,   // 前置Task ID(DAG)
}
```

### 4.4 Checkpoint — 检查点

```rust
pub struct Checkpoint {
    pub quest_id: String,
    pub checkpoint_id: String,
    pub memory_snapshot_hash: String, // SHA-256 hex
    pub serialized_state: Vec<u8>,    // MessagePack序列化
    pub created_at: DateTime<Utc>,
}
```

**WHY MessagePack而非直接存Quest**: 支持版本演进，字段增减不破坏旧检查点。

### 4.5 CLV — 上下文潜在向量

```rust
pub struct CLV(Array1<f32>);  // 固定512维
```

**关键方法**:

* `CLV::zero()` → 零向量

* `CLV::from_vec(v: Vec<f32>)` → Result\<CLV, NexusError>

* `CLV::basis(index: usize)` → Option\<CLV>（one-hot 基向量 e(index);越界返回 None 而非 panic）

* `CLV::cosine_similarity(&self, other: &CLV)` → f32

* `cosine_similarity_slices(a: &[f32], b: &[f32])` → f32（**定义位于 L0** `nexus_contracts::util`，L1 以 `pub use` 重导出，`use nexus_core::cosine_similarity_slices` 路径不变）

**数值边界（统一 fail-safe：恒不返回 NaN）**: 零向量任一侧、以及分量溢出导致归一化结果非有限时，均返回 0.0 而非 NaN；`f32::clamp` 对 NaN 会原样放行，故溢出必须显式降级，否则 NaN 会泄进下游 Top-K 排序使顺序不确定。不等长输入取 `min(len)` 计算，不 panic。

### 4.6 ThinkingMode — TTG三级思考模式

```rust
pub enum ThinkingMode {
    Fast,     // 简单任务，低延迟
    Standard, // 常规任务，平衡
    Deep,     // 复杂任务，高深度
}
```

***

## 5. 事件系统

> 权威源: [event-bus/src/types.rs](file:///d:/Chimera%20CLI/crates/event-bus/src/types.rs)

### 5.1 双通道架构

| 通道            | 适用事件       | 实现                             | 语义           |
| ------------- | ---------- | ------------------------------ | ------------ |
| **broadcast** | Normal事件   | `tokio::sync::broadcast`       | 发布-订阅，可被背压丢弃 |
| **mpsc**      | Critical事件 | `Vec<UnboundedSender>` fan-out | 点对点，确保送达     |

### 5.2 EventMetadata — 事件元数据

```rust
pub struct EventMetadata {
    pub event_id: Uuid,           // UUIDv7(时间有序)
    pub timestamp: DateTime<Utc>,
    pub source: String,           // 发布者crate名
}
```

### 5.3 EventSeverity — 事件严重级别

```rust
pub enum EventSeverity {
    Normal,    // 可被背压丢弃
    Critical,  // 不可丢弃，走mpsc
}
```

### 5.4 Critical事件清单(必须走mpsc)

> **P2-12 同步(2026-07-28)+ 双清单口径修正(2026-08-30 代码级复算)**:本表为 **`severity()=Critical` 事件(17 个)**(v2.3.1-omega 基线 13 个 + MCA M0 新增 `AffinityQuotaExhausted`(ADR-065) + P1-5 新增 `FormalViolation`
> + Phase 10 W4 新增 `StopRulingIssued`/`ErrorSignatureMatched`(ADR-085 双清单对齐)),
> 权威源为 [`NexusEvent::severity()`](file:///d:/Chimera%20CLI/crates/event-bus/src/classification.rs#L46)(`classification.rs:46-91` 综合 match,types.rs 单表分类)。
> **mpsc 旁路清单为 severity-Critical 的子集(13 个,`bus.rs::is_critical_mpsc_event()` 唯一事实源)**:含 `SkepticVeto/RedTeamAudit/BudgetExceeded/AgentTaskFailed/AsaIntervention/AffinityQuotaExhausted/R2FreezeViolation/R2FreezeRollbackFailed/FormalViolation/VetoOverridden/R1ShadowRollbackFailed/StopRulingIssued/ErrorSignatureMatched`。
> **现状注记**:`CheckpointSaved/ConsensusReached/SlowConsumerDropped/OrphanCallDetected` 4 个为 severity-Critical 但**未列入 mpsc 旁路**(记忆/协商/慢消费者/孤儿检测语义,依赖 broadcast 重订阅恢复;若需强投递保证须在 `is_critical_mpsc_event()` 登记并同步红线)。历史:`VetoOverridden`/`R1ShadowRollbackFailed` 于 Phase 10 W5 补入旁路;`FormalVerificationFailed` 按 ADR-159 定稿为 `GsoeError` 变体、非事件。
>   **v2.10.0-omega 同步(2026-07-31)**:在当前 v2.10.0 基线中 Critical 事件清单保持 13 不变;新增 3 个
>   **Normal 级**观测事件(`DebateCompleted` / `DelegationCompleted` /
>   `ParliamentStrategyCapChanged`),专门为 L8 协调度量接线闭环服务,不污染
>   Critical 旁路通道语义(回归面最小化)。v2.11.0-omega 同步(2026-07-31):
>   `DebateCompleted` 追加 `#[serde(default)]` 的 divergence/abstention\_rate/
>   consensus\_margin Option 字段(零破坏,见 ADR-064 M2 多维共识质量)。
>   **v2.13.0-omega (MCA M0) 同步**:新增 `AffinityQuotaExhausted` Critical 事件,总数 13 → 14。

| 事件                       | 原因                                            | 新增来源                  |
| ------------------------ | --------------------------------------------- | --------------------- |
| `CheckpointSaved`        | 丢失将导致Quest无法恢复                                | Week 2                |
| `ConsensusReached`       | 议会共识必须送达,丢失导致执行不一致                            | Week 2                |
| `SlowConsumerDropped`    | 慢消费者被丢弃必须通知,丢失导致订阅方状态漂移                       | Week 4                |
| `OrphanCallDetected`     | 孤儿调用检测(对应 Claude Code 尸检 5.4% 孤儿调用教训)         | Week 4                |
| `SkepticVeto`            | 安全否决必须送达,丢失导致高风险操作继续执行                        | Week 5                |
| `VetoOverridden`         | 否决覆盖审计不可丢失,丢失导致覆盖行为不可追溯                       | P1-3                  |
| `RedTeamAudit`           | 红队审计结果必须送达,丢失导致安全机制失效                         | Week 5                |
| `BudgetExceeded`         | 预算超限必须触发治理,丢失导致资源持续消耗至OOM                     | F-001 修复              |
| `AgentTaskFailed`        | Agent任务失败影响Quest完整性,丢失导致失败无人响应                | ADR-026(v2.0.0)       |
| `AsaIntervention`        | ASA安全干预必须送达,丢失导致高风险操作继续执行                     | P1-W2.1.4             |
| `R1ShadowRollbackFailed` | R1影子模式回滚失败,丢失导致退化策略持续生效                       | P4-W16.2.2            |
| `R2FreezeViolation`      | R2冻结违反等同安全事件(奖励黑客风险立即生效)                      | ADR-042 决策 4          |
| `R2FreezeRollbackFailed` | R2回滚失败意味着R2路径代码可能仍在生效                         | ADR-042 决策 4          |
| `AffinityQuotaExhausted` | 厂商额度耗尽必须触发降级链切换,丢失导致请求持续打向死通道                 | ADR-065 (MCA M0)      |
| `FormalViolation`        | 形式化验证违反,丢失导致契约违反无人审议、候选继续进入后续阶段               | P1-5                  |
| `StopRulingIssued`       | 停止裁决丢失导致 Quest 无界运行,必须保证投递到 quest-engine 取消路径 | Phase 10 W4 (v2.27.0) |
| `ErrorSignatureMatched`  | 错误签名匹配丢失导致 Debug 算子无法检索同签名兄弟                  | Phase 10 W4 (v2.27.0) |

> **🔴 红线**:
>
> * `BudgetExceeded.severity()` 必须返回 `Critical`(F-001 修复,Hard Constraint 第 10 条)
>
> * `AgentTaskFailed.severity()` 必须返回 `Critical`(ADR-026,§6.2 红线)
>
> * `R2FreezeViolation` / `R2FreezeRollbackFailed` 必须返回 `Critical`(ADR-042 决策 4)
>
> * `StopRulingIssued` / `ErrorSignatureMatched` 必须返回 `Critical`(Phase 10 §16.4,ADR-085 双清单对齐)
>
> - 权威源:`NexusEvent::severity()` 方法(`classification.rs:46-91` 综合 match;types.rs 单表分类,`event_types.rs` 镜像已按 ADR-160 退役),通过显式 `match` 分支(非通配符)确保新增 Critical 事件必须修改此方法

### 5.5 核心事件变体(部分)

| 事件                                                    | 发布方→订阅方                              | 载荷关键字段                                                                                                                  |
| ----------------------------------------------------- | ------------------------------------ | ----------------------------------------------------------------------------------------------------------------------- |
| `UserIntentEncoded`                                   | L10→L9                               | intent\_id, raw\_text, risk\_level                                                                                      |
| `QuestCreated`                                        | L9→L8                                | quest\_id, title, task\_count                                                                                           |
| `ThinkingModeSwitched`                                | L9→L8                                | quest\_id, from\_mode, to\_mode, reason                                                                                 |
| `CheckpointSaved` \[Critical]                         | L9→存储                                | quest\_id, checkpoint\_id, hash                                                                                         |
| `OmniSparseMasksComputed`                             | L6→L2/L3                             | masks(五维度)                                                                                                              |
| `ConsensusReached` \[Critical]                        | L8→L5                                | proposal\_id, outcome                                                                                                   |
| `BudgetExceeded` \[Critical]                          | L8→全系统                               | budget\_type, current, limit                                                                                            |
| `SkepticVeto` \[Critical]                             | L8→L4                                | proposal\_id, veto\_reason                                                                                              |
| `RedTeamAudit` \[Critical]                            | L4→L8                                | audit\_result, vulnerabilities                                                                                          |
| `AgentTaskFailed` \[Critical]                         | L9→L8/L4                             | agent\_id, task\_id, error                                                                                              |
| `SecurityAuditCompleted`                              | L4→监控                                | audit\_result, risk\_score                                                                                              |
| `MemoryMetricsReported`                               | L2→L9                                | tier\_stats, hit\_rate                                                                                                  |
| `RouterStatsUpdated`                                  | L9→L10                               | hit\_rate, p50/p95/p99 latency                                                                                          |
| `QuestCompleted`                                      | L9→L10                               | quest\_id, status(Completed/Failed/Cancelled)                                                                           |
| **`DebateCompleted`** \[v2.10.0+ Normal]              | L8→L9/efficiency-monitor             | debate\_id, weighted\_approval\_rate, participation\_rate, latency\_ms, divergence, abstention\_rate, consensus\_margin |
| **`DelegationCompleted`** \[v2.10.0+ Normal]          | L9 chimera-mas→L9/efficiency-monitor | batch\_id, agent\_count, total\_overhead\_ms, quest\_id                                                                 |
| **`ParliamentStrategyCapChanged`** \[v2.10.0+ Normal] | L8→efficiency-monitor                | from\_cap, to\_cap, reason, ratio\_at\_change                                                                           |

### 5.6 Phase 10 §16 跨层协同闭环新增事件(v2.27.0-omega 已收编)

> **状态声明 (2026-08-20 收编)**:以下 8 个变体为 **Phase 10 §16 跨层协同闭环审计修复**(对应
> `docs/reports/phase10-cross-layer-closure-report.md` W1-W7)的**正式发布新增**,随 **v2.27.0-omega**
> (2026-08-19 发布)已提交并**收编入权威口径**。本文档权威口径现为 **144 变体**(types.rs 单表枚举 + metadata() 分类,2026-08-19 实测;event_types.rs 镜像按 ADR-160 退役);§5.6 从"在途提示"转为"发布记录",8 个事件已并入权威基线,
> `check_doc_consistency.ps1` 的 \[GAP-F2] 随收编自然消除(权威口径 = 144 = 工作区实测)。

| 事件                                  | 域归属             | 发布方→订阅方                                                                                       | 载荷关键字段                                                 | Wave |
| ----------------------------------- | --------------- | --------------------------------------------------------------------------------------------- | ------------------------------------------------------ | ---- |
| `StopRulingIssued` \[Critical]      | L9 Quest 停止姿态   | `chimera-cli/quest_loop.rs` (ThreeFactorAdjudicator)→`quest-engine/control.rs`(cancel\_quest) | quest\_id, reason, payload                             | W4   |
| `VariantApproved`                   | L5 GSOE 变体批准    | `quest_loop.rs`→`faae-router/variant_subscriber.rs`(批准注册表)                                    | variant\_id, approval                                  | W4   |
| `ParentSelected`                    | L9 终局父本         | `quest_loop.rs`→`faae-router/variant_subscriber.rs`(register\_visit 同步 UCB)                   | parent\_id, child                                      | W4   |
| `ErrorSignatureMatched` \[Critical] | L4 零信任错误签名      | `seccore/error_signature_collector.rs::extract_and_publish`→组合根订阅器(待 Debug 算子路由装配)            | signature, context                                     | W4   |
| `TokenLedgerRecorded`               | L1 token 账本     | token\_ledger 记录路径→L3 持久化                                                                     | ledger\_id, delta                                      | W4   |
| `AssessmentUpdated`                 | L9 自我评估         | RuntimeAuditor 周期报告(组合根装配)→L9 策略调整                                                            | assessment, score                                      | W4   |
| `BusThroughputReported`             | L1 EventBus 吞吐量 | `event-bus/bus.rs::spawn_throughput_reporter`→订阅器                                             | published\_total, rate                                 | W6   |
| `SecurityInterceptionReported`      | L4 沙箱拦截率        | `seccore/interception_stats.rs` + `sandbox.rs::spawn_interception_reporter`→订阅器               | total\_requests, blocked\_requests, interception\_rate | W6   |

> **诚实数据原则 (v4.0 预留)**:`ErrorSignatureMatched` 的消费端、L10 用户满意度、L4 误拦截率、
> RLTrajectory 下游训练消费均无真实数据源,**禁止实施伪造采集**,仅在真实通道上线后激活
> (见 `phase10-cross-layer-closure-report.md` §三)。

### 5.7 审计遗留修复接线登记(fix-audit-followup,2026-08-28)

> **来源**:docs/reports/audit-followup/(WS-1\~WS-5)。变更均为"接线/门禁/定稿",**144 变体总数不变**。

| 类别            | 项                                                                                                                                                                                                                                                                                               | 处置                                                                                                                        |
| ------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| B1 旁路生产者补齐    | `R2FreezeRollbackFailed`(Critical 旁路成员,此前 0 生产发布)                                                                                                                                                                                                                                               | auto-dpo `freeze_guard::attempt_rollback_with_guard` 回滚失败真实发布(metadata.source="auto-dpo:freeze\_guard");不触碰 R2 冻结关键词      |
| 幽灵事件接线 13     | `McpMessageReceived`(mcp-mesh)/`DecayMetricsReported`(decay-engine)/`ChtcAdapterStatus`(chtc-bridge)/`ClvSnapshotReported`(nexus-core CLV)/`BudgetMetricsUpdated`(decb-governor)                                                                                                                | 有 TUI 专用面板的 5 个补真实生产者(字段照 §5.2 定义,severity=Normal)                                                                        |
| 幽灵事件接线 13(续)  | `EvolutionTriggered`(gsoe)/`AuditLogged`(seccore Merkle)/`R1ShadowRegressionDetected`+`R1ShadowPromotionReady`(chimera-mas shadow)/`AffinityUnknownField`(repo-wiki)/`CacheAffinityApplied`(scc-cache)/`ContextBudgetAllocated`(chimera-mas INV-7)/`ActivationThresholdAdjusted`(gea-activator) | 语义点真实发布 + 单测断言投递                                                                                                          |
| 幽灵事件预留 1      | `BenchmarkMetricsCollected`                                                                                                                                                                                                                                                                     | **无自然发布流**(efficiency-monitor 仅 Prometheus 采样,efficiency 无 7 字段基准快照)→ 正式登记 **预留(未接线,禁删)**                                 |
| Critical 清单定稿 | `FormalVerificationFailed`                                                                                                                                                                                                                                                                      | 确认为 **`GsoeError`** **变体**(gsoe-evolution/src/error.rs:98),**非 NexusEvent**;**从 Critical 事件文档清单剔除并加注**(旁路口径 13 不变,无新增事件面) |
| C1 红线 #8      | `xts_top_k`(nexus-contracts/util)                                                                                                                                                                                                                                                               | 替换 7 站 8 处 `sort_by` 调用点(O(n),select\_nth\_unstable\_by)                                                                  |
| E1 门禁         | `check_perf_redlines.ps1` Part 3                                                                                                                                                                                                                                                                | 全量 bench 三态登记(gated/registered/dev-only),unknown=0 门禁                                                                     |

### 5.8 FormalVerificationFailed 定稿注记(2026-08-28)

审计(维度2)确认:`FormalVerificationFailed` 在代码库中是 **`GsoeError`** **的错误变体**(gsoe-evolution/src/error.rs:98,
由 aegis/critic 返回),**不是** `NexusEvent`。此前规则文档/红线 §5 将其列为 Critical 事件属"规格名 vs 实现名"漂移。
**定稿决定**:不新增虚假事件面——从 Critical 事件清单删除该称谓;若未来需要其走事件总线,须重新立项新增
NexusEvent 变体并注册 mpsc 旁路(禁止为对齐文档而伪造发布)。既有 Critical 旁路成员清单(13 个,`is_critical_mpsc_event`)不受影响。

### 5.9 微观重复收敛登记(第三轮冗余审计,2026-08-30)

> **来源**:`docs/reports/audit-redundancy/第三轮执行报告.md` + ADR-160"后续轮次进度"。
> **落点约定**:跨 crate 的**无副作用微观算法**(Top-K / 激活函数 / 分位数)一律收敛到 L0
> `nexus_contracts::util`,禁止在各 crate 再写本地副本;受 ADR-033 例外治理约束(见 §3.0)。

| 批次 | 收敛                     | 结果                                                                                                                                       |
| -- | ---------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| A  | `select_top_k_desc` ×3 | → `util::xts_top_k_by`(O(n));生产调用点归零,本地定义归零                                                                                              |
| B  | `sigmoid` ×2(字节级相同)    | → `util::sigmoid`;`gea-activator` 新增 L9→L0 边(依赖铁律允许)                                                                                     |
| C  | `percentile` ×14       | 分层:6 个 `tests/` 函数体 → `util::percentile_sorted<T: Copy>`;7 个 bench 按 ADR-159 决策 3 **冻结登记**(不删);`affinity_metrics.rs` 有意保留独立实现(O(n) 方法形态) |
| D  | `cheap_index` 余弦分叉     | **诊断不改码**:6 维分叉(输入类型/不等长语义/NaN/零向量阈值/clamp/4.2× 性能)须独立 ADR + TDD                                                                         |

**口径变更**:`percentile_sorted` 统一为 `round((n-1)·p)` 索引(原各站 `trunc(n·p)` 混用),索引差 ≤1 个样本;
已用 `--release -- --ignored` 实跑 16 项 p95 SLO 红线验证无回退。

> ⚠️ **测试方法学**:p95 红线测试全部标 `#[ignore]`,debug 模式 `cargo test` 会给出"ignored"的**假绿**;
> 唯一有效断言路径是 `cargo test --release -p <crate> -- --ignored`。

***

## 6. 依赖关系铁律

> 权威源: 本规则 §2.2

### 6.1 依赖方向

```
L(N) → L(N)   ✓ 同层互引允许
L(N) → L(N-1) ✓ 向下依赖允许
L(N) → L(N+1) ✗ 向上依赖禁止
L(N) ──event-bus── L(M)  ✓ 跨层通信只能走Event Bus
L(N) ──mcp-mesh─── L(M)  ✓ 跨进程通信只能走MCP Mesh
```

### 6.2 硬约束

1. **nexus-core最小依赖**: 不能直接import上层任何crate
2. **event-bus唯一跨层通道**: 所有状态变更必须通过事件类型广播
3. **违规必须拒绝**: 任何违反依赖方向的import必须被拒绝，除非有ADR记录特批
4. **dev-dependencies例外**: 测试代码可绕过生产依赖方向，但仅限`tests/`目录
5. **#!\[forbid(unsafe\_code)]**: 所有crate必须声明，crate级属性，不传播到依赖

### 6.3 已修正的历史违规(通过Event Bus)

| 违规编号  | 原违规路径                        | 修正方式 | 事件类型                      |
| ----- | ---------------------------- | ---- | ------------------------- |
| V1    | OSA→HCW 向上依赖                 | 事件通知 | `OmniSparseMasksComputed` |
| V2    | MLC→efficiency-monitor 跨层    | 事件通知 | `MemoryMetricsReported`   |
| V3/V4 | Parliament→GSOE/AutoDPO 向上依赖 | 事件通知 | `ConsensusReached`        |

***

## 7. 端到端数据流

```
用户输入
  ↓
[NMC Encoder] (L2) ─── 多模态编码 ───→ CLV (512维)
  ↓ UserIntentEncoded
[Quest Engine] (L9) ─── DAG分解 + TTG ───→ Quest + Tasks
  ↓ QuestCreated
[Parliament] (L8) ─── 多角色辩论 + 投票 ───→ 审议结果
  ↓ ConsensusReached / SkepticVeto
[OSA Coordinator] (L6) ─── 五维稀疏掩码 ───→ OmniSparseMasks
  ↓ OmniSparseMasksComputed
[HCW/MLC/CMT] (L2/L3) ─── 稀疏加载 ───→ 上下文/记忆/能力
  ↓
[KVBSR/FaaE/SESA] (L6) ─── 语义路由 ───→ 选中专家/能力
  ↓ ModelRouteSelected
[GEA Activator] (L9) ─── 门控激活 ───→ 激活门控信号
  ↓
[PVL Layer] (L7) ─── 并行流式生成验证 ───→ 候选输出
  ↓
[GQEP Executor] (L7) ─── 聚集执行 + QEEP保证 ───→ 结果
  ↓
[MTPE/SSRA] (L7) ─── 预测加速/策略融合 ───→ 最终输出
  ↓
[SecCore] (L4) ─── 沙箱执行 + 审计 ───→ 执行结果
  ↓
[RepoWiki/GSOE/AutoDPO] (L5) ─── 知识沉淀/进化/DPO ───→ 学习更新
  ↓ QuestCompleted / CheckpointSaved
[Event Bus] (L1) ─── 广播状态变更 ───→ 全系统
  ↓
[Chimera TUI] (L10) ─── 14面板实时展示 ───→ 用户
```

***

## 8. 关键设计模式

### 8.1 枚举分发(Enum Dispatch)优先于Trait Object

**WHY**: 避免Box<dyn Trait>的动态分发开销与vtable复杂度，编译期可穷举match。

**示例**: chtc-bridge的`IdeAdapter`是enum而非trait object：

```rust
pub enum IdeAdapter {
    VSCode(VSCodeAdapter),
    Vim(VimAdapter),
    Emacs(EmacsAdapter),
    IntelliJ(IntelliJAdapter),
    Zed(ZedAdapter),
}
```

**例外**: chimera-tui的`TuiDataSource`因CLI入口实例化便利性使用trait object。

### 8.2 Arc零拷贝共享

异步任务间共享只读数据使用`Arc::clone(&x)`(增量引用计数)，而非`Arc::new(self.x.clone())`(创建独立副本)。

### 8.3 双通道事件投递

* Normal事件: `tokio::sync::broadcast`，支持多订阅者，可被背压丢弃

* Critical事件: `Vec<UnboundedSender>` fan-out mpsc，点对点确保送达

### 8.4 spawn\_blocking隔离阻塞I/O

所有rusqlite调用必须包装在`tokio::task::spawn_blocking`中，避免阻塞async runtime。

**示例模式** (repo-wiki/scc-cache 79处遵循):

```rust
let result = tokio::task::spawn_blocking(move || {
    conn.execute("INSERT INTO ...", params![])?;
    Ok::<_, rusqlite::Error>(())
}).await??;
```

### 8.5 id\_newtype! 宏统一ID类型

所有实体ID使用newtype模式，类型安全，防止不同ID类型混用。

### 8.6 Top-K用select\_nth\_unstable (O(n))

禁止使用`sort_by`(O(n log n))做Top-K选择，必须用`select_nth_unstable`。

### 8.7 FuturesUnordered并发收集

优于`join_all`，减少内存占用，支持流式结果处理。

### 8.8 publish\_blocking()同步发布

sync方法(audit/verify\_security/switch\_tier)使用`publish_blocking`；async方法使用`publish().await`配合作用域MutexGuard。

***

## 9. 工程红线与实战教训

### 9.1 原始六条尸检红线(Claude Code教训)

| 问题    | Claude Code教训       | 本项目红线                                         |
| ----- | ------------------- | --------------------------------------------- |
| 函数太大? | `print.ts` 3167行神函数 | **单函数 ≤200行，超过必须拆模块**                         |
| 结果丢了? | 5.4%孤儿调用            | **所有异步操作必须有GQEP聚集/超时处理**                      |
| 裸奔?   | 命令插值+auth跳过         | **所有外部调用经SecCore沙箱+Decay衰减**                  |
| 竞态?   | void Promise无await  | **所有async必须await或spawn管理**                    |
| 功能乱?  | 44个未发布标志            | **禁止功能标志，用能力场自然进化替代**                         |
| 内存爆炸? | 1M Token暴力加载        | **必须经HCW分层+OSA稀疏化后再加载**(1M = 128K实际 + 8×稀疏压缩) |

### 9.2 Week 1-8 新增红线(违反即阻塞发布)

| 红线                                     | 教训来源                         | 说明                                                                         |
| -------------------------------------- | ---------------------------- | -------------------------------------------------------------------------- |
| **禁止持锁.await**                         | faae-router 4 Critical       | DashMap/Mutex写锁跨await导致死锁，必须快照→释放→await                                    |
| **rusqlite必须spawn\_blocking**          | repo-wiki/scc-cache 79处      | rusqlite非async，直接调用阻塞runtime                                               |
| **broadcast先subscribe再spawn**          | Week 6 SSRA + Week 7 4 crate | `bus.subscribe()`必须在`tokio::spawn()`之前同步调用，否则事件静默丢失                        |
| **BudgetExceeded severity = Critical** | C2修复                         | 禁止降级，必须返回`EventSeverity::Critical`                                         |
| **Critical安全事件用mpsc**                  | efficiency-monitor           | SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded必须用mpsc channel确保送达 |
| **sqlite-vec禁用**                       | ADR-005降级                    | sqlite-vec 0.1.9 binding需unsafe，改内存KNN(10-1000 entry scale)                |
| **Top-K用select\_nth\_unstable**        | 工程约定                         | O(n)替代O(n log n) sort\_by                                                  |
| **f32禁止隐式转f64比较**                      | sesa-router教训                | `0.4f32 as f64`精度膨胀导致稀疏度误判，全程保持f32                                         |

### 9.3 async反模式清单

1. ❌ 持锁跨`.await` → ✅ 锁内取快照→释放锁→await快照
2. ❌ async中直接调用rusqlite → ✅ `spawn_blocking`包装
3. ❌ `spawn`之后再`subscribe` → ✅ subscribe在spawn之前同步调用
4. ❌ `with_event_bus` consume bus后subscribe → ✅ 构造器内部subscribe或subscribe后再传入
5. ❌ `Arc::new(self.chains.clone())`创建独立副本 → ✅ `Arc::clone(&self.chains)`共享引用
6. ❌ fire-and-forget spawn无JoinHandle管理 → ✅ 幂等操作可接受；关键路径必须管理JoinHandle
7. ❌ sync方法中直接调用async publish() → ✅ sync方法用`publish_blocking()`

***

## 10. 构建、测试与运行

### 10.1 环境设置

**Windows PowerShell (首次配置)**:

```powershell
$env:CARGO_HOME = 'D:\Chimera CLI\.toolchain\cargo'
$env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'
$env:TMP = 'D:\Chimera CLI\tmp'
$env:TEMP = 'D:\Chimera CLI\tmp'
$env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"
```

或运行安装脚本自动配置:

```powershell
.\install.ps1 -SetupEnv
```

> **工具链已固化**: `.cargo/config.toml` 已入库（固化 linker=gcc / incremental=false / FTS5），日常开发无需手动设置 CARGO\_INCREMENTAL 或 linker。**`rust-toolchain.toml`** **刻意不入库**（全局单值配置会破坏跨平台 CI：MSVC 宿主展开为 MSVC、写死 GNU 三元组又令 Linux/macOS runner 失败）；GNU 工具链 channel 由 `install.ps1 -SetupEnv` 的项目本地 `rustup default stable-x86_64-pc-windows-gnu` 保证。; v2.21.0-omega (P9-T2) 新增 \[profile.test]: opt-level=0 + codegen-units=16 + debug=0 加速测试编译

### 10.2 常用构建命令

```powershell
# 快速类型检查(推荐日常使用)
cargo check --workspace

# 只检查单个crate
cargo check -p <crate-name>

# 完整构建
cargo build --workspace

# Release构建(体积优化，产物<50MB)
cargo build --workspace --release
```

### 10.3 测试命令

```powershell
# 全量测试(43 crate 单元+集成+E2E,累计 11564 tests / 0 failed(2026-08-31 当前工作树全量重测,485 test target;静态 #[test] 计数 11433,差值为 doctest+宏展开);演进 v2.27.0=10836 → v2.28.0=11564;更早 PROBE P-1.3 基线 8455 见 PROBE 进度报告)
cargo test --workspace

# 单crate测试
cargo test -p <crate-name>

# 单测试用例
cargo test -p <crate-name> <test-name>

# 显示测试输出
cargo test --workspace -- --nocapture

# 压力测试(#[ignore]标记)
cargo test --workspace -- --ignored --nocapture

# Release模式压力测试(性能阈值测试必须release模式)
cargo test --workspace --release -- --ignored --nocapture
```

**已注册的E2E/安全/压测target**:

* `week5_event_flow`, `week6_setup`, `week6_main_flow`, `week6_security`

* `week7_setup`, `week7_main_flow`, `week7_security`, `week7_stress`

* `quest_lifecycle`, `full_integration`, `stress_test`, `week8_final_acceptance`

* `owasp_top10`

运行单独E2E:

```powershell
cargo test --test week8_final_acceptance
cargo test --test owasp_top10
```

#### 测试加速工具(P9-T2/P9-T3,2026-08-05)

| 工具                 | 路径                                             | 说明                                                                                            |
| ------------------ | ---------------------------------------------- | --------------------------------------------------------------------------------------------- |
| 三档 nextest profile | `.config/nextest.toml`                         | `ci-fast`(PR 快轨) / `default`(=full) / `stress`(nightly 压测,test-threads=2)                     |
| 等待缩放宏              | `nexus-contracts::test_scale::scaled_timeout!` | `CHIMERA_TEST_TIMEOUT_SCALE` 环境变量驱动,clamp \[0.01,1.0],缺省 1.0 与原行为等价;ADR-033 "纯类型+零逻辑"约束唯一例外   |
| 测试编译加速             | `.cargo/config.toml [profile.test]`            | `opt-level=0` + `codegen-units=16` + `debug=0` + `[profile.test.build-override]`,冷全量编译 \~-75% |
| 基准采集脚本             | `scripts/bench_test_runtime.{sh,ps1}`          | ci-fast/default/stress 三档分派 + JSON 报告解析                                                       |
| 临时文件清理             | `scripts/clean_test_temp.ps1`                  | 清理 tmp/ 下 .tmp\* 残留(实测 47 目录)                                                                 |

**实测**: ci-fast 档 wall time 12.94s → 4.12s(-68.1%);深度优化后 3.629s(-71.9%)。qeep-protocol 36 处协议超时已替换为 scaled\_timeout!(P9-T3)。

### 10.4 Lint与格式

```powershell
# clippy(Windows下--jobs 2避免OOM)
$env:RUST_MIN_STACK = '33554432'
$env:CARGO_INCREMENTAL = '0'
cargo clippy --workspace --all-targets --jobs 2 -- -D warnings

# 格式化检查
cargo fmt --all -- --check

# 应用格式化
cargo fmt --all
```

### 10.5 基准测试

```powershell
# 单个crate的criterion基准
cargo bench -p <crate-name>
```

### 10.6 安全审计

```powershell
cargo audit --deny warnings `
  --ignore RUSTSEC-2026-0190 `
  --ignore RUSTSEC-2026-0002 `
  --ignore RUSTSEC-2024-0436 `
  --ignore RUSTSEC-2025-0141 `
  --ignore RUSTSEC-2025-0119
```

> 5个ignore为经评估确认不影响项目的间接依赖(与 audit.yml 保持一致)。

### 10.7 Fuzz测试(委托Linux CI)

Windows GNU环境无法运行cargo-fuzz(libFuzzer仅适配MSVC)，本地做静态检查:

```powershell
# fuzz配置静态检查
cargo check --manifest-path fuzz/Cargo.toml
```

实际fuzz运行由`.github/workflows/fuzz.yml`在tag推送时在Linux CI上执行(6 target × 300s,与 `fuzz/Cargo.toml` 同步)。

### 10.8 Docker

```powershell
# 构建镜像
docker build -t chimera-cli:local .

# 验证
docker run --rm chimera-cli:local --version
# 期望输出: chimera 2.28.0-omega
```

基础镜像: `gcr.io/distroless/cc-debian12`(无shell，nonroot UID 65532)\
镜像体积: < 100MB(release.yml断言)

### 10.9 运行CLI

```bash
# 查看版本
cargo run -p chimera-cli -- --version

# Wiki查询
cargo run -p chimera-cli -- wiki "查询关键词"

# 启动TUI(无子命令默认启动TUI)
cargo run -p chimera-cli
cargo run -p chimera-cli -- tui

# 单次任务
cargo run -p chimera-cli -- run "任务描述"

# 使用配置文件
cargo run -p chimera-cli -- --config ~/.chimera/omega.yaml wiki "查询"
```

安装后可直接使用:

```bash
chimera --version
chimera              # 默认启动TUI
chimera wiki "查询"
chimera tui
```

### 10.10 发布前检查清单

1. ✅ 确认`Cargo.toml` workspace.package.version与待发布tag一致
2. ✅ 确认`CHANGELOG.md`已存在对应版本汇总章节
3. ✅ `cargo check --workspace`
4. ✅ `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings`
5. ✅ `cargo fmt --all -- --check`
6. ✅ `cargo test --workspace`
7. ✅ `cargo test --workspace --release -- --ignored --nocapture`
8. ✅ `cargo audit --deny warnings --ignore RUSTSEC-2026-0190 --ignore RUSTSEC-2026-0002 --ignore RUSTSEC-2024-0436 --ignore RUSTSEC-2025-0141 --ignore RUSTSEC-2025-0119`
9. ✅ `cargo check --manifest-path fuzz/Cargo.toml`
10. ✅ Docker镜像验证(或降级验证`scripts/verify_docker_locally.ps1`)
11. ✅ `cargo build --workspace --release`确认binary体积<50MB
12. ✅ `git tag v<x.y.z>-omega && git push origin v<x.y.z>-omega`

***

## 11. 架构决策记录 (ADR)

> 权威源: 本节 + `docs/architecture/adr_index.md` + `docs/architecture/ADR-*.md`(物理文件,主编号至 **ADR-170**(ADR-161~169 为 Phase R 草案);Phase 1-5 治理 ADR 以四份合并档落档:ADR-095~134 / ADR-135~144 / ADR-145~152 / ADR-153~156,另有 ADR-157/158/159/160 单档;含 ADR-053 rev0\~rev4 多版本)
> **v2.28.0-omega 实证状态** (2026-08-30 穷举 43 个 Cargo.toml + types.rs 精确枚举 + 可达性棘轮):
>
> * **主编号至 ADR-170** (ADR-001~006 + ADR-026~037 + ADR-042~085 + ADR-086~094 融合裁决 + ADR-095\~160 Phase 1-5 治理 + ADR-161~169 Phase R 草案 + ADR-170 十一定律收录 + ADR-SIMD-001(预留);编号-主题映射以 `adr_index.md` 为准)
>
> * **NexusEvent 变体数 = 144(权威口径,types.rs 单表;`event_types.rs`** **镜像已按 ADR-160 决策 5 退役删除,分类真值源收敛为 types.rs 一处)**
>
> * **`#![forbid(unsafe_code)]`** **43/43 crate 全合规**(lib.rs 层声明;依赖 crate 内部 unsafe 如 rusqlite/prometheus-client 不传播)

| ADR         | 主题                                       | 决策                                                                                                                                                                                                                                       | 落地状态                                                                 | 代码实证                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| ----------- | ---------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| ADR-001     | 沙箱运行时选择                                  | 优先 gVisor,降级 seccomp+WASM                                                                                                                                                                                                                | ⚠️ **降级**                                                            | `seccore/Cargo.toml` **无 wasmtime 依赖**(ADR-035 决策 3:wasmtime 已下沉到 seccore 直接声明 + `wasm-sandbox` feature gate 隔离);实际为 `tokio::process::Command` + 策略过滤;`sandbox.rs:474` 注释"当前实现为降级版本"                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| ADR-002     | 能力衰减模型设计                                 | 连续权限流体模型,能力随时间/风险动态衰减                                                                                                                                                                                                                    | ✅ 落地                                                                 | `decay-engine` 3 个依赖(thiserror/dashmap/tracing),2 源文件                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| ADR-003     | Event Bus 实现选型                           | Tokio broadcast + mpsc 双通道                                                                                                                                                                                                               | ✅ 落地                                                                 | **144 个 NexusEvent 变体(权威口径,types.rs 单表 + metadata() 分类;event\_types.rs 镜像已按 ADR-160 退役)** (v2.3.1=74 → v3.1.0=+8 TuiAction/Chat → v5.0=+3 R1Shadow → polish-v2.7=+24 增量 → v2.10.0=+3 观测事件 → MCA M0=+6 Affinity 事件 → P9 演进=+11 → W10=+2 TuiHello/TuiHelloAck → Phase 10=+8 StopRuling/VariantApproved/ParentSelected/ErrorSignatureMatched/TokenLedgerRecorded/AssessmentUpdated/BusThroughputReported/SecurityInterceptionReported),broadcast + mpsc 旁路(Critical 事件);`arc-swap` RCU 原语保护内环共享状态(P2-W7.2.3);v2.11.0 新增 `publish_batch`/`publish_batch_blocking` 原语(摊销 receiver\_count 背压采样,N=5 降 16.3% / N=10 降 20.4%) |
| ADR-004     | 消息序列化协议                                  | MessagePack(rmp-serde)                                                                                                                                                                                                                   | ✅ 落地                                                                 | 18 个文件使用 rmp-serde,Checkpoint 持久化主流协议                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| ADR-005     | 持久化存储选型                                  | SQLite + 向量,sqlite-vec 降级为内存 KNN(10-1000 entry)                                                                                                                                                                                          | ⚠️ **部分降级**                                                          | `repo-wiki/Cargo.toml` L50 注释 `# sqlite-vec = { workspace = true }`(已注释);v5.0 P2 引入 `hnsw_rs` HNSW 近似最近邻(10K-100K entry scale,生产路径)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| ADR-026     | CHIMERA-MAS 多 Agent 协同子系统                | L9 Quest 层归属 + event-bus 扩展 + AgentTask wrapper                                                                                                                                                                                          | ✅ 落地                                                                 | `chimera-mas/Cargo.toml` 24 个内部 crate 依赖 + 5 个 Part II 章节;v2.0.0-omega Stage A 骨架                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| ADR-027     | CHIMERA-MAS 四象限 + 优先级调度                  | 孙代理四象限(INV-3/4) + WSJF + E01-E08 专家团队                                                                                                                                                                                                    | ✅ 落地                                                                 | `chimera-mas/src/quadrant.rs` + `scheduler.rs`;v2.1.0-omega Stage B                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| ADR-028     | CHIMERA-MAS Part II 闭环能力                 | 7 项闭环(上下文预算/分块/归档/知识/稳定/PDCA/INV)                                                                                                                                                                                                        | ✅ 落地                                                                 | `chimera-mas/src/{context,chunker,archive,knowledge,stability,pdca,invariants}.rs`;v2.2.0-omega;INV-7/INV-8 不变量                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| **ADR-029** | **AETHER TUI v3.1 交互式重构**                | **自研渲染引擎(纯 safe Rust,L3 双缓冲 diff/L4 Flexbox/L5 组件)+ Action 统一协议(8 变体 append-only)+ Registry 单一事实源 + InputRouter 5 态路由(Normal/Insert/Command/GPrefix/WPrefix)+ i18n 默认中文 +** **`v3-engine`** **feature 双轨迁移**                             | ✅ **M0 落地**                                                          | `chimera-tui/src/{actions,engine,components,i18n,input}/` 5 模块 + `event-bus` 8 变体(types.rs:1549-1655) + `chimera-cli` action\_orchestrator 订阅;commit `bf9aa75`(2026-07-21);M2 起 v3-engine feature 启用渲染                                                                                                                                                                                                                                                                                                                                                                                                               |
| **ADR-030** | **unsafe 红线不特批**                         | **安全等价物重写(arc-swap / crossbeam / bumpalo 替代 unsafe 库)**                                                                                                                                                                                  | ✅ 落地                                                                 | workspace 引入 `arc-swap = "1.7"`(event-bus L251-255);`#![forbid(unsafe_code)]` 43/43 crate 全覆盖                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| **ADR-031** | **Harness-as-Spec + omega-learner 边界**   | **C2 嫁接点命名映射表 + §5.2 九项裁决对账附录 + omega-learner 异步下发 SelectorPolicy::Learned**                                                                                                                                                             | ✅ 落地                                                                 | `omega-learner/src/{bandit,policy,shadow}.rs`;LinUCB + Shadow Mode + 本地 fallback                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| **ADR-032** | **双通道评估器(RHI-CG)**                       | **通道 A 提议 + 通道 B 否决(双通道互不覆盖)**                                                                                                                                                                                                           | ✅ 落地                                                                 | `auto-dpo/src/rhi_channel_a.rs` + `gsoe-evolution/src/ci_gate.rs`;P5.1/P5.2 完整闭环                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| **ADR-033** | **L0 nexus-contracts**                   | **纯类型 + 零逻辑 + 零依赖契约层,依赖铁律扩展** **`L(N) → L(0)`** **恒允许**                                                                                                                                                                                  | ✅ 落地                                                                 | `crates/nexus-contracts/src/lib.rs` 仅依赖 `serde` workspace;承载 `OmniSparseMasks` / `HarnessSpec` / `TemporalMeta` / `NamespaceQuota` / `SelectorPolicy` / `BudgetTier`(ADR-054 决策 3,P9-T3 上提) / `command_validation` 契约(`Command`/`CommandPolicy`/`AttackType` + `CommandValidator` trait,ADR-054 决策 3,P9-T4) + `domain` 契约(`ThinkingMode`/`MultimodalInput`/`UserIntent`/`Quest`/`Task`,ADR-054 决策 6,P9-T7) + `event_payload` 契约(`EventSeverity`/`TaskPriority`/`AgentStatus`,P9-T7)                                                                                                                                  |
| **ADR-034** | **灰度=能力场 + 编译期 feature**                 | **否决运行时 Feature Flag,采用 CapabilityToken 四态 + 编译期** **`v3-engine`** **/** **`wasm-sandbox`** **feature 双轨**                                                                                                                               | ✅ 落地                                                                 | `chimera-tui/Cargo.toml:10-12` `v3-engine` feature 默认 off;`seccore/Cargo.toml` `wasm-sandbox` feature gate                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| **ADR-035** | **威胁模型下修 + wasmtime 沙箱重启路径**             | **wasmtime 下沉到 seccore 直接声明 +** **`wasm-sandbox`** **feature gate 隔离 + 重启路径**                                                                                                                                                            | ✅ 落地                                                                 | `seccore/Cargo.toml` wasmtime 独立声明;`sandbox.rs:474` 注释降级说明 + 重启路径代码                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| **ADR-037** | **能力场灰度工程化方案**                           | **CapabilityToken 四态 + EWMA α=0.1 + AsaIntervention 安全闭环**                                                                                                                                                                               | ✅ 落地                                                                 | `parliament/src/capability_token.rs` + `acb-governor/src/ewma.rs`;P4-W14.5 实施                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| **ADR-042** | **R2 形式化验证器冻结**                          | **R2(GSOE×AutoDPO 约束 RL)FormalVerifier 落地前无条件冻结 + 5 项工程实施决策**                                                                                                                                                                            | ✅ 落地                                                                 | `gsoe-evolution/src/freeze.rs`;5 项决策:冻结范围 + 期限 + 三阶递进解冻 + 违反处置 + 工程硬约束                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| **ADR-043** | **R1 召回配额 CQL/IQL 影子模式设计**               | **影子模式开关 + 对比报告 + 2 周解冻条件 EWMA≥0.7/胜率≥71.4% + 回滚预案 4 项**                                                                                                                                                                                 | ✅ 落地                                                                 | `omega-learner/src/shadow.rs`;P4-W16.2.4 实施                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| **ADR-044** | **RHI-CG 双通道工程实施**                       | **JudgeClient + LlmInvoker + CiGate trait;P5.1 决策回溯 + P5.2 通道 B 预留**                                                                                                                                                                     | ✅ 落地                                                                 | `auto-dpo/src/rhi_judge_client.rs` + `gsoe-evolution/src/ci_gate.rs`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| **ADR-045** | **INV-9 命名调和**                           | **`check_inv9_delegation_acyclic`** **为权威名,解除 Channel B 实施约束**                                                                                                                                                                           | ✅ 落地                                                                 | `gsoe-evolution/src/invariants.rs` 命名修正                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| **ADR-046** | **ImmuneSystem facade 设计**               | **悖论三探针(memory\_paradox/reasoning\_trap/evolution\_hack) + 事件订阅镜像 + 不可进化面定义**                                                                                                                                                            | ✅ 落地                                                                 | `parliament/src/immune_system.rs` + 3 子探针模块;P5.3 完整落地                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| ADR-065     | MCA 总纲与 L10 mca-gateway 多通道亲和网关          | 网关落位 L10 独立 crate(第 38 个,仅依赖 L0/L1);三协议 Codec + spec 驱动适配器;流式数据面走 bounded mpsc 不进 event-bus                                                                                                                                              | ✅ 落地                                                                 | `mca-gateway/Cargo.toml` 第 38 个 workspace member;`gateway.rs` + `affinity/` 子模块;M4 全量落地                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| ADR-066     | 能力协商与三态降级协议                              | 三态降级(FullFidelity/DegradedNotified/ChannelRejected);TTG×七厂商思考映射数据化;会话状态守恒;健康探针 EWMA + 熔断互补                                                                                                                                               | ✅ 落地                                                                 | `mca-gateway/src/negotiation/` + `health.rs`;E5 哨兵 proptest 200 例                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| ADR-068     | 成本治理与峰谷模型                                | model-router 通道化;omega-learner s9 路由臂;acb-governor 成本模型(EWMA+CostVerdict);跨厂商级联降级 FrugalGPT                                                                                                                                              | ✅ 落地                                                                 | `model-router/src/channel.rs` + `omega-learner/src/arm9.rs` + `acb-governor/src/cost_model.rs`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| ADR-067     | 跨厂商级联与去相关议会                              | 跨厂商去相关(ProviderAffinityRegistry);厂商集中度免疫探针(EWMA>70% 告警);级联升级复用 ConsensusQualityMetrics                                                                                                                                                   | ✅ 落地                                                                 | `parliament/src/provider_affinity.rs` + `acb-governor/src/concentration_probe.rs`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| **ADR-054** | **Phase 9 三环循环元架构重组设计(2026-08-04 落档预备)** | **8 项决策:① 内环边界 9 候选不变(omega-learner/chimera-mas 留外环) ② L0/L1 三底座永久外环基础设施 ③ 2 条生产违规边解耦(quest→decb 类型上提 L0 / parliament→seccore 事件化) ④ 内环三层通信(共享内存+mpsc+EventBus) ⑤ 保持单 workspace+依赖审计脚本 ⑥ 病理处置确认(D3/D4 已消除) ⑦ 接口冻结+门禁发布 ⑧ criterion 四基准** | Proposed, **P9-T3/T4/T5/T7 部分落地(2 条生产违规边已消除,D1 首批下沉完成)(2026-08-04)** | 依据 `_blueprints/three-ring-reorg/Phase0_评估报告_v2.20.md`(P9-T1,38 crate 实况);权威索引见 `adr_index.md`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |

\| **ADR-086\~094** | **十源融合文档逐项裁决(v4.0 §2.5)** | **对《Agent\_CLI\_Architecture\_Fusion》9 项主张逐条裁决(采纳/增强/否决)** | ✅ 落档 | 见 v4.0 六编总案 §2.5;无独立物理文件,裁决并入 Phase 1 治理 |
\| **ADR-095\~134** | **Phase 1 治理奠基(40 条合并档)** | **ComputeBridge 双运行时(rayon 独立池替换 spawn\_blocking 桥接)、ShardedBus 分片总线双跑、CBMR 微批写、CausalGraph 归因、22 周计划奠基、无锁三件套等** | ✅ 落地 | `ADR-095-134-phase1-consolidation.md`;对应九源手册 W1-W8、v4.0 WI 系列 |
\| **ADR-132** | **CausalGraph 因果归因管道(补建)** | **causal.rs VectorClock(Lamport)+ diff 事件 5s 窗口因果链回溯;归因 P95 >5s 降级人工兜底** | ✅ 落地 | `ADR-132-causal-graph-attribution.md`(独立补建档,Phase 4 W22) |
\| **ADR-135\~144** | **Phase 2 治理(10 条合并档)** | **135 rayon 入 L1 批准 / 136 ShardedBus 首批 3 crate 接入 / 137·151 否决 3 个新建 crate(增强既有) / 138 合并登记口径 / 139 CSC→hcw-window / 140 RSB→mlc-engine / 141 session-store 新建 L3 / 142 RTL Shadow 转正流程 / 143 telemetry→event-bus 增强 / 144 decay 重构登记为架构限制(Amdahl 0.97×)** | ✅ 落地 | `ADR-135-144-phase2-governance.md`;W9 治理周 |
\| **ADR-145\~152** | **Phase 3 治理(8 条合并档)** | **145 mas-sched 拆出 L9(strangler) / 146 nexus-hook L9(seccore 沙箱) / 147 execpolicy 六模式映射 / 148 nexus-subagent L7(Task Auction+Swarm≤8+嵌套禁令) / 149 事件双轨注册表配额(≤64/空间,144 内置分毫不动) / 150 MCP client\_v2 适配层 / 151 否决 nexus-moe-router+nexus-learn 新建 / 152 WI-34 中期验收诚实口径** | ✅ 落地 | `ADR-145-152-phase3-governance.md`;W15-W18 |
\| **ADR-153\~156** | **Phase 4 治理(4 条合并档)** | **153 分片总线 Go/No-Go(双跑零 diff≥7 天→Go 全量 B 级) / 154 供应商漂移守卫(provider\_drift,只报警不熔断) / 155 WI-34 终验偏差登记 / 156 集成与影子双跑波次** | ✅ 落地 | `ADR-153-156-phase4-governance.md`;W21-W24 |
\| **ADR-157** | **利用率双口径三条件联合判定** | **probe 双侧负载 + TokioProbe.busy\_us(cfg 双口径);combined 0.552→0.999(release),三条件全达成** | ✅ 落地 | `ADR-157-dual-track-utilization-judgment.md`(P5-T1) |
\| **ADR-158** | **payload 级双跑比对** | **canonical\_fingerprint + compare\_cross\_instance 跨实例指纹比对** | ✅ 落地(13/13,含 50 组 proptest) | 登记于 `docs/reports/phase5-wave5-closure.md` T4,无独立物理文件 |
\| **ADR-159** | **审计遗留项系统性修复治理** | **fix-audit-followup 波次,审计发现项闭环登记** | ✅ 落地 | `ADR-159-audit-followup-governance.md` |
\| **ADR-160** | **生产可达性棘轮 + event\_types 镜像退役** | **43=28 可达+15 孤岛冻结清单(CI 棘轮,新孤岛未登记即失败);event-bus 分层子枚举镜像 event\_types.rs 退役删除,分类真值源收敛 types.rs 单表;依赖层图 .sh/.ps1 双源追平** | ✅ 落地 | `ADR-160-crate-reachability-gate-and-event-types-retirement.md` + `scripts/check_crate_reachability.sh` + `crate_reachability_freeze.txt` |
\| **ADR-170** | **正式收录 OMEGA 第 10/11 定律(Ω₁₀-Card / Ω₁₁-Synthesize)** | **Ω₁₀-Card=经验卡片数据结构定律(不可变+版本化+append-only 事件流);Ω₁₁-Synthesize=按需记忆合成算法定律(懒加载+非阻塞+Debug→同错误签名兄弟定向检索);守恒表述升级为"OMEGA 十一定律(九基座+两扩展)";原文编号拟 161 被孤岛偿还占用,顺延 170** | ✅ Accepted(2026-09-02) | `ADR-170-omega-11th-laws.md` |

### 11.1 ADR 降级追溯

**ADR-001 沙箱降级路径**:

```
设计目标(gVisor)
    ↓ [技术约束:Windows 平台 gVisor 不可用,gVisor0.4 仍处实验阶段]
降级方案:seccomp 风格进程级隔离 + tokio::process 沙箱
    ↓ [进一步约束:WASM 沙箱未启用,无 wasmtime 依赖]
当前实现:`seccore/src/sandbox.rs:474` 注释明确"当前实现为降级版本"
    ↓ 后续路径
可重启路径:引入 wasmtime + WasmEdge 重建 WASM 沙箱(gVisor 在 Linux CI 验证)
```

**ADR-005 向量检索降级路径**:

```
设计目标(SQLite + sqlite-vec HNSW)
    ↓ [技术冲突:sqlite-vec 0.1.9 Rust binding 需 sqlite3_auto_extension + unsafe]
冲突:违反 `#![forbid(unsafe_code)]` crate 级属性(§2.2 依赖铁律延伸,工程红线 §6.1)
    ↓ 降级方案
方案 1(当前):内存 KNN,适用 10-1000 entry scale(`repo-wiki/src/vector.rs`)
方案 2(未来):Week 6 NMC 集成后引入 HNSW/Annoy 专用向量索引
方案 3(待定):自定义 sqlite-vec 绑定,封装 unsafe 到独立 crate
```

***

## 12. 目录结构索引

### 12.1 根目录

```
D:\Chimera CLI\
├── .cargo/
│   └── config.toml              # Cargo配置(linker、incremental、SQLITE_ENABLE_FTS5)
├── .github/
│   └── workflows/
│       ├── audit.yml             # 每日cargo audit + PR触发
│       ├── fuzz.yml              # tag触发fuzz(6 target × 300s，Linux CI)
│       ├── release.yml           # tag触发5平台build + docker + release
│       └── test-install-scripts.yml
├── crates/                       # **43 个 crate 源码**(v2.28.0-omega,见 §3 索引 + §3.11 可达性清单;28 生产可达/15 冻结孤岛)
│   ├── nexus-core/          (L1)
│   ├── event-bus/           (L1)
│   ├── model-router/        (L1)
│   ├── nmc-encoder/         (L2)
│   ├── hcw-window/          (L2)
│   ├── mlc-engine/          (L2)
│   ├── scc-cache/           (L3)
│   ├── lsct-tiering/        (L3)
│   ├── cmt-tiering/         (L3)
│   ├── seccore/             (L4)
│   ├── decay-engine/        (L4)
│   ├── qeep-protocol/       (L4)
│   ├── repo-wiki/           (L5)
│   ├── gsoe-evolution/      (L5)
│   ├── auto-dpo/            (L5)
│   ├── osa-coordinator/     (L6)
│   ├── kvbsr-router/        (L6)
│   ├── faae-router/         (L6)
│   ├── sesa-router/         (L6)
│   ├── pvl-layer/           (L7)
│   ├── gqep-executor/       (L7)
│   ├── mtpe-executor/       (L7)
│   ├── ssra-fusion/         (L7)
│   ├── parliament/          (L8)
│   ├── acb-governor/        (L8)
│   ├── decb-governor/       (L8)
│   ├── quest-engine/        (L9)
│   ├── gea-activator/       (L9)
│   ├── efficiency-monitor/  (L9)
│   ├── chimera-mas/         (L9)
│   ├── mcp-mesh/            (L10)
│   ├── csn-substitutor/     (L10)
│   ├── chtc-bridge/         (L10)
│   ├── mca-gateway/         (L10)
│   ├── chimera-tui/         (L10)
│   └── chimera-cli/         (L10)
├── docs/
│   ├── architecture/
│   │   ├── README.md
│   │   └── CODE_WIKI.md       # 本文件
│   ├── grafana/
│   │   ├── README.md
│   │   └── dashboard.json
│   ├── release/
│   │   └── release_guide.md
│   └── tui/
│       └── README.md
├── examples/
│   ├── config.sample.toml
│   └── config.sample.yaml
├── fuzz/                         # Fuzz测试(独立workspace)
│   ├── Cargo.toml
│   └── fuzz_targets/
│       ├── cacr_budget_parse.rs
│       ├── checkpoint_deserialize.rs
│       ├── config_section_parse.rs
│       ├── event_serialize.rs
│       ├── quest_parse.rs
│       └── seccore_sandbox.rs
├── scripts/
│   ├── setup-gpg-signing.ps1
│   ├── verify-p0-cleanup.ps1
│   ├── check_fuzz_config.ps1
│   ├── verify_docker_locally.ps1
│   └── (对应的.sh脚本)
├── tests/                        # Workspace根E2E测试
│   ├── e2e/                      # 11个E2E测试
│   ├── security/
│   │   └── owasp_top10.rs       # OWASP A01-A10渗透测试
│   └── stress/
│       └── week7_stress.rs      # 1000次压测
├── Formula/
│   └── chimela.rb               # Homebrew formula
├── bucket/
│   └── chimela.json             # Scoop manifest
├── Cargo.toml                    # Workspace根配置(43 members)
├── Cargo.lock
├── Dockerfile                    # 多阶段distroless镜像
├── CHANGELOG.md                  # 版本演进史
├── README.md                     # 项目首页
├── LICENSE                       # Apache-2.0
├── install.ps1                   # Windows安装脚本
├── install.sh                    # Linux/macOS安装脚本
└── test_version_verification.ps1 # CI版本校验模拟
```

### 12.2 Crate内部标准布局

```
my-crate/
├── Cargo.toml          # version.workspace=true, edition.workspace=true
├── src/
│   ├── lib.rs          # 公开API导出 + #![forbid(unsafe_code)] + prelude
│   ├── types.rs        # 核心类型定义
│   ├── config.rs       # 配置解析(Figment多源)
│   ├── error.rs        # 错误类型(thiserror enum)
│   └── ...             # 功能子模块
├── benches/            # Criterion基准测试
│   └── *.rs
└── tests/              # 集成测试 + proptest
    ├── integration.rs
    └── proptest.rs
```

### 12.3 关键文档入口

| 文档                | 路径                                                                                                   | 用途                             |
| ----------------- | ---------------------------------------------------------------------------------------------------- | ------------------------------ |
| **CODE\_WIKI.md** | [docs/architecture/CODE\_WIKI.md](file:///d:/Chimera%20CLI/docs/architecture/CODE_WIKI.md)           | **本文件** — 架构权威源                |
| 项目规则              | [.trae/rules/nuxus规则.md](file:///d:/Chimera%20CLI/.trae/rules/nuxus规则.md)                            | 全局规则、红线、async/SQLite/安全        |
| 项目命令              | [.claude/CLAUDE.md](file:///d:/Chimera%20CLI/.claude/CLAUDE.md)                                      | 环境设置、常用命令、CI/CD                |
| CHANGELOG         | [CHANGELOG.md](file:///d:/Chimera%20CLI/CHANGELOG.md)                                                | Week 1-8验收记录 + v1.0.0-omega GA |
| README            | [README.md](file:///d:/Chimera%20CLI/README.md)                                                      | 项目首页、安装、快速开始                   |
| 从零搭建终极文档          | [AETHER\_NEXUS\_OMEGA\_从零搭建终极文档\_v3.md](file:///d:/Chimera%20CLI/docs/architecture/AETHER_NEXUS_OMEGA_从零搭建终极文档_v3.md)  | 工程实施升级参考(已落位 docs/architecture,2026-08-30 复核) |
| 模块级优化报告           | [AETHER\_NEXUS\_OMEGA\_模块级系统性优化分析报告.md](file:///d:/Chimera%20CLI/docs/architecture/AETHER_NEXUS_OMEGA_模块级系统性优化分析报告.md) | 模块优化主参考(已落位 docs/architecture,2026-08-30 复核) |

***

## 附录: 版本历史

| 版本                     | 日期                         | 主要里程碑                                                                                                                                                                                                                                                                                                                                                      |
| ---------------------- | -------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| v1.0.0-omega           | 2026-06-28                 | GA发布，34 crate全覆盖，3000+测试全绿                                                                                                                                                                                                                                                                                                                                 |
| v1.4.0-omega           | 2026-07-09                 | L1-L10全部34 crate功能完整，E2E测试体系建立                                                                                                                                                                                                                                                                                                                             |
| v1.5.7-omega           | 2026-07-12                 | 首个含GitHub Release artifacts的版本                                                                                                                                                                                                                                                                                                                             |
| v1.7.0-omega           | 2026-07-14                 | TUI完整重构(M0-M6)，P0安全修复，Scoop/Homebrew分发                                                                                                                                                                                                                                                                                                                     |
| v1.8.0-omega           | 2026-07-15                 | TUI企业级套件 + v3.1 M0 引擎骨架 + 8 项 TUI 决策                                                                                                                                                                                                                                                                                                                       |
| v2.0.0-omega           | 2026-07-22                 | `chimera-mas` 多 Agent 协同子系统(ADR-026,35→37 crate)                                                                                                                                                                                                                                                                                                           |
| v2.1.0-omega           | 2026-07-24                 | 四象限稳定分工 + WSJF 调度(ADR-027)                                                                                                                                                                                                                                                                                                                                 |
| v2.2.0-omega           | 2026-07-25                 | Part II 七项闭环能力(ADR-028,INV-7/INV-8)                                                                                                                                                                                                                                                                                                                        |
| v2.3.0-omega           | 2026-07-26                 | Phase A 架构审计 + Phase B TUI 收尾 + Phase C 治理                                                                                                                                                                                                                                                                                                                 |
| v2.3.1-omega           | 2026-07-26                 | 发布流程补救(patch)                                                                                                                                                                                                                                                                                                                                              |
| v2.4.0-omega           | 2026-07-26                 | `nexus-contracts` L0 + `omega-learner` L6(ADR-033)                                                                                                                                                                                                                                                                                                         |
| v2.8.0-omega           | 2026-07-29                 | polish-v2.7 Phase 1-6 合并发布(closure Stage A)                                                                                                                                                                                                                                                                                                                |
| v2.9.0-omega           | 2026-07-30                 | L10 深度打磨与跨层优化(P0/P1/P2/P3 全档)                                                                                                                                                                                                                                                                                                                              |
| v2.10.0-omega          | 2026-07-31                 | L8 协调度量接线闭环 + 推理悖论红线风控(ADR-063)                                                                                                                                                                                                                                                                                                                            |
| **v2.11.0-omega**      | **2026-07-31**             | **L8 Parliament 深度优化第二轮(ADR-064):M1 override 度量盲区修复 + M2 多维共识质量 + M3 悖论风险模型 + M4 委托可维护性;event-bus** **`publish_batch`** **原语;`arc-swap`** **优化 O(R×T) clone**                                                                                                                                                                                              |
| v2.12.0\~v2.13.0-omega | 2026-08-01                 | MCA PANTHEON + 形式化验证器 M0                                                                                                                                                                                                                                                                                                                                   |
| v2.14.0\~v2.19.0-omega | 2026-08-02                 | P2 Sprint 14 项任务全量交付                                                                                                                                                                                                                                                                                                                                       |
| v2.20.0-omega          | 2026-08-03                 | PROBE HCW-Sparse 深度优化完整闭环(P-1\~P3,38 crates,126 NexusEvent,ADR-070/071)                                                                                                                                                                                                                                                                                    |
| v2.21.0-omega          | 2026-08-04                 | CLI --help 规整化 + LLM 统一入口(`chimera llm` + `/llm` slash)                                                                                                                                                                                                                                                                                                    |
| v2.22.0-omega          | 2026-08-07                 | MCA token 效率深度优化(coalescing + token\_estimate + 亲和缓存)                                                                                                                                                                                                                                                                                                      |
| v2.24.0-omega          | 2026-08-08                 | Phase 9 三环循环元架构重组收尾(P9-T12)+ RUSTSEC-2026-0217/0222/0223 修复                                                                                                                                                                                                                                                                                                |
| v2.25.0-omega          | 2026-08-08                 | Milestone B 全部交付 B-1\~B-6(RL 共享类型/Ambient Mode/九层防御/PlatformGroundingSpec/Agent Grep CLI/关键路径) + Milestone C R2 解冻前置 + Milestone D RL 全栈三位一体闭环                                                                                                                                                                                                             |
| **v2.26.0-omega**      | **2026-08-11**             | **Concord TUI 重构 W0~W11 全部收尾:SlashCommandRegistry 53 命令注册 + `/` 一级整合(ADR-075) + Chat/Quest 双轨会话模式(ADR-076) + ApprovalMode 动态 Shift+Tab(ADR-074) + NewlineGate 闸门(ADR-078) + i18n 中英门户 + 10 份 ADR-074~083 落档;9954 passed / 0 failed**                                                                                                    |
| **v2.27.0-omega**      | **2026-08-19**             | **Phase 10 §16 跨层协同闭环审计修复正式发布(W1-W7:经验卡片组合根 + Quest 生命周期桥 + 卡片生成触发点 + 事件协议补齐 + mpsc 双清单对齐 + 合成闭环 + 奖励缺口);NexusEvent 136→144;10836 passed / 0 failed;权威基线升级(ADR-085 双态收编)**                                                                                                                                                                                 |
| **v2.27.1-omega**      | **2026-08-20**             | **GPG 签名补发 + MCA E2E 超时加固(无功能性变更)**                                                                                                                                                                                                                                                                                                                        |
| **v2.28.0-omega**      | **2026-08-28 起在途(未打 tag)** | **Phase 1-5 Ch12 波次 W1-W26 全部收尾:ComputeBridge 双运行时 + ShardedBus 分片总线双跑 + CausalGraph 归因(ADR-132)+ 供应商漂移守卫 + 利用率双口径(ADR-157)+ payload 双跑(ADR-158);新增 5 crate(39 app-server/40 session-store/41 mas-sched/42 nexus-hook/43 nexus-subagent,38→43);ADR-095\~160 治理;ADR-160 可达性棘轮(28 可达/15 孤岛)+ event\_types.rs 镜像退役;11564 passed / 0 failed;\[2.28.1] 在途补丁登记** |

***

## 13. 8 位资深专家分布式深度分析摘要 (Code Wiki v2.0 专属)

> **生成日期**: 2026-07-23(初版)· 2026-07-30(v2.8.0-omega 基线同步)· 2026-07-31(v2.11.0-omega 基线同步)· 2026-08-01(v2.13.0-omega 同步)· 2026-08-02(v2.19.0-omega 同步)· 2026-08-05(v2.21.0-omega 同步)· 2026-08-09(v2.25.0-omega 同步)· 2026-08-11(v2.26.0-omega 同步)· 2026-08-20(v2.27.1-omega 同步)· **2026-08-30(v2.28.0 在途同步)**
> **生成方法**: superpowers-main 极致深度思考 + staff-engineer-mode 按 surface 路由专家 + 6 个并行 Task 子代理深度源码分析 + 7 处 Cargo.toml 实证修正
> **分析基线**: `Cargo.toml` workspace.package.version = `2.28.0-omega`(代码实况,2026-08-30 核验,工作区 feat/phase1-w1-w8 在途,最新已发 tag v2.27.0-omega(注:v2.27.1-omega 为 CHANGELOG-only 补丁,本地与 origin 均无 tag)),**43 members(28 生产可达 + 15 ADR-160 冻结孤岛)**,**11564 tests / 0 failed(2026-08-31 当前工作树全量重测,485 test target)**,**144 NexusEvent 变体(types.rs 单表,event\_types.rs 镜像已退役)**
> **时点声明(2026-08-30)**:本节 §13.2~13.6 的**明细数据为 Code Wiki v2.0(2026-07-23)编制时的 v1.x/v2.19 时点快照**(如"TUI 17 面板""38 crate 中 33 依赖"等),保留为历史证据;一切当前口径以本文档 §1(身份/测试规模)、§3(43 crate 索引 + §3.11 孤岛)、§5(144 事件)与顶部三方一致块为准。

### 13.1 专家团队组建

| 专家 ID   | 角色     | 10+ 年经验方向    | 重点分析 Surface                                                                                                           |
| ------- | ------ | ------------ | ---------------------------------------------------------------------------------------------------------------------- |
| **E01** | 首席架构师  | 分布式系统/架构治理   | **11 层架构(L0 + L1-L10) + 43 crate(28 可达/15 孤岛)依赖铁律 + ADR 实证**                                                           |
| **E02** | 安全架构师  | 零信任/红蓝对抗     | L4 Security(seccore/decay-engine/qeep-protocol)+ OWASP A01-A10                                                         |
| **E03** | 记忆系统专家 | 神经形态 AI/认知架构 | L2 Memory(nmc-encoder/hcw-window/mlc-engine) + L3 Storage(scc-cache/lsct-tiering/cmt-tiering)                          |
| **E04** | 路由算法专家 | 运筹学/优化理论     | L6 Router(osa-coordinator/kvbsr-router/faae-router/sesa-router) + EDSB 概率均衡                                            |
| **E05** | 生产系统专家 | SRE/DevOps   | CI/CD 5 平台 matrix + Docker distroless + release pipeline + 5 platform binary                                           |
| **E06** | 认知科学专家 | 思维模型/任务分解    | L7 Execution(pvl-layer/gqep-executor/mtpe-executor/ssra-fusion) + L8 Parliament(parliament/acb-governor/decb-governor) |
| **E07** | 任务调度专家 | 调度算法/PDCA    | L9 Quest(quest-engine/gea-activator/efficiency-monitor/chimera-mas) + Part II 7 闭环                                     |
| **E08** | 前端交互专家 | TUI/UX 工程    | L10 Interface(chimera-tui/chimera-cli/chtc-bridge/mcp-mesh/csn-substitutor)                                            |

### 13.2 6 个并行子代理深度分析结果

| 子代理                 | 分析 surface                                   | 关键发现                                                                                                                                                                                                    |
| ------------------- | -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **子代理 1** (E01)     | **38 个 Cargo.toml** 穷举审计                     | **8 处不一致**(已 7 处修正):seccore/mcp-mesh 虚标依赖;decay-engine/qeep-protocol/auto-dpo/gsoe-evolution 中度虚标;repo-wiki 虚标 dashmap;v2.4.0-omega 新增 nexus-contracts/omega-learner 已纳入索引                              |
| **子代理 2** (E02)     | L4 Security 全部 3 crate 源码 + OWASP 测试         | 4 层防御模型(SecCore Sandbox + Merkle Audit + ASA + QEEP);OWASP A01-A10 全部覆盖;A02(加密)用 SHA-256 链式哈希;A03(注入)用零信任白名单 + Merkle 拦截                                                                                |
| **子代理 3** (E03)     | L2/L3 全部 6 crate 源码                          | HCW 重要性评分:`score = w1·recency + w2·frequency + w3·relevance`(实现在 `hcw-window/src/selector.rs`);MLC 四级基于访问频率 + 时间衰减;NMC 5 模态加权融合                                                                         |
| **子代理 4** (E04)     | L6 全部 4 crate 源码                             | OSA 5 维稀疏掩码(Routing/Context/Memory/Audit/Budget);EDSB 概率均衡 O(1) 散列;FaaE 工具即专家语义匹配;SESA 三层路由(前置→掩码→稀疏→激活)                                                                                                |
| **子代理 5** (E05)     | .github/workflows + Dockerfile + release.yml | 5 平台 matrix(WoA + Linux x86\_64/aarch64 + macOS x86\_64/aarch64);`fail-fast: false`;Docker distroless/cc-debian12 + nonroot UID 65532;HEALTHCHECK + RUST\_BACKTRACE=1;fuzz 委托 Linux CI(Windows-GNU 不可用) |
| **子代理 6** (E06/E07) | L7/L8/L9 全部 11 crate + chimera-mas 16 子模块    | PVL 5 步反馈闭环(produce→verify→diff→refine→commit);ACB 自适应预算 + DECB 动态紧急预算;TTG 三级思考切换(Quick/Standard/Deep);LHQP 检查点 MessagePack;INV-7 预算触发 LRU 淘汰 Warm/Cold(非 Hot);INV-8 归档单调 Hot→Warm→Cold→Ice             |
| **子代理 7** (E08)     | L10 全部 5 crate 源码                            | TUI 17 面板(v1.8: 14+3 新增);5 IDE 适配器 enum dispatch(VSCode/Vim/Emacs/IntelliJ/Zed);MCP Mesh 进程内消息 + 事件总线(无 HTTP);CSN 降级链 + 相似度匹配;5 平台 binary 3.44MB                                                        |

### 13.3 7 处 Cargo.toml 实证修正清单 (Code Wiki v2.0 核心交付)

| # | crate            | v1 描述                                              | v2.0 实证(基于 Cargo.toml)                                                                      | 性质                          |
| - | ---------------- | -------------------------------------------------- | ------------------------------------------------------------------------------------------- | --------------------------- |
| 1 | `seccore`        | 11 依赖(含 wasmtime/rusqlite/dashmap/uuid/nexus-core) | **8 依赖**:tokio · serde · sha2 · hex · chrono · tracing · thiserror · event-bus              | **严重虚标**(5 项实际无)            |
| 2 | `decay-engine`   | 7 依赖(含 tokio/serde/chrono/nexus-core/event-bus)    | **3 依赖**:thiserror · dashmap · tracing                                                      | **严重虚标**(4 项实际无)            |
| 3 | `qeep-protocol`  | 9 依赖(含 serde/tracing/nexus-core/event-bus)         | **5 依赖**:tokio · uuid · chrono · dashmap · thiserror                                        | **严重虚标**(4 项实际无)            |
| 4 | `auto-dpo`       | 9 依赖(含 dashmap/rand/chrono/uuid/nexus-core)        | **6 依赖**:event-bus · tokio · serde · serde\_json · thiserror · tracing                      | **中度虚标**(5 项实际无)            |
| 5 | `gsoe-evolution` | 10 依赖(含 rand/dashmap/chrono/uuid)                  | **8 依赖**:tokio · serde · anyhow · thiserror · tracing · ndarray · event-bus · nexus-core    | **中度虚标**(4 项实际无)            |
| 6 | `mcp-mesh`       | 14 依赖(含 reqwest/axum/sha2/hex/nexus-core/seccore)  | **9 依赖**:tokio · serde · anyhow · thiserror · tracing · uuid · chrono · dashmap · event-bus | **严重虚标**(6 项实际无,HTTP 实际未启用) |
| 7 | `repo-wiki`      | 12 依赖(缺 prometheus-client)                         | **15 依赖**(新增 prometheus-client,移除虚标 dashmap)                                                | **轻度虚标 + 漏标**               |

### 13.4 关键不变量与约束编码

| 不变量                                    | 编码位置                                                                                  | 触发条件                                                                              | 动作                                          |
| -------------------------------------- | ------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------- | ------------------------------------------- |
| **INV-7 上下文预算界**                       | `chimera-mas/src/invariants.rs` `InvariantChecker::check_inv7_budget()`               | `m_total > MEMORY_BUDGET_MB × MEMORY_BUDGET_UTILIZATION`(130 × 0.9 = 117 MB)      | LRU 淘汰 Warm/Cold(保留 Hot)                    |
| **INV-8 归档单调性**                        | `chimera-mas/src/invariants.rs` `InvariantChecker::check_inv8_archive_monotonicity()` | 检测到 Cold→Warm 升级请求                                                                | 拒绝并走 `archive::upgrade_with_audit()` 留痕     |
| **MAX\_AGENT\_DEPTH = 5**              | `chimera-mas/src/delegation.rs` `MAX_AGENT_DEPTH` 常量                                  | 深度 = 1(根) + 子任务级数;5 级时叶子必须 leaf                                                   | 委托拒绝,返回 `MasError::DepthExceeded`           |
| **`#![forbid(unsafe_code)]`**          | **43 个 crate** `lib.rs` 第 1 行                                                         | 任何 unsafe 块                                                                       | `rustc` 编译失败                                |
| **Critical 事件 mpsc**                   | `event-bus/src/bus.rs` `publish_critical()`                                           | `SkepticVeto`/`RedTeamAudit`/`AsaIntervention`/`BudgetExceeded`/`AgentTaskFailed` | mpsc fan-out,保证送达                           |
| **BudgetExceeded severity = Critical** | `event-bus/src/classification.rs:46`(`NexusEvent::severity()` 综合 match)               | 任何 BudgetExceeded 事件                                                              | `severity()` 必须返回 `EventSeverity::Critical` |

### 13.5 与三方权威源的一致性声明

| 权威源                                    | 字段           | 值                                                                                                                                                                                                                 | 一致性 |
| -------------------------------------- | ------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --- |
| `Cargo.toml` workspace.package.version | version      | `2.19.0-omega`                                                                                                                                                                                                    | ✅   |
| `Cargo.toml` `[workspace.members]`     | crate 数      | **38**(含 chimera-mas + nexus-contracts + omega-learner + mca-gateway)                                                                                                                                             | ✅   |
| `CHANGELOG.md` v2.19.0-omega           | 测试规模         | 8455(含 MCA 亲和体系 + P3 Sprint:ASA PPO/贝叶斯平均/DelegationExecutor 超时映射/CoordinationMetricsCollector 上限锁/ConsensusQualityMetrics 熵基归一化/gVisor benchmark)                                                                | ✅   |
| `event-bus/src/types.rs`               | event 变体     | 129(74 v2.3.1 → +7 Agent → +28 v2.x → +3 观测事件 → +6 MCA Affinity → +11 至 v2.20.0 快照,2026-08-05;与 §11 表头一致)                                                                                                         | ✅   |
| `crates/chimera-mas/Cargo.toml`        | 内部依赖         | 24 个 workspace 成员                                                                                                                                                                                                 | ✅   |
| `nuxus规则.md` §3.4                      | 当前阶段         | 第三阶段(模块级系统性优化)                                                                                                                                                                                                    | ✅   |
| `.claude/CLAUDE.md` §1                 | 工具链          | `D:\Chimera CLI\.toolchain\`,GNU stable                                                                                                                                                                           | ✅   |
| `project_memory.md` Hard Constraints   | Critical 事件数 | 14(含 R2Freeze/R2Rollback/AffinityQuotaExhausted/CheckpointSaved/ConsensusReached/SlowConsumer/OrphanCall/SkepticVeto/VetoOverridden/RedTeamAudit/BudgetExceeded/AgentTaskFailed/AsaIntervention/R1ShadowRollback) | ✅   |

### 13.6 关键架构洞察(分布式分析沉淀)

★ **Insight 1 — L1 上帝 crate 现象**:`nexus-core` 被 **38 crate** 中 33 个依赖;`event-bus` 被 **38 crate** 中 30+ 个依赖。两者构成 L1 上帝节点,任何核心 API 变更需 major 版本升级。改进方向:内环/外环重组(参考 `三环循环_十层接口_元架构重组深度分析.md` Phase 0;**Phase 0 评估已于 2026-08-04 完成,P9-T1**);v5.0 P2 引入 `nexus-contracts` 零依赖契约层作为 L0 解耦基底(ADR-033)。

★ **Insight 2 — L6 Router 星型耦合**:`osa-coordinator` 是 L6 5 个 router 的中心节点(kvbsr-router/faae-router/sesa-router/omega-learner 均依赖),形成 L6 内部星型。改进方向:将 `OmniSparseMasks` 提取到 `nexus-contracts` L0 作为零依赖共享类型(ADR-033 已落地)。

★ **Insight 3 — chimera-mas 协同广度**:`chimera-mas` 作为 L9 hub 协同 16 个跨层 crate(L1/L2/L3/L4/L6/L7/L8/L9 全部),是迄今依赖最广的单一 crate(24 个内部依赖 + 3 个外部),但严格遵守依赖铁律(只依赖 L(N-1) 及以下)。

★ **Insight 4 — 双通道事件总线的工程价值**:`event-bus` 的 Normal 通道(broadcast)+ Critical 通道(mpsc fan-out)设计,有效解决"事件静默丢失"问题(Week 6 SSRA 教训 + Week 7 4 crate 遵循)。

★ **Insight 5 — 三重悖论在 Chimera 的具体映射**:① 记忆悖论:MLC 基于访问频率的冷热迁移无法区分任务相关性,OSA 静态稀疏掩码无法替代 MemCon 式自适应;② 推理悖论:**43 crate** 跨层协调成本存在阈值,SkepticVeto 可被策略性利用;③ 进化悖论:GSOE/AutoDPO 验证器层级为 L3(执行反馈),存在"奖励黑客"游戏化风险。改进方向:R2 形式化验证器(L4/L5)跃迁,落地前无条件冻结(ADR-042 已落地);P5.3 ImmuneSystem facade 三探针(ADR-046)实时监控悖论状态。

### 13.7 后续工作建议

1. **Code Wiki v3.0 路线图**:接入 LLM 自动审计脚本,每周自动对比 Cargo.toml 与文档,检测虚标/漏标(本版本已完成 7 处修正)
2. **专家评审固化**:8 位虚拟专家角色模板化,纳入 PR review 流程
3. **ADR 自动降级追溯**:ADR-001/005 降级路径用 mermaid graph 渲染,纳入 CI 文档构建
4. **INV-7/INV-8 不变量 proptest 强化**:chimera-mas 1000 次 proptest 已落地,可推广到其他关键不变量

***

**NEXUS-OMEGA** — Ω-Sparse · Ω-Compress · Ω-Evolve · Ω-Event

> Code Wiki v2.0 · 2026-07-23 · 8 位专家分布式深度分析交付物

### 🔖 v2.26.0-omega 同步批注（2026-08-15 追加）

> **基线状态**：**v2.26.0-omega**（2026-08-11）· **38 crates** · **136 NexusEvent 变体**（types.rs 枚举 + metadata() 双重验证）· **9,954 tests passed / 0 failed**
>
> **★ R2 解冻影子期标注**：R2（GSOE×AutoDPO 约束 RL）仍处解冻影子期（≥14 天，2026-07-29 rev4 治理签署），五要素 fail-closed 门禁持续生效，五关键词（constrained\_rl / r2\_policy / train\_r2 / GsoeAutoDpoRL / evolve\_with\_constrained\_rl）CI 扫描继续生效。ADR-042 无条件冻结 + ADR-043 影子模式 + ADR-053 rev4 治理签署全部有效。
>
> **★ RL 开发闸门（Rust-First,2026-08-15 治理决策）**：现阶段只做 Rust 侧；Python 侧（RL 版）训练服务仅保留规划（`docs/architecture/rl-client-protocol.md` 协议契约保留为设计资产，Python 服务实体禁止实施）；待整个 Rust 系统彻底成熟并稳定运行后（R2 解冻 + 稳定性观察期通过）再开启 RL。权威源：`.trae/rules/nuxus规则.md` §1.4 / `AGENTS.md` §1.2。

### 🔖 v2.27.1-omega 同步批注（2026-08-20 追加）

> **基线状态**：**v2.27.1-omega**（2026-08-20）· **38 crates** · **144 NexusEvent 变体**（types.rs 枚举 + metadata() 双重验证，2026-08-19 实测）· **10,836 tests passed / 0 failed**
>
> **★ Phase 10 §16 跨层协同闭环已发布收编**：v2.27.0-omega（2026-08-19）正式发布（W1-W7 全波次闭环），v2.27.1-omega（2026-08-20）为 GPG 签名补发 + MCA E2E 超时加固。ADR-085 双态治理已收编：权威口径从 136（v2.26.0-omega）升级为 144（v2.27.0-omega），`check_doc_consistency.ps1` GAP-F2 自然消除。在途 8 事件见 §5.6 已转为发布记录。

