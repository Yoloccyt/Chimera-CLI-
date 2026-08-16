//! 按需记忆合成 — OpenMLE 懒加载祖先/兄弟节点 + 算子差异化上下文（设计文档 §7.1）
//!
//! 对应架构层: **L2 Memory**（mlc-engine 子模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §7.1
//! 对应论文: 清华 Frontis-MA1 OpenMLE（按需记忆合成，Prompt 降低 60%-86%）
//!
//! # 核心职责
//!
//! 为给定目标卡片与算子类型，懒加载最相关的祖先/兄弟节点上下文：
//! - **Draft**: 祖先 top-N（起草参考既有最佳实践）
//! - **Improve**: 高 progress 祖先 + 高分成功兄弟（改进借鉴）
//! - **Debug**: 同错误哈希的修复卡片（定向复用修复方案）
//! - **Crossover**: 高新颖度兄弟（融合多样化路径）
//!
//! # 设计约束（铁律5）
//!
//! - **懒加载不阻塞**: 仅检索选中算子所需的上下文子集，非全量加载；
//!   本模块为同步纯函数（无 IO），上下文规模由 `max_ancestors`/`max_siblings` 约束，
//!   保证 O(上下文规模) 而非 O(全库)
//! - **铁律3**: 只读消费 L0 卡片（不修改）
//! - **铁律4**: 上下文选择为纯函数（输入确定则输出确定）

use crate::experience_card_system::ExperienceCardSystem;
use nexus_contracts::experience_card::AtomicOperator;
use nexus_contracts::ExperienceCard;

// ============================================================
// 合成记忆输出
// ============================================================

/// 合成记忆 — 按需合成的上下文产物
#[derive(Clone, Debug)]
pub struct SynthesizedMemory {
    /// 目标卡片（克隆，铁律3 只读）
    pub target: ExperienceCard,
    /// 祖先洞察（method_family: score / progress 摘要）
    pub ancestor_insights: Vec<String>,
    /// 兄弟模式（method_family: novelty 摘要）
    pub sibling_patterns: Vec<String>,
    /// 估算 token 消耗（祖先 + 兄弟的 token_usage 总和）
    pub estimated_tokens: usize,
    /// 选中上下文的完整卡片（供下游注入 prompt）
    pub context_cards: Vec<ExperienceCard>,
}

// ============================================================
// 按需合成器
// ============================================================

/// 按需记忆合成器 — 无状态纯函数集合（铁律5 懒加载）
#[derive(Debug, Default, Clone, Copy)]
pub struct OnDemandSynthesizer;

impl OnDemandSynthesizer {
    /// 创建合成器
    pub fn new() -> Self {
        Self
    }

    /// 按需合成记忆 — 懒加载祖先/兄弟 + 算子差异化上下文选择
    ///
    /// - `system`: 经验卡片系统（提供祖先/兄弟检索）
    /// - `target_card`: 目标卡片（待合成的上下文锚点）
    /// - `operator`: 算子类型（决定上下文选择策略）
    /// - `max_ancestors` / `max_siblings`: 上下文规模上限（懒加载约束）
    pub fn synthesize(
        &self,
        system: &ExperienceCardSystem,
        target_card: &ExperienceCard,
        operator: &AtomicOperator,
        max_ancestors: usize,
        max_siblings: usize,
    ) -> SynthesizedMemory {
        let ancestors = self.find_ancestors(system, target_card, max_ancestors);
        let siblings = self.find_siblings(system, target_card, max_siblings);
        let selected =
            self.select_context_by_operator(operator, &ancestors, &siblings, target_card);
        let estimated_tokens = self.estimate_tokens(&selected);
        SynthesizedMemory {
            target: target_card.clone(),
            ancestor_insights: self.extract_insights(&selected.ancestors),
            sibling_patterns: self.extract_patterns(&selected.siblings),
            estimated_tokens,
            context_cards: selected.context,
        }
    }

