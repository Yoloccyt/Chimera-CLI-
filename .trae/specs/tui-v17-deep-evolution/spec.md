# TUI v1.7-omega 深化演进 Spec

> **change-id**: `tui-v17-deep-evolution`
> **版本定位**: v1.7-omega(中版本增强,2-4 周迭代周期)
> **基线**: 基于 `realtime-data-driven-tui-panels` 规格 P0-P3 全部交付后的 chimera-tui(M0-M5 完成)
> **参考 CLI**: Claude Code CLI / Qodex CLI / Kimi Code CLI(整体设计风格参考)

---

## Why

chimera-tui 已完成 M0-M5 全部迭代(8 面板 + Panel trait 插件化 + EventBus 双向控制 + 鼠标 + 弹窗栈 + 搜索过滤 + 主题枚举),但仍存在三类核心缺口:

1. **监控覆盖不全**:Decay 衰减曲线、EventStream 全量事件流、Router(KVBSR+SESA)路由统计、MCP Mesh 节点状态、CHTC 跨平台适配器五个关键运维视角缺失,运维人员无法在 TUI 内完成全链路观测。
2. **交互体验未达参考 CLI 水准**:Claude Code / Qodex / Kimi Code 普遍具备事件详情 overlay、运行时主题切换、跨面板跳转、历史回放等交互能力,本项目 TUI 当前仅支持基础导航与命令面板,与参考 CLI 存在体验差距。
3. **性能可持续性未夯实**:当前 250ms tick + 全量重绘模式在事件密度提升(1000+ 事件/秒)时存在渲染瓶颈风险,缺少增量渲染与虚拟滚动机制,长期运行的可维护性与可扩展性不足。

本规格通过新增 5 个监控面板、交互能力升级、性能优化、历史回放、跨面板联动、MCP/CHTC 集成、主题运行时切换七大方向,将 chimera-tui 从"功能完备"推进到"运维就绪 + 体验对齐参考 CLI"。

---

## What Changes

### A. 新增监控面板(5 个)

- 新增 **Decay 衰减面板**(`PanelId::Decay`):可视化 `decay-engine` 的能力衰减曲线、当前衰减系数、最近衰减事件
- 新增 **EventStream 全量事件流面板**(`PanelId::EventStream`):与现有 Log 面板(摘要 10 条)区分,提供可滚动的全量事件流 + 事件类型筛选 + 时间范围过滤
- 新增 **Router 路由统计面板**(`PanelId::Router`):KVBSR + SESA + FaaE 三大路由器的命中率、路由延迟、热点 capability_id 统计
- 新增 **MCP 节点状态面板**(`PanelId::McpNodes`):MCP Mesh 节点拓扑、连接状态、消息吞吐量
- 新增 **CHTC 跨平台适配器面板**(`PanelId::Chtc`):5 IDE 适配器(VSCode/JetBrains/Vim/Emacs/LSP)的连接状态、最近请求、兼容性矩阵

### B. 交互能力升级

- **Enter 查看事件详情**:在 Parliament / Log / EventStream 面板按 Enter 弹出详情 overlay(完整事件载荷 + MessagePack 解码 + 上下游事件链)
- **? Help overlay**:全局快捷键 `?` 触发帮助 overlay(浮动在当前面板之上,不切换面板),Esc 关闭
- **全局快捷键系统增强**:统一 `j/k` 上下导航、`g/G` 跳顶/底、`/` 搜索、`:` 命令、`?` 帮助、`Tab/Shift+Tab` 切换面板、`数字` 直接跳转面板
- **流式事件输出**(参考 Claude Code):EventStream 面板支持事件流式追加 + 自动滚动到底部 + 用户滚动时暂停自动滚动

### C. 性能优化

- **增量渲染**:基于 `TuiState` 的 dirty 标记,只重绘变化的面板区域,减少每帧 ratatui Buffer 操作
- **虚拟滚动**:Log / EventStream / Parliament 面板支持万级事件列表,仅渲染可见区域 + 上下缓冲行
- **可调 tick 暴露**:`TuiConfig` 新增 `tick_interval_ms` 字段(100-1000ms 范围),通过 CLI `--tick` 参数或配置文件覆盖
- **FPS 显示**:Health 面板新增实时 FPS 指标(基于 `frame_count` 与时间戳计算)

