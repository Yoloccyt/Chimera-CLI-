# Chimera CLI v3.4.0-omega 统一架构设计与Rust侧实现规范 —— 二十三篇前沿论文融合权威版

> # 🔖 档案化权威基线核准横幅（2026-09-02 追加，历史文档只加注不改写）
>
> **档案化时点**：2026-09-02
> **权威基线**：v2.28.0-omega（在途未打 tag）· 43 crates（28 生产可达 / 15 ADR-160 冻结孤岛）· 144 NexusEvent（types.rs 单表，event_types.rs 镜像已退役）· 11,564 tests / 485 target（2026-08-31 实测，以实测为准）· ADR 主编号至 160（新编号段自 ADR-161 起）
> **tag 事实订正**：v2.27.1-omega 本地与 origin 均无 tag（CHANGELOG-only 补丁），实际最新已发 tag = v2.27.0-omega
>
> **与现行代码已知偏差**：
> - 基线锚 v2.27.1 已陈旧；
> - 约 53 crate 上限（含弹性子模块化条款）vs 现行 43；
> - Ω₁₀-Card / Ω₁₁-Synthesize 十/十一符号尚未并入现行权威九定律集——已由新 ADR（自 ADR-161 起）正式收录为 "OMEGA 十一定律（九基座 + 两扩展）"；
> - 文档内部 136/144 事件口径不一致，现行真值 = 144（types.rs 单表）。
>
> 本文档已档案化：历史溯源 + 愿景参考，权威基线以代码 + CODE_WIKI.md + CHANGELOG.md + ADR 为准

> **⚠️ RL 开发闸门 (2026-08-16 治理决策)**: 现阶段**只做 Rust 侧**；Python 侧(RL 版)训练服务**仅保留规划**（本文档所述协议/方案/规划内容保留为设计资产），Python 服务实体**禁止实施**；待整个 Rust 系统彻底成熟并稳定运行后（R2 解冻 + 稳定性观察期通过）再开启 RL。权威源：`.trae/rules/nuxus规则.md` §1.4 / `AGENTS.md` §1.2 当前焦点。

> **版本**: v3.4.0-omega (融合终版)  
> **状态**: Rust 侧完整架构规范 + 十四/六/十三论文全栈融合  
> **基线实证**: Chimera CLI v2.27.1-omega (38 crates · 144 NexusEvent · 10836 passed/0 failed · 86 ADR · `#![forbid(unsafe_code)]` 38/38)  
> **论文来源**: 
> - **十四篇**（天津大学郝建业 / CMU / 人大RUC-NLPIR / 北大DCAI / jcode / 腾讯混元 / 快手KwaiKAT / QoderAI / 小米Darwin / 微软OpenForge / Evolvent RSIBench / PenguinHarness / MemoHarness / 清华Frontis-MA1 OpenMLE）
> - **六篇**（OPENFORGE RL×Dressage / RSIBench-Data / Qoder四层次 / MSCE Memory-Skill Co-Evolution / HiLS-Attention / TencentDB Agent Memory）
> - **十三篇**（Harness Engineering 完整技术栈：理论基础层 / 工程参考层 / 评估与进化层 / 训练基建与系统层）
> **核心范式**: Persistent Agent = πθ ⊕ H ⊕ E ⊕ C ⊕ M ⊕ Ω-Card ⊕ Ω-Synthesize (模型 + Harness + 进化 + 协同 + 记忆-技能融合 + 经验卡片 + 按需合成)  
> **最后更新**: 2026-08-16  
> **权威声明**: 本文档是 Chimera CLI v2.26.0 实证基线与二十三篇前沿论文洞察的唯一融合权威参考，取代此前所有分散白皮书与优化方案

> # 🔖 落地进度对照（2026-08-30 追加）
>
> 本文档是 **Phase 1-5 Ch12（W1-W26）的 Rust 侧实现规范依据之一**。截至 2026-08-30，其规划的工程项已随 26 周路线图全部波次收尾：
> - **基线已演进**:本文档 §头部"基线实证 v2.27.1 / 38 crates / 10836 / 86 ADR"为撰写时点值;**当前 v2.28.0-omega 在途 = 43 crates(28 生产可达/15 冻结孤岛,ADR-160)· 144 NexusEvent(types.rs 单表)· 11522 tests · ADR 主编号至 160**。
> - **5 个新 crate 已建**:`nexus-app-server`(WI-01 宿主门面)/ `session-store`(ADR-141)/ `mas-sched`(ADR-145)/ `nexus-hook`(ADR-146)/ `nexus-subagent`(ADR-148),接口面与本文档规范一致;另有 3 个规划新建被 ADR-137/151 否决,能力落入既有 crate。
> - **并行化/通信规范已落地**:ComputeBridge 双运行时、ShardedBus 分片总线、CBMR 微批写、CausalGraph 因果归因见 ADR-095~156 合并档;双跑零 diff 后 ADR-153 Go 全量 B 级。
> - **状态定性**:本文档仍为 Rust 侧**架构规范参考**(长期有效),但其中"待实施/路线图/W1-W26 排期"语义已全部完成;后续新增工作以 CODE_WIKI §3 + 新 ADR 为准。RL 闸门(第 3 行)持续有效。

---

## 目录

