# ADR-043: R1(召回配额 CQL/IQL)影子模式设计 — 2 周解冻前置

## 状态

已批准 (Accepted) (2026-07-25)

> **状态说明**: 本 ADR 于 2026-07-25 由 P4-W16.2.4 子任务创建并批准。本 ADR 是 v5.0 设计文档 §7.5 "影子模式 2 周为 R1 解冻前置"的工程实施层落地,定义 R1(召回配额 CQL/IQL)影子模式的开关机制、对比报告、解冻条件与回滚预案。属 append-only 扩展(对齐 ADR-028 决策 1 哲学),不修改既有 ADR-037(CapabilityToken 四态)或 ADR-042(R2 冻结)的裁决结论。

## 背景

- **v5.0 设计文档 §7.5 经验回放池** 明确指出:"离线 RL 两接缝(R1 召回配额 CQL/IQL、R2 GSOE×AutoDPO 约束 RL)维持 v4.0 设计,R2 在 FormalVerifier 落地前无条件冻结;回放池 ≥10K 轨迹 + **影子模式 2 周为 R1 解冻前置**。"
- **风险矩阵 R3**(P4_P5_IMPLEMENTATION_PLAN.md 第 288 行):"离线 RL 轨迹选择偏差 | 中概率 | 中影响 | 探索噪声 + 影子对比 | 影子 < 静态基线 → R1 回炉"。
- **v5.0 设计文档 §7.5 风险矩阵**(line 613):"离线 RL 轨迹选择偏差 | 中 | 中 | 探索噪声 + 影子对比 | 影子 < 静态基线 → R1 回炉"。
- **ADR-037 决策 2**(CapabilityToken 四态):`CapabilityTokenStatus::Provisional` 状态(level < 0.3 阈值,不允许 Learned 策略生效)天然承载影子模式语义——R1 训练策略已生成但未授权部署。
- **ADR-042 决策 1**(R2 冻结范围):R2 路径完全禁用,R1 路径不在冻结范围,但需满足影子模式 2 周前置。
- **当前代码基线**(v2.3.1-omega → v2.4.0-omega 实际发布,2026-07-26 核实):
  - [crates/omega-learner/src/replay_pool.rs](file:///D:/Chimera CLI/crates/omega-learner/src/replay_pool.rs) 已实现 `ReplayPool<T>`(P4-W16.2.1,34 个单元测试全绿) — R1 训练的轨迹来源
  - [crates/decay-engine/src/capability_registry.rs](file:///D:/Chimera CLI/crates/decay-engine/src/capability_registry.rs) 已实现 `CapabilityTokenRegistry`(ADR-037 决策 4) — R1 影子模式开关的承载位置
  - [crates/nexus-contracts/src/capability_token.rs](file:///D:/Chimera CLI/crates/nexus-contracts/src/capability_token.rs) 已定义 `CapabilityTokenStatus::Provisional`(ADR-037 决策 2) — R1 影子模式状态
  - **R1 算法(CQL/IQL)尚未实现**:35 crate × 624 .rs 文件零命中 `CQL` / `IQL` / `ConservativeQL` / `ImplicitQL`,P4-W16.2.2 待实现
- 经 E01 首席架构师 + E05 生产系统专家 + E07 任务调度专家分布式深度分析与多轮交叉验证,确认 R1 影子模式可通过 **复用 CapabilityToken::Provisional 状态 + 新增对比报告类型** 落地,无需新建 crate、无需修改核心领域类型、无需引入 unsafe 依赖。
- 本 ADR 记录 R1 影子模式的 5 项工程实施决策,作为 P4-W16.2.4 验收依据与 P4-W16.2.2 R1 算法实施的前置约束。

> **ADR 编号确认**: `docs/architecture/adr_index.md` 现有最大编号为 ADR-042(2026-07-25 核实,本 ADR 创建前)。本 ADR 编号确认为 **ADR-043**,作为下一个连续编号,与既有规划不冲突。

> **与 ADR-037 的关系**: 本 ADR 是 ADR-037 决策 2(CapabilityToken 四态)的**应用场景具体化**,不修改 ADR-037 的四态定义与状态转换规则,仅明确 R1 影子模式如何复用 `Provisional` 状态。属 append-only 扩展。

> **与 ADR-042 的关系**: 本 ADR 是 ADR-042 决策 1(R2 冻结范围,澄清 R1 不在冻结范围)的**互补决策**——R1 虽不冻结,但需满足影子模式 2 周前置。两份 ADR 共同构成"离线 RL 两接缝解冻条件"的完整定义。

## 决策

经专家团队多轮结构化思考与多路径交叉验证,对 R1(召回配额 CQL/IQL)影子模式作出以下 5 项工程实施决策:

### 决策 1: 影子模式开关机制 — 复用 CapabilityToken::Provisional 状态

R1 影子模式的开关机制**复用 ADR-037 决策 2 的 `CapabilityTokenStatus::Provisional` 状态**,不新建独立开关类型。

**复用关系映射**:

| 影子模式概念 | CapabilityToken 承载 | 说明 |
|------------|---------------------|------|
| 影子模式激活 | `status = Provisional` | R1 训练策略已生成但未达 ACTIVATION_THRESHOLD(0.3),不允许 Learned 策略生效 |
| 影子模式退出(解冻) | `status = Authorized`(`maybe_promote()` 成功) | EWMA ≥ 0.7 + 2 周观察期 + 无 ASA → 自动提升为 Authorized,R1 策略正式生效 |
| 影子模式期间 R1 策略不部署 | `allows_learned() = false` | 编排器查询 token → 未授权 → `fallback_to_static()`(C4 合规第三层) |
| 影子模式期间 R1 策略记录 | `level` 字段持续更新 | R1 训练策略通过 `record_outcome()` 更新 EWMA,level 渐进上升 |
| 影子模式异常中断 | `trigger_asa_intervention()` | AsaIntervention 触发 → status = Cooldown,60s 冷却后恢复 Provisional 或连续 3 次 → Frozen |

**影子模式开关的具体接入路径**:

```text
R1 训练(omega-learner replay_pool.sample() → CQL/IQL 算法)
  │
  ▼ 生成 R1 训练策略(Learned)
  │
omega-learner 调用 decay_engine.capability_registry.register_token(seam=S7_RecallQuota, level=0.2)
  │
  ▼ CapabilityToken { status: Provisional, level: 0.2 }
  │
编排器(chimera-cli / quest-engine)查询 token:
  ├─ should_activate_learned(seam=S7) → false (Provisional)
  ├─ holder.fallback_to_static()  ← R1 策略不部署,仅记录
  └─ 同时:R1 训练策略通过 record_outcome(reward) 更新 EWMA → level 渐进上升
  │
  ▼ 2 周观察期 + EWMA ≥ 0.7 + 无 ASA
  │
maybe_promote() → status = Authorized
  │
  ▼ 编排器查询 token:
  ├─ should_activate_learned(seam=S7) → true (Authorized)
  └─ holder.update_policy(Learned(R1_policy))  ← R1 策略正式生效
```

**新增 SeamId 变体**:为承载 R1 离线 RL 接缝,扩展 `omega_learner::SeamId` 与 `nexus_contracts::SeamId` 枚举新增 `S7RecallQuota` 变体(R1 接缝)。

> **与 ADR-037 决策 2 的关系**:本决策复用 `Provisional` 状态,不修改四态定义。`Provisional` 在 ADR-037 中已定义为"未达 ACTIVATION_THRESHOLD(0.3),不允许 Learned",这与影子模式"策略已生成但不部署"的语义完全一致。

### 决策 2: R1 训练策略的存储与对比 — 新增 ShadowComparisonReport 类型

影子模式期间需记录 R1 训练策略与 L3 主路径策略的对比,新增 `ShadowComparisonReport` 类型承载对比数据。

**类型定义**(拟落地于 `crates/omega-learner/src/shadow_mode.rs`):

```rust
/// 影子模式对比报告 — R1 训练策略 vs L3 主路径策略的每日对比快照
///
/// 对应架构层: L6 Router(omega-learner)
/// 对应 ADR-043 决策 2
///
/// # 设计原则
/// - **纯数据类型**:无方法,仅承载对比快照,便于序列化与持久化
/// - **可序列化**:派生 `Serialize + Deserialize`,支持写入审计日志
/// - **时间证据**:包含 `report_date`,便于追踪 2 周观察期内的对比趋势
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ShadowComparisonReport {
    /// 报告日期(UTC,每日生成一份)
    pub report_date: chrono::DateTime<chrono::Utc>,
    /// 所属接缝(固定为 S7RecallQuota,但保留字段便于扩展)
    pub seam: SeamId,
    /// R1 训练策略的指标快照
    pub r1_metrics: StrategyMetrics,
    /// L3 主路径策略的指标快照(基线)
    pub l3_baseline_metrics: StrategyMetrics,
    /// 对比结论(R1 是否优于 L3 基线)
    pub comparison: ComparisonResult,
    /// 观察期剩余天数(2 周 = 14 天,每日递减)
    pub remaining_days: u16,
}

/// 策略指标快照 — R1 与 L3 通用指标容器
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct StrategyMetrics {
    /// 召回率 [0.0, 1.0](R1 接缝的核心指标)
    pub recall_rate: f32,
    /// 误杀率 [0.0, 1.0](R1 接缝的反指标,越低越好)
    pub false_block_rate: f32,
    /// 延迟惩罚 [0.0, 1.0](归一化,R1 接缝的代价指标)
    pub latency_penalty: f32,
    /// 综合得分 [-0.5, 1.0](recall_rate - 0.5 * false_block_rate - 0.3 * latency_penalty)
    pub composite_score: f32,
    /// 样本数(当日 R1 训练的轨迹数)
    pub sample_count: u64,
}

/// 对比结论 — R1 是否优于 L3 基线
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum ComparisonResult {
    /// R1 显著优于 L3(composite_score 差值 ≥ +0.1)
    R1SignificantlyBetter,
    /// R1 略优于 L3(composite_score 差值 ∈ [+0.02, +0.1))
    R1SlightlyBetter,
    /// R1 与 L3 持平(composite_score 差值 ∈ [-0.02, +0.02))
    Tied,
    /// R1 略差于 L3(composite_score 差值 ∈ [-0.1, -0.02))
    R1SlightlyWorse,
    /// R1 显著差于 L3(composite_score 差值 ≤ -0.1)
    R1SignificantlyWorse,
}

impl ComparisonResult {
    /// 判定 R1 是否达到解冻门槛(2 周观察期内累计 R1 优于 L3 的天数比例)
    ///
    /// 解冻条件(ADR-043 决策 3):14 天内 R1SignificantlyBetter 或 R1SlightlyBetter 的天数 ≥ 10 天(71.4%)
    pub fn r1_is_better(self) -> bool {
        matches!(self, Self::R1SignificantlyBetter | Self::R1SlightlyBetter)
    }
}
```

**对比报告生成机制**:

1. **每日生成**:编排器每日 UTC 00:00 触发 `ShadowComparisonReport::generate()`,从 ReplayPool 采样 R1 训练策略与 L3 主路径策略的指标
2. **持久化存储**:对比报告写入 `crates/omega-learner/data/shadow_reports/<date>.json`(开发期)/ 数据库(生产期,P5 阶段接入)
3. **告警触发**:若连续 3 天 `ComparisonResult::R1SignificantlyWorse`,发布 `NexusEvent::R1ShadowRegressionDetected` (Normal 级 broadcast),通知 E05 生产系统专家审查

**新增 NexusEvent 变体**:1 个
- `R1ShadowRegressionDetected { metadata, report_date, regression_streak }`(Normal 级 broadcast,决策 2 对比报告告警)

### 决策 3: 2 周解冻条件 — EWMA + 对比胜率 + 无 ASA + 评审通过

R1 影子模式解冻(从 Provisional 提升为 Authorized)必须**同时满足**以下 4 项条件:

**条件 1: EWMA 达标(自动,ADR-037 决策 1)**

- R1 训练策略的 EWMA ≥ `EWMA_PROMOTION_THRESHOLD`(0.7)
- EWMA 基于 `record_outcome(reward)` 持续更新,反映 R1 策略的真实表现
- 单次 EWMA ≥ 0.7 不足以解冻,需持续 2 周观察期(条件 3)

**条件 2: 对比胜率达标(决策 2 对比报告)**

- 14 天观察期内,`ComparisonResult::r1_is_better()` 为 true 的天数 ≥ 10 天(71.4%)
- 即 R1 训练策略在召回率/误杀率/延迟惩罚综合得分上优于 L3 主路径的天数比例 ≥ 71.4%
- 阈值 71.4% 的依据:统计学意义上,p < 0.05 的胜率阈值(单尾二项检验,14 天中 10 天以上胜率的概率,在 R1 无真实优势时约为 5.9%,接近 5% 显著性水平)

**条件 3: 2 周观察期(时间约束)**

- 影子模式必须持续至少 14 天(2 周),不可提前解冻
- 观察期从 R1 训练策略首次注入 CapabilityToken(Provisional 状态)开始计算
- 观察期内若 R1 训练策略被回滚(如发现退化),观察期重置

**条件 4: 无 AsaIntervention(安全约束)**

- 2 周观察期内,R1 接缝(S7RecallQuota)未触发 AsaIntervention 事件
- AsaIntervention 触发会自动将 token 状态置为 Cooldown(ADR-037 决策 3),观察期中断
- 连续 3 次 AsaIntervention → token 状态置为 Frozen,需手动 unfreeze() 重启观察期

**解冻评审流程**:

1. **自动检查**:编排器每日 UTC 00:00 检查 4 项条件是否全部满足
2. **评审触发**:4 项条件全部满足时,发布 `NexusEvent::R1ShadowPromotionReady` (Normal 级 broadcast),通知 E05 生产系统专家
3. **人工评审**:E05 生产系统专家 + E01 首席架构师 + E03 记忆系统专家三方书面批准
4. **解冻执行**:`decay_engine.capability_registry.maybe_promote(seam=S7)` → status = Authorized
5. **评审报告归档**:`docs/audit/r1_shadow_promotion_<date>.md`,记录 14 天对比数据、评审意见、解冻决策

**新增 NexusEvent 变体**:1 个
- `R1ShadowPromotionReady { metadata, report_date, win_rate, ewma_level }`(Normal 级 broadcast,决策 3 解冻评审触发)

### 决策 4: 影子模式回滚预案 — 自动回滚 + 告警 + 重置观察期

若影子模式期间发现 R1 训练策略退化(对比报告显示 R1 显著差于 L3),触发以下回滚预案:

**回滚触发条件**(任一满足):

1. **连续 3 天 R1 显著差于 L3**:决策 2 对比报告连续 3 天 `ComparisonResult::R1SignificantlyWorse`
2. **AsaIntervention 触发**:R1 接缝触发 AsaIntervention 事件(ADR-037 决策 3 自动置为 Cooldown)
3. **EWMA 崩塌**:R1 训练策略的 EWMA 在 24 小时内下降 ≥ 0.3(从 0.7 跌至 0.4 以下)
4. **召回率显著下降**:R1 接缝的召回率较 L3 基线下降 ≥ 5%(绝对值,如从 95% 降至 90%)

**回滚处置流程**(三步):

1. **自动回滚**(立即,~1 分钟):
   - `decay_engine.capability_registry.trigger_asa_intervention(seam=S7)` → status = Cooldown
   - `holder.fallback_to_static()` → 编排器回退到 L3 主路径策略
   - R1 训练策略不部署到生产路径(本就未部署,因 Provisional 状态)
2. **告警广播**(立即,~5 分钟):
   - 发布 `NexusEvent::R1ShadowRegressionDetected` Normal 级事件(决策 2)
   - 通知 E05 生产系统专家 + E03 记忆系统专家
   - 暂停 R1 训练(停止从 ReplayPool 采样)
3. **观察期重置**(24 小时内):
   - 观察期重置为 14 天(从头开始)
   - 输出回滚报告:`docs/audit/r1_shadow_rollback_<date>.md`
   - 评审 R1 算法是否需调整(CQL/IQL 超参数、奖励函数、探索噪声)
   - 评审通过后,R1 训练重启,观察期重新计算

**回滚失败的处置**:

- 若 `trigger_asa_intervention` 失败(如 CapabilityTokenRegistry 内部错误):
  - 发布 `NexusEvent::R1ShadowRollbackFailed` (Critical 级,走 mpsc 旁路通道)
  - 立即 `git revert` 最近一次 R1 算法相关 commit
  - 通知 E01 首席架构师 + E02 安全架构师介入

**新增 NexusEvent 变体**:1 个
- `R1ShadowRollbackFailed { metadata, reason }`(Critical 级,走 mpsc 旁路通道,决策 4 回滚失败)

**处置优先级**:P0(最高,阻断一切其他工作)—— R1 回滚失败等同于安全事件

### 决策 5: 工程实施层面的硬约束 — S7 接缝扩展 + 文档同步 + CI 检查

R1 影子模式在工程实施层面的硬约束:

**1. S7 接缝扩展**(SeamId 新增变体)

- `crates/nexus-contracts/src/seam.rs` 新增 `SeamId::S7RecallQuota` 变体(R1 接缝)
- `crates/omega-learner/src/seam.rs` 同步新增 `SeamId::S7RecallQuota` 变体(双定义,ADR-037 决策 6 模式)
- `SeamId::S7RecallQuota` 的 `code_anchor` = `"crates/omega-learner/src/r1_recall_quota.rs"`(P4-W16.2.2 待实现)
- `SeamId::S7RecallQuota` 的 `short_name` = `"S7-recall-quota"`

**2. R1 训练模块占位**(P4-W16.2.2 实施前)

- `crates/omega-learner/src/r1_recall_quota.rs` 新建模块,承载 R1 CQL/IQL 算法(P4-W16.2.2 任务)
- 本 ADR 仅定义影子模式开关机制,不实现 R1 算法本身
- R1 算法实施时,必须遵循本 ADR 决策 1 的接入路径(通过 CapabilityToken::Provisional 控制部署)

**3. 文档同步**(本 ADR 批准后 24 小时内)

- [CHANGELOG.md](file:///D:/Chimera CLI/CHANGELOG.md) 新增"ADR-043 R1 影子模式"条目
- [docs/architecture/CODE_WIKI.md](file:///D:/Chimera CLI/docs/architecture/CODE_WIKI.md) omega-learner 条目补 R1 影子模式说明
- [docs/architecture/adr_index.md](file:///D:/Chimera CLI/docs/architecture/adr_index.md) 新增 ADR-043 条目
- [.trae/specs/nexus-omega-v5-implementation-plan/tasks.md](file:///D:/Chimera CLI/.trae/specs/nexus-omega-v5-implementation-plan/tasks.md) P4-W16.2.4 标记为完成,引用本 ADR

**4. CI 检查**(`.github/workflows/release.yml`)

- 新增 `r1-shadow-mode-guard` job(在 build/test 之后,非主干阻塞):
  - 扫描 `crates/omega-learner/src/` 源码,确认 `SeamId::S7RecallQuota` 变体存在
  - 扫描 `crates/omega-learner/src/shadow_mode.rs` 文件存在(P4-W16.2.2 实施 R1 时创建)
  - 若 P4-W16.2.2 实施后 `shadow_mode.rs` 不存在,CI 失败
- 集成测试 `crates/omega-learner/tests/r1_shadow_mode_test.rs`:
  - 断言 `CapabilityToken` 初始状态为 `Provisional` 时,编排器 `fallback_to_static()`
  - 断言 `maybe_promote()` 成功后(模拟 EWMA ≥ 0.7 + 2 周观察期),状态变为 `Authorized`
  - 断言 `trigger_asa_intervention()` 触发后,状态变为 `Cooldown`,观察期重置

## 理由

### 决策 1 理由(复用 CapabilityToken::Provisional)

- **C4 合规**:影子模式开关复用既有能力场机制(CapabilityToken),不引入运行时 Feature Flag。符合 ADR-034 决策 1(灰度走能力场)与 ADR-037 决策 2(CapabilityToken 四态)。
- **避免重复造轮子**:`CapabilityToken::Provisional` 状态的语义("已生成但未达阈值,不允许 Learned")与影子模式("策略已训练但不部署")完全一致。新建独立开关类型将导致双份状态管理逻辑。
- **与六接缝(S1-S6)对称**:R1 接缝(S7RecallQuota)与 S1-S6 共享相同的 CapabilityToken 机制,编排器查询逻辑统一(均为 `should_activate_learned(seam)`),无特殊路径。
- **状态转换一致性**:R1 影子模式的状态转换(Provisional → Authorized / Cooldown / Frozen)完全复用 ADR-037 决策 2 的状态转换图,无新增状态。

### 决策 2 理由(新增 ShadowComparisonReport 类型)

- **可证伪性**(§3.4.1 第 6 条):影子模式的解冻决策必须基于可量化的对比数据,而非主观判断。`ShadowComparisonReport` 提供客观证据(召回率/误杀率/延迟惩罚/综合得分)。
- **时间证据包**(§3.4.5 记忆悖论红线):对比报告包含 `report_date`,2 周观察期内的对比趋势可追溯,避免"幽灵记忆"(新旧对比结果共存无法区分时间有效性)。
- **纯数据类型设计**:`ShadowComparisonReport` 是纯数据类型(无方法),便于序列化、持久化、跨模块传输。复杂的对比逻辑(如 `r1_is_better()`)封装在 `ComparisonResult` 枚举方法中。
- **告警触发机制**:连续 3 天 `R1SignificantlyWorse` 触发 `R1ShadowRegressionDetected` 事件,符合"问题早发现早处理"原则。Normal 级 broadcast(非 Critical),因影子模式期间 R1 策略本就未部署,退化不影响生产路径。

### 决策 3 理由(2 周解冻条件)

- **EWMA 达标 + 对比胜率双约束**:EWMA 是 R1 策略的"内部评估"(基于奖励信号),对比胜率是 R1 策略的"外部评估"(对比 L3 基线)。双约束确保 R1 策略既"自我评估良好"又"相对优势明显"。
- **14 天观察期的依据**:14 天足够覆盖工作日 + 周末的任务模式变化(代码重构/debug/test/docs/feature 五种 task_type),避免短期任务偏差导致误判。短于 14 天(如 7 天)可能因任务集中(如全周 debug 任务)导致对比偏差;长于 14 天(如 30 天)过度延迟 R1 解冻。
- **71.4% 胜率阈值的统计学依据**:14 天中 10 天以上胜率的概率,在 R1 无真实优势(零假设)时约为 5.9%(二项分布 B(14, 0.5) 的 P(X ≥ 10) ≈ 0.059),接近 5% 显著性水平。这是"可证伪"原则的体现——R1 必须证明自己优于 L3,而非"不差于"L3。
- **无 AsaIntervention 约束**:AsaIntervention 是安全护栏(§6.2 红线),若 R1 接缝触发 ASA,说明 R1 策略存在安全问题,不可解冻。这与 ADR-037 决策 3 的"连续 3 次 ASA → Frozen"机制一致。
- **人工评审必要性**:4 项自动条件全部满足后,仍需 E05 + E01 + E03 三方评审,确保架构、生产、记忆三视角全覆盖。这是"长期主义"原则的体现——不为短期通过牺牲长期可维护性。

### 决策 4 理由(回滚预案)

- **4 项回滚触发条件**:每项条件对应一类退化模式(对比劣势 / 安全事件 / EWMA 崩塌 / 召回率下降),确保各类退化都能被检测。
- **回滚不等于撤销 R1 算法**:影子模式期间 R1 策略本就未部署(Provisional 状态),回滚仅是"将 token 置为 Cooldown + 重置观察期",不影响 R1 算法代码。R1 算法的调整(CQL/IQL 超参数)在评审通过后重启观察期。
- **Critical 级回滚失败事件**:`R1ShadowRollbackFailed` 走 mpsc 旁路通道(§6.2 红线 5),确保回滚失败时专家团队立即介入。回滚失败等同于安全事件(无法回退到 L3 主路径),需 P0 优先级处置。
- **观察期重置**:回滚后观察期重置为 14 天,而非"从断点继续"。这是"长期主义"原则的体现——R1 策略退化后,需重新证明 2 周稳定性,而非"补足剩余天数"。

### 决策 5 理由(S7 接缝扩展 + 文档同步 + CI 检查)

- **S7 接缝扩展的必要性**:R1 是离线 RL 接缝,与六接缝(S1-S6 在线学习)不同。但为了编排器统一查询(`should_activate_learned(seam)`),R1 必须作为 SeamId 变体之一。新增 `S7RecallQuota` 不修改既有 S1-S6 变体,符合 append-only 原则。
- **双定义模式**(ADR-037 决策 6):`SeamId` 在 L0(nexus-contracts)与 L6(omega-learner)双定义,L0 用于跨层契约,L6 用于扩展方法。R1 接缝遵循相同模式。
- **CI 检查的必要性**:P4-W16.2.2 实施 R1 算法后,必须同步创建 `shadow_mode.rs` 模块。CI 检查确保 R1 算法与影子模式机制同步落地,避免"R1 算法存在但影子模式未实现"的不一致状态。
- **集成测试的必要性**:CI 静态扫描可能漏检(如 `shadow_mode.rs` 存在但内容不完整),集成测试通过状态转换断言,提供运行时层面的防护。

## 影响

### 新增内容

- **新增 SeamId 变体**:1 个(S7RecallQuota,在 L0 nexus-contracts 与 L6 omega-learner 双定义)
- **新增 NexusEvent 变体**:3 个(共 77 → 80)
  - `R1ShadowRegressionDetected { metadata, report_date, regression_streak }`(Normal 级 broadcast)— 决策 2 对比告警
  - `R1ShadowPromotionReady { metadata, report_date, win_rate, ewma_level }`(Normal 级 broadcast)— 决策 3 解冻评审触发
  - `R1ShadowRollbackFailed { metadata, reason }`(Critical 级,走 mpsc 旁路通道)— 决策 4 回滚失败
- **新增 MasError 变体**:1 个(共 35 → 36)
  - `R1ShadowRollbackFailed { reason }`(决策 4 回滚失败)
- **新增 omega-learner 模块**:`crates/omega-learner/src/shadow_mode.rs`(承载 `ShadowComparisonReport` / `StrategyMetrics` / `ComparisonResult` 类型)
- **新增 omega-learner 模块占位**:`crates/omega-learner/src/r1_recall_quota.rs`(P4-W16.2.2 实施 R1 算法时创建,本 ADR 仅占位)
- **新增 CI job**:`.github/workflows/release.yml` 新增 `r1-shadow-mode-guard` job(在 build/test 之后,非主干阻塞)
- **新增集成测试**:`crates/omega-learner/tests/r1_shadow_mode_test.rs`(状态转换断言)
- **新增 audit 文档目录**:R1 影子模式回滚报告与解冻评审报告归档至 `docs/audit/r1_shadow_*`
- **新增 ADR**:本 ADR(ADR-043)

### 修改内容

- **`crates/nexus-contracts/src/seam.rs`**:新增 `SeamId::S7RecallQuota` 变体
- **`crates/omega-learner/src/seam.rs`**:同步新增 `SeamId::S7RecallQuota` 变体(双定义)
- **`crates/omega-learner/src/lib.rs`**:新增 `shadow_mode` 模块声明 + 重导出
- **`crates/event-bus/src/types.rs`**:新增 `R1ShadowRegressionDetected` / `R1ShadowPromotionReady` / `R1ShadowRollbackFailed` 事件变体
- **`crates/chimera-mas/src/error.rs`**:新增 `R1ShadowRollbackFailed` 变体
- **`.github/workflows/release.yml`**:新增 `r1-shadow-mode-guard` job
- **`CHANGELOG.md`**:新增"ADR-043 R1 影子模式"条目
- **`docs/architecture/CODE_WIKI.md`**:omega-learner 条目补 R1 影子模式说明
- **`docs/architecture/adr_index.md`**:新增 ADR-043 条目
- **`.trae/specs/nexus-omega-v5-implementation-plan/tasks.md`**:P4-W16.2.4 标记为完成,引用本 ADR

### 资源影响评估

| 维度 | 评估 |
|------|------|
| crate 数量 | 35(不变,增量在既有 crate 内) |
| 依赖变更 | 无新增外部依赖(仅新增事件变体与类型) |
| Docker/binary 体积 | 不受影响(纯 Rust 新增代码,无 unsafe 依赖) |
| NexusEvent 变体数 | 77 → 80(新增 3 个 R1 影子模式事件) |
| MasError 变体数 | 35 → 36(新增 `R1ShadowRollbackFailed`) |
| SeamId 变体数 | 6 → 7(新增 `S7RecallQuota`,双定义) |
| 测试覆盖 | 新增 ~15 个测试(r1_shadow_mode_test 集成测试) |
| CI 时间 | `r1-shadow-mode-guard` job 增加 ~30 秒(源码扫描 + 集成测试),非主干阻塞 |
| 版本号 | 不变(本 ADR 是约束性决策,非功能性新增) |

## 考虑的方案

### 方案 A: 复用 CapabilityToken::Provisional + 新增对比报告类型(采纳)

- **内容**:R1 影子模式开关复用 ADR-037 CapabilityToken 四态机制(Provisional = 影子模式),新增 `ShadowComparisonReport` 类型承载对比数据。
- **采纳理由**:
  1. 复用既有机制,避免重复造轮子(C4 合规)
  2. 与六接缝(S1-S6)对称,编排器查询逻辑统一
  3. 对比报告类型提供可证伪的解冻证据(§3.4.1 第 6 条)
  4. append-only 策略,零回归风险

### 方案 B: 新建独立 ShadowMode 开关类型(否决)

- **内容**:新建 `ShadowModeToggle` 类型,独立于 CapabilityToken 管理 R1 影子模式开关。
- **否决理由**:
  1. **违反 C4 决策**:独立开关类型本质是运行时 Feature Flag,违反 ADR-034 决策 1(灰度走能力场)
  2. **状态管理重复**:`ShadowModeToggle` 与 `CapabilityToken` 双份状态管理逻辑,导致编排器查询两套状态
  3. **与六接缝不一致**:S1-S6 接缝用 CapabilityToken,R1 用 ShadowModeToggle,破坏对称性
  4. **过度工程化**:CapabilityToken::Provisional 已能承载影子模式语义,新建类型是过早抽象

### 方案 C: R1 直接解冻,无影子模式(否决)

- **内容**:R1 算法实施后直接解冻,不经过影子模式 2 周观察期。
- **否决理由**:
  1. **违反 v5.0 设计文档 §7.5**:明确"影子模式 2 周为 R1 解冻前置"
  2. **R3 风险无法控制**:R1 算法(CQL/IQL)可能存在轨迹选择偏差,无影子模式对比无法检测
  3. **奖励黑客风险**:R1 训练策略可能在 L2 评判分数上升但 L3 执行反馈无改善,无对比报告无法发现
  4. **与 ADR-042 不对称**:R2 在 FormalVerifier 落地前无条件冻结,R1 虽不冻结但需影子模式前置,两者都有解冻前置条件,体现"防御性架构"一致性

## 合规性

- **§2.1 分层映射**:符合。本 ADR 不改变分层结构,增量在 `omega-learner`(L6)/ `nexus-contracts`(L0)/ `event-bus`(L1)/ `chimera-mas`(L9)内,不改分层。
- **§2.2 依赖铁律**:符合。R1 影子模式是约束性决策,不引入新的跨层依赖。`R1ShadowRollbackFailed` 事件走 EventBus 异步广播(§6.2 红线),Critical 级走 mpsc 旁路通道。
- **§3.3.1 第 1 条(OMEGA 四定律守恒)**:符合。Ω-Evolve 由 gsoe-evolution 单一实现,R1 影子模式不改变 Ω-Evolve 的实现位置,仅约束 R1 路径的部署。
- **§3.3.1 第 4 条(领域类型稳定性)**:符合。不改 `UserIntent` / `Quest` / `Checkpoint` / `OmniSparseMasks` / `CLV` / `NexusState`。
- **§3.3.1 第 5 条(向后兼容)**:符合。append-only 扩展(新增 SeamId 变体 + 事件变体 + 类型 + CI job),不修改既有 API 签名。
- **§3.3.1 第 6 条(新 crate 准入)**:符合。不新建 crate。
- **§3.4.1 第 6 条(性能可证伪)**:符合。R1 影子模式解冻基于 4 项可量化条件(EWMA / 对比胜率 / 观察期 / 无 ASA),`ShadowComparisonReport` 提供客观证据。
- **§3.4.1 第 7 条(学术支撑落地)**:符合。R1 影子模式的统计学依据(71.4% 胜率阈值,二项检验 p ≈ 0.059)基于统计学假设检验理论。
- **§3.4.5 三重悖论红线(进化悖论)**:符合。R1 影子模式是进化悖论红线的工程实施层面防御——通过对比报告确保 R1 策略真实优于 L3 基线,防奖励黑客。
- **§4.1 编码规范**:符合。`#![forbid(unsafe_code)]` 保持;库层 thiserror;无生产路径 unwrap/expect;单函数 ≤200 行。
- **§4.4 async 反模式**:符合。不持锁跨 .await;Critical 级事件走 mpsc 旁路通道;`R1ShadowRollbackFailed` 事件发布用 `publish_blocking` 或 `publish_critical().await`。
- **§6.1 架构红线**:符合。不引入功能旗(决策 1 复用 CapabilityToken,非独立开关);单函数 ≤200 行;async 必须 await 或 spawn 管理。
- **§6.2 Week 1-8 实战新红线**:符合。`R1ShadowRollbackFailed` 为 Critical 级,走 mpsc 旁路通道(对齐红线 5);不持锁 .await。
- **C2 决策(嫁接 auto-dpo / gsoe-evolution)**:符合。R1 影子模式不影响 C2 嫁接决策,仅约束 R1 路径的部署。
- **C4 决策(灰度走能力场)**:符合。R1 影子模式开关复用 CapabilityToken(ADR-037),非运行时旗。
- **ADR-026 / ADR-028 既有决策**:全部保持。`MasError` 变体扩展沿用 append-only(ADR-028 决策 1);`NexusEvent` 变体扩展沿用 append-only(ADR-026)。
- **ADR-037 决策 2(CapabilityToken 四态)**:符合。本 ADR 决策 1 复用 `Provisional` 状态,不修改四态定义与状态转换规则。
- **ADR-042 决策 1(R2 冻结范围)**:符合。本 ADR 是 R1 路径的解冻前置设计,与 ADR-042 R2 冻结范围互补(ADR-042 冻结 R2,本 ADR 设计 R1 影子模式)。

## 相关文档

- **设计文档**: [NEXUS-OMEGA_v5.0_系统性完整设计文档.md](file:///D:/Chimera CLI/NEXUS-OMEGA_v5.0_系统性完整设计文档.md) §7.5 经验回放池 — R1 影子模式设计源
- **规则**: [.trae/rules/nuxus规则.md](file:///D:/Chimera CLI/.trae/rules/nuxus规则.md) §2.1(分层映射)/§2.2(依赖铁律)/§3.3.1(第二阶段开发原则)/§3.4.1(第三阶段开发原则)/§3.4.5(三重悖论红线)/§4.1(编码规范)/§4.4(async 反模式)/§6.1(架构红线)/§6.2(Week 1-8 新红线)
- **spec**: [.trae/specs/nexus-omega-v5-implementation-plan/spec.md](file:///D:/Chimera CLI/.trae/specs/nexus-omega-v5-implementation-plan/spec.md) P4-W16.2 章节
- **tasks**: [.trae/specs/nexus-omega-v5-implementation-plan/tasks.md](file:///D:/Chimera CLI/.trae/specs/nexus-omega-v5-implementation-plan/tasks.md) P4-W16.2.4(R1 影子模式设计)
- **CODE_WIKI.md**: [docs/architecture/CODE_WIKI.md](file:///D:/Chimera CLI/docs/architecture/CODE_WIKI.md) §3.1(crate 索引)/§2.3(ADR 表)
- **ADR 索引**: [docs/architecture/adr_index.md](file:///D:/Chimera CLI/docs/architecture/adr_index.md)(本 ADR 同步更新)
- **关联 ADR**:
  - [ADR-037](file:///D:/Chimera CLI/docs/architecture/ADR-037-capability-token-grayscale-engineering.md)(能力场灰度工程化 — 决策 2 CapabilityToken 四态,本 ADR 决策 1 复用 Provisional 状态)
  - [ADR-042](file:///D:/Chimera CLI/docs/architecture/ADR-042-r2-freeze-before-formal-verifier.md)(R2 冻结 — 决策 1 澄清 R1 不在冻结范围,本 ADR 是 R1 解冻前置设计)
  - [ADR-032](file:///D:/Chimera CLI/docs/architecture/ADR-032-dual-channel-evaluator.md)(RHI-CG 双通道评估器 — 决策 5 奖励护栏,R1 影子模式对比报告是奖励护栏的延伸)
  - [ADR-034](file:///D:/Chimera CLI/docs/architecture/ADR-034-capability-field-feature-flag.md)(灰度=能力场 — C4 决策,本 ADR 决策 1 遵循)
- **代码基线**:
  - [crates/omega-learner/src/replay_pool.rs](file:///D:/Chimera CLI/crates/omega-learner/src/replay_pool.rs)(`ReplayPool<T>` — R1 训练的轨迹来源)
  - [crates/decay-engine/src/capability_registry.rs](file:///D:/Chimera CLI/crates/decay-engine/src/capability_registry.rs)(`CapabilityTokenRegistry` — R1 影子模式开关的承载位置)
  - [crates/nexus-contracts/src/capability_token.rs](file:///D:/Chimera CLI/crates/nexus-contracts/src/capability_token.rs)(`CapabilityTokenStatus::Provisional` — R1 影子模式状态)
  - [crates/event-bus/src/types.rs](file:///D:/Chimera CLI/crates/event-bus/src/types.rs)(NexusEvent — R1 影子模式事件新增位置)
  - [crates/chimera-mas/src/error.rs](file:///D:/Chimera CLI/crates/chimera-mas/src/error.rs)(MasError — `R1ShadowRollbackFailed` 新增位置)
- **P4_P5 实施计划**: [.trae/specs/nexus-omega-v5-implementation-plan/P4_P5_IMPLEMENTATION_PLAN.md](file:///D:/Chimera CLI/.trae/specs/nexus-omega-v5-implementation-plan/P4_P5_IMPLEMENTATION_PLAN.md) 第 288/405/613 行(R1 影子模式相关上下文)

---

> **维护者**: NEXUS-OMEGA 团队
> **创建日期**: 2026-07-25
> **基线版本**: v2.3.1-omega(创建时,P4 阶段进行中)
> **决策者**: E01 首席架构师 + E05 生产系统专家 + E07 任务调度专家(分布式评审)
> **分析团队**: 3 专家视角分布式深度分析(首席架构 + 生产系统 + 任务调度)
> **解冻责任方**: E05 生产系统专家 + E01 首席架构师 + E03 记忆系统专家(三方书面批准)
> **预计首次解冻评审时间**: P4-W16.2.2 R1 算法实施完成 + 影子模式运行 2 周后(预计 2026-08-15,视 P4-W16.2.2 进度)
