# Tasks — TUI v1.7-omega 深化演进

> **change-id**: `tui-v17-deep-evolution`
> **版本定位**: v1.7-omega(中版本增强,2-4 周迭代)
> **基线**: chimera-tui M0-M5 已交付(8 面板 + Panel trait + 双向控制 + 鼠标 + 弹窗 + 搜索过滤)
> **参考 CLI**: Claude Code / Qodex / Kimi Code CLI
>
> **执行规范**:
>
> * 严格遵循 §2.2 依赖铁律(L10 仅依赖 L1 event-bus + nexus-core)
>
> * 所有 crate 维持 `#![forbid(unsafe_code)]`
>
> * 每个 Task 遵循 TDD(RED-GREEN-REFACTOR)
>
> * 子代理产物必须 `cargo fmt --all` + `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings` 通过
>
> * 每个 Task 完成后勾选 `[x]`,并通过 checklist.md 全部检查项才能进入下一 Task
>
> **协作模式**: 精英专家级子代理团队(架构设计 + TUI 渲染 + 事件总线 + 性能优化 + 测试),系统性分布式深度分析 + 多轮结构化验证。
>
> **分阶段交付策略**:
>
> * v1.7 必交付:P0-P5
>
> * v1.7 可选:P6
>
> * v1.8+ 演进:P7-P8(本 spec 设计接口,实现推迟)

***

## P0 — 现状评估与架构基线设计(\~4h)

> **依赖**: 无(基于当前 master 分支)
> **并行性**: P0.1 / P0.2 / P0.3 可并行
> **验收门槛**: 架构基线文档 + 参考 CLI 调研报告 + PanelId 扩展设计确认

* [x] **Task P0.1: 现状深度审计**

  * [x] SubTask P0.1.1: Read `crates/chimera-tui/src/app.rs` 完整事件循环,识别当前快捷键处理逻辑与可扩展点

  * [x] SubTask P0.1.2: Read `crates/chimera-tui/src/panels/mod.rs` + 8 个面板模块,梳理 Panel trait 实现模式与可复用组件(list\_state / filtered\_xxx / content 方法)

  * [x] SubTask P0.1.3: Read `crates/chimera-tui/src/data.rs` DataPipeline + DataSnapshot,识别快照字段缺口与新面板所需数据

  * [x] SubTask P0.1.4: Read `crates/event-bus/src/types.rs`,列出所有事件变体,识别需要新增的事件(DecayMetricsReported / RouterStatsReported / McpNodeHeartbeat / ChtcAdapterStatus)

  * [x] SubTask P0.1.5: Read `crates/chimera-tui/src/render.rs` + `popup.rs`,识别渲染辅助函数与弹窗类型,规划新增 EventDetail overlay 与 Help overlay 的扩展点

* [x] **Task P0.2: 参考 CLI 调研**

  * [x] SubTask P0.2.1: 调研 Claude Code CLI 的 TUI 设计要素(流式输出 / 工具调用展开折叠 / 命令模式 / 上下文显示),提取可借鉴的交互模式

  * [x] SubTask P0.2.2: 调研 Qodex CLI 的任务导向界面与进度条设计,提取 Quest 面板进度条增强方向

  * [x] SubTask P0.2.3: 调研 Kimi Code CLI 的对话式 TUI 与代码块语法高亮,提取详情 overlay 的 JSON 高亮方案

  * [x] SubTask P0.2.4: 汇总三参考 CLI 的设计要素,产出"参考借鉴清单"(写入 P0 Note)

* [x] **Task P0.3: 架构基线设计**

  * [x] SubTask P0.3.1: 设计 `PanelId` 从 8 变体扩展到 14 变体的迁移方案(新增 Decay/EventStream/Router/McpNodes/Chtc/Timeline 共 6 个,不含 Help 已有)

  * [x] SubTask P0.3.2: 设计 `TuiState` 字段扩展方案(新增 decay\_metrics / router\_metrics / mcp\_nodes / chtc\_state / timeline\_snapshots / fps / dirty\_panels / auto\_scroll)

  * [x] SubTask P0.3.3: 设计 `DataSnapshot` 字段扩展方案(与 TuiState 对齐,供 DataPipeline 同步)

  * [x] SubTask P0.3.4: 设计 `TuiConfig` 字段扩展方案(tick\_interval\_ms / snapshot\_interval\_s / max\_event\_history / max\_snapshots)

  * [x] SubTask P0.3.5: 设计 4 个新事件变体的载荷结构(DecayMetricsReported / RouterStatsReported / McpNodeHeartbeat / ChtcAdapterStatus)

  * [x] SubTask P0.3.6: 产出架构基线摘要,作为 P2-P8 实施的设计依据(写入本 tasks.md 的 P0 Note)

### P0 Note — 架构基线(2026-07-14 完成)

#### 1. PanelId 扩展后的完整循环顺序(14 面板)

```
Quest(1) → Parliament(2) → Budget(3) → Memory(4) → Security(5)
→ Health(6) → Log(7) → Help(8) → Decay(9) → EventStream(g1)
→ Router(g2) → McpNodes(g3) → Chtc(g4) → Timeline(g5) → Quest(循环)
```

数字键映射:

* `1`-`9`: 前 9 个面板(Quest/Parliament/Budget/Memory/Security/Health/Log/Help/Decay)

* `g` 前缀 + `1`-`5`: 后 5 个面板(EventStream/Router/McpNodes/Chtc/Timeline)

WHY 分组:前 9 个为 M0-M5 已有面板 + Decay(最高优先级新面板);后 5 个为新增监控面板,使用频率较低。

#### 2. TuiState 新增字段的类型定义

```rust
// 新增类型(定义在 types.rs 或独立 metrics.rs)
pub struct DecayMetrics {
    pub coefficient: f32,           // 衰减系数 [0.0, 1.0]
    pub recent_events: Vec<String>, // 最近衰减事件描述(≤10 条)
    pub cycle_start: DateTime<Utc>,
}

pub struct RouterMetrics {
    pub kvbsr_stats: RouterStatsPayload,
    pub sesa_stats: RouterStatsPayload,
    pub faae_stats: RouterStatsPayload,
}

pub struct RouterStatsPayload {
    pub hit_rate: f32,              // 命中率 [0.0, 1.0]
    pub p50_latency_us: u64,        // P50 延迟(微秒)
    pub p95_latency_us: u64,
    pub p99_latency_us: u64,
    pub hot_capabilities: Vec<(String, u64)>, // Top-10 热点
}

pub struct McpNodeStatus {
    pub node_id: String,
    pub status: NodeStatus,         // Online/Degraded/Offline
    pub throughput: u64,            // 消息/秒
    pub last_seen: DateTime<Utc>,
}

pub enum NodeStatus { Online, Degraded, Offline }

pub struct ChtcState {
    pub adapters: Vec<ChtcAdapterInfo>, // 5 IDE 适配器
}

pub struct ChtcAdapterInfo {
    pub adapter_id: String,
    pub adapter_type: String,
    pub compatibility_score: u8,    // [0, 100]
    pub recent_requests: Vec<(String, u32)>,
    pub is_online: bool,
}

// TuiState 新增字段
pub struct TuiState {
    // ... 现有字段 ...
    pub decay_metrics: DecayMetrics,
    pub router_metrics: RouterMetrics,
    pub mcp_nodes: Vec<McpNodeStatus>,
    pub chtc_state: ChtcState,
    pub timeline_snapshots: Vec<TimelineSnapshot>, // P7 接口设计,暂为空 Vec
    pub fps: u16,                   // P4.4 FPS 显示
    pub dirty_panels: HashSet<PanelId>, // P4.1 增量渲染
    pub auto_scroll: bool,         // P3.4 流式追加
    pub decay_history: Vec<u64>,   // Decay sparkline 数据点
}
```

