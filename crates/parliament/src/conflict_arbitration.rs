//! 冲突仲裁 — TencentDB 两阶段仲裁（设计文档 §13.2）
//!
//! 对应架构层: **L8 Parliament**（parliament 子模块，规范指定落点）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §13.2
//! 对应论文: TencentDB Agent Memory（两阶段冲突仲裁：候选召回 → 判断）
//!
//! # 核心职责
//!
//! 新记忆卡片入库前的冲突仲裁两阶段流水线：
//! 1. **候选召回**（[`CandidateRetriever`]）：从既有卡片检索相似候选
//! 2. **判断**（[`ModelJudge`]）：四决策 AddNew（新增）/ Skip（跳过重复）/
//!    Update（更新既有）/ Merge（合并互补）
//!
//! # 设计约束（铁律）
//!
//! - **铁律1 零运行时 Python**: `ModelJudge` 为 trait 注入点，默认实现
//!   [`RuleBasedJudge`] 为纯规则判断（同 auto_builder SandboxExec 注入先例）；
//!   v4.0 模型接线经 trait 替换，本模块零改动
//! - **铁律3**: 消费 L0 [`AtomicMemoryCard`] 只读（仲裁不修改卡片）
//! - **防御分支**: 空召回短路 AddNew；未知决策路径回退 AddNew（不 panic）

use async_trait::async_trait;
use nexus_contracts::memory_pyramid::AtomicMemoryCard;

/// 仲裁结果（规范 §13.2 ArbitrationResult）
#[derive(Clone, Debug, PartialEq)]
pub enum ArbitrationResult {
    /// 新增卡片（无冲突）
    AddNew,
    /// 跳过（与既有卡片重复）
    Skip,
    /// 更新既有卡片（old_id 为目标 card_id）
    Update(Box<str>),
    /// 合并多张既有卡片（old_ids 为目标 card_id 列表）
    Merge(Vec<Box<str>>),
}

/// 模型判断决策（规范 §13.2 ModelDecision，ModelJudge 输出）
#[derive(Clone, Debug, PartialEq)]
pub enum ModelDecision {
    /// 新增
    AddNew,
    /// 跳过
    Skip,
    /// 更新既有（目标 card_id）
    Update(Box<str>),
    /// 合并（目标 card_id 列表）
    Merge(Vec<Box<str>>),
}

/// 候选召回 trait — 两阶段仲裁第一阶段注入点
///
/// 默认实现 [`RuleBasedRetriever`]（规则式相似度）；生产可替换为
/// 语义召回（L2 mlc-engine 记忆金字塔协同经调用方接线）。
pub trait CandidateRetriever: Send + Sync {
    /// 从既有卡片召回与新卡片相似的候选
    fn retrieve_similar(
        &self,
        new_card: &AtomicMemoryCard,
        existing_cards: &[AtomicMemoryCard],
    ) -> Vec<AtomicMemoryCard>;
}

/// 规则式候选召回 — 内容子串/场景匹配（铁律1 规则实现）
#[derive(Clone, Debug)]
pub struct RuleBasedRetriever {
    /// 最小公共词数阈值（内容词交集 ≥ 阈值视为相似）
    pub min_shared_terms: usize,
}

impl Default for RuleBasedRetriever {
    fn default() -> Self {
        Self {
            min_shared_terms: 2,
        }
    }
}

impl CandidateRetriever for RuleBasedRetriever {
    fn retrieve_similar(
        &self,
        new_card: &AtomicMemoryCard,
        existing_cards: &[AtomicMemoryCard],
    ) -> Vec<AtomicMemoryCard> {
        existing_cards
            .iter()
            .filter(|existing| {
                // 场景一致 + 内容词交集门控
                if existing.scene != new_card.scene {
                    return false;
                }
                let shared = shared_term_count(&new_card.content, &existing.content);
                shared >= self.min_shared_terms
            })
            .cloned()
            .collect()
    }
}

/// 模型判断 trait — 两阶段仲裁第二阶段注入点（D-4）
///
/// 默认实现 [`RuleBasedJudge`]（规则判断矩阵）；v4.0 模型接线经
/// trait 替换（铁律1：运行时零 Python，注入点预留）。
#[async_trait]
pub trait ModelJudge: Send + Sync {
    /// 判断新卡片与候选集的仲裁决策
    async fn judge(
        &self,
        new_card: &AtomicMemoryCard,
        candidates: &[AtomicMemoryCard],
    ) -> ModelDecision;
}

