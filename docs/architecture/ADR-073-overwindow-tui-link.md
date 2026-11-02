# ADR-073:超窗/RAG 检索链 TUI 生产接线与展示联动

> **编号改号声明(2026-08-09)**: 原编号 ADR-072 已于 2026-08-07 被 Token 效率深度优化版占用(编号唯一性铁律)。本文件裁决改号为 ADR-073;历史引用见文末改号登记,旧引用不再改写。

- **状态**:Accepted(评审通过,待用户最终签收)
- **日期**:2026-08-06
- **决策者**:P1 功能闭环专家;8 专家评审 2026-08-06 通过(见文末);待用户最终签收
- **相关**:ADR-061(v3-engine M2)、ADR-070(HCW-Sparse PROBE)、ADR-071(recall 基线)
- **对应架构层**:L10 Interface(chimera-tui / chimera-cli 组合根)

## 背景(Context)

评估报告(F-2/F-3)确认:超窗兜底链(`OverWindowFallbackTriggered` 由 hcw-window 发布、
`OverWindowBridge` 已在 chimera-cli 实现 kvbsr→repo-wiki→hcw 两级检索)在 TUI 中
**零展示、零联动**——chimera-tui 全 crate 无 overwindow/RAG 引用,`commands/tui.rs`
未创建 bridge。同时存在两处生产断线:F-4(`tick_interval_ms` 配置未接线)、
F-3(task.* 5 个 is_core 空转入口必然失败)。

## 决策(Decision)

1. **会话总线 + 桥接注入,不做全局 EventBus 共享**:
   在 `commands/tui.rs` 组合根构造 `OverWindowBridge`(挂 TUI 会话总线),以
   `OverWindowHandle`(桥 + 会话语料提供者闭包)注入 Action 编排器。
   全局总线共享列为独立里程碑(避免大爆炸式改造);TUI 会话内触发的事件已可
   经 subscriber → pipeline → latest_events 到达 OverWindow 面板,闭环成立。
2. **`overwindow.run` 复用既有 `TuiActionRequested/Completed/Failed` 协议**:
   不新增事件变体;编排器按 action_id 路由到 `OverWindowBridge::run`,结果摘要
   回发 Completed/Failed,状态栏上屏(经 ActionFeedbackSync 既有链路)。
   需 `query` 参数(命令栏 `:overwindow run <词>`);palette 无参派发明确失败,
   语义同 agent.chat(不空跑、不伪造)。
3. **OverWindow 面板零管道侵入**:新面板从 `latest_events` 过滤
   `OverWindowFallbackTriggered`(语料规模/有效窗口/候选数/装窗数),复刻
   SelfAssessment/DagViz 的零侵入模式——不改 DataPipeline/TuiState 字段,
   不与其他专家并发编辑的数据层冲突。面板注册进 FocusManager 循环末尾。
4. **超窗有效窗口取 128K token**(HCW 四档之三):语料估算(字符数/4,与
   `chunk_corpus` 一致)≤ 窗口时不触发(零开销语义,不调 provider 不发事件)。
5. **task.* 空转入口下线**:删除 task 域 5 个 is_core 描述符(task.rs)与
   i18n 死键;编排器保留 task.* 防御分支(仍回 Failed 而非静默)。task CRUD
   真实化需 per-task 调度子系统,列为 Phase 3 ADR 独立立项。
6. **tick 配置断线修复**:`commands/tui.rs` 先加载持久化 `TuiConfig`,再以
   `DataSourceConfig::from_tui_config(&tui_config)` 构造管道;`SetTickInterval`
   持久化后下次启动生效(event_loop 提示语义由 P3 收口)。

## 影响(Consequences)

- 正向:超窗/RAG 链在 TUI 可观测、可触发、可诊断(F-2/F-3 闭环);tick 配置
  生效(F-4);交互入口不再说谎(F-3)。
- 成本:`overwindow.run` 每次执行重建块表 O(语料)——会话级规模可接受,
  复用 overwindow_bridge 锁外构建 + 原子 swap 模式(写锁仅覆盖 Arc 赋值)。
