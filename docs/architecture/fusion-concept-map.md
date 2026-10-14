# Fusion Concept Map（概念映射表）

> **文档头元信息**
>
> - **生成时点**：2026-09-02
> - **用途**：把三份根目录设计文档（`Chimera_CLI_v4.0_架构重构与算法优化一体化方案（六编重构终版）.md`、`Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md`、`Chimera_CLI_九源交叉融合彻底重构工程手册V1.0.md` 及 `Chimera_全模型亲和适配体系设计文档_v1.0.md`）的核心主张映射到**现行代码库**（crate 数以 `Cargo.toml` `workspace.members` 为准），作为「代码为准 + 映射表」融合落地的**唯一检索入口**。
> - **权威基线（指针制，本文件不登记计数）**：版本以 `Cargo.toml` `workspace.package.version` 为准，NexusEvent 以 `crates/event-bus/src/types.rs` 为准，可达性以 `bash scripts/check_crate_reachability.sh` 实时输出为准（2026-09-22 现测 = workspace 41 成员 / 生产可达 32 / 冻结孤岛 0 / feature-gated 9）。本文件**生成时点**为 v2.28.0-omega（当时记作 43 crates / 28 可达 + 14 孤岛 + 1 GATED / 144 事件）——**该组数字是历史点位，不得当现状读**。测试数以 `docs/reports/` 最新回归报告为准（生成时点登记 11587 passed / 0 failed，2026-09-02 重测）。
> - **原则**：路径一律经 Grep/Glob 实际核实，以代码为准；未独立成 crate/module 的概念标注「经 ADR 否决/合入」或「缺失」，**不编造路径**。

---

## 1. 概念映射表