/// 规则式判断 — 规则判断矩阵（铁律1 默认实现）
///
/// 判断优先级：
/// 1. 内容完全一致 → Skip（重复）
/// 2. 高度重叠（≥70% 词交集占比）→ Update（最相似者）
/// 3. 互补内容（多候选且低重叠）→ Merge
/// 4. 否则 → AddNew
#[derive(Clone, Debug, Default)]
pub struct RuleBasedJudge;

#[async_trait]
impl ModelJudge for RuleBasedJudge {
    async fn judge(
        &self,
        new_card: &AtomicMemoryCard,
        candidates: &[AtomicMemoryCard],
    ) -> ModelDecision {
        if candidates.is_empty() {
            return ModelDecision::AddNew;
        }
        // 1. 完全一致 → Skip
        if candidates.iter().any(|c| c.content == new_card.content) {
            return ModelDecision::Skip;
        }
        // 2. 高度重叠 → Update（最相似者）
        let mut best_overlap: Option<(f32, &AtomicMemoryCard)> = None;
        for candidate in candidates {
            let overlap = overlap_ratio(&new_card.content, &candidate.content);
            if best_overlap.is_none_or(|(best, _)| overlap > best) {
                best_overlap = Some((overlap, candidate));
            }
        }
        if let Some((overlap, best)) = best_overlap {
            if overlap >= 0.7 {
                return ModelDecision::Update(best.card_id.clone());
            }
            // 3. 互补内容 → Merge（多候选且均有实质重叠）
            if candidates.len() >= 2
                && candidates
                    .iter()
                    .all(|c| overlap_ratio(&new_card.content, &c.content) >= 0.3)
            {
                return ModelDecision::Merge(
                    candidates.iter().map(|c| c.card_id.clone()).collect(),
                );
            }
            // 低重叠单候选 → AddNew（best 仅用于调试可观测性）
            let _ = best;
        }
        ModelDecision::AddNew
    }
}

/// 冲突仲裁器 — 两阶段流水线编排（规范 §13.2 ConflictArbitrator）
pub struct ConflictArbitrator {
    /// 候选召回（第一阶段注入点）
    candidate_retriever: Box<dyn CandidateRetriever>,
    /// 模型判断（第二阶段注入点，D-4）
    model_judge: Box<dyn ModelJudge>,
}

impl Default for ConflictArbitrator {
    fn default() -> Self {
        Self::new(
            Box::new(RuleBasedRetriever::default()),
            Box::new(RuleBasedJudge),
        )
    }
}

impl ConflictArbitrator {
    /// 创建仲裁器（注入召回器与判断器）
    pub fn new(
        candidate_retriever: Box<dyn CandidateRetriever>,
        model_judge: Box<dyn ModelJudge>,
    ) -> Self {
        Self {
            candidate_retriever,
            model_judge,
        }
    }

    /// 仲裁新卡片 — 召回 → 判断（空召回短路 AddNew）
    pub async fn arbitrate(
        &self,
        new_card: &AtomicMemoryCard,
        existing_cards: &[AtomicMemoryCard],
    ) -> ArbitrationResult {
        let candidates = self
            .candidate_retriever
            .retrieve_similar(new_card, existing_cards);
        if candidates.is_empty() {
            return ArbitrationResult::AddNew;
        }
        let decision = self.model_judge.judge(new_card, &candidates).await;
        match decision {
            ModelDecision::AddNew => ArbitrationResult::AddNew,
            ModelDecision::Skip => ArbitrationResult::Skip,
            ModelDecision::Update(old_id) => ArbitrationResult::Update(old_id),
            ModelDecision::Merge(old_ids) => ArbitrationResult::Merge(old_ids),
        }
    }
}

/// 内容词交集计数（规则召回/判断共用，词以空白分割）
fn shared_term_count(a: &str, b: &str) -> usize {
    let terms_a: Vec<&str> = a.split_whitespace().collect();
    let terms_b: Vec<&str> = b.split_whitespace().collect();
    terms_a.iter().filter(|t| terms_b.contains(t)).count()
}

