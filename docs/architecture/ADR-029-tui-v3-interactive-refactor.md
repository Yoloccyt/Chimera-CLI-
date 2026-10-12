# ADR-029: AETHER TUI v3.1 交互式重构 — 自研渲染引擎与 Action 统一协议

## 状态

已接受 (2026-07-21)

## 背景

- 项目当前处于 v2.3.1-omega,`chimera-tui`(L10 Interface 层)已是成熟的**事件驱动监控仪表盘**(ratatui 0.29 + crossterm 0.28,约 20 面板 / 40+ 测试 / 2 benches),数据流为 `EventBus → EventSubscriber → DataPipeline → TuiApp.update() → render`。
- `Chimera_tui_v3_architecture.md`(AETHER TUI v3.0)提出五层架构(OS 抽象 / 终端抽象 / 双缓冲差异渲染 / Flexbox 布局 / 类 React 组件系统)与四种布局模式(IDE/Chat/Vim/Focus),定位从"监控式 TUI"升级为"**交互式 TUI**"。
- 经三视角并行规划(简洁可维护 / 性能可扩展 / 最小变更低风险)与两轮批判性评审,确认重构须同时满足三条项目铁律:`#![forbid(unsafe_code)]`、L10 依赖铁律(仅依赖 event-bus + nexus-core)、TDD 守恒(不删测试 + CI 全绿)。
- 本 ADR 记录 v3.1 交互式重构的 8 项关键架构决策,作为 M0-M5 里程碑实现的权威依据。

> **ADR 编号确认**: `docs/architecture/adr_index.md` 现有最大编号为 ADR-028 + ADR-SIMD-001。本 ADR 编号确认为 **ADR-029**。

## 决策

### 决策 1: 自研渲染引擎从零重建 — 在安全铁律下的正确解读

以 `crossterm` 保留为 L1/L2 终端后端(其 unsafe 封装在自身 crate,不违反 `#![forbid(unsafe_code)]`),**自研 L3 双缓冲差异渲染 + L4 Flexbox/Grid 布局 + L5 组件系统**(纯 safe Rust)。

- "自研渲染引擎"= 重写**重绘策略**(双缓冲 diff、行级脏追踪、样式池),而非重写 raw mode / TTY / ANSI 跨平台层(那需要 unsafe/FFI)。
- 新增 `crates/chimera-tui/src/engine/`:`rect` / `style`(含 `StylePool` 去重) / `buffer`(含 `DoubleBuffer` / `DirtyTracker`) / `diff`(逐格 diff,M1 合并 Span) / `writer`(经 crossterm 输出 ANSI)。

### 决策 2: Action 统一协议 — append-only 扩展 NexusEvent

将 TUI 交互升级为统一 Action 协议,`event-bus/src/types.rs` **append-only** 新增 8 个变体(不改动现有 74 变体序列化):

- `TuiActionRequested { action_id, payload, source }` — 三入口统一派发点(severity=Info)
- `TuiActionProgressed { action_id, delta }` — 流式进度(severity=Normal)
- `TuiActionCompleted` / `TuiActionFailed` — 终态反馈(severity=Info)
- `TuiChatSubmitted` / `TuiChatResponseChunk` / `TuiChatCompleted` / `TuiChatStatusChanged` — 对话子协议(agent.chat 特化)
- 配套 `ActionSource{Chat,Palette,Panel}` 与 `ChatStatus{Thinking,ToolExecuting,Idle}` 枚举。
- 命名以 `TuiAction*` / `TuiChat*` 前缀区别于既有 7 个 MAS `Agent*` 事件,避免语义碰撞。`payload` 为 JSON 字符串,保持 L1 event-bus 与 TUI 动作语义解耦。

### 决策 3: 高频流式事件禁走 mpsc 旁路 — 走 broadcast + Normal

`TuiChatResponseChunk`(token 流)与 `TuiActionProgressed`(进度)为高频事件,severity 定为 **Normal**,走 broadcast 通道 + 低延迟 drain。

- **理由**: event-bus 的 mpsc 旁路(`is_critical_mpsc_event`)仅为稀有安全告警事件(SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded/AgentTaskFailed)保留;高频 token 灌入会破坏其点对点投递保证语义。
- TUI 侧只标记光标行 dirty 实现增量渲染;`event_loop` 交互模式下降低 poll 超时(20-30ms)使 token 及时上屏。

### 决策 4: Agent 编排在 chimera-cli — L10 依赖铁律合规

交互式 Agent 对话的编排(QueryLoop / 域处理器)放在 `chimera-cli`(bin,可依赖任意下层),经 EventBus 与 TUI 双向通信;`chimera-tui` crate 仍只依赖 event-bus + nexus-core。

