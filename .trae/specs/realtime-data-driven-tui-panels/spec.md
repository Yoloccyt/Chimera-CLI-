# 实时数据驱动面板系统 TUI Spec

## Why

当前 `chimera-cli tui` 子命令仅启动一个静态占位界面（chimera-tui crate 已实现基础渲染框架，但面板内容为硬编码文本）。为了让用户在终端中实时观察 Chimera CLI 的运行状态，需要将 TUI 面板接入真实数据源：event-bus 的 `NexusEvent` 流、`quest-engine` 的任务状态以及 `efficiency-monitor` 的预算/资源指标。

## What Changes

- 在 `chimera-tui` crate 中引入实时数据订阅层，通过 `event-bus` 订阅 `NexusEvent` 事件流。
- 新增与 `quest-engine` 的接口适配器，获取真实 `Quest` 列表与状态。
- 新增与 `efficiency-monitor` 的接口适配器，获取预算消耗指标与趋势数据。
- 在 `TuiState` 中维护实时数据缓存，面板渲染从静态文本切换为动态数据。
- 新增数据处理中间层（`DataPipeline`），统一多源数据的时间对齐、去重与刷新频率控制。
- 保持 `chimera-cli/src/commands/tui.rs` 的调用方式不变，仅增加配置透传。
- 为 `chimera-tui` 补充集成测试：事件驱动更新、数据同步一致性、多面板切换后数据不丢失。
- 更新文档：`CHANGELOG.md` 追加 v1.5.9-omega（或下一版本）章节，`CODE_WIKI.md` 更新 TUI 数据流描述。

## Impact

- Affected specs: `v1-4-0-omega-p2-implementation`（监控指标基础）、`v1-1-0-f2-rusqlite-descoping`（事件总线基础）。
- Affected code: `crates/chimera-tui/`, `crates/chimera-cli/src/commands/tui.rs`, `crates/event-bus/src/types.rs`（可能新增事件变体或扩展数据载荷）。
- Breaking changes: 无公共 API 破坏；`TuiConfig` 增加可选数据源配置字段，默认行为不变。

## ADDED Requirements

### Requirement: Event-Bus 实时数据订阅

The system SHALL subscribe to `NexusEvent` stream and update TUI panels in real time.

#### Scenario: 事件到达时面板刷新

- **WHEN** a `NexusEvent` is published on the event bus
- **THEN** the TUI data layer receives the event within 100ms and refreshes the corresponding panel content

#### Scenario: 事件订阅优雅关闭

- **WHEN** the user exits the TUI (q / Esc)
- **THEN** the event subscription is dropped and the subscriber task terminates cleanly

### Requirement: Quest-Engine 任务数据集成

The system SHALL fetch and display real `Quest` tasks from `quest-engine`.

#### Scenario: TUI 启动时加载任务列表

- **WHEN** the TUI starts
- **THEN** it fetches the current `Quest` list from `quest-engine` and renders it in the Quest panel within 1 second

#### Scenario: 任务状态更新同步

- **WHEN** a `Quest` is created, updated, or completed
- **THEN** the Quest panel reflects the change within 2 seconds

### Requirement: Efficiency-Monitor 预算指标集成

The system SHALL fetch and display budget consumption metrics from `efficiency-monitor`.

#### Scenario: TUI 启动时加载预算指标

- **WHEN** the TUI starts
- **THEN** it fetches the current budget tier, consumption, and utilization from `efficiency-monitor` and renders them in the Budget panel within 1 second

#### Scenario: 预算指标实时刷新

- **WHEN** a budget-related `NexusEvent` (e.g., `BudgetExceeded`) is published
- **THEN** the Budget panel updates immediately and visually highlights critical state

### Requirement: 数据处理中间层

The system SHALL provide a `DataPipeline` that unifies multi-source data ingestion.

#### Scenario: 多源数据一致性

- **WHEN** data arrives from event-bus, quest-engine, and efficiency-monitor at different times
- **THEN** the `DataPipeline` aligns snapshots per tick (default 250ms) so the UI renders a consistent view

#### Scenario: 重复事件去重

- **WHEN** duplicate events of the same type arrive within the tick window
- **THEN** only the latest event of that type is retained

## MODIFIED Requirements

### Requirement: TUI 面板渲染

The existing static panel rendering SHALL be replaced with data-driven rendering while preserving the 5-panel layout (Quest / Parliament / Budget / Log / Help).

#### Scenario: Quest 面板显示真实任务

- **WHEN** the Quest panel is active
- **THEN** it displays the list of `Quest` objects (id, status, current task, thinking mode) instead of placeholder text

#### Scenario: Budget 面板显示实时指标

- **WHEN** the Budget panel is active
- **THEN** it displays the current budget tier, consumption, utilization, and recent trend instead of placeholder text

#### Scenario: Parliament 面板显示最新事件

- **WHEN** the Parliament panel is active
- **THEN** it displays the latest `Parliament` / `Skeptic` / `RedTeam` events from the event stream instead of static vote summary

## REMOVED Requirements

无移除项。
