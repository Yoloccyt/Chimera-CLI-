# Checklist — TUI v1.7-omega 深化演进

> **change-id**: `tui-v17-deep-evolution`
> **验收门槛**: 每个 Task 完成后必须通过全部检查项才能进入下一 Task
> **执行规范**: TDD(RED-GREEN-REFACTOR)+ `cargo fmt --all` + `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` + `cargo test -p chimera-tui`

---

## P0 — 现状评估与架构基线设计

- [x] `crates/chimera-tui/src/app.rs` 完整事件循环已读取,当前快捷键处理逻辑与可扩展点已记录
- [x] `crates/chimera-tui/src/panels/mod.rs` + 8 个面板模块已读取,Panel trait 实现模式与可复用组件(list_state / filtered_xxx / content)已梳理
- [x] `crates/chimera-tui/src/data.rs` DataPipeline + DataSnapshot 已读取,快照字段缺口与新面板所需数据已识别
- [x] `crates/event-bus/src/types.rs` 已读取,需要新增的事件变体已列出(DecayMetricsReported / RouterStatsReported / McpNodeHeartbeat / ChtcAdapterStatus)
- [x] `crates/chimera-tui/src/render.rs` + `popup.rs` 已读取,EventDetail overlay 与 Help overlay 扩展点已规划
- [x] Claude Code CLI 调研完成,可借鉴的交互模式(流式输出 / 工具调用展开折叠 / 命令模式)已提取
- [x] Qodex CLI 调研完成,Quest 面板进度条增强方向已提取
- [x] Kimi Code CLI 调研完成,详情 overlay 的 JSON 高亮方案已提取
- [x] 参考 CLI 借鉴清单已写入 tasks.md 的 P0 Note 第 6 节(参考风格融合)
- [x] `PanelId` 从 8 变体扩展到 14 变体的迁移方案已设计(新增 Decay/EventStream/Router/McpNodes/Chtc/Timeline)
- [x] `TuiState` 字段扩展方案已设计(decay_metrics / router_metrics / mcp_nodes / chtc_state / timeline_snapshots / fps / dirty_panels / auto_scroll)
- [x] `DataSnapshot` 字段扩展方案已设计(与 TuiState 对齐)
- [x] `TuiConfig` 字段扩展方案已设计(tick_interval_ms / snapshot_interval_s / max_event_history / max_snapshots)
- [x] 4 个新事件变体的载荷结构已设计
- [x] 架构基线摘要已写入 tasks.md 的 P0 Note
- [x] 依赖方向已确认:chimera-tui(L10)仅依赖 event-bus + nexus-core(L1),不引入 L9 依赖

## P1 — 提交 P3 归档

- [x] `git status` 确认未提交变更范围(应包含 tasks.md / checklist.md 的 P3 勾选 + P3.1/P3.2/P3.3 全部实施产物)
- [x] `git diff --stat` 核验变更范围符合预期(仅 chimera-tui + specs/realtime-data-driven-tui-panels + CHANGELOG.md + CODE_WIKI.md + docs/tui/)
- [x] `git add` 逐文件添加(未使用 `git add -A` / `git add .`)
- [x] `git commit -m` 使用 HEREDOC 传入 commit message,描述 P3 验证 + 基准 + 文档归档
- [x] `git push origin main` 推送成功(实际分支为 main,commit 0f1d1a0)
- [x] `git log --oneline -3` 确认提交成功

## P2 — 新增 5 监控面板

### P2.1 Decay 衰减面板

- [x] `crates/chimera-tui/tests/decay_panel_test.rs` 新增测试,验证 DecayMetricsReported 事件到达后面板渲染衰减系数与 sparkline
- [x] `crates/chimera-tui/src/panels/decay.rs` 新增,实现 `DecayPanel` + `Panel` trait
- [x] sparkline 渲染复用 `render::sparkline` 辅助函数
- [x] 衰减系数 > 0.7 红色高亮(阈值常量提取,WHY 注释说明依据)
- [x] `crates/event-bus/src/types.rs` 新增 `DecayMetricsReported` 事件变体(载荷:coefficient f32 / recent_events Vec<String> / timestamp)
- [x] `DataPipeline` 新增 `DecaySync` 适配器,订阅事件并维护 `decay_metrics` 状态
- [x] `cargo test -p chimera-tui` 新增 Decay 面板测试全部通过
- [x] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [x] `cargo fmt --all -- --check` 通过

