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
use nexus_core::Quest;
use tracing::{error, info, warn};

use crate::config::ParliamentConfig;
// MCA P2-1:跨厂商辩论策略 — Skeptic 与 Producer 异厂商通道
use crate::cross_vendor::CrossVendorDebate;
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
// B5 模块内聚:`debate.rs`(3901 行)按职责下沉为子模块
// ============================================================

// WHY 拆分点选 DPO/算子:两者与 `Parliament` 状态机无共享可变状态,仅被单向调用,
// 切走后本文件只剩「类型定义 + Parliament 的审议实现」一条主线。
// `pub use dpo`:`lib.rs` 的 `pub use debate::{DpoPair, DpoPairGenerator, Parliament}`
// 与 `prelude` 依赖这两个名字从本模块可见,再导出即保持公开面零变化。
mod dpo;
mod ops;
#[cfg(test)]
mod tests;

pub use dpo::{DpoPair, DpoPairGenerator};
use ops::{consensus_outcome_label, generate_opinion};

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
    /// MCA P2-1:跨厂商辩论策略 — Skeptic 与 Producer 异厂商通道(ADR-067)
    ///
    /// 当 Some 时，在辩论前通过 `prepare_debate` 确定每个角色的厂商分配，
    /// 确保 Skeptic 与 Producer 使用不同厂商通道，修复同源相关失败(病理 D3)。
    /// None = 未启用(向后兼容)，既有的 deliberate 行为不受影响。
    cross_vendor_debate: Option<CrossVendorDebate>,
    /// 行为契约集合（Milestone B-3c 生产接线，P1-4）
    ///
    /// 默认空 Vec = 零行为变化（deliberate 不设闸门，向后兼容）。
    /// 注入后 `deliberate_with_contract_guard` 在审议前强制校验。
    contracts: Vec<nexus_contracts::behavior_contract::BehaviorContract>,
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
            // MCA P2-1:默认禁用跨厂商辩论(向后兼容，既有的 deliberate 行为不受影响)
            cross_vendor_debate: None,
            // P1-4:默认空契约集合(向后兼容，deliberate 不设闸门)
            contracts: Vec::new(),
        }
    }

    /// 注入行为契约集合（可选）— 启用审议前置闸门（P1-4 生产接线）
    ///
    /// # 语义
    /// 注入后调用 `deliberate_with_contract_guard` 时，任一契约 Violated
    /// 即发布 `FormalViolation`（Critical + mpsc 旁路）并否决候选，
    /// 不进入辩论（九层防御 L0 "行为契约不可违反"）。
    pub fn with_behavior_contracts(
        mut self,
        contracts: Vec<nexus_contracts::behavior_contract::BehaviorContract>,
    ) -> Self {
        self.contracts = contracts;
        self
    }

    /// 契约守护审议 — 违反即否决（Milestone B-3c 生产接线，P1-4）
    ///
    /// `observed` 为调用方（编排器）提供的"已满足断言"观测集合。
    /// 任一契约 Violated → 发布 FormalViolation + 调用 handle_formal_violation
    /// + 返回 `Err(ParliamentError::ContractViolated)`（候选被否决，不进入辩论）。
    pub async fn deliberate_with_contract_guard(
        &self,
        quest: &Quest,
        proposal: &Proposal,
        observed: &[String],
    ) -> Result<Consensus, ParliamentError> {
        if let Some(verdict) =
            crate::formal_violation::enforce_and_audit(&self.event_bus, &self.contracts, observed)
        {
            warn!(verdict = %verdict, "行为契约违反:候选在审议前被否决");
            return Err(ParliamentError::ContractViolated(verdict));
        }
        self.deliberate(quest, proposal).await
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
        // 步骤 0.5:跨厂商辩论准备(MCA P2-1)
        // ============================================================
        // 如果启用了跨厂商辩论，在辩论前确定每个角色的厂商分配
        // 确保 Skeptic 与 Producer 使用不同厂商通道
        let cross_vendor_assignment = if let Some(ref cvd) = self.cross_vendor_debate {
            match cvd.prepare_debate(quest, proposal) {
                Ok(assignment) => {
                    info!(
                        quest_id = %quest.quest_id,
                        producer = ?assignment.producer_provider,
                        skeptic = ?assignment.skeptic_provider,
                        enforced = assignment.cross_vendor_enforced,
                        "跨厂商辩论:角色分配完成"
                    );
                    Some(assignment)
                }
                Err(e) => {
                    warn!(error = %e, "跨厂商辩论准备失败，使用默认通道");
                    None
                }
            }
        } else {
            None
        };

        // ============================================================
        // 步骤 1:按策略分派(三路径互斥)
        // ============================================================
        // 各路径额外返回度量载体(FastPath 无投票为 None),供下方统一
        // 发布 DebateCompleted 时携带投票率与多维共识质量。
        // MCA P2-1:传递 cross_vendor_assignment 确保 Skeptic 角色解析
        // 使用 AffinityRouter 的跨厂商分配(异厂商通道影响 Skeptic 置信度)。
        let (consensus, metrics) = match effective_strategy {
            ActivationStrategy::FastPath => {
                self.deliberate_fastpath(quest, proposal, cross_vendor_assignment.as_ref())
                    .await?
            }
            ActivationStrategy::Simplified => {
                self.deliberate_simplified(quest, proposal, cross_vendor_assignment.as_ref())
                    .await?
            }
            ActivationStrategy::Full => {
                self.deliberate_full(quest, proposal, cross_vendor_assignment.as_ref())
                    .await?
            }
        };

        // ============================================================
        // 步骤 2~4:审议收尾副作用(发布事件/趋势/缓存/仪表盘)
        // P2-11:提取为 finalize_deliberation(本函数 253 行超 200 行红线)
        // ============================================================
        self.finalize_deliberation(
            quest,
            proposal,
            debate_start,
            effective_strategy,
            metrics.as_ref(),
            &consensus,
            cache_key,
        )
        .await;

        Ok(consensus)
    }

    /// 审议收尾副作用:发布 DebateCompleted + 质量趋势 + 缓存 + 悖论仪表盘
    ///
    /// WHY 独立方法(P2-11):deliberate_with_policy 主流程超 _audit_fnlen 200 行红线;
    /// 本块为纯收尾(观测事件/缓存/仪表盘),与上方的决策编排正交。
    /// 顺序不变量:发布 → 趋势(缓存前)→ 缓存 → 仪表盘(缓存后),拆分保持原序。
    /// 锁纪律:全部锁在语句级作用域内释放,不跨 await 持锁(§4.4 反模式 1);
    /// publish_debate_completed_event 的 await 在取锁之前完成。
    #[allow(clippy::too_many_arguments)] // 对齐 voting.rs 既有模式
    async fn finalize_deliberation(
        &self,
        quest: &Quest,
        proposal: &Proposal,
        debate_start: Instant,
        effective_strategy: ActivationStrategy,
        metrics: Option<&DebateVoteMetrics>,
        consensus: &Consensus,
        cache_key: ProposalKey,
    ) {
        // ============================================================
        // 步骤 2:发布 DebateCompleted 观测事件(协调度量接线闭环 + M2 多维质量)
        // ============================================================
        publish_debate_completed_event(
            &self.event_bus,
            &quest.quest_id,
            &proposal.proposal_id,
            debate_start.elapsed().as_secs_f64() * 1000.0,
            effective_strategy.short_name(),
            metrics.map(DebateVoteMetrics::vote_rates),
            metrics.map(|m| &m.quality),
            consensus_outcome_label(consensus),
        )
        .await;

        // ============================================================
        // ADR-064:推送质量指标到趋势分析器(共识判定完成后,缓存写入之前)
        // ============================================================
        // WHY 在缓存写入之前:避免缓存命中时跳过趋势分析,确保每次实际
        // 审议都参与趋势统计。FastPath 路径 metrics=None,不推送。
        // 毒锁降级:使用 unwrap_or_else(|e| e.into_inner()) 恢复(§4.1 约定)。
        if let Some(m) = metrics {
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
                metrics.map_or(0.0f32, |m| {
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
    /// # 参数
    /// - `_cross_vendor`:跨厂商辩论角色分配（FastPath 无 Opinion 生成，仅接收保持签名一致）
    ///
    /// # 返回
    /// `(共识, None)` — FastPath 无投票,投票率恒为 `None`
    /// (DebateCompleted 事件的 weighted_approval_rate 随之为 None)。
    async fn deliberate_fastpath(
        &self,
        quest: &Quest,
        proposal: &Proposal,
        _cross_vendor: Option<&crate::cross_vendor::CrossVendorAssignment>,
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
    /// # 参数
    /// - `cross_vendor`:跨厂商辩论角色分配（用于 Skeptic 角色解析，确保异厂商通道影响置信度）
    ///
    /// # 返回
    /// `(共识, Some((加权赞成率, 参与率)))` — 投票率取自 `VoteResult`,
    /// 供 DebateCompleted 事件携带(共识质量 proxy,协调度量接线闭环)。
    async fn deliberate_simplified(
        &self,
        quest: &Quest,
        proposal: &Proposal,
        cross_vendor: Option<&crate::cross_vendor::CrossVendorAssignment>,
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

        // 并发收集 3 关键角色 Opinion（传递 cross_vendor 用于 Skeptic 角色解析）
        let opinions = self
            .collect_opinions_filtered(quest, proposal, &SIMPLIFIED_ROLES, cross_vendor)
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
    /// # 参数
    /// - `cross_vendor`:跨厂商辩论角色分配（用于 Skeptic 角色解析，确保异厂商通道影响置信度）
    ///
    /// # 返回
    /// `(共识, Some((加权赞成率, 参与率)))` — 语义同 `deliberate_simplified`。
    async fn deliberate_full(
        &self,
        quest: &Quest,
        proposal: &Proposal,
        cross_vendor: Option<&crate::cross_vendor::CrossVendorAssignment>,
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

        // 5 角色并行辩论,并发收集 Opinion（传递 cross_vendor 用于 Skeptic 角色解析）
        let opinions = self.collect_opinions(quest, proposal, cross_vendor).await?;

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

        let opinions = self.collect_opinions(quest, proposal, None).await?;
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
    /// # 参数
    /// - `cross_vendor`:跨厂商辩论角色分配（传递到 generate_opinion 用于 Skeptic 角色解析）
    ///
    /// # 错误
    /// - `DebateTimeout`:超时后无任何 Opinion 收集到(极端情况)
    async fn collect_opinions(
        &self,
        quest: &Quest,
        proposal: &Proposal,
        cross_vendor: Option<&crate::cross_vendor::CrossVendorAssignment>,
    ) -> Result<Vec<Opinion>, ParliamentError> {
        // 委托给 collect_opinions_filtered,传入全部 5 角色
        // WHY 委托:避免 5 角色路径与 filtered 路径逻辑重复,
        // collect_opinions_filtered 是统一的并发收集实现
        self.collect_opinions_filtered(quest, proposal, &Role::all(), cross_vendor)
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
    /// - `cross_vendor`:跨厂商辩论角色分配（传递到 generate_opinion 用于 Skeptic 角色解析）
    ///
    /// # 错误
    /// - `DebateTimeout`:超时后无任何 Opinion 收集到(极端情况)
    async fn collect_opinions_filtered(
        &self,
        quest: &Quest,
        proposal: &Proposal,
        roles: &[Role],
        cross_vendor: Option<&crate::cross_vendor::CrossVendorAssignment>,
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
        // MCA P2-1:Arc 共享 cross_vendor assignment（若存在），避免每角色深拷贝
        // 先 cloned() 解引用再 Arc::new，避免 `Arc::new(&T)` 产生双重引用
        let cross_vendor_arc = cross_vendor.cloned().map(std::sync::Arc::new);

        // 构建角色 Opinion 生成 future 流
        let mut stream: FuturesUnordered<_> =
            roles
                .iter()
                .map(|&role| {
                    let quest = std::sync::Arc::clone(&quest_arc);
                    let proposal = std::sync::Arc::clone(&proposal_arc);
                    let cross_vendor = cross_vendor_arc.as_ref().map(std::sync::Arc::clone);
                    async move {
                        generate_opinion(role, &quest, &proposal, cross_vendor.as_deref()).await
                    }
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

    /// 启用跨厂商辩论（MCA P2-1）
    ///
    /// 在辩论前通过 `AffinityRouter` 确定每个角色的厂商分配，
    /// 确保 Skeptic 与 Producer 使用不同厂商通道，修复同源相关失败(病理 D3)。
    ///
    /// # 参数
    /// - `config`:跨厂商辩论配置（启用/禁用、回退策略）
    ///
    /// # 设计
    /// 使用 `ProviderAffinityRegistry::default()` 创建空注册表（无预设绑定）。
    /// 外部调用方可通过 `registry()` 获取注册表引用后手动绑定角色。
    /// 默认空注册表时，`resolve_provider` 会从 proposal 上下文推断默认 provider。
    pub fn enable_cross_vendor_debate(&mut self, config: crate::cross_vendor::CrossVendorConfig) {
        use crate::cross_vendor::AffinityRouter;
        use std::sync::Arc;

        let registry = Arc::new(crate::provider_affinity::ProviderAffinityRegistry::default());
        let router = AffinityRouter::new(config, registry);
        self.cross_vendor_debate = Some(CrossVendorDebate::new(router, self.event_bus.clone()));
    }

    /// 启用跨厂商辩论并注入预配置的 ProviderAffinityRegistry（MCA P2-1 E2E 测试用）
    ///
    /// 与 `enable_cross_vendor_debate` 的区别：允许外部传入已绑定角色的注册表，
    /// 使测试可以设置特定的厂商绑定而无需通过 `infer_default_provider` 推断。
    ///
    /// # 参数
    /// - `config`:跨厂商辩论配置
    /// - `registry`:预配置的 ProviderAffinityRegistry（已绑定角色）
    pub fn enable_cross_vendor_debate_with_registry(
        &mut self,
        config: crate::cross_vendor::CrossVendorConfig,
        registry: std::sync::Arc<crate::provider_affinity::ProviderAffinityRegistry>,
    ) {
        use crate::cross_vendor::AffinityRouter;

        let router = AffinityRouter::new(config, registry);
        self.cross_vendor_debate = Some(CrossVendorDebate::new(router, self.event_bus.clone()));
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
