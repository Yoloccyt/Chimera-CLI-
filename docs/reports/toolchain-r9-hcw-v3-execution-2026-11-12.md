# R9 批次执行报告：HCW v3 蓝图落地——上下文/记忆层 + 并行调度（2026-11-12）

> 分支 `feat/toolchain-parallel-optimization`，承接 R8（07cfe4a9..9dabc748，ADR-288/289）。
> 设计蓝本：《HCW_上下文架构重构蓝图_v3.0_1M时代能力感知分层.md》（用户指定，2026-11-04 编制）。
> 本报告为 R9 批次单一事实源。证据行号以探索时点工作树（HEAD=0ddda7f4 前后）为准。

## 0. 关键假设与用户裁决

| 裁决点 | 用户选择 | 蓝图原文 | 偏离处理 |
|--------|---------|---------|---------|
| R9 范围 | P0+P1+P3桥+快赢（P2 实验、P3 传输面留后续） | P0→P3 四阶段 | 按依赖序推进，P3 只做 session-store 桥 |
| P2 w4 深度 | **预埋+默认开启** | w4 默认 off（ADR-198 三态） | 退化安全阀（source=None 条目 w4 恒 0）+ `CHIMERA_W4_DISABLED=1` env 回退；ADR-290 声明偏离 |
| 事件预算 | 按蓝图 +2 Normal 变体 | ContextCapabilityUpdated/WindowPlanApplied | 六锁同步 154→156、121→123，ordinal 尾号续增（ADR-197 惯例） |
| checkpoint 卫星态 | 接线恢复 | —（蓝图未涉及） | 修 from_bytes 降级缺陷 + 恢复路径重建桥接 |

## 1. 三路深审事实底座（Explore 代理 + 主会话复核）

### 1.1 L0/mca-gateway 落点（P0/P3）
- nexus-contracts-mca 风格锚点：`#[non_exhaustive]` 先例 EndpointSpec（affinity.rs:338）、Box<str> 惯例（:29-30）、serde JSON/rmp 双往返测试先例（:904/:912）、内联测试带 allow 头（:820）；生产依赖仅 serde（L0 零依赖铁律）
- **health.rs EWMA 基建零改动可复用**（ChannelHealth :35-42 AtomicU64 位打包无锁 EWMA / HealthRegistry :127）；但 `ProviderDegraded` 事件**有消费方无生产方**（efficiency-monitor/affinity_metrics.rs:219 已消费；mca-gateway 零发布点）——P0 须补生产方
- **ADR-154 漂移守卫是纯文档、零代码**（ProviderDriftDetected 全仓 grep 零命中）——蓝图若引是新建实现
- ADR-201 model_visible_context 投影出口完整存在（server.rs:411-425）——蓝图附录 B 待确认项 1 无需新建
- transport_invoke（adapters.rs:530-573）404 分支（:566）是 threads 回退插入点；ResponsesCodec 无状态（每轮重发）；**mca-gateway 全 src 无 thread_id/previous_response_id 概念**
- 组合根 build_mca_gateway（composition.rs:601-615）为 assembly-only（debug_assert spec_count==0），feature mca 门控——threads 传输面无灰度环境（P3 传输面不做决策依据）

### 1.2 HCW/OSA 落点（P1/P2）
- **capacity()/effective_capacity() 已是 config 驱动**（types.rs:102-157）——枚举 L0-L3 不动、容量注入是 P1 最小形态
- WindowTier match 散布面：types.rs:57-97（as_str/parse/upgrade/downgrade）、selector.rs:50-72、affinity.rs:52-60、config.rs:98-128 validate；外部依赖 mca-gateway/hcw_integration.rs:70、chimera-cli/selector_orchestrator.rs:27、**chimera-mas budget_model.rs:68-96 平行 ContextTier 四层硬编码同款数字**、omega-learner/s1_density.rs:187
- **w1·recency+w2·frequency+w3·relevance 在 compressor.rs:83-107**（非 WindowSelector）；权重契约 L0 nexus-contracts-seams policy.rs:61-128（is_valid 和≈1.0 校验——加 w4 必改）
- **ContextEntry 无来源类型字段**（types.rs:204-229）——w4 需新增（#[serde(default)] 保 checkpoint 兼容）；唯一生产构造点 chimera-mas manager.rs:415-420（未标注来源）
- OSA→HCW 单向链路：compute_context_mask（coordinator.rs:405）→ OmniSparseMasksComputed → spawn_mask_listener（window.rs:615-662）
- decb BudgetStats `get_stats()`（governor.rs:487-520）是 WindowGovernor 预算查询入口；ThinkingMode 在 L0 domain.rs:73-82；TTG `current_mode()`（ttg.rs:444）同步可查
- 卡片系统无 compression_ratio 指标（P2 测量需新建，本轮不做）
- R8 ADR-288 兼容点：三阶段协议（读锁快照带 mutations 版本→锁外 compress→差集合并重验容量）+ apply_pending_mask_if_any 惰性兜底顺序 + spawn_mask_listener 锁纪律不可破坏