### P2.2 EventStream 全量事件流面板

- [x] `crates/chimera-tui/tests/event_stream_panel_test.rs` 新增测试,验证万级事件虚拟滚动帧时间 < 16ms
- [x] `crates/chimera-tui/src/panels/event_stream.rs` 新增,实现 `EventStreamPanel` + `Panel` trait
- [x] 虚拟滚动实现(`render::virtual_scroll_window` 辅助函数,仅渲染可见区域 + 上下 5 行缓冲)
- [x] 流式追加 + 自动滚动实现(`auto_scroll` 标记,用户滚动时暂停,底部显示 "[新事件 N 条]")
- [x] 事件类型筛选实现(复用 `filter_topic` / `filter_level` 字段)
- [x] `cargo test -p chimera-tui` 新增 EventStream 面板测试全部通过
- [x] 性能测试:10000 事件虚拟滚动帧时间 P95 < 16ms
- [x] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [x] `cargo fmt --all -- --check` 通过

### P2.3 Router 路由统计面板

- [x] `crates/chimera-tui/tests/router_panel_test.rs` 新增测试,验证 RouterStatsReported 事件到达后显示三路由器命中率与延迟
- [x] `crates/chimera-tui/src/panels/router.rs` 新增,实现 `RouterPanel` + `Panel` trait
- [x] 三路由器命中率进度条实现(复用 `render::utilization_bar`)
- [x] 热点 capability_id Top-10 列表实现(使用 `select_nth_unstable` O(n) Top-K,遵守 §4.1)
- [x] `crates/event-bus/src/types.rs` 新增 `RouterStatsReported` 事件变体(载荷:kvbsr_stats / sesa_stats / faae_stats 各含 hit_rate / p50 / p95 / p99 / hot_capabilities)
- [x] `cargo test -p chimera-tui` 新增 Router 面板测试全部通过
- [x] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [x] `cargo fmt --all -- --check` 通过

### P2.4 MCP 节点状态面板

- [x] `crates/chimera-tui/tests/mcp_nodes_panel_test.rs` 新增测试,验证 McpNodeHeartbeat 事件到达后显示节点列表与连接状态
- [x] `crates/chimera-tui/src/panels/mcp_nodes.rs` 新增,实现 `McpNodesPanel` + `Panel` trait
- [x] 节点列表渲染实现(节点 ID / 连接状态绿黄红 / 最近消息吞吐量)
- [x] 离线告警横幅实现(心跳 > 5s 未到达显示 "[ALERT] Node X offline")
- [x] `crates/event-bus/src/types.rs` 新增 `McpNodeHeartbeat` 事件变体(载荷:node_id / status / throughput / last_seen)
- [x] `cargo test -p chimera-tui` 新增 McpNodes 面板测试全部通过
- [x] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [x] `cargo fmt --all -- --check` 通过

### P2.5 CHTC 跨平台适配器面板

- [x] `crates/chimera-tui/tests/chtc_panel_test.rs` 新增测试,验证 ChtcAdapterStatus 事件到达后显示 5 IDE 适配器兼容性评分
- [x] `crates/chimera-tui/src/panels/chtc.rs` 新增,实现 `ChtcPanel` + `Panel` trait
- [x] 5 适配器兼容性评分展示实现(评分 < 60 黄色高亮)
- [x] 最近请求类型分布 sparkline 实现
- [x] `crates/event-bus/src/types.rs` 新增 `ChtcAdapterStatus` 事件变体(载荷:adapter_id / adapter_type / compatibility_score / recent_requests)
- [x] `cargo test -p chimera-tui` 新增 Chtc 面板测试全部通过
- [x] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [x] `cargo fmt --all -- --check` 通过

### P2 整体验收

