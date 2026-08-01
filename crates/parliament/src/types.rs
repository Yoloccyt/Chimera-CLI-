//! Parliament 核心类型 — 5 角色对抗性议会的领域模型
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:AHIRT(Anti-Hack Intelligent Red Team,反黑客红队)
//!
//! # 设计决策(WHY)
//! - `RoleId` 用 newtype:类型安全,防止与其他 ID 混用(§4.3 命名模式)
//! - `Opinion.position` 为 f32 ∈ {0.0, 0.5, 1.0}:0.0 反对 / 0.5 弃权 / 1.0 赞成,
//!   用 f32 而非 enum 便于加权计算(Σ 权重 × 立场)
//! - `Consensus` 为 enum:三种互斥结果(达成/拒绝/否决),携带不同上下文
//! - `Proposal.risk_level` 为 f32 ∈ [0.0, 1.0]:与 UserIntent.risk_level(0-100)不同,
//!   议会内部归一化为 [0.0, 1.0] 便于阈值比较

use serde::{Deserialize, Serialize};

// 使用 L1 Core 的 id_newtype! 宏生成 RoleId newtype
// WHY 集中宏:消除各 crate 重复实现 newtype,确保所有 ID 类型行为一致
// (Deref/AsRef/Borrow/From/Display),且 #[serde(transparent)] 保证向后兼容
nexus_core::id_newtype!(RoleId, "议会角色唯一标识");

// ============================================================
// 角色枚举 — 5 角色对抗性议会
// ============================================================

/// 议会角色枚举 — 5 个固定角色,各司其职
///
/// WHY 固定 5 角色:对应 AHIRT 反黑客红队设计,5 角色覆盖
/// 架构设计(Architect)、风险审查(Skeptic)、性能优化(Optimizer)、
/// 知识检索(Librarian)、创意发散(Bard)五个互补维度,
/// 避免单一视角导致的决策盲区。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    /// 架构师:关注系统架构合理性、依赖方向、模块边界
    Architect,
    /// 怀疑者:红队视角,挑战提案风险,拥有否决权(can_veto = true)
    Skeptic,
    /// 优化者:关注性能、资源占用、执行效率
    Optimizer,
    /// 图书馆员:关注知识检索、历史先例、文档完整性
    Librarian,
    /// 吟游诗人:关注创意发散、用户体验、替代方案
    Bard,
}

impl Role {
    /// 返回角色的字符串标识(用于日志、序列化)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Architect => "architect",
            Self::Skeptic => "skeptic",
            Self::Optimizer => "optimizer",
            Self::Librarian => "librarian",
            Self::Bard => "bard",
        }
    }

    /// 返回所有角色枚举值(固定顺序,用于遍历注册)
    pub fn all() -> [Role; 5] {
        [
            Self::Architect,
            Self::Skeptic,
            Self::Optimizer,
            Self::Librarian,
            Self::Bard,
        ]
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================
// 角色画像
// ============================================================

/// 角色画像 — 描述一个议会角色的能力与权限
///
/// `voting_weight` 为投票权重,所有角色权重和应为 1.0(归一化)。
/// `can_veto` 为 true 时,该角色立场为 0.0(反对)可触发否决,
/// 否决优先于共识判定。WHY Skeptic 独占否决权:红队视角的
/// 风险否决是 AHIRT 的核心安全防线,避免高风险提案通过。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoleProfile {
    /// 角色唯一 ID
    pub role_id: RoleId,
    /// 角色枚举
    pub role: Role,
    /// 专长描述(如 "系统架构与依赖分析")
    pub specialty: String,
    /// 偏好的模型 ID(Week 6 NMC 后接入真实模型路由)
    pub model_preference: String,
    /// 投票权重 [0.0, 1.0],所有角色权重和应为 1.0
    pub voting_weight: f32,
    /// 是否拥有否决权(仅 Skeptic 为 true)
    pub can_veto: bool,
}