- 依赖铁律核验:L10(commands/tui.rs/action_orchestrator)→ L10 内组件
  (overwindow_bridge)同层互引;L2 hcw-window 不 import L5/L6(provider 闭包
  注入,ADR-033 先例);跨层通信仅 EventBus。无向上依赖。
- 兼容性:无公共事件类型变更;`DataSnapshot` 不变(零管道侵入);新增
  `PanelId::OverWindow` 为 additive 变体,`next()/prev()` 循环末尾接入。

## 验收(Test Plan)

- 编排器单测:超窗触发→Completed+`OverWindowFallbackTriggered`;窗口内→
  Completed 无触发事件;缺 query/未注入桥→Failed(4 个新测试)。
- 面板单测:空态提示/触发详情渲染/最新在前(3 个新测试)。
- 回归:`cargo test -p chimera-tui` + `cargo test -p chimera-cli` 全绿;
  clippy 无新增告警;task.* 相关测试已迁移到 System 域断言。
- 人工验证(真实终端):Tab 循环可达 OverWindow 面板;`:overwindow run <词>`
  触发后面板显示候选数;`~/.chimera/tui.yaml` 改 tick_interval_ms 重启生效。

## 实施收口登记(2026-08-06,P1/P2/P3 合并后)

> 本文档原为 P1 超窗链路草案;P1/P2/P3 三线合并后,将跨公共 API 的变更
> 统一登记于此,保证 ADR 编号与 SemVer 可见变更一一对应。

- **P2 公共 API 变更登记**:`TuiDataSource::snapshot()` 返回类型由
  `DataSnapshot` 改为 `Arc<DataSnapshot>`(共享不可变快照,每帧读取零拷贝)。
  属 SemVer 可见的 trait 签名变更,已同步适配 14 个集成测试文件的 mock 实现;
  `DataSnapshot` 新增 `#[serde(default)] revision` 字段(向后兼容,旧快照反序列化
  为 revision=0,`update()` 对 revision==0 走全量比较路径,测试语义不变)。
- **P3 交互收口**:`i` 详情死路径改绑 `v` 键;g5 未注册面板跳转改为状态栏警告;
  `quit_requires_confirm`(默认 false 零回归);Repeat 视同 Press;命令栏 Ctrl 组合键
  不进缓冲(Ctrl+L 仍切换语言);PgUp/PgDn 翻页落地;panel_menu 面板清单与注册同步。
- **基线修复登记**:`TuiState::load_from_file` 兑现"只恢复视图/布局字段"文档契约
  (白名单与 `apply_view_fields` 一致),运行时数据一律重置——修复测试封闭性
  (本机 ~/.chimera/tui_state.yaml 残留运行时字段曾导致 incremental_render_test
  误标无关面板)与生产会话陈旧状态污染。
- **CHANGELOG 合并说明**:TUI 变更条目未直接写入根 `CHANGELOG.md`(该文件正被
  并发的外部 CLI/llm 特性开发维护,避免冲突);建议条目文本由协调人随本次交付
  报告提供,待外部条目稳定后统一合并。

### F-5 palette 参数输入流登记(2026-08-06,后续任务)

- `ActionDescriptor` 新增 `requires_query: bool` 元字段(默认 false):
  标注"需要用户文本输入才能执行"的动作(`agent.chat` / `quest.start` /
  `overwindow.run`)。单一事实源驱动三入口分流,不引入硬编码动作清单。
- `TuiState` 新增 `pending_action: Option<PendingAction>`(serde skip,会话瞬态):
  palette 选中需 query 动作时置位并进入 Insert 参数收集态;提交以
  `{"query": text}` 经 `dispatch_action` 统一派发(复用 TuiActionRequested 协议),
  完成后回 Normal;Esc 取消;空输入不派发、不丢失 pending。
- 无参动作(palette 空 payload 直发)行为零回归;命令栏带参命令路径不变。
- 效果:修复评估 F-5("agent.chat/quest.start/overwindow.run 经 palette 必失败"),
  入口与底层语义对齐(编排器仍校验 query,空文本不会到达编排器)。