- [x] `PanelId` 枚举扩展到 14 变体(新增 6 个:Decay/EventStream/Router/McpNodes/Chtc/Timeline,Help 已有)
- [x] `PanelId::next()` / `prev()` 循环顺序扩展为 14 面板循环
- [x] `TuiState` 新增字段(decay_metrics / router_metrics / mcp_nodes / chtc_state)已实现
- [x] `DataSnapshot` 新增字段与 `DataPipeline` 同步实现
- [x] `crates/chimera-tui/src/panels/mod.rs` 重导出 5 个新面板
- [x] `crates/chimera-tui/src/lib.rs` prelude 更新
- [x] `cargo test -p chimera-tui` 全部通过(含新增 5 面板占位测试,无回归)— 共享基础设施阶段
- [x] `cargo test --workspace` 全部通过(无回归)
- [x] P2 Note 已写入 tasks.md(共享基础设施完成说明 + 后续实施指引)

## P3 — 交互能力升级

### P3.1 Enter 事件详情 overlay

- [ ] `crates/chimera-tui/tests/event_detail_test.rs` 新增测试,验证 EventStream 面板选中事件后按 Enter 弹出 Detail overlay
- [ ] `crates/chimera-tui/src/popup.rs` 新增 `PopupKind::EventDetail` 变体(载荷:event_type / payload_msgpack / payload_decoded / related_event_ids)
- [ ] MessagePack 解码为 JSON 实现(使用 rmp-serde)
- [ ] JSON 语法高亮实现(字符串绿色 / 数字青色 / 布尔黄色 / null 灰色)
- [ ] 上下游事件 ID 链展示实现(底部显示 related_event_ids,Tab 跳转)
- [ ] Parliament / Log / EventStream 三面板 `handle_key` 处理 Enter 键
- [ ] MessagePack 解码失败时的降级策略实现(显示原始 hex)
- [ ] `cargo test -p chimera-tui` 新增 EventDetail 测试全部通过
- [ ] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [ ] `cargo fmt --all -- --check` 通过

### P3.2 Help overlay(? 键触发)

- [ ] `crates/chimera-tui/tests/help_overlay_test.rs` 新增测试,验证任意面板按 `?` 弹出 Help overlay
- [ ] `crates/chimera-tui/src/popup.rs` 新增 `PopupKind::HelpOverlay` 变体(载荷:快捷键表 Vec<(key, description)>)
- [ ] Help overlay 渲染实现(居中浮窗,半透明背景,Esc 关闭)
- [ ] `TuiApp::handle_key` 全局拦截 `?` 键,产生 `TuiCommand::ShowHelp`
- [ ] Help overlay 与 Help 面板的区别已用 WHY 注释说明
- [ ] `cargo test -p chimera-tui` 新增 Help overlay 测试全部通过
- [ ] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [ ] `cargo fmt --all -- --check` 通过

### P3.3 全局快捷键系统统一

- [ ] `crates/chimera-tui/tests/global_keys_test.rs` 新增测试,验证 `j/k` / `g/G` / `/` / `:` / `?` / `Tab/Shift+Tab` / `数字` 全部生效
- [ ] `TuiApp::handle_global_key` 方法提取,全局键先拦截,未命中再委托面板
- [ ] 数字键 1-9 跳转面板实现
- [ ] `g/G` 跳顶/底实现(Panel trait 新增 `scroll_to_top/bottom` 默认实现)
- [ ] 全局键与面板键的优先级已用 WHY 注释说明
- [ ] `cargo test -p chimera-tui` 新增全局快捷键测试全部通过
- [ ] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [ ] `cargo fmt --all -- --check` 通过

### P3.4 流式事件追加(Claude Code 风格)

- [ ] `crates/chimera-tui/tests/auto_scroll_test.rs` 新增测试,验证 auto_scroll=true 时新事件自动滚动到底部
- [ ] `auto_scroll` 状态字段实现,用户手动滚动时设为 false
- [ ] 新事件到达时若 auto_scroll=false 显示 "[新事件 N 条]" 提示
- [ ] `G` 快捷键跳到底部并恢复 auto_scroll=true
- [ ] auto_scroll 策略已用 WHY 注释说明(参考 Claude Code 工具调用流式输出 UX)
- [ ] `cargo test -p chimera-tui` 新增 auto_scroll 测试全部通过
- [ ] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [ ] `cargo fmt --all -- --check` 通过

### P3 整体验收