impl RoleProfile {
    /// 创建新的角色画像
    pub fn new(
        role_id: impl Into<String>,
        role: Role,
        specialty: impl Into<String>,
        model_preference: impl Into<String>,
        voting_weight: f32,
        can_veto: bool,
    ) -> Self {
        Self {
            role_id: RoleId::new(role_id),
            role,
            specialty: specialty.into(),
            model_preference: model_preference.into(),
            voting_weight,
            can_veto,
        }
    }
}

// ============================================================
// 提案
// ============================================================

/// 提案 — 待议会审议的执行计划
///
/// `risk_level` ∈ `[0.0, 1.0]`(归一化),影响 Skeptic 的否决倾向:
/// 风险越高,Skeptic 越倾向于反对(模拟红队风险厌恶)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Proposal {
    /// 提案唯一 ID
    pub proposal_id: String,
    /// 关联的 Quest ID
    pub quest_id: String,
    /// 提案内容(自然语言描述)
    pub content: String,
    /// 风险等级 `[0.0, 1.0]`(归一化)
    pub risk_level: f32,
}

impl Proposal {
    /// 创建新的提案
    pub fn new(
        proposal_id: impl Into<String>,
        quest_id: impl Into<String>,
        content: impl Into<String>,
        risk_level: f32,
    ) -> Self {
        Self {
            proposal_id: proposal_id.into(),
            quest_id: quest_id.into(),
            content: content.into(),
            risk_level: risk_level.clamp(0.0, 1.0),
        }
    }
}

// ============================================================
// 意见(单个角色的投票)
// ============================================================

/// 意见 — 单个角色对提案的立场与理由
///
/// `position` ∈ {0.0(反对), 0.5(弃权), 1.0(赞成)},
/// 用 f32 而非 enum 便于加权计算(Σ 权重 × 立场)。
/// `confidence` ∈ [0.0, 1.0] 表示角色对自身判断的置信度,
/// 当前占位实现未使用,Week 6 NMC 接入真实模型后用于置信度加权。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Opinion {
    /// 投票角色
    pub role: Role,
    /// 立场:0.0 反对 / 0.5 弃权 / 1.0 赞成
    pub position: f32,
    /// 置信度 [0.0, 1.0]
    pub confidence: f32,
    /// 投票理由(自然语言)
    pub rationale: String,
}

impl Opinion {
    /// 创建新的意见
    pub fn new(role: Role, position: f32, confidence: f32, rationale: impl Into<String>) -> Self {
        Self {
            role,
            position,
            confidence: confidence.clamp(0.0, 1.0),
            rationale: rationale.into(),
        }
    }

    /// 判断是否为赞成票(position == 1.0)
    pub fn is_approve(&self) -> bool {
        (self.position - 1.0).abs() < 1e-6
    }

    /// 判断是否为反对票(position == 0.0)
    pub fn is_reject(&self) -> bool {
        self.position.abs() < 1e-6
    }

    /// 判断是否为弃权票(position == 0.5)
    pub fn is_abstain(&self) -> bool {
        (self.position - 0.5).abs() < 1e-6
    }
}

// ============================================================
// 共识结果
// ============================================================

/// 共识结果 — 议会审议的最终判定
///
/// 三种互斥结果:
/// - `Reached`:共识达成,携带决议哈希与可选 DPO 训练对 ID
/// - `Rejected`:提案被拒绝,携带拒绝原因
/// - `Vetoed`:Skeptic 否决,携带否决原因与冻结的能力列表
///
/// WHY Vetoed 独立于 Rejected:否决是安全机制(红队防线),
/// 需与普通拒绝区分,触发能力冻结等后续安全动作。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Consensus {
    /// 共识达成:赞成率 ≥ 阈值且无法定人数不足或否决
    Reached {
        /// 决议内容哈希(SHA-256 hex),供后续审计与去重
        decision_hash: String,
        /// 若共识产生 DPO 训练对,携带 pair_id 供 AutoDPO 消费
        dpo_pair_id: Option<String>,
    },
    /// 提案被拒绝:赞成率 < 阈值或法定人数不足
    Rejected {
        /// 拒绝原因
        reason: String,
    },
    /// Skeptic 否决:红队防线触发,优先于共识判定
    Vetoed {
        /// 否决原因
        veto_reason: String,
        /// 被冻结的能力 ID 列表(供 SecCore 消费)
        frozen_capabilities: Vec<String>,
    },
}