### U-3 面板正文 i18n 收口登记(2026-08-06,后续任务)

- 迁移范围(第一波):Health 系统资源摘要(CPU/RAM/Disk/Net)、Router 正文、
  Decay 正文、OverWindow 全文(含 P1 遗留字面量标题)。Budget/Memory 已在前轮
  由 P3 迁移。
- 迁移范围(第二波):Quest(列表标题/空态/元信息/任务统计/详情/批量确认)、
  Parliament(正文标题/空态/投票 FOR-AGAINST/检测率/失败计数/免疫行)、
  Security(正文标题/空态/检测率标签/详情弹窗全字段/冻结列表)、Log(正文/
  空态/虚拟滚动提示/过滤指示器)。新增 48 键 + `common.none`;Parliament
  RedTeamAudit 行 `risk=` 标签修正为 `detection=`(与 Security 一致,U-1)。
- 实现约定:`t!()` 返回运行时 `&'static str`,不能作 `format!` 字面量,故所有
  含数值的文案采用"标签键 + 调用点拼接"模式(与 Health 面板既有风格一致),
  不引入占位符模板;key 与宏调用保持同处一行(i18n_completeness_test 行级
  扫描器约束);新键全部加入 `SEED_KEYS` 保证 zh/en 对齐。
- 测试策略:in-file 单测断言默认中文渲染(零全局 locale 竞态);集成帧断言
  对 TestBackend 的 CJK 逐格空白做压缩后匹配;`all_t_macro_keys_exist_in_
  both_tables` / `seed_keys_are_used_in_code` 自动守护新增键。
- 未迁移面板(第三波,单独立项):event_stream / task_manager / chtc /
  mcp_nodes / clv_vector / osa_sparse / sysinfo / timeline / dag_viz /
  self_assessment / pvl_score / metrics_dashboard / resource_monitor /
  chat / help,其中 event_stream 与 task_manager 英文文案量最大。

### M3 v3-engine 输出接线登记(2026-08-06,遗留问题 3)

- **决策**:事件循环输出路径切换为自研 `DoubleBuffer` diff + `TerminalWriter`
  ANSI 写出;面板仍渲染到内存 ratatui 帧(TestBackend),经 `from_ratatui_buffer`
  翻译后交由引擎 diff/writer——面板代码与 ~74 个测试零改动,渐进迁移可回滚。
- **宽字符前置修复**:ratatui 0.29 不写 skip 标志,续格保留旧内容;compat 按
  Unicode 显示宽度(新增直接依赖 `unicode-width`,ratatui 同源传递依赖)定位
  续格并映射为哨兵 `Cell::WIDE_CONTINUATION`,`TerminalWriter` 跳过续格输出,
  避免默认中文 UI 汉字右半格被空格覆盖。engine_compat_test 等价规则同步更新
  (续格哨兵 ↔ ratatui 空符号/前格宽度 ≥2)。
- **回退**:`CHIMERA_NO_V3_ENGINE=1` env 或 `--no-v3-engine` 走原 ratatui
  `Terminal::draw` 路径(2 个版本周期兼容窗口,与 M2 布局切换同策略)。
- **基准**:`v3_output_bench`(criterion,正式 100 样本):80×24 相同帧 14.92µs
  (零输出增量路径)、120×40 全量 37.75µs,均低于引擎文档 100µs 预算;与
  quick 基线对比无回归(-0.17% / -0.86%)。
- **遗留**:M4 让 `dirty_panels` 驱动面板跳过未变渲染(当前 diff 已省输出,
  帧内面板重建仍全量);M5 移除 ratatui 依赖与兼容层。真实终端人工验证仍为
  发布前必做项(AGENTS.md UI 铁律)。

### M4 v1 Log 过滤缓存登记(2026-08-06,遗留问题 2)

- 提取共享 `FilterCache`(键 = snapshot revision + keyword/topic/level,存
  latest_events 正序索引),EventStream 与 Log 统一复用(消除 EventStream 私有
  缓存实现与 Log 每帧 O(n) 过滤的重复)。revision == 0(测试桩)或无关键字时
  走无缓存路径,防就地修改导致的陈旧结果。