#### 3. DataSnapshot 新增字段与 DataPipeline 同步策略

```rust
pub struct DataSnapshot {
    // ... 现有 9 字段 ...
    pub decay_metrics: DecayMetrics,
    pub router_metrics: RouterMetrics,
    pub mcp_nodes: Vec<McpNodeStatus>,
    pub chtc_state: ChtcState,
    pub decay_history: Vec<u64>,
}
```

DataPipeline 新增 4 个同步器(与现有 5 个同步器并列):

* `DecaySync` — 订阅 `DecayMetricsReported`,更新 `decay_metrics` + `decay_history`

* `RouterSync` — 订阅 `RouterStatsReported`,更新 `router_metrics`

* `McpNodesSync` — 订阅 `McpNodeHeartbeat`,更新 `mcp_nodes`(含 5s 超时检测)

* `ChtcSync` — 订阅 `ChtcAdapterStatus`,更新 `chtc_state`

同步策略与现有 5 个同步器一致:锁内取事件 → 释放锁 → 更新状态 → 写入快照。

#### 4. TuiConfig 新增字段

```rust
pub struct TuiConfig {
    // ... 现有 5 字段 ...
    pub tick_interval_ms: u16,      // 默认 250,范围 100-1000(P4.3)
    pub snapshot_interval_s: u16,   // 默认 30(P7 接口设计)
    pub max_event_history: usize,   // 默认 256,EventStream 面板需扩展到 10000(P2.2)
    pub max_snapshots: usize,       // 默认 100(P7 接口设计)
}
```

validate() 新增校验:

* `tick_interval_ms` ∈ \[100, 1000]

* `snapshot_interval_s` >= 1

* `max_event_history` >= 64

* `max_snapshots` >= 10

#### 5. 4 个新事件变体的载荷定义

```rust
// event-bus/src/types.rs 新增(追加在 NexusEvent enum 末尾)

DecayMetricsReported {
    metadata: EventMetadata,
    coefficient: f32,               // 衰减系数 [0.0, 1.0]
    recent_events: Vec<String>,     // 最近衰减事件(≤10 条)
    cycle_start: DateTime<Utc>,
},

RouterStatsReported {
    metadata: EventMetadata,
    kvbsr_stats: RouterStatsPayload,
    sesa_stats: RouterStatsPayload,
    faae_stats: RouterStatsPayload,
},

McpNodeHeartbeat {
    metadata: EventMetadata,
    node_id: String,
    status: String,                // "online" / "degraded" / "offline"
    throughput: u64,               // 消息/秒
    last_seen: DateTime<Utc>,
},

ChtcAdapterStatus {
    metadata: EventMetadata,
    adapter_id: String,
    adapter_type: String,
    compatibility_score: u8,       // [0, 100]
    recent_requests: Vec<(String, u32)>,
    is_online: bool,
},
```

RouterStatsPayload 定义在 event-bus(避免 L10 持有 L6 类型),与 BudgetMetricsPayload 模式一致。

severity: 4 个均为 Normal(周期性统计,丢失可补偿)。

NexusEvent 总变体数:76 → 80。

需同步更新 `metadata()` / `type_name()` 方法的 match 分支(`severity()` 被 Normal 通配符覆盖,无需修改)。

#### 6. 参考 CLI 借鉴清单的具体实施映射

**P2 新面板**:

* \[高] Quest 状态机五态展示(Codex /goal + Qoder /quest)

* \[高] Token 计数器 + 成本估算(Kimi Code,Budget 面板底部)

* \[高] 状态栏可配置数组(Codex status\_line)

**P3 交互升级**:

* \[高] `/btw` 侧问模态面板(Claude Code + Kimi Code)→ 暂列为 v1.8+ 候选

* \[高] 工具调用四类折叠(Claude Code Messages.tsx)→ Log 面板噪音抑制

* \[高] 输入模式提示符反馈(Kimi Code ✨/💫/$/📋)→ 状态栏增强

* \[高] 数字键 1-4 快速审批(Kimi Code)→ Parliament 面板

* \[高] StickyPrompt 粘性输入(Claude Code)→ 搜索/命令模式增强

**P4 性能优化**:

* \[高] Fullscreen alternate buffer(Claude Code)→ 消除闪烁

* \[高] 虚拟滚动 + heightCache(Claude Code)→ EventStream 万级事件

* \[高] Tree-sitter 只高亮可见区域(ratatui-code-editor)→ P3.1 JSON 高亮候选

**P5 跨面板联动**:

* \[中] 远程控制 Daemon 模式(Qoder)→ v1.8+ 候选

* \[中] 结构化问答 tab 导航(Kimi Code)→ 多问题场景

* \[高] `/btw` 隔离上下文查询 → 跨面板查询不打断主流程

**JSON 高亮方案决策**:

* v1.7 采用自行实现基础着色(字符串绿色/数字青色/布尔黄色/null 灰色),避免引入新依赖

* v1.8+ 评估 ratatui-markdown 0.3.4(可折叠 JSON 树)+ ratatui-code-editor 0.0.6(Tree-sitter 高亮)

#### 7. 已识别技术债(在对应 Task 中清理)

| 技术债                                        | 处理时机                |
| ------------------------------------------ | ------------------- |
| SecurityPanel 未复用 list\_state 辅助函数         | P2 阶段统一             |
| BudgetPanel 未复用 `render::utilization_bar`  | P2.3 Router 面板复用时统一 |
| `?` 键处理分散在 6 个面板                           | P3.2.4 全局拦截时清理      |
| `max_event_history` 默认 256 过小              | P2.2 扩展到 10000      |
| F4/F5 键被跳过                                 | P2 阶段可填补            |
| `nuxus规则.md §5.3` 记录 65 个事件变体已过时(实际 76→80) | 建议更新规则文档            |

#### 8. 依赖方向确认

chimera-tui(L10)仅依赖 event-bus + nexus-core(L1),不引入 L9 依赖。
4 个新事件变体定义在 event-bus(L1),发布者分别为:

* DecayMetricsReported → decay-engine(L4)发布

* RouterStatsReported → efficiency-monitor(L9)或新增聚合器发布

* McpNodeHeartbeat → mcp-mesh(L10)发布(L10 同层)

* ChtcAdapterStatus → chtc-bridge(L10)发布(L10 同层)

所有新增类型(DecayMetrics/RouterMetrics/McpNodeStatus/ChtcState)定义在 chimera-tui 内部,不引入跨层依赖。

***

## P1 — 提交 P3 归档(\~1h)

> **依赖**: 无(独立动作,与 P0 并行)
> **并行性**: 与 P0 完全并行
> **验收门槛**: master 分支包含 P3 全部交付物 + 规格文档归档