impl Consensus {
    /// 判断是否为共识达成
    pub fn is_reached(&self) -> bool {
        matches!(self, Self::Reached { .. })
    }

    /// 判断是否被否决
    pub fn is_vetoed(&self) -> bool {
        matches!(self, Self::Vetoed { .. })
    }

    /// 判断是否被拒绝(含否决)
    pub fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected { .. } | Self::Vetoed { .. })
    }
}

// ============================================================
// 辩论结果
// ============================================================

/// 辩论结果 — 一次 deliberation 的完整产出
///
/// 包含共识判定、所有角色意见、加权赞成率与参与率,
/// 用于审计日志与 DPO 训练对生成。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DebateResult {
    /// 共识判定
    pub consensus: Consensus,
    /// 所有角色的意见列表
    pub opinions: Vec<Opinion>,
    /// 加权赞成率 [0.0, 1.0](Σ 权重 × 立场,弃权不计入分母)
    pub weighted_approval_rate: f32,
    /// 参与率 [0.0, 1.0](已投票角色数 / 总角色数,含弃权)
    pub participation_rate: f32,
}

impl DebateResult {
    /// 创建新的辩论结果
    pub fn new(
        consensus: Consensus,
        opinions: Vec<Opinion>,
        weighted_approval_rate: f32,
        participation_rate: f32,
    ) -> Self {
        Self {
            consensus,
            opinions,
            weighted_approval_rate,
            participation_rate,
        }
    }
}

// ============================================================
// 审议缓存
// ============================================================

/// 审议缓存键 — 标识一次审议的唯一性
///
/// 包含 `proposal_id`、`strategy`(影响审议深度)和 `risk_level_bucket`
/// (量化到 ±0.05,使相近风险共享缓存)。
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ProposalKey {
    /// 提案 ID
    pub proposal_id: String,
    /// 策略(影响审议深度)
    pub strategy: String,
    /// 风险等级桶(量化到 ±0.05,使相近风险共享缓存)
    pub risk_level_bucket: u32,
}

/// 审议结果 LRU 缓存 — 最多 10 条
///
/// 使用 `Vec<(ProposalKey, Consensus)>` 实现简单 LRU,
/// 最近访问的条目在末尾,淘汰时移除头部。
///
/// WHY 手写 LRU 而非 `lru` crate:避免引入新依赖(Vec 手写足够
/// 处理 10 条规模,符合 §4.1 零新依赖铁律)。
#[derive(Debug)]
pub struct DeliberationCache {
    /// 缓存条目(最近使用在末尾)
    entries: Vec<(ProposalKey, Consensus)>,
    /// 最大条目数
    max_entries: usize,
}

impl DeliberationCache {
    /// 创建新的审议结果缓存
    ///
    /// # 参数
    /// - `max_entries`:最大条目数,`None` 时默认为 10
    pub fn new(max_entries: Option<usize>) -> Self {
        Self {
            entries: Vec::new(),
            max_entries: max_entries.unwrap_or(10),
        }
    }

    /// 查找缓存条目,并将命中条目移动到末尾(LRU 保序)
    ///
    /// # 返回
    /// - `Some(&Consensus)`:命中,返回缓存结果的引用
    /// - `None`:未命中
    pub fn get(&mut self, key: &ProposalKey) -> Option<&Consensus> {
        let pos = self.entries.iter().position(|(k, _)| k == key)?;
        // 将命中条目移到末尾(最近使用)
        let entry = self.entries.remove(pos);
        self.entries.push(entry);
        // 安全:pos 来自 position() 找到的有效索引,entries.last() 必定为 Some
        Some(
            &self
                .entries
                .last()
                .expect("entries.last() after push 不应为 None")
                .1,
        )
    }