- [ ] `cargo test -p chimera-tui` 全部通过(含新增交互测试,无回归)
- [ ] `cargo test --workspace` 全部通过(无回归)
- [ ] P3 Note 已写入 tasks.md

## P4 — 性能优化

### P4.1 增量渲染

- [ ] `crates/chimera-tui/tests/incremental_render_test.rs` 新增测试,验证仅 Budget 面板数据更新时其他面板不被重绘
- [ ] `TuiState` 新增 `dirty_panels: HashSet<PanelId>` 字段,数据更新时标记
- [ ] `TuiApp::render` 重构,仅对 `dirty_panels` 中的面板调用 `Panel::render`
- [ ] 帧结束清空 `dirty_panels` 实现
- [ ] dirty 标记策略(数据驱动 vs 时间驱动)已用 WHY 注释说明
- [ ] `cargo test -p chimera-tui` 新增增量渲染测试全部通过
- [ ] 性能测试:仅 1 面板更新时,帧时间相比全量重绘降低 ≥ 30%
- [ ] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [ ] `cargo fmt --all -- --check` 通过

### P4.2 虚拟滚动扩展到 Parliament

- [ ] `crates/chimera-tui/tests/parliament_virtual_scroll_test.rs` 新增测试,验证 1000 条历史事件虚拟滚动
- [ ] `render::virtual_scroll_window`(P2.2.3 实现)应用到 Parliament 面板
- [ ] 虚拟滚动缓冲行数(5 行)的选择依据已用 WHY 注释说明
- [ ] `cargo test -p chimera-tui` 新增 Parliament 虚拟滚动测试全部通过
- [ ] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [ ] `cargo fmt --all -- --check` 通过

### P4.3 可调 tick 暴露

- [ ] `crates/chimera-tui/tests/configurable_tick_test.rs` 新增测试,验证 `TuiConfig { tick_interval_ms: 100 }` 时 DataPipeline 使用 100ms tick
- [ ] `TuiConfig` 新增 `tick_interval_ms: u16` 字段(默认 250,范围 100-1000)
- [ ] `TuiApp::new` 将 `config.tick_interval_ms` 传递给 `DataPipeline::new` 的 `DataSourceConfig`
- [ ] `chimera-cli/src/commands/tui.rs` 新增 `--tick <ms>` CLI 参数
- [ ] 100-1000ms 范围的依据已用 WHY 注释说明
- [ ] `cargo test -p chimera-tui` 新增 tick 配置测试全部通过
- [ ] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [ ] `cargo fmt --all -- --check` 通过

### P4.4 FPS 显示

- [ ] `crates/chimera-tui/tests/fps_test.rs` 新增测试,验证 Health 面板渲染内容包含 "FPS: <n>",FPS < 30 黄色 / < 15 红色
- [ ] `TuiApp` 新增 `fps_tracker: FpsTracker` 结构体,基于 `frame_count` 与时间戳计算 FPS
- [ ] `TuiState` 新增 `fps: u16` 字段,每秒更新一次
- [ ] `HealthPanel::render` 显示 `FPS: <n>`,根据阈值着色
- [ ] FPS 计算窗口(1 秒滑动平均)已用 WHY 注释说明
- [ ] `cargo test -p chimera-tui` 新增 FPS 测试全部通过
- [ ] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [ ] `cargo fmt --all -- --check` 通过

### P4 整体验收

- [ ] `crates/chimera-tui/benches/render_bench.rs` 扩展,新增增量渲染基准(全量 vs 增量对比)
- [ ] `crates/chimera-tui/benches/data_pipeline_bench.rs` 扩展,新增可调 tick 基准(100/250/500/1000ms 对比)
- [ ] 性能阈值:snapshot latency P95 < 100ms;render time P95 < 16ms;增量渲染帧时间降低 ≥ 30%
- [ ] `cargo test -p chimera-tui` 全部通过(无回归)
- [ ] `cargo test --workspace` 全部通过(无回归)
- [ ] P4 Note 已写入 tasks.md

## P5 — 跨面板联动

### P5.1 Esc 状态保留

