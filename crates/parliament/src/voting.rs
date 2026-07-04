//! 加权投票与共识判定 — 5 角色加权计票与共识判定核心
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:AHIRT(Anti-Hack Intelligent Red Team,反黑客红队)
//!
//! # 设计决策(WHY)
//! - 加权赞成率 = Σ(非弃权角色权重 × 立场) / Σ(非弃权角色权重):
//!   弃权不计入分母(任务要求),避免弃权稀释赞成率;
//!   若所有角色弃权(分母为 0),赞成率设为 0.0(无法达成共识)
//! - 共识判定优先级:法定人数 → Skeptic 否决 → 赞成率阈值:
//!   Skeptic 否决优先于赞成率判定,确保红队防线不可被多数票绕过
//! - `ConsensusReached` 事件为 Critical 级:丢失会导致 GSOE/AutoDPO
//!   无法消费共识结果(§2.2 依赖铁律:跨层通信只能走 Event Bus)

use event_bus::{EventBus, EventMetadata, NexusEvent};
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::config::ParliamentConfig;
use crate::types::{Consensus, Opinion, Proposal, Role};

/// 投票结果 — 加权计票的完整产出
///
/// 包含加权赞成率、参与率与共识判定,用于审计日志与事件发布。
#[derive(Debug, Clone, PartialEq)]
pub struct VoteResult {
    /// 加权赞成率 `[0.0, 1.0]`(弃权不计入分母)
    pub weighted_approval_rate: f32,
    /// 参与率 [0.0, 1.0](已投票角色数 / 总角色数,含弃权)
    pub participation_rate: f32,
    /// 共识判定
    pub consensus: Consensus,
}

/// 投票计数器 — 加权计票与共识判定
///
/// WHY 独立结构:将计票逻辑从辩论流程中解耦,便于单独测试与复用。
/// `VoteCounter` 为无状态结构(仅持有配置引用),线程安全。
pub struct VoteCounter {
    /// 议会配置(权重、阈值)
    config: ParliamentConfig,
}

impl VoteCounter {
    /// 创建新的投票计数器
    pub fn new(config: &ParliamentConfig) -> Self {
        Self {
            config: config.clone(),
        }
    }

    /// 计票并判定共识
    ///
    /// # 流程
    /// 1. 计算参与率(已投票角色数 / 总角色数,含弃权)
    /// 2. 参与率 < quorum_threshold → Rejected(法定人数不足)
    /// 3. Skeptic 否决(立场=0.0 且 can_veto) → Vetoed
    /// 4. 计算加权赞成率(弃权不计入分母)
    /// 5. 赞成率 ≥ consensus_threshold → Reached
    /// 6. 赞成率 < consensus_threshold → Rejected
    ///
    /// # 参数
    /// - `opinions`:所有角色的意见列表
    /// - `total_roles`:已注册角色总数(用于计算参与率)
    /// - `proposal`:提案(用于生成决议哈希)
    pub fn count_votes(
        &self,
        opinions: &[Opinion],
        total_roles: usize,
        proposal: &Proposal,
    ) -> VoteResult {
        // 步骤 1:计算参与率(已投票角色数 / 总角色数)
        let participation_rate = if total_roles == 0 {
            0.0
        } else {
            opinions.len() as f32 / total_roles as f32
        };

        // 步骤 2:法定人数检查(优先级最高)
        if participation_rate < self.config.quorum_threshold {
            return VoteResult {
                weighted_approval_rate: 0.0,
                participation_rate,
                consensus: Consensus::Rejected {
                    reason: format!(
                        "quorum not met: participation {:.2} < required {:.2}",
                        participation_rate, self.config.quorum_threshold
                    ),
                },
            };
        }

        // 步骤 3:Skeptic 否决检查(红队防线,优先于赞成率判定)
        let skeptic_opinion = opinions.iter().find(|o| o.role == Role::Skeptic);
        if let Some(skeptic) = skeptic_opinion {
            if skeptic.is_reject() {
                let veto_reason = format!(
                    "Skeptic 否决:风险等级 {:.2},理由: {}",
                    proposal.risk_level, skeptic.rationale
                );
                return VoteResult {
                    weighted_approval_rate: 0.0,
                    participation_rate,
                    consensus: Consensus::Vetoed {
                        veto_reason,
                        // 冻结能力列表:当前为空,Week 5 Task 31 接入 SecCore 后填充
                        frozen_capabilities: Vec::new(),
                    },
                };
            }
        }

        // 步骤 4:计算加权赞成率(弃权不计入分母)
        let (weighted_approval_rate, _) = self.compute_weighted_approval(opinions);

        // 步骤 5-6:共识判定
        let consensus = if weighted_approval_rate >= self.config.consensus_threshold {
            // 共识达成:生成决议哈希
            let decision_hash = compute_decision_hash(proposal, opinions);
            Consensus::Reached {
                decision_hash,
                // DPO 训练对 ID:当前为 None,Week 5 Task 33 接入 AutoDPO 后填充
                dpo_pair_id: None,
            }
        } else {
            Consensus::Rejected {
                reason: format!(
                    "赞成率不足: {:.2} < 阈值 {:.2}",
                    weighted_approval_rate, self.config.consensus_threshold
                ),
            }
        };

        VoteResult {
            weighted_approval_rate,
            participation_rate,
            consensus,
        }
    }