### 1.3 一致性/并行面（P3 桥/快赢）
- **session-store 404 回放四缺口全缺**：① 唯一持久化事件类型 turn.submit（payload 仅 turn_id JSON）——Item 文本/工具调用/审批均未落盘；② AppServer replay_since（Item 级内存 DashMap，server.rs:335-342）与 session-store replay()（事件级磁盘）之间无桥；③ Offset↔ItemId 游标无换算；④ 无线程级回放入口
- **ledger_parity_locks.rs:174-190 锁测试**：组合根出现 replay_since 生产调用即失败并要求同批更新 ADR——P3 桥必须同批处理
- recall_hybrid 生产无调用方（全在测试；sparse_select_top_k.rs:8 明文）——并行化收益为零，Wave 4 冻结面（omega-learner S7）维持
- checkpoint 卫星态：SearchTree/LongTaskMap from_bytes 全仓零调用（死 API）；恢复不重发 QuestCreated → QuestBridgeState 不存在；from_bytes 会把 external_storage 降级 InMemory（long_task_map.rs:326）
- **coarse.rs 三信号并行已在盘**（coarse.rs:128 注释「R9：依赖/语义/共变更并行；panic 回退串行」，compute_signals :211-230 thread::scope）——R8 深审该项已被并行批次修复，台账关闭
- mesh 剩余串行点：reconcile_pending_transactions 两级串行（mesh.rs:972/987/1000）最高价值；2PC 阶段序 prepare→WAL→commit 协议固有串行不可并行
- **capability_of 恒 None（mcp_tool_executor.rs:237-239）→ 所有 mcp 工具保守串行计划层**——当前 mcp 路径并行封顶点（登记不动，独立裁决）
- scc select_victim 维持不采纳（容量上界自辩；TOCTOU 已登记 lru.rs:36-38）

## 2. R9 工作包（可落地）

| WP | crate（所有权） | 内容 | 蓝图对应 | 执行 |
|----|----------------|------|---------|------|
| WP-0 | event-bus + chimera-tui | +2 Normal 变体（六锁同步 156/123，ordinal 尾号 155/156）+ TUI 穷举同步 | §六 | 主会话 ✅ 0ddda7f4 |
| WP-A | nexus-contracts-mca | ContextCapabilityProfile + CalibrationTier + ThreadSupport + 公开先验灌入表 | §5.2 P0 | 代理 |
| WP-F | mcp-mesh | reconcile_pending_transactions 两级 FuturesUnordered（+SERIAL_RECONCILE 回退开关） | —（快赢） | 代理 |
| WP-G | nmc-encoder | 模型 OnceLock 缓存 + ffmpeg 探测 memoize | —（快赢） | 代理 |
| WP-B | mca-gateway | 有效窗口探针骨架（observe 档）+ ProviderDegraded 补生产方 + ContextCapabilityUpdated 发布 | §5.3 P0 | 代理（依赖 A） |
| WP-C | hcw-window | WindowPlan/WindowGovernor + R/S/E/M 容量注入（枚举不动）+ ADR-288 协议兼容 + 双跑对照 | §5.4 P1 | 代理（依赖 A+0） |
| WP-E | session-store + nexus-app-server | 事件 schema 扩展 + event→Item 重构 + Offset↔ItemId 游标 + 线程级回放入口 + ledger_parity_locks 同批 | §5.5 P3 桥 | 代理 |
| WP-H | quest-engine + chimera-cli | checkpoint 卫星态嵌入/还原 + from_bytes 降级修复 + quest_loop 恢复重建桥接 | —（一致性） | 代理 |
| WP-D | nexus-contracts-seams + hcw-window | w4：ContextEntry.source + SelectorWeights 四维 + compute_importance_score 第四项 + 退化安全阀 + env 开关 | §5.4 P2 | 主会话（C 后） |

## 3. 验收标准（可验证）

1. 逐 crate 定向测试全绿 + clippy -D warnings
2. **hcw criterion 基准零回退**（蓝图 P1 验收线：compress/window_select/coarse/fine/rerank 全组）
3. 事件锁 156/123 机器绿；TUI 穷举 match 同步绿
4. w4 行为锁：source=None 条目打分逐位不变（默认开启 = 存量零影响）
5. P3 桥：404 注入测试回放重建零丢失；ledger_parity_locks 更新后绿
6. WindowPlanApplied 快照可回放（governor 单测 + proptest 预算边界）
7. 组合态：chimera-cli cargo check + quest_lifecycle + cli 测试

## 4. 回滚方式（可回溯）

- 每 WP 独立 commit；w4 有 `CHIMERA_W4_DISABLED` env 开关；WindowGovernor 默认静态回退档；P3 桥 append-only 不破坏旧数据；WP-0 事件为纯增量（revert 即回 154 锁）
- ADR-290（HCW v3 P0+P1+P2 预埋 + Ω₂ 重定义 ADR 化 + w4 偏离声明）+ ADR-291（P3 桥 + 卫星态接线）收官落档

