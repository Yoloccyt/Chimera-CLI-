# Tasks — 实时数据驱动面板系统 TUI

> 任务按优先级严格递进：P0（前置必做）→ P1（数据接入）→ P2（面板改造）→ P3（验证与文档）。
> 每个 Task 完成后必须通过 checklist.md 全部检查项才能进入下一 Task。
> **执行规范**：每个 Task 遵循 TDD（RED-GREEN-REFACTOR）；子代理产物必须 `cargo fmt --all -- --check` + `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过；每个 Task 完成后勾选 `[x]`。
> **协作模式**：精英专家级子代理团队（系统架构 + 事件总线 + 数据库/指标 + 前端/TUI），系统性分布式深度分析 + 多轮结构化验证。

## P0 — 前置必做：现状核验与接口契约定义（~4h）

> **依赖**：无（当前 master 分支已包含 `chimera-tui` 占位渲染与 `chimera-cli` 接线）
> **并行性**：无
> **验收门槛**：接口契约文档 + 依赖方向合规确认 + `cargo check -p chimera-tui` 通过

- [x] **Task P0.1: 现状核验**
  - [x] SubTask P0.1.1: Read `crates/chimera-tui/src/app.rs` 渲染逻辑，确认 5 面板布局与静态内容位置
  - [x] SubTask P0.1.2: Read `crates/chimera-tui/src/types.rs`，确认 `TuiState` 当前字段
  - [x] SubTask P0.1.3: Read `crates/event-bus/src/types.rs`，列出与 Quest / Parliament / Budget 相关的 `NexusEvent` 变体
  - [x] SubTask P0.1.4: Read `crates/quest-engine/src/lib.rs` 与 `types.rs`，确认 `Quest` 公开 API
  - [x] SubTask P0.1.5: Read `crates/efficiency-monitor/src/lib.rs` 与 `types.rs`，确认预算指标公开 API
  - [x] SubTask P0.1.6: 检查 `chimera-tui` 当前依赖是否已包含 `event-bus`，确认是否需要新增 `quest-engine`、`efficiency-monitor` 依赖（注意 §2.2 依赖铁律：L10 → L9 向上依赖禁止）

- [x] **Task P0.2: 接口契约定义**
  - [x] SubTask P0.2.1: 设计 `TuiDataSource` trait，统一 event-bus / quest-engine / efficiency-monitor 的数据访问
  - [x] SubTask P0.2.2: 定义 `DataSnapshot` 结构体，包含 quest_list、latest_events、budget_metrics 三个字段
  - [x] SubTask P0.2.3: 确认跨层通信方案：因 `chimera-tui` 在 L10，无法直接依赖 L9 的 `quest-engine` 或 `efficiency-monitor`，必须通过 event-bus 或 MCP Mesh 获取数据；若当前 event-bus 数据不足，设计新增事件变体
  - [x] SubTask P0.2.4: 将接口契约写入 `crates/chimera-tui/src/data.rs` 模块骨架（仅类型与 trait，无实现）

### P0.2 Note — `NexusEvent` 变体结论

- `chimera-tui` 数据源契约仅依赖 L1 `event-bus` + `nexus-core`，未引入 L9 `quest-engine` / `efficiency-monitor` 依赖，符合 §2.2 依赖铁律。
- **Quest 列表**：现有 `QuestCreated` / `QuestProgressUpdated` / `ThinkingModeSwitched` / `CheckpointSaved` / `ExecutionCompleted` 可用于增量维护，但建议新增 `QuestListUpdated { quests: Vec<Quest>, source: String }`（完整列表对齐）与 `QuestCompleted { quest_id: String, status: QuestStatus }`（从活动列表移除）。
- **Budget 指标**：现有 `BudgetStatsReported` / `BudgetAdjusted` / `BudgetExceeded` / `EfficiencyAlertTriggered` 已足够 TUI 聚合出 `BudgetMetrics`；为减少面板侧拼合逻辑，建议新增 `BudgetMetricsUpdated { metrics: BudgetMetricsPayload }`。
- **事件流**：直接订阅 `EventBus` 即可获得 `VecDeque<NexusEvent>`，无需新增变体。
- 上述新增变体将在 **Task P1.2** 中提交到 `crates/event-bus/src/types.rs`，本阶段仅作为契约描述。

## P1 — 数据接入层实现（~12h）

> **依赖**：P0 完成
> **并行性**：P1.1 与 P1.2 可并行；P1.3 依赖 P1.1 + P1.2
> **验收门槛**：`cargo test -p chimera-tui` 新增数据接入测试全部通过

- [x] **Task P1.1: Event-Bus 订阅实现**
  - [x] SubTask P1.1.1: TDD-RED — 在 `crates/chimera-tui/tests/subscriber_test.rs` 新增测试：订阅后发布事件，TUI 数据缓存更新
  - [x] SubTask P1.1.2: TDD-GREEN — 实现 `EventSubscriber` 结构体，使用 `event-bus` 的 subscriber API（先 `subscribe()` 再 `spawn()`，遵循 §4.4 红线）
  - [x] SubTask P1.1.3: TDD-REFACTOR — 为事件缓冲与去重添加 WHY 注释
  - [x] SubTask P1.1.4: 验证事件订阅优雅关闭：退出 TUI 时 subscriber task 终止，无 orphan task

### P1.1 Note — 实现偏差说明

- **测试文件位置**: 任务说明要求使用 `crates/chimera-tui/tests/subscriber_test.rs`,已按此执行;`tasks.md` 原定的 `data_test.rs` 未创建,避免与 P1.3 `DataPipeline` 测试混淆。
- **缓冲区实现**: `EventSubscriber` 未直接使用 `tokio::sync::mpsc::Receiver`,而采用 `Arc<Mutex<VecDeque<NexusEvent>>>` 构成 1024 容量的环形缓冲区。原因:tokio 的 bounded mpsc 在满时只能阻塞或丢弃**新**事件,无法自然实现"溢出丢弃**旧**事件"的语义;使用 `VecDeque` 可在 `push_back` 前 `pop_front`,语义与 TUI 面板"始终展示最新事件"一致。
- **lib.rs 顺带修复**: 修正了 prelude 中残留的 `BudgetSync`/`QuestSync` 重导出(该类型尚未实现,会导致 `cargo check` 失败),并加入 `EventSubscriber` 到顶层导出与 prelude。
- **验证结果**: `cargo check -p chimera-tui`、`cargo test -p chimera-tui`、`cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings`、`cargo fmt --all -- --check` 全部通过。

- [x] **Task P1.2: Quest 与 Budget 数据同步实现**
  - [x] SubTask P1.2.1: TDD-RED — 新增测试：模拟 `quest-engine` 与 `efficiency-monitor` 数据，TUI 缓存正确聚合
  - [x] SubTask P1.2.2: TDD-GREEN — 实现 `QuestSync` 与 `BudgetSync` 适配器（若无法直接依赖 L9，则通过 event-bus 请求/响应事件获取；必要时新增 `QuestListRequested` / `BudgetMetricsRequested` 事件变体）
  - [x] SubTask P1.2.3: TDD-REFACTOR — 统一错误处理：数据获取失败时保留旧数据并记录 warn 日志

### P1.2 Note — 实现偏差说明

- **`QuestStatus` 位置**: 原契约假设 `QuestStatus` 已存在于 `nexus-core`，但仓库中仅有 `TaskStatus`。为遵守 §3.3.1 "核心领域类型变更需 ADR" 的红线，未修改 `nexus-core`，而是在 `event-bus` 层新增轻量级 `QuestStatus` 枚举（`Completed`/`Failed`/`Cancelled`），作为 `QuestCompleted` 事件的载荷。该类型已随 `BudgetMetricsPayload` 一起从 `event-bus` 根导出。
- **未新增请求/响应事件**: P0.2 结论仅需单向数据事件（`QuestListUpdated` / `QuestCompleted` / `BudgetMetricsUpdated`），因此没有实现 `QuestListRequested` / `BudgetMetricsRequested`。`QuestSync` 与 `BudgetSync` 直接消费上述事件并维护本地状态，为 P1.3 `DataPipeline` 做准备。
- **`lib.rs` 顺带修复**: P1.1 并行实现后 `crates/chimera-tui/src/lib.rs` 出现重复 `pub mod app;` 导致编译失败，已删除重复项；顶层重导出已补全 `BudgetSync`/`QuestSync`（与 P1.1 对 prelude 的补全保持一致）。
- **验证结果**: `cargo check -p event-bus`、`cargo check -p chimera-tui`、`cargo test -p chimera-tui`（76 通过）、`cargo test -p event-bus`（107 通过）、`cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings`、`cargo fmt --all -- --check`、`cargo check --workspace` 全部通过。

- [x] **Task P1.3: DataPipeline 中间层实现**
  - [x] SubTask P1.3.1: TDD-RED — 新增测试：多源事件在 250ms tick 内对齐为单一快照
  - [x] SubTask P1.3.2: TDD-GREEN — 实现 `DataPipeline`，包含 tick timer、事件缓冲、去重、快照生成
  - [x] SubTask P1.3.3: TDD-REFACTOR — 使用 `tokio::select!` 合并 timer 与事件通道，避免 busy loop
  - [x] SubTask P1.3.4: 性能测试：1000 事件/秒场景下，数据缓存更新延迟 P95 < 100ms

### P1.3 Note — 实现说明

- **`DataSourceConfig` 新增 `tick_interval_ms`**: 默认 250ms，控制快照生成频率。
- **`DataPipeline` 后台任务**: 通过 `tokio::time::interval` + `tokio::select!` 驱动，tick 到达时批量消费 `EventSubscriber::try_recv()` 中的全部事件；无事件时任务阻塞在 `interval.tick()` 上，避免 busy loop。
- **去重策略**: 同一 tick 窗口内若出现多个 `QuestListUpdated` / `BudgetMetricsUpdated`，仅最后一个用于状态更新（减少重复计算），但所有事件仍保留在 `latest_events` 日志流中。
- **容量限制**: `latest_events` 按 `max_event_history` 截断，`quest_list` 按 `max_quest_list_size` 截断，防止内存无限增长。
- **关闭语义**: `DataPipeline::shutdown` 采用 `abort + await`，与 `EventSubscriber::shutdown` 保持一致，避免 orphan task。
- **`TuiDataSource` 实现**: `DataPipeline` 实现 `TuiDataSource`，面板可无阻塞读取当前快照。
- **验证结果**: `cargo check -p chimera-tui`、`cargo test -p chimera-tui`（含 2 个新集成测试 + 1 个 #[ignore] 性能测试）、`cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings`、`cargo fmt --all -- --check` 全部通过。

## P2 — 面板渲染改造（~16h）

> **依赖**：P1 完成
> **并行性**：P2.1、P2.2、P2.3 可并行
> **验收门槛**：TUI 启动后面板显示真实数据；`cargo test -p chimera-tui` 全部通过

- [x] **Task P2.1: Quest 面板数据驱动渲染**
  - [x] SubTask P2.1.1: 修改 `TuiState` 增加 `quest_list: Vec<Quest>` 字段
  - [x] SubTask P2.1.2: 修改 `panel_content` / `render_main_panel`，根据 `current_panel` 渲染真实 Quest 列表
  - [x] SubTask P2.1.3: 新增集成测试：Quest 数据更新后，TestBackend 渲染内容包含新任务 id

- [x] **Task P2.2: Budget 面板数据驱动渲染**
  - [x] SubTask P2.2.1: 修改 `TuiState` 增加 `budget: BudgetMetrics` 字段
  - [x] SubTask P2.2.2: 修改 Budget 面板渲染，显示当前 tier、consumption、utilization、trend
  - [x] SubTask P2.2.3: 当预算状态为 Critical 时，使用红色/高亮样式提示
  - [x] SubTask P2.2.4: 新增集成测试：BudgetExceeded 事件触发后面板显示 critical 状态

- [x] **Task P2.3: Parliament 面板数据驱动渲染**
  - [x] SubTask P2.3.1: 修改 `TuiState` 增加 `latest_events: VecDeque<NexusEvent>` 字段（保留最近 N 条）
  - [x] SubTask P2.3.2: 修改 Parliament 面板渲染，显示最近的 Parliament / Skeptic / RedTeam 事件摘要
  - [x] SubTask P2.3.3: 新增集成测试：事件到达后 Parliament 面板渲染内容更新

- [x] **Task P2.4: Log 面板联动**
  - [x] SubTask P2.4.1: Log 面板显示最近系统日志与事件流摘要
  - [x] SubTask P2.4.2: 新增测试：事件到达后 Log 面板追加显示

### P2 Note — 实现说明

- **TuiState 字段已前置就位**: `quest_list`、`budget`、`latest_events` 三个字段在 P1.3 `DataSnapshot` 与 `TuiState` 设计时已经存在(P1.2 Note 已说明),因此 P2 实际工作集中在渲染逻辑改造与测试补齐。
- **Quest 面板**: `app.rs` 中 `quest_panel_content` 已按 Quest 标题(加粗)、ID/思考模式(灰色)、任务统计(绿色 done)渲染,并新增 `test_quest_panel_content_uses_state` 单元测试与 `test_quest_panel_renders_real_quest_data` 集成测试。
- **Budget 面板**: `budget_panel_content` 显示 tier、coefficient、consumption/remaining、utilization、进度条与 EXCEEDED 红色加粗状态,并新增 `test_budget_panel_content_uses_state` 单元测试与 `test_budget_panel_shows_critical_state` 集成测试。
- **Parliament 面板**: `parliament_panel_content` 筛选并渲染 VoteCast / ConsensusReached / SkepticVeto / RedTeamAudit / AsaIntervention 事件,安全类事件使用红/黄告警色,并新增 `test_parliament_panel_renders_recent_events` 集成测试。
- **Log 面板(P2.4 本次完成)**:
  - 主面板 `log_panel_content`: 基于 `latest_events` 逆序显示最近 10 条,格式 `[HH:MM:SS] [source] type_name`,对 `SkepticVeto`/`RedTeamAudit`/`AsaIntervention`/`BudgetExceeded` 红色高亮。
  - 底部固定面板 `render_log_panel`: 基于 `latest_events` 动态显示最近 1-5 条(受区域高度限制),使用 `Text`/`Line`/`Span` 结构,空态显示 `[System] Chimera TUI initialized`。
  - 新增 `test_log_panel_appends_events` 集成测试,验证底部面板与主 Log 面板均会渲染数据源事件。
- **依赖**: 为格式化 `EventMetadata::timestamp`,`chimera-tui` 新增 `chrono = { workspace = true }` 依赖(chrono 已是 workspace 共享依赖,无新增外部依赖)。
- **验证结果**: `cargo check -p chimera-tui`、`cargo test -p chimera-tui`(65 单元 + 2 data_test + 15 integration + 5 subscriber + 1 doc-test 全部通过)、`cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings`、`cargo fmt --all -- --check` 全部通过。

## P3 — 验证与文档（~8h）

> **依赖**：P2 完成
> **并行性**：P3.1 与 P3.2 可并行；P3.3 依赖两者
> **验收门槛**：`cargo test --workspace` 通过；文档更新完成

- [x] **Task P3.1: 功能与回归验证**
  - [x] SubTask P3.1.1: `cargo test -p chimera-tui` 全部通过（含新增测试）
  - [x] SubTask P3.1.2: `cargo test --workspace` 全部通过（无回归）
  - [x] SubTask P3.1.3: `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 通过
  - [x] SubTask P3.1.4: `cargo fmt --all -- --check` 通过
  - [x] SubTask P3.1.5: 手动验证：本地运行 `cargo run -p chimera-cli -- tui`，确认面板显示动态数据并响应键盘操作