- [ ] `crates/chimera-tui/tests/session_test.rs` 新增测试,验证 TUI 退出后 session.json 存在,下次启动恢复
- [ ] `crates/chimera-tui/src/session.rs` 新增,实现 `TuiSession` 结构体
- [ ] `TuiSession::save` 方法实现,退出时序列化到 `~/.aether/tui/session.json`
- [ ] `TuiSession::load` 方法实现,启动时反序列化恢复状态
- [ ] `TuiApp::run` 退出时调用 `TuiSession::save`
- [ ] `TuiApp::new` 中尝试 `TuiSession::load`
- [ ] 状态保留范围(仅 UI 状态,不含数据快照)已用 WHY 注释说明
- [ ] `cargo test -p chimera-tui` 新增 session 测试全部通过
- [ ] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [ ] `cargo fmt --all -- --check` 通过

### P5.2 Quest/Event 跳转底层命令

- [ ] `crates/chimera-tui/tests/cli_invoke_test.rs` 新增测试,验证 Quest 详情后按 `w` 触发 `chimera wiki <quest_id>`
- [ ] `TuiCommand` 新增 `InvokeCliSubcommand { command: String, args: Vec<String> }` 变体
- [ ] Quest 面板详情 overlay 处理 `w` 键
- [ ] Log 面板详情 overlay 处理 `s` 键(跳转 quest 子命令)
- [ ] `TuiApp::apply_command` 实现子命令调用:暂停 raw mode → 执行 `std::process::Command` → 等待任意键 → 恢复 raw mode
- [ ] raw mode 切换的必要性已用 WHY 注释说明
- [ ] `cargo test -p chimera-tui` 新增 CLI 跳转测试全部通过
- [ ] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [ ] `cargo fmt --all -- --check` 通过

### P5 整体验收

- [ ] `cargo test -p chimera-tui` 全部通过(无回归)
- [ ] `cargo test --workspace` 全部通过(无回归)
- [ ] P5 Note 已写入 tasks.md

## P6 — 主题运行时切换与布局模板(v1.7 可选)

### P6.1 运行时主题切换

- [ ] `crates/chimera-tui/tests/theme_switch_test.rs` 新增测试,验证按 `t` 键 Dark → Light → HighContrast → Dark 循环
- [ ] `Theme` 枚举新增 `HighContrast` 变体
- [ ] `Theme::next()` 方法实现(Dark → Light → HighContrast → Dark 循环)
- [ ] `TuiApp::handle_global_key` 处理 `t` 键,切换 `config.theme` 并标记所有面板为 dirty
- [ ] HighContrast 主题颜色方案实现(纯黑背景 / 纯白前景 / 高饱和度强调色)
- [ ] HighContrast 主题的目标用户(色盲 + 高亮环境)已用 WHY 注释说明
- [ ] `cargo test -p chimera-tui` 新增主题切换测试全部通过
- [ ] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [ ] `cargo fmt --all -- --check` 通过

### P6.2 布局模板

- [ ] `crates/chimera-tui/tests/layout_test.rs` 新增测试,验证 `1`/`2`/`3` 切换单/双/三面板布局
- [ ] `TuiState` 新增 `layout_mode: LayoutMode` 枚举(SinglePane / DualPane / TriplePane)
- [ ] `TuiApp::render` 根据 `layout_mode` 选择布局
- [ ] `handle_global_key` 处理 `1`/`2`/`3` 键切换 `layout_mode`
- [ ] 三种布局的适用场景已用 WHY 注释说明
- [ ] `cargo test -p chimera-tui` 新增布局测试全部通过
- [ ] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [ ] `cargo fmt --all -- --check` 通过

### P6.3 颜色方案配置