> 列：**文档源/概念**、**所属文档章节**、**现行落地 crate/子模块/文件**（相对路径，均为 `d:\Chimera CLI\` 之下）、**落地状态**、**可行性**、**备注/ADR 引用**。
>
> 文档源缩写：
> - **V4** = `Chimera_CLI_v4.0_架构重构与算法优化一体化方案（六编重构终版）.md`
> - **V34** = `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md`
> - **9YS** = `Chimera_CLI_九源交叉融合彻底重构工程手册V1.0.md`
> - **CAF** = `Chimera_全模型亲和适配体系设计文档_v1.0.md`

| 文档源/概念 | 所属文档章节 | 现行落地 crate/子模块/文件 | 落地状态 | 可行性 | 备注/ADR 引用 |
|---|---|---|---|---|---|
| ComputeBridge / DetReduce（计算桥接/降维归约） | V4 §计算层 | `crates/nexus-core/src/compute/bridge.rs`、`crates/nexus-core/src/compute/reduce.rs`（`compute/` 下另有 `mod.rs`/`seam.rs`/`hts.rs`/`dispatch.rs`/`utilization.rs`） | 已落地 | 高 | — |
| 分片总线 + ShardedBus + CBF（跨层信用流/背压/段性能） | V34 §事件总线 | `crates/event-bus/src/shard.rs`（分片）、`credit_flow.rs`（CBF）、`backpressure.rs`（背压）、`segment_per.rs`（段性能） | 已落地 | 高 | broadcast + Critical mpsc 双通道见 `bus.rs::is_critical_mpsc_event()`；**变体数不在此登记**（原写“144 事件”为 2026-09-02 时点值，已陈旧；现值以 `crates/event-bus/src/types.rs` 为准） |
| CSC 四级压缩 + ThinkingPreserve（层次压缩+推理保留） | V34 §记忆压缩 | `crates/hcw-window/src/compressor.rs`、`crates/hcw-window/src/window.rs`（`HcwWindow`:90，四级窗口主体；无 pipeline.rs）、`crates/hcw-window/src/preserve.rs` | 已落地 | 高 | Ω₂-Compress；hcw 四级窗口 4K/32K/128K/1M |
| HiLS（层次长短期记忆调度） | V34 §记忆调度 | `crates/hcw-window/src/hils.rs` | 已落地 | 高 | 与 recall/ 子模块协同 |
| TSR × MoE（任务自路由×专家混合） | V4 §路由 | `crates/faae-router/src/tsr_moe.rs` + `crates/mas-sched/`（L9 调度面） | 已落地（TSR MoE）/部分（跨调度） | 中 | `mas-sched` = ADR-145，L9；`faae-router` 为 L6 |
| CBMR 微批写（会话存储） | V4 §存储 | `crates/session-store/`（`src/{writer,segment,tree,replay}.rs`） | 已落地 | 高 | `session-store` = ADR-141，L3 |
| PTC 并行工具协调 | V4 §工具编排 | `crates/nexus-contracts/src/tool_plan.rs`（`ToolPlan`/`ToolNode`/`ToolOp`/`guards`/`SideEffectDecl`） | 已落地 | 高 | 注：任务下发的默认提法「若不存在标注缺失」——经核实**存在**；契约层纯类型 |
| SER 两阶段检索（粗排+精排） | V34 §知识检索 | `crates/repo-wiki/src/search.rs`、`src/vector/{memory_knn_store,hnsw_store}.rs`、`src/fts.rs`、`src/agent_grep.rs`（repo-wiki 检索面） | 已落地 | 高 | Ω₆-Reuse；repo-wiki 既有 FTS5 + HNSW 两段式 |
| 三因子父本选择（进化父本） | V4 §进化 | `crates/gsoe-evolution/src/three_factor_selector.rs` | 已落地 | 高 | Ω₃-Evolve |
| AEGIS 四阶段（Planner/Evolver/Digester/Critic） | V34 §进化引擎 | `crates/gsoe-evolution/src/aegis/`（`mod.rs`/`planner.rs`/`evolver.rs`/`digester.rs`/`critic.rs`） | 已落地 | 高 | Ω₃-Evolve；AEGIS 四阶段引擎 |
| 按需记忆合成（生成式记忆） | V34 §记忆合成 | `crates/mlc-engine/src/on_demand_synthesizer.rs` | 已落地 | 高 | 经核实路径存在；Ω₂ Mem-π |
| 错误签名收集 | V4 §安全 | `crates/seccore/src/error_signature_collector.rs`（配套 `crates/seccore/tests/error_signature_collector_test.rs`） | 已落地 | 高 | 经核实路径存在 |
| Paddock-Sandbox 解耦（沙箱/OS 后端/执行策略） | V9YS §沙箱 | `crates/seccore/src/paddock_sandbox.rs`、`os_backend.rs`、`execpolicy.rs`（配套 `sandbox.rs`/`sandbox_wasm.rs`/`gvisor.rs`） | 已落地 | 高 | UNLEARNABLE 红线 1：seccomp 不可降 |
| 经验卡片（Experience Card） | V34 §经验沉淀 | `crates/event-bus/src/experience_card_bus.rs`（总线）+ `crates/pvl-layer/src/card_generator.rs`（卡片生成）+ `crates/nexus-contracts/src/experience_card.rs`（类型） | 已落地 | 高 | Ω₄-Event；三处成链（契约类型 → 生成 → 总线） |
| Token Ledger / GIP（Token 账本/生成-输入平衡） | V4 §信用 | `crates/event-bus/src/token_ledger.rs` + `crates/nexus-contracts/src/token_evidence.rs` | 已落地 | 高 | 经核实均存在；credit/credit_flow 协同 |
| MCSM 信号守恒（Multi-Channel Signal Module） | V34 §信号 | `crates/nexus-contracts/src/mcsm.rs` | 已落地 | 高 | 经核实路径存在；L0 契约 |
| LPA 分层提示词（分层 Prompt 架构） | V4 §提示 | `crates/chimera-cli/src/`（CLI 入口，分层提示词组装） | 部分 | 中 | L10 `chimera-cli`；未见独立 `lpa.rs` 模块（以 `crates/chimera-cli/src/` 目录为准，未发现独立文件时的兜底归因） |
| WI-01~34（工作项 1-34） | V4 §里程碑 | 五新 crate 与既有 crate 子模块：L10 `nexus-app-server`（WI-01，`src/{server,backend,sse,approval,transport,protocol,subagent_engine}.rs`）、L3 `session-store`（WI-02）、L9 `mas-sched`/`nexus-hook`、L7 `nexus-subagent`（`src/{auction,runtime,cancel}.rs`） | 已落地 | 高 | 38→43 crate 五新成员；ADR-141/145/146/148 |
| Runtime Auditor（五维证据纪律） | V34 §自评估 | `crates/efficiency-monitor/src/auditor.rs`（配套 `monitor.rs`/`dashboard.rs`/`collectors.rs`/`oscillation_detector.rs`） | 已落地 | 高 | Ω₈-Assess；Runtime Auditor 证据纪律 |
| Ω₁₀ / Ω₁₁（OMEGA 新增定律） | V4 §九定律扩展 | `docs/architecture/ADR-170-omega-11th-laws.md`（正式收录） | 已落地（ADR-170 收录） | 高 | **已由 ADR-170 收录（2026-09-02）**：Ω₁₀-Card / Ω₁₁-Synthesize 作为 Rust 侧扩展定律正式纳入（原"规划中无代码/ADR"表述失效）；守恒表述升级为"十一定律(九基座+两扩展)" |
| 记忆策略自适应（MinimalRecall→…→AggressivePruning） | V34 §记忆策略 | `crates/mlc-engine/src/memory_strategy_learner.rs`（+ `src/mem_con/controller.rs`） | 已落地 | 高 | 三重悖论免疫·记忆悖论 |
| 进化悖论 L4 形式化门 | V34 §进化 | `crates/gsoe-evolution/src/formal_gate.rs`、`src/formal/{mod,invariant_closure,critic_monotonicity,lineage_checker}.rs` | 已落地 | 高 | 进化悖论 L4 跃迁；R2 解冻影子期（ADR-053 rev4） |
| 信用分配 SHARP / 三元分解奖励 | CAF §信用 | `crates/parliament/src/sharp.rs`、`src/mappo.rs` | **模块已落地 / 生产零消费** | 高 | Ω₅-Credit；2026-10-09 v3.5 实测：两个类型仅在 `parliament/tests/mappo_sharp_e2e_test.rs` 被构造，全仓无生产装配点 ⇒ 本行不再写“已落地”单值（“导出/存在 ≠ 已接线”，终局原则 3） |
| 关键路径动态识别（行为定位） | CAF §定位 | `crates/parliament/src/critical_path.rs` | 已落地 | 高 | Ω₇-Locate |
| 变体隔离 + 停止策略 | CAF §保留 | `crates/parliament/src/variant_pool.rs`（`VariantPool`，导出 `lib.rs:202`；ADR-051 明示 L8 子模块不新建 crate）、`crates/gsoe-evolution/src/checkpoint_preserver.rs` | 已落地 | 高 | Ω₉-Preserve |
| 稀疏全维掩码 + 按需激活 | CAF §稀疏 | `crates/osa-coordinator/src/`、`crates/sesa-router/src/{activation,sparsity}.rs` | 已落地 | 高 | Ω₁-Sparse；sesa 稀疏激活 |
| 全局模型亲和（MCA 体系） | CAF §亲和 | `crates/mca-gateway/`（L10） | 部分（feature 门控） | 中 | ADR-065~068；ADR-160 标注为 feature 门控孤岛，默认 binary 不含 |

---

## 2. 未落地 / 缺失项清单

> 以下为文档设想但现行代码库（2026-09-22 实测 41 crate，权威源 `Cargo.toml` `workspace.members`；43 为 v2.28.0 时点值）中**无独立 crate 或独立模块**的项。均已按「经 ADR 否决并合入既有 crate」或「缺失」标注，**无编造路径**。

| 文档设想项 | 现状裁决 | 说明 |
|---|---|---|
| **nexus-moe-router**（独立 MoE 路由 crate） | 缺失（合入既有 crate） | 文中概念未独立成 crate；`tsr_moe` 落地于 `crates/faae-router/src/tsr_moe.rs`。未见虚拟 `nexus-moe-router` 层级属主。 |
| **nexus-sparse-attention**（独立稀疏注意力 crate） | 缺失（合入既有 crate） | 稀疏激活落地于 `crates/sesa-router/src/sparsity.rs` 与 `activation.rs`；无独立注意力 crate。 |
| **tool_plan** | 已落地（非缺失） | 经核实存在于 `crates/nexus-contracts/src/tool_plan.rs`（L0 契约，非独立 crate）。任务默认提法「若不存在标注缺失」——此处**已存在**。 |
| **on_demand_synthesizer** | 已落地（非缺失） | 经核实存在于 `crates/mlc-engine/src/on_demand_synthesizer.rs`。 |
| **error_signature_collector** | 已落地（非缺失） | 经核实存在于 `crates/seccore/src/error_signature_collector.rs`。 |
| **experience_card_bus** | 已落地（非缺失） | 经核实存在于 `crates/event-bus/src/experience_card_bus.rs`。 |
| **Ω₁₀ / Ω₁₁** | 非缺失（已由 ADR-170 正式收录） | **已由 ADR-170 收录（2026-09-02）**：Ω₁₀-Card / Ω₁₁-Synthesize 已作为 Rust 侧扩展定律落档并纳入"十一定律"守恒表述（原"无代码、无 ADR-161"规划态失效）。 |
| **lpa** 独立模块 | 缺失（合入 CLI） | 分层提示词组装归 `crates/chimera-cli/src/`，未见独立 `lpa.rs` 模块。 |
| **Paddock-Sandbox 独立进程** | 缺失（沙箱组件解耦） | 已组件化解耦（sandbox/os_backend/execpolicy/gvisor 分文件），但未作为独立进程/独立 crate 部署。 |

---

## 3. 使用说明与漂移校验

### 3.1 唯一检索入口用法
- **新增概念 → 查本表**：任何新设计概念落地前，须先在本表检索是否已有现存 crate/模块承接，避免重复建 crate。
- **文档声明 vs 代码真值**：本表「落地状态」以代码为准；文档章节号做指引，不含求真。**记忆/文档会陈旧**，引用路径前须再次 Grep/Glob 验证。

### 3.2 漂移校验
- **概念→代码漂移**：当某概念对应文件的 crate 隶属、路径、模块拆分任一变更时，须回扫本表更新「现行落地」列与该行的「落地状态」。
- **新增/否决概念**：一旦有文档设想项被新增为 crate/模块，须从 §2「未落地/缺失清单」移入 §1 映射表；一旦被 ADR 否决，须在 §2 标注「经 ADR-XXXX 否决」。
- **基线漂移**：crate 数、NexusEvent 变体数、ADR 主编号、测试数变化时，同步更新文头「权威基线」。

### 3.3 与 scripts/check_doc_consistency.ps1 联动（建议）
- 建议在 `scripts/check_doc_consistency.ps1` 中新增一条「概念映射文件路径存在性校验」：对 §1 映射表每个「已落地」行所引文件执行 `Test-Path`，任一缺失即非零退出，把本表纳入 CI 文档巡检（与 Cargo.toml member 校验同机制）。
- 现阶段（2026-08-30 口径）`check_doc_consistency.ps1` 覆盖 6 类 14 项；纳入本表可新增第 7 类「Fusion 概念↔代码」。接入前已有 ADR-166 对巡检口径的修正（本地 .ps1 为权威执行体），本表联动遵循同一口径。

---

## 4. ADR 临号作废回执（2026-10-08 / v3.5 W6-2，C-49）

背景：六份融合文档各自“预留”了 ADR 号段，但仓库同一编号已被**不同主题**的实档占用 ⇒ 照号施工会引用错档。本表是唯一的“临号↔实档”调和记录。

| 文档自拟号段 | 文档声称主题 | 仓库同实档异义（现取） | 处置 |
|---|---|---|---|
| V34 §20.2 ADR-077~095 | 上下文治理 / 渐进加载等 | `ADR-080-context-curation.md`、`ADR-081-context-curation.md`、`ADR-082-tui-hello-agent-tree.md`、`ADR-083-*`、`ADR-084-*` 均已是**另一主题**的实档 | 临号作废，不得引用 |
| 三源 v5 ADR-095~102 | 混合运行时 / 手写 SIMD / 事件元数据 V3 | `ADR-095-134-phase1-consolidation.md` 实为 Phase 1 汇总；且 `adr_index.md:6` 注明 ADR-086~094（十源融合裁决）登记于 V4 §2.5、本目录无文件 | 临号作废（方案内 SIMD/第三运行时另被 ADR-SIMD-001 / v3.0 §2.2 否决） |
| v6 §6.2 ADR-103~110 | GPU 落点（`compute/memory.rs`、`kernels/`、`gpu_*.rs`） | 这些路径全部不存在；GPU 已由 ADR-195 维持否决 + G-86 加速器族禁列机器门 | 临号作废（方案本体亦被否） |
| 十六源 §0.4/§3.2 ADR-170~182 | “J-01 引入 GPU，**supersede ADR-123**” | `ADR-170-omega-11th-laws.md` 实为 Ω 十一定律（同号异义） | 临号作废 + 主张被 ADR-195 affirm 否决 |

**取号与占位规则回执**：

1. 新 ADR 一律自 **ADR-202** 起取号（现测最大主编号 201），取号前先查 `docs/architecture/adr_index.md` 与磁盘实档，避免同号异义复发。
2. 占位号（`ADR-XXX-*`）落盘已被 `scripts/adr_index_sync.py` 的 `PLACEHOLDER` + `PLACEHOLDER_FREEZE` 拒绝（新占位号必红）；现存唯一 VOID 占位档 `ADR-XXX-inner-ring-bug-fix.md` 依该脚本自述协议（“最后一条引用退役才删盘并从冻结集移除”）**保留在盘**，且只可作历史取证，不可当有效 ADR 引用。
3. **覆盖度诚实声明**：本表 §1 只映射 **4 个文档源**（V4 / V34 / 9YS / CAF）；三源 v5、十六源 v3、CPU_GPU v6 的主张裁决**不在本表**，其处置记录在 v3.0~v3.4 谱系、`CHANGELOG.md` 与 ADR-195/196/199。把“本表已覆盖全部六份”当作事实是错的（本轮全栈总账已逐条补裁，见 `Chimera架构重构与优化方案_v3.5-全栈终局总账.md`）。

---

*本概念映射表于 2026-09-02 生成，所有路径经 Grep/Glob 实测核实，未经验证的映射项一律标注「缺失」而非编造路径。ADR 临号作废回执与覆盖度声明于 2026-10-08（v3.5 W6-2）追加。*