### D. 历史回放(高级能力)

- **状态快照持久化**:每 N 秒(N 可配,默认 30s)将 `TuiState` + `DataSnapshot` 序列化为 MessagePack 存储到 `~/.aether/tui/snapshots/`
- **时间轴 scrubbing**:新增 `PanelId::Timeline` 面板,显示最近 1 小时的事件密度热力图,支持左右键 scrubbing 到任意时间点
- **回放引擎**:scrubbing 到指定时间点后,从快照恢复 `TuiState`,并以 1x/2x/4x 速度回放后续事件流

### E. 跨面板联动

- **Esc 状态保留**:退出 TUI 时序列化 `TuiState`(当前面板 + 过滤器 + 滚动位置)到 `~/.aether/tui/session.json`,下次启动恢复
- **Quest/Event 跳转底层命令**:在 Quest 面板按 `Enter` 查看 Quest 详情后,可选 `w` 跳转到 `chimera-cli wiki <quest_id>` 子命令;在 Log 面板按 `Enter` 查看事件详情后,可选 `s` 跳转到 `chimera-cli quest <related_quest_id>`

### F. MCP/CHTC 集成

- **MCP Mesh 节点状态可视化**:订阅 MCP Mesh 心跳事件,在 McpNodes 面板显示节点拓扑图(ASCII)、连接状态(绿/黄/红)、最近消息吞吐量
- **CHTC 跨平台适配器可视化**:订阅 CHTC 适配器状态事件,在 Chtc 面板显示 5 IDE 适配器的连接矩阵、最近请求类型分布、兼容性评分

### G. 主题与布局增强

- **运行时主题切换**:`t` 键在 Dark/Light/HighContrast 三主题间循环切换(无需重启 TUI)
- **高对比度主题**:新增 `Theme::HighContrast`,适配色盲用户与高亮环境
- **布局模板**:支持 `1`/`2`/`3` 三种布局模板(单面板全屏 / 双面板分屏 / 三面板分屏),通过数字键快速切换

### H. 参考 CLI 风格融合

- **Claude Code 风格**:EventStream 面板的事件流式追加 + 工具调用展开/折叠(Enter 展开详情,Backspace 折叠)
- **Qodex 风格**:Quest 面板进度条增强(分段进度条显示 sub-task 完成度)
- **Kimi Code 风格**:详情 overlay 中的 MessagePack 载荷支持 JSON 语法高亮(关键字段着色)

### I. P3 归档提交(Task 0)

- 提交 `realtime-data-driven-tui-panels` 规格的 P3 全部交付物(P3.1 验证 + P3.2 基准 + P3.3 文档 + 规格文档归档)到 master 分支

---

## Impact

### 受影响的规格

- `realtime-data-driven-tui-panels`:本规格是其后续演进,不修改其已交付内容,仅在此基础上扩展
- 无其他规格受影响(本规格新增能力均封装在 chimera-tui crate 内部)

### 受影响的代码

- `crates/chimera-tui/src/types.rs`:`PanelId` 新增 7 个变体(Decay/EventStream/Router/McpNodes/Chtc/Timeline);`TuiState` 新增 `decay_metrics`、`router_metrics`、`mcp_nodes`、`chtc_state`、`timeline_snapshots`、`fps`、`dirty_panels` 字段
- `crates/chimera-tui/src/panels/`:新增 `decay.rs`、`event_stream.rs`、`router.rs`、`mcp_nodes.rs`、`chtc.rs`、`timeline.rs` 六个面板模块
- `crates/chimera-tui/src/data.rs`:`DataSnapshot` 扩展 `decay_metrics`、`router_metrics`、`mcp_nodes`、`chtc_state` 字段;新增 `HistoryStore` 结构体管理快照持久化
- `crates/chimera-tui/src/app.rs`:`TuiApp` 新增 `history_store`、`replay_engine`、`fps_tracker` 字段;事件循环新增 `?`/`t`/`数字` 快捷键处理
- `crates/chimera-tui/src/config.rs`:`TuiConfig` 新增 `tick_interval_ms`、`snapshot_interval_s`、`max_event_history` 字段;`Theme` 新增 `HighContrast` 变体
- `crates/chimera-tui/src/render.rs`:新增增量渲染辅助函数 `render_diff`、虚拟滚动辅助函数 `virtual_scroll_window`
- `crates/chimera-tui/src/popup.rs`:`PopupKind` 新增 `EventDetail` 变体(支持 MessagePack 解码 + JSON 语法高亮)
- `crates/event-bus/src/types.rs`:新增 `DecayMetricsReported`、`RouterStatsReported`、`McpNodeHeartbeat`、`ChtcAdapterStatus` 四个事件变体(用于 TUI 订阅)
- `crates/chimera-cli/src/commands/tui.rs`:CLI 参数新增 `--tick`、`--theme`、`--layout`、`--snapshot-interval` 选项

