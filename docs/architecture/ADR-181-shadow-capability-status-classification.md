# ADR-181: 影子架构能力状态归类 — 分片已接线订正 + 内环一致性四层"接入或退役"预算

- **状态**: Accepted（2026-09-04）
- **决策者**: 架构治理（E01 口径），执行会话 C8（评审报告 `DEEP_RESEARCH_NEXUS_OMEGA_architecture_review.md` F-A3-3/F-A3-4 处置）
- **承接**: ADR-175（死模块删除与孤岛偿还计划）/ ADR-176（方向 E-H 决策汇总）/ ADR-033 / ADR-047
- **备注**: 评审快照（2026-09-04 晨间）称"因果一致性三层零接线、ShardedBus 灰度未接生产"；
  本 ADR 以当前工作树复测**部分推翻并订正**该口径（发现 1），并按"宣称-现实差距"治理原则归类处置。

## 背景

C8 清点复测发现两个方向相反的偏差：

**发现 1（订正过时负面口径）**：ShardedBus 已实际接线——osa-coordinator、faae-router、
gqep-executor、gsoe-evolution、efficiency-monitor 五处生产组合根以
`let _ = event_bus.enable_sharding(DEFAULT_SHARD_COUNT)` 尝试启用（无 tokio runtime 时
返回 Err 自动降级单流，幂等拒绝重复初始化）。评审"灰度基建就绪但未接生产"过时；
event-bus `lib.rs` shard/telemetry 模块头的"默认不启用"表述也不再完整（缺"各组合根
运行时 opt-in"事实）。

**发现 2（确证零接线并归类）**：以下五模块在生产代码（排除 event-bus 自身与其
tests/examples）中外部引用为 **0**（逐名 grep 实证）：

| 模块 | API | 内部使用 | 现状 |
|---|---|---|---|
| `rcu::MonotonicState` | arc-swap RCU 单调读状态 | 仅本模块测试 | 零消费（宣称的"内环第二层因果一致性"未接线） |
| `causal::VectorClock` | 逻辑时钟因果判定 | `attribution.rs` 内部复用 | 零外部消费 |
| `formal::CausalConsistencyChecker` | 属性 #4 事件因果验证器 | 本 crate 单测 + 根 E2E | 消费方为验证器 E2E（FormalVerifierGate 证据链合法消费），非生产路径 |
| `attribution::CausalAttributionLedger` | P4-T6 归因台账 | 本 crate 测试 + examples | 零生产消费 |
| `shadow_diff`（Recorder/Report/ShadowSojournLedger） | P4-T4 双跑指纹比对 + ≥7 天台账 | examples/shadow_sojourn + 运维脚本 run_shadow_sojourn.ps1 | 灰度观察工具在位（手动），未接 CI/生产进程 |

这些模块均有完整测试覆盖（非 ADR-175 定义的"死模块"——死模块判据是无测试无引用的
纯尸体），但处于"**有实现、有测试、零生产消费者**"的中间态——评审所称"影子架构堆积"
的实质。长期维持该状态的成本：文档口径漂移风险（本 ADR 即为一次修正）、重构者误判
可用能力、维护面虚胖。

## 决策

### 决策 1：状态标签制（立即生效）

五模块 doc 头与 event-bus `lib.rs` 模块声明处挂统一状态标签（三态之一）：

- **PRODUCTION-WIRED**：生产代码存在消费方（shard 订正为此态）；
- **EXPERIMENTAL-UNWIRED**：实现与测试完整、零生产消费、等待接线决策（rcu / causal /
  attribution 的 ledger 部分 / shadow_diff）；
- **VERIFIER-EVIDENCE-ONLY**：仅供形式化验证证据链消费，非运行时路径（formal）。

### 决策 2：接线或退役预算（有期限的中间态）

EXPERIMENTAL-UNWIRED 能力不永久冻结，绑定以下偿还路径（对齐 ADR-175 批次机制）：

| 能力 | 接线路径（二选一即达） | 退役条件 |
|---|---|---|
| rcu::MonotonicState | ~~① 三环内环状态快照改造时以 `MonotonicState` 承载共享读（P2-W7.2.3 原设计意图）~~；② 若内环改造采用其他原语，则删除并在 ADR-176 方向 E 登记。**✅ 已退役（2026-09-13）**：内环里程碑（P2-W7.2.3 → v2.28.x 全波次收尾）结束仍零生产消费（grep 实证，仅 lib.rs 声明/导出与模块自身测试），按退役条件触发，以 ADR-175 口径（验证零消费 → 删除模块 + lib.rs 声明/导出/注释引用清理 → grep 功能引用零残留 + cargo check/test 全绿 + clippy -D warnings）执行删除；`arc-swap` 依赖保留（bus.rs 生产路径在用）；完整实现（含 16 项测试）存档于 git 基线 tag `baseline-2026-09-13` | ~~下一内环里程碑结束仍零消费 → 按 ADR-175 删除口径执行~~ → **已执行** |
| causal::VectorClock | 保留为 attribution 基座（内部消费成立）；对外 API 面不扩大 | attribution 退役时一并处置 |
| attribution::CausalAttributionLedger | ADR-132 因果归因管线接入 telemetry 导出侧，或降为内部结构 | 下一遥测批次结束零消费 → 删除（examples 同步退役） |
| shadow_diff + ShadowSojournLedger | 分片转正前必须完成 ≥7 天双跑（run_shadow_sojourn.ps1 手动口径）；转正决策数据源即本台账 | 分片转正决策完成后，若不再需要常态比对 → 保留 7 天窗口语义、归档 examples |
| formal::CausalConsistencyChecker | 维持 VERIFIER-EVIDENCE-ONLY（R2 解冻证据链组成，见 formal_gate 7 验证器清单），不接生产热路径 | 不退役（形式化验证证据本体） |

### 决策 3：文档口径同步义务

任何描述上述能力的文档/规则条目在引用时必须与三态标签一致；禁止将
EXPERIMENTAL-UNWIRED 能力表述为"已生效"（本次订正 `lib.rs` shard/telemetry/因果
一致性三层相关注释；根文档（CLAUDE.md/AGENTS.md/CODE_WIKI）中的
"因果一致性三层(L251-255)"类表述由收口批次同步，见 C14 清单追加项）。

### 决策 4：不删除（本轮）

本轮**不执行删除**：五模块均有测试防护且两个（formal/shadow_diff）已承担治理证据职能；
删除只在决策 2 的退役条件被触发时按 ADR-175 口径（先证引用清零 → 删除 → 回归）执行。

## 后果

- 正面：宣称-现实差距双向消除（既订正"未接线"的过时低估，也标注"零消费"的高估）；
  中间态获得有期限的处置预算，维护面不再无限期虚胖。
- 负面：五个 doc 头新增标签文字；后续里程碑需回看本 ADR 的退役条件核对。
- 验证：event-bus 全量测试零行为变更通过；adr_index 登记本 ADR；C8 进度报告归档。