- TUI 只发起 `TuiActionRequested` / `TuiChatSubmitted`、接收 `*Progressed/Completed/Failed/Chunk`,不直接持有 Agent 状态,不引入向上依赖。

### 决策 5: Action Registry 单一事实源 + codegen 防漂移

新增 `crates/chimera-tui/src/actions/`:`ActionDescriptor` / `ActionRegistry`(索引 + 查询 + 模糊匹配)/ 六域分包(quest/task/export/view/system/config)/ `codegen`(生成斜杠命令 / 命令面板条目 / 帮助内容)。

- **铁律**: 斜杠命令、命令面板、? 帮助、快捷键描述全部由 `codegen` 从 Registry 自动生成,禁止任一入口手写功能清单——这是"三入口行为一致、永不漂移"的工程保证。
- **防上帝对象**: Action 按域分包注册,Registry 只做索引;注册项突破 `MAX_ACTIONS = 40` 触发熔断,回到"核心功能 ≤3 键可达"重新裁剪粒度。
- 首期注册基线 §五(约 17-21 个 Action),既有功能 100% 可经至少一个入口触发。

### 决策 6: InputRouter 三态路由 — 提级为正式任务

新增 `crates/chimera-tui/src/input/router.rs`:`InputRouter` 无状态纯函数 `route(mode, key) -> RouteTarget`,实现 Normal/Insert/Command 三态路由表。

- **理由**: 引入 Insert/Vim 模式 + 可交互面板后,按键所有权(全局快捷键 vs 面板动作 vs 聊天原始输入)立即复杂化,InputRouter 是交互式 TUI 的前置条件,而非技术债。
- 优先级: Normal(全局快捷键 > 模式切换 > 焦点轮转 > 焦点面板);Insert(Esc 退出 > 少数全局键 > 提交/退格 > 原始字符);Command(Esc > 上下选择 > 提交 > 退格 > 检索字符)。以 D 类快照测试穷举验证。
- **注(M2 item② router 完备化扩展,2026-07-23 实证)**: 实际实现为 **5 态** —— 三主态(Normal/Insert/Command) + 两个瞬态前缀态(`GPrefix` / `WPrefix`,对应 `g` 前缀与 Ctrl+W 前缀)。代码实装于 `input/router.rs:26-45`。`RouteTarget` 含 22 个变体,14 项 D 类快照测试覆盖全部 5 态。

### 决策 7: i18n 默认中文 + 运行时快捷键切换 — 编译期静态表

新增 `crates/chimera-tui/src/i18n/`:`Locale{Zh,En}` + 全局 `AtomicU8`(默认中文)+ `t!` 宏 + `Ctrl+L` 运行时切换。

- **不引 fluent/gettext**: 文案 < 500 条,用 `match` 编译期静态表,零运行时初始化开销、不扩大 cargo audit 面、不增二进制体积。
- key 命名 `模块.组件.用途`;缺失回退到 key 本身(渐进接入期不 panic);中英表由单测强制对齐(禁止半译)。`ActionDescriptor` 存 i18n key 而非字符串,locale 切换时帮助与 palette 文案同步切换。

### 决策 8: feature-flag 双轨渐进迁移 — TDD 守恒 + CI 全绿

以 `v3-engine` feature 开关实现自研引擎与 ratatui 双轨并存,逐面板迁移,每个里程碑 `cargo test --workspace` 全绿、随时可回滚;M5 移除 ratatui 与兼容层,引擎成为唯一渲染路径。

- M0-M1 引擎默认编译供 CI 类型检查;M2 起经 `RatatuiCompat` 兼容层桥接现有面板与渲染快照测试;现有 ~40 测试按四类(A 逻辑保留 / B 渲染快照桥接 / C 交互闭环新增 / D 输入路由快照)迁移,不删测试。

## 理由

- **调和"从零重建"与三条铁律**: 终态完全自研引擎(honors 用户明确选择),但路径双轨渐进,不违反 TDD 守恒 / CI 全绿 / 长期主义。
- **交互能力不被引擎进度绑架**: Action 协议 / Registry / InputRouter 均不依赖渲染引擎,与引擎开发并行推进。
- **append-only + 语义前缀区分**: NexusEvent 用 `#[serde(tag,content)]` 字符串 tag,新增变体不破坏现有序列化;`Tui*` 前缀避免与 MAS `Agent*` 碰撞。

## 影响