### 受影响的依赖

- `crates/chimera-tui/Cargo.toml`:无新增外部依赖(ratatui + crossterm + chrono + event-bus + nexus-core 已足够);dev-dependencies 可能新增 `tempfile` 用于快照持久化测试
- 不引入跨层依赖(遵守 §2.2 依赖铁律,L10 仅依赖 L1)

---

## ADDED Requirements

### Requirement: Decay 衰减面板

系统 SHALL 提供 `PanelId::Decay` 面板,订阅 `DecayMetricsReported` 事件,展示能力衰减曲线(sparkline)、当前衰减系数、最近 10 条衰减事件。

#### Scenario: 衰减事件到达后面板更新

- **WHEN** `decay-engine` 发布 `DecayMetricsReported` 事件
- **THEN** Decay 面板在 250ms tick 内更新衰减系数数值
- **AND** sparkline 追加新数据点
- **AND** 最近事件列表 prepend 新事件

#### Scenario: 衰减系数超过阈值时高亮

- **WHEN** 衰减系数 > 0.7(高衰减)
- **THEN** 衰减系数数值以红色加粗显示
- **AND** sparkline 高于 0.7 的区域填充红色

### Requirement: EventStream 全量事件流面板

系统 SHALL 提供 `PanelId::EventStream` 面板,与现有 Log 面板(摘要 10 条)区分,支持万级事件流虚拟滚动、事件类型筛选、时间范围过滤、流式追加与自动滚动。

#### Scenario: 万级事件流虚拟滚动

- **GIVEN** EventStream 面板有 10000 条历史事件
- **WHEN** 用户按 `j/k` 或鼠标滚轮滚动
- **THEN** 仅渲染可见区域 + 上下各 5 行缓冲,帧时间 < 16ms
- **AND** 滚动位置在用户操作期间不丢失

#### Scenario: 流式追加与自动滚动

- **GIVEN** 用户未手动滚动,EventStream 面板在底部
- **WHEN** 新事件到达
- **THEN** 自动滚动到底部显示新事件
- **AND** 用户手动向上滚动后,暂停自动滚动,底部显示 "[新事件 N 条]" 提示

### Requirement: Router 路由统计面板

系统 SHALL 提供 `PanelId::Router` 面板,订阅 `RouterStatsReported` 事件,展示 KVBSR + SESA + FaaE 三大路由器的命中率、路由延迟 P50/P95/P99、热点 capability_id Top-10。

#### Scenario: 路由器命中率展示

- **WHEN** Router 面板渲染
- **THEN** 显示三路由器各自的命中率(百分比 + 进度条)
- **AND** 路由延迟 P50/P95/P99 三列对比
- **AND** 热点 capability_id Top-10 列表(按命中次数降序)

### Requirement: MCP 节点状态面板

系统 SHALL 提供 `PanelId::McpNodes` 面板,订阅 `McpNodeHeartbeat` 事件,展示 MCP Mesh 节点拓扑(ASCII 图)、连接状态(绿/黄/红)、最近消息吞吐量。

#### Scenario: 节点离线告警

- **WHEN** 某节点心跳超过 5s 未到达
- **THEN** 该节点在拓扑图中显示红色
- **AND** 面板顶部显示 "[ALERT] Node X offline" 横幅

### Requirement: CHTC 跨平台适配器面板