1. [执行摘要与TL;DR](#1-执行摘要与tldr)
2. [二十三篇论文融合总矩阵与交叉验证](#2-二十三篇论文融合总矩阵与交叉验证)
3. [设计哲学：OMEGA十定律与Rust侧铁律](#3-设计哲学omega十定律与rust侧铁律)
4. [架构总览：十层认知系统 × 二十三论文 × 53 Crate](#4-架构总览十层认知系统--二十三论文--53-crate)
5. [L0 Contracts：契约层——六维控制面 + 经验卡片 + 平台接地 + Token证据](#5-l0-contracts契约层)
6. [L1 Core：核心层——Event Bus经验卡片化 + Segment-aware PER + 统计学习接口 + RL预留](#6-l1-core核心层)
7. [L2 Memory：记忆层——经验卡片系统 + 按需记忆合成 + 双层经验库 + 记忆图谱 + HiLS注意力](#7-l2-memory记忆层)
8. [L3 Storage：存储层——金字塔存储 + 经验卡片持久化 + 三因子索引 + 分层采样](#8-l3-storage存储层)
9. [L4 Security：安全层——Paddock-Sandbox解耦 + AutoBuilder + 错误签名 + 六类状态反馈](#9-l4-security安全层)
10. [L5 Knowledge：知识层——四套原子算子 + 三因子父本选择 + AEGIS-GSOE + 变体隔离 + 双层经验库 + Skill生命周期](#10-l5-knowledge知识层)
11. [L6 Router：路由层——Skills渐进加载 + 算子路由 + 六维动态调整 + 三因子选择 + 工具裁剪](#11-l6-router路由层)
12. [L7 Execution：执行层——PVL算子化 + 经验卡片生成 + Process-Score + Segment-aware验证 + 熵加权](#12-l7-execution执行层)
13. [L8 Parliament：议会层——变体审议 + 三因子裁决 + 停止策略 + 行为定位 + 冲突仲裁](#13-l8-parliament议会层)
14. [L9 Quest：任务层——Ambient Mode + 搜索树管理 + 长任务地图 + 长时程信用分配](#14-l9-quest任务层)
15. [L10 Interface：接口层——Runtime Auditor + 自我评估面板 + 经验卡片可视化 + OmniMessage预留 + Concord TUI](#15-l10-interface接口层)
16. [跨层协同：评估-进化-记忆-技能四位一体闭环](#16-跨层协同评估-进化-记忆-技能四位一体闭环)
17. [RL架构预留：Rust侧接口设计 · Python侧v4.0计划](#17-rl架构预留rust侧接口设计--python侧v40计划)
18. [安全与熔断：十层防御体系 + 降级路线](#18-安全与熔断十层防御体系--降级路线)
19. [实施路线图：v2.26.0 → v3.3.0 → v3.4.0](#19-实施路线图v2260--v330--v340)
20. [附录](#20-附录)

---


## 1. 执行摘要与TL;DR

### 1.1 决策摘要

| 决策点 | 结论 | 依据 |
|--------|------|------|
| **基线锚定** | v2.27.1-omega 38 crate 实证架构不可动摇 | 10836 tests 全绿、86 ADR 落档、144 NexusEvent 变体、Cargo.toml 三方一致性验证 |
| **Rust侧先行** | v3.3.0-v3.4.0 全部新增能力先用 Rust 统计/规则/启发式实现，预留 RL 接口 | 清华 OpenMLE 经验卡片 + 三因子选择 + 按需记忆合成可立即产生价值；PyTorch 依赖重、调试复杂 |
| **Dressage验证ADR-065** | Paddock/Sandbox解耦 + Proxy token-level evidence + Segment-aware training 与 `rl-client` + `event-bus` PER + `seccore` sandbox **完全同构** | Dressage 论文详细实现 |
| **MSCE验证L2-L5** | 三层记忆(L1 Trace/L2 Policy/L3 Env) + Skill生命周期(probationary→active→archived) + 双信号价值回填 与 Chimera MLC四级 + SkillGraph + AEGIS **完全可映射** | MSCE 论文实验数据 |
| **HiLS增强L2** | 分层稀疏注意力(块间+块内softmax) + landmark token端到端训练 可作为 `hcw-window` 的注意力机制升级，支持64K→512K上下文 | HiLS 论文性能数据 |
| **TencentDB验证L2-L3-L5** | 四层金字塔(L0 Raw→L1 Atomic→L2 Scene→L3 Persona) + 检索三方式(字面+语义+混合) + 注入策略(用户消息前/系统提示末尾) + 冲突仲裁 与 MLC + CMT + repo-wiki **完全可映射** | TencentDB 实测数据 |
| **32K失败/64K成功警示** | Dressage OpenClaw实验：32K上下文训练reward上升但评测下降(segment过多)，64K上下文成功。→ **HCW窗口必须≥64K** | Dressage 实验数据 |
| **工具schema裁剪** | Dressage：33个工具→4个，13.5K→1.7K tokens。→ **OSA五维稀疏的实证验证** | Dressage 实验数据 |
| **OpenMLE经验卡片** | 清华 Frontis-MA1：每个执行节点生成结构化经验卡片，记录分数/改进幅度/方法家族/错误签名；Token消耗降低41.7%，Prompt降低50.3% | OpenMLE 实测数据 |
| **三因子父本选择** | Quality + Progress + Novelty，UCB + Softmax + 冷却系数，避免只按分数采样丢失潜力分支 | OpenMLE 核心算法 |
| **按需记忆合成** | 仅在调用算子时检索相关祖先与兄弟节点，Prompt长度降低60%-86% | OpenMLE 核心机制 |
| **云端编排层** | 暂缓但纳入路线图(Phase 8)，基于Qoder Forward Mode + TencentDB Memory Hub | 用户约束 |

### 1.2 轨道视图

```
v2.26.0-omega (现在) ──────→ v3.3.0-omega (8周) ──────→ v3.4.0-omega (目标)
     │                              │                              │
  38 crates                      +15 crates                     +0 crate
  71 ADR                         +12 ADR                        +4 ADR
  9954 tests                     +~2000 tests                   +~1000 tests
  136 NexusEvent                 +14 事件(经验卡片相关)         +6 事件
  HCW 256K/1M                    → HCW+HiLS 64K/256K/512K/2M   → 稳定
  MLC 四级                       → MLC-MSCE-TencentDB 融合四级  → 稳定
  Concord TUI v3.1               → +经验卡片可视化面板          → 稳定
  无经验卡片                     → 经验卡片系统全量落地         → 稳定
  无三因子选择                   → 三因子父本选择生产级         → 稳定
  无按需记忆合成                 → 按需记忆合成懒加载           → 稳定
  无Segment-aware                → Segment-aware PER/Validation  → 稳定
  无Token-level evidence         → Proxy token ledger           → 稳定
  无Skill生命周期                → Skill probationary→active    → 稳定
  无云端编排                     → 路线图Phase 8                → Forward Mode v0.1
```

---

## 2. 二十三篇论文融合总矩阵与交叉验证

### 2.1 三层论文结构总览

二十三篇论文共同构成了 Harness Engineering 的**完整技术栈**，可分为四层：

```
┌─────────────────────────────────────────────────────────────┐
│                    评估层 (Evaluation)                       │
│  Qoder Better Harness — 判断 Harness 好坏的标准              │
│  腾讯 Harness Handbook — 行为定位与代码修改导航                │
│  RSIBench-Data — RSI 递归自我改进的评测框架                  │
│  清华 OpenMLE — 经验卡片与三因子评估体系                      │
├─────────────────────────────────────────────────────────────┤
│                    进化层 (Evolution)                          │
│  小米 AEGIS — 四阶段进化引擎 + 变体隔离                      │
│  CMU Meta-agent — 自动适配 Harness                         │
│  MemoHarness — 六维控制面搜索 + 双层经验库                   │
│  PenguinHarness — Agent 构建 Agent 自我改进流水线            │
│  MSCE — 记忆-技能协同进化 + 双信号价值回填                   │
├─────────────────────────────────────────────────────────────┤
│                    基建层 (Infrastructure)                   │
│  微软 OpenForge/Dressage — 真实 Harness 训练框架           │
│  快手 KAT-Coder — AutoBuilder + Process-Score              │
│  jcode — 多会话低内存 Harness                                │
│  HiLS-Attention — 分层稀疏注意力长上下文机制                 │
│  TencentDB Agent Memory — 四层金字塔 + 检索三方式            │
├─────────────────────────────────────────────────────────────┤
│                    理论基础 (Foundation)                     │
│  郝建业 — Harness Engineering 范式定义                       │
│  RUC 149页 — Agent = πθ ⊕ H 理论框架                         │
│  北大 DataFlow — NL2Pipeline gap 与平台接地                  │
│  Qoder四层次 — Context→Harness→Loop→Graph                  │
│  Dressage — Token级证据 + Segment-aware + Paddock-Sandbox   │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 核心洞察交叉验证总表

| 洞察 | 来源论文 | 交叉结论 | Chimera落点 | 状态 |
|------|---------|----------|-------------|------|
| **Context/Harness边界** | Dressage/PenguinHarness/Qoder/RUC | **Agent逻辑与环境解耦是共识** | L4 `seccore` + L1 `rl-client` + L0 `omni-message` | 🟢 Rust实现 |
| **Token级证据** | Dressage | **训练必须保留token-level evidence** | L1 `event-bus` PER扩展 + `token-ledger` | 🟢 Rust实现 |
| **Segment-aware训练** | Dressage | **长轨迹必须分段训练** | L1 `event-bus` + L7 `pvl-layer` | 🟢 Rust实现 |
| **32K失败/64K成功** | Dressage/HiLS | **上下文窗口必须≥64K** | L2 `hcw-window` 升级64K基线 | 🟢 Rust实现 |
| **工具schema裁剪** | Dressage | **OSA稀疏掩码实证验证** | L6 `osa-coordinator` | 🟢 Rust实现 |
| **经验卡片** | OpenMLE | **每个执行节点必须生成结构化卡片** | L1-L7 全链路经验卡片流 | 🟢 Rust实现 |
| **三因子父本选择** | OpenMLE | **Q+P+N 优于单一分数采样** | L5 `three-factor-selector` + L6 `parent-selector` | 🟢 Rust实现 |
| **按需记忆合成** | OpenMLE | **懒加载祖先+兄弟节点，Prompt降60%-86%** | L2 `on-demand-synthesizer` | 🟢 Rust实现 |
| **四套原子算子** | OpenMLE | **Draft/Improve/Debug/Crossover贯穿始终** | L7 `atomic-operators` | 🟢 Rust实现 |
| **记忆三层** | MSCE/TencentDB/RUC | **记忆分层是共识，MSCE和TencentDB可映射** | L2 `mlc-engine` 融合四级 | 🟢 Rust实现 |
| **Skill生命周期** | MSCE | **技能必须有状态机** | L5 `skill-graph` 扩展 | 🟢 Rust实现 |
| **双信号价值回填** | MSCE | **PER优先级需要融合反思信号** | L1 `event-bus` PER扩展 | 🟢 Rust实现 |
| **分层稀疏注意力** | HiLS | **HCW窗口选择器可升级** | L2 `hils-attention` | 🟡 新增crate |
| **检索三方式** | TencentDB | **记忆召回需要多路融合** | L2 `mlc-engine` + L3 `cmt-tiering` | 🟢 Rust实现 |
| **注入策略** | TencentDB | **上下文注入位置影响缓存和成本** | L2 `hcw-window` 注入策略 | 🟢 Rust实现 |
| **冲突仲裁** | TencentDB | **议会审议可借鉴两阶段仲裁** | L8 `parliament` 扩展 | 🟢 Rust实现 |
| **长任务地图** | TencentDB | **Quest checkpoint可借鉴** | L9 `quest-engine` LHQP扩展 | 🟢 Rust实现 |
| **Paddock-Sandbox解耦** | Dressage | **what-to-do vs where-it-runs** | L4 `seccore` 扩展 | 🟢 Rust实现 |
| **Process-Score九维度** | 快手KAT | **探索/定位/忠实/最小/验证/诚实/效率/鲁棒/可读** | L7 `process-score-calculator` | 🟢 Rust实现 |
| **AEGIS四阶段引擎** | 小米 | **Digester→Planner→Evolver→Critic** | L5 `aegis-gsoe` | 🟢 Rust实现 |
| **变体隔离** | 小米/RSIBench | **保留历史最佳，避免灾难性遗忘** | L5 `variant-pool` + `checkpoint-preserver` | 🟢 Rust实现 |
| **六维控制面** | MemoHarness | **D1-D6可编辑控制面** | L0 `six-dimension-contracts` | 🟢 Rust实现 |
| **OmniMessage解耦** | PenguinHarness | **模型-环境统一消息协议** | L0 `omni-message` 预留 | 🔵 预留接口 |

### 2.3 关键数据对比（二十三篇论文实证）

| 来源 | 关键数据 | 对Chimera的启示 |
|------|---------|----------------|
| Dressage | SWE-bench Verified: 32.6%→37.8% (+5.2pp) | 真实Harness训练可迁移 |
| Dressage | OpenClaw 32K: 训练reward↑评测↓(0.583→0.441) | **窗口不足导致segment过多，信用分配噪声** |
| Dressage | OpenClaw 64K: Pass^3 +14.0pp, segment 2.1→1.2 | **64K是Agent RL的最低上下文门槛** |
| Dressage | 工具裁剪: 33→4, 13.5K→1.7K tokens | **OSA稀疏掩码的极端验证** |
| Dressage | 成本: $4.36-$6.12/RL任务 | L4 AutoBuilder预算熔断 |
| MSCE | EvoAgentBench Pass@1: 信息检索+4.61, 数学+4.00, 软件工程+15.39 | 记忆-技能协同进化有效 |
| MSCE | 跨域迁移: 平均+3.93pp | 技能可跨域复用 |
| MSCE | 长期进化: p0→p100 Pass@1持续上升，成本先升后降 | 进化需要长期视角 |
| HiLS | 8K训练→512K PPL 4.94→5.95 (Full-Attn 32K崩溃) | **分层稀疏注意力是长上下文唯一出路** |
| HiLS | RULER 8K: 72分 vs Full-Attn 34分 | 压缩有去噪效果 |
| HiLS | 7B续训50B: RULER 97.42, 128K/256K PPL 2.55/3.10 | 可轻量迁移 |
| TencentDB | 写入→提炼: 10秒内可检索 | 记忆时效性基准 |
| TencentDB | 长任务地图: Token 2.21亿→8500万, 通过率33%→50% | 任务地图机制有效 |
| TencentDB | 团队记忆: Skill+Wiki+CodeGraph+Chat → Memory Hub | 云端编排需要团队记忆 |
| OpenMLE | 总Token消耗降低41.7%，Prompt Token降低50.3% | 经验卡片+按需合成效率 |
| OpenMLE | 搜索历史动态缩放（动态奖励归一化） | L7 PVL滑动窗口归一化 |
| OpenMLE | 放大最优样本梯度权重(~4倍，熵加权) | L7 PVL统计加权 |
| 小米AEGIS | Qwen 53%→97%(+44%)，弱模型+强Harness | Harness决定上限 |
| 快手KAT | 83.5%仓库无法运行，AutoBuilder解决 | L4环境构建验证 |
| RSIBench | 58.33%超过首次，78.26%低于峰值 | **必须保留历史最佳checkpoint** |
| jcode | 10会话仅117MB（Claude Code的1/19.7） | 内存优化至<50MB |
| 腾讯Handbook | 行为定位L1→L2→L3，BGPD算法 | L5 repo-wiki + L7 PVL |

---

## 3. 设计哲学：OMEGA十定律与Rust侧铁律

### 3.1 OMEGA十定律（二十三论文融合权威版）

| 定律 | 符号 | 含义 | 来源 | Rust侧实现状态 | 未来RL升级 |
|------|------|------|------|---------------|-----------|
| **Ω-Sparse** | Ω₁ | 全维稀疏掩码 | Chimera原生 | ✅ OSA静态+动态稀疏掩码 | PPO Actor网络 |
| **Ω-Compress** | Ω₂ | 四级窗口+神经形态记忆 | Chimera原生 | ✅ HCW+MLC四级记忆 | Mem-π生成式记忆 |
| **Ω-Evolve** | Ω₃ | GRPO风格进化 | DeepSeek-R1+小米AEGIS | 🚧 AEGIS四阶段规则引擎 | 在线GRPO策略网络 |
| **Ω-Event** | Ω₄ | 事件驱动架构 | Chimera原生 | ✅ Event Bus 136变体+经验卡片 | 异步PER |
| **Ω-Credit** | Ω₅ | 信用分配 | SHARP | 🚧 统计加权信用分配 | SHARP Shapley值 |
| **Ω-Reuse** | Ω₆ | 复用率优先 | Altman+MemoHarness | ✅ SkillGraph复用率统计 | 复用率奖励函数 |
| **Ω-Locate** | Ω₇ | 行为定位 | 腾讯Handbook | ✅ L1→L2→L3 Handbook导航 | RL学习导航策略 |
| **Ω-Assess** | Ω₈ | 自我评估 | Qoder | ✅ Runtime Auditor五维度 | 评估策略网络 |
| **Ω-Preserve** | Ω₉ | 保留历史最佳 | RSIBench+小米 | ✅ Checkpoint保留机制 | 停止策略网络 |
| **Ω-Card** | Ω₁₀ | 经验卡片 | **清华OpenMLE** | **✅ Event Bus→结构化卡片** | 卡片嵌入网络 |
| **Ω-Synthesize** | Ω₁₁ | 按需记忆合成 | **清华OpenMLE** | **✅ 懒加载祖先+兄弟检索** | 记忆生成网络 |

> 注：Ω-Card与Ω-Synthesize是一枚硬币的两面——Ω-Card是数据结构定律，Ω-Synthesize是算法定律。

### 3.2 Rust侧铁律（所有新增crate必须遵循）

```rust
// 每个新增crate的lib.rs顶部必须包含：
#![forbid(unsafe_code)]

// 铁律1：零运行时Python依赖
// 铁律2：所有RL接口为async trait，实现方可替换（RulePolicyFallback为默认）
// 铁律3：经验卡片为不可变数据结构，版本化存储（写入后不可变，更新即新建版本）
// 铁律4：三因子评分为纯函数，无副作用（输入确定则输出确定）
// 铁律5：按需记忆合成为懒加载，不阻塞主流程（异步合成，超时回退到基础上下文）
// 铁律6：所有统计学习机制必须可导出为RLTrajectory（为v4.0升级预留数据流）
// 铁律7：错误签名必须结构化收集，支持哈希去重和聚类
// 铁律8：六类状态反馈（Success/Error/MissingCode/NoSubmit/ScoreFailed/Timeout）必须全链路追踪
// 铁律9：Segment-aware PER必须共享parent_traj_id，anchor segment承载终局reward
// 铁律10：Paddock不可依赖Sandbox内部实现（解耦红线）
```

### 3.3 架构设计第一性原理

1. **经验优先**：没有经验卡片的执行是无效的，没有三因子评分的搜索是盲目的
2. **算子化抽象**：所有代码生成/修改/修复/融合操作必须映射到Draft/Improve/Debug/Crossover四套原子算子
3. **分层解耦**：L0契约不可变，L4安全硬约束不可覆盖，其余层通过Event Bus松耦合
4. **统计先行**：v3.3.0-v3.4.0所有"智能"选择先用统计方法（EMA/UCB/Softmax）实现，v4.0再替换为神经网络
5. **防御纵深**：十层每层都有独立防御机制，L4 SecCore为绝对红线
6. **基线不可动摇**：v2.27.1-omega的38 crate/144 NexusEvent/10836 tests是任何新增组件的不可破坏基线

---

## 4. 架构总览：十层认知系统 × 二十三论文 × 53 Crate

### 4.1 完整架构图（v3.4.0融合版）

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ L10 Interface —— Rust侧完整实现 (6 crates + 7 新增 = 13 crates)              │
│  chimera-cli · chimera-tui(22面板+v3.1引擎+经验卡片可视化) · chtc-bridge ·   │
│  mcp-mesh · csn-substitutor · mca-gateway · [self-assessment-panel ·         │
│  experience-card-viz · dag-viz-panel · search-tree-viz · runtime-auditor]  │
│  [OmniMessage预留接口 · RL状态面板预留]                                        │
├─────────────────────────────────────────────────────────────────────────────┤
│ L9 Quest —— Rust侧完整实现 (4 crates + 5 新增 = 9 crates)                    │
│  quest-engine(DAG+LHQP+TTG) · gea-activator · efficiency-monitor ·          │
│  chimera-mas(四象限+WSJF) · [ambient-mode · search-tree-manager ·            │
│  experience-card-collector · stop-strategy · long-term-credit-assigner]     │
│  [Hierarchical RL预留 · 长程信用分配预留]                                      │
├─────────────────────────────────────────────────────────────────────────────┤
│ L8 Parliament —— Rust侧完整实现 (3 crates + 6 新增 = 9 crates)               │
│  parliament(Skeptic+Security+Execution) · acb-governor · decb-governor ·    │
│  [variant-parliament · three-factor-adjudicator · stop-ruling ·              │
│  behavior-localization · critical-path-identifier · conflict-arbitration]    │
│  [MAPPO+SHARP预留 · Self-Play预留]                                           │
├─────────────────────────────────────────────────────────────────────────────┤
│ L7 Execution —— Rust侧完整实现（算子化抽象）(4 crates + 6 新增 = 10 crates)    │
│  pvl-layer → [atomic-operators(Draft/Improve/Debug/Crossover)] ·            │
│  gqep-executor · mtpe-executor · ssra-fusion ·                               │
│  [experience-card-generator · process-score-calculator(规则版) ·            │
│  dynamic-verification-depth · entropy-weighted-scoring ·                    │
│  segment-aware-validation · hint-boosted-recovery(预留)]                    │
│  [GTPO Turn-Level预留 · RLVR预留]                                            │
├─────────────────────────────────────────────────────────────────────────────┤
│ L6 Router —— Rust侧完整实现 (5 crates + 5 新增 = 10 crates)                  │
│  osa-coordinator · kvbsr-router · faae-router · sesa-router ·                 │
│  omega-learner(LinUCB) · [skills-progressive-loader · operator-router ·     │
│  six-dimension-adjuster · harness-config-manager · parent-selector ·         │
│  tool-schema-pruner]                                                        │
│  [PPO+ActFocus+DVAO预留 · 注意力策略网络预留]                                 │
├─────────────────────────────────────────────────────────────────────────────┤
│ L5 Knowledge —— Rust侧完整实现（进化引擎骨架）(3 crates + 7 新增 = 10 crates)  │
│  repo-wiki · gsoe-evolution → [aegis-gsoe(规则版) · four-operators ·         │
│  three-factor-selector · variant-pool · checkpoint-preserver ·               │
│  dual-experience-bank · msce-integration · skill-lifecycle ·                 │
│  procedural-blueprint · behavior-localization · meta-agent-adapter(预留)]  │
│  [在线GRPO预留 · SkillGraph联合进化预留 · Cross-Harness GRPO预留]              │
├─────────────────────────────────────────────────────────────────────────────┤
│ L4 Security —— Rust侧完整实现（硬约束不可覆盖）(3 crates + 4 新增 = 7 crates) │
│  seccore(沙箱+Merkle+ASA) · decay-engine · qeep-protocol ·                   │
│  [auto-builder · error-signature-collector · output-validator ·              │
│  six-class-feedback-integrator · paddock-sandbox]                            │
│  [🔒 硬约束不可覆盖]                                                          │
├─────────────────────────────────────────────────────────────────────────────┤
│ L3 Storage —— Rust侧完整实现 (3 crates + 4 新增 = 7 crates)                  │
│  scc-cache · lsct-tiering · cmt-tiering ·                                    │
│  [experience-card-storage · three-factor-index · dual-experience-pool ·      │
│  error-signature-index · pyramid-storage · per-priority-sampler(预留)]       │
├─────────────────────────────────────────────────────────────────────────────┤
│ L2 Memory —— Rust侧完整实现（按需记忆合成）(3 crates + 6 新增 = 9 crates)    │
│  nmc-encoder · hcw-window · mlc-engine ·                                     │
│  [experience-card-system · on-demand-synthesizer · memory-graph ·            │
│  memory-sideagent · ancestor-sibling-index · experience-card-cache ·          │
│  global-experience-board · hils-attention]                                     │
│  [DQN+Mem-π预留]                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ L1 Core —— Rust侧完整实现（经验卡片基础设施）(3 crates + 5 新增 = 8 crates)   │
│  nexus-core · event-bus → [experience-card-bus · token-ledger ·              │
│  segment-aware-per · model-router · [rl-types · rl-client(预留接口) ·        │
│  stat-learning-policy · state-encoder(预留) · action-decoder(预留) ·         │
│  trajectory-reconstructor(预留)]                                           │
│  [OpenForge-Proxy预留 · 多Harness训练预留]                                    │
├─────────────────────────────────────────────────────────────────────────────┤
│ L0 Contracts —— Rust侧完整实现（零依赖契约层）(1 crate + 6 新增 = 7 crates)  │
│  nexus-contracts · [experience-card-contracts · six-dimension-contracts ·   │
│  operator-contracts · platform-grounding-specs · execution-status-contracts ·│
│  error-signature-contracts · token-evidence-contracts · memory-pyramid-contracts ·│
│  skill-lifecycle-contracts · omni-message(预留) · behavior-contracts ·        │
│  rl-hook-contracts]                                                          │
└─────────────────────────────────────────────────────────────────────────────┘
```

> ** Crate 统计**: 基线 38 + 新增 ~15 = **~53 crates**（部分新增模块以内嵌子模块形式落地，不独立crate）

### 4.2 新增/强化组件清单（v3.4.0融合态）

| 组件 | 层 | 来源 | 功能 | 融合状态 | 落地方案 |
|------|-----|------|------|----------|----------|
| `experience-card` | L0 | OpenMLE | 经验卡片类型定义与契约 | 🟢 Rust实现 | `nexus-contracts`新增模块 |
| `three-factor` | L0 | OpenMLE | 三因子评分类型与算法 | 🟢 Rust实现 | `nexus-contracts`新增模块 |
| `operator-contracts` | L0 | OpenMLE | 四套原子算子契约 | 🟢 Rust实现 | `nexus-contracts`新增模块 |
| `execution-status-contracts` | L0 | OpenMLE | 六类状态反馈契约 | 🟢 Rust实现 | `nexus-contracts`新增模块 |
| `error-signature-contracts` | L0 | OpenMLE | 错误签名类型与契约 | 🟢 Rust实现 | `nexus-contracts`新增模块 |
| `token-evidence-contracts` | L0 | Dressage | TokenLedgerEntry/SegmentMetadata | 🟢 Rust实现 | `nexus-contracts`新增模块 |
| `memory-pyramid-contracts` | L0 | MSCE+TencentDB | 记忆金字塔契约类型 | 🟢 Rust实现 | `nexus-contracts`新增模块 |
| `skill-lifecycle-contracts` | L0 | MSCE | Skill状态机契约 | 🟢 Rust实现 | `nexus-contracts`新增模块 |
| `six-dimension-contracts` | L0 | MemoHarness | 六维控制面契约 | 🟢 Rust实现 | `nexus-contracts`新增模块 |
| `rl-hook-contracts` | L0 | RL预留 | RL状态/动作/经验类型 | 🟢 Rust实现 | `nexus-contracts`新增模块 |
| `experience-card-bus` | L1 | OpenMLE | Event Bus扩展为经验卡片流 | 🟢 Rust实现 | `event-bus`内部扩展 |
| `token-ledger` | L1 | Dressage | Proxy记录token IDs/logprobs/mask | 🟢 Rust实现 | `event-bus`内部扩展 |
| `segment-aware-per` | L1 | Dressage | 轨迹分段PER，共享父轨迹身份 | 🟢 Rust实现 | `event-bus`内部扩展 |
| `stat-learning-policy` | L1 | RL预留 | 统计学习接口层（SlidingWindow/EMA） | 🟢 Rust实现 | `nexus-core`新增模块 |
| `rl-types` | L1 | RL预留 | RL状态/动作/经验类型 | 🟢 Rust实现 | `nexus-core`新增模块 |
| `rl-client` | L1 | RL预留 | gRPC客户端骨架+RulePolicyFallback | 🟢 Rust实现 | `rl-client`升级 |
| `experience-card-system` | L2 | OpenMLE | 案例级+全局经验板+方法家族统计 | 🟢 Rust实现 | 新增`experience-card-system` crate |
| `on-demand-synthesizer` | L2 | OpenMLE | 按需记忆合成（懒加载） | 🟢 Rust实现 | 新增`on-demand-synthesizer` crate |
| `ancestor-sibling-index` | L2 | OpenMLE | 祖先/兄弟节点快速索引 | 🟢 Rust实现 | `experience-card-system`内部 |
| `global-experience-board` | L2 | OpenMLE | 搜索树全局统计+错误聚类 | 🟢 Rust实现 | `experience-card-system`内部 |
| `hils-attention` | L2 | HiLS | 分层稀疏注意力机制 | 🟡 新增crate | `crates/hils-attention` |
| `retrieval-three-way` | L2 | TencentDB | 字面+语义+混合检索 | 🟢 Rust实现 | `mlc-engine`内部扩展 |
| `injection-strategy` | L2 | TencentDB | 用户消息前/系统提示末尾注入 | 🟢 Rust实现 | `hcw-window`内部扩展 |
| `conflict-degradation` | L2 | TencentDB | 检索降级链 | 🟢 Rust实现 | `mlc-engine`内部扩展 |
| `pyramid-storage` | L3 | TencentDB | L0 Raw→L1 Atomic→L2 Scene→L3 Persona | 🟢 Rust实现 | `cmt-tiering`内部扩展 |
| `experience-card-storage` | L3 | OpenMLE | SQLite持久化+三因子索引 | 🟢 Rust实现 | 新增`experience-card-storage` crate |
| `three-factor-index` | L3 | OpenMLE | 三因子评分复合索引 | 🟢 Rust实现 | `experience-card-storage`内部 |
| `error-signature-index` | L3 | OpenMLE | 错误签名倒排索引 | 🟢 Rust实现 | `experience-card-storage`内部 |
| `auto-builder` | L4 | 快手 | 双智能体环境构建 | 🟢 Rust实现 | 新增`auto-builder` crate |
| `error-signature-collector` | L4 | OpenMLE | 结构化错误签名提取+聚类 | 🟢 Rust实现 | 新增`error-signature-collector` crate |
| `six-class-feedback-integrator` | L4 | OpenMLE | 六类状态全链路追踪 | 🟢 Rust实现 | `error-signature-collector`内部 |
| `paddock-sandbox` | L4 | Dressage | Paddock(what-to-do) + SandboxProvider(where) | 🟢 Rust实现 | `seccore`内部扩展 |
| `four-operators` | L5 | OpenMLE | Draft/Improve/Debug/Crossover | 🟢 Rust实现 | 新增`four-operators` crate |
| `three-factor-selector` | L5 | OpenMLE | UCB+Softmax+冷却父本选择 | 🟢 Rust实现 | 新增`three-factor-selector` crate |
| `variant-pool` | L5 | 小米 | 变体隔离池+统计路由 | 🟢 Rust实现 | 新增`variant-pool` crate |
| `checkpoint-preserver` | L5 | RSIBench | 保留历史最佳checkpoint | 🟢 Rust实现 | 新增`checkpoint-preserver` crate |
| `dual-experience-bank` | L5 | MemoHarness | 案例级+全局双层经验库 | 🟢 Rust实现 | 新增`dual-experience-bank` crate |
| `msce-integration` | L5 | MSCE | 三层记忆融合+双信号价值回填 | 🟢 Rust实现 | 新增`msce-integration` crate |
| `skill-lifecycle` | L5 | MSCE | Skill状态机(probationary→active→archived) | 🟢 Rust实现 | `skill-graph`内部扩展 |
| `aegis-gsoe` | L5 | 小米 | AEGIS四阶段规则引擎 | 🟢 Rust实现 | 新增`aegis-gsoe` crate |
| `operator-router` | L6 | OpenMLE | 算子路由（Greedy/ThreeFactor/UCB/Cooling） | 🟢 Rust实现 | 新增`operator-router` crate |
| `skills-progressive-loader` | L6 | PenguinHarness | Skills渐进加载（Index First, Body on Demand） | 🟢 Rust实现 | 新增`skills-progressive-loader` crate |
| `six-dimension-adjuster` | L6 | MemoHarness | 六维控制面动态调整 | 🟢 Rust实现 | `osa-coordinator`内部扩展 |
| `parent-selector` | L6 | OpenMLE | L6层三因子父本选择接口 | 🟢 Rust实现 | `operator-router`内部 |
| `tool-schema-pruner` | L6 | Dressage | 基于使用频率动态裁剪工具schema | 🟢 Rust实现 | `osa-coordinator`内部扩展 |
| `atomic-operators` | L7 | OpenMLE | 算子执行引擎 | 🟢 Rust实现 | 新增`atomic-operators` crate |
| `experience-card-generator` | L7 | OpenMLE | PVL验证结果→经验卡片转换 | 🟢 Rust实现 | 新增`experience-card-generator` crate |
| `process-score-calculator` | L7 | 快手 | 九维度过程评分（规则版） | 🟢 Rust实现 | 新增`process-score-calculator` crate |
| `dynamic-verification-depth` | L7 | 快手 | 基于风险和历史成功率的验证深度调整 | 🟢 Rust实现 | 新增`dynamic-verification-depth` crate |
| `entropy-weighted-scoring` | L7 | OpenMLE | 熵加权统计评分 | 🟢 Rust实现 | `process-score-calculator`内部 |
| `segment-aware-validation` | L7 | Dressage | 轨迹分段验证，共享父轨迹身份 | 🟢 Rust实现 | `pvl-layer`内部扩展 |
| `variant-parliament` | L8 | 小米 | 变体审议（三角色+烟雾测试） | 🟢 Rust实现 | `parliament`内部扩展 |
| `three-factor-adjudicator` | L8 | OpenMLE | 三因子裁决（Skeptic/Security/Execution） | 🟢 Rust实现 | 新增`three-factor-adjudicator` crate |
| `stop-strategy` | L8 | RSIBench | 停止策略（尝试次数+分数差距+停滞计数） | 🟢 Rust实现 | 新增`stop-strategy` crate |
| `behavior-localization` | L8 | 腾讯 | 行为定位导航 | 🟢 Rust实现 | `parliament`内部扩展 |
| `conflict-arbitration` | L8 | TencentDB | 候选召回→模型判断(新增/跳过/更新/合并) | 🟢 Rust实现 | `parliament`内部扩展 |
| `ambient-mode` | L9 | jcode | 后台常驻模式 | 🟢 Rust实现 | 新增`ambient-mode` crate |
| `search-tree-manager` | L9 | OpenMLE | 搜索树管理（扩展/选择/剪枝/最优路径） | 🟢 Rust实现 | 新增`search-tree-manager` crate |
| `long-term-credit-assigner` | L9 | SHARP | 长程信用分配（统计版） | 🟢 Rust实现 | 新增`long-term-credit-assigner` crate |
| `long-task-map` | L9 | TencentDB | 详细过程转存外部文件，上下文留任务地图 | 🟢 Rust实现 | `quest-engine`内部扩展 |
| `self-assessment-panel` | L10 | Qoder | 五维度+三因子自我评估面板 | 🟢 Rust实现 | `chimera-tui`新增面板 |
| `experience-card-viz` | L10 | OpenMLE | 经验卡片可视化 | 🟢 Rust实现 | `chimera-tui`新增面板 |
| `runtime-auditor` | L10 | Qoder | 运行时审计器（静态配置≠已执行验证） | 🟢 Rust实现 | 新增`runtime-auditor` crate |
| `omni-message` | L0 | PenguinHarness | 模型-环境解耦协议 | 🔵 预留接口 | `docs/roadmap/omni-message.md` |
| `openforge-proxy` | L1 | 微软 | Proxy拦截 | 🔵 预留接口 | `docs/roadmap/openforge-proxy.md` |
| `grpo-trainer` | L5 | DeepSeek-R1 | 在线GRPO | 🔴 v4.0计划 | — |
| `ppo-trainer` | L6 | PPO | PPO策略网络 | 🔴 v4.0计划 | — |
| `mappo-trainer` | L8 | MAPPO | 多智能体PPO | 🔴 v4.0计划 | — |
| `dqn-trainer` | L2 | DQN | DQN记忆策略 | 🔴 v4.0计划 | — |
| `gtpo-trainer` | L7 | GTPO | Turn-Level优化 | 🔴 v4.0计划 | — |

---


## 5. L0 Contracts：契约层——六维控制面 + 经验卡片 + 平台接地 + Token证据

### 5.1 设计原则

`nexus-contracts`（ADR-033）是零依赖契约层，承载跨层共享类型。v3.4.0遵循以下原则：
- **仅依赖`serde`**，无运行时逻辑
- **经验卡片为不可变数据结构**，版本化存储
- **三因子评分为纯函数**，无副作用
- **六维控制面统一暴露**，每层按需订阅
- **Token证据与Segment元数据标准化**，为v4.0 RL训练预留数据流

### 5.2 经验卡片契约（OpenMLE核心 + Dressage Token证据）

OpenMLE的核心创新：每个执行节点生成**经验卡片**，记录结构化信息。卡片是Chimera的Event Bus一级公民。融合Dressage的Token级证据，形成完整的经验-证据闭环。

```rust
// crates/nexus-contracts/src/experience_card.rs
#![forbid(unsafe_code)]

use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

/// 经验卡片：OpenMLE的核心数据结构，Chimera的Event Bus一级公民
/// 融合Dressage: 每张卡片关联TokenLedgerEntry IDs，形成证据链
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ExperienceCard {
    pub card_id: String,              // UUIDv7
    pub task_id: String,
    pub node_id: String,
    pub parent_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub operator: AtomicOperator,
    pub score: f32,                   // 0.0-1.0
    pub delta_vs_parent: f32,         // 相对父节点的改进幅度
    pub method_family: String,        // 方法家族（如"draft_pipeline"）
    pub error_signature: Option<ErrorSignature>,
    pub three_factor: ThreeFactorScore,
    pub execution_status: ExecutionStatus,
    pub token_evidence_ids: Vec<String>, // 关联的TokenLedgerEntry IDs (Dressage)
    pub segment_id: Option<String>,      // 关联的Segment ID (Dressage)
    pub metadata: CardMetadata,
}

/// 四套原子算子（OpenMLE）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub enum AtomicOperator {
    Draft,      // 从零起草
    Improve,    // 迭代改进
    Debug,      // 错误修复
    Crossover,  // 代码融合
}

impl AtomicOperator {
    pub fn is_generative(&self) -> bool {
        matches!(self, AtomicOperator::Draft | AtomicOperator::Crossover)
    }
    pub fn is_modifying(&self) -> bool {
        matches!(self, AtomicOperator::Improve | AtomicOperator::Debug)
    }
    pub fn default_token_estimate(&self) -> usize {
        match self {
            AtomicOperator::Draft => 5000,
            AtomicOperator::Improve => 3000,
            AtomicOperator::Debug => 2000,
            AtomicOperator::Crossover => 4000,
        }
    }
}

/// 三因子评分（OpenMLE核心）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ThreeFactorScore {
    pub quality: f32,     // 绝对质量（如验证通过率）
    pub progress: f32,    // 相对父本的改进幅度
    pub novelty: f32,     // 方法新颖性（避免重复相同路径）
}

impl ThreeFactorScore {
    pub fn selection_utility(&self) -> f32 {
        self.quality + self.progress + self.novelty
    }
    pub fn normalize(&self, max_q: f32, max_p: f32, max_n: f32) -> NormalizedThreeFactor {
        NormalizedThreeFactor {
            quality: self.quality / max_q.max(1e-8),
            progress: self.progress / max_p.max(1e-8),
            novelty: self.novelty / max_n.max(1e-8),
        }
    }
    pub fn default_root() -> Self {
        Self { quality: 0.0, progress: 0.0, novelty: 1.0 }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NormalizedThreeFactor {
    pub quality: f32,
    pub progress: f32,
    pub novelty: f32,
}

/// 错误签名（OpenMLE + 结构化收集）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ErrorSignature {
    pub error_type: String,
    pub error_location: String,
    pub error_summary: String,
    pub error_hash: String,  // SHA-256前16位
}

impl ErrorSignature {
    pub fn compute_hash(error_type: &str, error_summary: &str) -> String {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(error_type.as_bytes());
        hasher.update(error_summary.as_bytes());
        format!("{:x}", hasher.finalize())[..16].to_string()
    }
}

/// 执行状态（六类，OpenMLE）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ExecutionStatus {
    Success,      // 成功
    Error,        // 执行错误
    MissingCode,  // 未生成代码
    NoSubmit,     // 未提交
    ScoreFailed,  // 评分失败
    Timeout,      // 超时
}

impl ExecutionStatus {
    pub fn is_retryable(&self) -> bool {
        matches!(self, ExecutionStatus::Error | ExecutionStatus::Timeout | ExecutionStatus::ScoreFailed)
    }
    pub fn generates_meaningful_card(&self) -> bool {
        matches!(self, ExecutionStatus::Success | ExecutionStatus::Error | ExecutionStatus::Timeout)
    }
}

/// 卡片元数据
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct CardMetadata {
    pub execution_time_ms: u64,
    pub token_usage: TokenUsage,
    pub lines_changed: i32,
    pub skills_used: Vec<String>,
    pub environment: EnvironmentInfo,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct EnvironmentInfo {
    pub rust_version: String,
    pub os: String,
    pub cpu_arch: String,
    pub chimera_version: String,
}
```

### 5.3 Token级证据契约（Dressage融合）

```rust
// crates/nexus-contracts/src/token_evidence.rs
#![forbid(unsafe_code)]

/// TokenLedgerEntry: 单次模型调用的token级证据 (Dressage核心)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TokenLedgerEntry {
    pub entry_id: String,           // UUIDv7
    pub turn_id: u32,
    pub session_id: String,
    pub instance_id: String,        // 分布式训练标识
    pub input_token_ids: Vec<u32>,
    pub output_token_ids: Vec<u32>,
    pub output_logprobs: Vec<f32>,
    pub loss_mask: Vec<bool>,
    pub weight_version: String,     // 模型权重版本
    pub tool_calls: Vec<ToolCallRecord>,
    pub moe_routing: Option<Vec<Vec<u32>>>,
    pub timestamp: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ToolCallRecord {
    pub tool_name: String,
    pub arguments: String,
    pub result: String,
    pub latency_ms: u32,
}

/// SegmentMetadata: 轨迹分段元数据 (Dressage)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SegmentMetadata {
    pub segment_id: String,
    pub parent_traj_id: String,     // 父轨迹ID（所有segment共享）
    pub segment_index: u32,
    pub is_anchor: bool,            // 是否anchor segment（承载终局reward）
    pub token_entries: Vec<String>, // 关联的TokenLedgerEntry IDs
    pub context_snapshot: Vec<u8>,  // MessagePack
    pub start_turn: u32,
    pub end_turn: u32,
    pub creation_reason: SegmentCreationReason,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum SegmentCreationReason {
    HistoryCompaction,      // 历史压缩
    ToolSchemaChange,       // 工具schema变更
    MessageRewrite,         // 消息重写
    TITOFallback,           // TITO回退
    NaturalBoundary,        // 自然边界
    MaxLengthReached,       // 达到最大长度
}
```

### 5.4 记忆金字塔契约（MSCE + TencentDB融合）

```rust
// crates/nexus-contracts/src/memory_pyramid.rs
#![forbid(unsafe_code)]

/// MemoryPyramidLevel: 记忆金字塔层级 (MSCE L1/L2/L3 × TencentDB L0/L1/L2/L3)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum MemoryPyramidLevel {
    L0RawLog,           // 全量原始对话（TencentDB L0）
    L1AtomicMemory,     // 结构化卡片（TencentDB L1 / MSCE L1 Trace）
    L2SceneBlock,       // 场景档案（TencentDB L2 / MSCE L2 Policy）
    L3Persona,          // 人格摘要（TencentDB L3 / MSCE L3 Env Cognition）
}

/// AtomicMemoryCard: L1原子记忆卡片（TencentDB + MSCE L1 Trace融合）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AtomicMemoryCard {
    pub card_id: String,
    pub card_type: AtomicCardType,
    pub priority: u8,
    pub scene: String,
    pub content: String,
    pub source_traj_id: Option<String>,
    pub state_snapshot: Option<RLStateVector>,
    pub action_record: Option<RLActionVector>,
    pub observation: Option<String>,
    pub reflection: Option<String>,
    pub value: Option<f32>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum AtomicCardType {
    Preference, Event, Rule,     // TencentDB
    Trace, Policy, EnvCognition, // MSCE
}

/// SceneBlock: L2场景档案（TencentDB + MSCE L2 Policy融合）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SceneBlock {
    pub block_id: String,
    pub scene_name: String,
    pub cards: Vec<String>,
    pub summary: String,
    pub heat_value: u32,
    pub trigger_phi: Option<String>,   // MSCE: 触发器
    pub procedure_pi: Option<String>,  // MSCE: 执行过程
    pub verification_kappa: Option<String>, // MSCE: 验证条件
    pub boundary_beta: Option<String>, // MSCE: 边界条件
    pub gain_gamma: Option<f32>,       // MSCE: 增益
}

/// PersonaSummary: L3人格摘要（TencentDB L3）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PersonaSummary {
    pub persona_id: String,
    pub user_id: String,
    pub summary: String,
    pub preferences: Vec<String>,
    pub rules: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
}
```

### 5.5 Skill生命周期契约（MSCE）

```rust
// crates/nexus-contracts/src/skill_lifecycle.rs
#![forbid(unsafe_code)]

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum SkillLifecycleState {
    Probationary,   // 试用期：刚生成，待验证
    Active,         // 激活：通过验证，可检索使用
    Archived,       // 归档：长期未使用或负面反馈，降级存储
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SkillLifecycleContract {
    pub skill_id: String,
    pub state: SkillLifecycleState,
    pub probation_start: u64,
    pub probation_end: Option<u64>,
    pub activation_threshold: u32,  // 激活所需成功次数（默认3）
    pub success_count: u32,
    pub failure_count: u32,
    pub archive_threshold: u32,   // 归档所需失败次数（默认5）
    pub last_used: u64,
}
```

### 5.6 六维控制面契约（MemoHarness融合）

```rust
// crates/nexus-contracts/src/six_dimensions.rs
#![forbid(unsafe_code)]

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SixDimensionConfig {
    pub d1_context: D1ContextConfig,
    pub d2_tool: D2ToolConfig,
    pub d3_generation: D3GenerationConfig,
    pub d4_orchestration: D4OrchestrationConfig,
    pub d5_memory: D5MemoryConfig,
    pub d6_output: D6OutputConfig,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct D1ContextConfig {
    pub max_tokens: usize,
    pub compression: CompressionStrategy,
    pub inject_examples: bool,
    pub structured_prompt: bool,
    pub on_demand_synthesis: bool,         // OpenMLE
    pub ancestor_retrieval_depth: u32,     // OpenMLE
    pub sibling_retrieval_count: u32,      // OpenMLE
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum CompressionStrategy { None, Truncate, Summarize, Semantic }

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct D2ToolConfig {
    pub max_tools_per_step: usize,
    pub retrieval_top_k: usize,
    pub reranking: bool,
    pub tool_timeout_ms: u64,
    pub progressive_skill_loading: bool,   // PenguinHarness
    pub max_full_skill_load: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct D3GenerationConfig {
    pub max_output_tokens: usize,
    pub temperature: f32,
    pub top_p: f32,
    pub candidate_sampling: bool,
    pub operator_selection: OperatorSelectionStrategy, // OpenMLE
    pub entropy_weighting: bool,                         // OpenMLE
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum OperatorSelectionStrategy { Greedy, ThreeFactor, UCB, Cooling }

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct D4OrchestrationConfig {
    pub workflow: WorkflowType,
    pub max_iterations: u32,
    pub retry_policy: RetryPolicy,
    pub search_tree_depth: u32,            // OpenMLE
    pub budget_hours: f32,
    pub stop_strategy: StopStrategyConfig, // RSIBench
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum WorkflowType { SingleCall, PlanExecute, MultiAgent, SearchTree }

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub backoff_multiplier: f32,
    pub max_backoff_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StopStrategyConfig {
    pub max_attempts: u32,
    pub stagnation_threshold: u32,
    pub score_gap_threshold: f32,
    pub preserve_best: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct D5MemoryConfig {
    pub retention: RetentionPolicy,
    pub summarization_trigger: usize,
    pub eviction: EvictionStrategy,
    pub on_demand_synthesis: bool,
    pub ancestor_retrieval_depth: u32,
    pub sibling_retrieval_count: u32,
    pub dual_experience_bank: bool,        // MemoHarness
    pub global_experience_board: bool,     // OpenMLE
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum RetentionPolicy { Forever, TimeBased { ttl_hours: u64 }, ScoreBased { min_score: f32 } }

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum EvictionStrategy { LRU, LFU, ScoreWeighted }

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct D6OutputConfig {
    pub extraction_format: ExtractionFormat,
    pub validation_rules: Vec<ValidationRule>,
    pub fallback: FallbackStrategy,
    pub collect_error_signatures: bool,     // OpenMLE
    pub track_execution_status: bool,     // OpenMLE
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ExtractionFormat { Raw, Json, Markdown, CodeBlocks }

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ValidationRule { pub rule_type: String, pub condition: String }

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum FallbackStrategy { Retry, Skip, AskHuman, UseDefault }
```

### 5.7 RL预留钩子契约

```rust
// crates/nexus-contracts/src/rl_hooks.rs
#![forbid(unsafe_code)]

#[async_trait::async_trait]
pub trait RLHook: Send + Sync {
    fn export_trajectory(&self) -> RLTrajectory;
    fn load_policy(&mut self, policy: SerializedPolicy);
    fn report_reward(&self, reward: f32);
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SerializedPolicy {
    pub format: PolicyFormat,
    pub bytes: Vec<u8>,
    pub version: String,
    pub layer: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum PolicyFormat { ONNX, SafeTensors, Json }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RLTrajectory {
    pub episode_id: String,
    pub states: Vec<RLStateVector>,
    pub actions: Vec<RLActionVector>,
    pub rewards: Vec<f32>,
    pub timestamps: Vec<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RLStateVector {
    pub clv: [f32; 512],
    pub layer_features: [f32; 128],
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RLActionVector {
    pub layer: String,
    pub action_code: u32,
    pub parameters: Vec<f32>,
}
```

### 5.8 OmniMessage协议（PenguinHarness预留）

```rust
// crates/nexus-contracts/src/omni_message.rs (预留接口)
#![forbid(unsafe_code)]

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum OmniMessage {
    ModelRequest { request_id: String, prompt: String, model_config: ModelConfig, timestamp: u64 },
    ModelResponse { request_id: String, content: String, usage: TokenUsage, timestamp: u64 },
    ToolRequest { request_id: String, tool_name: String, parameters: serde_json::Value, timestamp: u64 },
    ToolResult { request_id: String, success: bool, output: String, error: Option<String>, timestamp: u64 },
    StateUpdate { key: String, value: serde_json::Value, timestamp: u64 },
    TraceRecord { step: u32, action: String, observation: String, reward: f32, timestamp: u64 },
}
```

---


## 6. L1 Core：核心层——Event Bus经验卡片化 + Segment-aware PER + 统计学习接口 + RL预留

### 6.1 经验卡片Event Bus

将Event Bus扩展为经验卡片的一级公民，支持双通道（Normal broadcast + Critical mpsc）分级投递，四索引（task/node/factor/error）快速检索。

```rust
// crates/event-bus/src/experience_card_bus.rs
#![forbid(unsafe_code)]
use nexus_contracts::{ExperienceCard, ThreeFactorScore, ExecutionStatus};
use tokio::sync::{broadcast, mpsc};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct ExperienceCardBus {
    base_bus: EventBus,
    card_broadcast: broadcast::Sender<ExperienceCard>,
    card_critical: mpsc::UnboundedSender<ExperienceCard>,
    card_index: DashMap<String, Vec<ExperienceCard>>,      // task_id -> cards
    node_index: DashMap<String, ExperienceCard>,           // node_id -> card
    factor_cache: DashMap<String, ThreeFactorScore>,        // card_id -> factor
    error_index: DashMap<String, Vec<String>>,              // error_hash -> card_ids
    total_cards: AtomicU64,
    total_evaluated: AtomicU64,
}

impl ExperienceCardBus {
    pub fn new(base_bus: EventBus) -> Self {
        let (broadcast_tx, _) = broadcast::channel(1024);
        let (critical_tx, _) = mpsc::unbounded_channel();
        Self {
            base_bus, card_broadcast: broadcast_tx, card_critical: critical_tx,
            card_index: DashMap::new(), node_index: DashMap::new(),
            factor_cache: DashMap::new(), error_index: DashMap::new(),
            total_cards: AtomicU64::new(0), total_evaluated: AtomicU64::new(0),
        }
    }

    pub async fn publish(&self, card: ExperienceCard) -> Result<(), BusError> {
        // 索引更新（同步，无锁）
        self.card_index.entry(card.task_id.clone()).or_default().push(card.clone());
        self.node_index.insert(card.node_id.clone(), card.clone());
        self.factor_cache.insert(card.card_id.clone(), card.three_factor.clone());
        if let Some(ref sig) = card.error_signature {
            self.error_index.entry(sig.error_hash.clone()).or_default().push(card.card_id.clone());
        }
        self.total_cards.fetch_add(1, Ordering::SeqCst);
        if card.execution_status == ExecutionStatus::Success {
            self.total_evaluated.fetch_add(1, Ordering::SeqCst);
        }

        // 分级投递：高分走Critical(mpsc)，中分走Normal(broadcast)，低分静默丢弃
        if card.score > 0.8 {
            self.card_critical.send(card)?;
        } else if card.score > 0.5 {
            let _ = self.card_broadcast.send(card);
        }
        Ok(())
    }

    pub fn get_cards_by_task(&self, task_id: &str) -> Vec<ExperienceCard> {
        self.card_index.get(task_id).map(|v| v.clone()).unwrap_or_default()
    }

    pub fn get_card_by_node(&self, node_id: &str) -> Option<ExperienceCard> {
        self.node_index.get(node_id).map(|v| v.clone())
    }

    pub fn get_top_cards_by_factor(&self, task_id: &str, k: usize) -> Vec<ExperienceCard> {
        let mut cards = self.get_cards_by_task(task_id);
        cards.sort_by(|a, b| {
            let score_a = a.three_factor.selection_utility();
            let score_b = b.three_factor.selection_utility();
            score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal)
        });
        cards.into_iter().take(k).collect()
    }

    pub fn get_cards_by_error_hash(&self, error_hash: &str) -> Vec<ExperienceCard> {
        self.error_index.get(error_hash).map(|ids| {
            ids.iter().filter_map(|id| self.node_index.get(id).map(|c| c.clone())).collect()
        }).unwrap_or_default()
    }

    pub fn get_global_stats(&self) -> GlobalCardStats {
        GlobalCardStats {
            total_cards: self.total_cards.load(Ordering::SeqCst),
            total_evaluated: self.total_evaluated.load(Ordering::SeqCst),
            unique_errors: self.error_index.len() as u64,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GlobalCardStats {
    pub total_cards: u64,
    pub total_evaluated: u64,
    pub unique_errors: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum BusError {
    #[error("Critical channel closed")] CriticalChannelClosed,
    #[error("Broadcast error: {0}")] BroadcastError(String),
}
```

### 6.2 Segment-aware PER（Dressage核心）

轨迹分段优先级经验回放，共享父轨迹身份，只有anchor segment承载终局reward，prompt-equal denominator避免segment数量影响梯度。

```rust
// crates/event-bus/src/segment_per.rs
#![forbid(unsafe_code)]
use nexus_contracts::{RLTrajectory, SegmentMetadata};

pub struct SegmentAwarePER {
    per_buffer: PERBuffer,
    segment_registry: HashMap<String, Vec<SegmentMetadata>>,
    anchor_rewards: HashMap<String, f32>,
}

impl SegmentAwarePER {
    pub fn add_segment(&mut self, exp: RLExperience, segment: &SegmentMetadata, td_error: f32) {
        let mut exp = exp;
        if segment.is_anchor {
            self.anchor_rewards.insert(segment.parent_traj_id.clone(), exp.reward);
        }
        exp.segment_id = Some(segment.segment_id.clone());
        exp.parent_traj_id = Some(segment.parent_traj_id.clone());

        // prompt-equal denominator: 避免segment数量影响梯度
        let segment_count = self.segment_registry
            .get(&segment.parent_traj_id)
            .map(|v| v.len() as f32)
            .unwrap_or(1.0);
        let adjusted_td_error = td_error / segment_count.sqrt();
        self.per_buffer.add(exp, adjusted_td_error);
    }

    pub fn broadcast_reward(&mut self, parent_traj_id: &str, reward: f32) {
        self.anchor_rewards.insert(parent_traj_id.to_string(), reward);
    }

    pub fn sample_segment_batch(&mut self, batch_size: usize) -> (Vec<&RLExperience>, Vec<f32>) {
        let (samples, weights) = self.per_buffer.sample(batch_size);
        // 确保同轨迹的segment共享相同的advantage归一化
        let mut traj_groups: HashMap<String, Vec<&RLExperience>> = HashMap::new();
        for sample in &samples {
            if let Some(ref traj_id) = sample.parent_traj_id {
                traj_groups.entry(traj_id.clone()).or_default().push(*sample);
            }
        }
        (samples, weights)
    }
}
```

### 6.3 统计学习接口层

Rust侧先用统计方法实现80%的RL价值，为v4.0神经网络预留接口。

```rust
// crates/nexus-core/src/stat_learning.rs
#![forbid(unsafe_code)]
use std::collections::{HashMap, VecDeque};

pub trait StatLearningPolicy: Send + Sync {
    type State: Clone + Hash + Eq;
    type Action: Clone + Hash + Eq;
    fn predict(&self, state: &Self::State) -> Self::Action;
    fn update(&mut self, state: &Self::State, action: &Self::Action, reward: f32);
    fn export_trajectory(&self) -> RLTrajectory;
    fn get_action_stats(&self) -> HashMap<Self::Action, ActionStats>;
}

#[derive(Clone, Debug)]
pub struct ActionStats {
    pub count: u32,
    pub avg_reward: f32,
    pub last_reward: f32,
    pub confidence: f32,
}

/// Sliding Window EMA Policy（OpenMLE动态奖励归一化）
pub struct SlidingWindowPolicy<S, A> {
    window_size: usize,
    history: VecDeque<(S, A, f32)>,
    action_counts: HashMap<A, u32>,
    action_rewards: HashMap<A, f32>,
    epsilon: f32,
}

impl<S: Clone + Hash + Eq, A: Clone + Hash + Eq + Default> StatLearningPolicy for SlidingWindowPolicy<S, A> {
    type State = S; type Action = A;

    fn predict(&self, _state: &S) -> A {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        if rng.gen::<f32>() < self.epsilon {
            let actions: Vec<&A> = self.action_counts.keys().collect();
            if !actions.is_empty() { return actions[rng.gen_range(0..actions.len())].clone(); }
        }
        self.action_rewards.iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(a, _)| a.clone()).unwrap_or_default()
    }

    fn update(&mut self, state: &S, action: &A, reward: f32) {
        self.history.push_back((state.clone(), action.clone(), reward));
        if self.history.len() > self.window_size { self.history.pop_front(); }
        *self.action_counts.entry(action.clone()).or_insert(0) += 1;
        let current = self.action_rewards.entry(action.clone()).or_insert(0.0);
        *current = (*current * 0.9) + (reward * 0.1);  // EMA
    }

    fn export_trajectory(&self) -> RLTrajectory {
        RLTrajectory {
            states: self.history.iter().map(|(s, _, _)| s.clone()).collect(),
            actions: self.history.iter().map(|(_, a, _)| a.clone()).collect(),
            rewards: self.history.iter().map(|(_, _, r)| *r).collect(),
        }
    }

    fn get_action_stats(&self) -> HashMap<A, ActionStats> {
        self.action_rewards.iter().map(|(action, reward)| {
            let count = self.action_counts.get(action).copied().unwrap_or(0);
            (action.clone(), ActionStats {
                count, avg_reward: *reward,
                last_reward: self.history.iter().rev().find(|(_, a, _)| a == action).map(|(_, _, r)| *r).unwrap_or(0.0),
                confidence: if count > 0 { *reward / (count as f32).sqrt() } else { f32::MAX },
            })
        }).collect()
    }
}

/// UCB Policy（OpenMLE三因子选择基础）
pub struct UCBPolicy<S, A> {
    total_visits: u32,
    action_visits: HashMap<A, u32>,
    action_values: HashMap<A, f32>,
    exploration_constant: f32,
    _phantom: std::marker::PhantomData<S>,
}

impl<S, A: Clone + Hash + Eq> UCBPolicy<S, A> {
    pub fn new(exploration_constant: f32) -> Self {
        Self { total_visits: 0, action_visits: HashMap::new(), action_values: HashMap::new(), exploration_constant, _phantom: std::marker::PhantomData }
    }
    fn ucb_score(&self, action: &A) -> f32 {
        let value = self.action_values.get(action).copied().unwrap_or(0.0);
        let visits = self.action_visits.get(action).copied().unwrap_or(0);
        if visits == 0 { return f32::MAX; }
        value + self.exploration_constant * ((2.0 * (self.total_visits as f32).ln()) / (visits as f32)).sqrt()
    }
}

impl<S: Clone + Hash + Eq, A: Clone + Hash + Eq + Default> StatLearningPolicy for UCBPolicy<S, A> {
    type State = S; type Action = A;
    fn predict(&self, _state: &S) -> A {
        self.action_values.keys().chain(self.action_visits.keys()).collect::<std::collections::HashSet<_>>()
            .iter().max_by(|a, b| self.ucb_score(a).partial_cmp(&self.ucb_score(b)).unwrap_or(std::cmp::Ordering::Equal))
            .map(|a| (*a).clone()).unwrap_or_default()
    }
    fn update(&mut self, _state: &S, action: &A, reward: f32) {
        self.total_visits += 1;
        *self.action_visits.entry(action.clone()).or_insert(0) += 1;
        let value = self.action_values.entry(action.clone()).or_insert(0.0);
        *value = (*value * 0.9) + (reward * 0.1);
    }
    fn export_trajectory(&self) -> RLTrajectory { RLTrajectory { states: vec![], actions: vec![], rewards: vec![] } }
    fn get_action_stats(&self) -> HashMap<A, ActionStats> {
        self.action_values.iter().map(|(action, reward)| {
            let count = self.action_visits.get(action).copied().unwrap_or(0);
            (action.clone(), ActionStats { count, avg_reward: *reward, last_reward: *reward,
                confidence: if count > 0 { self.exploration_constant * ((2.0 * (self.total_visits as f32).ln()) / (count as f32)).sqrt() } else { f32::MAX } })
        }).collect()
    }
}
```

### 6.4 RL客户端骨架（RulePolicyFallback默认）

```rust
// crates/rl-client/src/lib.rs
#![forbid(unsafe_code)]
use nexus_contracts::{RLStateVector, RLActionVector, RLTrajectory, SerializedPolicy};
use async_trait::async_trait;

#[async_trait]
pub trait RLClient: Send + Sync {
    async fn predict(&mut self, state: RLStateVector) -> Result<RLActionVector, RLError>;
    async fn report_experience(&mut self, trajectory: RLTrajectory) -> Result<(), RLError>;
    async fn sync_policy(&mut self, layer: &str) -> Result<SerializedPolicy, RLError>;
}

/// 默认回退：规则策略，零Python依赖
pub struct RulePolicyFallback;

#[async_trait]
impl RLClient for RulePolicyFallback {
    async fn predict(&mut self, _state: RLStateVector) -> Result<RLActionVector, RLError> {
        Ok(RLActionVector { layer: "fallback".to_string(), action_code: 0, parameters: vec![0.1] })
    }
    async fn report_experience(&mut self, _trajectory: RLTrajectory) -> Result<(), RLError> {
        tracing::debug!("Experience reported to fallback (local only)");
        Ok(())
    }
    async fn sync_policy(&mut self, layer: &str) -> Result<SerializedPolicy, RLError> {
        Ok(SerializedPolicy {
            format: nexus_contracts::PolicyFormat::Json,
            bytes: vec![],
            version: "rule-fallback-v3.4.0".to_string(),
            layer: layer.to_string(),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RLError {
    #[error("Policy not found: {0}")] PolicyNotFound(String),
    #[error("Network error: {0}")] NetworkError(String),
    #[error("Invalid state: {0}")] InvalidState(String),
}
```

---

## 7. L2 Memory：记忆层——经验卡片系统 + 按需记忆合成 + 双层经验库 + 记忆图谱 + HiLS注意力

### 7.1 经验卡片系统（OpenMLE核心）

```rust
// crates/experience-card-system/src/lib.rs
#![forbid(unsafe_code)]
use nexus_contracts::{ExperienceCard, ThreeFactorScore, AtomicOperator, ExecutionStatus, ErrorSignature};
use std::collections::HashMap;

pub struct ExperienceCardSystem {
    case_cards: Vec<ExperienceCard>,
    global_board: GlobalExperienceBoard,
    method_stats: HashMap<String, MethodStatistics>,
    node_index: HashMap<String, usize>,
    visit_counts: HashMap<String, u32>,
    exploration_weight: f32,
    cooling_coefficient: f32,
}

#[derive(Clone, Debug, Default)]
pub struct GlobalExperienceBoard {
    pub total_nodes: u64,
    pub total_evaluated: u64,
    pub best_score: f32,
    pub average_score: f32,
    pub method_distribution: HashMap<String, u32>,
    pub error_clusters: HashMap<String, Vec<ErrorSignature>>,
    pub frequent_errors: Vec<(String, u32)>,
}

#[derive(Clone, Debug, Default)]
pub struct MethodStatistics {
    pub count: u32,
    pub total_score: f32,
    pub avg_score: f32,
    pub best_score: f32,
    pub success_rate: f32,
}

impl ExperienceCardSystem {
    pub fn new(exploration_weight: f32, cooling_coefficient: f32) -> Self {
        Self {
            case_cards: Vec::new(),
            global_board: GlobalExperienceBoard::default(),
            method_stats: HashMap::new(),
            node_index: HashMap::new(),
            visit_counts: HashMap::new(),
            exploration_weight, cooling_coefficient,
        }
    }

    pub fn add_card(&mut self, card: ExperienceCard) {
        let idx = self.case_cards.len();
        self.node_index.insert(card.node_id.clone(), idx);
        self.case_cards.push(card.clone());
        self.global_board.total_nodes += 1;
        if card.execution_status == ExecutionStatus::Success { self.global_board.total_evaluated += 1; }
        if card.score > self.global_board.best_score { self.global_board.best_score = card.score; }

        let stats = self.method_stats.entry(card.method_family.clone()).or_default();
        stats.count += 1; stats.total_score += card.score;
        stats.avg_score = stats.total_score / stats.count as f32;
        if card.score > stats.best_score { stats.best_score = card.score; }

        let success_count = self.case_cards.iter()
            .filter(|c| c.method_family == card.method_family && c.execution_status == ExecutionStatus::Success)
            .count() as u32;
        stats.success_rate = success_count as f32 / stats.count as f32;

        *self.global_board.method_distribution.entry(card.method_family.clone()).or_insert(0) += 1;
        if let Some(ref sig) = card.error_signature {
            self.global_board.error_clusters.entry(sig.error_type.clone()).or_default().push(sig.clone());
        }
        let total_score: f32 = self.case_cards.iter().map(|c| c.score).sum();
        self.global_board.average_score = total_score / self.case_cards.len() as f32;
    }

    /// 三因子父本选择（OpenMLE核心算法）
    pub fn select_parent(&mut self, candidates: &[ExperienceCard]) -> Option<&ExperienceCard> {
        if candidates.is_empty() { return None; }
        let max_quality = candidates.iter().map(|c| c.three_factor.quality).fold(0.0, f32::max).max(1e-8);
        let max_progress = candidates.iter().map(|c| c.three_factor.progress.abs()).fold(0.0, f32::max).max(1e-8);
        let max_novelty = candidates.iter().map(|c| c.three_factor.novelty).fold(0.0, f32::max).max(1e-8);

        let mut scored: Vec<(&ExperienceCard, f32)> = candidates.iter().map(|c| {
            let normalized = c.three_factor.normalize(max_quality, max_progress, max_novelty);
            let ucb_bonus = self.ucb_bonus(&c.node_id);
            let cooling = self.cooling_factor();
            let utility = normalized.quality + normalized.progress + normalized.novelty
                + ucb_bonus * self.exploration_weight - cooling;
            (c, utility)
        }).collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let selected = scored.first().map(|(c, _)| *c);
        if let Some(card) = selected {
            *self.visit_counts.entry(card.node_id.clone()).or_insert(0) += 1;
        }
        selected
    }

    fn ucb_bonus(&self, node_id: &str) -> f32 {
        let visits = self.visit_counts.get(node_id).copied().unwrap_or(0);
        if visits == 0 { return f32::MAX; }
        let total_visits: u32 = self.visit_counts.values().sum();
        if total_visits == 0 { return 0.0; }
        (2.0 * (total_visits as f32).ln() / visits as f32).sqrt()
    }

    fn cooling_factor(&self) -> f32 {
        let total_visits: u32 = self.visit_counts.values().sum();
        if total_visits == 0 { return 0.0; }
        self.cooling_coefficient * (total_visits as f32).ln().max(0.0)
    }

    /// 按需记忆合成（OpenMLE核心：懒加载祖先+兄弟节点）
    pub fn synthesize_memory_on_demand(&self, target_card: &ExperienceCard, operator: &AtomicOperator,
        max_ancestors: usize, max_siblings: usize) -> SynthesizedMemory {
        let ancestors = self.find_ancestors(target_card, max_ancestors);
        let siblings = self.find_siblings(target_card, max_siblings);
        let selected = self.select_context_by_operator(operator, &ancestors, &siblings, target_card);
        SynthesizedMemory {
            target: target_card.clone(),
            ancestor_insights: self.extract_insights(&selected.ancestors),
            sibling_patterns: self.extract_patterns(&selected.siblings),
            estimated_tokens: self.estimate_tokens(&selected.ancestors, &selected.siblings),
            context_cards: selected.context,
        }
    }

    fn select_context_by_operator<'a>(&self, operator: &AtomicOperator, ancestors: &[&'a ExperienceCard],
        siblings: &[&'a ExperienceCard], target: &ExperienceCard) -> SelectedContext<'a> {
        match operator {
            AtomicOperator::Draft => {
                let selected: Vec<_> = ancestors.iter().map(|&c| c).take(3).collect();
                SelectedContext { ancestors: selected.clone(), siblings: vec![], context: selected.into_iter().cloned().collect() }
            }
            AtomicOperator::Improve => {
                let mut high_progress: Vec<_> = ancestors.iter().filter(|&&c| c.three_factor.progress > 0.1).map(|&c| c).collect();
                high_progress.sort_by(|a, b| b.three_factor.progress.partial_cmp(&a.three_factor.progress).unwrap());
                let successful_siblings: Vec<_> = siblings.iter().filter(|&&c| c.score > 0.7).map(|&c| c).take(2).collect();
                let mut context = high_progress.clone(); context.extend(successful_siblings.clone());
                SelectedContext { ancestors: high_progress.into_iter().take(3).collect(), siblings: successful_siblings.into_iter().cloned().collect(), context: context.into_iter().cloned().collect() }
            }
            AtomicOperator::Debug => {
                if let Some(ref target_sig) = target.error_signature {
                    let similar_fixes: Vec<_> = siblings.iter().filter(|&&c| {
                        c.error_signature.as_ref().map(|es| es.error_hash == target_sig.error_hash).unwrap_or(false)
                    }).map(|&c| c).take(3).collect();
                    SelectedContext { ancestors: vec![], siblings: similar_fixes.clone(), context: similar_fixes.into_iter().cloned().collect() }
                } else { SelectedContext { ancestors: vec![], siblings: vec![], context: vec![] } }
            }
            AtomicOperator::Crossover => {
                let mut novel_siblings: Vec<_> = siblings.iter().map(|&c| c).collect();
                novel_siblings.sort_by(|a, b| b.three_factor.novelty.partial_cmp(&a.three_factor.novelty).unwrap());
                let selected: Vec<_> = novel_siblings.into_iter().take(2).collect();
                SelectedContext { ancestors: vec![], siblings: selected.clone(), context: selected.into_iter().cloned().collect() }
            }
        }
    }

    fn find_ancestors(&self, card: &ExperienceCard, max_depth: usize) -> Vec<&ExperienceCard> {
        let mut ancestors = vec![];
        let mut current_id = card.parent_id.as_ref();
        for _ in 0..max_depth {
            if let Some(pid) = current_id {
                if let Some(&idx) = self.node_index.get(pid) {
                    let parent = &self.case_cards[idx];
                    ancestors.push(parent);
                    current_id = parent.parent_id.as_ref();
                } else { break; }
            } else { break; }
        }
        ancestors
    }

    fn find_siblings(&self, card: &ExperienceCard, max_count: usize) -> Vec<&ExperienceCard> {
        if let Some(ref parent_id) = card.parent_id {
            self.case_cards.iter().filter(|c| c.parent_id.as_ref() == Some(parent_id) && c.node_id != card.node_id).take(max_count).collect()
        } else { vec![] }
    }

    fn extract_insights(&self, cards: &[&ExperienceCard]) -> Vec<String> {
        cards.iter().map(|c| format!("{}: score={:.2}, progress={:.2}", c.method_family, c.score, c.three_factor.progress)).collect()
    }
    fn extract_patterns(&self, cards: &[&ExperienceCard]) -> Vec<String> {
        cards.iter().map(|c| format!("{}: novelty={:.2}", c.method_family, c.three_factor.novelty)).collect()
    }
    fn estimate_tokens(&self, ancestors: &[&ExperienceCard], siblings: &[&ExperienceCard]) -> usize {
        ancestors.iter().chain(siblings.iter()).map(|c| c.metadata.token_usage.total_tokens as usize).sum()
    }
}

#[derive(Clone, Debug)]
pub struct SynthesizedMemory {
    pub target: ExperienceCard,
    pub ancestor_insights: Vec<String>,
    pub sibling_patterns: Vec<String>,
    pub estimated_tokens: usize,
    pub context_cards: Vec<ExperienceCard>,
}

#[derive(Clone, Debug)]
struct SelectedContext<'a> {
    ancestors: Vec<&'a ExperienceCard>,
    siblings: Vec<&'a ExperienceCard>,
    context: Vec<ExperienceCard>,
}
```

### 7.2 双层经验库（MemoHarness + OpenMLE融合）

```rust
// crates/dual-experience-bank/src/lib.rs
#![forbid(unsafe_code)]
use nexus_contracts::{ExperienceCard, ExecutionStatus};
use chrono::{DateTime, Utc};

pub struct DualExperienceBank {
    case_bank: Vec<CaseExperience>,
    global_bank: Vec<GlobalExperience>,
    distill_threshold: usize,
    last_distillation: DateTime<Utc>,
    task_type_index: HashMap<String, Vec<usize>>,
}

#[derive(Clone, Debug)]
pub struct CaseExperience {
    pub card: ExperienceCard,
    pub task_type: String,
    pub distilled: bool,
    pub inserted_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct GlobalExperience {
    pub applicable_task_types: Vec<String>,
    pub success_patterns: Vec<SuccessPattern>,
    pub failure_patterns: Vec<FailurePattern>,
    pub effective_strategies: Vec<StrategyRecord>,
    pub distilled_at: DateTime<Utc>,
    pub confidence: f32,
    pub source_case_count: usize,
}

#[derive(Clone, Debug)]
pub struct SuccessPattern {
    pub method_family: String,
    pub score_range: (f32, f32),
    pub key_factors: Vec<String>,
    pub avg_token_usage: u32,
}

#[derive(Clone, Debug)]
pub struct FailurePattern {
    pub error_signature: String,
    pub error_type: String,
    pub fix_strategy: String,
    pub frequency: u32,
    pub avg_fix_time_ms: u64,
}

#[derive(Clone, Debug)]
pub struct StrategyRecord {
    pub dimension: String,
    pub strategy_value: String,
    pub avg_improvement: f32,
    pub sample_count: u32,
}

impl DualExperienceBank {
    pub fn new(distill_threshold: usize) -> Self {
        Self { case_bank: Vec::new(), global_bank: Vec::new(), distill_threshold, last_distillation: Utc::now(), task_type_index: HashMap::new() }
    }

    pub fn add_case(&mut self, case: CaseExperience) {
        let idx = self.case_bank.len();
        self.task_type_index.entry(case.task_type.clone()).or_default().push(idx);
        self.case_bank.push(case);
        let undistilled_count = self.case_bank.iter().filter(|c| !c.distilled).count();
        if undistilled_count >= self.distill_threshold { self.distill_global(); }
    }

    fn distill_global(&mut self) {
        let undistilled: Vec<usize> = self.case_bank.iter().enumerate().filter(|(_, c)| !c.distilled).map(|(i, _)| i).collect();
        let mut by_task_type: HashMap<String, Vec<usize>> = HashMap::new();
        for idx in &undistilled { let task_type = self.case_bank[*idx].task_type.clone(); by_task_type.entry(task_type).or_default().push(*idx); }
        for (task_type, indices) in by_task_type {
            let cases: Vec<_> = indices.iter().map(|&i| &self.case_bank[i]).collect();
            let global = GlobalExperience {
                applicable_task_types: vec![task_type],
                success_patterns: self.extract_success_patterns(&cases),
                failure_patterns: self.extract_failure_patterns(&cases),
                effective_strategies: self.extract_strategies(&cases),
                distilled_at: Utc::now(),
                confidence: (cases.len() as f32 / self.distill_threshold as f32).min(1.0),
                source_case_count: cases.len(),
            };
            self.global_bank.push(global);
        }
        for idx in undistilled { self.case_bank[idx].distilled = true; }
        self.last_distillation = Utc::now();
    }

    fn extract_success_patterns(&self, cases: &[&CaseExperience]) -> Vec<SuccessPattern> {
        let success_cases: Vec<_> = cases.iter().filter(|c| c.card.execution_status == ExecutionStatus::Success && c.card.score > 0.7).collect();
        let mut by_method: HashMap<String, Vec<&CaseExperience>> = HashMap::new();
        for case in success_cases { by_method.entry(case.card.method_family.clone()).or_default().push(case); }
        by_method.into_iter().map(|(method, cases)| {
            let scores: Vec<f32> = cases.iter().map(|c| c.card.score).collect();
            let avg_score = scores.iter().sum::<f32>() / scores.len() as f32;
            let min_score = scores.iter().fold(f32::MAX, |a, &b| a.min(b));
            let max_score = scores.iter().fold(f32::MIN, |a, &b| a.max(b));
            let avg_tokens = cases.iter().map(|c| c.card.metadata.token_usage.total_tokens).sum::<u32>() / cases.len() as u32;
            SuccessPattern { method_family: method, score_range: (min_score, max_score), key_factors: vec![format!("avg_score={:.2}", avg_score)], avg_token_usage: avg_tokens }
        }).collect()
    }

    fn extract_failure_patterns(&self, cases: &[&CaseExperience]) -> Vec<FailurePattern> {
        let failure_cases: Vec<_> = cases.iter().filter(|c| c.card.error_signature.is_some()).collect();
        let mut by_error: HashMap<String, Vec<&CaseExperience>> = HashMap::new();
        for case in failure_cases { if let Some(ref sig) = case.card.error_signature { by_error.entry(sig.error_hash.clone()).or_default().push(case); } }
        by_error.into_iter().map(|(hash, cases)| {
            let first = cases.first().unwrap();
            let avg_time = cases.iter().map(|c| c.card.metadata.execution_time_ms).sum::<u64>() / cases.len() as u64;
            FailurePattern { error_signature: hash, error_type: first.card.error_signature.as_ref().unwrap().error_type.clone(), fix_strategy: "Apply known fix from similar cases".to_string(), frequency: cases.len() as u32, avg_fix_time_ms: avg_time }
        }).collect()
    }

    fn extract_strategies(&self, cases: &[&CaseExperience]) -> Vec<StrategyRecord> {
        let mut by_operator: HashMap<String, Vec<f32>> = HashMap::new();
        for case in cases.iter().filter(|c| c.card.execution_status == ExecutionStatus::Success) {
            by_operator.entry(format!("{:?}", case.card.operator)).or_default().push(case.card.score);
        }
        by_operator.into_iter().map(|(op, scores)| {
            let avg = scores.iter().sum::<f32>() / scores.len() as f32;
            StrategyRecord { dimension: "operator".to_string(), strategy_value: op, avg_improvement: avg, sample_count: scores.len() as u32 }
        }).collect()
    }

    pub fn retrieve(&self, query: &TaskQuery) -> RetrievedExperiences {
        let global = self.global_bank.iter().filter(|g| g.applicable_task_types.contains(&query.task_type)).collect();
        let cases = self.retrieve_similar_cases(query);
        RetrievedExperiences { global, cases }
    }

    fn retrieve_similar_cases(&self, query: &TaskQuery) -> Vec<&CaseExperience> {
        if let Some(indices) = self.task_type_index.get(&query.task_type) {
            indices.iter().filter_map(|&i| self.case_bank.get(i)).filter(|c| c.card.score >= query.min_score).take(query.max_results).collect()
        } else { vec![] }
    }
}

#[derive(Clone, Debug)]
pub struct TaskQuery { pub task_type: String, pub min_score: f32, pub max_results: usize }

#[derive(Clone, Debug)]
pub struct RetrievedExperiences<'a> { pub global: Vec<&'a GlobalExperience>, pub cases: Vec<&'a CaseExperience> }
```

### 7.3 记忆金字塔融合（MSCE + TencentDB + Chimera MLC四级）

```rust
// crates/mlc-engine/src/pyramid.rs
#![forbid(unsafe_code)]
use nexus_contracts::{AtomicMemoryCard, SceneBlock, PersonaSummary, MemoryPyramidLevel};

pub struct MemoryPyramid {
    l0_raw_logs: Vec<RawLogEntry>,
    l1_atomic_cards: Vec<AtomicMemoryCard>,
    l2_scene_blocks: Vec<SceneBlock>,
    l3_personas: Vec<PersonaSummary>,
    literal_searcher: LiteralSearcher,
    semantic_searcher: SemanticSearcher,
    hybrid_ranker: HybridRanker,
    degradation_chain: DegradationChain,
}

impl MemoryPyramid {
    /// 写入原始对话（TencentDB L0: 毫秒级落盘）
    pub async fn write_raw_log(&mut self, user_msg: &str, assistant_msg: &str) -> String {
        let entry = RawLogEntry {
            id: Uuid::new_v4().to_string(),
            user_message: user_msg.to_string(),
            assistant_message: assistant_msg.to_string(),
            timestamp: now(),
        };
        self.l0_raw_logs.push(entry.clone());
        entry.id
    }

    /// 异步提炼（TencentDB: 后台模型调用，约6秒；融合MSCE L1 Trace）
    pub async fn distill_atomic_cards(&mut self, session_id: &str) -> Vec<AtomicMemoryCard> {
        let logs: Vec<&RawLogEntry> = self.l0_raw_logs.iter().filter(|l| l.session_id == session_id).collect();
        let cards = self.model_distill(&logs).await;
        let filtered = self.deduplicate_cards(cards).await;
        self.l1_atomic_cards.extend(filtered.clone());
        filtered
    }

    /// 检索三方式融合（TencentDB）
    pub async fn retrieve(&self, query: &str, session_id: &str, timeout_ms: u32) -> Vec<RetrievalResult> {
        let start = Instant::now();
        let literal_results = self.literal_searcher.search(query);
        let semantic_results = if self.degradation_chain.is_semantic_available() {
            self.semantic_searcher.search(query).await.ok()
        } else { None };
        let hybrid_results = match semantic_results {
            Some(semantic) => self.hybrid_ranker.rank(&literal_results, &semantic),
            None => literal_results.into_iter().map(|r| RetrievalResult::from_literal(r)).collect(),
        };
        if start.elapsed().as_millis() > timeout_ms as u128 { return vec![]; }
        hybrid_results
    }

    /// 注入策略（TencentDB优化 + HCW分层上下文窗口）
    pub fn inject_context(&self, query: &str, retrieved: &[RetrievalResult],
                          user_message: &mut String, system_prompt: &mut String) {
        let dynamic_cards: Vec<&RetrievalResult> = retrieved.iter()
            .filter(|r| r.card_type != AtomicCardType::Preference)
            .take(3).collect();
        if !dynamic_cards.is_empty() {
            let injection = dynamic_cards.iter()
                .map(|r| format!("[记忆] {}: {}", r.scene, r.content))
                .collect::<Vec<_>>().join("
");
            *user_message = format!("{}

{}", injection, user_message);
        }
        let persona_cards: Vec<&RetrievalResult> = retrieved.iter()
            .filter(|r| r.card_type == AtomicCardType::Preference).collect();
        if !persona_cards.is_empty() {
            let persona_injection = persona_cards.iter().map(|r| r.content.clone()).collect::<Vec<_>>().join("; ");
            *system_prompt = format!("{}

[用户画像] {}", system_prompt, persona_injection);
        }
    }
}

/// 降级链（TencentDB故障容忍 → Chimera L4 csn-substitutor扩展）
pub struct DegradationChain {
    semantic_available: bool,
    keyword_available: bool,
}

impl DegradationChain {
    pub fn retrieve_strategy(&self) -> RetrieveStrategy {
        match (self.semantic_available, self.keyword_available) {
            (true, true) => RetrieveStrategy::Hybrid,
            (true, false) => RetrieveStrategy::SemanticOnly,
            (false, true) => RetrieveStrategy::KeywordOnly,
            (false, false) => RetrieveStrategy::Empty,
        }
    }
}
```

### 7.4 HiLS-Attention集成（新增Crate）

```rust
// crates/hils-attention/src/lib.rs
#![forbid(unsafe_code)]
use nexus_core::CLV;

/// HiLS-Attention: 分层稀疏注意力（腾讯混元 arXiv 2607.02980）
/// 与Chimera映射: 替代/增强 `hcw-window` 的窗口选择器
pub struct HiLSAttention {
    chunk_size: usize,            // 默认128 tokens
    num_landmarks: usize,
    top_k_chunks: usize,
    sliding_window_size: usize,
    use_hope: bool,               // HoPE位置编码
    q_cal_adapter: QCalAdapter, // 低秩查询校准（0.6%额外参数）
    gqa_compatible: bool,
    m_query_pack: usize,          // 打包M个query token（默认16）
}

impl HiLSAttention {
    /// chunk-mass surrogate = 相关性项 * 熵偏置项
    pub fn compute_chunk_importance(&self, query: &CLV, chunk: &Chunk) -> f32 {
        let landmark_key = chunk.get_landmark_key();
        let relevance = query.cosine_similarity(&landmark_key).exp();
        let entropy = chunk.compute_attention_entropy(query);
        let chunk_mass = relevance * (1.0 + entropy);
        chunk_mass
    }

    /// 两级softmax: 块间选择Top-K → 块内计算token权重
    pub fn forward(&self, query: &CLV, chunks: &[Chunk]) -> AttentionOutput {
        let mut chunk_scores: Vec<(usize, f32)> = chunks.iter().enumerate()
            .map(|(i, c)| (i, self.compute_chunk_importance(query, c)))
            .collect();
        chunk_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let selected_chunks: Vec<&Chunk> = chunk_scores.iter().take(self.top_k_chunks)
            .map(|(i, _)| &chunks[*i]).collect();

        let mut output = AttentionOutput::new();
        for chunk in &selected_chunks {
            let intra_chunk_weights = chunk.compute_intra_attention(query);
            output.add_chunk(chunk, intra_chunk_weights);
        }
        let local_window = self.get_local_window(query);
        output.add_local_window(local_window);
        output
    }

    /// 高效Kernel: 打包M个query token，Tensor Core利用率更高
    pub fn forward_batched(&self, queries: &[CLV], chunks: &[Chunk]) -> Vec<AttentionOutput> {
        let mut outputs = vec![];
        for query_batch in queries.chunks(self.m_query_pack) {
            let union_chunks = self.compute_union_chunks(query_batch, chunks);
            for query in query_batch {
                outputs.push(self.forward_single(query, &union_chunks));
            }
        }
        outputs
    }
}

/// 与Chimera HCW的集成接口
impl HierarchicalWindow {
    pub fn with_hils_attention(mut self, hils: HiLSAttention) -> Self {
        self.window_selector = Box::new(HiLSWindowSelector::new(hils));
        self
    }
}
```

### 7.5 MSCE双信号价值回填

```rust
// crates/msce-integration/src/value_backfill.rs
#![forbid(unsafe_code)]

/// MSCE双信号价值回填: Vt = αt * Rt + (1-αt) * γ * Vt+1
pub struct DualSignalBackfill {
    gamma: f32,
    reflection_scorer: ReflectionScorer,
}

impl DualSignalBackfill {
    pub fn backfill_values(&self, traces: &mut [L1Trace]) {
        let mut next_value = 0.0;
        for trace in traces.iter_mut().rev() {
            let alpha = self.reflection_scorer.score(&trace.reflection);
            let rt = trace.environmental_feedback.unwrap_or(0.0);
            let vt = alpha * rt + (1.0 - alpha) * self.gamma * next_value;
            trace.value = Some(vt);
            next_value = vt;
        }
    }
}

pub struct ReflectionScorer {
    model: ScoringModel,
}

impl ReflectionScorer {
    /// α≈1: 反思非常可靠，主要依赖环境反馈
    /// α≈0: 反思不可靠，主要依赖后续价值传播
    pub fn score(&self, reflection: &str) -> f32 {
        let features = self.extract_features(reflection);
        let raw_score = self.model.predict(&features);
        raw_score.clamp(0.0, 1.0)
    }
}
```

---


## 8. L3 Storage：存储层——金字塔存储 + 经验卡片持久化 + 三因子索引 + 分层采样

### 8.1 金字塔存储映射（TencentDB四层 → CMT-tiering热/温/冷/冰）

```rust
// cmt-tiering/src/pyramid_storage.rs
#![forbid(unsafe_code)]
use nexus_contracts::{MemoryPyramidLevel, AtomicMemoryCard, SceneBlock, PersonaSummary};

pub struct PyramidStorageMapper {
    hot_tier: HotTier,      // L3 Persona (每轮注入)
    warm_tier: WarmTier,    // L2 Scene Block (场景检索)
    cold_tier: ColdTier,    // L1 Atomic Memory (规则/偏好)
    ice_tier: IceTier,      // L0 Raw Log (审计/追溯)
}

impl PyramidStorageMapper {
    pub fn store_pyramid_level(&mut self, level: MemoryPyramidLevel, data: &[u8]) {
        match level {
            MemoryPyramidLevel::L0RawLog => {
                self.ice_tier.store(data, StoragePriority::Archive);
            }
            MemoryPyramidLevel::L1AtomicMemory => {
                self.cold_tier.store(data, StoragePriority::HighValue);
            }
            MemoryPyramidLevel::L2SceneBlock => {
                self.warm_tier.store(data, StoragePriority::MediumValue);
            }
            MemoryPyramidLevel::L3Persona => {
                self.hot_tier.store(data, StoragePriority::Critical);
            }
        }
    }

    /// 分层采样比例（TencentDB + Dressage经验）
    pub fn sample_pyramid(&self, batch_size: usize) -> Vec<StorageEntry> {
        let hot_samples = self.hot_tier.sample(batch_size / 4);      // 25% Hot
        let warm_samples = self.warm_tier.sample(batch_size / 4);    // 25% Warm
        let cold_samples = self.cold_tier.sample(batch_size / 2);    // 50% Cold (高权重)
        let ice_samples = self.ice_tier.sample(0);                   // 0% Ice (仅离线)
        [hot_samples, warm_samples, cold_samples, ice_samples].concat()
    }
}
```

### 8.2 经验卡片持久化（OpenMLE + SQLite复合索引）

```rust
// crates/experience-card-storage/src/lib.rs
#![forbid(unsafe_code)]
use nexus_contracts::{ExperienceCard, ThreeFactorScore, ErrorSignature, ExecutionStatus};
use rusqlite::{Connection, params};

pub struct ExperienceCardStorage {
    conn: Arc<Mutex<Connection>>,
    hot_cache: dashmap::DashMap<String, ExperienceCard>,
    hot_capacity: usize,
}

impl ExperienceCardStorage {
    pub async fn new(db_path: &str, hot_capacity: usize) -> Result<Self, StorageError> {
        let conn = Connection::open(db_path)?;
        let storage = Self { conn: Arc::new(Mutex::new(conn)), hot_cache: dashmap::DashMap::new(), hot_capacity };
        storage.init().await?; Ok(storage)
    }

    pub async fn init(&self) -> Result<(), StorageError> {
        let conn = self.conn.lock().await;
        conn.execute("CREATE TABLE IF NOT EXISTS experience_cards (
            card_id TEXT PRIMARY KEY, task_id TEXT NOT NULL, node_id TEXT NOT NULL UNIQUE,
            parent_id TEXT, operator TEXT NOT NULL, score REAL NOT NULL,
            delta_vs_parent REAL NOT NULL, method_family TEXT NOT NULL,
            error_hash TEXT, error_type TEXT,
            quality REAL NOT NULL, progress REAL NOT NULL, novelty REAL NOT NULL,
            execution_status TEXT NOT NULL, created_at TEXT NOT NULL,
            execution_time_ms INTEGER, prompt_tokens INTEGER, completion_tokens INTEGER,
            total_tokens INTEGER, lines_changed INTEGER, skills_used TEXT, metadata BLOB
        )", [])?;
        // 五维复合索引（OpenMLE三因子 + 错误签名 + 方法家族）
        conn.execute("CREATE INDEX IF NOT EXISTS idx_three_factor ON experience_cards (task_id, quality DESC, progress DESC, novelty DESC)", [])?;
        conn.execute("CREATE INDEX IF NOT EXISTS idx_error_hash ON experience_cards (error_hash)", [])?;
        conn.execute("CREATE INDEX IF NOT EXISTS idx_method_family ON experience_cards (method_family, score DESC)", [])?;
        conn.execute("CREATE INDEX IF NOT EXISTS idx_created_at ON experience_cards (created_at DESC)", [])?;
        conn.execute("CREATE INDEX IF NOT EXISTS idx_task_status ON experience_cards (task_id, execution_status)", [])?;
        Ok(())
    }

    pub async fn store(&self, card: &ExperienceCard) -> Result<(), StorageError> {
        if card.score > 0.7 || self.hot_cache.len() < self.hot_capacity {
            self.hot_cache.insert(card.card_id.clone(), card.clone());
            if self.hot_cache.len() > self.hot_capacity {
                let to_evict = self.hot_cache.iter().min_by(|a, b| {
                    a.value().score.partial_cmp(&b.value().score).unwrap_or(std::cmp::Ordering::Equal)
                }).map(|e| e.key().clone());
                if let Some(id) = to_evict { self.hot_cache.remove(&id); }
            }
        }
        let conn = self.conn.lock().await;
        conn.execute("INSERT INTO experience_cards (...) VALUES (...) ON CONFLICT(card_id) DO UPDATE SET ...",
            params![/* 全部字段 */])?;
        Ok(())
    }

    pub async fn query_by_three_factor(&self, task_id: &str, min_quality: f32, k: usize) -> Result<Vec<ExperienceCard>, StorageError> {
        let hot_results: Vec<_> = self.hot_cache.iter()
            .filter(|e| e.value().task_id == task_id && e.value().three_factor.quality >= min_quality)
            .map(|e| e.value().clone()).collect();
        if hot_results.len() >= k {
            let mut results = hot_results;
            results.sort_by(|a, b| {
                let score_a = a.three_factor.selection_utility();
                let score_b = b.three_factor.selection_utility();
                score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal)
            });
            return Ok(results.into_iter().take(k).collect());
        }
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare("SELECT * FROM experience_cards WHERE task_id = ?1 AND quality >= ?2 ORDER BY (quality + progress + novelty) DESC LIMIT ?3")?;
        let cards = stmt.query_map(params![task_id, min_quality, k as i64], |row| self.row_to_card(row))?;
        cards.collect::<Result<Vec<_>, _>>().map_err(|e| StorageError::QueryError(e.to_string()))
    }

    pub async fn query_by_error_signature(&self, error_hash: &str, limit: usize) -> Result<Vec<ExperienceCard>, StorageError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare("SELECT * FROM experience_cards WHERE error_hash = ?1 ORDER BY score DESC, created_at DESC LIMIT ?2")?;
        let cards = stmt.query_map(params![error_hash, limit as i64], |row| self.row_to_card(row))?;
        cards.collect::<Result<Vec<_>, _>>().map_err(|e| StorageError::QueryError(e.to_string()))
    }
}
```

---

## 9. L4 Security：安全层——Paddock-Sandbox解耦 + AutoBuilder + 错误签名 + 六类状态反馈

### 9.1 Paddock-Sandbox解耦（Dressage核心）

Paddock负责what-to-do（初始化、调用、暂停/恢复、清理），SandboxProvider负责where-it-runs（本地bubblewrap、远程E2B、自定义）。Agent逻辑不需要知道sandbox内部实现，sandbox不需要理解Agent思考方式。

```rust
// seccore/src/paddock_sandbox.rs
#![forbid(unsafe_code)]

pub struct Paddock {
    rollout_manager: RolloutManager,
    agent_adapter: AgentAdapter,
}

pub struct SandboxProvider {
    sandbox_type: SandboxType,
    runtime: Box<dyn SandboxRuntime>,
}

#[derive(Clone, Debug)]
pub enum SandboxType {
    LocalBubblewrap,
    RemoteE2B,
    Custom,
}

impl Paddock {
    pub async fn initialize_rollout(&self, task: &TaskSpec) -> RolloutContext {
        let session = self.agent_adapter.create_session(task).await;
        let sandbox_config = self.prepare_sandbox_config(task);
        RolloutContext { session, sandbox_config }
    }

    pub async fn execute_step(&self, ctx: &mut RolloutContext, action: &Action) -> StepResult {
        let agent_output = self.agent_adapter.execute(ctx, action).await;
        let verification = ctx.sandbox.verify(&agent_output).await;
        StepResult { agent_output, verification }
    }

    pub async fn pause_rollout(&self, ctx: &RolloutContext) -> Checkpoint {
        let agent_checkpoint = self.agent_adapter.pause(&ctx.session).await;
        let sandbox_checkpoint = ctx.sandbox.pause().await;
        Checkpoint { agent_checkpoint, sandbox_checkpoint }
    }

    pub async fn resume_rollout(&self, checkpoint: &Checkpoint) -> RolloutContext {
        let session = self.agent_adapter.resume(&checkpoint.agent_checkpoint).await;
        let sandbox = self.sandbox_provider.resume(&checkpoint.sandbox_checkpoint).await;
        RolloutContext { session, sandbox }
    }

    pub async fn finalize_rollout(&self, ctx: &RolloutContext) -> Trajectory {
        let token_evidence = self.proxy.collect_evidence(&ctx.session).await;
        ctx.sandbox.cleanup().await;
        Trajectory { token_evidence, segments: self.segmentize(token_evidence) }
    }
}
```

### 9.2 AutoBuilder（快手KAT融合）

83.5%的仓库无法直接运行。双智能体协同构建可运行环境。

```rust
// crates/auto-builder/src/lib.rs
#![forbid(unsafe_code)]

pub struct AutoBuilder {
    build_agent: BuildAgent,
    verify_agent: VerifyAgent,
}

impl AutoBuilder {
    pub async fn build(&self, repo: &Repo) -> BuildResult {
        let analysis = self.build_agent.analyze(repo).await;
        let script = self.build_agent.generate_script(&analysis);
        let verification = self.verify_agent.verify(&script).await;
        if !verification.success {
            let fixed = self.build_agent.fix(&script, &verification.errors).await;
            return self.build(&repo.with_script(&fixed)).await;
        }
        BuildResult { script, test_results: verification.results, success_rate: verification.pass_rate }
    }
}
```

### 9.3 错误签名收集器（OpenMLE + 结构化）

```rust
// crates/error-signature-collector/src/lib.rs
#![forbid(unsafe_code)]
use nexus_contracts::{ErrorSignature, ExecutionStatus};
use regex::Regex;

pub struct ErrorSignatureCollector {
    known_patterns: Vec<(Regex, String)>,
    signature_frequency: HashMap<String, u32>,
    error_type_frequency: HashMap<String, u32>,
}

impl ErrorSignatureCollector {
    pub fn new() -> Self {
        let mut patterns = vec![];
        patterns.push((Regex::new(r"error\[(?P<type>E\d+)\]:\s*(?P<summary>.+)").unwrap(), "CompilationError".to_string()));
        patterns.push((Regex::new(r"thread '\w+' panicked at (?P<location>.+?),\s*(?P<summary>.+)").unwrap(), "RuntimePanic".to_string()));
        patterns.push((Regex::new(r"assertion failed:\s*(?P<summary>.+)").unwrap(), "AssertionFailure".to_string()));
        patterns.push((Regex::new(r"test result: FAILED\.\s*(?P<summary>\d+ failed, \d+ passed)").unwrap(), "TestFailure".to_string()));
        patterns.push((Regex::new(r"timeout after (?P<duration>\d+ms)").unwrap(), "Timeout".to_string()));
        Self { known_patterns: patterns, signature_frequency: HashMap::new(), error_type_frequency: HashMap::new() }
    }

    pub fn extract(&mut self, output: &str, location: &str) -> Option<ErrorSignature> {
        for (pattern, error_type) in &self.known_patterns {
            if let Some(captures) = pattern.captures(output) {
                let summary = captures.name("summary").map(|m| m.as_str().to_string())
                    .unwrap_or_else(|| output.lines().next().unwrap_or("").to_string());
                let hash = ErrorSignature::compute_hash(error_type, &summary);
                self.update_frequency(&hash, error_type);
                return Some(ErrorSignature { error_type: error_type.clone(), error_location: location.to_string(), error_summary: summary.chars().take(100).collect(), error_hash: hash });
            }
        }
        // 通用回退
        let keywords = [("error", "GenericError"), ("Error", "GenericError"), ("ERROR", "GenericError"), ("failed", "GenericFailure"), ("Failed", "GenericFailure"), ("panic", "GenericPanic"), (" Panic", "GenericPanic")];
        for (keyword, error_type) in &keywords {
            if output.contains(keyword) {
                let first_line = output.lines().next().unwrap_or("");
                let hash = ErrorSignature::compute_hash(error_type, first_line);
                self.update_frequency(&hash, error_type);
                return Some(ErrorSignature { error_type: error_type.to_string(), error_location: location.to_string(), error_summary: first_line.chars().take(100).collect(), error_hash: hash });
            }
        }
        None
    }

    fn update_frequency(&mut self, hash: &str, error_type: &str) {
        *self.signature_frequency.entry(hash.to_string()).or_insert(0) += 1;
        *self.error_type_frequency.entry(error_type.to_string()).or_insert(0) += 1;
    }

    pub fn get_frequent_signatures(&self, threshold: u32) -> Vec<(String, String, u32)> {
        self.signature_frequency.iter().filter(|(_, count)| **count >= threshold)
            .map(|(hash, count)| {
                let error_type = self.error_type_frequency.iter().find(|(_, _)| true).map(|(t, _)| t.clone()).unwrap_or_default();
                (hash.clone(), error_type, *count)
            }).collect()
    }
}

/// 六类状态反馈全链路追踪（OpenMLE）
pub struct ExecutionFeedbackIntegrator;
impl ExecutionFeedbackIntegrator {
    pub fn classify(success: bool, has_output: bool, has_submission: bool, score: Option<f32>, timed_out: bool, error_output: Option<&str>) -> ExecutionStatus {
        if timed_out { return ExecutionStatus::Timeout; }
        if !success { if error_output.is_some() { return ExecutionStatus::Error; } return ExecutionStatus::ScoreFailed; }
        if !has_submission { return ExecutionStatus::NoSubmit; }
        if !has_output { return ExecutionStatus::MissingCode; }
        if score.is_none() { return ExecutionStatus::ScoreFailed; }
        ExecutionStatus::Success
    }
}
```

---

## 10. L5 Knowledge：知识层——四套原子算子 + 三因子父本选择 + AEGIS-GSOE + 变体隔离 + 双层经验库 + Skill生命周期

### 10.1 四套原子算子（OpenMLE核心）

Draft/Improve/Debug/Crossover贯穿SFT/RL/推理全生命周期。

```rust
// crates/four-operators/src/lib.rs
#![forbid(unsafe_code)]
use nexus_contracts::{AtomicOperator, ExperienceCard, ErrorSignature, ExecutionStatus};
use async_trait::async_trait;

#[async_trait]
pub trait AtomicOperatorTrait: Send + Sync {
    fn operator_type(&self) -> AtomicOperator;
    async fn execute(&self, context: &OperatorContext) -> Result<OperatorResult, OperatorError>;
    fn estimate_cost(&self, context: &OperatorContext) -> ResourceCost;
    fn is_applicable(&self, context: &OperatorContext) -> bool;
}

pub struct OperatorContext {
    pub task_id: String,
    pub task_type: String,
    pub parent_card: Option<ExperienceCard>,
    pub error_signature: Option<ErrorSignature>,
    pub requirements: String,
    pub code: Option<String>,
    pub global_bank: Option<crate::dual_experience_bank::DualExperienceBank>,
    pub memory_synthesizer: Option<crate::on_demand_synthesizer::OnDemandSynthesizer>,
    pub storage: Option<crate::experience_card_storage::ExperienceCardStorage>,
}

pub struct OperatorResult {
    pub code: String,
    pub score: f32,
    pub operator: AtomicOperator,
    pub execution_status: ExecutionStatus,
    pub error_signature: Option<ErrorSignature>,
    pub metadata: nexus_contracts::CardMetadata,
}

pub struct ResourceCost { pub estimated_tokens: usize, pub estimated_time_ms: u64 }

#[derive(Debug, thiserror::Error)]
pub enum OperatorError {
    #[error("No parent card")] NoParent,
    #[error("No error signature")] NoErrorSignature,
    #[error("Insufficient candidates")] InsufficientCandidates,
    #[error("Execution failed: {0}")] ExecutionFailed(String),
}

pub struct DraftOperator;
#[async_trait]
impl AtomicOperatorTrait for DraftOperator {
    fn operator_type(&self) -> AtomicOperator { AtomicOperator::Draft }
    async fn execute(&self, context: &OperatorContext) -> Result<OperatorResult, OperatorError> {
        let global_patterns = if let Some(ref bank) = context.global_bank {
            bank.retrieve(&crate::dual_experience_bank::TaskQuery { task_type: context.task_type.clone(), min_score: 0.7, max_results: 3 })
        } else { crate::dual_experience_bank::RetrievedExperiences { global: vec![], cases: vec![] } };
        let code = format!("// Draft for: {}
fn main() {{}}", context.requirements);
        Ok(OperatorResult { code, score: 0.5, operator: AtomicOperator::Draft, execution_status: ExecutionStatus::Success, error_signature: None, metadata: nexus_contracts::CardMetadata { execution_time_ms: 30000, token_usage: nexus_contracts::TokenUsage { prompt_tokens: 2000, completion_tokens: 3000, total_tokens: 5000 }, lines_changed: 10, skills_used: vec![], environment: nexus_contracts::EnvironmentInfo::default() } })
    }
    fn estimate_cost(&self, _context: &OperatorContext) -> ResourceCost { ResourceCost { estimated_tokens: 5000, estimated_time_ms: 30000 } }
    fn is_applicable(&self, _context: &OperatorContext) -> bool { true }
}

pub struct ImproveOperator;
#[async_trait]
impl AtomicOperatorTrait for ImproveOperator {
    fn operator_type(&self) -> AtomicOperator { AtomicOperator::Improve }
    async fn execute(&self, context: &OperatorContext) -> Result<OperatorResult, OperatorError> {
        let parent = context.parent_card.as_ref().ok_or(OperatorError::NoParent)?;
        let improved_code = format!("{}
// Improved", parent.code);
        Ok(OperatorResult { code: improved_code, score: (parent.score + 0.1).min(1.0), operator: AtomicOperator::Improve, execution_status: ExecutionStatus::Success, error_signature: None, metadata: nexus_contracts::CardMetadata { execution_time_ms: 20000, token_usage: nexus_contracts::TokenUsage { prompt_tokens: 1500, completion_tokens: 1500, total_tokens: 3000 }, lines_changed: 5, skills_used: vec![], environment: nexus_contracts::EnvironmentInfo::default() } })
    }
    fn estimate_cost(&self, _context: &OperatorContext) -> ResourceCost { ResourceCost { estimated_tokens: 3000, estimated_time_ms: 20000 } }
    fn is_applicable(&self, context: &OperatorContext) -> bool { context.parent_card.is_some() }
}

pub struct DebugOperator;
#[async_trait]
impl AtomicOperatorTrait for DebugOperator {
    fn operator_type(&self) -> AtomicOperator { AtomicOperator::Debug }
    async fn execute(&self, context: &OperatorContext) -> Result<OperatorResult, OperatorError> {
        let parent = context.parent_card.as_ref().ok_or(OperatorError::NoParent)?;
        let error = parent.error_signature.as_ref().ok_or(OperatorError::NoErrorSignature)?;
        let similar_fixes = if let Some(ref storage) = context.storage {
            storage.query_by_error_signature(&error.error_hash, 5).await.unwrap_or_default()
        } else { vec![] };
        let best_fix = similar_fixes.iter().filter(|c| c.execution_status == ExecutionStatus::Success)
            .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));
        let fixed_code = if let Some(fix) = best_fix {
            format!("{}
// Fixed using {} (score: {:.2})", parent.code, fix.card_id, fix.score)
        } else { format!("{}
// Generic fix for: {}", parent.code, error.error_type) };
        Ok(OperatorResult { code: fixed_code, score: parent.score, operator: AtomicOperator::Debug, execution_status: ExecutionStatus::Success, error_signature: None, metadata: nexus_contracts::CardMetadata { execution_time_ms: 15000, token_usage: nexus_contracts::TokenUsage { prompt_tokens: 1000, completion_tokens: 1000, total_tokens: 2000 }, lines_changed: 2, skills_used: vec![], environment: nexus_contracts::EnvironmentInfo::default() } })
    }
    fn estimate_cost(&self, _context: &OperatorContext) -> ResourceCost { ResourceCost { estimated_tokens: 2000, estimated_time_ms: 15000 } }
    fn is_applicable(&self, context: &OperatorContext) -> bool { context.parent_card.is_some() && context.parent_card.as_ref().unwrap().error_signature.is_some() }
}

pub struct CrossoverOperator;
#[async_trait]
impl AtomicOperatorTrait for CrossoverOperator {
    fn operator_type(&self) -> AtomicOperator { AtomicOperator::Crossover }
    async fn execute(&self, context: &OperatorContext) -> Result<OperatorResult, OperatorError> {
        let candidates = if let Some(ref storage) = context.storage {
            storage.query_by_three_factor(&context.task_id, 0.7, 10).await.unwrap_or_default()
        } else { vec![] };
        if candidates.len() < 2 { return Err(OperatorError::InsufficientCandidates); }
        let mut sorted = candidates; sorted.sort_by(|a, b| b.three_factor.novelty.partial_cmp(&a.three_factor.novelty).unwrap_or(std::cmp::Ordering::Equal));
        let merged_code = format!("// Crossover of {} and {}
{}", sorted[0].card_id, sorted[1].card_id, sorted[0].code);
        let score = (sorted[0].score + sorted[1].score) / 2.0;
        Ok(OperatorResult { code: merged_code, score, operator: AtomicOperator::Crossover, execution_status: ExecutionStatus::Success, error_signature: None, metadata: nexus_contracts::CardMetadata { execution_time_ms: 25000, token_usage: nexus_contracts::TokenUsage { prompt_tokens: 2000, completion_tokens: 2000, total_tokens: 4000 }, lines_changed: 8, skills_used: vec![], environment: nexus_contracts::EnvironmentInfo::default() } })
    }
    fn estimate_cost(&self, _context: &OperatorContext) -> ResourceCost { ResourceCost { estimated_tokens: 4000, estimated_time_ms: 25000 } }
    fn is_applicable(&self, _context: &OperatorContext) -> bool { true }
}
```

### 10.2 三因子父本选择器（OpenMLE核心算法）

Quality + Progress + Novelty，UCB + Softmax + 冷却系数，避免只按分数采样丢失潜力分支。

```rust
// crates/three-factor-selector/src/lib.rs
#![forbid(unsafe_code)]
use nexus_contracts::{ExperienceCard, ThreeFactorScore, NormalizedThreeFactor};

pub struct ThreeFactorSelector {
    exploration_weight: f32,
    cooling_coefficient: f32,
    visit_counts: HashMap<String, u32>,
    total_visits: u32,
    temperature: f32,
}

impl ThreeFactorSelector {
    pub fn new(exploration_weight: f32, cooling_coefficient: f32, temperature: f32) -> Self {
        Self { exploration_weight, cooling_coefficient, visit_counts: HashMap::new(), total_visits: 0, temperature: temperature.max(0.1) }
    }

    pub fn select(&mut self, candidates: &[ExperienceCard]) -> Option<ExperienceCard> {
        if candidates.is_empty() { return None; }
        let max_quality = candidates.iter().map(|c| c.three_factor.quality).fold(0.0, f32::max).max(1e-8);
        let max_progress = candidates.iter().map(|c| c.three_factor.progress.abs()).fold(0.0, f32::max).max(1e-8);
        let max_novelty = candidates.iter().map(|c| c.three_factor.novelty).fold(0.0, f32::max).max(1e-8);

        let mut scored: Vec<(ExperienceCard, f32)> = candidates.iter().map(|c| {
            let normalized = c.three_factor.normalize(max_quality, max_progress, max_novelty);
            let ucb_bonus = self.ucb_bonus(&c.node_id);
            let cooling = self.cooling_factor();
            let utility = normalized.quality + normalized.progress + normalized.novelty
                + ucb_bonus * self.exploration_weight - cooling;
            (c.clone(), utility)
        }).collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let selected = self.softmax_sample(&scored)?;
        *self.visit_counts.entry(selected.node_id.clone()).or_insert(0) += 1;
        self.total_visits += 1;
        Some(selected)
    }

    fn ucb_bonus(&self, node_id: &str) -> f32 {
        let visits = self.visit_counts.get(node_id).copied().unwrap_or(0);
        if visits == 0 { return f32::MAX; }
        if self.total_visits == 0 { return 0.0; }
        (2.0 * (self.total_visits as f32).ln() / visits as f32).sqrt()
    }

    fn cooling_factor(&self) -> f32 {
        if self.total_visits == 0 { return 0.0; }
        self.cooling_coefficient * (self.total_visits as f32).ln().max(0.0)
    }

    fn softmax_sample(&self, scored: &[(ExperienceCard, f32)]) -> Option<ExperienceCard> {
        if scored.is_empty() { return None; }
        let max_utility = scored.iter().map(|(_, s)| *s).fold(f32::MIN, f32::max);
        let exp_scores: Vec<f32> = scored.iter().map(|(_, s)| ((s - max_utility) / self.temperature).exp()).collect();
        let sum_exp: f32 = exp_scores.iter().sum();
        if sum_exp.is_nan() || sum_exp <= 0.0 || !sum_exp.is_finite() {
            return scored.iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)).map(|(c, _)| c.clone());
        }
        let probs: Vec<f32> = exp_scores.iter().map(|e| e / sum_exp).collect();
        let mut rng = rand::thread_rng();
        let sample: f32 = rand::Rng::gen(&mut rng);
        let mut cumsum = 0.0;
        for (i, prob) in probs.iter().enumerate() {
            cumsum += prob;
            if sample <= cumsum { return Some(scored[i].0.clone()); }
        }
        Some(scored.last().unwrap().0.clone())
    }
}
```

### 10.3 AEGIS-GSOE四阶段引擎（小米融合）

弱模型+强Harness > 强模型+弱Harness。Qwen 53%→97%(+44%)。

```rust
// crates/aegis-gsoe/src/lib.rs
#![forbid(unsafe_code)]
use nexus_contracts::ExperienceCard;

pub struct AegisGsoe {
    digester: RuleBasedDigester,
    planner: StatisticalPlanner,
    evolver: TemplateEvolver,
    critic: RuleBasedCritic,
}

#[derive(Clone, Debug)]
pub struct Trajectory { pub cards: Vec<ExperienceCard>, pub success: bool, pub final_score: f32, pub total_tokens: u64 }

#[derive(Clone, Debug)]
pub struct DigestedTrajectories { pub success_patterns: Vec<SuccessPattern>, pub failure_patterns: Vec<FailurePattern> }

#[derive(Clone, Debug)]
pub struct SuccessPattern { pub method_family: String, pub avg_score: f32, pub key_factors: Vec<String> }

#[derive(Clone, Debug)]
pub struct FailurePattern { pub error_type: String, pub error_hash: String, pub frequency: u32, pub avg_score: f32 }

#[derive(Clone, Debug)]
pub struct AdaptationPlan { pub adaptations: Vec<Adaptation> }

#[derive(Clone, Debug)]
pub struct Adaptation { pub target: FailurePattern, pub direction: String, pub confidence: f32 }

#[derive(Clone, Debug)]
pub struct HarnessDelta { pub adaptations: Vec<Adaptation>, pub estimated_improvement: f32 }

impl AegisGsoe {
    pub fn new() -> Self {
        Self { digester: RuleBasedDigester, planner: StatisticalPlanner::new(), evolver: TemplateEvolver, critic: RuleBasedCritic }
    }
    pub fn evolve(&mut self, trajectories: &[Trajectory]) -> Result<HarnessDelta, String> {
        let digested = self.digester.compress(trajectories);
        let plan = self.planner.plan(&digested);
        let candidates = self.evolver.generate(&plan);
        let selected = self.critic.select(candidates, trajectories);
        Ok(selected)
    }
}

pub struct RuleBasedDigester;
impl RuleBasedDigester {
    pub fn compress(&self, trajectories: &[Trajectory]) -> DigestedTrajectories {
        let mut failure_patterns = vec![]; let mut success_patterns = vec![];
        for traj in trajectories {
            if traj.success && traj.final_score > 0.7 {
                success_patterns.push(self.extract_success_pattern(traj));
            } else {
                failure_patterns.extend(self.extract_failure_patterns(traj));
            }
        }
        DigestedTrajectories { success_patterns, failure_patterns: self.cluster_by_error_signature(failure_patterns) }
    }
    fn extract_success_pattern(&self, traj: &Trajectory) -> SuccessPattern {
        let last = traj.cards.last().unwrap();
        SuccessPattern { method_family: last.method_family.clone(), avg_score: traj.final_score, key_factors: vec![format!("operator: {:?}", last.operator)] }
    }
    fn extract_failure_patterns(&self, traj: &Trajectory) -> Vec<FailurePattern> {
        traj.cards.iter().filter(|c| c.error_signature.is_some()).map(|c| {
            let sig = c.error_signature.as_ref().unwrap();
            FailurePattern { error_type: sig.error_type.clone(), error_hash: sig.error_hash.clone(), frequency: 1, avg_score: c.score }
        }).collect()
    }
    fn cluster_by_error_signature(&self, patterns: Vec<FailurePattern>) -> Vec<FailurePattern> {
        let mut clusters: HashMap<String, Vec<FailurePattern>> = HashMap::new();
        for p in patterns { clusters.entry(p.error_hash.clone()).or_default().push(p); }
        clusters.into_iter().map(|(hash, group)| {
            let total_freq: u32 = group.iter().map(|p| p.frequency).sum();
            let avg_score = group.iter().map(|p| p.avg_score).sum::<f32>() / group.len() as f32;
            FailurePattern { error_type: group.first().unwrap().error_type.clone(), error_hash: hash, frequency: total_freq, avg_score }
        }).collect()
    }
}

pub struct StatisticalPlanner { adaptation_success_rate: HashMap<String, f32> }
impl StatisticalPlanner {
    pub fn new() -> Self { Self { adaptation_success_rate: HashMap::new() } }
    pub fn plan(&self, digested: &DigestedTrajectories) -> AdaptationPlan {
        let mut adaptations = vec![];
        for failure in &digested.failure_patterns {
            let best_fix = self.adaptation_success_rate.iter()
                .filter(|(k, _)| k.contains(&failure.error_type) || failure.error_type.contains(*k))
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(k, _)| k.clone());
            if let Some(fix) = best_fix {
                adaptations.push(Adaptation { target: failure.clone(), direction: fix.clone(), confidence: self.adaptation_success_rate[&fix] });
            }
        }
        AdaptationPlan { adaptations }
    }
}

pub struct TemplateEvolver;
impl TemplateEvolver {
    pub fn generate(&self, plan: &AdaptationPlan) -> Vec<HarnessDelta> {
        plan.adaptations.iter().map(|adapt| HarnessDelta { adaptations: vec![adapt.clone()], estimated_improvement: adapt.confidence * 0.1 }).collect()
    }
}

pub struct RuleBasedCritic;
impl RuleBasedCritic {
    pub fn select(&self, candidates: Vec<HarnessDelta>, _trajectories: &[Trajectory]) -> HarnessDelta {
        candidates.into_iter().max_by(|a, b| a.estimated_improvement.partial_cmp(&b.estimated_improvement).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or(HarnessDelta { adaptations: vec![], estimated_improvement: 0.0 })
    }
}
```

### 10.4 变体隔离池 + 保留历史最佳（小米 + RSIBench融合）

RSIBench关键发现：78.26%的继续搜索最终以低于历史峰值结束。必须显式保留历史最佳。

```rust
// crates/variant-pool/src/lib.rs
#![forbid(unsafe_code)]
use nexus_contracts::SixDimensionConfig;
use chrono::{DateTime, Utc};

pub struct VariantPool {
    variants: Vec<HarnessVariant>,
    best_variants: HashMap<String, String>,
    default_variant_id: String,
}

#[derive(Clone, Debug)]
pub struct HarnessVariant {
    pub variant_id: String,
    pub name: String,
    pub config: SixDimensionConfig,
    pub performance_history: Vec<PerformanceRecord>,
    pub avg_score: f32,
    pub created_at: DateTime<Utc>,
    pub status: VariantStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub enum VariantStatus { Pending, Active, Deprecated }

#[derive(Clone, Debug)]
pub struct PerformanceRecord { pub task_type: String, pub score: f32, pub timestamp: DateTime<Utc>, pub tokens_used: u32 }

impl VariantPool {
    pub fn new(default_variant: HarnessVariant) -> Self {
        let id = default_variant.variant_id.clone();
        Self { variants: vec![default_variant], best_variants: HashMap::new(), default_variant_id: id }
    }
    pub fn add_variant(&mut self, variant: HarnessVariant) { self.variants.push(variant); }
    pub fn select_variant(&self, task_type: &str) -> Option<&HarnessVariant> {
        if let Some(best_id) = self.best_variants.get(task_type) {
            if let Some(v) = self.variants.iter().find(|v| v.variant_id == *best_id && v.status == VariantStatus::Active) {
                return Some(v);
            }
        }
        self.variants.iter().filter(|v| v.status == VariantStatus::Active)
            .max_by(|a, b| a.avg_score.partial_cmp(&b.avg_score).unwrap_or(std::cmp::Ordering::Equal))
    }
    pub fn update_performance(&mut self, variant_id: &str, task_type: &str, score: f32, tokens_used: u32) {
        if let Some(variant) = self.variants.iter_mut().find(|v| v.variant_id == variant_id) {
            variant.performance_history.push(PerformanceRecord { task_type: task_type.to_string(), score, timestamp: Utc::now(), tokens_used });
            variant.avg_score = variant.avg_score * 0.9 + score * 0.1;
            let current_best_score = self.best_variants.get(task_type).and_then(|id| self.variants.iter().find(|v| v.variant_id == *id)).map(|v| v.avg_score).unwrap_or(0.0);
            if variant.avg_score > current_best_score { self.best_variants.insert(task_type.to_string(), variant_id.to_string()); }
        }
    }
    pub fn prune(&mut self, keep_count: usize) {
        let mut scored: Vec<_> = self.variants.iter_mut().filter(|v| v.status == VariantStatus::Active).collect();
        scored.sort_by(|a, b| b.avg_score.partial_cmp(&a.avg_score).unwrap_or(std::cmp::Ordering::Equal));
        for variant in scored.into_iter().skip(keep_count) { variant.status = VariantStatus::Deprecated; }
    }
}

pub struct CheckpointPreserver {
    best_checkpoints: HashMap<String, BestCheckpoint>,
}

#[derive(Clone, Debug)]
pub struct BestCheckpoint {
    pub checkpoint_id: String,
    pub score: f32,
    pub timestamp: DateTime<Utc>,
    pub code: String,
    pub card_id: String,
}

impl CheckpointPreserver {
    pub fn new() -> Self { Self { best_checkpoints: HashMap::new() } }
    pub fn preserve(&mut self, task_id: &str, checkpoint: BestCheckpoint) {
        let should_update = self.best_checkpoints.get(task_id).map(|best| checkpoint.score > best.score).unwrap_or(true);
        if should_update { self.best_checkpoints.insert(task_id.to_string(), checkpoint); }
    }
    pub fn get_best(&self, task_id: &str) -> Option<&BestCheckpoint> { self.best_checkpoints.get(task_id) }
}
```

### 10.5 Skill生命周期状态机（MSCE）

```rust
// skill-graph/src/lifecycle.rs
#![forbid(unsafe_code)]
use nexus_contracts::{SkillLifecycleContract, SkillLifecycleState};

pub struct SkillLifecycleManager {
    skills: HashMap<String, SkillLifecycleContract>,
    probation_period_ms: u64,
    activation_threshold: u32,
    archive_threshold: u32,
}

impl SkillLifecycleManager {
    pub fn record_outcome(&mut self, skill_id: &str, success: bool) {
        let mut contract = self.skills.get_mut(skill_id).unwrap();
        contract.last_used = now();
        match contract.state {
            SkillLifecycleState::Probationary => {
                if success {
                    contract.success_count += 1;
                    if contract.success_count >= contract.activation_threshold {
                        contract.state = SkillLifecycleState::Active;
                        contract.probation_end = Some(now());
                    }
                } else {
                    contract.failure_count += 1;
                    if contract.failure_count >= contract.archive_threshold {
                        contract.state = SkillLifecycleState::Archived;
                    }
                }
            }
            SkillLifecycleState::Active => {
                if !success {
                    contract.failure_count += 1;
                    if contract.failure_count >= contract.archive_threshold {
                        contract.state = SkillLifecycleState::Archived;
                    }
                } else { contract.failure_count = 0; }
            }
            SkillLifecycleState::Archived => {}
        }
    }
    pub fn get_active_skills(&self) -> Vec<&SkillNode> {
        self.skills.iter().filter(|(_, c)| c.state == SkillLifecycleState::Active)
            .map(|(id, _)| self.skill_graph.get_node(id).unwrap()).collect()
    }
}
```

---


## 11. L6 Router：路由层——Skills渐进加载 + 算子路由 + 六维动态调整 + 三因子选择 + 工具裁剪

### 11.1 Skills渐进加载（PenguinHarness "Index First, Body on Demand"）

```rust
// crates/skills-progressive-loader/src/lib.rs
#![forbid(unsafe_code)]
use nexus_contracts::CLV;

pub struct ProgressiveSkillLoader {
    skill_index: Vec<SkillMetadata>,
    skill_bodies: Arc<tokio::sync::Mutex<HashMap<String, SkillBody>>>,
    similarity_threshold: f32,
}

#[derive(Clone, Debug)]
pub struct SkillMetadata {
    pub skill_id: String,
    pub name: String,
    pub description: String,
    pub embedding: CLV,
    pub tags: Vec<String>,
    pub body_size: usize,
    pub last_used: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Clone, Debug)]
pub struct SkillBody {
    pub skill_id: String,
    pub code: String,
    pub examples: Vec<String>,
    pub tests: Vec<String>,
    pub documentation: String,
}

#[derive(Clone, Debug)]
pub struct LoadedSkill { pub metadata: SkillMetadata, pub body: SkillBody }

impl ProgressiveSkillLoader {
    pub fn new(similarity_threshold: f32) -> Self {
        Self { skill_index: Vec::new(), skill_bodies: Arc::new(tokio::sync::Mutex::new(HashMap::new())), similarity_threshold: similarity_threshold.max(0.5).min(0.95) }
    }

    pub fn register_index(&mut self, metadata: Vec<SkillMetadata>) { self.skill_index = metadata; }

    pub async fn load_skills(&self, task_embedding: &CLV, max_index_count: usize, max_full_load: usize) -> Vec<LoadedSkill> {
        let mut scored: Vec<(SkillMetadata, f32)> = self.skill_index.iter().map(|meta| {
            let similarity = meta.embedding.cosine_similarity(task_embedding);
            (meta.clone(), similarity)
        }).filter(|(_, score)| *score >= self.similarity_threshold).collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let top_indexed: Vec<_> = scored.into_iter().take(max_index_count).collect();
        let to_load = top_indexed.iter().take(max_full_load).collect::<Vec<_>>();
        let mut loaded = vec![];
        for (meta, score) in top_indexed {
            let body = if to_load.iter().any(|(m, _)| m.skill_id == meta.skill_id) {
                self.load_body(&meta.skill_id).await
            } else {
                SkillBody { skill_id: meta.skill_id.clone(), code: format!("// Not loaded (sim: {:.2})", score), examples: vec![], tests: vec![], documentation: meta.description.clone() }
            };
            loaded.push(LoadedSkill { metadata: meta, body });
        }
        loaded
    }

    async fn load_body(&self, skill_id: &str) -> SkillBody {
        let cache = self.skill_bodies.lock().await;
        if let Some(body) = cache.get(skill_id) { return body.clone(); }
        drop(cache);
        let body = SkillBody { skill_id: skill_id.to_string(), code: format!("// Loaded {}", skill_id), examples: vec![], tests: vec![], documentation: "Loaded on demand".to_string() };
        let mut cache = self.skill_bodies.lock().await;
        cache.insert(skill_id.to_string(), body.clone());
        body
    }

    pub async fn get_stats(&self) -> LoaderStats {
        let cache = self.skill_bodies.lock().await;
        LoaderStats { total_indexed: self.skill_index.len(), bodies_loaded: cache.len(), memory_saved_ratio: 1.0 - (cache.len() as f32 / self.skill_index.len().max(1) as f32) }
    }
}

#[derive(Clone, Debug)]
pub struct LoaderStats { pub total_indexed: usize, pub bodies_loaded: usize, pub memory_saved_ratio: f32 }
```

### 11.2 算子路由器（OpenMLE Greedy/ThreeFactor/UCB/Cooling）

```rust
// crates/operator-router/src/lib.rs
#![forbid(unsafe_code)]
use nexus_contracts::{AtomicOperator, OperatorSelectionStrategy, ExecutionStatus};
use four_operators::{AtomicOperatorTrait, DraftOperator, ImproveOperator, DebugOperator, CrossoverOperator};

pub struct OperatorRouter {
    operators: HashMap<AtomicOperator, Box<dyn AtomicOperatorTrait>>,
    selection_strategy: OperatorSelectionStrategy,
    history: Vec<OperatorSelectionRecord>,
    ucb_constant: f32,
    cooling_rate: f32,
    total_selections: u32,
}

#[derive(Clone, Debug)]
pub struct OperatorSelectionRecord {
    pub task_type: String,
    pub selected_operator: AtomicOperator,
    pub result_score: f32,
    pub execution_status: ExecutionStatus,
    pub timestamp: DateTime<Utc>,
}

impl OperatorRouter {
    pub fn new(strategy: OperatorSelectionStrategy) -> Self {
        let mut operators: HashMap<AtomicOperator, Box<dyn AtomicOperatorTrait>> = HashMap::new();
        operators.insert(AtomicOperator::Draft, Box::new(DraftOperator));
        operators.insert(AtomicOperator::Improve, Box::new(ImproveOperator));
        operators.insert(AtomicOperator::Debug, Box::new(DebugOperator));
        operators.insert(AtomicOperator::Crossover, Box::new(CrossoverOperator));
        Self { operators, selection_strategy: strategy, history: Vec::new(), ucb_constant: 1.414, cooling_rate: 0.01, total_selections: 0 }
    }

    pub fn select_operator(&mut self, task_type: &str, context: &OperatorContext) -> Option<AtomicOperator> {
        let applicable: Vec<_> = self.operators.iter().filter(|(_, op)| op.is_applicable(context)).map(|(op, _)| op.clone()).collect();
        if applicable.is_empty() { return None; }
        let selected = match self.selection_strategy {
            OperatorSelectionStrategy::Greedy => self.select_greedy(task_type, &applicable),
            OperatorSelectionStrategy::ThreeFactor => self.select_three_factor(task_type, &applicable),
            OperatorSelectionStrategy::UCB => self.select_ucb(task_type, &applicable),
            OperatorSelectionStrategy::Cooling => self.select_cooling(task_type, &applicable),
        };
        self.total_selections += 1;
        selected
    }

    pub fn record_result(&mut self, task_type: &str, operator: AtomicOperator, score: f32, status: ExecutionStatus) {
        self.history.push(OperatorSelectionRecord { task_type: task_type.to_string(), selected_operator: operator, result_score: score, execution_status: status, timestamp: Utc::now() });
    }

    fn select_greedy(&self, task_type: &str, applicable: &[AtomicOperator]) -> Option<AtomicOperator> {
        let mut best_operator = applicable.first()?.clone();
        let mut best_score = -1.0;
        for op in applicable {
            let avg_score = self.history.iter().filter(|r| r.task_type == task_type && r.selected_operator == *op && r.execution_status == ExecutionStatus::Success).map(|r| r.result_score).sum::<f32>();
            let count = self.history.iter().filter(|r| r.selected_operator == *op).count() as f32;
            let score = if count > 0.0 { avg_score / count } else { 0.0 };
            if score > best_score { best_score = score; best_operator = op.clone(); }
        }
        Some(best_operator)
    }

    fn select_three_factor(&self, task_type: &str, applicable: &[AtomicOperator]) -> Option<AtomicOperator> {
        let mut best_operator = applicable.first()?.clone();
        let mut best_utility = -1.0;
        for op in applicable {
            let records: Vec<_> = self.history.iter().filter(|r| r.task_type == task_type && r.selected_operator == *op).collect();
            if records.is_empty() { return Some(op.clone()); }
            let quality = records.iter().map(|r| r.result_score).sum::<f32>() / records.len() as f32;
            let progress = records.iter().map(|r| r.result_score).fold(0.0, f32::max) - quality;
            let novelty = 1.0 / (records.len() as f32 + 1.0);
            let utility = quality + progress + novelty;
            if utility > best_utility { best_utility = utility; best_operator = op.clone(); }
        }
        Some(best_operator)
    }

    fn select_ucb(&self, task_type: &str, applicable: &[AtomicOperator]) -> Option<AtomicOperator> {
        let mut best_operator = applicable.first()?.clone();
        let mut best_score = -f32::MAX;
        for op in applicable {
            let records: Vec<_> = self.history.iter().filter(|r| r.task_type == task_type && r.selected_operator == *op).collect();
            let visits = records.len() as f32;
            let avg_reward = if visits > 0.0 { records.iter().map(|r| r.result_score).sum::<f32>() / visits } else { 0.0 };
            let ucb = if visits > 0.0 && self.total_selections > 0 {
                avg_reward + self.ucb_constant * ((2.0 * (self.total_selections as f32).ln()) / visits).sqrt()
            } else { f32::MAX };
            if ucb > best_score { best_score = ucb; best_operator = op.clone(); }
        }
        Some(best_operator)
    }

    fn select_cooling(&self, task_type: &str, applicable: &[AtomicOperator]) -> Option<AtomicOperator> {
        let epsilon = (-self.cooling_rate * self.total_selections as f32).exp();
        let mut rng = rand::thread_rng();
        if rand::Rng::gen::<f32>(&mut rng) < epsilon {
            let idx = rand::Rng::gen_range(&mut rng, 0..applicable.len());
            return Some(applicable[idx].clone());
        }
        self.select_greedy(task_type, applicable)
    }
}
```

### 11.3 工具Schema动态裁剪（Dressage实证）

33个工具→4个，13.5K→1.7K tokens。基于使用频率动态裁剪。

```rust
// osa-coordinator/src/tool_pruning.rs
#![forbid(unsafe_code)]

pub struct ToolSchemaPruner {
    tool_usage_stats: HashMap<String, ToolUsageStats>,
    pruning_threshold: f32,
    min_tools: usize,
}

#[derive(Clone, Debug)]
pub struct ToolUsageStats {
    pub tool_name: String,
    pub call_count: u32,
    pub success_count: u32,
    pub total_tokens_consumed: u32,
    pub last_used: u64,
}

impl ToolSchemaPruner {
    pub fn analyze_trajectories(&mut self, trajectories: &[Trajectory]) {
        for traj in trajectories {
            for step in &traj.steps {
                if let Some(ref tool_call) = step.tool_call {
                    let stats = self.tool_usage_stats.entry(tool_call.tool_name.clone()).or_insert(ToolUsageStats {
                        tool_name: tool_call.tool_name.clone(), call_count: 0, success_count: 0, total_tokens_consumed: 0, last_used: 0,
                    });
                    stats.call_count += 1;
                    if step.success { stats.success_count += 1; }
                    stats.last_used = now();
                }
            }
        }
    }

    pub fn prune_tools(&self, available_tools: &[ToolSchema]) -> Vec<ToolSchema> {
        let mut scored_tools: Vec<(&ToolSchema, f32)> = available_tools.iter().map(|tool| {
            let stats = self.tool_usage_stats.get(&tool.name);
            let score = match stats {
                Some(s) => {
                    let frequency = s.call_count as f32 / self.total_trajectories() as f32;
                    let success_rate = s.success_count as f32 / s.call_count.max(1) as f32;
                    let recency = self.recency_score(s.last_used);
                    frequency * 0.4 + success_rate * 0.4 + recency * 0.2
                }
                None => 0.0,
            };
            (tool, score)
        }).collect();
        scored_tools.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let keep_count = scored_tools.len().max(self.min_tools);
        scored_tools.into_iter().take(keep_count).map(|(t, _)| t.clone()).collect()
    }
}
```

---

## 12. L7 Execution：执行层——PVL算子化 + 经验卡片生成 + Process-Score + Segment-aware验证 + 熵加权

### 12.1 经验卡片生成器（PVL验证结果→经验卡片转换）

```rust
// crates/experience-card-generator/src/lib.rs
#![forbid(unsafe_code)]
use nexus_contracts::{ExperienceCard, ThreeFactorScore, AtomicOperator, ErrorSignature, ExecutionStatus, CardMetadata, TokenUsage, EnvironmentInfo};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct ExperienceCardGenerator {
    card_counter: AtomicU64,
    chimera_version: String,
}

pub struct ValidationResult {
    pub success: bool, pub score: f32,
    pub error_type: Option<String>, pub error_location: Option<String>, pub error_message: Option<String>,
    pub execution_time_ms: u64, pub token_usage: TokenUsage, pub lines_changed: i32,
}

pub struct ExecutionMetadata {
    pub task_id: String, pub parent_id: Option<String>,
    pub operator: AtomicOperator, pub code: String, pub skills_used: Vec<String>,
}

impl ExperienceCardGenerator {
    pub fn new(chimera_version: String) -> Self { Self { card_counter: AtomicU64::new(0), chimera_version } }

    pub fn generate(&self, metadata: &ExecutionMetadata, validation: &ValidationResult) -> ExperienceCard {
        let card_id = format!("card_{:010}", self.card_counter.fetch_add(1, Ordering::SeqCst));
        let node_id = format!("node_{:010}", self.card_counter.load(Ordering::SeqCst));
        let three_factor = self.compute_three_factor(validation.score, metadata.parent_id.as_ref(), metadata.operator.clone(), validation);
        let error_signature = if !validation.success {
            validation.error_type.as_ref().and_then(|et| {
                validation.error_message.as_ref().map(|em| ErrorSignature::from_output(et, validation.error_location.as_deref().unwrap_or("unknown"), em))
            })
        } else { None };
        let execution_status = if validation.success { ExecutionStatus::Success }
            else if validation.error_message.is_some() { ExecutionStatus::Error }
            else { ExecutionStatus::ScoreFailed };
        ExperienceCard {
            card_id, task_id: metadata.task_id.clone(), node_id, parent_id: metadata.parent_id.clone(),
            created_at: Utc::now(), operator: metadata.operator.clone(), score: validation.score,
            delta_vs_parent: three_factor.progress, method_family: self.infer_method_family(metadata.operator.clone()),
            error_signature, three_factor, execution_status,
            token_evidence_ids: vec![], segment_id: None,
            metadata: CardMetadata {
                execution_time_ms: validation.execution_time_ms,
                token_usage: validation.token_usage.clone(),
                lines_changed: validation.lines_changed,
                skills_used: metadata.skills_used.clone(),
                environment: EnvironmentInfo { rust_version: "1.80".to_string(), os: std::env::consts::OS.to_string(), cpu_arch: std::env::consts::ARCH.to_string(), chimera_version: self.chimera_version.clone() }
            }
        }
    }

    fn compute_three_factor(&self, score: f32, _parent_id: Option<&String>, operator: AtomicOperator, validation: &ValidationResult) -> ThreeFactorScore {
        let quality = score.clamp(0.0, 1.0);
        let progress = 0.0;
        let base_novelty = match operator {
            AtomicOperator::Draft => 0.3,
            AtomicOperator::Improve => 0.5,
            AtomicOperator::Debug => 0.2,
            AtomicOperator::Crossover => 0.8,
        };
        let token_efficiency = if validation.token_usage.total_tokens > 0 {
            let baseline = 5000.0;
            let ratio = baseline / validation.token_usage.total_tokens as f32;
            ratio.min(1.0) * 0.2
        } else { 0.0 };
        let novelty = (base_novelty + token_efficiency).min(1.0);
        ThreeFactorScore { quality, progress, novelty }
    }

    fn infer_method_family(&self, operator: AtomicOperator) -> String {
        match operator {
            AtomicOperator::Draft => "draft_pipeline".to_string(),
            AtomicOperator::Improve => "iterative_improvement".to_string(),
            AtomicOperator::Debug => "error_fix".to_string(),
            AtomicOperator::Crossover => "code_merge".to_string(),
        }
    }
}
```

### 12.2 Process-Score九维度（快手KAT融合）

探索/定位/忠实/最小/验证/诚实/效率/鲁棒/可读。

```rust
// crates/process-score-calculator/src/lib.rs
#![forbid(unsafe_code)]
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct ProcessScore {
    pub exploration: f32, pub localization: f32, pub fidelity: f32,
    pub minimality: f32, pub verification: f32, pub honesty: f32,
    pub efficiency: f32, pub robustness: f32, pub readability: f32,
}

pub struct ProcessTrajectory {
    pub actions: Vec<TrajectoryAction>, pub total_tokens: u64, pub final_score: f32,
    pub target_score: f32, pub code_changes: Vec<CodeChange>,
    pub verification_steps: Vec<VerificationStep>, pub reported_errors: Vec<String>, pub actual_errors: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct TrajectoryAction { pub operator: nexus_contracts::AtomicOperator, pub timestamp_ms: u64, pub success: bool }

#[derive(Clone, Debug)]
pub struct CodeChange { pub file_path: String, pub lines_added: i32, pub lines_removed: i32 }

#[derive(Clone, Debug)]
pub struct VerificationStep { pub step_type: String, pub passed: bool, pub coverage_percent: f32 }

impl ProcessScore {
    pub fn from_trajectory(traj: &ProcessTrajectory) -> Self {
        Self {
            exploration: Self::score_exploration(traj), localization: Self::score_localization(traj),
            fidelity: Self::score_fidelity(traj), minimality: Self::score_minimality(traj),
            verification: Self::score_verification(traj), honesty: Self::score_honesty(traj),
            efficiency: Self::score_efficiency(traj), robustness: Self::score_robustness(traj),
            readability: Self::score_readability(traj),
        }
    }

    pub fn overall(&self) -> f32 {
        self.exploration * 0.15 + self.localization * 0.15 + self.fidelity * 0.15 +
        self.minimality * 0.10 + self.verification * 0.15 + self.honesty * 0.10 +
        self.efficiency * 0.10 + self.robustness * 0.05 + self.readability * 0.05
    }

    fn score_exploration(traj: &ProcessTrajectory) -> f32 {
        let unique: HashSet<_> = traj.actions.iter().map(|a| a.operator.clone()).collect();
        (unique.len() as f32 / 4.0).min(1.0)
    }
    fn score_localization(traj: &ProcessTrajectory) -> f32 {
        let debug_actions: Vec<_> = traj.actions.iter().filter(|a| matches!(a.operator, nexus_contracts::AtomicOperator::Debug)).collect();
        if debug_actions.is_empty() { return 1.0; }
        debug_actions.iter().filter(|a| a.success).count() as f32 / debug_actions.len() as f32
    }
    fn score_fidelity(traj: &ProcessTrajectory) -> f32 {
        if traj.target_score <= 0.0 { return 1.0; }
        (traj.final_score / traj.target_score).min(1.0)
    }
    fn score_minimality(traj: &ProcessTrajectory) -> f32 {
        let total: i32 = traj.code_changes.iter().map(|c| c.lines_added.abs() + c.lines_removed.abs()).sum();
        if total == 0 { return 1.0; }
        let baseline = 50.0;
        (baseline / (total as f32 + baseline)).min(1.0)
    }
    fn score_verification(traj: &ProcessTrajectory) -> f32 {
        if traj.verification_steps.is_empty() { return 0.5; }
        traj.verification_steps.iter().map(|v| v.coverage_percent).sum::<f32>() / traj.verification_steps.len() as f32 / 100.0
    }
    fn score_honesty(traj: &ProcessTrajectory) -> f32 {
        let reported: HashSet<_> = traj.reported_errors.iter().collect();
        let actual: HashSet<_> = traj.actual_errors.iter().collect();
        if actual.is_empty() { return if reported.is_empty() { 1.0 } else { 0.5 }; }
        let correct = reported.intersection(&actual).count();
        let precision = if reported.is_empty() { 1.0 } else { correct as f32 / reported.len() as f32 };
        let recall = if actual.is_empty() { 1.0 } else { correct as f32 / actual.len() as f32 };
        let f1 = if precision + recall > 0.0 { 2.0 * precision * recall / (precision + recall) } else { 0.0 };
        let penalty = ((reported.len() - correct) as f32 * 0.1 + (actual.len() - correct) as f32 * 0.2).min(0.5);
        (f1 - penalty).max(0.0)
    }
    fn score_efficiency(traj: &ProcessTrajectory) -> f32 {
        if traj.final_score <= 0.0 { return 0.0; }
        let tokens_per_point = traj.total_tokens as f32 / traj.final_score;
        let baseline = 10000.0;
        if tokens_per_point < baseline { 1.0 } else { (baseline / tokens_per_point).min(1.0) }
    }
    fn score_robustness(_traj: &ProcessTrajectory) -> f32 { 0.5 }
    fn score_readability(_traj: &ProcessTrajectory) -> f32 { 0.5 }
}
```

### 12.3 动态验证深度 + 熵加权（OpenMLE + 快手融合）

```rust
// crates/dynamic-verification-depth/src/lib.rs
#![forbid(unsafe_code)]

pub struct DynamicVerifier {
    depth_effectiveness: HashMap<VerificationDepth, f32>,
    default_depth: VerificationDepth,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum VerificationDepth { FullVerify, StandardVerify, IncrementalVerify, SyntaxOnly, SkipVerify }

#[derive(Clone, Debug)]
pub struct TaskRisk { pub level: u8, pub factors: Vec<String> }

impl DynamicVerifier {
    pub fn new() -> Self {
        let mut de = HashMap::new();
        de.insert(VerificationDepth::FullVerify, 0.95);
        de.insert(VerificationDepth::StandardVerify, 0.90);
        de.insert(VerificationDepth::IncrementalVerify, 0.85);
        de.insert(VerificationDepth::SyntaxOnly, 0.70);
        de.insert(VerificationDepth::SkipVerify, 0.50);
        Self { depth_effectiveness: de, default_depth: VerificationDepth::StandardVerify }
    }

    pub fn select_depth(&self, task_risk: &TaskRisk, operator: &nexus_contracts::AtomicOperator) -> VerificationDepth {
        if task_risk.level > 80 { return VerificationDepth::FullVerify; }
        if matches!(operator, nexus_contracts::AtomicOperator::Crossover) && task_risk.level > 50 { return VerificationDepth::FullVerify; }
        if matches!(operator, nexus_contracts::AtomicOperator::Debug) { return VerificationDepth::StandardVerify; }
        let historical_best = self.depth_effectiveness.iter().max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal)).map(|(d, _)| d.clone()).unwrap_or(self.default_depth.clone());
        if task_risk.level > 50 { return VerificationDepth::StandardVerify; }
        historical_best
    }

    pub fn update_effectiveness(&mut self, depth: VerificationDepth, success: bool) {
        let current = self.depth_effectiveness.entry(depth).or_insert(0.5);
        let reward = if success { 1.0 } else { 0.0 };
        *current = *current * 0.9 + reward * 0.1;
    }
}

pub struct EntropyWeightedScorer;
impl EntropyWeightedScorer {
    pub fn score(card: &nexus_contracts::ExperienceCard, candidates: &[nexus_contracts::ExperienceCard]) -> f32 {
        let base_score = card.three_factor.selection_utility();
        let scores: Vec<f32> = candidates.iter().map(|c| c.three_factor.quality.exp()).collect();
        let sum_scores: f32 = scores.iter().sum();
        let p = if sum_scores > 0.0 { card.three_factor.quality.exp() / sum_scores } else { 1.0 / candidates.len() as f32 };
        let entropy = if p > 0.0 && p < 1.0 { -p * p.ln() - (1.0-p) * (1.0-p).ln() } else { 0.0 };
        base_score * (1.0 + entropy * 0.5)
    }
}
```

### 12.4 Segment-aware验证（Dressage核心）

```rust
// pvl-layer/src/segment_validation.rs
#![forbid(unsafe_code)]

pub struct SegmentAwareValidator {
    base_validator: PvlLayer,
    segment_registry: HashMap<String, Vec<SegmentMetadata>>,
}

impl SegmentAwareValidator {
    pub async fn validate_segment(&self, segment: &SegmentMetadata, test_cases: &[TestCase]) -> SegmentValidationResult {
        let token_entries = self.get_token_entries(segment);
        let syntax_pass = self.verify_syntax(&token_entries);
        let logic_pass = self.verify_logic(&token_entries);
        let sandbox_pass = self.run_sandbox_tests(&token_entries, test_cases).await;
        let segment_reward = self.compute_segment_reward(syntax_pass, logic_pass, sandbox_pass);
        SegmentValidationResult {
            segment_id: segment.segment_id.clone(), parent_traj_id: segment.parent_traj_id.clone(),
            syntax_pass, logic_pass, sandbox_pass, segment_reward, is_anchor: segment.is_anchor,
        }
    }

    pub fn broadcast_final_reward(&mut self, parent_traj_id: &str, final_reward: f32) {
        let segments = self.segment_registry.get_mut(parent_traj_id).unwrap();
        for segment in segments.iter_mut() {
            if segment.is_anchor { segment.final_reward = Some(final_reward); }
            else {
                let process_reward = segment.process_reward.unwrap_or(0.0);
                let propagated = process_reward + final_reward * 0.3;
                segment.final_reward = Some(propagated);
            }
        }
    }
}
```

---


## 13. L8 Parliament：议会层——变体审议 + 三因子裁决 + 停止策略 + 行为定位 + 冲突仲裁

### 13.1 变体审议 + 三因子裁决（小米 + OpenMLE融合）

```rust
// crates/three-factor-adjudicator/src/lib.rs
#![forbid(unsafe_code)]
use nexus_contracts::{ThreeFactorScore, ExperienceCard};
use variant_pool::{HarnessVariant, PerformanceRecord, BestCheckpoint};

pub struct ThreeFactorAdjudicator {
    skeptic_threshold: f32,
    security_threshold: f32,
    execution_threshold: f32,
    regression_tolerance: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Vote { Approve, Reject, Abstain }

#[derive(Clone, Debug)]
pub struct AdjudicationResult {
    pub three_factor: ThreeFactorScore,
    pub votes: Vec<(String, Vote)>,
    pub decision: ParliamentDecision,
    pub reasoning: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ParliamentDecision { Approve, Reject(String), RequestMoreData(String) }

#[derive(Clone, Debug)]
pub struct SmokeResults {
    pub tests_passed: u32, pub tests_failed: u32,
    pub has_regression: bool, pub regression_details: Vec<String>,
}

impl ThreeFactorAdjudicator {
    pub fn new(skeptic_threshold: f32, security_threshold: f32, execution_threshold: f32, regression_tolerance: f32) -> Self {
        Self { skeptic_threshold, security_threshold, execution_threshold, regression_tolerance }
    }

    pub fn adjudicate_variant(&self, variant: &HarnessVariant, baseline: &HarnessVariant, smoke_results: &SmokeResults) -> AdjudicationResult {
        let quality_delta = variant.avg_score - baseline.avg_score;
        let progress = variant.performance_history.iter().map(|r| r.score).sum::<f32>() / variant.performance_history.len().max(1) as f32;
        let novelty = self.compute_variant_novelty(variant, baseline);
        let three_factor = ThreeFactorScore { quality: variant.avg_score, progress: quality_delta, novelty };

        let skeptic_vote = if three_factor.progress > self.skeptic_threshold { Vote::Approve }
            else if three_factor.progress > 0.0 { Vote::Abstain } else { Vote::Reject };
        let security_vote = if smoke_results.has_regression { Vote::Reject }
            else if variant.avg_score >= baseline.avg_score * (1.0 - self.regression_tolerance) { Vote::Approve } else { Vote::Reject };
        let execution_vote = if three_factor.quality > self.execution_threshold { Vote::Approve }
            else if three_factor.quality > self.execution_threshold * 0.8 { Vote::Abstain } else { Vote::Reject };

        let votes = vec![
            ("Skeptic".to_string(), skeptic_vote.clone()),
            ("Security".to_string(), security_vote.clone()),
            ("Execution".to_string(), execution_vote.clone()),
        ];

        let decision = if security_vote == Vote::Reject {
            ParliamentDecision::Reject("Security: regression detected".to_string())
        } else {
            let approve_count = votes.iter().filter(|(_, v)| *v == Vote::Approve).count();
            let reject_count = votes.iter().filter(|(_, v)| *v == Vote::Reject).count();
            if reject_count >= 2 { ParliamentDecision::Reject("Insufficient support".to_string()) }
            else if approve_count >= 2 { ParliamentDecision::Approve }
            else { ParliamentDecision::RequestMoreData("Need more evidence".to_string()) }
        };

        let reasoning = format!("Q: {:.2}, P: {:.2}, N: {:.2}. Skeptic={:?}, Security={:?}, Execution={:?}",
            three_factor.quality, three_factor.progress, three_factor.novelty, skeptic_vote, security_vote, execution_vote);
        AdjudicationResult { three_factor, votes, decision, reasoning }
    }

    pub fn adjudicate_stop(&self, context: &StopContext) -> StopRuling {
        if context.attempts >= context.max_attempts {
            return StopRuling::Stop { reason: format!("Max attempts ({}) reached", context.max_attempts), preserve_best: true, selected_checkpoint: context.best_checkpoint.clone() };
        }
        if context.stagnation_count >= context.stagnation_threshold {
            return StopRuling::Stop { reason: format!("Stagnation: {} attempts without improvement", context.stagnation_count), preserve_best: true, selected_checkpoint: context.best_checkpoint.clone() };
        }
        if context.attempts > 10 {
            if let Some(ref best) = context.best_checkpoint {
                let gap = context.current_score / best.score;
                if gap < context.score_gap_threshold {
                    return StopRuling::Stop { reason: format!("Current {:.2} below best {:.2}, ratio={:.2}", context.current_score, best.score, gap), preserve_best: true, selected_checkpoint: Some(best.clone()) };
                }
            }
        }
        if context.attempts > 20 {
            if !matches!(context.current_operator, nexus_contracts::AtomicOperator::Crossover | nexus_contracts::AtomicOperator::Improve) {
                return StopRuling::SuggestSwitch { suggested: nexus_contracts::AtomicOperator::Crossover, reason: "Late-stage: switch to Crossover/Improve".to_string() };
            }
        }
        StopRuling::Continue
    }

    fn compute_variant_novelty(&self, variant: &HarnessVariant, baseline: &HarnessVariant) -> f32 {
        let config_diff = if variant.config == baseline.config { 0.0 } else { 0.5 };
        let history_bonus = (variant.performance_history.len() as f32 / 100.0).min(0.5);
        (config_diff + history_bonus).min(1.0)
    }
}

#[derive(Clone, Debug)]
pub struct StopContext {
    pub attempts: u32, pub max_attempts: u32,
    pub stagnation_count: u32, pub stagnation_threshold: u32,
    pub current_score: f32, pub best_score: f32, pub score_gap_threshold: f32,
    pub current_operator: nexus_contracts::AtomicOperator,
    pub best_checkpoint: Option<BestCheckpoint>,
}

#[derive(Clone, Debug)]
pub enum StopRuling {
    Continue,
    Stop { reason: String, preserve_best: bool, selected_checkpoint: Option<BestCheckpoint> },
    SuggestSwitch { suggested: nexus_contracts::AtomicOperator, reason: String },
}
```

### 13.2 冲突仲裁（TencentDB两阶段仲裁 → Parliament扩展）

```rust
// parliament/src/conflict_arbitration.rs
#![forbid(unsafe_code)]
use nexus_contracts::{AtomicMemoryCard, ParliamentDecision, Vote};

pub struct ConflictArbitrator {
    candidate_retriever: CandidateRetriever,
    model_judge: ModelJudge,
}

impl ConflictArbitrator {
    pub async fn arbitrate(&self, new_card: &AtomicMemoryCard, existing_cards: &[AtomicMemoryCard]) -> ArbitrationResult {
        let candidates = self.candidate_retriever.retrieve_similar(new_card, existing_cards);
        if candidates.is_empty() { return ArbitrationResult::AddNew; }
        let decision = self.model_judge.judge(new_card, &candidates).await;
        match decision {
            ModelDecision::AddNew => ArbitrationResult::AddNew,
            ModelDecision::Skip => ArbitrationResult::Skip,
            ModelDecision::Update(old_id) => ArbitrationResult::Update(old_id),
            ModelDecision::Merge(old_ids) => ArbitrationResult::Merge(old_ids),
        }
    }
}

impl Parliament {
    pub async fn deliberate_with_conflict_arbitration(&self, variant: &HarnessVariant) -> ParliamentDecision {
        let base_decision = self.review(variant);
        if variant.has_memory_changes() || variant.has_skill_changes() {
            let conflicts = self.detect_memory_conflicts(variant);
            if !conflicts.is_empty() {
                let arbitration = self.conflict_arbitrator.arbitrate_batch(&conflicts).await;
                if arbitration.has_unresolved_conflicts() {
                    return ParliamentDecision::Reject("Unresolved memory conflicts".to_string());
                }
            }
        }
        base_decision
    }
}
```

---

## 14. L9 Quest：任务层——Ambient Mode + 搜索树管理 + 长任务地图 + 长时程信用分配

### 14.1 搜索树管理（OpenMLE核心）

```rust
// crates/search-tree-manager/src/lib.rs
#![forbid(unsafe_code)]
use nexus_contracts::{ExperienceCard, AtomicOperator, ThreeFactorScore, ExecutionStatus};

pub struct SearchTreeManager {
    nodes: HashMap<String, ExperienceCard>,
    children: HashMap<String, Vec<String>>,
    current_depth: u32,
    max_depth: u32,
    best_node_id: Option<String>,
}

impl SearchTreeManager {
    pub fn new(max_depth: u32) -> Self {
        Self { nodes: HashMap::new(), children: HashMap::new(), current_depth: 0, max_depth, best_node_id: None }
    }

    pub fn create_root(&mut self, task_id: &str) -> String {
        let root_id = format!("root_{}", task_id);
        let root = ExperienceCard {
            card_id: format!("card_root_{}", task_id), task_id: task_id.to_string(), node_id: root_id.clone(),
            parent_id: None, created_at: chrono::Utc::now(), operator: AtomicOperator::Draft,
            score: 0.0, delta_vs_parent: 0.0, method_family: "root".to_string(),
            error_signature: None, three_factor: ThreeFactorScore::default_root(),
            execution_status: ExecutionStatus::Success, token_evidence_ids: vec![], segment_id: None,
            metadata: nexus_contracts::CardMetadata::default(),
        };
        self.nodes.insert(root_id.clone(), root);
        self.children.insert(root_id.clone(), vec![]);
        self.best_node_id = Some(root_id.clone());
        root_id
    }

    pub fn expand_node(&mut self, parent_id: &str, _operator: AtomicOperator, card: ExperienceCard) -> Result<String, TreeError> {
        if self.current_depth >= self.max_depth { return Err(TreeError::MaxDepthReached); }
        if !self.nodes.contains_key(parent_id) { return Err(TreeError::ParentNotFound); }
        let child_id = card.node_id.clone();
        self.children.entry(parent_id.to_string()).or_default().push(child_id.clone());
        self.nodes.insert(child_id.clone(), card);
        self.children.insert(child_id.clone(), vec![]);
        self.current_depth += 1;
        self.update_best_node(&child_id);
        Ok(child_id)
    }

    pub fn get_best_path(&self) -> Vec<&ExperienceCard> {
        match &self.best_node_id { Some(id) => self.trace_path(id), None => vec![] }
    }

    fn trace_path(&self, node_id: &str) -> Vec<&ExperienceCard> {
        let mut path = vec![];
        let mut current = node_id;
        while let Some(card) = self.nodes.get(current) {
            path.push(card);
            match &card.parent_id { Some(parent) => current = parent, None => break }
        }
        path.reverse(); path
    }

    pub fn prune(&mut self, threshold: f32) {
        let to_remove: Vec<String> = self.nodes.values().filter(|n| n.score < threshold && self.children.get(&n.node_id).map(|c| c.is_empty()).unwrap_or(true)).map(|n| n.node_id.clone()).collect();
        for node_id in to_remove {
            self.nodes.remove(&node_id); self.children.remove(&node_id);
            for children in self.children.values_mut() { children.retain(|c| c != &node_id); }
        }
    }

    pub fn get_stats(&self) -> TreeStats {
        TreeStats {
            total_nodes: self.nodes.len(), max_depth: self.current_depth,
            best_score: self.best_node_id.as_ref().and_then(|id| self.nodes.get(id)).map(|n| n.score).unwrap_or(0.0),
            leaf_nodes: self.nodes.keys().filter(|id| self.children.get(*id).map(|c| c.is_empty()).unwrap_or(true)).count(),
        }
    }

    fn update_best_node(&mut self, node_id: &str) {
        if let Some(new_card) = self.nodes.get(node_id) {
            let should_update = self.best_node_id.as_ref().and_then(|id| self.nodes.get(id)).map(|best| new_card.score > best.score).unwrap_or(true);
            if should_update { self.best_node_id = Some(node_id.to_string()); }
        }
    }
}

#[derive(Clone, Debug)]
pub struct TreeStats { pub total_nodes: usize, pub max_depth: u32, pub best_score: f32, pub leaf_nodes: usize }

#[derive(Debug, thiserror::Error)]
pub enum TreeError { #[error("Max depth reached")] MaxDepthReached, #[error("Parent not found")] ParentNotFound }
```

### 14.2 长任务地图（TencentDB机制）

Token消耗2.21亿→8500万，通过率33%→50%。详细过程转存外部文件，上下文只留任务地图。

```rust
// quest-engine/src/long_task_map.rs
#![forbid(unsafe_code)]

pub struct LongTaskMap {
    task_nodes: Vec<TaskNode>,
    task_edges: Vec<TaskEdge>,
    external_storage: ExternalStorage,
}

#[derive(Clone, Debug)]
pub struct TaskNode {
    pub node_id: String, pub step_number: u32,
    pub state_summary: String,    // 短摘要（放上下文）
    pub detail_ref: String,       // 详细记录的外部引用
    pub next_action: String,
    pub status: NodeStatus,
}

impl LongTaskMap {
    pub fn create_map(&mut self, quest: &Quest) -> TaskMapRef {
        let root = TaskNode {
            node_id: "root".to_string(), step_number: 0,
            state_summary: quest.title.clone(),
            detail_ref: self.external_storage.store(&quest.description),
            next_action: "start".to_string(), status: NodeStatus::Pending,
        };
        self.task_nodes.push(root);
        TaskMapRef { map_id: Uuid::new_v4().to_string(), root_id: "root".to_string() }
    }

    pub fn record_step(&mut self, map_ref: &TaskMapRef, step_result: &StepResult) {
        let node = TaskNode {
            node_id: format!("node_{}", self.task_nodes.len()),
            step_number: self.task_nodes.len() as u32,
            state_summary: self.summarize_state(&step_result.state),
            detail_ref: self.external_storage.store(&step_result.detail),
            next_action: step_result.next_action.clone(),
            status: if step_result.success { NodeStatus::Completed } else { NodeStatus::Failed },
        };
        self.task_nodes.push(node);
        let prev_id = self.task_nodes[self.task_nodes.len() - 2].node_id.clone();
        self.task_edges.push(TaskEdge { from: prev_id, to: self.task_nodes[self.task_nodes.len() - 1].node_id.clone(), action: step_result.action.clone() });
    }

    pub fn inject_map_to_context(&self, map_ref: &TaskMapRef, context: &mut String) {
        let map_summary = self.task_nodes.iter()
            .map(|n| format!("[{}] {} → {}", n.step_number, n.state_summary, n.next_action))
            .collect::<Vec<_>>().join("
");
        *context = format!("{}

[任务地图]
{}", context, map_summary);
    }
}
```

### 14.3 Ambient Mode（jcode融合）

后台常驻，定期整理记忆，等待资源恢复。

```rust
// quest-engine/src/ambient_mode.rs
#![forbid(unsafe_code)]

pub struct AmbientMode {
    quest_engine: QuestEngine,
    memory_organizer: MemoryOrganizer,
    checkpoint_scheduler: CheckpointScheduler,
    stop_strategy: StopStrategy,
}

impl AmbientMode {
    pub async fn start(&mut self) {
        loop {
            tokio::time::sleep(Duration::from_secs(300)).await;
            self.memory_organizer.organize().await;
            self.checkpoint_scheduler.save_checkpoints().await;
            if let Some(stop_decision) = self.stop_strategy.check().await {
                match stop_decision {
                    StopDecision::Stop { reason, preserve } => {
                        tracing::info!("Ambient mode stopping: {}", reason);
                        if let Some(checkpoint) = preserve { self.preserve_checkpoint(&checkpoint).await; }
                        break;
                    }
                    _ => {}
                }
            }
            if let Some(new_resource) = self.wait_for_resource().await {
                self.resume_quests(&new_resource).await;
            }
        }
    }
}
```

---

## 15. L10 Interface：接口层——Runtime Auditor + 自我评估面板 + 经验卡片可视化 + OmniMessage预留 + Concord TUI

### 15.1 Runtime Auditor（Qoder五维度证据纪律）

静态发现 ≠ 已执行验证。五维度：任务理解、可控执行、改动验证、可靠交付、经验沉淀。

```rust
// crates/runtime-auditor/src/lib.rs
#![forbid(unsafe_code)]
use nexus_contracts::ExperienceCard;

pub struct RuntimeAuditor { event_bus: ExperienceCardBus }

#[derive(Clone, Debug)]
pub struct SelfAssessment {
    pub task_comprehension: f32, pub controllable_execution: f32,
    pub change_verification: f32, pub reliable_delivery: f32,
    pub experience_accumulation: f32,
    pub three_factor_quality: f32, pub three_factor_progress: f32, pub three_factor_novelty: f32,
    pub overall_score: f32,
}

impl RuntimeAuditor {
    pub fn evaluate(&self, _window: Duration, cards: &[ExperienceCard]) -> SelfAssessment {
        let tc = self.score_task_comprehension(cards);
        let ce = self.score_controllable_execution(cards);
        let cv = self.score_change_verification(cards);
        let rd = self.score_reliable_delivery(cards);
        let ea = self.score_experience_accumulation(cards);
        let (tq, tp, tn) = if cards.is_empty() { (0.0, 0.0, 0.0) } else {
            let q = cards.iter().map(|c| c.three_factor.quality).sum::<f32>() / cards.len() as f32;
            let p = cards.iter().map(|c| c.three_factor.progress).sum::<f32>() / cards.len() as f32;
            let n = cards.iter().map(|c| c.three_factor.novelty).sum::<f32>() / cards.len() as f32;
            (q, p, n)
        };
        let overall = tc * 0.20 + ce * 0.20 + cv * 0.25 + rd * 0.20 + ea * 0.15;
        SelfAssessment { task_comprehension: tc, controllable_execution: ce, change_verification: cv, reliable_delivery: rd, experience_accumulation: ea, three_factor_quality: tq, three_factor_progress: tp, three_factor_novelty: tn, overall_score: overall }
    }

    fn score_task_comprehension(&self, cards: &[ExperienceCard]) -> f32 {
        if cards.is_empty() { return 0.0; }
        cards.iter().filter(|c| c.execution_status == nexus_contracts::ExecutionStatus::Success).count() as f32 / cards.len() as f32
    }
    fn score_controllable_execution(&self, cards: &[ExperienceCard]) -> f32 {
        if cards.is_empty() { return 0.0; }
        cards.iter().filter(|c| c.error_signature.is_none()).count() as f32 / cards.len() as f32
    }
    fn score_change_verification(&self, _cards: &[ExperienceCard]) -> f32 { 0.8 }
    fn score_reliable_delivery(&self, cards: &[ExperienceCard]) -> f32 {
        if cards.len() < 2 { return 1.0; }
        let scores: Vec<f32> = cards.iter().map(|c| c.score).collect();
        let avg = scores.iter().sum::<f32>() / scores.len() as f32;
        let variance = scores.iter().map(|s| (s - avg).powi(2)).sum::<f32>() / scores.len() as f32;
        let std_dev = variance.sqrt();
        (1.0 - std_dev).max(0.0)
    }
    fn score_experience_accumulation(&self, cards: &[ExperienceCard]) -> f32 {
        if cards.is_empty() { return 0.0; }
        cards.iter().filter(|c| c.score > 0.7).count() as f32 / cards.len() as f32
    }
}
```

### 15.2 自我评估面板 + 经验卡片可视化（TUI扩展）

```rust
// chimera-tui/src/panels/self_assessment.rs
#![forbid(unsafe_code)]

pub struct SelfAssessmentPanel { auditor: RuntimeAuditor }

impl Panel for SelfAssessmentPanel {
    fn render(&self, frame: &mut Frame, area: Rect) {
        let assessment = self.auditor.evaluate();
        let chunks = Layout::default().direction(Direction::Vertical)
            .constraints([Constraint::Length(3); 8]).split(area);
        self.render_gauge(frame, chunks[0], "任务理解", assessment.task_comprehension);
        self.render_gauge(frame, chunks[1], "可控执行", assessment.controllable_execution);
        self.render_gauge(frame, chunks[2], "改动验证", assessment.change_verification);
        self.render_gauge(frame, chunks[3], "可靠交付", assessment.reliable_delivery);
        self.render_gauge(frame, chunks[4], "经验沉淀", assessment.experience_accumulation);
        self.render_gauge(frame, chunks[5], "三因子-质量", assessment.three_factor_quality);
        self.render_gauge(frame, chunks[6], "三因子-进度", assessment.three_factor_progress);
        self.render_gauge(frame, chunks[7], "三因子-新颖", assessment.three_factor_novelty);
    }
}

// chimera-tui/src/panels/experience_card_viz.rs
#![forbid(unsafe_code)]

pub struct ExperienceCardVizPanel { card_system: ExperienceCardSystem }

impl Panel for ExperienceCardVizPanel {
    fn render(&self, frame: &mut Frame, area: Rect) {
        let stats = self.card_system.get_global_stats();
        let text = format!(
            "总卡片: {} | 已评估: {} | 唯一错误: {}
方法分布: {:?}
最佳分数: {:.2} | 平均分数: {:.2}",
            stats.total_nodes, stats.total_evaluated, stats.unique_errors,
            stats.method_distribution, stats.best_score, stats.average_score
        );
        frame.render_widget(Paragraph::new(text), area);
    }
}
```

### 15.3 上下文注入策略面板（TencentDB优化）

动态卡片放用户消息前（每轮都变），人格摘要放系统提示末尾（几轮才变，利用缓存）。

```rust
// chimera-tui/src/panels/injection_strategy.rs
#![forbid(unsafe_code)]

pub struct InjectionStrategyPanel {
    current_dynamic_cards: Vec<AtomicMemoryCard>,
    current_persona_summary: Option<PersonaSummary>,
    cache_hit_rate: f32,
    token_savings: u32,
}

impl Panel for InjectionStrategyPanel {
    fn render(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage(40), Constraint::Percentage(40), Constraint::Percentage(20)]).split(area);
        let dynamic_block = Block::default().title("动态卡片（用户消息前）").borders(Borders::ALL);
        let dynamic_text = self.current_dynamic_cards.iter().map(|c| format!("[{}] {}: {}", c.card_type, c.scene, c.content)).collect::<Vec<_>>().join("
");
        frame.render_widget(Paragraph::new(dynamic_text).block(dynamic_block), chunks[0]);

        let persona_block = Block::default().title("人格摘要（系统提示末尾）").borders(Borders::ALL);
        let persona_text = self.current_persona_summary.as_ref().map(|p| p.summary.clone()).unwrap_or_else(|| "无".to_string());
        frame.render_widget(Paragraph::new(persona_text).block(persona_block), chunks[1]);

        let stats_block = Block::default().title("缓存统计").borders(Borders::ALL);
        let stats_text = format!("缓存命中率: {:.1}%
Token节省: {}
策略: 动态卡片每轮更新 | 人格摘要复用缓存", self.cache_hit_rate * 100.0, self.token_savings);
        frame.render_widget(Paragraph::new(stats_text).block(stats_block), chunks[2]);
    }
}
```

---


## 16. 跨层协同：评估-进化-记忆-技能四位一体闭环

### 16.1 数据流闭环

```
L7 Execution (PVL验证/算子执行)
    → 生成ExperienceCard（L7 experience-card-generator）
    → 发布到L1 ExperienceCardBus（分级投递：Critical/Broadcast）
    → L3 Storage持久化（SQLite + 复合索引）
    → L2 Memory按需合成（OnDemandSynthesizer：祖先V + 兄弟H）
    → L5 Knowledge三因子选择（ThreeFactorSelector：UCB+Softmax+冷却）
    → L6 Router算子路由（OperatorRouter：Greedy/ThreeFactor/UCB/Cooling）
    → L9 Quest搜索树扩展（SearchTreeManager：扩展/选择/剪枝）
    → L8 Parliament变体审议（ThreeFactorAdjudicator：三角色裁决）
    → L10 Interface可视化（SelfAssessmentPanel：五维度+三因子）
    → 反馈到L7 Execution（闭环）
```

### 16.2 三因子选择闭环

```
L5 Knowledge (ThreeFactorSelector)
    → 从L3 Storage检索候选卡片（query_by_three_factor）
    → 计算Quality + Progress + Novelty（归一化）
    → UCB bonus（未探索节点强制加分）
    → 冷却因子（随时间降低探索）
    → Softmax概率采样（保留随机性）
    → 更新访问计数
    → L6 Router选择算子（基于选择记录历史）
    → L7 Execution执行算子
    → 生成新卡片 → 回到L5
```

### 16.3 按需记忆合成闭环

```
L6 Router (OperatorRouter) 选择算子
    → L2 Memory (OnDemandSynthesizer) 按需合成
    → 检索祖先(V) + 兄弟(H)
    → 根据算子类型选择相关节点：
       Draft → 高质量祖先（3个）
       Improve → 高进度祖先 + 成功兄弟
       Debug → 相同错误签名的兄弟（关键！）
       Crossover → 高新颖性兄弟（2个）
    → 合成精简上下文
    → 估算Token数（用于控制上下文长度）
    → 如果超过阈值，进一步压缩
    → 传递给L7 Execution
```

### 16.4 跨层事件协议

| 事件类型 | 产生层 | 消费层 | 优先级 | 说明 |
|---------|--------|--------|--------|------|
| ExperienceCardGenerated | L7 | L1, L3, L5 | Normal | 新经验卡片生成 |
| HighScoreCard | L7 | L1, L3, L8 | Critical | 高分卡片（score>0.8） |
| ErrorSignatureMatched | L4 | L2, L5 | Critical | 错误签名匹配成功 |
| ParentSelected | L5 | L6, L9 | Normal | 三因子父本选择结果 |
| OperatorExecuted | L7 | L6, L9 | Normal | 算子执行完成 |
| VariantApproved | L8 | L5, L6 | Normal | 变体审议通过 |
| StopRuling | L8 | L9 | Critical | 停止策略裁决 |
| AssessmentUpdated | L10 | L9 | Low | 自我评估更新 |
| SegmentValidated | L7 | L1 | Normal | Segment验证完成 |
| TokenLedgerRecorded | L1 | L3 | Normal | Token证据记录 |

### 16.5 跨层奖励传播（二十三论文融合版）

```
L10 Interface: 用户满意度 + 24h返回率 + 五维度评估 + 缓存命中率 [Ω₆ Reuse + Ω₈ Assess + Ω₁₀ Card + Ω₁₁ Synthesize]
    ↓ 反向传播
L9 Quest: 任务完成率 + 停止策略正确性 + 搜索树效率 + 长任务地图效率 + 保留最佳 [Ω₃ Evolve + Ω₉ Preserve]
    ↓ 反向传播
L8 Parliament: 决策正确率 + Shapley贡献 + 变体审议成功率 + 冲突仲裁成功率 + 停止策略准确率 [Ω₅ Credit]
    ↓ 反向传播
L7 Execution: 验证通过率 + Process-Score + Segment-aware验证 + 经验卡片生成率 + 熵加权效率 [Ω₄ Event + Ω₁₀ Card]
    ↓ 反向传播
L6 Router: 命中率 + Skills加载效率 + 算子选择效率 + 个性化匹配 + 工具schema裁剪效率 + 三因子选择效率 [Ω₁ Sparse + Ω₁₁ Synthesize]
    ↓ 反向传播
L5 Knowledge: 知识复用率 + SkillGraph密度 + AEGIS收敛 + 跨域迁移增益 + Skill生命周期效率 + 双层经验库命中率 [Ω₃+Ω₆]
    ↓ 反向传播
L4 Security: 安全拦截率(正) + 误拦截率(负) + Paddock-Sandbox效率 + AutoBuilder成功率 + 错误签名收集率 [硬约束]
    ↓ 反向传播（仅负奖励信号）
L3 Storage: 缓存命中率 + 存储成本 + 采样效率 + 金字塔层级效率 + 三因子索引效率 [Ω₂ Compress]
    ↓ 反向传播
L2 Memory: 记忆召回率 + Mem-π准确率 + 检索三方式命中率 + 注入策略效率 + 双信号回填质量 + 按需合成效率 + HiLS注意力效率 [Ω₂ Compress + Ω₁₁ Synthesize]
    ↓ 反向传播
L1 Core: CLV质量 + Event Bus吞吐量 + Token Ledger完整性 + Segment-aware PER效率 + 统计学习接口效率 [Ω₄ Event]
    ↓ 反向传播
L0 Contracts: 平台接地验证通过率 + 行为契约遵守率 + TokenEvidence格式合规 + MemoryPyramid格式合规 + 经验卡片契约合规 [Ω₈ Assess]
```

---

## 17. RL架构预留：Rust侧接口设计 · Python侧v4.0计划

### 17.1 设计原则

1. **接口同构**：Rust侧的`StatLearningPolicy`与RL的`Policy`接口同构（State→Action）
2. **数据兼容**：所有统计学习历史可导出为`RLTrajectory`（状态-动作-奖励序列）
3. **策略可替换**：v3.4.0的`RulePolicyFallback`可在v4.0无缝替换为`GrpcRLClient`
4. **分层独立**：每层可独立升级RL策略，不影响其他层

### 17.2 Rust侧预留接口

| 组件 | v3.4.0实现 | v4.0升级路径 |
|------|-----------|-------------|
| `StatLearningPolicy` | SlidingWindowPolicy（EMA）/ UCBPolicy | 替换为ONNX策略网络 |
| `RLClient::predict()` | 规则策略回退 | gRPC调用Python RL Service |
| `RLClient::report_experience()` | 本地SQLite存储 | 发送到Python训练服务 |
| `RLClient::sync_policy()` | 加载本地JSON配置 | 从Python服务拉取ONNX模型 |
| `ExperienceCard::export_trajectory()` | 本地序列化 | 上传到训练集群 |
| `ThreeFactorSelector` | UCB+Softmax+冷却（统计） | 神经网络学习权重 |
| `OperatorRouter` | Greedy/ThreeFactor/UCB/Cooling | PPO Actor网络 |
| `DynamicVerifier` | 风险自适应+历史EMA | 神经网络自适应验证深度 |

### 17.3 Python侧v4.0计划（仅保留规划，禁止实施）

| 组件 | v3.4.0状态 | v4.0计划 | 依赖 |
|------|-----------|---------|------|
| GRPO训练器 | 预留接口 | `rl_service/trainers/grpo.py` | PyTorch + TRL |
| PPO训练器 | 预留接口 | `rl_service/trainers/ppo.py` | PyTorch + Stable-Baselines3 |
| MAPPO训练器 | 预留接口 | `rl_service/trainers/mappo.py` | PyTorch + MAPPO实现 |
| DQN训练器 | 预留接口 | `rl_service/trainers/dqn.py` | PyTorch |
| GTPO训练器 | 预留接口 | `rl_service/trainers/gtpo.py` | PyTorch + 自定义 |
| gRPC服务 | 预留proto | `rl_service/main.py` | tonic (Rust) + grpcio (Python) |
| ONNX导出 | 预留接口 | `rl_service/export.py` | ONNX Runtime |
| 经验回放 | Rust侧SQLite | `rl_service/replay_buffer.py` | 自定义PER实现 |
| Dressage Proxy | 预留接口 | `rl_service/dressage_proxy.py` | OpenForge风格 |
| Segment-aware Trainer | 预留接口 | `rl_service/segment_trainer.py` | 自定义 |

### 17.4 升级路径

```
v3.4.0 (Rust侧完整实现)
    │
    ├── 统计学习：SlidingWindow + UCB + EMA + Softmax + 冷却
    ├── 规则策略：RulePolicyFallback
    ├── 数据收集：本地SQLite存储经验轨迹
    ├── 经验卡片：全链路生成+索引+可视化
    ├── 三因子选择：生产级UCB+Softmax
    │
    ↓ 运行数月，积累足够轨迹数据（目标：10万+经验卡片）

v3.9.0 (RL预训练)
    │
    ├── 导出Rust侧轨迹 → Python训练服务
    ├── 离线训练初始策略网络
    ├── 验证策略网络效果 > 统计策略
    │
    ↓ 策略网络效果优于统计策略

v4.0.0 (RL在线接入)
    │
    ├── 启动Python RL Service（gRPC）
    ├── Rust侧加载ONNX策略网络
    ├── 在线PPO/GRPO训练
    ├── 策略网络定期同步
    └── 十层策略全面神经网络化
```

---

## 18. 安全与熔断：十层防御体系 + 降级路线

### 18.1 十层防御矩阵

| 层级 | 防御机制 | 类型 | Rust侧实现 | RL可优化范围 | 硬约束 |
|------|---------|------|-----------|-------------|--------|
| L10 | 自我评估面板 + Runtime Auditor + 经验卡片可视化 | 评估 | ✅ 五维度+三因子 | 评估阈值 | ❌ |
| L9 | 停止策略 + 保留最佳 + 搜索树剪枝 + 长任务地图 | 策略 | ✅ 规则阈值 | 停止条件 | ❌ |
| L8 | 变体审议 + 三因子裁决 + 行为定位 + 冲突仲裁 | 治理 | ✅ 三角色投票 | 裁决阈值 | ❌ |
| L7 | PVL算子化验证 + 动态验证深度 + Segment-aware | 验证 | ✅ 风险自适应 | 验证深度 | ❌ |
| L6 | OSA稀疏掩码 + 算子路由 + Skills渐进加载 + 工具裁剪 | 预防 | ✅ 已落地 | 路由策略 | ❌ |
| L5 | AEGIS Critic + 变体隔离 + 双层经验库 + Skill生命周期 | 约束 | ✅ 规则引擎 | 选择策略 | ❌ |
| L4 | SecCore沙箱 + Decay Engine + QEEP + AutoBuilder + Paddock-Sandbox | **硬约束** | ✅ 不可覆盖 | **不可优化** | ✅ |
| L3 | 经验库完整性校验 + 分层存储 + 三因子索引 | 数据 | ✅ SQLite+索引 | 采样策略 | ❌ |
| L2 | 按需记忆合成 + 记忆图谱 + 检索降级链 + HiLS | 记忆 | ✅ 异步合成 | 合成策略 | ❌ |
| L1 | Event Bus Critical通道 + Token Ledger + Segment PER | 通信 | ✅ 双通道 | 事件路由 | ❌ |
| L0 | 契约不可变 + 类型安全 + 经验卡片契约 + Token证据 | 基础 | ✅ 零依赖 | 不可变更 | ✅ |

### 18.2 绝对红线（融合态，不可协商）

```rust
pub const FUSION_UNLEARNABLE_RULES: &[&str] = &[
    "seccomp沙箱策略不可降低",
    "Merkle审计链完整性不可篡改",
    "零孤儿调用保证不可绕过",
    "最小权限底线不可突破",
    "#![forbid(unsafe_code)]不可移除",
    "BudgetExceeded severity = Critical不可降级",
    "安全拦截决策不可由RL替代",
    "R2形式化验证器落地前GSOE进化路径冻结(ADR-042)",
    "Variant性能退化不可超过25%(VariantContract约束)",
    "PER缓冲区不可被Python侧直接修改(只读gRPC)",
    "Token Ledger不可丢失（训练证据完整性）",
    "Segment parent_traj_id不可篡改（轨迹身份一致性）",
    "Anchor segment终局reward不可被非anchor覆盖",
    "Paddock不可依赖Sandbox内部实现（解耦红线）",
    "检索降级链不可阻塞对话主流程（可用性底线）",
    "经验卡片写入后不可变（版本化存储）",
    "三因子评分为纯函数无副作用（输入确定则输出确定）",
    "六类状态反馈必须全链路追踪（Success/Error/MissingCode/NoSubmit/ScoreFailed/Timeout）",
    "Rust侧零运行时Python依赖（铁律1）",
];
```

### 18.3 熔断机制

| 熔断条件 | 触发层 | 动作 | 恢复策略 |
|---------|--------|------|---------|
| 连续10次Error状态 | L7 | 强制切换Debug算子 | 成功修复后自动恢复 |
| 经验卡片生成率<10% | L7 | 告警并降级为手动模式 | 人工介入后恢复 |
| 变体审议连续Reject | L8 | 暂停新变体引入 | 调整阈值后重试 |
| 搜索树深度>max_depth | L9 | 强制剪枝 | 自动执行 |
| 停止策略触发Stop | L9 | 保留最佳并终止任务 | 新任务自动恢复 |
| 存储层查询超时 | L3 | 回退到热层缓存 | 查询恢复后自动恢复 |
| 沙箱逃逸检测 | L4 | **立即终止所有任务** | 需人工审计后恢复 |
| 32K上下文segment过多 | L2 | 强制切换64K基线 | 自动执行 |
| 工具schema过度裁剪 | L6 | 白名单保护必要工具 | 自动恢复 |
| 长任务地图外部存储丢失 | L9 | 本地副本+冗余存储 | 自动恢复 |
| 检索降级链过度降级 | L2 | 连续10轮返回空则告警 | 人工检查 |
| Skill生命周期过于严格 | L5 | 激活率<30%调整阈值 | 自动调整 |
| Token Ledger丢失 | L1 | 本地WAL+远程备份 | 自动恢复 |
| Blackbox Agent无token返回 | L1 | 降级到文本级证据 | 自动降级 |
| HiLS注意力维度不匹配 | L2 | 保留fallback全注意力 | 渐进式替换 |

---

## 19. 实施路线图：v2.26.0 → v3.3.0 → v3.4.0

### Phase 0：契约层（Week 1）

| 任务 | 交付物 | 来源 | 验收标准 |
|------|--------|------|---------|
| 经验卡片类型 | `nexus-contracts/src/experience_card.rs` | OpenMLE | 完整ExperienceCard+AtomicOperator+ThreeFactorScore+ErrorSignature+ExecutionStatus |
| Token证据契约 | `nexus-contracts/src/token_evidence.rs` | Dressage | TokenLedgerEntry+SegmentMetadata+SegmentCreationReason |
| 记忆金字塔契约 | `nexus-contracts/src/memory_pyramid.rs` | MSCE+TencentDB | MemoryPyramidLevel+AtomicMemoryCard+SceneBlock+PersonaSummary |
| Skill生命周期契约 | `nexus-contracts/src/skill_lifecycle.rs` | MSCE | SkillLifecycleState+SkillLifecycleContract |
| 六维控制面 | `nexus-contracts/src/six_dimensions.rs` | MemoHarness | D1-D6完整配置结构，含OpenMLE扩展字段 |
| RL预留钩子 | `nexus-contracts/src/rl_hooks.rs` | RL预留 | RLHook trait + SerializedPolicy + RLTrajectory |
| OmniMessage预留 | `nexus-contracts/src/omni_message.rs` | PenguinHarness | 统一消息协议枚举 |

### Phase 1：核心层（Week 2）

| 任务 | 交付物 | 来源 | 验收标准 |
|------|--------|------|---------|
| 经验卡片Event Bus | `event-bus/src/experience_card_bus.rs` | OpenMLE | 双通道（Normal+Critical）+ 四索引（task/node/factor/error） |
| Segment-aware PER | `event-bus/src/segment_per.rs` | Dressage | 轨迹分段+prompt-equal denominator+anchor reward |
| Token Ledger | `event-bus/src/token_ledger.rs` | Dressage | 记录input/output IDs/logprobs/mask/version |
| 统计学习接口层 | `nexus-core/src/stat_learning.rs` | RL预留 | SlidingWindowPolicy + UCBPolicy，可导出RLTrajectory |
| RL客户端骨架 | `rl-client/src/lib.rs` | RL预留 | RulePolicyFallback默认实现 + GrpcRLClient预留结构 |

### Phase 2：记忆+存储层（Week 3-4）

| 任务 | 交付物 | 来源 | 验收标准 |
|------|--------|------|---------|
| 经验卡片系统 | `experience-card-system/src/lib.rs` | OpenMLE | 全局经验板 + 三因子父本选择（UCB+Softmax+冷却） |
| 按需记忆合成 | `on-demand-synthesizer/src/lib.rs` | OpenMLE | 算子类型感知合成 + Token估算 + 懒加载 |
| 双层经验库 | `dual-experience-bank/src/lib.rs` | MemoHarness | 自动蒸馏 + 案例/全局双层检索 |
| 经验卡片存储 | `experience-card-storage/src/lib.rs` | OpenMLE | SQLite + 5个复合索引 + 热层缓存 |
| 金字塔存储映射 | `cmt-tiering/src/pyramid_storage.rs` | TencentDB | L0-L3映射到热/温/冷/冰 |
| HiLS-Attention | `hils-attention/src/lib.rs` | HiLS | 块间+块内softmax + landmark token |
| 检索三方式 | `mlc-engine/src/retrieval_three_way.rs` | TencentDB | 字面+语义+混合 + 降级链 |
| 注入策略 | `hcw-window/src/injection_strategy.rs` | TencentDB | 动态卡片+人格摘要+缓存统计 |

### Phase 3：安全+知识层（Week 5-6）

| 任务 | 交付物 | 来源 | 验收标准 |
|------|--------|------|---------|
| 错误签名收集器 | `error-signature-collector/src/lib.rs` | OpenMLE | 5种已知模式 + 通用回退 + 频率统计 |
| AutoBuilder | `auto-builder/src/lib.rs` | 快手 | 双智能体循环 + 六类状态反馈 |
| Paddock-Sandbox解耦 | `seccore/src/paddock_sandbox.rs` | Dressage | Paddock+SandboxProvider分离 |
| 四套原子算子 | `four-operators/src/lib.rs` | OpenMLE | Draft/Improve/Debug/Crossover完整实现 |
| 三因子选择器 | `three-factor-selector/src/lib.rs` | OpenMLE | UCB + Softmax + 冷却 + 访问计数 |
| AEGIS-GSOE | `aegis-gsoe/src/lib.rs` | 小米 | 四阶段流水线（Digester→Planner→Evolver→Critic） |
| 变体隔离池 | `variant-pool/src/lib.rs` | 小米 | 统计路由 + EMA更新 + 淘汰机制 |
| 保留历史最佳 | `checkpoint-preserver/src/lib.rs` | RSIBench | 任务级最佳保留 + 自动更新 |
| MSCE集成 | `msce-integration/src/lib.rs` | MSCE | 三层记忆融合 + 双信号价值回填 |
| Skill生命周期 | `skill-graph/src/lifecycle.rs` | MSCE | 试用期→激活→归档状态机 |

### Phase 4：路由+执行层（Week 7-8）

| 任务 | 交付物 | 来源 | 验收标准 |
|------|--------|------|---------|
| Skills渐进加载 | `skills-progressive-loader/src/lib.rs` | PenguinHarness | Index First + Body on Demand + 相似度过滤 |
| 算子路由器 | `operator-router/src/lib.rs` | OpenMLE | 4种策略（Greedy/ThreeFactor/UCB/Cooling） |
| 工具schema裁剪 | `osa-coordinator/src/tool_pruning.rs` | Dressage | 基于频率动态裁剪 + 白名单保护 |
| 经验卡片生成器 | `experience-card-generator/src/lib.rs` | OpenMLE | PVL结果→卡片转换 + 三因子计算 + 错误签名提取 |
| Process-Score | `process-score-calculator/src/lib.rs` | 快手 | 九维度规则评分 + 加权综合 |
| 动态验证深度 | `dynamic-verification-depth/src/lib.rs` | 快手 | 风险自适应 + 历史EMA更新 |
| 熵加权统计 | `process-score-calculator/src/entropy_weighted.rs` | OpenMLE | 熵加权评分 |
| Segment-aware验证 | `pvl-layer/src/segment_validation.rs` | Dressage | 轨迹分段验证 + reward广播 |

### Phase 5：议会+任务层（Week 9-10）

| 任务 | 交付物 | 来源 | 验收标准 |
|------|--------|------|---------|
| 变体审议 | `parliament/src/variant_parliament.rs` | 小米 | 三角色+烟雾测试 |
| 三因子裁决器 | `three-factor-adjudicator/src/lib.rs` | OpenMLE | 三角色投票 + 停止策略裁决 |
| 冲突仲裁 | `parliament/src/conflict_arbitration.rs` | TencentDB | 候选召回→模型判断 |
| 行为定位 | `parliament/src/behavior_localization.rs` | 腾讯 | L1→L2→L3自动导航 |
| 搜索树管理 | `search-tree-manager/src/lib.rs` | OpenMLE | 扩展/选择/剪枝/最优路径回溯 |
| 长任务地图 | `quest-engine/src/long_task_map.rs` | TencentDB | 外部存储+上下文注入 |
| 长时程信用分配 | `long-term-credit-assigner/src/lib.rs` | SHARP | 统计版信用分配 |
| Ambient Mode | `quest-engine/src/ambient_mode.rs` | jcode | 后台常驻+定期整理 |

### Phase 6：接口层+集成（Week 11-12）

| 任务 | 交付物 | 来源 | 验收标准 |
|------|--------|------|---------|
| Runtime Auditor | `runtime-auditor/src/lib.rs` | Qoder | 五维度评估 + 改动验证评分 |
| 自我评估面板 | `chimera-tui/src/panels/self_assessment.rs` | Qoder | ratatui Gauge组件 + 实时更新 |
| 经验卡片可视化 | `chimera-tui/src/panels/experience_card_viz.rs` | OpenMLE | 全局统计+方法分布+错误聚类 |
| 注入策略面板 | `chimera-tui/src/panels/injection_strategy.rs` | TencentDB | 动态卡片+人格摘要+缓存统计 |
| 跨层协同测试 | `tests/cross_layer_integration.rs` | — | 经验卡片流端到端测试 |
| 性能基准测试 | `benches/experience_card_flow.rs` | — | 1000卡片/秒生成+存储+检索 |
| RL预留接口验证 | `tests/rl_hook_compatibility.rs` | — | 轨迹导出+策略加载兼容性 |
| 全量回归 | `cargo test --workspace` | — | 9954+ tests全绿，0破坏现有测试 |

### Phase 7：稳定化与文档（Week 13-14）

| 任务 | 交付物 | 验收标准 |
|------|--------|---------|
| ADR-077~095落档 | `docs/architecture/ADR-0xx.md` | 16份新增ADR |
| NexusEvent扩展至150变体 | `event-bus/src/types.rs` | 新增14个经验卡片相关事件 |
| 三方一致性验证 | `scripts/check_doc_consistency.ps1` | Cargo.toml/CHANGELOG/CODE_WIKI一致 |
| 压力测试 | `cargo test --workspace --release -- --ignored` | 1000 episode无崩溃 |
| 版本发布 | `v3.3.0-omega` | 全量53 crate `cargo check --workspace` |

### Phase 8：v3.4.0融合终版（Week 15-16）

| 任务 | 交付物 | 验收标准 |
|------|--------|---------|
| 二十三论文融合文档 | 本文档 | 十四+六+十三论文全映射 |
| 跨论文交叉验证矩阵 | §2.2 | 23篇论文×Chimera设计完整覆盖 |
| 性能优化 | 全crate | 经验卡片流<50ms端到端延迟 |
| 最终回归 | `cargo test --workspace` | 11000+ tests全绿 |
| 发布 | `v3.4.0-omega` | 53 crate，150 NexusEvent，80+ ADR |

---

## 20. 附录

### 20.1 二十三篇论文 × Chimera 完整映射矩阵

| Chimera v3.4.0设计 | 1郝 | 2CMU | 3RUC | 4北大 | 5jcode | 6腾讯 | 7快手 | 8Qoder | 9小米 | 10微软 | 11RSI | 12Penguin | 13Memo | 14清华 | 15Dressage | 16RSIBench | 17Qoder4 | 18MSCE | 19HiLS | 20TencentDB | 验证 |
|-------------------|-----|------|------|-------|--------|-------|-------|--------|-------|--------|-------|-----------|--------|--------|-----------|-----------|----------|--------|--------|-------------|------|
| **经验卡片系统** | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | **新增** |
| **三因子父本选择** | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | **新增** |
| **按需记忆合成** | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | **新增** |
| **四套原子算子** | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | **新增** |
| **六类状态反馈** | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | **新增** |
| **动态奖励归一化** | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | **新增** |
| **熵加权统计** | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | **新增** |
| **统计学习接口层** | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | **新增** |
| **Token级证据** | — | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | **新增** |
| **Segment-aware** | — | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | **新增** |
| **Paddock-Sandbox** | — | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | **新增** |
| **记忆金字塔** | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | **✅** | **新增** |
| **Skill生命周期** | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | **新增** |
| **双信号价值回填** | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | **新增** |
| **HiLS注意力** | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | **新增** |
| **检索三方式** | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | **新增** |
| **注入策略** | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | **新增** |
| **冲突仲裁** | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | **新增** |
| **长任务地图** | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | **新增** |
| **Process-Score** | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | — | — | — | — | — | — | — | **新增** |
| **AEGIS引擎** | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | — | — | — | — | — | **新增** |
| **变体隔离** | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | — | — | — | — | — | **新增** |
| **OpenForge-Proxy** | — | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | — | — | — | — | **新增** |
| **双层经验库** | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | — | **新增** |
| **Skills渐进加载** | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | — | — | **新增** |
| **自我评估** | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | — | — | — | — | — | — | **新增** |
| **保留历史最佳** | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | — | — | — | **新增** |
| **OmniMessage** | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | — | — | **新增** |
| **停止策略** | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | — | — | — | **新增** |
| **六维控制面** | — | — | — | — | — | — | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | — | **新增** |
| **行为定位** | — | — | — | — | — | **✅** | — | — | — | — | — | — | — | — | — | — | — | — | — | — | **新增** |
| **AutoBuilder** | — | — | — | — | — | — | **✅** | — | — | — | — | — | — | — | — | — | — | — | — | — | **新增** |
| 十层分层架构 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | **20/20** |
| Event Bus | ✅ | — | ✅ | — | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 19/20 |
| Parliament | ✅ | ✅ | ✅ | — | — | ✅ | ✅ | ✅ | ✅ | — | ✅ | — | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 17/20 |
| Quest DAG | ✅ | — | ✅ | ✅ | ✅ | — | ✅ | ✅ | ✅ | — | ✅ | — | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 17/20 |
| PVL验证 | ✅ | — | ✅ | ✅ | — | ✅ | ✅ | ✅ | ✅ | — | ✅ | — | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 17/20 |
| MLC记忆 | ✅ | — | ✅ | — | ✅ | — | ✅ | ✅ | ✅ | — | ✅ | — | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 16/20 |
| GSOE进化 | — | ✅ | ✅ | — | ✅ | — | ✅ | ✅ | ✅ | — | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 18/20 |
| SkillGraph | — | — | ✅ | ✅ | — | — | ✅ | ✅ | ✅ | — | — | — | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 15/20 |

### 20.2 新增 ADR 清单（v3.4.0融合态）

| ADR | 主题 | 状态 |
|-----|------|------|
| ADR-077 | TokenLedger契约: input/output IDs, logprobs, mask, version | 提议 |
| ADR-078 | SegmentMetadata契约: parent_traj_id, segment_index, anchor, creation_reason | 提议 |
| ADR-079 | MemoryPyramid契约: L0 Raw/L1 Atomic/L2 Scene/L3 Persona | 提议 |
| ADR-080 | SkillLifecycle契约: probationary→active→archived | 提议 |
| ADR-081 | MSCE L1 Trace提取: 状态/动作/观察/反思/价值 | 提议 |
| ADR-082 | MSCE L2 Policy归纳: 触发器/过程/验证/边界/增益 | 提议 |
| ADR-083 | MSCE双信号价值回填: Vt = αtRt + (1-αt)γVt+1 | 提议 |
| ADR-084 | HiLS-Attention集成: 块间+块内softmax, landmark token | 提议 |
| ADR-085 | TencentDB检索三方式: 字面+语义+混合 | 提议 |
| ADR-086 | TencentDB注入策略: 用户消息前动态/系统提示末尾稳定 | 提议 |
| ADR-087 | TencentDB冲突仲裁: 候选召回→模型判断 | 提议 |
| ADR-088 | TencentDB长任务地图: 详细过程转存外部文件 | 提议 |
| ADR-089 | Dressage工具schema裁剪: 基于使用频率动态裁剪 | 提议 |
| ADR-090 | Dressage Paddock-Sandbox解耦: what-to-do vs where-it-runs | 提议 |
| ADR-091 | Dressage Blackbox Agent支持: HTTP/CLI agent适配 | 提议 |
| ADR-092 | 云端编排路线图: Forward Mode v0.1 (Phase 8) | 计划 |
| ADR-093 | OpenMLE经验卡片: 结构化卡片+三因子+错误签名 | 提议 |
| ADR-094 | OpenMLE三因子父本选择: UCB+Softmax+冷却 | 提议 |
| ADR-095 | OpenMLE按需记忆合成: 懒加载祖先+兄弟 | 提议 |

### 20.3 版本历史（融合态）

| 版本 | 日期 | 主要里程碑 |
|------|------|-----------|
| v2.11.0-omega | 2026-07-31 | 基线：L8 Parliament深度优化第二轮，37 crate全绿 |
| v2.20.0-omega | 2026-08-03 | PROBE HCW-Sparse深度优化完整闭环，38 crate，8455 tests |
| v2.21.0-omega | 2026-08-04 | CLI LLM统一入口 |
| v2.22.0-omega | 2026-08-07 | MCA token效率深度优化 |
| v2.24.0-omega | 2026-08-08 | Phase 9三环循环元架构重组收尾 |
| v2.25.0-omega | 2026-08-08 | Milestone B全部交付 |
| **v2.26.0-omega** | **2026-08-11** | **Concord TUI重构 W0~W11全部收尾：53 slash命令/双轨会话/ApprovalMode/i18n，9954 passed/0 failed** |
| **v3.3.0-omega** | **目标: 2026-10-15** | **Rust侧八周冲刺：经验卡片系统+三因子选择+按需记忆合成+四套原子算子+双层经验库+AEGIS-GSOE+变体隔离+保留历史最佳+Process-Score+动态验证深度+Segment-aware+HiLS-Attention+检索三方式+注入策略+冲突仲裁+长任务地图+Ambient Mode+Runtime Auditor+自我评估面板** |
| **v3.4.0-omega** | **目标: 2026-11-30** | **融合终版：二十三篇论文全栈融合，53 crate，150 NexusEvent，80+ ADR，11000+ tests，经验卡片流生产级** |
| v4.0.0-omega | 目标: 2027-02-28 | Python RL Service接入：PPO/GRPO/MAPPO在线训练，策略网络全面替代统计规则 |

### 20.4 关键设计模式速查（CODE_WIKI融合）

| 模式 | 应用位置 | 说明 |
|------|---------|------|
| 枚举分发(Enum Dispatch) | chtc-bridge, operator-router | 避免Box<dyn Trait>动态分发开销 |
| Arc零拷贝共享 | 全crate | 异步任务间共享只读数据 |
| 双通道事件投递 | event-bus | Normal(broadcast) + Critical(mpsc fan-out) |
| spawn_blocking隔离 | repo-wiki, scc-cache等 | 所有rusqlite调用必须包装 |
| id_newtype!宏 | nexus-core | 类型安全，防止不同ID混用 |
| Top-K用select_nth_unstable | osa-coordinator, mlc-engine | O(n)替代O(n log n) |
| FuturesUnordered并发收集 | gqep-executor, pvl-layer | 优于join_all，减少内存占用 |
| publish_blocking()同步发布 | seccore, parliament | sync方法使用publish_blocking |
| 经验卡片不可变 | 全系统 | 写入后不可变，更新即新建版本 |
| 三因子纯函数 | three-factor-selector | 输入确定则输出确定，无副作用 |

### 20.5 工程红线与实战教训（CODE_WIKI融合）

| 红线 | 教训来源 | 说明 |
|------|---------|------|
| 禁止持锁.await | faae-router 4 Critical | DashMap/Mutex写锁跨await导致死锁 |
| rusqlite必须spawn_blocking | 79处遵循 | rusqlite非async，直接调用阻塞runtime |
| broadcast先subscribe再spawn | Week 6-7 | bus.subscribe()必须在tokio::spawn()之前 |
| BudgetExceeded severity = Critical | F-001修复 | 禁止降级，必须返回Critical |
| Critical安全事件用mpsc | efficiency-monitor | SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded |
| sqlite-vec禁用 | ADR-005 | 需unsafe，改内存KNN |
| Top-K用select_nth_unstable | 工程约定 | O(n)替代O(n log n) |
| f32禁止隐式转f64 | sesa-router | 0.4f32 as f64精度膨胀 |
| 经验卡片写入后不可变 | Rust铁律3 | 版本化存储，更新即新建 |
| 六类状态必须全链路追踪 | Rust铁律8 | Success/Error/MissingCode/NoSubmit/ScoreFailed/Timeout |

---

> **NEXUS-OMEGA Fusion v3.4.0** — Ω₁ Sparse · Ω₂ Compress · Ω₃ Evolve · Ω₄ Event · Ω₅ Credit · Ω₆ Reuse · Ω₇ Locate · Ω₈ Assess · Ω₉ Preserve · Ω₁₀ Card · Ω₁₁ Synthesize
>
> Chimera CLI v3.4.0-omega 统一架构设计与Rust侧实现规范 —— 二十三篇前沿论文融合权威版 · 2026-08-16
>
> **基线不可动摇，进化有迹可循，经验卡片驱动，记忆-技能协同进化，安全永不妥协。**