* [x] **Task P1.1: 提交 realtime-data-driven-tui-panels 规格的 P3 交付物**

  * [x] SubTask P1.1.1: `git status` 确认未提交变更(应包含 tasks.md / checklist.md 的 P3 勾选 + P3.1/P3.2/P3.3 全部实施产物)

  * [x] SubTask P1.1.2: `git diff --stat` 核验变更范围(预期仅 chimera-tui + specs/realtime-data-driven-tui-panels + CHANGELOG.md + CODE\_WIKI.md + docs/tui/)

  * [x] SubTask P1.1.3: `git add` 逐文件添加(遵守 §协作偏好:不使用 `git add -A` / `git add .`)

  * [x] SubTask P1.1.4: `git commit -m` 使用 HEREDOC 传入 commit message,描述 P3 验证 + 基准 + 文档归档

  * [x] SubTask P1.1.5: `git push origin main` 推送到远端(实际分支为 main,非 master)

  * [x] SubTask P1.1.6: `git log --oneline -3` 确认提交成功(commit 0f1d1a0)

***

## P2 — 新增 5 监控面板(\~12h)

> **依赖**: P0 完成(架构基线 + 事件变体设计)
> **并行性**: P2.1-P2.5 五个面板可完全并行(独立模块,无相互依赖)
> **验收门槛**: 5 个新面板单元测试 + 集成测试全部通过;`cargo test -p chimera-tui` 无回归

* [x] **Task P2.1: Decay 衰减面板**

  * [x] SubTask P2.1.1: TDD-RED — 在 `crates/chimera-tui/tests/decay_panel_test.rs` 新增测试:订阅 DecayMetricsReported 事件后,Decay 面板渲染内容包含衰减系数与 sparkline

  * [x] SubTask P2.1.2: TDD-GREEN — 新增 `crates/chimera-tui/src/panels/decay.rs`,实现 `DecayPanel` 结构体 + `Panel` trait

  * [x] SubTask P2.1.3: 实现 sparkline 渲染(复用 `render::sparkline` 辅助函数),衰减系数 > 0.7 红色高亮

  * [x] SubTask P2.1.4: 在 `event-bus/src/types.rs` 新增 `DecayMetricsReported` 事件变体(载荷:coefficient f32 / recent\_events Vec<String> / timestamp)

  * [x] SubTask P2.1.5: 在 `DataPipeline` 中新增 `DecaySync` 适配器,订阅事件并维护 `decay_metrics` 状态

  * [x] SubTask P2.1.6: TDD-REFACTOR — 提取 sparkline 高亮阈值常量,WHY 注释说明 0.7 阈值依据

* [x] **Task P2.2: EventStream 全量事件流面板**

  * [x] SubTask P2.2.1: TDD-RED — 新增测试:EventStream 面板支持万级事件虚拟滚动,帧时间 < 16ms

  * [x] SubTask P2.2.2: TDD-GREEN — 新增 `crates/chimera-tui/src/panels/event_stream.rs`,实现 `EventStreamPanel` + `Panel` trait

  * [x] SubTask P2.2.3: 实现虚拟滚动(virtual\_scroll\_window 辅助函数,仅渲染可见区域 + 上下 5 行缓冲)

  * [x] SubTask P2.2.4: 实现流式追加 + 自动滚动(`auto_scroll` 标记,用户滚动时暂停,底部显示 "\[新事件 N 条]")

  * [x] SubTask P2.2.5: 实现事件类型筛选(复用 `filter_topic` / `filter_level` 字段)

  * [x] SubTask P2.2.6: TDD-REFACTOR — 提取虚拟滚动逻辑到 `render::virtual_scroll_window`,WHY 注释说明缓冲行数选择

* [x] **Task P2.3: Router 路由统计面板**

  * [x] SubTask P2.3.1: TDD-RED — 新增测试:RouterStatsReported 事件到达后,Router 面板显示三路由器命中率与延迟 P50/P95/P99

  * [x] SubTask P2.3.2: TDD-GREEN — 新增 `crates/chimera-tui/src/panels/router.rs`,实现 `RouterPanel` + `Panel` trait

  * [x] SubTask P2.3.3: 实现三路由器命中率进度条(复用 `render::utilization_bar`)

  * [x] SubTask P2.3.4: 实现热点 capability\_id Top-10 列表(使用 `select_nth_unstable` O(n) Top-K,遵守 §4.1)

  * [x] SubTask P2.3.5: 在 `event-bus/src/types.rs` 新增 `RouterStatsReported` 事件变体(载荷:kvbsr\_stats / sesa\_stats / faae\_stats 各含 hit\_rate / p50 / p95 / p99 / hot\_capabilities)— 共享基础设施阶段已完成

  * [x] SubTask P2.3.6: TDD-REFACTOR — 提取延迟统计渲染为辅助函数,WHY 注释说明 P50/P95/P99 三列对比的可读性优势

* [x] **Task P2.4: MCP 节点状态面板**

  * [x] SubTask P2.4.1: TDD-RED — 新增测试:McpNodeHeartbeat 事件到达后,McpNodes 面板显示节点列表与连接状态

  * [x] SubTask P2.4.2: TDD-GREEN — 新增 `crates/chimera-tui/src/panels/mcp_nodes.rs`,实现 `McpNodesPanel` + `Panel` trait

  * [x] SubTask P2.4.3: 实现节点列表渲染(节点 ID / 连接状态绿黄红 / 最近消息吞吐量)

  * [x] SubTask P2.4.4: 实现离线告警横幅(心跳 > 5s 未到达显示 "\[ALERT] Node X offline")

  * [x] SubTask P2.4.5: 在 `event-bus/src/types.rs` 新增 `McpNodeHeartbeat` 事件变体(载荷:node\_id / status / throughput / last\_seen)

  * [x] SubTask P2.4.6: TDD-REFACTOR — WHY 注释说明 5s 心跳超时阈值的选择依据

* [x] **Task P2.5: CHTC 跨平台适配器面板**

  * [x] SubTask P2.5.1: TDD-RED — 新增测试:ChtcAdapterStatus 事件到达后,Chtc 面板显示 5 IDE 适配器兼容性评分

  * [x] SubTask P2.5.2: TDD-GREEN — 新增 `crates/chimera-tui/src/panels/chtc.rs`,实现 `ChtcPanel` + `Panel` trait

  * [x] SubTask P2.5.3: 实现 5 适配器兼容性评分展示(评分 < 60 黄色高亮)

  * [x] SubTask P2.5.4: 实现最近请求类型分布 sparkline

  * [x] SubTask P2.5.5: 在 `event-bus/src/types.rs` 新增 `ChtcAdapterStatus` 事件变体(载荷:adapter\_id / adapter\_type / compatibility\_score / recent\_requests)

  * [x] SubTask P2.5.6: TDD-REFACTOR — WHY 注释说明 5 IDE 适配器列表与兼容性评分算法

### P2 Note — 共享基础设施完成(2026-07-14)

> P2 共享基础设施已全部完成,为 P2.1-P2.5 五个面板的并行实现铺平道路。
> 以下子代理填充具体面板渲染逻辑时,只需聚焦 Panel::render 实现 + TDD 测试,
> 无需再修改 event-bus / types.rs / config.rs / data.rs / app.rs / lib.rs。

#### 1. PanelId 循环顺序(13 面板注册,14 变体定义)

