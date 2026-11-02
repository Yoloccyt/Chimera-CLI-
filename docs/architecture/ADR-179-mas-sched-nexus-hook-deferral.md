# ADR-179: mas-sched 与 nexus-hook 接线延期决策

- **状态**: Accepted（2026-09-03，架构重构批次2 Task 4 决策）
- **决策者**: 架构批次（spec: `.trae/specs/implement-arch-refactor-batch2`）E01 提案；用户批准批次 spec 后终裁
- **关联**:
  - ADR-145（mas-sched shadow-first 预留）
  - ADR-146（nexus-hook 生命周期钩子预留）
  - ADR-160 / ADR-171 / ADR-175（冻结孤岛与偿还批次计划）
  - `crates/mas-sched/Cargo.toml`（仅 L0 契约/错误/序列化依赖,nexus-core/event-bus/tracing 已移除）
  - `scripts/crate_reachability_freeze.txt`（两行登记）

> **编号说明**: 承接 ADR-176（2026-09-03 批次1）顺延为 **ADR-179**,属决策/登记型 ADR(序号 177/178 为批次2 实施 ADR,按提交时序落档)。

## 背景

批次2 探索(arch-batch2 勘察)确认两孤岛的接线前置条件**未成熟**:

1. **mas-sched**(ADR-145):当前仅依赖 L0 `nexus-contracts` 契约/错误/序列化;其 `Cargo.toml` 注释明示 `nexus-core / event-bus / tracing` **因未实现已移除**,待事件面落地才以 `optional + feature` 引入。即:调度器拆分面所依赖的事件面(与 chimera-mas 的交互契约)尚未建立。
2. **nexus-hook**(ADR-146):生命周期钩子以"空配置 = 现状安全回退"为保留语义;其 HookTriggerEvent(DynamicEvent 实现者,ADR-173 轨二冻结)注册表面尚未有生产装配路径。

冻结清单中两行均标记 `[T11-b1 WIP: repayment=awaiting composition-root wiring, no scheduled date]`,批次2 对其复核并给出显式决策。

## 决策

1. **本轮不接线,登记延期**:mas-sched / nexus-hook 维持冻结孤岛登记,本期不做组合根接线。
2. **延期理由(登记)**:① mas-sched 事件面依赖未落地(拆分面契约未建,接线=空转);② nexus-hook 生产注册路径依赖轨二(DynamicEvent)生产装配,而 ADR-173 已将轨二冻结未激活;③ 两 crate 合计约 2,254 行(899+1,355),接线收益当前低于与 chimera-mas 拆分演进互锁的风险。
3. **解冻触发条件(登记)**:以下任一成立即可重新评估接线——① chimera-mas 拆分事件面(L0 契约或 event 变体)落地并评审通过;② 轨二 DynamicEvent 经 ADR 解冻并建立生产注册路径;③ 组合根出现明确消费方(计划性接线点)。
4. **棘轮语义不变**:两行继续登记(行数不增),freeze.txt 注释追加 "@see ADR-179 延期" 以终止 "no scheduled date" 悬置。

## 否决项

- **否决本轮强行接线**:事件面未落地时接线会产生空转装配面(与 ADR-172 对 model-router"接线=为接线而接线"同一判据)。
- **否决从冻结清单除名**:除名会虚增生产可达数、破坏 ADR-160 棘轮口径。

## 后果

- **正向**: 两孤岛从"无日期悬置"升级为"显式决策 + 解冻触发条件";预算/精力转向高价值项(mca M4、StrategyCapGuard)。
- **无负向**: 零代码改动;孤岛数保持 15(与 Task 2 mca 行删除后的 14 为同批次内两动作,终态 14)。

## 验证

- [x] mas-sched / nexus-hook `Cargo.toml` 依赖面复核(仅 L0,证据已固化)
- [x] freeze.txt 两行追加 "@see ADR-179 延期" 注记(行数不增)【Task 4.2,待落地】
- [x] `scripts/check_crate_reachability.py` EXIT=0(终态 frozen=14)【批次收口验证】