系统 SHALL 提供 `PanelId::Chtc` 面板,订阅 `ChtcAdapterStatus` 事件,展示 5 IDE 适配器(VSCode/JetBrains/Vim/Emacs/LSP)的连接矩阵、最近请求类型分布、兼容性评分。

#### Scenario: 适配器兼容性评分展示

- **WHEN** Chtc 面板渲染
- **THEN** 显示 5 适配器各自兼容性评分(0-100)
- **AND** 评分 < 60 的适配器以黄色高亮
- **AND** 最近请求类型分布以 sparkline 展示

### Requirement: Enter 事件详情 overlay

系统 SHALL 在 Parliament / Log / EventStream 面板支持 `Enter` 键,弹出事件详情 overlay,展示完整事件载荷 + MessagePack 解码 + JSON 语法高亮 + 上下游事件链。

#### Scenario: 查看事件详情

- **GIVEN** EventStream 面板选中某事件
- **WHEN** 用户按 `Enter`
- **THEN** 弹出 Detail overlay,标题为事件类型名
- **AND** 内容区按 JSON 格式展示完整载荷,关键字段着色
- **AND** 底部显示上下游事件 ID 链(可按 `Tab` 跳转)
- **AND** `Esc` 关闭 overlay 返回原面板

### Requirement: 历史回放引擎

系统 SHALL 提供历史回放能力,包括状态快照持久化、时间轴 scrubbing、回放引擎三部分。

#### Scenario: 时间轴 scrubbing

- **GIVEN** Timeline 面板显示最近 1 小时事件密度热力图
- **WHEN** 用户按 `←/→` 移动时间游标
- **THEN** 游标定位的时间点高亮
- **AND** 底部状态栏显示该时间点的快照时间戳
- **AND** 按 `Enter` 从该时间点开始 1x 速度回放

#### Scenario: 状态快照持久化

- **GIVEN** TUI 运行中,`snapshot_interval_s = 30`
- **WHEN** 每 30s 触发一次快照
- **THEN** `TuiState` + `DataSnapshot` 序列化为 MessagePack
- **AND** 存储到 `~/.aether/tui/snapshots/<timestamp>.msgpack`
- **AND** 超过 24h 的快照自动清理

### Requirement: 跨面板联动

系统 SHALL 支持跨面板联动,包括 Esc 状态保留、Quest/Event 跳转底层命令。

#### Scenario: Esc 状态保留

- **GIVEN** TUI 运行中,当前面板为 Budget,过滤器已设置
- **WHEN** 用户按 `Esc` 退出 TUI(或 `q` 退出)
- **THEN** `TuiState`(当前面板 + 过滤器 + 滚动位置)序列化到 `~/.aether/tui/session.json`
- **AND** 下次 `chimera tui` 启动时恢复到退出前状态

#### Scenario: Quest 跳转 wiki 子命令

- **GIVEN** Quest 面板选中某 Quest
- **WHEN** 用户按 `Enter` 查看详情后按 `w`
- **THEN** TUI 暂停,执行 `chimera wiki <quest_id>` 子命令
- **AND** 输出在终端显示
- **AND** 按任意键返回 TUI

### Requirement: 运行时主题切换

系统 SHALL 支持 `t` 键在 Dark/Light/HighContrast 三主题间循环切换,无需重启 TUI。

#### Scenario: 主题循环切换

- **GIVEN** 当前主题为 Dark
- **WHEN** 用户按 `t`
- **THEN** 主题切换为 Light,所有面板立即重绘
- **AND** 再按 `t` 切换为 HighContrast
- **AND** 再按 `t` 回到 Dark

### Requirement: 布局模板

系统 SHALL 支持数字键 `1`/`2`/`3` 快速切换三种布局模板。

#### Scenario: 切换到单面板全屏

- **GIVEN** 当前为默认三面板布局
- **WHEN** 用户按 `1`
- **THEN** 当前焦点面板全屏显示,侧边栏与日志面板隐藏
- **AND** 按 `2` 恢复双面板分屏,按 `3` 恢复三面板布局

### Requirement: 增量渲染

系统 SHALL 实现增量渲染,基于 `TuiState` 的 dirty 标记,只重绘变化的面板区域。

#### Scenario: 单面板更新时只重绘该面板

