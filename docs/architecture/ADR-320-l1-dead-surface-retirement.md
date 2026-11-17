# ADR-320: L1 死面退役决策（R3 批：`report_snapshot` / NexusState budget 面 / 孤儿变体）

- 状态: Accepted（决策批准；施工前置 = 在途 L1-B 线落地，见 D4）
- 日期: 2026-11-16
- 批次: L1 层深度重构 R3（第一卷 §5.1 R3 + 第二卷 §5.1 协调协议）
- 设计源: 第一卷取证（nexus-core 全维解剖：`CLV::report_snapshot`/`update_quest_progress`/`global_budget`/`model_registry_snapshot` 生产零调用面 grep 实测）
- 关联: ADR-311 D2.1（零生产装配面处置判据：三条件 grep + 测试消费不构成降级理由 + 退役即物理删除）、ADR-312（B5 撤主张面退役先例，311 D2.1 子章节）、ADR-160（装配面台账）、ADR-197（序数只增不改）

---

## D1 — 处置对象与判据

| 对象 | 证据（生产零调用 grep 实测） | 处置 |
|---|---|---|
| `crates/nexus-core/src/clv.rs::CLV::report_snapshot` | 全 workspace 生产 0 调用（仅自身测试） | 物理删除；`ClvSnapshotReported` 事件契约保留在 event-bus（TUI 面）；CRATE-CONTRACT `PRODUCERS` 注记修正 |
| `crates/nexus-core/src/state.rs::NexusState::{global_budget,set_global_budget,model_registry_snapshot,set_model_registry_snapshot}` | 生产零读者；setter 仅 quest-engine 测试调用 | 字段+方法整体退役；quest-engine 测试同步改 |
| `crates/nexus-core/src/state.rs::StateChangeKind::{GlobalBudgetChanged,ModelRegistryChanged}` | 随上一行联动退役 | registry 行 + variant 三层锁（155→153）+ I8c 散文计数同批三处 |
| `crates/event-bus/src/types.rs` 三个 RESERVED 孤儿变体（`EvolutionTriggered`/`ContextBudgetAllocated`/`BenchmarkMetricsCollected`） | 注释自承生产发布端缺失 | 挂 `#[deprecated(since="2.30")]` 注记 + 文档登记；不物理删除（序数只增不改 ADR-197） |

判据逐条对齐 ADR-311 D2.1：① 生产引用=0（grep 实测，量纲=crates/*/src）；② 测试消费不构成保留理由；③ 退役即物理删除（孤儿变体除外——其破坏面大于收益，注记降级为已裁决豁免）。

## D2 — 与 L1-B-4 门的 freeze 协调协议

在途线新增 `scripts/check_l1_dead_surface.py`（L0-B 零装配公共面门）+ `scripts/l1_dead_surface_freeze.txt` 台账（双向判红：NEW/STALE）。本批删除公共面将使台账行变 STALE，协议：

1. **顺序**：L1-B 线先合入（台账以删除前公共面建册）→ 本批施工 → STALE 行由本批同 PR 第二 commit 删除。
2. **反序保护**：若本批先合入，在途线建册时相关项已不存在 → 册中无此行 → 无 STALE，无损。
3. 台账变更与代码删除**同批改**（门的双向判红强制这一纪律，不可拆分）。

## D3 — 兼容与退役窗口

- 本批随 2.30.0-omega 合入；无 deprecated 过渡窗（生产零调用，直接删除符合 ADR-311 D2.1）。
- 事件契约面（`ClvSnapshotReported`）保留——退役的是 L1 侧生产入口，不是事件类型。

## D4 — 前置与回滚

- **施工前置**：在途 L1-B 线（formal/shadow_diff/telemetry 退役 + G-161/G-162 门）落地——其脏文件覆盖 `crates/event-bus/src/lib.rs`/`attribution.rs` 与本批触碰面（registry/variant 锁）相交。
- **回滚**：PR 双 commit（event-bus 锁先行，nexus-core 删除随后），revert 逆序；台账同批在 commit 内。