/// 内容重叠占比（交集 / 较小集合，∈ [0,1]；空内容返回 0.0）
fn overlap_ratio(a: &str, b: &str) -> f32 {
    let terms_a: Vec<&str> = a.split_whitespace().collect();
    let terms_b: Vec<&str> = b.split_whitespace().collect();
    if terms_a.is_empty() || terms_b.is_empty() {
        return 0.0;
    }
    let shared = terms_a.iter().filter(|t| terms_b.contains(t)).count();
    shared as f32 / terms_a.len().min(terms_b.len()) as f32
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::memory_pyramid::AtomicCardType;

    fn card(id: &str, scene: &str, content: &str) -> AtomicMemoryCard {
        AtomicMemoryCard::new(
            id,
            AtomicCardType::Policy,
            100,
            scene,
            content,
            None,
            None,
            None,
            None,
            1_700_000_000_000,
        )
    }

    #[tokio::test]
    async fn empty_recall_short_circuit_add_new() {
        let arbitrator = ConflictArbitrator::default();
        let new_card = card("new", "scene-1", "unique content terms");
        // 空既有集 → 召回空 → AddNew 短路
        let result = arbitrator.arbitrate(&new_card, &[]).await;
        assert_eq!(result, ArbitrationResult::AddNew);
    }

    #[tokio::test]
    async fn duplicate_content_skips() {
        let arbitrator = ConflictArbitrator::default();
        let existing = card("old", "scene-1", "fix type error in parser");
        let new_card = card("new", "scene-1", "fix type error in parser");
        let result = arbitrator.arbitrate(&new_card, &[existing]).await;
        assert_eq!(result, ArbitrationResult::Skip);
    }

    #[tokio::test]
    async fn high_overlap_updates() {
        let arbitrator = ConflictArbitrator::default();
        let existing = card("old", "scene-1", "fix type error in parser module");
        // 高度重叠（4/5 词交集 = 0.8 ≥ 0.7）→ Update(old)
        let new_card = card("new", "scene-1", "fix type error in parser function");
        let result = arbitrator.arbitrate(&new_card, &[existing]).await;
        assert_eq!(result, ArbitrationResult::Update(Box::from("old")));
    }

    #[tokio::test]
    async fn complementary_content_merges() {
        let arbitrator = ConflictArbitrator::default();
        // 两候选各有实质重叠（≥0.3）但新卡片非完全一致/高重叠单点
        let c1 = card("c1", "scene-1", "alpha beta gamma delta");
        let c2 = card("c2", "scene-1", "alpha beta epsilon zeta");
        // 新卡片与 c1/c2 均有 0.5 重叠（2/4），< 0.7 单点但 ≥ 0.3 双候选
        let new_card = card("new", "scene-1", "alpha beta eta theta");
        let result = arbitrator.arbitrate(&new_card, &[c1, c2]).await;
        assert_eq!(
            result,
            ArbitrationResult::Merge(vec![Box::from("c1"), Box::from("c2")])
        );
    }

    #[tokio::test]
    async fn scene_mismatch_no_recall() {
        let arbitrator = ConflictArbitrator::default();
        let existing = card("old", "scene-other", "fix type error in parser");
        let new_card = card("new", "scene-1", "fix type error in parser");
        // 场景不一致 → 召回空 → AddNew
        let result = arbitrator.arbitrate(&new_card, &[existing]).await;
        assert_eq!(result, ArbitrationResult::AddNew);
    }

    #[tokio::test]
    async fn custom_judge_injection() {
        // D-4 trait 注入替换验证
        struct AlwaysSkipJudge;
        #[async_trait]
        impl ModelJudge for AlwaysSkipJudge {
            async fn judge(
                &self,
                _new_card: &AtomicMemoryCard,
                _candidates: &[AtomicMemoryCard],
            ) -> ModelDecision {
                ModelDecision::Skip
            }
        }
        let arbitrator = ConflictArbitrator::new(
            Box::new(RuleBasedRetriever::default()),
            Box::new(AlwaysSkipJudge),
        );
        let existing = card("old", "scene-1", "shared terms here");
        let new_card = card("new", "scene-1", "shared terms here");
        let result = arbitrator.arbitrate(&new_card, &[existing]).await;
        assert_eq!(result, ArbitrationResult::Skip, "注入 judge 应生效");
    }

    #[test]
    fn overlap_ratio_bounds() {
        assert!((overlap_ratio("a b c", "a b c") - 1.0).abs() < 1e-6);
        assert!(overlap_ratio("a b", "c d").abs() < 1e-6);
        assert!(overlap_ratio("", "a").abs() < 1e-6, "空内容防御");
    }
}