    /// 算子差异化上下文选择 — OpenMLE 核心（铁律4 纯函数）
    fn select_context_by_operator<'a>(
        &self,
        operator: &AtomicOperator,
        ancestors: &[&'a ExperienceCard],
        siblings: &[&'a ExperienceCard],
        target: &ExperienceCard,
    ) -> SelectedContext<'a> {
        match operator {
            // Draft: 祖先 top-3（起草参考既有最佳实践，无兄弟）
            AtomicOperator::Draft => {
                let selected: Vec<&ExperienceCard> = ancestors.iter().take(3).copied().collect();
                SelectedContext {
                    ancestors: selected.clone(),
                    siblings: Vec::new(),
                    context: selected.into_iter().cloned().collect(),
                }
            }
            // Improve: 高 progress 祖先 + 高分成功兄弟
            AtomicOperator::Improve => {
                let mut high_progress: Vec<&ExperienceCard> = ancestors
                    .iter()
                    .filter(|c| c.three_factor.progress > 0.1)
                    .copied()
                    .collect();
                // 按 progress 降序（Top-K 规模小，sort 可接受；但遵循红线用 select_nth_unstable_by）
                high_progress.sort_unstable_by(|a, b| {
                    b.three_factor
                        .progress
                        .partial_cmp(&a.three_factor.progress)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let successful_siblings: Vec<&ExperienceCard> = siblings
                    .iter()
                    .filter(|c| c.score > 0.7)
                    .take(2)
                    .copied()
                    .collect();
                let ancestors_top: Vec<&ExperienceCard> =
                    high_progress.iter().take(3).copied().collect();
                let mut context: Vec<ExperienceCard> =
                    ancestors_top.iter().map(|c| (*c).clone()).collect();
                context.extend(successful_siblings.iter().map(|c| (*c).clone()));
                SelectedContext {
                    ancestors: ancestors_top,
                    siblings: successful_siblings,
                    context,
                }
            }
            // Debug: 同错误哈希的修复卡片（定向复用修复方案）
            AtomicOperator::Debug => {
                if let Some(ref target_sig) = target.error_signature {
                    let similar_fixes: Vec<&ExperienceCard> = siblings
                        .iter()
                        .filter(|c| {
                            c.error_signature
                                .as_ref()
                                .map(|es| es.error_hash == target_sig.error_hash)
                                .unwrap_or(false)
                        })
                        .take(3)
                        .copied()
                        .collect();
                    let context = similar_fixes.iter().map(|c| (*c).clone()).collect();
                    SelectedContext {
                        ancestors: Vec::new(),
                        siblings: similar_fixes,
                        context,
                    }
                } else {
                    SelectedContext {
                        ancestors: Vec::new(),
                        siblings: Vec::new(),
                        context: Vec::new(),
                    }
                }
            }
            // Crossover: 高新颖度兄弟 top-2（融合多样化路径）
            AtomicOperator::Crossover => {
                let mut novel_siblings: Vec<&ExperienceCard> = siblings.to_vec();
                novel_siblings.sort_unstable_by(|a, b| {
                    b.three_factor
                        .novelty
                        .partial_cmp(&a.three_factor.novelty)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let selected: Vec<&ExperienceCard> = novel_siblings.into_iter().take(2).collect();
                let context = selected.iter().map(|c| (*c).clone()).collect();
                SelectedContext {
                    ancestors: Vec::new(),
                    siblings: selected,
                    context,
                }
            }
        }
    }

    /// 查找祖先 — 沿 parent_id 回溯至 max_depth（懒加载，O(depth)）
    fn find_ancestors<'a>(
        &self,
        system: &'a ExperienceCardSystem,
        card: &ExperienceCard,
        max_depth: usize,
    ) -> Vec<&'a ExperienceCard> {
        let mut ancestors = Vec::new();
        let mut current_id = card.parent_id.as_ref();
        for _ in 0..max_depth {
            let Some(pid) = current_id else { break };
            let Some(parent) = system.get_card_by_node(pid) else {
                break;
            };
            ancestors.push(parent);
            current_id = parent.parent_id.as_ref();
        }
        ancestors
    }

    /// 查找兄弟 — 同 parent_id 的其他卡片（懒加载，取 max_count）
    fn find_siblings<'a>(
        &self,
        system: &'a ExperienceCardSystem,
        card: &ExperienceCard,
        max_count: usize,
    ) -> Vec<&'a ExperienceCard> {
        let Some(ref parent_id) = card.parent_id else {
            return Vec::new();
        };
        system
            .cards()
            .iter()
            .filter(|c| {
                c.parent_id.as_deref() == Some(parent_id.as_ref()) && c.node_id != card.node_id
            })
            .take(max_count)
            .collect()
    }