- 基准(criterion 正式 100 样本):Log 关键字过滤 1 万事件,冷路径 9.63ms/帧 →
  缓存命中 146.7ns/帧;EventStream 关键字过滤 9.50ms(content 66.1µs)。

### M4 二期 Quest/Parliament 渲染缓存登记(2026-08-06,遗留问题 2 收口)

- Quest/Parliament 面板新增渲染缓存键(Quest: revision + filter_keyword +
  selected + 排序后的 selected_indices + locale;Parliament: revision + selected
  + 虚拟滚动 window + locale),命中时跳过 O(n) 过滤与格式串重建,仅 clone
  缓存 Text 供 Paragraph 写入本帧 buffer;revision == 0(测试桩)自动禁用缓存。
- 基准(criterion 正式 100 样本,100 条数据):Quest 冷 362µs → 命中 105µs(≈3.4×);
  Parliament 冷 93.1µs → 命中 80.8µs(≈1.15×,虚拟滚动已削减构建量)。
- 新增 12 个单元测试 + 1 个 proptest(缓存键与多选集合迭代顺序无关、对选中项
  敏感);附带修复 app/render.rs 与 panels/overwindow.rs 两处既有 locale 并行
  竞态测试(仓库既有脆弱点,本次暴露后顺手修复)。
- 遗留:M4 三期候选——命中路径仍含 `Paragraph::new(text.clone())` 克隆成本,
  彻底零克隆需引擎级 Cell 复用(另行评估)。

### 协调人复核记录与冒烟验证(2026-08-06,遗留问题 1/4)

- **冒烟验证**:非 TTY 重定向下 `chimera tui`(v3 默认启用)稳定运行 8s+,
  stdout 含完整 ANSI 帧序列(`?1049h` 备用屏、MoveTo、样式),第二帧起仅输出
  状态栏变化格(增量 diff 生效),中文面板文案(任务列表/暂无进行中的任务)
  无渲染破损;无 panic/错误输出。真实终端交互验证仍待用户执行(Windows
  Terminal + `CHIMERA_NO_V3_ENGINE=1` 对比)。
- **复核结论**:P1(超窗/RAG/tick/task.* 下线)、P2(Arc 快照/虚拟滚动/缓存/
   基准)、P3(交互死路径/快捷键/退出安全/i18n 两波)、M3(v3 输出接线/宽字符)、
   M4 v1/v2(FilterCache 与 Quest/Parliament 渲染缓存)均已通过独立门禁(全量
   回归、clippy 零告警、criterion 数据),实现与本文档决策一致。状态见下方
   「8 专家评审纪要」:Accepted(评审通过,待用户最终签收)。
- **M5(遗留问题 3)**:移除 ratatui 依赖与兼容层,按 ADR-061/072 约定在
  2 个版本周期后执行,本轮不实施(避免与 v3 输出路径并存窗口冲突)。
- **验证产物(遗留问题 1)**:`scripts/tui_verify.ps1`(三场景非交互验证:
  v3 默认 / CHIMERA_NO_V3_ENGINE=1 回退 / stdin 管道,ANSI 归一化断言中文
  文案);`docs/architecture/TUI_MANUAL_VERIFICATION_CHECKLIST.md`(真实终端
  人工验证清单,待用户在 Windows Terminal 签收);
  `docs/architecture/M5_RATATUI_REMOVAL_ANALYSIS.md`(M5 前置分析:56 源文件/
  315 处 ratatui 引用,阶段 0→3 路线图与六大硬阻塞)。实测:v3 首帧 6.9KB vs
  后续最大 435B(增量 diff),回退路径 3.3KB vs 344B;两路径均稳定运行无 panic。

## 8 专家评审纪要(2026-08-06,遗留问题 4)

