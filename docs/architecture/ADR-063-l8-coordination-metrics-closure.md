# ADR-063: L8 协调度量接线闭环 + 推理悖论红线策略封顶

> **状态**: Accepted (2026-07-31)
> **决策日期**: 2026-07-31
> **关联 ADR**: ADR-031(learner 边界)/ ADR-026(chimera-mas 子系统)
> **关联分析**: P2-1(三重悖论推理悖论红线 — 协调成本/推理增益比值度量)、
> `AI_Agent_三重悖论_x_Chimera_深度映射分析.md` 推理悖论红线
> **权威源**: 本 ADR 记录三指标采集接线闭环与 ratio 反馈策略封顶的落地方式

---

## 1. 背景

`quest-engine/src/coordination_metrics.rs` 的度量类型体系与下游
EWMA → `CoordinationRatioReported` → efficiency-monitor 告警链路完整,
但三个关键指标(`parliament_debate_latency_ms` / `delegation_overhead_ms` /
`consensus_quality`)在生产路径**从未被填充**(builder 仅在测试中调用):

- Parliament(debate.rs)无计时埋点;`VoteCounter::count_votes` 产出的
  `weighted_approval_rate` 被丢弃,不进事件不出 crate
- chimera-mas `execute_single_task` 有单任务 duration,但批次不聚合、不上报
- `complete_quest` 只填微秒级 EventBus 延迟(TTG 传 0),ratio 几乎永不越过
  `paradox_threshold=1.0`,推理悖论告警形同虚设(测试注释承认须手动注入)

推理悖论红线要求"定期测量协调成本/推理增益比值"并具备风险控制回路,
本决策补齐采集端接线并新增事后反馈风控。

## 2. 决策

### 决策 1:事件契约 — 新增 Normal 级观测事件,不扩容 Critical 事件

新增 3 个 `NexusEvent` 变体(均 Normal 级,走 broadcast 通配符):

| 事件 | 发布方 | 关键字段 |
|---|---|---|
| `DebateCompleted` | parliament(L8) | quest_id / debate_latency_ms / strategy / weighted_approval_rate(Option) / participation_rate(Option) / outcome |
| `DelegationCompleted` | chimera-mas(L9) | parent_id / quest_id(Option) / total_overhead_ms / sub_task_count / success_count |
| `ParliamentStrategyCapChanged` | parliament StrategyCapGuard | old_cap / new_cap / ratio / threshold |

WHY 不扩容 Critical 级 `ConsensusReached`:其走 mpsc 旁路且被
GSOE/AutoDPO/SecCore 多方消费,扩字段回归面大;观测数据与治理决策事件
分离更干净("事件是事实,告警是解释"原则,同 `CoordinationRatioReported`)。

### 决策 2:采集埋点口径

- **审议延迟**:`deliberate_with_policy` 入口到共识返回的 wall-clock
  (含 Skeptic 检测 + Opinion 收集 + 投票 + 事件发布),全路径上报
  (Reached/Rejected/Vetoed 含前置否决短路)
- **委托开销**:`execute_delegation` / `execute_batch_delegation` 批次
  wall-clock(**非**各子任务 duration 求和——并行执行求和会重复计费);
  `AgentTask` 新增 `#[serde(default)] quest_id: Option<String>` + `with_quest()`
  builder 承载归因(切块继承)
- **共识质量口径**:`consensus_quality = weighted_approval_rate`
  (共识置信度 proxy,**非**决策正确率 ground truth;真实"决策正确率复盘"
  留待未来 GSOE 反馈闭环,代码注释已声明诚实边界)
- **TTG 切换延迟**:`switch_thinking_mode` / `ttg_auto_select` 计时累加
  (修复 complete_quest 硬编码 0.0 的缺口)

### 决策 3:quest-engine 订阅合并(尽力合并语义)

`metrics_sync` 模块按 quest_id 维护 `PendingCoordSample` 缓存
(审议取最近值 / 委托累加 / 质量取最近 Some / TTG 累加);
`complete_quest` take 后经既有 builder 填充,`cancel_quest` 清理防泄漏。
事件丢失/时序错过时字段保持 None(与 Option 设计一致,不阻塞不报错)。
依赖铁律合规:L8/L9 互不直接依赖,接线只走 event-bus。

### 决策 4:StrategyCapGuard — ratio 反馈策略封顶(与 LinUCB 互补)

- 滞后带状态机:ratio > threshold 连续 3 次 → 封顶降一档
  (Full→Simplified→FastPath);ratio < 0.8×threshold 连续 5 次
  且驻留 ≥ 30s → 升一档;带内双计数器清零(防抖振)
- `deliberate` 生效策略 = min(学习策略, 封顶) — 封顶仅做上界,
  **不改写** omega-learner 输出(S5 接缝是事前选择,封顶是事后反馈,互补)
- **安全不变量**:Skeptic 否决检查在任何封顶档位照常执行(红队防线
  不可被风控降级绕过,proptest + 集成测试双重守护)
- 配置入 `ParliamentConfig.strategy_cap`(`#[serde(default)]` 向后兼容)

## 3. 否决方案

| 方案 | 否决理由 |
|---|---|
| 投票 quorum 短路/数学上界提前结束收集 | 改变共识语义且需 Skeptic 特判;Opinion 生成目前是 stub,短路只省微秒级编排开销,收益不可证伪 |
| 审议结果 TTL 缓存/记忆化 | 陈旧共识风险(内容哈希相同但系统状态已变);否决/拒绝不可缓存使收益进一步缩水 |
| 扩容 Critical 级 ConsensusReached | mpsc 旁路 + 多方消费,回归面大于新增 Normal 观测事件 |
| `deliberate` 返回类型改为 DebateResult | 公开 API 破坏性变更需 major 版本;事件携带零破坏送达同样数据 |
| consensus_quality 推迟到下阶段 | weighted_approval_rate 在审议路径内唾手可得,proxy 口径立即闭环成本极低 |

## 4. 验收标准

1. 三指标生产接线单测/集成测试全覆盖(审议三策略 + 否决路径 / 委托
   成功 + 失败 + 空批次 + 批量路径 / 合并 + 取消清理)
2. E2E `coordination_metrics_closure_e2e`:真实审议 + 委托 →
   `CoordinationRatioReported` 携带真实成本 → efficiency-monitor 真实触发
   `paradox-risk-coordination-ratio` 告警(不再手动注入);覆盖正常/
   无审议/Vetoed 三分支
3. StrategyCapGuard:滞后带状态机全转换路径单测 + proptest
   (封顶单调有界/一次一档/min 上界)+ Skeptic 不变量集成测试
4. criterion 基线:parliament 三档策略(fastpath/simplified/veto_path)+
   chimera-mas delegation fan-out(1/4/16),作为埋点开销回归对照
   (<5% 红线)与封顶降档收益证据