    /// 祖先洞察提取 — method_family: score/progress 摘要
    fn extract_insights(&self, cards: &[&ExperienceCard]) -> Vec<String> {
        cards
            .iter()
            .map(|c| {
                format!(
                    "{}: score={:.2}, progress={:.2}",
                    c.method_family, c.score, c.three_factor.progress
                )
            })
            .collect()
    }

    /// 兄弟模式提取 — method_family: novelty 摘要
    fn extract_patterns(&self, cards: &[&ExperienceCard]) -> Vec<String> {
        cards
            .iter()
            .map(|c| format!("{}: novelty={:.2}", c.method_family, c.three_factor.novelty))
            .collect()
    }

    /// 估算 token — 选中上下文的 token_usage 总和（懒加载预算控制）
    fn estimate_tokens(&self, selected: &SelectedContext<'_>) -> usize {
        selected
            .context
            .iter()
            .map(|c| c.metadata.token_usage.total_tokens as usize)
            .sum()
    }
}

/// 算子选择后的上下文（内部中间态）
struct SelectedContext<'a> {
    ancestors: Vec<&'a ExperienceCard>,
    siblings: Vec<&'a ExperienceCard>,
    context: Vec<ExperienceCard>,
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use nexus_contracts::experience_card::{
        CardMetadata, ErrorSignature, ExecutionStatus, ThreeFactorScore,
    };

    fn card(node: &str, parent: Option<&str>, score: f32) -> ExperienceCard {
        ExperienceCard {
            card_id: Box::from(format!("card-{node}")),
            task_id: Box::from("t1"),
            node_id: Box::from(node),
            parent_id: parent.map(Box::from),
            created_at: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("合法时间戳"),
            operator: AtomicOperator::Draft,
            score,
            delta_vs_parent: 0.0,
            method_family: Box::from(format!("fam-{node}")),
            error_signature: None,
            three_factor: ThreeFactorScore {
                quality: score,
                progress: 0.2,
                novelty: 0.5,
            },
            execution_status: ExecutionStatus::Success,
            token_evidence_ids: Vec::new(),
            segment_id: None,
            metadata: CardMetadata::default(),
        }
    }

    fn build_chain_system() -> ExperienceCardSystem {
        // 链: root → a → target；兄弟: target 与 sib1/sib2 同父 a
        let mut system = ExperienceCardSystem::new(1.414, 0.1);
        system.add_card(card("root", None, 0.5));
        system.add_card(card("a", Some("root"), 0.6));
        system.add_card(card("target", Some("a"), 0.7));
        system.add_card(card("sib1", Some("a"), 0.8));
        system.add_card(card("sib2", Some("a"), 0.9));
        system
    }

    #[test]
    fn draft_selects_ancestors_only() {
        let system = build_chain_system();
        let target = system.get_card_by_node("target").expect("存在").clone();
        let synth = OnDemandSynthesizer::new();
        let mem = synth.synthesize(&system, &target, &AtomicOperator::Draft, 3, 3);
        // Draft: 祖先 a + root，无兄弟
        assert!(!mem.ancestor_insights.is_empty(), "应包含祖先洞察");
        assert!(mem.sibling_patterns.is_empty(), "Draft 不含兄弟模式");
        assert!(!mem.context_cards.is_empty());
    }

    #[test]
    fn debug_selects_same_error_siblings() {
        let mut system = ExperienceCardSystem::new(1.414, 0.1);
        let mut target = card("target", Some("a"), 0.3);
        target.error_signature = Some(ErrorSignature {
            error_type: Box::from("compile_error"),
            error_location: Box::from("src/x.rs"),
            error_summary: Box::from("E0308"),
            error_hash: Box::from("hash-x"),
        });
        let mut fix = card("fix", Some("a"), 0.9);
        fix.error_signature = Some(ErrorSignature {
            error_type: Box::from("compile_error"),
            error_location: Box::from("src/y.rs"),
            error_summary: Box::from("E0308 fix"),
            error_hash: Box::from("hash-x"), // 同哈希
        });
        system.add_card(card("a", None, 0.5));
        system.add_card(target.clone());
        system.add_card(fix);
        let synth = OnDemandSynthesizer::new();
        let mem = synth.synthesize(&system, &target, &AtomicOperator::Debug, 3, 5);
        // Debug: 应选中同哈希的 fix 兄弟
        assert_eq!(mem.context_cards.len(), 1, "应选中 1 个同哈希修复卡片");
        assert_eq!(mem.context_cards[0].node_id.as_ref(), "fix");
    }

    #[test]
    fn debug_without_error_signature_empty() {
        let system = build_chain_system();
        let target = system.get_card_by_node("target").expect("存在").clone();
        let synth = OnDemandSynthesizer::new();
        let mem = synth.synthesize(&system, &target, &AtomicOperator::Debug, 3, 3);
        assert!(
            mem.context_cards.is_empty(),
            "无错误签名时 Debug 上下文为空"
        );
    }

    #[test]
    fn crossover_selects_high_novelty_siblings() {
        let system = build_chain_system();
        let target = system.get_card_by_node("target").expect("存在").clone();
        let synth = OnDemandSynthesizer::new();
        let mem = synth.synthesize(&system, &target, &AtomicOperator::Crossover, 3, 5);
        // Crossover: 选高新颖度兄弟 top-2
        assert!(mem.context_cards.len() <= 2);
        assert!(mem.ancestor_insights.is_empty(), "Crossover 不含祖先");
    }

    #[test]
    fn improve_selects_high_progress_and_successful_siblings() {
        let system = build_chain_system();
        let target = system.get_card_by_node("target").expect("存在").clone();
        let synth = OnDemandSynthesizer::new();
        let mem = synth.synthesize(&system, &target, &AtomicOperator::Improve, 3, 5);
        // Improve: 高分兄弟（>0.7: sib1=0.8, sib2=0.9）应被选中
        let sibling_nodes: Vec<&str> = mem
            .context_cards
            .iter()
            .map(|c| c.node_id.as_ref())
            .collect();
        assert!(
            sibling_nodes.contains(&"sib1") || sibling_nodes.contains(&"sib2"),
            "应包含高分成功兄弟"
        );
    }

    #[test]
    fn root_card_no_ancestors_no_siblings() {
        let mut system = ExperienceCardSystem::new(1.414, 0.1);
        system.add_card(card("root", None, 0.5));
        let target = system.get_card_by_node("root").expect("存在").clone();
        let synth = OnDemandSynthesizer::new();
        let mem = synth.synthesize(&system, &target, &AtomicOperator::Draft, 3, 3);
        assert!(mem.ancestor_insights.is_empty(), "根节点无祖先");
        assert!(mem.sibling_patterns.is_empty(), "根节点无兄弟");
        assert!(mem.context_cards.is_empty(), "根节点 Draft 无上下文");
    }

    #[test]
    fn token_estimation_sums_context() {
        let mut system = ExperienceCardSystem::new(1.414, 0.1);
        let mut parent = card("a", None, 0.5);
        parent.metadata.token_usage.total_tokens = 100;
        let mut target = card("target", Some("a"), 0.7);
        target.metadata.token_usage.total_tokens = 50;
        system.add_card(parent);
        system.add_card(target.clone());
        let synth = OnDemandSynthesizer::new();
        let mem = synth.synthesize(&system, &target, &AtomicOperator::Draft, 3, 3);
        // Draft 选祖先 a（token=100）
        assert_eq!(mem.estimated_tokens, 100, "估算应等于祖先 token 总和");
    }

    #[test]
    fn lazy_loading_respects_max_bounds() {
        // 铁律5: 上下文规模受 max_ancestors/max_siblings 约束
        let mut system = ExperienceCardSystem::new(1.414, 0.1);
        // 深链: root → m1 → m2 → m3 → target
        system.add_card(card("root", None, 0.5));
        system.add_card(card("m1", Some("root"), 0.5));
        system.add_card(card("m2", Some("m1"), 0.5));
        system.add_card(card("m3", Some("m2"), 0.5));
        system.add_card(card("target", Some("m3"), 0.7));
        let target = system.get_card_by_node("target").expect("存在").clone();
        let synth = OnDemandSynthesizer::new();
        // max_ancestors=2 → 只回溯 m3, m2（不到 m1/root）
        let mem = synth.synthesize(&system, &target, &AtomicOperator::Draft, 2, 0);
        assert!(
            mem.ancestor_insights.len() <= 2,
            "懒加载应受 max_ancestors 约束（实际 {})",
            mem.ancestor_insights.len()
        );
    }
}