> 评审范围:本文档全部决策与登记(P1 超窗/tick/task.*、P2 Arc 快照、P3 交互
> 收口、F-5 参数流、U-3 i18n 两波、M3 v3 输出接线、M4 v1 FilterCache)及配套
> 代码证据。评审方式:逐节核对 + 代码抽查(overwindow.run/OverWindowHandle/
> from_tui_config/requires_query/pending_action/Arc<DataSnapshot>/revision/
> FilterCache/V3Output/PanelId::OverWindow 均已确认存在且与文档一致)。

### 结论汇总:8/8 有条件同意(无反对)

| 角色 | 结论 | 关键条件/意见 |
|---|---|---|
| E1 首席架构 | 有条件同意 | 会话总线 + 桥接注入正确避免全局总线大爆炸;`overwindow.run` 复用 TuiAction 协议零事件变更,符合依赖铁律。条件:M4 二期并入后复核一次公共 API 面 |
| E2 安全架构 | 有条件同意 | `forbid(unsafe_code)` 未破坏;overwindow 检索经既有编排器校验 query,不空跑;回退开关保留排障通道。条件:真实终端验证通过前不得宣称发布就绪 |
| E3 记忆系统 | 有条件同意 | OverWindow 面板从事件流派生展示,零管道侵入;128K 窗口估算与 chunk_corpus 一致。条件:确认会话语料规模上限不会导致 `overwindow.run` 单次重建超过可接受延迟(建议后续基准) |
| E4 路由算法 | 有条件同意 | FilterCache 键(revision+过滤器)设计正确,索引防悬垂;M4 二期缓存键需并入 selected/scroll/locale/theme,已列为实施条件 |
| E5 生产系统 | 有条件同意 | `Arc<DataSnapshot>` + revision 增量与 v3 diff 输出形成完整性能链路;criterion 数据(相同帧 15µs、Log 冷 9.74ms→热 175ns)可证伪。条件:取消性能用例 `#[ignore]` 防线并纳入 CI 常规门禁 |
| E6 认知科学 | 有条件同意 | `requires_query` 参数流消灭"有入口无出口"认知摩擦;`quit_requires_confirm` 默认关闭保证零回归。条件:i18n 第三波(15 面板)完成前,帮助/提示不得宣称全量双语 |
| E7 任务调度 | 有条件同意 | task.* 空转入口下线正确(诚实 Failed 优于假成功);palette 参数流 Esc/空输入语义完备。条件:task CRUD 真实化单独立项时复用 PendingAction 模式 |
| E8 前端交互 | 有条件同意 | v3 输出接线后非 TTY 冒烟:两路径均无 panic、备用屏/中文文案正常、增量 diff 生效(首帧 6.9KB vs 后续最大 435B);回退路径逐格定位文本已由 ANSI 归一化断言修正。条件:Windows Terminal 人工清单(scripts/tui_verify.ps1 + TUI_MANUAL_VERIFICATION_CHECKLIST.md)逐项通过后关闭遗留问题 1 |

### 评审一致条件(转 Accepted 的绑定条件)

1. 真实终端人工验证(Windows Terminal,含 `CHIMERA_NO_V3_ENGINE=1` 对比)由
   用户执行并逐项签收后才可宣称 UI 完成(AGENTS.md UI 铁律)。
2. M4 二期(Quest/Parliament 脏驱动面板缓存)完成后,由协调人并入本文档并
   触发一次增量复核;本文档状态已按"评审通过"置为 Accepted,最终签收仍归用户。
3. M5 阶段 0(冻结新 ratatui 依赖面)自本纪要起立即生效,详见
   `docs/architecture/M5_RATATUI_REMOVAL_ANALYSIS.md`。
4. CHANGELOG 合并与最终提交由协调人统一执行(本文档不直接编辑 CHANGELOG)。

### 评审发现的不一致与处置

- 回退路径(B)首次冒烟"中文 MISS"实为脚本断言缺陷(逐格 ANSI 定位打断连续
  字符串),已修复为"先剥离 ANSI 再断言",非产品缺陷;两路径中文文案均正常。
- ADR 正文"面板注册进 FocusManager 循环末尾"与实现一致(23 面板循环)。
- 未发现文档与实现的实质不一致;行号级漂移(如 event_loop 行号)不影响结论。