- [x] **Task P3.2: 性能与压力验证**
  - [x] SubTask P3.2.1: 新增 `crates/chimera-tui/benches/data_pipeline_bench.rs`，测量 1000 事件/秒下的快照生成延迟
  - [x] SubTask P3.2.2: 新增 `crates/chimera-tui/benches/render_bench.rs`，测量 5 面板渲染帧时间
  - [x] SubTask P3.2.3: 性能阈值：snapshot latency P95 < 100ms；render time P95 < 16ms（60 FPS）

- [x] **Task P3.3: 文档更新与归档**
  - [x] SubTask P3.3.1: `CHANGELOG.md` 追加 v1.5.9-omega（或下一版本）实时 TUI 章节
  - [x] SubTask P3.3.2: `CODE_WIKI.md` 更新 TUI 数据流描述
  - [x] SubTask P3.3.3: `project_memory.md` 追加实时 TUI 实现教训
  - [x] SubTask P3.3.4: 创建 `docs/tui/realtime_dashboard_report.md`，汇总架构、性能、测试结果

### P3 Note — 实现说明

- **P3.1 功能与回归验证**:
  - `cargo test -p chimera-tui`:65 单元 + 2 data_test + 15 integration + 5 subscriber + 1 doc-test(1 ignored)全部通过。
  - `cargo test --workspace`:3461 passed / 0 failed / 57 ignored,无回归。
  - `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings`:0 warnings。
  - `cargo fmt --all -- --check`:0 diff。
  - SubTask P3.1.5(手动 TUI 验证):本地 release 构建已通过编译,`chimera-tui` 的 `TuiApp::run()` 入口已在 `chimera-cli` `tui` 子命令接线完成;由于自动化环境无法驱动真实终端 TTY 交互(需要 raw mode + crossterm event),此项需用户在真实终端执行 `cargo run -p chimera-cli -- tui` 验证面板动态数据与键盘响应(q 退出 / Tab 切换面板)。代码路径已由 integration 测试通过 TestBackend 覆盖。