    /// 插入或覆盖缓存条目
    ///
    /// 若键已存在,更新值并移动到末尾;超限时淘汰头部(最久未使用)。
    pub fn insert(&mut self, key: ProposalKey, consensus: Consensus) {
        // 若已存在,移除旧条目(后续 push 新条目到末尾)
        if let Some(pos) = self.entries.iter().position(|(k, _)| *k == key) {
            self.entries.remove(pos);
        }
        self.entries.push((key, consensus));
        // 淘汰最久未使用的条目(头部)
        while self.entries.len() > self.max_entries {
            self.entries.remove(0);
        }
    }

    /// 返回当前缓存条目数
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 缓存是否为空
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_id_newtype() {
        let id = RoleId::new("role-architect");
        assert_eq!(id.as_str(), "role-architect");
        // Deref<Target=str> 允许 &id 当作 &str
        let s: &str = &id;
        assert_eq!(s, "role-architect");
        // From<&str>
        let id2 = RoleId::from("role-architect");
        assert_eq!(id, id2);
        // Display
        assert_eq!(id.to_string(), "role-architect");
    }

    #[test]
    fn test_role_as_str() {
        assert_eq!(Role::Architect.as_str(), "architect");
        assert_eq!(Role::Skeptic.as_str(), "skeptic");
        assert_eq!(Role::Optimizer.as_str(), "optimizer");
        assert_eq!(Role::Librarian.as_str(), "librarian");
        assert_eq!(Role::Bard.as_str(), "bard");
    }

    #[test]
    fn test_role_all_returns_five_roles() {
        let all = Role::all();
        assert_eq!(all.len(), 5);
        assert!(all.contains(&Role::Architect));
        assert!(all.contains(&Role::Skeptic));
        assert!(all.contains(&Role::Optimizer));
        assert!(all.contains(&Role::Librarian));
        assert!(all.contains(&Role::Bard));
    }

    #[test]
    fn test_role_display() {
        assert_eq!(Role::Architect.to_string(), "architect");
        assert_eq!(Role::Skeptic.to_string(), "skeptic");
    }

    #[test]
    fn test_role_profile_new() {
        let profile = RoleProfile::new(
            "role-skeptic",
            Role::Skeptic,
            "风险审查与红队对抗",
            "claude-3-opus",
            0.30,
            true,
        );
        assert_eq!(profile.role_id.as_str(), "role-skeptic");
        assert_eq!(profile.role, Role::Skeptic);
        assert_eq!(profile.specialty, "风险审查与红队对抗");
        assert_eq!(profile.model_preference, "claude-3-opus");
        assert!((profile.voting_weight - 0.30).abs() < 1e-6);
        assert!(profile.can_veto);
    }

    #[test]
    fn test_proposal_new_clamps_risk_level() {
        let p = Proposal::new("p-1", "q-1", "测试提案", 1.5);
        assert!((p.risk_level - 1.0).abs() < 1e-6);
        let p2 = Proposal::new("p-2", "q-1", "测试提案", -0.5);
        assert!(p2.risk_level.abs() < 1e-6);
    }

    #[test]
    fn test_opinion_predicates() {
        let approve = Opinion::new(Role::Architect, 1.0, 0.9, "架构合理");
        assert!(approve.is_approve());
        assert!(!approve.is_reject());
        assert!(!approve.is_abstain());

        let reject = Opinion::new(Role::Skeptic, 0.0, 0.8, "风险过高");
        assert!(!reject.is_approve());
        assert!(reject.is_reject());
        assert!(!reject.is_abstain());

        let abstain = Opinion::new(Role::Bard, 0.5, 0.5, "信息不足");
        assert!(!abstain.is_approve());
        assert!(!abstain.is_reject());
        assert!(abstain.is_abstain());
    }