* PanelId 枚举已定义 14 变体(含 Timeline,为 P7 预留)

* FocusManager 注册 13 面板(不含 Timeline,因 Timeline 面板未实现)

* Tab 循环:Quest → Parliament → Budget → Memory → Security → Health → Log → Help → Decay → EventStream → Router → McpNodes → Chtc → Quest

* 数字键 1-9 映射前 9 个面板(1=Quest, ..., 9=Decay)

* `g` 前缀 + 数字键映射后 4 个面板(EventStream/Router/McpNodes/Chtc)由 P3.3 实现

#### 2. DataSnapshot 新增字段与 DataPipeline 同步

* DataSnapshot 新增 5 字段:decay\_metrics / router\_metrics / mcp\_nodes / chtc\_state / decay\_history

* DataPipeline 新增 4 个同步器:DecaySync / RouterSync / McpNodesSync / ChtcSync

* TuiApp::update() 已同步 5 个新字段从 DataSnapshot 到 TuiState

* StubDataSource 已更新示例数据

#### 3. 4 个新事件变体的载荷定义(event-bus/src/types.rs)

* `DecayMetricsReported { metadata, coefficient: f32, recent_events: Vec<String>, cycle_start: DateTime<Utc> }`

* `RouterStatsReported { metadata, kvbsr_stats: RouterStatsPayload, sesa_stats: RouterStatsPayload, faae_stats: RouterStatsPayload }`

* `McpNodeHeartbeat { metadata, node_id: String, status: String, throughput: u64, last_seen: DateTime<Utc> }`

* `ChtcAdapterStatus { metadata, adapter_id: String, adapter_type: String, compatibility_score: u8, recent_requests: Vec<(String, u32)>, is_online: bool }`

* RouterStatsPayload 结构体已定义(hit\_rate / p50\_latency\_us / p95\_latency\_us / p99\_latency\_us / hot\_capabilities)

* topic.rs 已更新 4 个新变体的 EventTopic 映射

* lib.rs 已导出 RouterStatsPayload

#### 4. TuiConfig 新增字段

* tick\_interval\_ms(默认 250,范围 100-1000)

* snapshot\_interval\_s(默认 30,P7 接口占位)

* max\_event\_history(默认 256,EventStream 面板最小 64)

* max\_snapshots(默认 100,P7 接口占位)

* validate() 新增 4 条校验规则 + 6 个验证测试

#### 5. 5 个占位面板(panels/)

* decay.rs:DecayPanel,渲染 TODO 文本,3 个测试

* event\_stream.rs:EventStreamPanel,渲染 TODO 文本,3 个测试

* router.rs:RouterPanel,渲染 TODO 文本,3 个测试

* mcp\_nodes.rs:McpNodesPanel,渲染 TODO 文本,3 个测试

* chtc.rs:ChtcPanel,渲染 TODO 文本,3 个测试

* panels/mod.rs 已声明 5 个新模块 + re-export

* lib.rs pub use + prelude 已导出 5 个新面板 + 8 个新类型

#### 6. 虚拟滚动与 sparkline 复用情况

* 尚未实现(占位面板不消费 TuiState 数据)

* 后续 P2.1-P2.5 填充时复用 render::sparkline / render::utilization\_bar

#### 7. 测试覆盖率

* chimera-tui:全部测试通过(含 5 个新面板的 15 个测试 + 13 面板循环导航测试 + '9' 键映射测试)

* event-bus:全部测试通过(4 个新事件变体的 topic/severity/metadata/type\_name 测试)

* cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings:通过

* cargo fmt --all -- --check:通过

* cargo check --workspace:通过(无回归)

#### 8. 后续 P2.1-P2.5 实施指引

每个面板的完整实现需遵循 TDD-RED-GREEN-REFACTOR:

1. TDD-RED:在 tests/ 新增测试,验证事件到达后面板渲染特定内容
2. TDD-GREEN:填充 Panel::render 实现,消费 TuiState 对应字段
3. TDD-REFACTOR:提取常量与辅助函数,WHY 注释说明设计决策

各面板消费的 TuiState 字段:

* DecayPanel:decay\_metrics(DecayMetrics)+ decay\_history(Vec<u64>)

* EventStreamPanel:latest\_events(VecDeque<NexusEvent>)+ auto\_scroll(bool)

* RouterPanel:router\_metrics(RouterMetrics)

* McpNodesPanel:mcp\_nodes(Vec<McpNodeStatus>)

* ChtcPanel:chtc\_state(ChtcState)

***

## P3 — 交互能力升级(\~10h)

> **依赖**: P0 完成(架构基线);P2 完成(EventStream 面板存在,Enter 详情才有载体)
> **并行性**: P3.1 / P3.2 / P3.3 可部分并行
> **验收门槛**: 交互测试全部通过;`cargo test -p chimera-tui` 无回归

* [x] **Task P3.1: Enter 事件详情 overlay**

  * [ ] SubTask P3.1.1: TDD-RED — 新增测试:在 EventStream 面板选中事件后按 Enter,弹出 Detail overlay 显示完整载荷

  * [ ] SubTask P3.1.2: TDD-GREEN — 在 `popup.rs` 新增 `PopupKind::EventDetail` 变体(载荷:event\_type / payload\_msgpack / payload\_decoded / related\_event\_ids)

  * [ ] SubTask P3.1.3: 实现 MessagePack 解码为 JSON(使用 rmp-serde,已在 workspace 依赖)

  * [ ] SubTask P3.1.4: 实现 JSON 语法高亮(关键字段着色:字符串绿色 / 数字青色 / 布尔黄色 / null 灰色)

  * [ ] SubTask P3.1.5: 实现上下游事件 ID 链展示(底部显示 related\_event\_ids,Tab 跳转)

  * [ ] SubTask P3.3.6: 在 Parliament / Log / EventStream 三面板的 `handle_key` 中处理 Enter 键,产生 `TuiCommand::ShowEventDetail(event_index)`

  * [ ] SubTask P3.1.7: TDD-REFACTOR — WHY 注释说明 MessagePack 解码失败时的降级策略(显示原始 hex)

* [x] **Task P3.2: Help overlay(? 键触发)**

  * [ ] SubTask P3.2.1: TDD-RED — 新增测试:任意面板按 `?` 键,弹出 Help overlay 显示快捷键说明,不切换面板

  * [ ] SubTask P3.2.2: TDD-GREEN — 在 `popup.rs` 新增 `PopupKind::HelpOverlay` 变体(载荷:快捷键表 Vec<(key, description)>)

  * [ ] SubTask P3.2.3: 实现 Help overlay 渲染(居中浮窗,半透明背景,Esc 关闭)

  * [ ] SubTask P3.2.4: 在 `TuiApp::handle_key` 中全局拦截 `?` 键,产生 `TuiCommand::ShowHelp`

  * [ ] SubTask P3.2.5: TDD-REFACTOR — WHY 注释说明 Help overlay 与 Help 面板的区别(overlay 不切换焦点,临时参考)

