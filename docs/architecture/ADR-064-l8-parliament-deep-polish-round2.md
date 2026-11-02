# ADR-064: L8 Parliament 深度优化第二轮 — 算法级打磨与多维共识质量

> **状态**: Accepted (2026-07-31)
> **决策日期**: 2026-07-31
> **关联 ADR**: ADR-063(协调度量接线闭环,本轮的直接前置)
> **关联分析**: 推理悖论红线深化(测量盲区/多维质量/可证伪延迟优化)
> **权威源**: 本 ADR 记录 override 度量盲区修复、publish_batch 原语、
> Arc 共享优化、多维共识质量与两条风控线边界的落地方式

---

## 1. 背景

ADR-063 建成协调度量接线闭环后,三视角并行探源确认四处真实增量:

1. **override 度量盲区**:`deliberate_with_override` / `reopen_veto`(覆议复审)
   无计时、无 DebateCompleted 发布——绕过 Skeptic 防线的高风险复审路径
   反而不进协调度量
2. **事件发布固定开销**:经核实 `EventBus::publish` 内部纯同步
   (`broadcast::Sender::send` 无 await 点,bus.rs `#[allow(clippy::unused_async)]`),
   并发化发布收益为零;真实开销是每次 publish 重复的
   `receiver_count()`/`sender.len()` 背压采样
3. **O(R×T) clone**:`collect_opinions_filtered` 每角色深拷贝整个 Quest
   (含全部 Task Vec),Full=5 角色 × T 任务
4. **共识质量单一 proxy**:consensus_quality 仅 weighted_approval_rate,
   而 `compute_weighted_approval` 的 `non_abstain_weight_sum` 被丢弃,
   position/confidence 分布未利用

## 2. 决策

### 决策 1:override 路径补齐度量(strategy="full-override")

`deliberate_with_override` 入口 Instant 计时,三条返回路径
(无票据否决短路 / override 生效后达成 / override 后仍拒绝)均发布
DebateCompleted;strategy 标签 `"full-override"` 与常规路径区分,
避免混淆 EWMA 统计。复用既有 `publish_debate_completed_event`,零新 API。

### 决策 2:publish_batch 原语(证伪门禁通过)

event-bus 新增 `publish_batch` / `publish_batch_blocking`:
一次 `receiver_count()` + 一次背压采样,逐条保留 Critical 无订阅者告警、
`is_critical_mpsc_event` mpsc 旁路、lag 检测(语义与 `publish` 完全对齐);
空 Vec 早退。`publish_vote_events` 改为构造 `Vec<VoteCast>` 后单次 batch。

**证伪门禁实测**(criterion `publish_batch_vs_serial`,带订阅者并排对照):

| N | serial | batch | 收益 |
|---|--------|-------|------|
| 5 | 1.682 µs | 1.408 µs | **-16.3%** |
| 10 | 3.272 µs | 2.603 µs | **-20.4%** |

超过 5% 门禁阈值,采纳成立。

**顺序安全性**(已核实):VoteCast 下游无相对顺序依赖(TUI 仅倒序展示、
immune_system 不消费 VoteCast);唯一契约"全部 VoteCast 先于 ConsensusReached"
由调用点时序边界天然保证。

### 决策 3:collect_opinions_filtered Arc 共享

每角色 `quest.clone()`/`proposal.clone()` 改为一次深拷贝 + 每 future
`Arc::clone`(refcount),O(R×T) → O(T);`generate_opinion` 签名不变
(deref 强制),收集后计票语义零改动。

**证据口径(诚实记录)**:严格并排基线在 M0 建立时本轮改造已同批进行,
Arc 单项无独立前后对照;跨 run 参考(同基准 v2.10.0 → 本轮):
`deliberate_complex`(7 任务)8.21→5.41 µs(-34%),任务数越大收益越显著,
与 O(R×T)→O(T) 理论方向一致。`deliberate_full_50tasks`(9.46 µs)作为
未来回归基线归档。

### 决策 4:多维共识质量(ConsensusQualityMetrics)

`voting.rs` 新增 `ConsensusQualityMetrics { approval_rate, abstention_rate,
divergence, consensus_margin, skeptic_stance }`,在计票同一次遍历内派生
(零额外渐进成本):
- `divergence` = 加权 position 方差 / 0.25(Popoviciu 上界归一化):
  全体一致=0,半赞成半反对=1
- `abstention_rate` = 弃权权重和 / 总投票权重和
- `consensus_margin` = approval_rate − consensus_threshold(共识牢固程度)
- `skeptic_stance` = Skeptic position(红队立场单独暴露;无意见默认 0.5)