    #[test]
    fn test_opinion_confidence_clamped() {
        let o = Opinion::new(Role::Architect, 1.0, 1.5, "高置信度");
        assert!((o.confidence - 1.0).abs() < 1e-6);
        let o2 = Opinion::new(Role::Architect, 1.0, -0.5, "低置信度");
        assert!(o2.confidence.abs() < 1e-6);
    }

    #[test]
    fn test_consensus_predicates() {
        let reached = Consensus::Reached {
            decision_hash: "abc123".into(),
            dpo_pair_id: None,
        };
        assert!(reached.is_reached());
        assert!(!reached.is_rejected());
        assert!(!reached.is_vetoed());

        let rejected = Consensus::Rejected {
            reason: "赞成率不足".into(),
        };
        assert!(!rejected.is_reached());
        assert!(rejected.is_rejected());
        assert!(!rejected.is_vetoed());

        let vetoed = Consensus::Vetoed {
            veto_reason: "Skeptic 否决".into(),
            frozen_capabilities: vec!["cap-1".into()],
        };
        assert!(!vetoed.is_reached());
        assert!(vetoed.is_rejected());
        assert!(vetoed.is_vetoed());
    }

    #[test]
    fn test_debate_result_new() {
        let opinions = vec![Opinion::new(Role::Architect, 1.0, 0.9, "赞成")];
        let result = DebateResult::new(
            Consensus::Reached {
                decision_hash: "hash".into(),
                dpo_pair_id: None,
            },
            opinions.clone(),
            0.25,
            0.2,
        );
        assert!(result.consensus.is_reached());
        assert_eq!(result.opinions.len(), 1);
        assert!((result.weighted_approval_rate - 0.25).abs() < 1e-6);
        assert!((result.participation_rate - 0.2).abs() < 1e-6);
    }

    #[test]
    fn test_serde_roundtrip_role() {
        let role = Role::Skeptic;
        let json = serde_json::to_string(&role).unwrap();
        let restored: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(role, restored);
    }

    #[test]
    fn test_serde_roundtrip_role_profile() {
        let profile = RoleProfile::new("role-1", Role::Architect, "架构设计", "gpt-4", 0.25, false);
        let json = serde_json::to_string(&profile).unwrap();
        let restored: RoleProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(profile, restored);
    }

    #[test]
    fn test_serde_roundtrip_consensus() {
        let consensus = Consensus::Vetoed {
            veto_reason: "高风险".into(),
            frozen_capabilities: vec!["cap-1".into(), "cap-2".into()],
        };
        let json = serde_json::to_string(&consensus).unwrap();
        let restored: Consensus = serde_json::from_str(&json).unwrap();
        assert_eq!(consensus, restored);
    }

    // ============================================================
    // DeliberationCache 单元测试
    // ============================================================

