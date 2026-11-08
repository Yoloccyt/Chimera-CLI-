# ADR-257: nexus-subagent 装配接线（P2 / U-02）

- **状态**：Accepted（2026-11-08，Tranche 2 工单 E；deepdive §7 规格照单执行）
- **上游**：deepdive §7（D1 案 β / D2 并发包装 / D3 最小接线 / D4 双委托面不合并 / D5 契约头）
- **关联**：ADR-148（SubAgent 类型化 + Task Auction）/ ADR-211（mas-sched 委托面）/ ADR-244（组合根装配纪律）

## 1. 背景

`SubAgentQuestEngine`（L10 适配器）自陈 GATED-UNWIRED：API 已备、组合根零
消费。装配障碍 = `SubAgentQuestEngine::new(engine: QuestEngine)` 消费
owned，而 `AppContext.engine` 同为 owned——同一实例无法两处拥有，且
`QuestEngine` 未实现 Clone。

## 2. 决策

- **D1 · 案 β 采纳（QuestEngine 浅 Clone）**：`#[derive(Clone)]`——深审
  「全字段 Clone」前提经核验**部分不成立**（checkpoint_manager/ttg_governor/
  metrics 三字段类型无 Clone，深审枚举遗漏），落地为**三类型 Arc 内化**：
  `TtgGovernor{modes,last_budget_switch→Arc<Mutex>,arbitration→Option<Arc>}`、
  `CoordinationMetricsCollector{state→Arc<Mutex>}`、`CheckpointManager`
  derive Clone（PathBuf+usize 值快照,磁盘共享真源）。语义 = Clone 产生同一
  引擎状态的两个视图（Quest 注册表/TTG 模式态/度量态/仲裁层订阅全部共享）；
  `config` 为值快照。三类型**无字段赋值更新点**（grep 实证）⇒ Arc 内化零
  调用点破坏。案 α（适配器收 Arc）推导后仍卡 Clone——不成立；案 γ
  （AppContext.engine→Arc）波及全部命令面借用点——否决（维持）。
- **D2 · 并发包装 `Arc<tokio::sync::Mutex<SubAgentQuestEngine>>`**：
  `spawn_quest`/`join_next` 均收 `&mut self`（JoinSet 所有权语义）——
  tokio 异步锁允许跨 await 持有（AGENTS §4.4-1 禁令针对 std 同步锁,语义
  澄清）;**代价声明**：同一实例 spawn/join 串行化（与 JoinSet &mut 同构,
  可接受）。驱动形态（第一波）= 无常驻驱动任务,单消费者锁内循环留命令面
  波次;JoinSet→mpsc 的 L7 无锁改造**维持否决**。
- **D3 · 最小接线**：第一波仅装配 + 句柄暴露（`AppContext.subagent_engine`
  非 Option 字段）。**诚实登记：Quest 分解自动委托机制不存在**（生产码
  delegate/AgentTaskDelegated 0 命中实证）——不虚构委托;驱动入口
  `lock().await.spawn_quest(spec, goal, from_task)` 留命令面波次。
- **D4 · 双委托面不合并**：chimera-mas `RootOrchestrator::delegate`（深度 ≤5
  递归委托,AgentTask* 三变体现有发布方）与 nexus-subagent（Quest 型长任务
  派发,禁嵌套深度 1）**不互替**;事件共用三变体、按 from/to 归属区分
  （本适配器 from=`subagent-quest-engine`/to=task_id）。
- **D4b · 事件闭环**：适配器增 `Option<EventBus>` + `with_event_bus`
  builder——spawn（同步方法）经 `publish_blocking` 发 `AgentTaskDelegated`
  （from=引擎 ID/to=task_id/deadline=now+1h/priority=Medium）;join_next 发
  `AgentTaskCompleted`（from=task_id）/`AgentTaskFailed`（Critical,mpsc 旁路
  保证投递;retry_count=0）。发布失败仅 warn 不传染派发。§7.4 事件闭环
  TDD 行以此为前提（纯装配无总线无法发布——规格内部张力的落地裁决）。
- **D5 · 契约头同步**：subagent_engine.rs `GATED-UNWIRED → WIRED`（两处）;
  nexus-subagent CONSUMERS 计数不变（仍 1=nexus-app-server）。
- **block_on 审计项（spawn_blocking 审计唯一候选）登记暂缓**：
  subagent_engine.rs:92-106 的 `Handle::block_on` 驱动 async create_quest
  处于 **SubAgentTask 同步闭包**内——FnOnce(Spec,Token)->Result 契约属 L7
  runtime 原语,清理需改 SubAgentTask 为 async 闭包（L7 改动,deepdive D2
  明文本轮否决）。登记为独立工单（与 JoinSet→mpsc 同批评估）。

## 3. 实施

| 件 | 位置 |
| --- | --- |
| 三类型 Clone 化（Arc 内化 + derive） | quest-engine/src/{ttg.rs,coordination_metrics.rs,checkpoint.rs} |
| `#[derive(Clone)]` + 浅共享语义 doc | quest-engine/src/engine.rs |
| 装配（engine.clone() + with_event_bus）+ AppContext 字段 | composition.rs（:285 后 / AppContext / Ok 构造） |
| 契约头 WIRED | nexus-app-server/src/subagent_engine.rs（两处） |
| 集成 + proptest 矩阵 | chimera-cli/tests/subagent_wiring.rs（新建;dev-dep +nexus-subagent 类型面） |

## 4. 验收（deepdive §7.7 命令套件全绿，2026-11-08）

- quest-engine lib **254/0**（含 `quest_engine_clone_shares_state` 浅共享
  语义锁:两视图互见对方 Quest + list_quests 规模一致）
- nexus-app-server 既有适配器 4 测试回归绿（owned 签名未变）
- chimera-cli subagent_wiring **7/7**：装配可用性（spawned_total==0）/
  spawn→join 闭环（quest- 前缀）/ **SWARM_LIMIT=8 确定性 enforcement**
  （JoinSet::len 含未 join 项 ⇒ 8 spawn 后第 9 必拒）/ 事件闭环
  （Delegated+Completed 订阅到达且 task_id 归属正确）/ 零旁路反向断言 /
  prop_spawn_never_nests + prop_swarm_capacity_bound（各 64-case）
- composition root [OK] / reachability **38 可达 + new_gaps=0** /
  clippy -D warnings 0 告警 / fmt 收敛

## 5. 回滚

单 commit revert 三点同消（AppContext 字段/composition 装配行/quest-engine
Clone impl + 三类型 Arc 内化随批）;`SubAgentQuestEngine::new` 还原 owned
（其 4 条自测不需改）;回退后 crate 回 GATED-UNWIRED 诚实态。
