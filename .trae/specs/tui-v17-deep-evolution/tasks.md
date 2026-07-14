# Tasks — TUI v1.7-omega 深化演进

> **change-id**: `tui-v17-deep-evolution`
> **版本定位**: v1.7-omega(中版本增强,2-4 周迭代)
> **基线**: chimera-tui M0-M5 已交付(8 面板 + Panel trait + 双向控制 + 鼠标 + 弹窗 + 搜索过滤)
> **参考 CLI**: Claude Code / Qodex / Kimi Code CLI
>
> **执行规范**:
> - 严格遵循 §2.2 依赖铁律(L10 仅依赖 L1 event-bus + nexus-core)
> - 所有 crate 维持 `#![forbid(unsafe_code)]`
> - 每个 Task 遵循 TDD(RED-GREEN-REFACTOR)
> - 子代理产物必须 `cargo fmt --all` + `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
> - 每个 Task 完成后勾选 `[x]`,并通过 checklist.md 全部检查项才能进入下一 Task
>
> **协作模式**: 精英专家级子代理团队(架构设计 + TUI 渲染 + 事件总线 + 性能优化 + 测试),系统性分布式深度分析 + 多轮结构化验证。
>
> **分阶段交付策略**:
> - v1.7 必交付:P0-P5
> - v1.7 可选:P6
> - v1.8+ 演进:P7-P8(本 spec 设计接口,实现推迟)

---

## P0 — 现状评估与架构基线设计(~4h)

> **依赖**: 无(基于当前 master 分支)
> **并行性**: P0.1 / P0.2 / P0.3 可并行
> **验收门槛**: 架构基线文档 + 参考 CLI 调研报告 + PanelId 扩展设计确认

- [ ] **Task P0.1: 现状深度审计**
  - [ ] SubTask P0.1.1: Read `crates/chimera-tui/src/app.rs` 完整事件循环,识别当前快捷键处理逻辑与可扩展点
  - [ ] SubTask P0.1.2: Read `crates/chimera-tui/src/panels/mod.rs` + 8 个面板模块,梳理 Panel trait 实现模式与可复用组件(list_state / filtered_xxx / content 方法)
  - [ ] SubTask P0.1.3: Read `crates/chimera-tui/src/data.rs` DataPipeline + DataSnapshot,识别快照字段缺口与新面板所需数据
  - [ ] SubTask P0.1.4: Read `crates/event-bus/src/types.rs`,列出所有事件变体,识别需要新增的事件(DecayMetricsReported / RouterStatsReported / McpNodeHeartbeat / ChtcAdapterStatus)
  - [ ] SubTask P0.1.5: Read `crates/chimera-tui/src/render.rs` + `popup.rs`,识别渲染辅助函数与弹窗类型,规划新增 EventDetail overlay 与 Help overlay 的扩展点

- [ ] **Task P0.2: 参考 CLI 调研**
  - [ ] SubTask P0.2.1: 调研 Claude Code CLI 的 TUI 设计要素(流式输出 / 工具调用展开折叠 / 命令模式 / 上下文显示),提取可借鉴的交互模式
  - [ ] SubTask P0.2.2: 调研 Qodex CLI 的任务导向界面与进度条设计,提取 Quest 面板进度条增强方向
  - [ ] SubTask P0.2.3: 调研 Kimi Code CLI 的对话式 TUI 与代码块语法高亮,提取详情 overlay 的 JSON 高亮方案
  - [ ] SubTask P0.2.4: 汇总三参考 CLI 的设计要素,产出"参考借鉴清单"(写入 spec.md 的 H 节参考风格融合部分)

- [ ] **Task P0.3: 架构基线设计**
  - [ ] SubTask P0.3.1: 设计 `PanelId` 从 8 变体扩展到 14 变体的迁移方案(新增 Decay/EventStream/Router/McpNodes/Chtc/Timeline 共 6 个,不含 Help 已有)
  - [ ] SubTask P0.3.2: 设计 `TuiState` 字段扩展方案(新增 decay_metrics / router_metrics / mcp_nodes / chtc_state / timeline_snapshots / fps / dirty_panels / auto_scroll)
  - [ ] SubTask P0.3.3: 设计 `DataSnapshot` 字段扩展方案(与 TuiState 对齐,供 DataPipeline 同步)
  - [ ] SubTask P0.3.4: 设计 `TuiConfig` 字段扩展方案(tick_interval_ms / snapshot_interval_s / max_event_history / max_snapshots)
  - [ ] SubTask P0.3.5: 设计 4 个新事件变体的载荷结构(DecayMetricsReported / RouterStatsReported / McpNodeHeartbeat / ChtcAdapterStatus)
  - [ ] SubTask P0.3.6: 产出架构基线摘要,作为 P2-P8 实施的设计依据(写入本 tasks.md 的 P0 Note)

### P0 Note — 架构基线(待 P0 完成后填写)

> P0 完成后,在此处记录:
> - PanelId 扩展后的完整循环顺序
> - TuiState 新增字段的类型定义
> - DataSnapshot 新增字段与 DataPipeline 同步策略
> - 4 个新事件变体的载荷定义
> - 参考 CLI 借鉴清单的具体实施映射

---

## P1 — 提交 P3 归档(~1h)

> **依赖**: 无(独立动作,与 P0 并行)
> **并行性**: 与 P0 完全并行
> **验收门槛**: master 分支包含 P3 全部交付物 + 规格文档归档

- [ ] **Task P1.1: 提交 realtime-data-driven-tui-panels 规格的 P3 交付物**
  - [ ] SubTask P1.1.1: `git status` 确认未提交变更(应包含 tasks.md / checklist.md 的 P3 勾选 + P3.1/P3.2/P3.3 全部实施产物)
  - [ ] SubTask P1.1.2: `git diff --stat` 核验变更范围(预期仅 chimera-tui + specs/realtime-data-driven-tui-panels + CHANGELOG.md + CODE_WIKI.md + docs/tui/)
  - [ ] SubTask P1.1.3: `git add` 逐文件添加(遵守 §协作偏好:不使用 `git add -A` / `git add .`)
  - [ ] SubTask P1.1.4: `git commit -m` 使用 HEREDOC 传入 commit message,描述 P3 验证 + 基准 + 文档归档
  - [ ] SubTask P1.1.5: `git push origin master` 推送到远端
  - [ ] SubTask P1.1.6: `git log --oneline -3` 确认提交成功

---

## P2 — 新增 5 监控面板(~12h)

> **依赖**: P0 完成(架构基线 + 事件变体设计)
> **并行性**: P2.1-P2.5 五个面板可完全并行(独立模块,无相互依赖)
> **验收门槛**: 5 个新面板单元测试 + 集成测试全部通过;`cargo test -p chimera-tui` 无回归

- [ ] **Task P2.1: Decay 衰减面板**
  - [ ] SubTask P2.1.1: TDD-RED — 在 `crates/chimera-tui/tests/decay_panel_test.rs` 新增测试:订阅 DecayMetricsReported 事件后,Decay 面板渲染内容包含衰减系数与 sparkline
  - [ ] SubTask P2.1.2: TDD-GREEN — 新增 `crates/chimera-tui/src/panels/decay.rs`,实现 `DecayPanel` 结构体 + `Panel` trait
  - [ ] SubTask P2.1.3: 实现 sparkline 渲染(复用 `render::sparkline` 辅助函数),衰减系数 > 0.7 红色高亮
  - [ ] SubTask P2.1.4: 在 `event-bus/src/types.rs` 新增 `DecayMetricsReported` 事件变体(载荷:coefficient f32 / recent_events Vec<String> / timestamp)
  - [ ] SubTask P2.1.5: 在 `DataPipeline` 中新增 `DecaySync` 适配器,订阅事件并维护 `decay_metrics` 状态
  - [ ] SubTask P2.1.6: TDD-REFACTOR — 提取 sparkline 高亮阈值常量,WHY 注释说明 0.7 阈值依据

- [ ] **Task P2.2: EventStream 全量事件流面板**
  - [ ] SubTask P2.2.1: TDD-RED — 新增测试:EventStream 面板支持万级事件虚拟滚动,帧时间 < 16ms
  - [ ] SubTask P2.2.2: TDD-GREEN — 新增 `crates/chimera-tui/src/panels/event_stream.rs`,实现 `EventStreamPanel` + `Panel` trait
  - [ ] SubTask P2.2.3: 实现虚拟滚动(virtual_scroll_window 辅助函数,仅渲染可见区域 + 上下 5 行缓冲)
  - [ ] SubTask P2.2.4: 实现流式追加 + 自动滚动(`auto_scroll` 标记,用户滚动时暂停,底部显示 "[新事件 N 条]")
  - [ ] SubTask P2.2.5: 实现事件类型筛选(复用 `filter_topic` / `filter_level` 字段)
  - [ ] SubTask P2.2.6: TDD-REFACTOR — 提取虚拟滚动逻辑到 `render::virtual_scroll_window`,WHY 注释说明缓冲行数选择

- [ ] **Task P2.3: Router 路由统计面板**
  - [ ] SubTask P2.3.1: TDD-RED — 新增测试:RouterStatsReported 事件到达后,Router 面板显示三路由器命中率与延迟 P50/P95/P99
  - [ ] SubTask P2.3.2: TDD-GREEN — 新增 `crates/chimera-tui/src/panels/router.rs`,实现 `RouterPanel` + `Panel` trait
  - [ ] SubTask P2.3.3: 实现三路由器命中率进度条(复用 `render::utilization_bar`)
  - [ ] SubTask P2.3.4: 实现热点 capability_id Top-10 列表(使用 `select_nth_unstable` O(n) Top-K,遵守 §4.1)
  - [ ] SubTask P2.3.5: 在 `event-bus/src/types.rs` 新增 `RouterStatsReported` 事件变体(载荷:kvbsr_stats / sesa_stats / faae_stats 各含 hit_rate / p50 / p95 / p99 / hot_capabilities)
  - [ ] SubTask P2.3.6: TDD-REFACTOR — 提取延迟统计渲染为辅助函数,WHY 注释说明 P50/P95/P99 三列对比的可读性优势

- [ ] **Task P2.4: MCP 节点状态面板**
  - [ ] SubTask P2.4.1: TDD-RED — 新增测试:McpNodeHeartbeat 事件到达后,McpNodes 面板显示节点列表与连接状态
  - [ ] SubTask P2.4.2: TDD-GREEN — 新增 `crates/chimera-tui/src/panels/mcp_nodes.rs`,实现 `McpNodesPanel` + `Panel` trait
  - [ ] SubTask P2.4.3: 实现节点列表渲染(节点 ID / 连接状态绿黄红 / 最近消息吞吐量)
  - [ ] SubTask P2.4.4: 实现离线告警横幅(心跳 > 5s 未到达显示 "[ALERT] Node X offline")
  - [ ] SubTask P2.4.5: 在 `event-bus/src/types.rs` 新增 `McpNodeHeartbeat` 事件变体(载荷:node_id / status / throughput / last_seen)
  - [ ] SubTask P2.4.6: TDD-REFACTOR — WHY 注释说明 5s 心跳超时阈值的选择依据

- [ ] **Task P2.5: CHTC 跨平台适配器面板**
  - [ ] SubTask P2.5.1: TDD-RED — 新增测试:ChtcAdapterStatus 事件到达后,Chtc 面板显示 5 IDE 适配器兼容性评分
  - [ ] SubTask P2.5.2: TDD-GREEN — 新增 `crates/chimera-tui/src/panels/chtc.rs`,实现 `ChtcPanel` + `Panel` trait
  - [ ] SubTask P2.5.3: 实现 5 适配器兼容性评分展示(评分 < 60 黄色高亮)
  - [ ] SubTask P2.5.4: 实现最近请求类型分布 sparkline
  - [ ] SubTask P2.5.5: 在 `event-bus/src/types.rs` 新增 `ChtcAdapterStatus` 事件变体(载荷:adapter_id / adapter_type / compatibility_score / recent_requests)
  - [ ] SubTask P2.5.6: TDD-REFACTOR — WHY 注释说明 5 IDE 适配器列表与兼容性评分算法

### P2 Note — 实现说明(待 P2 完成后填写)

> P2 完成后,在此处记录:
> - 5 个新面板的 PanelId 循环顺序
> - DataSnapshot 新增字段与 DataPipeline 同步实现
> - 4 个新事件变体的实际载荷定义
> - 虚拟滚动与 sparkline 复用情况
> - 测试覆盖率与性能基准结果

---

## P3 — 交互能力升级(~10h)

> **依赖**: P0 完成(架构基线);P2 完成(EventStream 面板存在,Enter 详情才有载体)
> **并行性**: P3.1 / P3.2 / P3.3 可部分并行
> **验收门槛**: 交互测试全部通过;`cargo test -p chimera-tui` 无回归

- [ ] **Task P3.1: Enter 事件详情 overlay**
  - [ ] SubTask P3.1.1: TDD-RED — 新增测试:在 EventStream 面板选中事件后按 Enter,弹出 Detail overlay 显示完整载荷
  - [ ] SubTask P3.1.2: TDD-GREEN — 在 `popup.rs` 新增 `PopupKind::EventDetail` 变体(载荷:event_type / payload_msgpack / payload_decoded / related_event_ids)
  - [ ] SubTask P3.1.3: 实现 MessagePack 解码为 JSON(使用 rmp-serde,已在 workspace 依赖)
  - [ ] SubTask P3.1.4: 实现 JSON 语法高亮(关键字段着色:字符串绿色 / 数字青色 / 布尔黄色 / null 灰色)
  - [ ] SubTask P3.1.5: 实现上下游事件 ID 链展示(底部显示 related_event_ids,Tab 跳转)
  - [ ] SubTask P3.3.6: 在 Parliament / Log / EventStream 三面板的 `handle_key` 中处理 Enter 键,产生 `TuiCommand::ShowEventDetail(event_index)`
  - [ ] SubTask P3.1.7: TDD-REFACTOR — WHY 注释说明 MessagePack 解码失败时的降级策略(显示原始 hex)

- [ ] **Task P3.2: Help overlay(? 键触发)**
  - [ ] SubTask P3.2.1: TDD-RED — 新增测试:任意面板按 `?` 键,弹出 Help overlay 显示快捷键说明,不切换面板
  - [ ] SubTask P3.2.2: TDD-GREEN — 在 `popup.rs` 新增 `PopupKind::HelpOverlay` 变体(载荷:快捷键表 Vec<(key, description)>)
  - [ ] SubTask P3.2.3: 实现 Help overlay 渲染(居中浮窗,半透明背景,Esc 关闭)
  - [ ] SubTask P3.2.4: 在 `TuiApp::handle_key` 中全局拦截 `?` 键,产生 `TuiCommand::ShowHelp`
  - [ ] SubTask P3.2.5: TDD-REFACTOR — WHY 注释说明 Help overlay 与 Help 面板的区别(overlay 不切换焦点,临时参考)

- [ ] **Task P3.3: 全局快捷键系统统一**
  - [ ] SubTask P3.3.1: TDD-RED — 新增测试:`j/k` 上下导航、`g/G` 跳顶底、`/` 搜索、`:` 命令、`?` 帮助、`Tab/Shift+Tab` 切换面板、`数字` 跳转面板 全部生效
  - [ ] SubTask P3.3.2: TDD-GREEN — 重构 `TuiApp::handle_key`,将全局快捷键提取到 `handle_global_key` 方法,面板特定键 fallback 到 `Panel::handle_key`
  - [ ] SubTask P3.3.3: 实现数字键 1-9 跳转面板(1=Quest, 2=Parliament, ... 9=Timeline,超过 9 用 `g` 前缀 + 数字)
  - [ ] SubTask P3.3.4: 实现 `g/G` 跳顶/底(委托给当前面板的 `scroll_to_top/bottom` 方法,Panel trait 新增默认实现)
  - [ ] SubTask P3.3.5: TDD-REFACTOR — WHY 注释说明全局键与面板键的优先级(全局键先拦截,未命中再委托面板)

- [ ] **Task P3.4: 流式事件追加(Claude Code 风格)**
  - [ ] SubTask P3.4.1: TDD-RED — 新增测试:EventStream 面板在 auto_scroll=true 时,新事件到达自动滚动到底部
  - [ ] SubTask P3.4.2: TDD-GREEN — 实现 `auto_scroll` 状态字段,用户手动滚动时设为 false,新事件到达时若 false 则显示 "[新事件 N 条]" 提示
  - [ ] SubTask P3.4.3: 实现 "跳到底部" 快捷键 `G`(在 auto_scroll=false 时,按 G 跳到底部并恢复 auto_scroll=true)
  - [ ] SubTask P3.4.4: TDD-REFACTOR — WHY 注释说明 auto_scroll 策略(参考 Claude Code 工具调用流式输出的 UX)

### P3 Note — 实现说明(待 P3 完成后填写)

---

## P4 — 性能优化(~10h)

> **依赖**: P0 完成;P2 完成(虚拟滚动需在 EventStream 面板验证)
> **并行性**: P4.1 / P4.2 / P4.3 / P4.4 可部分并行
> **验收门槛**: 性能基准满足 P95 阈值;`cargo test -p chimera-tui` 无回归

- [ ] **Task P4.1: 增量渲染**
  - [ ] SubTask P4.1.1: TDD-RED — 新增测试:仅 Budget 面板数据更新时,其他面板的 `Panel::render` 不被调用
  - [ ] SubTask P4.1.2: TDD-GREEN — 在 `TuiState` 新增 `dirty_panels: HashSet<PanelId>` 字段,数据更新时标记对应面板为 dirty
  - [ ] SubTask P4.1.3: 重构 `TuiApp::render`,仅对 `dirty_panels` 中的面板调用 `Panel::render`,其他面板保留上一帧 Buffer
  - [ ] SubTask P4.1.4: 实现帧结束清空 `dirty_panels`(渲染后清空,下一帧重新标记)
  - [ ] SubTask P4.1.5: TDD-REFACTOR — WHY 注释说明 dirty 标记策略(数据驱动 vs 时间驱动)

- [ ] **Task P4.2: 虚拟滚动(已部分实现,扩展到 Parliament)**
  - [ ] SubTask P4.2.1: TDD-RED — 新增测试:Parliament 面板支持 1000 条历史事件虚拟滚动
  - [ ] SubTask P4.2.2: TDD-GREEN — 将 `render::virtual_scroll_window`(P2.2.3 实现)应用到 Parliament 面板
  - [ ] SubTask P4.2.3: TDD-REFACTOR — WHY 注释说明虚拟滚动缓冲行数(5 行)的选择依据

- [ ] **Task P4.3: 可调 tick 暴露**
  - [ ] SubTask P4.3.1: TDD-RED — 新增测试:`TuiConfig { tick_interval_ms: 100, .. }` 时,DataPipeline 实际使用 100ms tick
  - [ ] SubTask P4.3.2: TDD-GREEN — 在 `TuiConfig` 新增 `tick_interval_ms: u16` 字段(默认 250,范围 100-1000)
  - [ ] SubTask P4.3.3: 在 `TuiApp::new` 中将 `config.tick_interval_ms` 传递给 `DataPipeline::new` 的 `DataSourceConfig`
  - [ ] SubTask P4.3.4: 在 `chimera-cli/src/commands/tui.rs` 新增 `--tick <ms>` CLI 参数,覆盖配置
  - [ ] SubTask P4.3.5: TDD-REFACTOR — WHY 注释说明 100-1000ms 范围的依据(过低 CPU 占用高,过高事件延迟感知差)

- [ ] **Task P4.4: FPS 显示**
  - [ ] SubTask P4.4.1: TDD-RED — 新增测试:Health 面板渲染内容包含 "FPS: <n>",FPS < 30 黄色 / < 15 红色
  - [ ] SubTask P4.4.2: TDD-GREEN — 在 `TuiApp` 新增 `fps_tracker: FpsTracker` 结构体,基于 `frame_count` 与时间戳计算 FPS
  - [ ] SubTask P4.4.3: 在 `TuiState` 新增 `fps: u16` 字段,每秒更新一次
  - [ ] SubTask P4.4.4: 在 `HealthPanel::render` 中显示 `FPS: <n>`,根据阈值着色
  - [ ] SubTask P4.4.5: TDD-REFACTOR — WHY 注释说明 FPS 计算窗口(1 秒滑动平均)

### P4 Note — 实现说明(待 P4 完成后填写)

---

## P5 — 跨面板联动(~6h)

> **依赖**: P0 完成;P3 完成(Esc 状态保留依赖 InputMode 处理)
> **并行性**: P5.1 / P5.2 可并行
> **验收门槛**: 联动测试通过;`cargo test -p chimera-tui` 无回归

- [ ] **Task P5.1: Esc 状态保留**
  - [ ] SubTask P5.1.1: TDD-RED — 新增测试:TUI 退出后 `~/.aether/tui/session.json` 存在,包含当前面板 + 过滤器 + 滚动位置;下次启动恢复
  - [ ] SubTask P5.1.2: TDD-GREEN — 新增 `crates/chimera-tui/src/session.rs` 模块,实现 `TuiSession` 结构体(序列化当前面板 + 过滤器 + 各面板滚动位置)
  - [ ] SubTask P5.1.3: 实现 `TuiSession::save` 方法,退出时序列化到 `~/.aether/tui/session.json`
  - [ ] SubTask P5.1.4: 实现 `TuiSession::load` 方法,启动时反序列化恢复状态
  - [ ] SubTask P5.1.5: 在 `TuiApp::run` 退出时调用 `TuiSession::save`,在 `TuiApp::new` 中尝试 `TuiSession::load`
  - [ ] SubTask P5.1.6: TDD-REFACTOR — WHY 注释说明状态保留的范围(仅 UI 状态,不含数据快照;数据快照在 P7 历史回放处理)

- [ ] **Task P5.2: Quest/Event 跳转底层命令**
  - [ ] SubTask P5.2.1: TDD-RED — 新增测试:Quest 面板选中 Quest 后按 Enter 查看详情,再按 `w` 触发 `chimera wiki <quest_id>` 子命令
  - [ ] SubTask P5.2.2: TDD-GREEN — 在 `TuiCommand` 新增 `InvokeCliSubcommand { command: String, args: Vec<String> }` 变体
  - [ ] SubTask P5.2.3: 在 Quest 面板详情 overlay 中处理 `w` 键,产生 `TuiCommand::InvokeCliSubcommand { command: "wiki", args: [quest_id] }`
  - [ ] SubTask P5.2.4: 在 Log 面板详情 overlay 中处理 `s` 键,产生 `TuiCommand::InvokeCliSubcommand { command: "quest", args: [related_quest_id] }`
  - [ ] SubTask P5.2.5: 在 `TuiApp::apply_command` 中实现子命令调用:暂停 TUI raw mode → 执行 `std::process::Command::new("chimera").arg(command).args(args).status()` → 等待任意键 → 恢复 raw mode
  - [ ] SubTask P5.2.6: TDD-REFACTOR — WHY 注释说明 raw mode 切换的必要性(子命令输出需要正常终端模式)

### P5 Note — 实现说明(待 P5 完成后填写)

---

## P6 — 主题运行时切换与布局模板(~4h,v1.7 可选)

> **依赖**: P0 完成
> **并行性**: P6.1 / P6.2 / P6.3 可并行
> **验收门槛**: 主题切换测试通过;`cargo test -p chimera-tui` 无回归

- [ ] **Task P6.1: 运行时主题切换**
  - [ ] SubTask P6.1.1: TDD-RED — 新增测试:按 `t` 键,主题从 Dark → Light → HighContrast → Dark 循环切换,所有面板立即重绘
  - [ ] SubTask P6.1.2: TDD-GREEN — 在 `Theme` 枚举新增 `HighContrast` 变体
  - [ ] SubTask P6.1.3: 实现 `Theme::next()` 方法(Dark → Light → HighContrast → Dark 循环)
  - [ ] SubTask P6.1.4: 在 `TuiApp::handle_global_key` 中处理 `t` 键,切换 `config.theme` 并标记所有面板为 dirty
  - [ ] SubTask P6.1.5: 实现 HighContrast 主题的颜色方案(纯黑背景 / 纯白前景 / 高饱和度强调色)
  - [ ] SubTask P6.1.6: TDD-REFACTOR — WHY 注释说明 HighContrast 主题的目标用户(色盲 + 高亮环境)

- [ ] **Task P6.2: 布局模板**
  - [ ] SubTask P6.2.1: TDD-RED — 新增测试:按 `1` 单面板全屏,`2` 双面板分屏,`3` 三面板布局
  - [ ] SubTask P6.2.2: TDD-GREEN — 在 `TuiState` 新增 `layout_mode: LayoutMode` 枚举(SinglePane / DualPane / TriplePane)
  - [ ] SubTask P6.2.3: 实现 `TuiApp::render` 根据 `layout_mode` 选择布局(SinglePane: 当前面板全屏;DualPane: 主面板 + 侧边栏;TriplePane: 主面板 + 侧边栏 + 底部日志)
  - [ ] SubTask P6.2.4: 在 `handle_global_key` 中处理 `1`/`2`/`3` 键切换 `layout_mode`
  - [ ] SubTask P6.2.5: TDD-REFACTOR — WHY 注释说明三种布局的适用场景

- [ ] **Task P6.3: 颜色方案配置**
  - [ ] SubTask P6.3.1: TDD-RED — 新增测试:配置文件 `~/.aether/omega.yaml` 的 `tui.colors` 节可覆盖默认颜色
  - [ ] SubTask P6.3.2: TDD-GREEN — 在 `TuiConfig` 新增 `colors: ColorScheme` 结构体(各面板的 foreground / background / accent 字段)
  - [ ] SubTask P6.3.3: 实现 `ColorScheme::default_for_theme(theme)` 方法,根据主题返回默认颜色
  - [ ] SubTask P6.3.4: TDD-REFACTOR — WHY 注释说明颜色配置与主题的关系(主题是离散预设,颜色是细粒度覆盖)

### P6 Note — 实现说明(待 P6 完成后填写)

---

## P7 — 历史回放引擎(~10h,v1.8+ 演进,本 spec 仅设计接口)

> **依赖**: P0 完成;P2 完成(EventStream 数据源)
> **并行性**: P7.1 / P7.2 / P7.3 串行(快照 → 时间轴 → 回放引擎)
> **验收门槛**: 接口设计完成;实现推迟到 v1.8+

- [ ] **Task P7.1: 状态快照持久化**
  - [ ] SubTask P7.1.1: 设计 `HistoryStore` 结构体接口(`save_snapshot` / `load_snapshot` / `list_snapshots` / `cleanup_old`)
  - [ ] SubTask P7.1.2: 设计快照文件格式(`<timestamp>.msgpack`,含 TuiState + DataSnapshot)
  - [ ] SubTask P7.1.3: 设计快照存储路径(`~/.aether/tui/snapshots/`)与清理策略(超过 24h 自动清理)
  - [ ] SubTask P7.1.4: 设计 `TuiConfig.snapshot_interval_s` 字段(默认 30s)

- [ ] **Task P7.2: 时间轴 scrubbing**
  - [ ] SubTask P7.2.1: 设计 `PanelId::Timeline` 面板接口(事件密度热力图 + 时间游标)
  - [ ] SubTask P7.2.2: 设计 `←/→` 移动游标 + `Enter` 从该时间点开始回放的交互
  - [ ] SubTask P7.2.3: 设计热力图渲染算法(每秒一个 cell,颜色深度按事件密度)

- [ ] **Task P7.3: 回放引擎**
  - [ ] SubTask P7.3.1: 设计 `ReplayEngine` 结构体接口(`start_replay` / `pause` / `resume` / `set_speed` / `stop`)
  - [ ] SubTask P7.3.2: 设计回放速度(1x / 2x / 4x)与时间映射(回放时间 = 实际时间 × speed)
  - [ ] SubTask P7.3.3: 设计回放期间 TuiApp 的状态(只读模式,面板显示 "[REPLAY x2]" 横幅)

### P7 Note — 接口设计说明(待 P7 完成后填写)

---

## P8 — MCP/CHTC 集成深化(~8h,v1.8+ 演进,本 spec 仅设计接口)

> **依赖**: P0 完成;P2 完成(McpNodes / Chtc 面板基础渲染已交付)
> **并行性**: P8.1 / P8.2 可并行
> **验收门槛**: 接口设计完成;实现推迟到 v1.8+

- [ ] **Task P8.1: MCP Mesh 节点拓扑图**
  - [ ] SubTask P8.1.1: 设计 ASCII 拓扑图渲染算法(节点为方框,连接为线条,颜色表示状态)
  - [ ] SubTask P8.1.2: 设计节点状态更新订阅(McpNodeHeartbeat 事件 + 5s 超时检测)
  - [ ] SubTask P8.1.3: 设计拓扑布局算法(Force-directed 简化版,节点位置稳定)

- [ ] **Task P8.2: CHTC 兼容性矩阵评分**
  - [ ] SubTask P8.2.1: 设计 5×N 兼容性矩阵渲染(行:5 IDE 适配器,列:能力维度)
  - [ ] SubTask P8.2.2: 设计兼容性评分算法(基于最近 N 次请求的成功率 + 响应时间)
  - [ ] SubTask P8.2.3: 设计评分更新订阅(ChtcAdapterStatus 事件)

### P8 Note — 接口设计说明(待 P8 完成后填写)

---

## Task Dependencies

- **P0(现状评估 + 架构基线)**:无依赖,必须先行
- **P1(P3 归档提交)**:无依赖,与 P0 完全并行
- **P2(5 监控面板)**:依赖 P0(事件变体设计);5 面板内部可并行
- **P3(交互升级)**:依赖 P0;P3.1(Enter 详情)依赖 P2.2(EventStream 面板)
- **P4(性能优化)**:依赖 P0;P4.2(虚拟滚动扩展)依赖 P2.2
- **P5(跨面板联动)**:依赖 P0 + P3
- **P6(主题与布局)**:依赖 P0,可与 P2-P5 并行
- **P7(历史回放)**:依赖 P0 + P2,接口设计可在 P2 完成后启动,实现推迟 v1.8+
- **P8(MCP/CHTC 深化)**:依赖 P0 + P2,接口设计可在 P2 完成后启动,实现推迟 v1.8+

## 并行执行建议

- **批次 1(并行)**:P0(架构基线)+ P1(P3 归档提交)
- **批次 2(并行)**:P2.1-P2.5 五个新面板 + P6 主题布局
- **批次 3(并行)**:P3 交互升级 + P4 性能优化(P4.2 依赖 P2.2 完成后启动)
- **批次 4(串行)**:P5 跨面板联动
- **批次 5(并行,接口设计)**:P7 历史回放接口 + P8 MCP/CHTC 接口
- **批次 6(v1.8+ 实现)**:P7 + P8 实现(本 spec 不涵盖)

> **长期主义原则**:
> - 不跳级,不并行跨档;每批次完成后 `cargo fmt --all` + `cargo clippy` + `cargo test -p chimera-tui`
> - 每批次完成后 git commit + push,保持工作树干净
> - v1.7 必交付 P0-P5;P6 可选;P7-P8 仅设计接口
> - 代码质量:清晰模块化 + 高可读性 + 完善注释(功能描述 + 参数说明 + 关键算法解释)+ 行业最佳实践