## 5. 减法（明确不做）

threads 传输面（组合根 GATED 无灰度环境）；scc select_victim（维持不采纳）；recall_hybrid 并行（生产无调用方 + Wave 4 冻结面）；KV cache 层管理；标称=常驻全量装载；capability_of 补全（mcp 并行封顶点，登记独立裁决）；coarse 三信号（已在盘，台账关闭）。

## 6. 执行记录

### 6.1 重大环境事件：双会话同树（2026-11-12 发现，用户裁决「只做无冲突子集」）

工作树发现另一会话大规模在途改动（26 文件 +611/-81：新 `hcw-window/src/assemble.rs`「R9: 统一上下文装配内核（去重+单一预算账本+确定性驱逐）」+ 已占用 **ADR-290** 编号的 `ADR-290-cross-session-memory-consistency.md` + session-store/scc-cache/gqep/chimera-cli/TUI/scripts 基线）。与我的 WP-C（hcw）/WP-E（session-store+ledger_parity_locks）/WP-H（chimera-cli）/WP-D（hcw types.rs）**直接撞车**。处置（用户裁决）：我方收缩为无冲突子集，**WP-C/E/H/D 推迟至另一会话提交后下一批次接续**；我的 ADR 编号让出 290（改用 291 起）。此事件同时解开了 R8 深审「coarse.rs R9 注释」之谜——三信号并行系该并行会话所落。

**无冲突子集**：WP-0（event-bus 已在其扩展前提交）✅、WP-A（nexus-contracts-mca）✅、WP-G（nmc-encoder）✅、WP-F（mcp-mesh）进行中、WP-B（mca-gateway）进行中。

### 6.2 逐 WP 结果

| WP | 提交 | 改动 | 验收 |
|----|------|------|------|
| WP-0 事件地基 | `0ddda7f4` | event-bus types/registry/variant_count_test + TUI coverage_drift_test | event-bus 全 target 0 failed；六锁 156/123；ordinal 尾号 155/156（ADR-197 惯例） |
| WP-A L0 契约 | `7e810093` | nexus-contracts-mca 新 leaf context_capability（712 行） | 47 unit + 3 doctest 0 failed；clippy/fmt 净；偏离决策 6 项（Box<str>/条件计费面 PricingTier/文档约定不变量等）均有据 |
| WP-G nmc 缓存 | `5895f7eb` | onnx_backend.rs（+121/-37 TractPlan Arc<HashMap 缓存>+ffmpeg OnceLock）+ text.rs | 157/0；下游三 crate check 绿；**拒绝单条目方案有实证**（4 模型同序构造互相挤占）；收益诚实评估（当下生产≈0，bench/未来接线兜底） |
| WP-F mesh reconcile | `34f936e6` | mesh.rs（+546/-46 三段式：快照→FuturesUnordered 扁平扇出→per-tx 台账聚合；SERIAL_RECONCILE 回退开关） | **155/0**；语义裁决有据（2PC 原子性在 prepare 已确立，commit=独立收敛幂等重试，commit_phase :562-577 自证）；峰值在飞数确定性证明（6 臂屏障 peak==6） |
| WP-B 探针骨架 | `1ced2b15` | mca-gateway capability_probe.rs（新 ~560 行）+ health.rs（DegradedPublisher）+ lib.rs | **9 target 全绿（lib 252 含新增 14）**；observe 档默认按蓝图原文；rising edge 去重；不挂 record() 高频路径（C7） |

### 6.3 收官回归（无冲突子集五 crate 组合，--no-fail-fast）

`cargo test -p event-bus -p nexus-contracts-mca -p nmc-encoder -p mcp-mesh -p mca-gateway --jobs 2` → **1271 passed / 0 failed**。

### 6.4 登记推迟项（因另一会话占用冲突文件）

- ADR-291 的 adr_index.md 索引登记：adr_index.md 正被另一会话编辑（在途 M），推迟至其提交后与 WP-C/E/H/D 接续批次一并补登
- WP-D（w4）用户裁决「预埋+默认开启」的设计（退化安全阀 source=None 恒 0 + CHIMERA_W4_DISABLED env 开关 + DEFAULT 权重重分配建议 0.36/0.27/0.27/0.10）已随本报告存档，接续时直接实施

### 6.3 核实订正

- WP-A 代理报告「nexus-contracts umbrella facade 待补」经主会话核实为**保守误判**：`crates/nexus-contracts/src/lib.rs:193` 是组级 re-export（`pub use nexus_contracts_mca::mca`），新 leaf 子模块 `nexus_contracts::mca::context_capability` 经 Rust 模块语义自动可达，mca crate 根 leaf re-export（lib.rs:37）已由 WP-A 补齐——零额外动作。