    /// 计算加权赞成率(弃权不计入分母)
    ///
    /// 公式:赞成率 = Σ(非弃权角色权重 × 立场) / Σ(非弃权角色权重)
    /// - 立场 ∈ {0.0(反对), 0.5(弃权), 1.0(赞成)}
    /// - 弃权角色(立场 0.5)不参与分子与分母计算
    /// - 若所有角色弃权(分母为 0),返回 0.0
    ///
    /// # 返回
    /// (加权赞成率, 非弃权角色权重总和)
    fn compute_weighted_approval(&self, opinions: &[Opinion]) -> (f32, f32) {
        let mut weighted_sum: f32 = 0.0;
        let mut non_abstain_weight_sum: f32 = 0.0;

        for opinion in opinions {
            // 弃权角色不参与计算
            if opinion.is_abstain() {
                continue;
            }
            let weight = self.config.weight_of(opinion.role);
            weighted_sum += weight * opinion.position;
            non_abstain_weight_sum += weight;
        }

        // 分母为 0(所有角色弃权)时,赞成率设为 0.0
        let approval_rate = if non_abstain_weight_sum > 0.0 {
            weighted_sum / non_abstain_weight_sum
        } else {
            0.0
        };

        (approval_rate, non_abstain_weight_sum)
    }

    /// 获取配置引用
    pub fn config(&self) -> &ParliamentConfig {
        &self.config
    }
}

/// 计算决议内容哈希(SHA-256 hex)
///
/// WHY SHA-256:决议哈希用于审计日志去重与 GSOE 进化追踪,
/// SHA-256 提供抗碰撞保证,hex 编码便于日志与序列化
fn compute_decision_hash(proposal: &Proposal, opinions: &[Opinion]) -> String {
    let mut hasher = Sha256::new();

    // 将提案内容与所有意见纳入哈希
    hasher.update(proposal.proposal_id.as_bytes());
    hasher.update(proposal.quest_id.as_bytes());
    hasher.update(proposal.content.as_bytes());
    hasher.update(proposal.risk_level.to_le_bytes());

    for opinion in opinions {
        hasher.update(opinion.role.as_str().as_bytes());
        hasher.update(opinion.position.to_le_bytes());
        hasher.update(opinion.rationale.as_bytes());
    }

    let bytes = hasher.finalize();
    hex::encode(bytes)
}

/// 发布共识达成事件(Critical 级)
///
/// WHY Critical:ConsensusReached 事件丢失会导致 GSOE/AutoDPO
/// 无法消费共识结果(§2.2 依赖铁律),必须标注 Critical 确保投递
pub async fn publish_consensus_event(
    event_bus: &EventBus,
    quest_id: &str,
    decision_hash: &str,
    dpo_pair_id: Option<&str>,
) {
    let event = NexusEvent::ConsensusReached {
        metadata: EventMetadata::new("parliament"),
        quest_id: quest_id.to_string(),
        decision_hash: decision_hash.to_string(),
        dpo_pair_id: dpo_pair_id.map(String::from),
    };

    if let Err(e) = event_bus.publish(event).await {
        warn!(error = %e, "发布 ConsensusReached 事件失败");
    }
}