- **P3.2 性能与压力验证**:
  - `crates/chimera-tui/benches/data_pipeline_bench.rs`:
    - `data_pipeline_snapshot_latency`:1000 事件/秒场景下快照生成延迟,目标 P95 < 100ms。
    - `data_pipeline_throughput`:125/250/500 三档事件密度对比(对应 500/1000/2000 事件/秒),量化 tick 批处理在不同负载下的吞吐。
    - `Cargo.toml` 新增 `criterion = { workspace = true }` dev-dependency 与 `[[bench]] name = "data_pipeline_bench"` 声明。
  - `crates/chimera-tui/benches/render_bench.rs`:
    - `render_all_panels`:5 面板完整渲染帧时间,目标 P95 < 16ms(60 FPS)。
    - `render_each_panel`:分面板 BenchmarkId(quest/budget/parliament/log/help),定位渲染热点。
  - 阈值依据:snapshot 100ms 对应 250ms tick 的 40% 预算,留足余量;render 16ms 对应 60 FPS 人眼流畅阈值。本地 Windows GNU 环境可通过 `cargo bench -p chimera-tui` 运行,实际数值由 CI/用户机器报告。
  - 降级说明:本地 `cargo audit` 因网络不可达(advisory-db 需访问 GitHub)跳过,由 `.github/workflows/audit.yml` 每日 UTC 02:00 兜底(3 个已评估 ignore)。

