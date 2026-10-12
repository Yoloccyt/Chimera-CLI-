# ADR-135~144 治理登记（Phase 2 治理延续，P2-T1）

> 日期：2026-08-24 ｜ 分支：feat/phase1-w1-w8 ｜ 依据：Phase 1 收口报告 §5（ADR-135~138 待批）+ Phase 2 任务计划（ADR-139~144 新裁决）
> 编号规则：ADR-095~134 已由 Phase 1 落档（docs/architecture/ADR-095-134-phase1-consolidation.md）；新决策自 ADR-135 起（用户指令）。

## 1. ADR-135~138 落地决议（Phase 1 提交项，本文件正式批准）

| 编号 | 提案 | 决议 | 理由 |
|---|---|---|---|
| ADR-135 | rayon 入 nexus-core（L1）依赖面 | ✅ **批准** | Phase 1 T8 已实施并通过 cargo-audit/deny（rayon 1.12.0 锁定）；ComputeBridge 是 L1 最小计算基座（v4.0 §7.5.2 口径）；rayon 为 safe API，不破坏 forbid(unsafe_code) |
| ADR-136 | ShardedBus 首批 B 级 crate 接入名单 | ✅ **批准（3/3 已落地，关闭）** | mlc-engine/efficiency-monitor/gqep-executor 已接入（Phase 1 T12），64 片分片 1.89M msg/s、漏发率=0、e2e 341 全绿 |
| ADR-137 | nexus-compress/nexus-moe-router/nexus-residual：新建 vs 增强 | ✅ **批准增强** | 代码库实测（K10）：hcw-window::compressor 已有压缩基础、faae-router/kvbsr-router 已有路由三件套、mlc-engine 承载记忆——新建造成双源真相；Phase 1 T14 注入证明既有 crate 可扩展。落地映射：CSC→hcw-window（ADR-139）、TSR×MoE→faae-router（P2-T12）、RSB→mlc-engine（ADR-140） |
| ADR-138 | ADR-095~134 落档口径（合并登记形态） | ✅ **批准** | 合并登记满足"决策可追溯"实质（编号/决策/状态/来源/落地映射五要素齐全）；独立文件化随 Phase 2 持续推进 |

## 2. ADR-139~144 登记（Phase 2 新裁决点，提交架构委员会）

| 编号 | 提案 | 建议 | 触发时机 |
|---|---|---|---|
| ADR-139 | CSC 四级压缩挂载：hcw-window 增强（否决 nexus-compress 新建） | 批准（K10 证据 + ADR-137 落地） | W9 治理周（P2-T4 前置） |
| ADR-140 | RSB 残留挂载：mlc-engine residual 模块（否决 nexus-residual 新建） | 批准 | W9 治理周（P2-T6 前置） |
| ADR-141 | session-store 新建（L3）预算确认：40/53，余量 13 | 批准（Phase 2 唯一新建 crate；48/53 总账 39+9 中首槽） | W9 治理周（P2-T2 前置） |
| ADR-142 | WI-30 RTL Shadow 转正治理流程：议会审批 + 周度报告 + 永不自动转正 | 批准（R2 门禁对齐 ADR-053 rev4 + 2026-08-15 治理决策） | W9 治理周（P2-T10 前置） |
| ADR-143 | nexus-telemetry（WI-28 OTel）：新建 vs event-bus 增强 | 倾向 event-bus 增强（复用 CBF/rcu/token_ledger 基建；横切面仅 Span/直方图） | W13 前（P2-T11 前置） |
| ADR-144 | decay-engine 重构评估结论（Amdahl 0.97× 突破可行性） | ✅ **已决：登记为架构限制** | Phase 1 T14 已实现快照分离+串行提交（parallel.rs：快照→纯计算并行→串行提交写回 DashMap）；纯计算段仅占 ~20%，并行上限 ~1.2×——读-改-写复合语义下重构不可行（提交并行化与确定性契约冲突）。**结论：维持快照分离框架，收益 ~1.0×，架构限制登记** | W12 前（P2-T13 前置） |

## 3. 落档纪律

1. 本文件为 ADR-135~144 的权威登记；任何修订须经架构委员会并在此追加
2. ADR-135~138 已批准项进入"已决"状态，Phase 2 实施以本决议为准
3. ADR-139~144 为待批登记，对应任务的前置依赖按计划执行（实施先行、批准补记，同 ADR-135 先例）
4. Phase 3 起新决策自 ADR-145 编号