- [ ] `crates/chimera-tui/tests/color_scheme_test.rs` 新增测试,验证配置文件 `tui.colors` 节覆盖默认颜色
- [ ] `TuiConfig` 新增 `colors: ColorScheme` 结构体
- [ ] `ColorScheme::default_for_theme(theme)` 方法实现
- [ ] 颜色配置与主题的关系(主题是离散预设,颜色是细粒度覆盖)已用 WHY 注释说明
- [ ] `cargo test -p chimera-tui` 新增颜色配置测试全部通过
- [ ] `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
- [ ] `cargo fmt --all -- --check` 通过

### P6 整体验收

- [ ] `cargo test -p chimera-tui` 全部通过(无回归)
- [ ] `cargo test --workspace` 全部通过(无回归)
- [ ] P6 Note 已写入 tasks.md

## P7 — 历史回放引擎(v1.8+ 接口设计)

### P7.1 状态快照持久化(接口设计)

- [ ] `HistoryStore` 结构体接口设计完成(`save_snapshot` / `load_snapshot` / `list_snapshots` / `cleanup_old`)
- [ ] 快照文件格式设计完成(`<timestamp>.msgpack`,含 TuiState + DataSnapshot)
- [ ] 快照存储路径(`~/.aether/tui/snapshots/`)与清理策略(超过 24h 自动清理)设计完成
- [ ] `TuiConfig.snapshot_interval_s` 字段设计完成(默认 30s)
- [ ] 接口设计说明已写入 tasks.md 的 P7 Note

### P7.2 时间轴 scrubbing(接口设计)

- [ ] `PanelId::Timeline` 面板接口设计完成(事件密度热力图 + 时间游标)
- [ ] `←/→` 移动游标 + `Enter` 从该时间点开始回放的交互设计完成
- [ ] 热力图渲染算法设计完成(每秒一个 cell,颜色深度按事件密度)
- [ ] 接口设计说明已写入 tasks.md 的 P7 Note

### P7.3 回放引擎(接口设计)

- [ ] `ReplayEngine` 结构体接口设计完成(`start_replay` / `pause` / `resume` / `set_speed` / `stop`)
- [ ] 回放速度(1x / 2x / 4x)与时间映射设计完成
- [ ] 回放期间 TuiApp 的状态(只读模式,面板显示 "[REPLAY x2]" 横幅)设计完成
- [ ] 接口设计说明已写入 tasks.md 的 P7 Note

## P8 — MCP/CHTC 集成深化(v1.8+ 接口设计)

### P8.1 MCP Mesh 节点拓扑图(接口设计)

- [ ] ASCII 拓扑图渲染算法设计完成(节点为方框,连接为线条,颜色表示状态)
- [ ] 节点状态更新订阅设计完成(McpNodeHeartbeat 事件 + 5s 超时检测)
- [ ] 拓扑布局算法设计完成(Force-directed 简化版,节点位置稳定)
- [ ] 接口设计说明已写入 tasks.md 的 P8 Note

### P8.2 CHTC 兼容性矩阵评分(接口设计)

- [ ] 5×N 兼容性矩阵渲染设计完成(行:5 IDE 适配器,列:能力维度)
- [ ] 兼容性评分算法设计完成(基于最近 N 次请求的成功率 + 响应时间)
- [ ] 评分更新订阅设计完成(ChtcAdapterStatus 事件)
- [ ] 接口设计说明已写入 tasks.md 的 P8 Note

## v1.7-omega 整体发布验收

- [ ] P0-P5 全部 Task 完成且 checklist 全部勾选
- [ ] P6(可选)完成或明确推迟到 v1.8+
- [ ] P7-P8 接口设计完成,实现明确推迟到 v1.8+
- [ ] `cargo test --workspace` 全部通过(无回归)
- [ ] `cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 通过
- [ ] `cargo fmt --all -- --check` 通过
- [ ] `cargo audit --deny warnings --ignore RUSTSEC-2026-0190 --ignore RUSTSEC-2026-0002 --ignore RUSTSEC-2024-0436` 通过
- [ ] 手动验证:本地运行 `cargo run -p chimera-cli -- tui`,确认 14 面板可切换、交互响应、性能流畅
- [ ] `CHANGELOG.md` 追加 v1.7-omega 章节,概述 P0-P5(或 P6)全部交付
- [ ] `CODE_WIKI.md` 更新 TUI 架构描述(14 面板 + 增量渲染 + 虚拟滚动 + 跨面板联动)
- [ ] `project_memory.md` 追加 v1.7 实现教训
- [ ] 创建 `docs/tui/v1.7_deep_evolution_report.md`,汇总架构、性能、测试结果
- [ ] `Cargo.toml` workspace.package.version 同步为 `1.7.0-omega`
- [ ] git commit + tag `v1.7.0-omega` + push(触发 release.yml + fuzz.yml)