    #[test]
    fn test_deliberation_cache_new_default_max_entries() {
        let cache = DeliberationCache::new(None);
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_deliberation_cache_new_custom_max_entries() {
        let cache = DeliberationCache::new(Some(5));
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_deliberation_cache_insert_and_get() {
        let mut cache = DeliberationCache::new(None);
        let key = ProposalKey {
            proposal_id: "p-1".into(),
            strategy: "full".into(),
            risk_level_bucket: 4,
        };
        let consensus = Consensus::Reached {
            decision_hash: "abc".into(),
            dpo_pair_id: None,
        };

        cache.insert(key.clone(), consensus.clone());
        assert_eq!(cache.len(), 1);
        assert!(!cache.is_empty());

        let cached = cache.get(&key);
        assert!(cached.is_some());
        assert_eq!(*cached.unwrap(), consensus);
    }

    #[test]
    fn test_deliberation_cache_get_miss() {
        let mut cache = DeliberationCache::new(None);
        let key = ProposalKey {
            proposal_id: "p-miss".into(),
            strategy: "full".into(),
            risk_level_bucket: 4,
        };
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn test_deliberation_cache_insert_overwrite_same_key() {
        let mut cache = DeliberationCache::new(None);
        let key = ProposalKey {
            proposal_id: "p-1".into(),
            strategy: "full".into(),
            risk_level_bucket: 4,
        };
        let consensus1 = Consensus::Reached {
            decision_hash: "hash1".into(),
            dpo_pair_id: None,
        };
        let consensus2 = Consensus::Rejected {
            reason: "覆盖测试".into(),
        };

        cache.insert(key.clone(), consensus1);
        assert_eq!(cache.len(), 1);

        cache.insert(key.clone(), consensus2);
        assert_eq!(cache.len(), 1);
        let cached = cache.get(&key).unwrap();
        assert!(cached.is_rejected());
    }

    #[test]
    fn test_deliberation_cache_lru_eviction() {
        let mut cache = DeliberationCache::new(Some(3));
        let keys: Vec<ProposalKey> = (0..4)
            .map(|i| ProposalKey {
                proposal_id: format!("p-{i}"),
                strategy: "full".into(),
                risk_level_bucket: 4,
            })
            .collect();
        let consensus = Consensus::Reached {
            decision_hash: "hash".into(),
            dpo_pair_id: None,
        };

        for key in &keys {
            cache.insert(key.clone(), consensus.clone());
        }

        assert_eq!(cache.len(), 3);
        assert!(cache.get(&keys[0]).is_none(), "p-0 应被淘汰");
        assert!(cache.get(&keys[1]).is_some(), "p-1 应仍在缓存");
        assert!(cache.get(&keys[2]).is_some(), "p-2 应仍在缓存");
        assert!(cache.get(&keys[3]).is_some(), "p-3 应仍在缓存");
    }

    #[test]
    fn test_deliberation_cache_lru_reorder_on_get() {
        let mut cache = DeliberationCache::new(Some(3));
        let keys: Vec<ProposalKey> = (0..3)
            .map(|i| ProposalKey {
                proposal_id: format!("p-{i}"),
                strategy: "full".into(),
                risk_level_bucket: 4,
            })
            .collect();
        let consensus = Consensus::Reached {
            decision_hash: "hash".into(),
            dpo_pair_id: None,
        };

        for key in &keys {
            cache.insert(key.clone(), consensus.clone());
        }
        assert_eq!(cache.len(), 3);

        // 访问 keys[0](最旧),使其移到末尾
        cache.get(&keys[0]).unwrap();

        let key4 = ProposalKey {
            proposal_id: "p-3".into(),
            strategy: "full".into(),
            risk_level_bucket: 4,
        };
        cache.insert(key4, consensus.clone());
        assert_eq!(cache.len(), 3);

        assert!(cache.get(&keys[0]).is_some(), "被访问的 p-0 应保留");
        assert!(cache.get(&keys[1]).is_none(), "未访问的 p-1 应被淘汰");
        assert!(cache.get(&keys[2]).is_some(), "p-2 应保留");
    }

    #[test]
    fn test_proposal_key_hash_eq() {
        let k1 = ProposalKey {
            proposal_id: "p-1".into(),
            strategy: "full".into(),
            risk_level_bucket: 4,
        };
        let k2 = ProposalKey {
            proposal_id: "p-1".into(),
            strategy: "full".into(),
            risk_level_bucket: 4,
        };
        let k3 = ProposalKey {
            proposal_id: "p-2".into(),
            strategy: "full".into(),
            risk_level_bucket: 4,
        };
        assert_eq!(k1, k2);
        assert_ne!(k1, k3);
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher1 = DefaultHasher::new();
        let mut hasher2 = DefaultHasher::new();
        k1.hash(&mut hasher1);
        k2.hash(&mut hasher2);
        assert_eq!(hasher1.finish(), hasher2.finish());
    }
}