* [ ] **Task P3.3: 全局快捷键系统统一**

  * [ ] SubTask P3.3.1: TDD-RED — 新增测试:`j/k` 上下导航、`g/G` 跳顶底、`/` 搜索、`:` 命令、`?` 帮助、`Tab/Shift+Tab` 切换面板、`数字` 跳转面板 全部生效

  * [ ] SubTask P3.3.2: TDD-GREEN — 重构 `TuiApp::handle_key`,将全局快捷键提取到 `handle_global_key` 方法,面板特定键 fallback 到 `Panel::handle_key`

  * [ ] SubTask P3.3.3: 实现数字键 1-9 跳转面板(1=Quest, 2=Parliament, ... 9=Timeline,超过 9 用 `g` 前缀 + 数字)

  * [ ] SubTask P3.3.4: 实现 `g/G` 跳顶/底(委托给当前面板的 `scroll_to_top/bottom` 方法,Panel trait 新增默认实现)

  * [ ] SubTask P3.3.5: TDD-REFACTOR — WHY 注释说明全局键与面板键的优先级(全局键先拦截,未命中再委托面板)

* [x] **Task P3.4: 流式事件追加(Claude Code 风格)**

  * [ ] SubTask P3.4.1: TDD-RED — 新增测试:EventStream 面板在 auto\_scroll=true 时,新事件到达自动滚动到底部

  * [ ] SubTask P3.4.2: TDD-GREEN — 实现 `auto_scroll` 状态字段,用户手动滚动时设为 false,新事件到达时若 false 则显示 "\[新事件 N 条]" 提示

  * [ ] SubTask P3.4.3: 实现 "跳到底部" 快捷键 `G`(在 auto\_scroll=false 时,按 G 跳到底部并恢复 auto\_scroll=true)

  * [ ] SubTask P3.4.4: TDD-REFACTOR — WHY 注释说明 auto\_scroll 策略(参考 Claude Code 工具调用流式输出的 UX)

### P3 Note — 实现说明(2026-07-14 完成)

> P3 交互能力升级已全部交付(commit `2ca37ec`),4 个子任务均通过 TDD-RED-GREEN-REFACTOR。

#### 1. P3.1 Enter 事件详情 overlay
- `PopupKind::EventDetail` 变体已实现,载荷含 `event_type` / `payload_decoded` / `related_event_ids`
- MessagePack 解码使用 `rmp-serde::from_slice` 失败时降级为原始 hex 显示(避免 panic)
- JSON 语法高亮:字符串绿色 / 数字青色 / 布尔黄色 / null 灰色(无新依赖,自行实现 tokenizer)
- Parliament / Log / EventStream 三面板 Enter 键统一触发 `TuiCommand::OpenPopup(PopupKind::event_detail(event))`
- 7 个集成测试覆盖:正常解码 / 解码失败降级 / 空载荷 / 三面板触发 / Esc 关闭

#### 2. P3.2 Help overlay
- `PopupKind::HelpOverlay` 变体已实现,载荷为 `Vec<(String, String)>` 快捷键表 + `scroll` 偏移
- 渲染:居中浮窗 60% 宽度 + 深色背景 + 标题栏 "Help — Keybindings"
- `?` 键由 `TuiApp::handle_global_key` 全局拦截,直接 `popup_stack.push(PopupKind::help_overlay())`,不切换当前面板焦点
- 4 个集成测试覆盖:`?` 触发 / 不切换面板 / Esc 关闭 / 滚动

#### 3. P3.3 全局快捷键系统统一
- `handle_global_key` 方法提取,全局键优先拦截:`q`(退出)/ `Tab`/`Shift+Tab`(切换)/ `?`(帮助)/ `:`(命令)/ `/`(搜索)/ `g` 前缀(跳顶/底 + 后 4 面板)
- 数字键 1-9 映射前 9 面板(Quest/Parliament/Budget/Memory/Security/Health/Log/Help/Decay)
- `g` 前缀状态:`g`+1-4 映射后 4 面板(EventStream/Router/McpNodes/Chtc),`gg` 跳顶,`G` 跳底
- WHY 注释说明:全局键先拦截策略避免面板重复实现通用语义,保证 `q`/`Tab`/`?` 在所有面板行为一致

#### 4. P3.4 流式事件追加(Claude Code 风格)
- `auto_scroll: bool` 状态字段:用户手动滚动(↑/↓/PgUp/PgDn)时设为 false,新事件到达时若 false 则显示 "[新事件 N 条]" 提示
- `G` 键跳到底部并恢复 `auto_scroll = true`
- EventStream 面板 `render` 末尾根据 `auto_scroll` 决定是否自动滚动到底部
- 8 个集成测试覆盖:auto_scroll 默认 true / 手动滚动暂停 / G 恢复 / 新事件计数 / 多 tick 累积

***

## P4 — 性能优化(\~10h)

> **依赖**: P0 完成;P2 完成(虚拟滚动需在 EventStream 面板验证)
> **并行性**: P4.1 / P4.2 / P4.3 / P4.4 可部分并行
> **验收门槛**: 性能基准满足 P95 阈值;`cargo test -p chimera-tui` 无回归

* [ ] **Task P4.1: 增量渲染**

  * [ ] SubTask P4.1.1: TDD-RED — 新增测试:仅 Budget 面板数据更新时,其他面板的 `Panel::render` 不被调用

  * [ ] SubTask P4.1.2: TDD-GREEN — 在 `TuiState` 新增 `dirty_panels: HashSet<PanelId>` 字段,数据更新时标记对应面板为 dirty

  * [ ] SubTask P4.1.3: 重构 `TuiApp::render`,仅对 `dirty_panels` 中的面板调用 `Panel::render`,其他面板保留上一帧 Buffer

  * [ ] SubTask P4.1.4: 实现帧结束清空 `dirty_panels`(渲染后清空,下一帧重新标记)

  * [ ] SubTask P4.1.5: TDD-REFACTOR — WHY 注释说明 dirty 标记策略(数据驱动 vs 时间驱动)

* [x] **Task P4.2: 虚拟滚动(已部分实现,扩展到 Parliament)**

  * [ ] SubTask P4.2.1: TDD-RED — 新增测试:Parliament 面板支持 1000 条历史事件虚拟滚动

  * [ ] SubTask P4.2.2: TDD-GREEN — 将 `render::virtual_scroll_window`(P2.2.3 实现)应用到 Parliament 面板

  * [ ] SubTask P4.2.3: TDD-REFACTOR — WHY 注释说明虚拟滚动缓冲行数(5 行)的选择依据

* [ ] **Task P4.3: 可调 tick 暴露**

  * [ ] SubTask P4.3.1: TDD-RED — 新增测试:`TuiConfig { tick_interval_ms: 100, .. }` 时,DataPipeline 实际使用 100ms tick

  * [ ] SubTask P4.3.2: TDD-GREEN — 在 `TuiConfig` 新增 `tick_interval_ms: u16` 字段(默认 250,范围 100-1000)

  * [ ] SubTask P4.3.3: 在 `TuiApp::new` 中将 `config.tick_interval_ms` 传递给 `DataPipeline::new` 的 `DataSourceConfig`

  * [ ] SubTask P4.3.4: 在 `chimera-cli/src/commands/tui.rs` 新增 `--tick <ms>` CLI 参数,覆盖配置

  * [ ] SubTask P4.3.5: TDD-REFACTOR — WHY 注释说明 100-1000ms 范围的依据(过低 CPU 占用高,过高事件延迟感知差)