- **GIVEN** Budget 面板数据更新,其他面板无变化
- **WHEN** 渲染循环触发
- **THEN** 仅 Budget 面板区域调用 `Panel::render`
- **AND** 其他面板区域跳过渲染(保留上一帧 Buffer)
- **AND** 帧时间相比全量重绘降低 ≥ 30%

### Requirement: 可调 tick 暴露

系统 SHALL 通过 `TuiConfig.tick_interval_ms` 暴露 tick 间隔配置,范围 100-1000ms。

#### Scenario: CLI 参数覆盖 tick

- **GIVEN** 用户执行 `chimera tui --tick 100`
- **WHEN** TUI 启动
- **THEN** DataPipeline tick 间隔为 100ms
- **AND** 事件延迟感知降低,但 CPU 占用可能提升

### Requirement: FPS 显示

系统 SHALL 在 Health 面板显示实时 FPS(基于 `frame_count` 与时间戳计算)。

#### Scenario: FPS 实时显示

- **WHEN** TUI 运行中
- **THEN** Health 面板顶部显示 `FPS: <n>`(每秒更新一次)
- **AND** FPS < 30 时以黄色高亮,FPS < 15 时以红色高亮

---

## MODIFIED Requirements

### Requirement: PanelId 枚举扩展

`PanelId` 枚举从 8 个变体扩展为 15 个变体(新增 Decay / EventStream / Router / McpNodes / Chtc / Timeline)。

**修改前**:Quest / Parliament / Budget / Memory / Security / Health / Log / Help
**修改后**:Quest / Parliament / Budget / Memory / Security / Health / Log / Help / Decay / EventStream / Router / McpNodes / Chtc / Timeline

**迁移**:`PanelId::next()` / `prev()` 循环顺序扩展为 15 面板循环;`as_str()` / `title()` 新增 6 个匹配分支。

### Requirement: TuiConfig 字段扩展

`TuiConfig` 新增 `tick_interval_ms`、`snapshot_interval_s`、`max_event_history`、`max_snapshots` 字段,默认值分别为 250 / 30 / 10000 / 2880(24h × 120/h)。

### Requirement: TuiState 字段扩展

`TuiState` 新增 `decay_metrics`、`router_metrics`、`mcp_nodes`、`chtc_state`、`timeline_snapshots`、`fps`、`dirty_panels`、`auto_scroll` 字段,支持新面板与增量渲染。

### Requirement: Theme 枚举扩展

`Theme` 从 2 个变体扩展为 3 个变体(新增 `HighContrast`)。

---

## REMOVED Requirements

无。本规格为纯增量演进,不删除任何已有能力。

---

## 范围边界与分阶段交付策略

考虑到 v1.7-omega 定位为中版本(2-4 周迭代),7 个工作方向 + 3 个高级能力总计 10 个方向无法在单次迭代全部深度交付。采用分阶段交付策略:

### v1.7-omega 必交付(P0-P5)

- **P0**: 现状评估 + 参考 CLI 调研 + 架构基线设计
- **P1**: 提交 P3 归档(Task 0)
- **P2**: 新增 5 监控面板(Decay / EventStream / Router / McpNodes / Chtc)
- **P3**: 交互能力升级(Enter 详情 overlay + ? Help overlay + 全局快捷键 + 流式追加)
- **P4**: 性能优化(增量渲染 + 虚拟滚动 + 可调 tick + FPS 显示)
- **P5**: 跨面板联动(Esc 状态保留 + Quest 跳转 wiki)

### v1.7-omega 可选/部分交付(P6)

- **P6**: 主题运行时切换 + 布局模板 + HighContrast 主题

### v1.8+ 后续演进(P7-P8)

- **P7**: 历史回放引擎(状态快照 + Timeline scrubbing + 回放引擎)— 复杂度高,单独立项
- **P8**: MCP/CHTC 集成深化(节点拓扑图 + 兼容性矩阵评分)— 依赖 MCP Mesh 稳定

> **注**:P7/P8 在 v1.7 spec 中设计接口与事件变体,但实现可推迟到 v1.8+。P2 中新增的 McpNodes / Chtc 面板在 v1.7 阶段以"订阅事件 + 基础渲染"形式交付,深度可视化(拓扑图 / 矩阵评分)在 v1.8+ 演进。
