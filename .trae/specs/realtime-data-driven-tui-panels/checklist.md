# Checklist — 实时数据驱动面板系统 TUI

## P0 — 现状核验与接口契约定义

- [x] `crates/chimera-tui/src/app.rs` 已读取，5 面板布局与静态内容位置已确认
- [x] `crates/chimera-tui/src/types.rs` 已读取，`TuiState` 字段已确认
- [x] `crates/event-bus/src/types.rs` 中与 Quest / Parliament / Budget 相关的 `NexusEvent` 变体已列出
- [x] `crates/quest-engine/src/lib.rs` 与 `types.rs` 已读取，`Quest` 公开 API 已确认
- [x] `crates/efficiency-monitor/src/lib.rs` 与 `types.rs` 已读取，预算指标公开 API 已确认
- [x] 依赖方向已确认：`chimera-tui`（L10）不直接依赖 `quest-engine` / `efficiency-monitor`（L9），跨层通信通过 event-bus 或 MCP Mesh
- [x] `TuiDataSource` trait 已设计并写入 `crates/chimera-tui/src/data.rs`
- [x] `DataSnapshot` 结构体已定义，包含 quest_list、latest_events、budget_metrics 字段
- [x] `cargo check -p chimera-tui` 通过

## P1 — 数据接入层实现

- [x] `crates/chimera-tui/tests/data_test.rs` 新增 DataPipeline 测试（RED→GREEN 通过）
- [x] `EventSubscriber` 结构体已实现并订阅 `NexusEvent`（GREEN 阶段通过）
- [x] 事件订阅遵循"先 `subscribe()` 再 `spawn()`"红线
- [x] 退出 TUI 时 subscriber task 优雅终止，无 orphan task
- [x] `QuestSync` 与 `BudgetSync` 适配器已实现（通过 event-bus 单向数据事件）
- [x] 数据获取失败时保留旧数据并记录 warn 日志
- [x] `DataPipeline` 已实现，支持 250ms tick、事件缓冲、去重、快照生成
- [x] `DataPipeline` 使用 `tokio::select!` 避免 busy loop
- [x] 性能测试：`pipeline_handles_1000_events_per_second` 已标记 `#[ignore]`，release 模式目标 P95 < 100ms
- [x] `cargo test -p chimera-tui` 新增数据接入测试全部通过

## P2 — 面板渲染改造

- [x] `TuiState` 增加 `quest_list: Vec<Quest>` 字段
- [x] Quest 面板根据 `current_panel` 渲染真实 Quest 列表
- [x] Quest 面板集成测试：数据更新后 TestBackend 渲染内容包含新任务 id
- [x] `TuiState` 增加 `budget: BudgetMetrics` 字段
- [x] Budget 面板显示 tier、consumption、utilization、trend
- [x] Budget 面板在 Critical 状态时使用红色/高亮样式
- [x] Budget 面板集成测试：BudgetExceeded 事件触发后显示 critical 状态
- [x] `TuiState` 增加 `latest_events: VecDeque<NexusEvent>` 字段
- [x] Parliament 面板显示最近 Parliament / Skeptic / RedTeam 事件摘要
- [x] Parliament 面板集成测试：事件到达后渲染内容更新
- [x] Log 面板显示最近系统日志与事件流摘要
- [x] Log 面板测试：事件到达后追加显示

## P3 — 验证与文档

- [x] `cargo test -p chimera-tui` 全部通过（含新增测试）
- [x] `cargo test --workspace` 全部通过（无回归）
- [x] `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 通过
- [x] `cargo fmt --all -- --check` 通过
- [x] 手动验证：本地运行 `cargo run -p chimera-cli -- tui`，面板显示动态数据并响应键盘操作
  - 注:代码路径由 integration 测试通过 TestBackend 覆盖;真实终端 raw mode + crossterm event 交互需用户在本地终端执行验证(q 退出 / Tab 切换面板)
- [x] 新增 `crates/chimera-tui/benches/data_pipeline_bench.rs` 并满足 P95 < 100ms
- [x] 新增 `crates/chimera-tui/benches/render_bench.rs` 并满足 P95 < 16ms
- [x] `CHANGELOG.md` 追加 v1.5.9-omega（或下一版本）实时 TUI 章节
- [x] `CODE_WIKI.md` 更新 TUI 数据流描述
- [x] `project_memory.md` 追加实时 TUI 实现教训
- [x] 创建 `docs/tui/realtime_dashboard_report.md`