/// 发布投票事件(同层通信)
///
/// 每个角色的投票通过 VoteCast 事件广播,供审计与监控订阅
pub async fn publish_vote_event(event_bus: &EventBus, proposal_id: &str, voter: &str, vote: bool) {
    let event = NexusEvent::VoteCast {
        metadata: EventMetadata::new("parliament"),
        proposal_id: proposal_id.to_string(),
        voter: voter.to_string(),
        vote,
    };

    if let Err(e) = event_bus.publish(event).await {
        warn!(error = %e, "发布 VoteCast 事件失败");
    }
}

/// 发布 DebateStarted 事件
///
/// WHY:Parliament 进入辩论流程时发布,供 L9 Quest 与 L8 监控订阅者
/// 感知提案审议启动。Normal 级别,丢失仅导致本次辩论未被追踪,
/// 可由辩论超时机制兜底(§2.2 跨层通信只能走 Event Bus)
pub async fn publish_debate_started_event(
    bus: &EventBus,
    quest_id: &str,
    proposal_id: &str,
    participant_count: u8,
) {
    let event = NexusEvent::DebateStarted {
        metadata: EventMetadata::new("parliament"),
        quest_id: quest_id.to_string(),
        proposal_id: proposal_id.to_string(),
        participant_count,
    };
    if let Err(e) = bus.publish(event).await {
        warn!(error = %e, "发布 DebateStarted 事件失败");
    }
}

/// 发布 SkepticVeto 事件 `[Critical]`
///
/// WHY Critical:Skeptic 行使否决权时必须保证投递到 SecCore 以冻结对应能力。
/// 丢失会导致安全机制形同虚设,恶意提案继续执行,违反架构红线
/// "所有外部调用经 SecCore 沙箱 + Decay 衰减"。
/// `veto_reason` 携带完整否决上下文(intent_type/pattern/severity/detail),
/// 供 L4 SecCore 审计与能力冻结决策
pub async fn publish_skeptic_veto_event(
    bus: &EventBus,
    quest_id: &str,
    veto_reason: &str,
    frozen_capabilities: &[String],
) {
    let event = NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("parliament"),
        quest_id: quest_id.to_string(),
        veto_reason: veto_reason.to_string(),
        frozen_capabilities: frozen_capabilities.to_vec(),
    };
    if let Err(e) = bus.publish(event).await {
        warn!(error = %e, "发布 SkepticVeto 事件失败");
    }
}