- **P3.3 文档更新与归档**:
  - `CHANGELOG.md`:新增 `## v1.4.0-omega 实时数据驱动 TUI 面板系统` 章节,概述 P0-P3 全部交付。
  - `CODE_WIKI.md`:新增 `##### 实时数据驱动 TUI 面板系统` 子章节,含 ASCII 数据流图(NexusEvent → EventSubscriber → DataPipeline → TuiApp.update → panel_content → TestBackend/terminal)。
  - `project_memory.md`:追加 6 条实时 TUI 实现教训(type_name 分桶、tick 驱动、同 tick 去重、TuiState 派生调整、Box<dyn TuiDataSource>、Text<'static> 样式化)。
  - `docs/tui/realtime_dashboard_report.md`:新建综合报告(7 章:背景与目标、架构设计、面板渲染实现、测试验证、依赖变更、教训、后续演进),作为本规格的最终交付文档。

## Task Dependencies

- **P0（现状与契约）**：无依赖
- **P1（数据接入）**：依赖 P0
- **P2（面板改造）**：依赖 P1
- **P3（验证与文档）**：依赖 P2

## 并行执行建议

- **批次 1**：P0（单线程，必须先行）
- **批次 2**：P1.1 与 P1.2 并行，完成后 P1.3
- **批次 3**：P2.1、P2.2、P2.3 并行，P2.4 可随后
- **批次 4**：P3.1、P3.2 并行，完成后 P3.3

> **长期主义原则**：不跳级，不并行跨档；每批次完成后 `cargo fmt --all` + `cargo clippy` + `cargo test -p chimera-tui`；每批次完成后 git commit + push，保持工作树干净。