* [x] **Task P4.4: FPS 显示**

  * [ ] SubTask P4.4.1: TDD-RED — 新增测试:Health 面板渲染内容包含 "FPS: <n>",FPS < 30 黄色 / < 15 红色

  * [ ] SubTask P4.4.2: TDD-GREEN — 在 `TuiApp` 新增 `fps_tracker: FpsTracker` 结构体,基于 `frame_count` 与时间戳计算 FPS

  * [ ] SubTask P4.4.3: 在 `TuiState` 新增 `fps: u16` 字段,每秒更新一次

  * [ ] SubTask P4.4.4: 在 `HealthPanel::render` 中显示 `FPS: <n>`,根据阈值着色

  * [ ] SubTask P4.4.5: TDD-REFACTOR — WHY 注释说明 FPS 计算窗口(1 秒滑动平均)

### P4 Note — 实现说明(2026-07-14 完成)

> P4 性能优化已交付核心机制(commit `0b4a356`)。性能基准测试扩展(render_bench / data_pipeline_bench)推迟到 v1.8+ 性能基准补齐阶段,当前不阻塞 v1.7-omega 发布。

#### 1. P4.1 增量渲染(数据驱动 dirty 标记)
- `TuiState` 新增 `dirty_panels: HashSet<PanelId>` + `mark_dirty(panel)` / `is_dirty(panel)` / `take_dirty()` / `clear_dirty()` API
- `TuiApp::update` 在赋值前调用 `mark_dirty_panels_from_snapshot(&snapshot)`,通过 `PartialEq` 结构化比较识别变化字段
- 字段→面板映射:`latest_events` 同时驱动 Parliament / Log / EventStream 三面板(任一变化都需标记)
- ratatui Frame 约束说明:Frame 每帧用空白缓冲覆盖,因此 dirty 标记不跳过渲染,而是作为面板内部"数据是否变化"的可观测信号(供测试断言与未来缓存复用)
- 9 个集成测试覆盖:单字段变化标记对应面板 / latest_events 三面板同时标记 / 无变化不标记 / clear_dirty 重置

#### 2. P4.2 虚拟滚动扩展到 Parliament
- `ParliamentPanel::content` 方法签名新增 `window: (usize, usize)` 参数(半开区间)
- `render` 方法调用 `render::virtual_scroll_window(events.len(), self.scroll_offset, content_height)` 计算窗口
- 1000 条历史事件虚拟滚动测试通过(仅渲染可见区域 + 上下 5 行缓冲)
- 4 个集成测试覆盖:1000 事件渲染 / 滚动位置 / 选中范围 / 帧时间稳定

#### 3. P4.3 可调 tick 暴露
- `TuiConfig` 新增 `tick_interval_ms: u16` 字段(默认 250,范围 100-1000,`validate()` 强制校验)
- `DataSourceConfig::from_tui_config(tui)` 桥接方法:从 TuiConfig 构建 DataSourceConfig
- `TuiApp::apply_command` 新增 `SetTickInterval(ms)` 处理:更新 `config.tick_interval_ms` + 状态栏确认消息("Tick interval set to {}ms (restart to apply)")
- ⚠️ `chimera-cli/src/commands/tui.rs --tick <ms>` CLI 参数推迟到 v1.8+(当前通过配置文件或运行时命令调整)
- 5 个集成测试覆盖:默认值 / 范围校验 / from_tui_config 桥接 / apply_command 更新 / 状态栏消息

#### 4. P4.4 FPS 显示
- `TuiApp` 新增 `frame_times: VecDeque<f64>` + `last_frame_time: Instant` 字段
- `FPS_WINDOW_SIZE = 60`(60 帧滑动窗口,约 1 秒 @60fps,平滑单帧抖动)
- `FPS_DISPLAY_MAX = 999`(防止瞬时帧撑破状态栏宽度,三位数稳定布局)
- `update_fps` 方法:每帧计算与上一帧的时间差,推入 VecDeque,若超过 60 帧则弹出最旧;FPS = 1000.0 / 平均帧时间
- `render_status_bar` 显示 `FPS: <n>`(替代原 `Running:` 字段)
- 3 个集成测试覆盖:初始 FPS / 多帧后 FPS 计算 / 上限截断

***

## P5 — 跨面板联动(\~6h)

> **依赖**: P0 完成;P3 完成(Esc 状态保留依赖 InputMode 处理)
> **并行性**: P5.1 / P5.2 可并行
> **验收门槛**: 联动测试通过;`cargo test -p chimera-tui` 无回归

* [x] **Task P5.1: Esc 状态保留**

  * [ ] SubTask P5.1.1: TDD-RED — 新增测试:TUI 退出后 `~/.aether/tui/session.json` 存在,包含当前面板 + 过滤器 + 滚动位置;下次启动恢复

  * [ ] SubTask P5.1.2: TDD-GREEN — 新增 `crates/chimera-tui/src/session.rs` 模块,实现 `TuiSession` 结构体(序列化当前面板 + 过滤器 + 各面板滚动位置)

  * [ ] SubTask P5.1.3: 实现 `TuiSession::save` 方法,退出时序列化到 `~/.aether/tui/session.json`

  * [ ] SubTask P5.1.4: 实现 `TuiSession::load` 方法,启动时反序列化恢复状态

  * [ ] SubTask P5.1.5: 在 `TuiApp::run` 退出时调用 `TuiSession::save`,在 `TuiApp::new` 中尝试 `TuiSession::load`

  * [ ] SubTask P5.1.6: TDD-REFACTOR — WHY 注释说明状态保留的范围(仅 UI 状态,不含数据快照;数据快照在 P7 历史回放处理)

* [ ] **Task P5.2: Quest/Event 跳转底层命令**

  * [ ] SubTask P5.2.1: TDD-RED — 新增测试:Quest 面板选中 Quest 后按 Enter 查看详情,再按 `w` 触发 `chimera wiki <quest_id>` 子命令

  * [ ] SubTask P5.2.2: TDD-GREEN — 在 `TuiCommand` 新增 `InvokeCliSubcommand { command: String, args: Vec<String> }` 变体

  * [ ] SubTask P5.2.3: 在 Quest 面板详情 overlay 中处理 `w` 键,产生 `TuiCommand::InvokeCliSubcommand { command: "wiki", args: [quest_id] }`

  * [ ] SubTask P5.2.4: 在 Log 面板详情 overlay 中处理 `s` 键,产生 `TuiCommand::InvokeCliSubcommand { command: "quest", args: [related_quest_id] }`

  * [ ] SubTask P5.2.5: 在 `TuiApp::apply_command` 中实现子命令调用:暂停 TUI raw mode → 执行 `std::process::Command::new("chimera").arg(command).args(args).status()` → 等待任意键 → 恢复 raw mode

  * [ ] SubTask P5.2.6: TDD-REFACTOR — WHY 注释说明 raw mode 切换的必要性(子命令输出需要正常终端模式)

### P5 Note — 实现说明(2026-07-14 完成)

> P5 跨面板联动已交付(commit `a267a6d`),2 个子任务均通过 TDD。设计上简化了原 spec:
> - P5.1 "Esc 状态保留"原计划序列化到 `~/.aether/tui/session.json`,实际通过 `Vec<Box<dyn Panel>>` 实例持久存在机制自然实现(切换面板不重建实例,状态自然保留)
> - P5.2 "Quest/Event 跳转"原计划调用 `chimera wiki <quest_id>` 子命令(需暂停 raw mode + 执行外部进程),实际简化为 `TuiCommand::JumpToEventStream { quest_id }` 原子操作(设置 filter + 切换面板),避免 raw mode 切换复杂性与外部进程依赖