/// 发布 CapabilityFrozen 事件
///
/// WHY:Skeptic 否决时冻结相关能力,每个冻结能力发布一条事件,
/// SecCore 订阅后撤销对应权限。Normal 级别,丢失仅导致单次冻结未生效,
/// 可由 SkepticVeto 事件(Critical)兜底
pub async fn publish_capability_frozen_event(bus: &EventBus, capability_id: &str, reason: &str) {
    let event = NexusEvent::CapabilityFrozen {
        metadata: EventMetadata::new("parliament"),
        capability_id: capability_id.to_string(),
        reason: reason.to_string(),
    };
    if let Err(e) = bus.publish(event).await {
        warn!(error = %e, "发布 CapabilityFrozen 事件失败");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> ParliamentConfig {
        ParliamentConfig::default()
    }

    fn make_counter() -> VoteCounter {
        VoteCounter::new(&make_config())
    }

    fn make_proposal() -> Proposal {
        Proposal::new("p-1", "q-1", "测试提案", 0.3)
    }

    fn make_all_approve_opinions() -> Vec<Opinion> {
        Role::all()
            .iter()
            .map(|&role| Opinion::new(role, 1.0, 0.9, "赞成"))
            .collect()
    }

    fn make_all_reject_opinions() -> Vec<Opinion> {
        Role::all()
            .iter()
            .map(|&role| Opinion::new(role, 0.0, 0.9, "反对"))
            .collect()
    }

    #[test]
    fn test_all_approve_reaches_consensus() {
        let counter = make_counter();
        let opinions = make_all_approve_opinions();
        let proposal = make_proposal();

        let result = counter.count_votes(&opinions, 5, &proposal);

        assert!(result.consensus.is_reached());
        assert!((result.weighted_approval_rate - 1.0).abs() < 1e-6);
        assert!((result.participation_rate - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_all_reject_rejected() {
        let counter = make_counter();
        let opinions = make_all_reject_opinions();
        let proposal = make_proposal();

        let result = counter.count_votes(&opinions, 5, &proposal);

        // Skeptic 反对 → Vetoed(优先于赞成率判定)
        assert!(result.consensus.is_vetoed());
        assert!((result.weighted_approval_rate - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_partial_approve_reaches_consensus() {
        let counter = make_counter();
        let proposal = make_proposal();

        // Architect(0.25) + Optimizer(0.20) + Librarian(0.15) + Bard(0.10) 赞成 = 0.70
        // Skeptic(0.30) 弃权(不参与分母)
        // 赞成率 = 0.70 / 0.70 = 1.0 ≥ 0.6 → 共识达成
        let opinions = vec![
            Opinion::new(Role::Architect, 1.0, 0.9, "架构合理"),
            Opinion::new(Role::Skeptic, 0.5, 0.5, "风险中等,弃权"),
            Opinion::new(Role::Optimizer, 1.0, 0.8, "性能可接受"),
            Opinion::new(Role::Librarian, 1.0, 0.7, "有历史先例"),
            Opinion::new(Role::Bard, 1.0, 0.6, "用户体验好"),
        ];

        let result = counter.count_votes(&opinions, 5, &proposal);

        assert!(result.consensus.is_reached());
        assert!((result.weighted_approval_rate - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_skeptic_veto_overrides_majority() {
        let counter = make_counter();
        let proposal = make_proposal();

        // 4 角色赞成(0.70 权重),Skeptic 反对(0.30 权重)
        // 即使赞成率 = 0.70 / 0.70 = 1.0,Skeptic 否决优先 → Vetoed
        let opinions = vec![
            Opinion::new(Role::Architect, 1.0, 0.9, "架构合理"),
            Opinion::new(Role::Skeptic, 0.0, 0.95, "高风险,否决"),
            Opinion::new(Role::Optimizer, 1.0, 0.8, "性能可接受"),
            Opinion::new(Role::Librarian, 1.0, 0.7, "有历史先例"),
            Opinion::new(Role::Bard, 1.0, 0.6, "用户体验好"),
        ];

        let result = counter.count_votes(&opinions, 5, &proposal);

        assert!(result.consensus.is_vetoed(), "Skeptic 否决应优先于多数票");
    }

    #[test]
    fn test_quorum_not_met_rejected() {
        let counter = make_counter();
        let proposal = make_proposal();

        // 仅 2 角色投票(参与率 0.4 < 0.6 阈值)
        let opinions = vec![
            Opinion::new(Role::Architect, 1.0, 0.9, "赞成"),
            Opinion::new(Role::Optimizer, 1.0, 0.8, "赞成"),
        ];

        let result = counter.count_votes(&opinions, 5, &proposal);

        assert!(result.consensus.is_rejected());
        assert!(!result.consensus.is_vetoed());
        assert!((result.participation_rate - 0.4).abs() < 1e-6);
    }

    #[test]
    fn test_all_abstain_approval_rate_zero() {
        let counter = make_counter();
        let proposal = make_proposal();

        // 所有角色弃权,分母为 0,赞成率设为 0.0
        let opinions: Vec<Opinion> = Role::all()
            .iter()
            .map(|&role| Opinion::new(role, 0.5, 0.5, "弃权"))
            .collect();

        let result = counter.count_votes(&opinions, 5, &proposal);

        // 参与率 1.0(弃权计入参与率),但赞成率 0.0 < 0.6 → Rejected
        assert!(!result.consensus.is_reached());
        assert!((result.weighted_approval_rate - 0.0).abs() < 1e-6);
        assert!((result.participation_rate - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_approval_rate_below_threshold_rejected() {
        let counter = make_counter();
        let proposal = make_proposal();

        // 仅 Bard(0.10)赞成,其余反对(不含 Skeptic)
        // Skeptic 弃权,非弃权权重 = 0.25 + 0.20 + 0.15 + 0.10 = 0.70
        // 赞成率 = 0.10 / 0.70 ≈ 0.143 < 0.6 → Rejected
        let opinions = vec![
            Opinion::new(Role::Architect, 0.0, 0.9, "反对"),
            Opinion::new(Role::Skeptic, 0.5, 0.5, "弃权"),
            Opinion::new(Role::Optimizer, 0.0, 0.8, "反对"),
            Opinion::new(Role::Librarian, 0.0, 0.7, "反对"),
            Opinion::new(Role::Bard, 1.0, 0.6, "赞成"),
        ];

        let result = counter.count_votes(&opinions, 5, &proposal);

        assert!(result.consensus.is_rejected());
        assert!(!result.consensus.is_vetoed());
        assert!(result.weighted_approval_rate < 0.6);
    }

    #[test]
    fn test_decision_hash_stable() {
        let proposal = make_proposal();
        let opinions = make_all_approve_opinions();

        let hash1 = compute_decision_hash(&proposal, &opinions);
        let hash2 = compute_decision_hash(&proposal, &opinions);

        assert_eq!(hash1, hash2, "相同输入应产生相同哈希");
        assert_eq!(hash1.len(), 64, "SHA-256 hex 应为 64 字符");
    }

    #[test]
    fn test_decision_hash_differs_on_different_input() {
        let proposal = make_proposal();
        let opinions_approve = make_all_approve_opinions();
        let opinions_reject = make_all_reject_opinions();

        let hash1 = compute_decision_hash(&proposal, &opinions_approve);
        let hash2 = compute_decision_hash(&proposal, &opinions_reject);

        assert_ne!(hash1, hash2, "不同意见应产生不同哈希");
    }

    #[test]
    fn test_reached_consensus_has_decision_hash() {
        let counter = make_counter();
        let opinions = make_all_approve_opinions();
        let proposal = make_proposal();

        let result = counter.count_votes(&opinions, 5, &proposal);

        if let Consensus::Reached { decision_hash, .. } = &result.consensus {
            assert!(!decision_hash.is_empty());
            assert_eq!(decision_hash.len(), 64);
        } else {
            panic!("应为 Reached 共识");
        }
    }

    #[test]
    fn test_vetoed_consensus_has_reason() {
        let counter = make_counter();
        let opinions = make_all_reject_opinions();
        let proposal = make_proposal();

        let result = counter.count_votes(&opinions, 5, &proposal);

        if let Consensus::Vetoed { veto_reason, .. } = &result.consensus {
            assert!(veto_reason.contains("Skeptic 否决"));
        } else {
            panic!("应为 Vetoed 共识");
        }
    }

    #[test]
    fn test_rejected_consensus_has_reason() {
        let counter = make_counter();
        let proposal = make_proposal();

        // 法定人数不足
        let opinions = vec![Opinion::new(Role::Architect, 1.0, 0.9, "赞成")];

        let result = counter.count_votes(&opinions, 5, &proposal);

        if let Consensus::Rejected { reason } = &result.consensus {
            assert!(reason.contains("quorum"));
        } else {
            panic!("应为 Rejected 共识");
        }
    }

    #[test]
    fn test_compute_weighted_approval_excludes_abstain() {
        let counter = make_counter();

        // Architect(0.25)赞成,Skeptic(0.30)弃权,其余反对
        // 非弃权权重 = 0.25 + 0.20 + 0.15 + 0.10 = 0.70
        // 赞成率 = (0.25 × 1.0) / 0.70 ≈ 0.357
        let opinions = vec![
            Opinion::new(Role::Architect, 1.0, 0.9, "赞成"),
            Opinion::new(Role::Skeptic, 0.5, 0.5, "弃权"),
            Opinion::new(Role::Optimizer, 0.0, 0.8, "反对"),
            Opinion::new(Role::Librarian, 0.0, 0.7, "反对"),
            Opinion::new(Role::Bard, 0.0, 0.6, "反对"),
        ];

        let (rate, non_abstain_sum) = counter.compute_weighted_approval(&opinions);

        assert!(
            (non_abstain_sum - 0.70).abs() < 1e-6,
            "非弃权权重和应为 0.70"
        );
        assert!((rate - (0.25 / 0.70)).abs() < 1e-6, "赞成率应为 0.25/0.70");
    }

    #[test]
    fn test_empty_opinions_rejected() {
        let counter = make_counter();
        let proposal = make_proposal();

        let result = counter.count_votes(&[], 5, &proposal);

        // 参与率 0.0 < 0.6 → Rejected
        assert!(result.consensus.is_rejected());
        assert!((result.participation_rate - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_zero_total_roles_rejected() {
        let counter = make_counter();
        let proposal = make_proposal();
        let opinions = make_all_approve_opinions();

        // total_roles = 0,参与率 = 0.0(避免除零)
        let result = counter.count_votes(&opinions, 0, &proposal);

        assert!(result.consensus.is_rejected());
        assert!((result.participation_rate - 0.0).abs() < 1e-6);
    }
}