`VoteResult` 加 `quality` 字段(仅 crate 内部构造,零实际破坏);
`DebateCompleted` 追加 `#[serde(default)]` 的 divergence/abstention_rate/
consensus_margin Option 字段(旧数据反序列化兼容);quest-engine
`PendingCoordSample` 按"取最近 Some"合并,**仅供观测**——
InferenceGainSample 主通道仍以 weighted_approval_rate 为 proxy(口径不变)。

**诚实边界**:全部指标基于规则 stub 的 position/confidence 派生,
是共识"结构"的 proxy 而非决策正确率 ground truth(留待 GSOE 反馈闭环)。

### 决策 5:悖论风险模型形式化 + 两条风控线边界(不新建控制器)

**风险模型 = 三信号,全部映射到既有实现**:

| 信号 | 实现 | 角色 |
|------|------|------|
| ratio 越阈幅度 | CoordinationRatioReported + StrategyCapGuard 滞后带 | 触发条件 + 缓解(降档封顶) |
| Skeptic 否决异常率 | immune_system ReasoningTrapProbe 滑动窗口 | 风险评估 |
| 级联风险 | `compute_cascade_risk`(0.5×max_paradox + 0.3×circuit_open + 0.2×budget) | 实时预警(→ efficiency-monitor 告警) |

**成本-收益分析模型** = CoordinationCostSample 四分量 /
InferenceGainSample 三分量加权(0.5/0.3/0.2 缺失重归一)/
CoordinationToGainRatio(EWMA),已由 ADR-063 落地;本轮补 proptest
模拟验证(任意样本序列下 EWMA 自洽 + 恒定输入收敛)。

**观测联动(observation-not-control)**:StabilityMirror 新增
`ParliamentStrategyCapChanged` 滑动窗口(封顶升降档振荡本身是悖论风险
佐证信号),**仅计入观测窗口,不触发任何降档/膜厚控制动作**——
避免封顶线(StrategyCapGuard)与膜厚线(ImmuneSystem)双控制器振荡。
单测验证零控制副作用(degradation_level/断路器/否决窗口不变)。

### 决策 6:委托执行内核去重复

`execute_delegation` / `execute_batch_delegation` 主体高度重复
(仅 agent_id 中缀 `::sub::`/`::batch::` 与日志文案差异),抽为私有
`run_delegation_batch(parent_id, tasks, agent_id_infix, err_label)`;
公开 API 签名与行为完全不变(delegation_test 20 项回归守护)。

**委托开销派生口径**:开销评估完全基于既有 DelegationCompleted
{total_overhead_ms, sub_task_count, success_count} 在消费端派生
(per-task 均摊 = total/count、成功率 = success/count),零生产采集变更。

## 3. 否决方案

| 方案 | 否决理由 |
|---|---|
| FuturesUnordered/join_all 并发发布 VoteCast | 已验证 publish 内部纯同步(send 无 await 点),future 永不挂起,并发化收益为零 |
| quorum 短路 / 审议 TTL 缓存 | ADR-063 §3 已否决,维持(stub 下收益不可证伪 + 改变共识语义) |
| 新建成本-收益/风险控制器 | 既有三组件(ratio 度量/封顶/免疫探针)已构成闭环,新建属重复造轮子 |
| StrategyCapGuard 连续/梯度调节 | 受 L0 ActivationStrategy 三档枚举约束,细粒度化需改核心契约,回归面大 |
| 改 VoteResult/deliberate 公开签名传质量 | API 破坏需 major;DebateCompleted Option 字段零破坏送达 |
| 两条风控线控制联动(封顶↔膜厚互相触发) | 双重降档振荡风险;仅保留观测联动(决策 5) |
| 委托 per-task 开销分解模型 | wall-clock + 消费端派生已足够,分解属过度工程 |

## 4. 验收标准(实测结果)

1. **override 盲区**:三路径(达成/无票据否决/override 后计票)单测全绿,
   DebateCompleted 携带 latency + "full-override" 标签
2. **publish_batch**:摊销收益 -16%/-20%(N=5/10,超 5% 门禁);
   含 Critical 事件批量发布 mpsc 旁路不破坏(单测);VoteCast 计数完整性不变
3. **多维质量**:单测 ×4(全一致/全弃权/对立/无 Skeptic)+ proptest 区间
   有界;E2E 三场景(高分歧/全一致/高弃权)真实审议数据流入 quest-engine 缓存
4. **属性模拟**:StrategyCapGuard 滞后带内永不变更 + 持续越阈恰降两档饱和;
   Collector 任意序列 EWMA 自洽 + 恒定输入收敛
5. **基准归档**:带订阅者基准(full 6.86µs / simplified 6.11µs)修复
   无订阅者低估盲区;50tasks(9.46µs)与 delegation zero-delay 作为回归基线
