# ADR-006: Quest 结构体新增 priority 字段

> ⚠ **编号冲突提示(2026-09-22 登记)**:`ADR-006` 这个号在本仓库另有所指 ——
> `adr_index.md:64` 行写的是"异步运行时 Tokio",而 `docs/adr/ADR-006-rusqlite-descoping-from-nexus-core.md`
> 是另一份不相关决策(rusqlite 下沉 L3)。引用本决策时**请带文件名** `ADR-006-quest-priority-field.md`。
> 全貌见 `docs/architecture/adr_index.md` 末"编号冲突登记"。

## 状态

已接受 (2026-07-15)

## 上下文

- `Quest` 是核心领域类型(定义于 `nexus-core/src/types.rs`),按项目规则 `.trae/rules/nuxus规则.md` §3.3.1 第 4 条要求,核心领域类型变更需 ADR 记录。
- TUI 任务管理界面需要优先级概念,支持操作员调整 Quest 执行优先级,以便在并发 Quest 调度时按优先级排序。
- 优先级范围: 0-255(`u8`),默认值 128(中位值),数值越大优先级越高。
- 当前项目版本: `v1.7.0-omega`(GA 后演进阶段),适用 §3.3 第二阶段开发规则。

## 决策

在 `Quest` 结构体新增 `pub priority: u8` 字段,默认值 128。

字段将随 `Quest` 一并通过 Event Bus 广播(`QuestListUpdated` 等事件),保证优先级状态与 Quest 列表的单一数据源一致性。

## 考虑的方案

- **方案 A(采纳)**: 直接在 `Quest` 加字段 + `#[serde(default)]` 兼容
  - 优点: 优先级随 Quest 列表自然传递,TUI/调度器可直接读取;单一数据源,无跨结构同步问题;序列化向后兼容。
  - 缺点: `Quest` 结构体变大 1 字节;所有构造点需更新。

- **方案 B(否决)**: 独立 `PriorityMap<quest_id, u8>` 维护映射
  - 否决理由: 导致 `QuestListUpdated` 事件与优先级状态不一致;需在多个 crate 间维护映射一致性,违反"单一数据源"原则;增加 Event Bus 同步开销与竞态风险。

## 后果

- **正面**: 优先级随 Quest 列表传递,TUI 可直接展示;单一数据源,调度器(`quest-engine` / `gqep-executor`)可直接基于字段排序。
- **负面**: `Quest` 结构体变大 1 字节;所有 `Quest` 构造点需更新(初始化为 128)。
- **向后兼容**: `#[serde(default)]` 保证旧序列化数据(MessagePack / JSON)反序列化时 `priority` 取默认值 128,不破坏既有检查点(`Checkpoint`)与持久化数据。

## 合规性

- **§2.2 依赖铁律**: 不违反。`priority` 字段位于 L1(`nexus-core`),属类型内部字段新增,不引入跨层依赖。
- **§4.1 编码规范**: 符合。使用 workspace 级版本约定,无 `unsafe`,无 `unwrap()`/`expect()`。
- **§3.3.1 第 4 条**: 本 ADR 满足核心领域类型变更的记录要求。
- **§3.3.1 第 5 条(向后兼容)**: 通过 `#[serde(default)]` 实现 SemVer 兼容,属非破坏性变更(minor 级)。
- **§6.1 架构红线**: 不违反。字段新增不改变数据流主路径,不引入功能标志。