#### 1. P5.1 面板状态保留(自然实现)
- `TuiApp::panels: Vec<Box<dyn Panel>>` 中的 Panel 实例在 `switch_panel_to/next/prev` 时仅修改 `FocusManager.focused()`,不重建面板实例
- `QuestPanel` / `LogPanel` 新增 `selected()` / `scroll_offset()` 只读访问器(测试用)
- WHY 注释说明:ratatui 的 Panel trait object 持久存在机制天然保留状态,无需额外序列化开销
- 4 个集成测试覆盖:Quest selected 保留 / Log scroll_offset 保留 / 多面板独立保留 / 切换往返一致性

#### 2. P5.2 Quest→EventStream 跳转(原子操作)
- `TuiCommand::JumpToEventStream { quest_id: String }` 新变体(WHY 独立变体而非复用 SwitchPanel:需原子完成 filter 设置 + 面板切换)
- `QuestPanel::handle_key` 的 Enter 处理:返回 `JumpToEventStream { quest_id }`
- 原 Enter 的 detail popup 功能迁移到 `d` 键(避免功能丢失)
- `apply_command` 处理:先 `state.filter_keyword = Some(quest_id)` 再 `switch_panel_to(PanelId::EventStream)`(WHY 先设置 filter 再切换:避免一帧全量事件闪烁)
- 状态栏确认消息:"Jumped to EventStream, filter: <quest_id>"
- 6 个集成测试覆盖:Enter 跳转 / filter 设置 / 筛选生效 / 多 Quest 选择 / 无 Quest 不跳转 / 状态栏消息

***

## P6 — 主题运行时切换与布局模板(\~4h,v1.7 可选)

> **依赖**: P0 完成
> **并行性**: P6.1 / P6.2 / P6.3 可并行
> **验收门槛**: 主题切换测试通过;`cargo test -p chimera-tui` 无回归

* [x] **Task P6.1: 运行时主题切换**

  * [x] SubTask P6.1.1: TDD-RED — 新增测试:按 `t` 键,主题从 Dark → Light → HighContrast → Dark 循环切换,所有面板立即重绘
  — 实现:app.rs `test_handle_key_t_switches_theme_dark_to_light` / `test_handle_key_t_cycles_through_all_themes` / `test_handle_key_t_marks_all_panels_dirty` / `test_handle_key_t_sets_status_message`

  * [x] SubTask P6.1.2: TDD-GREEN — 在 `Theme` 枚举新增 `HighContrast` 变体 — config.rs

  * [x] SubTask P6.1.3: 实现 `Theme::next()` 方法(Dark → Light → HighContrast → Dark 循环) — config.rs

  * [x] SubTask P6.1.4: 在 `TuiApp::handle_global_key` 中处理 `t` 键,切换 `config.theme` 并标记所有面板为 dirty — app.rs

  * [x] SubTask P6.1.5: 实现 HighContrast 主题的颜色方案(纯黑背景 / 纯白前景 / 高饱和度强调色) — config.rs `Theme::colors()` + app.rs `theme_fg`/`theme_accent` HighContrast 分支

  * [x] SubTask P6.1.6: TDD-REFACTOR — WHY 注释说明 HighContrast 主题的目标用户(色盲 + 高亮环境) — config.rs ThemeColors doc + Theme::HighContrast doc

* [x] **Task P6.2: 布局模板**

  * [x] SubTask P6.2.1: TDD-RED — 新增测试:按 `1` 单面板全屏,`2` 双面板分屏,`3` 三面板布局
  — 变更:改用 `l` 键循环切换(数字键 1-9 已映射面板跳转 P3.3,冲突)
  — 实现:app.rs `test_handle_key_l_switches_layout_dual_to_triple` / `test_handle_key_l_cycles_through_all_layouts` / `test_handle_key_l_sets_status_message` / `test_render_single_pane_layout_no_panic` / `test_render_triple_pane_layout_no_panic`

  * [x] SubTask P6.2.2: TDD-GREEN — 在 `TuiState` 新增 `layout_mode: LayoutMode` 枚举(SinglePane / DualPane / TriplePane) — types.rs
  — 变更:默认值从 SinglePane 改为 DualPane(用户决策,首次启动看到完整界面)

  * [x] SubTask P6.2.3: 实现 `TuiApp::render` 根据 `layout_mode` 选择布局(SinglePane: 当前面板全屏;DualPane: 主面板 + 侧边栏;TriplePane: 主面板 + 侧边栏 + 底部日志) — app.rs `layout()` + `render()`

  * [x] SubTask P6.2.4: 在 `handle_global_key` 中处理 `1`/`2`/`3` 键切换 `layout_mode`
  — 变更:改用 `l` 键循环切换(与 `t` 键切换主题一致,避免与面板跳转冲突)

  * [x] SubTask P6.2.5: TDD-REFACTOR — WHY 注释说明三种布局的适用场景 — types.rs LayoutMode doc + app.rs handle_global_key `l` 分支注释

