# ADR-076:双模式默认分层 — Chat 第一默认,驾驶舱下沉(Concord W3 会话模式)

- **状态**:Accepted(随 Concord 重构 W3 波次实施落地)
- **日期**:2026-08-10
- **决策者**:Concord 重构波 3 实施(《Chimera_CLI_TUI重构方案_v1.0》§5 设计理念与 §15.3 提案沉淀)
- **相关**:ADR-029(TUI v3 交互重构/ActionRegistry)、ADR-034(禁用运行时 Feature Flag)、ADR-075(`/` 命令第一公民)、ADR-077(四源 codegen)
- **对应架构层**:L10 Interface(chimera-tui)

## 背景(Context)

重构方案 §5 裁定:TUI 的第一印象应是与主流 Agent CLI 同构的会话流(Conversation-First),
25 面板驾驶舱是高级资产而非默认负担。W1/W2 完成了命令翻转与地基收口,但默认视图仍是
面板布局——用户进入 TUI 第一眼看到的不是对话,与方案目标不符。同时需要回答三个问题:
(1)默认视图如何切换才不破坏 25 面板资产;(2)流式输出的半行闪烁如何根治(R3 风险);
(3)模式选择如何跨会话保留。

## 决策(Decision)

1. **ViewMode 双模式分层**:`TuiState.view_mode: ViewMode{Chat, Dashboard}`;
   Chat 为第一默认(新会话/旧状态文件兼容均得 Chat),Dashboard 为第二默认
   (25 面板驾驶舱完整保留,资产下沉不推倒)。
2. **配置入口**:`TuiConfig.default_view_mode`(方案 §8.2 既定增强项),构造器在无
   持久化状态时用其初始化;持久化状态存在时以状态文件为准(用户运行时选择优先),
   view_mode 纳入 load_from_file 恢复白名单。
3. **互切键位**:`\` 键复用为 Chat<->Dashboard 互切(方案 §7.4);原 `view.toggle_companion`
   动作保留但移除默认键,改经命令面板与新增公开入口 `TuiApp::toggle_companion()` 访问。
   `/chat` `/dashboard` 命令同步改接视图模式切换(W2 过渡映射退役)。
4. **Chat 视图布局**:会话流(弹性)+ composer 底栏(Fixed 3)+ statusline(Fixed 1),
   走既有 engine::layout::flex::split 求解器(不引入第二套布局系统);Chat 流复用
   已注册 Chat 面板实例(滚动/跟随状态跨模式保留);Normal 态 composer 显示输入提示占位。
5. **NewlineGate 行闸门(R3 缓解)**:流式增量在 ChatSync 累积层按完整行提交,
   半行与未闭合 fence 块暂存,Completed 冲刷残段;内容守恒由 proptest 常绿守护
   (任意切分下 feed+flush == 原始流);渲染层(v3 引擎)零改动,保住 diff<100μs。
6. **会话流内嵌卡片**:QuestPlanCard(从 quest_list 零管道派生计划步骤)+
   ReflectionCard(QuestCompleted{Failed/Cancelled} 逆序扫描),失败告警优先渲染。
7. **ADR-034 合规**:ViewMode 为会话状态而非运行时 feature flag;不新增 NexusEvent 变体。

## 后果(Consequences)

**正面**:
- 会话优先的第一印象,与五家主流 Agent CLI 交互同构(方案 §1 对齐清单)。
- 行闸门根治半行闪烁;ttfb 实测 118ns(断言式守护 <1ms)。
- 双模式互切一键(`\`)+ 命令(`/chat` `/dashboard`),模式选择持久化保留。

**代价与缓解**:
- 默认视图变更冲击既有渲染测试:Dashboard 断言类测试显式置 Dashboard + 禁用持久化
  (同时消除对用户状态文件的环境依赖,测试更密封);变更已诚实披露于进度报告。
- `\` 键语义迁移(R1 同类风险):companion 切换保留命令面板与程序化入口,
  README 注明迁移路径;`backslash_toggles_view_mode` 测试守护新语义。
- companion/模式切换的状态组合空间扩大:集成测试覆盖互切往返、Insert 模式隔离
  (`\` 在 Insert 内为文本字符不触发切换)。

## 验证(Verification,2026-08-10)

- chimera-tui 1318 passed / 0 failed(基线 1283,净增 35);chat_mode_test.rs 13 用例。
- workspace --tests 9590 + doctest 串行 295 = 9885 合计全绿(R8 红线未击穿)。
- clippy --workspace --all-targets -D warnings / fmt / 依赖铁律(38/38)/ 词表脚本 全 EXIT=0。
- bench 对比 refactor_base:v3_pipeline 全指标 ≤1.12ms(远小于 16ms 帧预算);
  diff ≤82μs(<100μs);push_history ≈106ns O(1);streaming_newline_gate 组
  (ttfb_first_line 118ns / feed_100_lines_charwise 33.7μs)已存入 refactor_base。