- `event-bus`: +8 NexusEvent 变体 + 2 枚举(ActionSource/ChatStatus),`severity()`/`metadata()`/`type_name()`/`topic()` 相应扩展,append-only 不破坏现有消费者。
- `chimera-tui`: 新增 `engine/` `components/` `i18n/` `actions/` `input/` 五模块 + `v3-engine` feature + 2 benches;`TuiCommand` 新增 `DispatchAction` 变体(三入口统一派发)。
- `chimera-cli`: M3 起 `commands/tui.rs` 订阅 `TuiActionRequested`/`TuiChatSubmitted` 并编排 Agent。
- CI/发布: append-only 不破坏 5 平台 build/test/docker;M5 移除 ratatui 后可清理 audit.yml 的 lru/paste ignore。

## 替代方案(被否决)

1. **一次性推翻式重写**: 直接替换 ratatui + 重写全部面板/测试。否决:违反 TDD 守恒、CI 长期红、回滚困难。
2. **流式 token 走 Critical mpsc 旁路**: 否决:mpsc 旁路仅留给稀有安全告警,高频 token 会破坏其投递保证。
3. **在 chimera-tui 内直接编排 Agent/工具**: 否决:违反 L10 依赖铁律。改为经事件 + chimera-cli 编排。
4. **永久保留 ratatui 仅重组面板**: 否决:不满足"自研渲染引擎从零重建"的明确选择;仅作 M1-M4 过渡手段。
5. **引入 fluent/gettext 做 i18n**: 否决:文案 < 500 条,外部 crate 增依赖与体积;编译期静态表足够且更快。
6. **每面板自定义交互语法 / Chat 中心制**: 否决:违背"简洁清晰"与"一切功能皆可交互";统一 Action 层 + 三入口 + 一套交互语法替代。

## M0 落地状态(2026-07-21)

- ✅ event-bus:8 事件 + 2 枚举 append-only 落地,`severity`/`metadata`/`type_name`/`topic` 扩展,2 项新单测通过。
- ✅ chimera-tui:`i18n` / `actions`(六域 + Registry + codegen)/ `input`(三态路由 + D 类快照)/ `engine`(rect/style/buffer/diff/writer,含 proptest 幂等)/ `components`(ComponentPanel 骨架)五模块落地;`v3-engine` feature + 2 benches(diff_engine / streaming,RED-first)。
- ✅ `TuiCommand::DispatchAction` + `apply_command` 处理(复用 `publish_control_event`,同步走 publish_blocking)。
- ✅ 验证:`cargo check --workspace` / `cargo test -p chimera-tui`(507 lib + 全部集成 + doctest)/ `cargo clippy -p chimera-tui -p event-bus --all-targets -- -D warnings` / `cargo fmt --all --check` 全绿,feature off 零回归。

## 附录修订 A:动作协议回执归属 request_id(2026-09-09)

> 来源:2026-09-09 chimera-tui 四维度评估报告 [F-1](高危,回执错配属数据/状态错误)。

**变更**:`TuiActionRequested` / `TuiActionCompleted` / `TuiActionFailed` 三变体新增 `request_id: String`(带 `#[serde(default)]`,空串 = 无归属,旧持久化事件回放兼容);编排器必须**原样回传**发起方生成的 `request_id`。

**动机**:原协议仅凭 `action_id` 无法区分同一动作的多次连续/并发请求;TUI 侧全局唯一 `pending_action_deadline` 会被先到回执无条件清空,产生"实际失败却显示成功"或"超时永不提示"的静默失败。

**配套语义**:
- `TuiState.pending_action_deadline: Option<Instant>` → `pending_actions: HashMap<String, Instant>`(`request_id` → 截止时刻),`TuiApp::check_action_timeout` 仅摘除过期项;
- `TuiActionProgressed` **有意不携带** `request_id`:进度为幂等流式展示、非回执,无消费方按请求归属,引入主键只扩大变更面无收益。后续如需按请求聚合进度,以独立 ADR 补充;
- TUI 以本地单调序号发号(`tui-{seq}`),进程内唯一即可完成配对,零依赖、可审计。

**验证**:event-bus / chimera-tui / chimera-cli `--all-targets` 编译零错误;归属不变量由 `completed_echoes_request_id_of_request` / `failed_echoes_request_id_of_request`(编排器)、`pending_actions_keyed_by_request_id` / `feedback_removes_only_matching_request` / `timeout_reports_and_clears_only_expired` / `action_request_seq_monotonic` + proptest `dispatched_requests_get_distinct_ids`(TUI)守护。

---

> **维护者**:NEXUS-OMEGA 团队
> **文档版本**:ADR-029 起草(2026-07-21,v3.1 M0);修订 A(2026-09-09,request_id 回执归属)
