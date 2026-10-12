# ADR-077:键位/焦点/命令/帮助四源 codegen 合一(Concord W1·P5 双源漂移收口)

- **状态**:Accepted(随 Concord 重构 W1 波次 T1.3/T1.4 实施落地)
- **日期**:2026-08-10
- **决策者**:Concord 重构波 1 实施(《Chimera_CLI_TUI重构方案_v1.0》§15.3 提案编号续写)
- **相关**:ADR-029(TUI v3 交互重构/ActionRegistry)、ADR-034(禁运行时 Feature Flag)
- **对应架构层**:L10 Interface(chimera-tui)

## 背景(Context)

重构方案 §2 P5 诊断出两处"声明式注册只做了一半"的双源结构,
W1 波次 T1.2 双源一致性不变量测试先行(INV-F / INV-K-A / INV-K-B),
**首测即捕获漂移实锤**:

1. **焦点双源(INV-F 红)**:`PanelId::next/prev` 手写静态环含未注册面板
   Timeline/Sysinfo,且 OsaSparse/ClvVector 相对顺序与 `TuiApp` 面板注册序
   相反——新增/下线面板需两处同步,漏改即静默漂移。
2. **键位双源(INV-K-B 红)**:`view.switch_layout` 被 `l` 键全局路由,
   但 `ActionDescriptor.default_key` 未声明——路由表硬编码与声明表脱节。

## 决策(Decision)

1. **焦点环单一事实源**:`types.rs` 新增 `PanelId::REGISTERED_FOCUS_ORDER`
   常量表(25 面板,含 Timeline/Sysinfo 接线注册);`next()/prev()` 改为
   派生实现(position 查找 + 环取模),`TuiApp` 注册序经 `make_panel` 工厂
   由同一源表映射。焦点遍历、静态环、注册序三者同源,INV-F 不变量测试守护。
2. **键位表 codegen 第四通道**:`codegen.rs` 新增 `key_bindings(reg)` 从
   `ActionDescriptor.default_key`/`alias_keys` 派生 `Vec<KeyBinding>`;
   `InputRouter::route_normal` 删除 7 处硬编码 `GlobalAction` 分支,改为
   `global_action_for(key)` 查表(未命中回退 `FocusPanel`)。WPrefix `w`
   与 Normal 同源查表;Insert 模式保留最小子集(Ctrl+L)并注释说明。
3. **别名键收进声明**:`ActionDescriptor` 新增 `alias_keys` 字段
   (如 export.run 的 `E`),历史上路由表的未声明等价键全部显式化。
4. **面板内键豁免标记**:新增 `global_route` 字段,`panel.drill_down`(Enter
   由焦点面板消费)标记 `false`,INV-K-A 据此区分全局路由键与面板内键。
5. **守护机制**:`tests/dual_source_invariant_test.rs` 三条不变量常绿守护;
   任何未来双源漂移在 CI 即时暴露。

## 备选与否决

| 备选 | 否决理由 |
|---|---|
| 维持双表人工同步 | INV-F/K-B 实锤证明人工同步不可靠;漂移静默无感知 |
| 运行时宏生成路由表 | 调试困难、错误信息差;编译期常量表 + 派生函数已足够 |
| FocusManager 独立静态序 | 正是 P5① 病理本身;必须与注册序同源 |

## 后果(Consequences)

- **行为变更(有意)**:焦点环新增 Timeline/Sysinfo(§7.3 接线清单);
  OsaSparse/ClvVector 相对顺序以注册序为准;g5 由"未注册提示"变为有效跳转。
- **性能**:路由查表为 ~8 条线性扫描,按键事件频率远低于帧预算,零可测影响;
  `next()/prev()` 改为 O(25) position 查找(原 O(1) match),Tab 切换路径
  增量 <1µs,无 SLO 风险。
- **后续波次衔接**:W2 斜杠命令翻转(SlashCommandSurface)直接消费
  `SlashCommandRegistry` + codegen 通道,四源合一扩展为五源(命令/键位/
  焦点/帮助/补全)无需再动路由骨架。

## 验证记录

- INV-F / INV-K-A / INV-K-B 全绿(首测红证据见进度报告 T1.2 节);
- chimera-tui 全量 1244 passed / 0 failed(2026-08-10);
- `scripts/check_dependency_rules.ps1` EXIT=0(38/38 crate)。