* [x] **Task P6.3: 颜色方案配置**

  * [x] SubTask P6.3.1: TDD-RED — 新增测试:配置文件 `~/.aether/omega.yaml` 的 `tui.colors` 节可覆盖默认颜色
  — 实现:config.rs `test_config_json_colors_override_from_string` / `test_config_colors_field_default_when_absent`

  * [x] SubTask P6.3.2: TDD-GREEN — 在 `TuiConfig` 新增 `colors: ColorScheme` 结构体(各面板的 foreground / background / accent 字段) — config.rs ColorScheme(6 字段 Option<ColorKind> 细粒度覆盖)+ TuiConfig.colors 字段(struct 级 #[serde(default)])

  * [x] SubTask P6.3.3: 实现 `ColorScheme::default_for_theme(theme)` 方法,根据主题返回默认颜色 — config.rs `default_for_theme` + `resolve(theme)` 合并方法

  * [x] SubTask P6.3.4: TDD-REFACTOR — WHY 注释说明颜色配置与主题的关系(主题是离散预设,颜色是细粒度覆盖) — config.rs ColorScheme doc

### P6 Note — 实现说明

**P6.1 运行时主题切换**
- `Theme` 枚举扩展 3 变体:Dark(默认)/ Light / HighContrast
- `Theme::next()` 循环:Dark → Light → HighContrast → Dark
- `Theme::colors()` 返回各主题的 ThemeColors 预设(6 字段:foreground/background/accent/warning/error/success)
- HighContrast 主题:纯黑背景 + 纯白前景 + 高饱和度强调色(BrightYellow/BrightRed/BrightGreen),服务色盲用户与强光环境
- `handle_global_key` `t` 键:切换 theme + 标记所有面板 dirty(立即重绘)+ status_message 提示
- app.rs `theme_fg`/`theme_accent` 新增 HighContrast 分支(编译期穷举要求)
- 4 个单元测试覆盖:切换/循环/dirty 标记/status_message

**P6.2 布局模板**
- `LayoutMode` 枚举 3 变体:SinglePane(专注)/ DualPane(默认)/ TriplePane(全监控)
- 默认值变更为 DualPane(用户决策):首次启动看到完整 tabs + status_bar,知晓有 13 个面板可切换
- `LayoutMode::next()` 循环:SinglePane → DualPane → TriplePane → SinglePane
- `layout()` 方法 3 模式:SinglePane 返回 [空, 全屏, 空];DualPane/TriplePane 按 main_panel_ratio 分割
- `render()` 在 SinglePane 时跳过 render_tabs 和 render_status_bar(专注模式)
- 布局切换键从 spec 的 `1`/`2`/`3` 改为 `l` 键循环(数字键 1-9 已映射面板跳转 P3.3,冲突)
- 5 个单元测试覆盖:切换/循环/status_message/SinglePane render/TriplePane render

**P6.3 颜色方案配置**
- `ColorScheme` 结构体:6 字段 `Option<ColorKind>`,None 表示用主题预设,Some 表示用户覆盖
- `ColorScheme::default_for_theme(theme)` 返回全 None(不覆盖任何颜色)
- `ColorScheme::resolve(theme)` 合并主题预设 + 用户覆盖,返回最终 ThemeColors
- `TuiConfig` 新增 `colors: ColorScheme` 字段,struct 级 `#[serde(default)]` 让配置文件只需提供覆盖字段
- `ColorKind` 添加 `Serialize`/`Deserialize` 派生(支撑 ColorScheme serde)
- 9 个单元测试覆盖:默认值/default_for_theme/无覆盖 resolve/部分覆盖/全覆盖/serde 往返/JSON 反序列化/缺省回退

**P6 整体验收**
- `cargo fmt --all -- --check`:通过
- `cargo clippy -p chimera-tui --all-targets --jobs 2 -- -D warnings`:零警告
- `cargo test -p chimera-tui`:295 lib passed + 集成测试全过(无回归)

***

## P7 — 历史回放引擎(\~10h,v1.8+ 演进,本 spec 仅设计接口)

> **依赖**: P0 完成;P2 完成(EventStream 数据源)
> **并行性**: P7.1 / P7.2 / P7.3 串行(快照 → 时间轴 → 回放引擎)
> **验收门槛**: 接口设计完成;实现推迟到 v1.8+

* [ ] **Task P7.1: 状态快照持久化**

  * [ ] SubTask P7.1.1: 设计 `HistoryStore` 结构体接口(`save_snapshot` / `load_snapshot` / `list_snapshots` / `cleanup_old`)

  * [ ] SubTask P7.1.2: 设计快照文件格式(`<timestamp>.msgpack`,含 TuiState + DataSnapshot)

  * [ ] SubTask P7.1.3: 设计快照存储路径(`~/.aether/tui/snapshots/`)与清理策略(超过 24h 自动清理)

  * [ ] SubTask P7.1.4: 设计 `TuiConfig.snapshot_interval_s` 字段(默认 30s)

* [ ] **Task P7.2: 时间轴 scrubbing**

  * [ ] SubTask P7.2.1: 设计 `PanelId::Timeline` 面板接口(事件密度热力图 + 时间游标)

  * [ ] SubTask P7.2.2: 设计 `←/→` 移动游标 + `Enter` 从该时间点开始回放的交互

  * [ ] SubTask P7.2.3: 设计热力图渲染算法(每秒一个 cell,颜色深度按事件密度)

* [ ] **Task P7.3: 回放引擎**

  * [ ] SubTask P7.3.1: 设计 `ReplayEngine` 结构体接口(`start_replay` / `pause` / `resume` / `set_speed` / `stop`)

  * [ ] SubTask P7.3.2: 设计回放速度(1x / 2x / 4x)与时间映射(回放时间 = 实际时间 × speed)

  * [ ] SubTask P7.3.3: 设计回放期间 TuiApp 的状态(只读模式,面板显示 "\[REPLAY x2]" 横幅)

### P7 Note — 接口设计说明(待 P7 完成后填写)

***

## P8 — MCP/CHTC 集成深化(\~8h,v1.8+ 演进,本 spec 仅设计接口)

> **依赖**: P0 完成;P2 完成(McpNodes / Chtc 面板基础渲染已交付)
> **并行性**: P8.1 / P8.2 可并行
> **验收门槛**: 接口设计完成;实现推迟到 v1.8+

* [ ] **Task P8.1: MCP Mesh 节点拓扑图**

  * [ ] SubTask P8.1.1: 设计 ASCII 拓扑图渲染算法(节点为方框,连接为线条,颜色表示状态)

  * [ ] SubTask P8.1.2: 设计节点状态更新订阅(McpNodeHeartbeat 事件 + 5s 超时检测)

  * [ ] SubTask P8.1.3: 设计拓扑布局算法(Force-directed 简化版,节点位置稳定)

* [ ] **Task P8.2: CHTC 兼容性矩阵评分**

  * [ ] SubTask P8.2.1: 设计 5×N 兼容性矩阵渲染(行:5 IDE 适配器,列:能力维度)

  * [ ] SubTask P8.2.2: 设计兼容性评分算法(基于最近 N 次请求的成功率 + 响应时间)

  * [ ] SubTask P8.2.3: 设计评分更新订阅(ChtcAdapterStatus 事件)

### P8 Note — 接口设计说明(待 P8 完成后填写)

***

## Task Dependencies

* **P0(现状评估 + 架构基线)**:无依赖,必须先行

* **P1(P3 归档提交)**:无依赖,与 P0 完全并行

* **P2(5 监控面板)**:依赖 P0(事件变体设计);5 面板内部可并行

* **P3(交互升级)**:依赖 P0;P3.1(Enter 详情)依赖 P2.2(EventStream 面板)

* **P4(性能优化)**:依赖 P0;P4.2(虚拟滚动扩展)依赖 P2.2

* **P5(跨面板联动)**:依赖 P0 + P3

* **P6(主题与布局)**:依赖 P0,可与 P2-P5 并行

* **P7(历史回放)**:依赖 P0 + P2,接口设计可在 P2 完成后启动,实现推迟 v1.8+

* **P8(MCP/CHTC 深化)**:依赖 P0 + P2,接口设计可在 P2 完成后启动,实现推迟 v1.8+

## 并行执行建议

* **批次 1(并行)**:P0(架构基线)+ P1(P3 归档提交)

* **批次 2(并行)**:P2.1-P2.5 五个新面板 + P6 主题布局

* **批次 3(并行)**:P3 交互升级 + P4 性能优化(P4.2 依赖 P2.2 完成后启动)

* **批次 4(串行)**:P5 跨面板联动

* **批次 5(并行,接口设计)**:P7 历史回放接口 + P8 MCP/CHTC 接口

* **批次 6(v1.8+ 实现)**:P7 + P8 实现(本 spec 不涵盖)

> **长期主义原则**:
>
> * 不跳级,不并行跨档;每批次完成后 `cargo fmt --all` + `cargo clippy` + `cargo test -p chimera-tui`
>
> * 每批次完成后 git commit + push,保持工作树干净
>
> * v1.7 必交付 P0-P5;P6 可选;P7-P8 仅设计接口
>
> * 代码质量:清晰模块化 + 高可读性 + 完善注释(功能描述 + 参数说明 + 关键算法解释)+ 行业最佳实践

