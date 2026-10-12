# ADR-183: TUI 经验卡片面板接线 + InjectionStrategy 下线

> **状态**: Accepted(2026-09-06,tag 未推,工作区在途批次)
> **来源**: 评估报告 `docs/reports/tui-four-dimension-eval-2026-09-06.md` §8 Top-2(FC-05)
> **变更目录**: `evaluate-tui-four-dimension`(.trae/specs/)

## 背景
TUI `ExperienceCardViz` 与 `InjectionStrategy` 两面板自 Phase 10 §15.2b/§15.3 注册以来,
生产态**从未注入数据提供者**(`make_panel` 以 `::new()` 构造,`with_provider` 全仓仅测试调用),
恒显 "Awaiting stats provider..."。评估判定为「有界面入口、无数据出口」的功能残缺(FC-05,高)。

## 勘察结论(代码证据)
- `ExperienceCardViz` 数据源**运行期真实存在**:chimera-cli 组合根经
  `spawn_experience_loop` 构造 `ExperienceCardBus`(experience_loop.rs:130)并装配
  `MlcEngine::with_card_bus`(experience_loop.rs:182-186,订阅总线消费卡片进
  `card_system`);`ExperienceLoopHandles` 暴露 `card_bus` 与 `mlc` 句柄(tui.rs:207 取得)。
  L2 `MlcEngine` 已内置 `ExperienceCardSystem`(engine.rs:151),含
  `global_board()/method_stats()` 全量统计。
- `InjectionStrategy` 数据源(mlc-engine 注入策略)**生产从未构造**:rg 全仓
  `ExperienceCardSystem::new` 仅测试代码命中;runtime 无任何注入快照产生
  (InjectionSnapshot 无从聚合)。接线 = "为面板而造系统",属循环论证。

## 决策
1. **接线 ExperienceCardViz**(运行期真实数据):
   - chimera-tui(L10):`TuiApp::install_experience_card_provider(Arc<dyn ExperienceCardStatsProvider>)`
     运行期替换面板实例 —— 数据源在组合根异步装配后才就绪,故不走构造期注入;
     未接线/失败时维持 `::new()` 诚实提示(降级不阻断)。
   - mlc-engine(L2):新增 `MlcEngine::card_system_view() -> CardSystemStatsView`
     (全局板 + 方法分布自足快照),不改变既有 `card_system_snapshot`。
   - chimera-cli(L10 组合根):`MlcCardStatsProvider` 适配器(dependency-倒置,
     trait 在 chimera-tui,实现驻留组合根);经验闭环装配成功后注入
     `handles.mlc` 的统计视图,失败降级为未接线(诚实提示)。
2. **下线 InjectionStrategy**(无数据源,避免恒空占位):
   - 从 `REGISTERED_FOCUS_ORDER` 移除(未注册变体回退环首/环尾语义,types.rs:259-271);
   - 面板代码与其测试**保留**(trait 注入桩可复测,未来数据源成熟可重新注册);
   - 受影响的环断言/焦点往返/tab_width 数学同步更新(four-file 同步纪律)。

## 影响
- **依赖方向**:chimera-cli→mlc-engine/chimera-tui 均为 L10→L2/L10 向下合规(§2.2);
  chimera-tui 不新增 L2+ 依赖(trait 注入,D-1 先例不变)。
- **行为**:ExperienceCardViz 面板生产态由恒空 → 真实统计(卡片数/已评估/错误聚类/方法分布/分数);
  InjectionStrategy 不再参与 Tab 环(26 面板),不可再被聚焦。
- **可达性**:mlc-engine 本就经 chimera-cli 依赖可达,未改变孤岛清单。
- **性能**:`card_system_view` 锁内纯映射(短临界区,不跨 await,§4.4-1 合规),每帧仅面板渲染时调用。

## 验证
- chimera-tui lib 863/863 绿 + integration 32/32 + mouse_interaction 8/8 + injection_strategy 8/8
  + experience_card_viz 7/7 + event_stream 15/15;
- `card_system_view` 空/进卡两用例(mlc-engine 集成测试,编译通过;执行受 D 盘 0B 磁盘约束,
  完整回归由 CI clean-runner 承担);
- clippy(-D warnings,chimera-tui/mlc-engine/chimera-cli)+ `cargo fmt --check` 全绿。