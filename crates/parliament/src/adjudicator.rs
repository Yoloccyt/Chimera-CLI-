//! 三因子裁决器 + 停止策略 — 小米 + OpenMLE 融合（设计文档 §13.1）
//!
//! 对应架构层: **L8 Parliament**（parliament 子模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §13.1
//! 对应论文: 小米（变体审议三角色）+ 清华 OpenMLE（三因子裁决 + RSIBench 停止策略）
//! 对应 ADR: ADR-049 决策 1（规范字面独立 crate three-factor-adjudicator，内嵌落点）
//!
//! # 核心职责
//!
//! - **三角色审议投票**: Skeptic（进步门控）/ Security（回归一票否决）/
//!   Execution（质量门控含弃权带），三态投票 Approve/Reject/Abstain
//! - **三因子裁决**: quality + progress + novelty 消费 L0 [`ThreeFactorScore`]
//!   契约（铁律4 纯函数）
//! - **停止策略裁决**（RSIBench）: max_attempts / stagnation / score_gap /
//!   后期算子切换建议，保留历史最佳（Ω₉-Preserve）
//!
//! # 落层偏差记录（规范原型适配）
//!
//! 1. 原型引用 `variant_pool::{HarnessVariant, PerformanceRecord}` 不存在
//!    → 裁决输入为自足类型 [`VariantPerformance`]（ADR-051: 池只存契约引用，
//!    变体本体在 L5 SpecRegistry；不耦合 L5 内部类型）
//! 2. 原型引用 `nexus_contracts::{ParliamentDecision, Vote}` 但 L0 无此类型
//!    → 定义在本模块（避免 L0 契约膨胀，单消费方无跨 crate 共享需求）
//! 3. `process_score: Option<f32>` 为 L7 [`TrajectoryProcessScore`] 协同的
//!    数值松耦合输入（调用方填充 overall()，不引入 L8→L7 依赖边）
//!
//! # 语义边界（与既有模块）
//!
//! - `variant_review.rs` 双态快速审查（Approve/Reject）保持零改动
//! - 本模块三态完整裁决（含 Abstain/RequestMoreData），两者并存职责分离

use nexus_contracts::experience_card::{AtomicOperator, ThreeFactorScore};
use nexus_contracts::VariantId;

/// 议会投票 — 三态语义（规范 §13.1 Vote）
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Vote {
    /// 批准
    Approve,
    /// 拒绝
    Reject,
    /// 弃权（证据不足但未达拒绝标准）
    Abstain,
}

/// 议会决策（规范 §13.1 ParliamentDecision）
#[derive(Clone, Debug, PartialEq)]
pub enum ParliamentDecision {
    /// 批准变体注册
    Approve,
    /// 拒绝（携带原因，供审计与 AEGIS 反馈）
    Reject(String),
    /// 证据不足，请求更多数据
    RequestMoreData(String),
}

/// 变体性能输入 — 裁决输入自足类型（D-3 偏差适配）
///
/// WHY 自足类型：规范原型 `HarnessVariant` 不存在，ADR-051 下变体本体在
/// L5 SpecRegistry，裁决仅需性能摘要。
#[derive(Clone, Debug)]
pub struct VariantPerformance {
    /// 变体标识（L0 VariantContract 主键）
    pub variant_id: VariantId,
    /// 平均评分（quality 因子来源）
    pub avg_score: f32,
    /// 历史评分序列（novelty history_bonus 来源）
    pub history_scores: Vec<f32>,
    /// 配置哈希（novelty config_diff 来源，与基线比较）
    pub config_hash: u64,
    /// L7 过程评分（可选，调用方从 TrajectoryProcessScore.overall() 填充，D-5）
    pub process_score: Option<f32>,
}

/// 烟雾测试结果（规范 §13.1 SmokeResults）
#[derive(Clone, Debug)]
pub struct SmokeResults {
    /// 通过测试数
    pub tests_passed: u32,
    /// 失败测试数
    pub tests_failed: u32,
    /// 是否检测到回归
    pub has_regression: bool,
    /// 回归详情列表
    pub regression_details: Vec<String>,
}

/// 裁决结果（规范 §13.1 AdjudicationResult）
#[derive(Clone, Debug)]
pub struct AdjudicationResult {
    /// 三因子评分（消费 L0 契约）
    pub three_factor: ThreeFactorScore,
    /// 三角色投票记录（角色名 + 投票）
    pub votes: Vec<(String, Vote)>,
    /// 议会决策
    pub decision: ParliamentDecision,
    /// 裁决推理摘要（可审计）
    pub reasoning: String,
}

/// 三因子裁决器 — 三角色审议 + 停止策略（规范 §13.1）
#[derive(Clone, Debug)]
pub struct ThreeFactorAdjudicator {
    /// Skeptic 进步阈值（progress > 阈值 → Approve）
    pub skeptic_threshold: f32,
    /// Security 回归容忍外的质量阈值（保留配置对称性）
    pub security_threshold: f32,
    /// Execution 质量阈值（quality > 阈值 → Approve；> 0.8×阈值 → Abstain）
    pub execution_threshold: f32,
    /// 回归容忍度（avg_score ≥ baseline × (1 - 容忍度) → 无回归）
    pub regression_tolerance: f32,
}

impl ThreeFactorAdjudicator {
    /// 创建裁决器（四阈值配置）
    pub fn new(
        skeptic_threshold: f32,
        security_threshold: f32,
        execution_threshold: f32,
        regression_tolerance: f32,
    ) -> Self {
        Self {
            skeptic_threshold,
            security_threshold,
            execution_threshold,
            regression_tolerance,
        }
    }

    /// 裁决变体 — 三因子计算 + 三角色投票 + 决策（铁律4 纯函数）
    ///
    /// 决策优先级：Security Reject（回归一票否决）→ reject≥2 →
    /// approve≥2 → RequestMoreData（证据不足）
    pub fn adjudicate_variant(
        &self,
        variant: &VariantPerformance,
        baseline: &VariantPerformance,
        smoke_results: &SmokeResults,
    ) -> AdjudicationResult {
        // 三因子计算（规范 §13.1 compute 语义：progress 取 vs 基线差值）
        let quality_delta = variant.avg_score - baseline.avg_score;
        let novelty = self.compute_variant_novelty(variant, baseline);
        let three_factor = ThreeFactorScore {
            quality: variant.avg_score,
            progress: quality_delta,
            novelty,
        };

        // 角色 1: Skeptic — 进步门控
        let skeptic_vote = if three_factor.progress > self.skeptic_threshold {
            Vote::Approve
        } else if three_factor.progress > 0.0 {
            Vote::Abstain
        } else {
            Vote::Reject
        };
        // 角色 2: Security — 回归一票否决（优先级最高）
        let security_vote = if smoke_results.has_regression {
            Vote::Reject
        } else if variant.avg_score >= baseline.avg_score * (1.0 - self.regression_tolerance) {
            Vote::Approve
        } else {
            Vote::Reject
        };
        // 角色 3: Execution — 质量门控（含 0.8× 弃权带）
        let execution_vote = if three_factor.quality > self.execution_threshold {
            Vote::Approve
        } else if three_factor.quality > self.execution_threshold * 0.8 {
            Vote::Abstain
        } else {
            Vote::Reject
        };

        let votes = vec![
            ("Skeptic".to_string(), skeptic_vote.clone()),
            ("Security".to_string(), security_vote.clone()),
            ("Execution".to_string(), execution_vote.clone()),
        ];

        // 决策合成（规范优先级）
        let decision = if security_vote == Vote::Reject {
            ParliamentDecision::Reject("Security: regression detected".to_string())
        } else {
            let approve_count = votes.iter().filter(|(_, v)| *v == Vote::Approve).count();
            let reject_count = votes.iter().filter(|(_, v)| *v == Vote::Reject).count();
            if reject_count >= 2 {
                ParliamentDecision::Reject("Insufficient support".to_string())
            } else if approve_count >= 2 {
                ParliamentDecision::Approve
            } else {
                ParliamentDecision::RequestMoreData("Need more evidence".to_string())
            }
        };

        let reasoning = format!(
            "Q: {:.2}, P: {:.2}, N: {:.2}. Skeptic={:?}, Security={:?}, Execution={:?}",
            three_factor.quality,
            three_factor.progress,
            three_factor.novelty,
            skeptic_vote,
            security_vote,
            execution_vote
        );
        AdjudicationResult {
            three_factor,
            votes,
            decision,
            reasoning,
        }
    }

    /// 停止策略裁决 — RSIBench 四分支（规范 §13.1 adjudicate_stop）
    ///
    /// 优先级：max_attempts → stagnation → score_gap（attempts>10）→
    /// 后期算子切换建议（attempts>20 且非 Crossover/Improve）→ Continue
    pub fn adjudicate_stop(&self, context: &StopContext) -> StopRuling {
        if context.attempts >= context.max_attempts {
            return StopRuling::Stop {
                reason: format!("Max attempts ({}) reached", context.max_attempts),
                preserve_best: true,
                selected_checkpoint: context.best_checkpoint.clone(),
            };
        }
        if context.stagnation_count >= context.stagnation_threshold {
            return StopRuling::Stop {
                reason: format!(
                    "Stagnation: {} attempts without improvement",
                    context.stagnation_count
                ),
                preserve_best: true,
                selected_checkpoint: context.best_checkpoint.clone(),
            };
        }
        if context.attempts > 10 {
            if let Some(ref best) = context.best_checkpoint {
                if best.score > 0.0 {
                    let gap = context.current_score / best.score;
                    if gap < context.score_gap_threshold {
                        return StopRuling::Stop {
                            reason: format!(
                                "Current {:.2} below best {:.2}, ratio={:.2}",
                                context.current_score, best.score, gap
                            ),
                            preserve_best: true,
                            selected_checkpoint: Some(best.clone()),
                        };
                    }
                }
            }
        }
        if context.attempts > 20
            && !matches!(
                context.current_operator,
                AtomicOperator::Crossover | AtomicOperator::Improve
            )
        {
            return StopRuling::SuggestSwitch {
                suggested: AtomicOperator::Crossover,
                reason: "Late-stage: switch to Crossover/Improve".to_string(),
            };
        }
        StopRuling::Continue
    }

    /// 变体新颖度 — config_diff + history_bonus（规范 §13.1，铁律4 纯函数）
    fn compute_variant_novelty(
        &self,
        variant: &VariantPerformance,
        baseline: &VariantPerformance,
    ) -> f32 {
        let config_diff = if variant.config_hash == baseline.config_hash {
            0.0
        } else {
            0.5
        };
        let history_bonus = (variant.history_scores.len() as f32 / 100.0).min(0.5);
        (config_diff + history_bonus).min(1.0)
    }
}

/// 停止检查点 — 自足类型（D-3 偏差适配，规范 BestCheckpoint 不存在）
///
/// 与 L5 `gsoe_evolution::Checkpoint` 经调用方转换接线（不新增 L8→L5 依赖边）。
#[derive(Clone, Debug, PartialEq)]
pub struct StopCheckpoint {
    /// 任务类型
    pub task_type: String,
    /// 检查点评分
    pub score: f32,
    /// 元数据（检查点标识/谱系等）
    pub metadata: String,
}

/// 停止裁决上下文（规范 §13.1 StopContext）
#[derive(Clone, Debug)]
pub struct StopContext {
    /// 当前尝试次数
    pub attempts: u32,
    /// 最大尝试次数
    pub max_attempts: u32,
    /// 连续无改进次数
    pub stagnation_count: u32,
    /// 停滞阈值
    pub stagnation_threshold: u32,
    /// 当前评分
    pub current_score: f32,
    /// 历史最佳评分
    pub best_score: f32,
    /// 分数差距阈值（current/best < 阈值 → 停止）
    pub score_gap_threshold: f32,
    /// 当前算子
    pub current_operator: AtomicOperator,
    /// 历史最佳检查点（Ω₉-Preserve 保留输入）
    pub best_checkpoint: Option<StopCheckpoint>,
}

/// 停止裁决（规范 §13.1 StopRuling）
#[derive(Clone, Debug, PartialEq)]
pub enum StopRuling {
    /// 继续进化
    Continue,
    /// 停止（保留历史最佳，Ω₉-Preserve）
    Stop {
        /// 停止原因（可审计）
        reason: String,
        /// 是否保留最佳检查点
        preserve_best: bool,
        /// 选中的检查点
        selected_checkpoint: Option<StopCheckpoint>,
    },
    /// 建议切换算子（后期探索耗尽）
    SuggestSwitch {
        /// 建议算子
        suggested: AtomicOperator,
        /// 建议原因
        reason: String,
    },
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn adjudicator() -> ThreeFactorAdjudicator {
        ThreeFactorAdjudicator::new(0.05, 0.6, 0.6, 0.1)
    }

    fn variant(avg_score: f32, config_hash: u64, history_len: usize) -> VariantPerformance {
        VariantPerformance {
            variant_id: VariantId::new("spec-a", 1),
            avg_score,
            history_scores: vec![avg_score; history_len],
            config_hash,
            process_score: None,
        }
    }

    fn smoke(has_regression: bool) -> SmokeResults {
        SmokeResults {
            tests_passed: 10,
            tests_failed: if has_regression { 2 } else { 0 },
            has_regression,
            regression_details: if has_regression {
                vec!["test_x regressed".into()]
            } else {
                Vec::new()
            },
        }
    }

    fn stop_context() -> StopContext {
        StopContext {
            attempts: 5,
            max_attempts: 50,
            stagnation_count: 0,
            stagnation_threshold: 5,
            current_score: 0.8,
            best_score: 0.9,
            score_gap_threshold: 0.95,
            current_operator: AtomicOperator::Draft,
            best_checkpoint: Some(StopCheckpoint {
                task_type: "t1".into(),
                score: 0.9,
                metadata: "ckpt-1".into(),
            }),
        }
    }

    #[test]
    fn approve_with_two_votes() {
        // 变体优于基线 + 无回归 + 高质量 → Skeptic/Security/Execution 全 Approve
        let result = adjudicator().adjudicate_variant(
            &variant(0.8, 1, 10),
            &variant(0.6, 1, 10),
            &smoke(false),
        );
        assert_eq!(result.decision, ParliamentDecision::Approve);
        assert_eq!(result.votes.len(), 3);
    }

    #[test]
    fn security_regression_veto() {
        // Security 一票否决（即使其他角色批准）
        let result = adjudicator().adjudicate_variant(
            &variant(0.9, 1, 10),
            &variant(0.6, 1, 10),
            &smoke(true),
        );
        assert_eq!(
            result.decision,
            ParliamentDecision::Reject("Security: regression detected".to_string())
        );
    }

    #[test]
    fn insufficient_support_rejects() {
        // Security Approve（0.4 ≥ 0.4×0.9）但 Skeptic（progress=0）与
        // Execution（quality 0.4 ≤ 0.48）双 Reject → Insufficient support
        let result = adjudicator().adjudicate_variant(
            &variant(0.4, 1, 0),
            &variant(0.4, 1, 10),
            &smoke(false),
        );
        assert_eq!(
            result.decision,
            ParliamentDecision::Reject("Insufficient support".to_string())
        );
    }

    #[test]
    fn security_quality_drop_veto() {
        // Security 因质量骤降 Reject（未达回归容忍）→ Security 原因拒绝
        let result = adjudicator().adjudicate_variant(
            &variant(0.3, 1, 0),
            &variant(0.6, 1, 10),
            &smoke(false),
        );
        assert_eq!(
            result.decision,
            ParliamentDecision::Reject("Security: regression detected".to_string())
        );
    }

    #[test]
    fn request_more_data_on_abstain_majority() {
        // progress ∈ (0, 阈值] → Skeptic Abstain；质量在弃权带 → Execution Abstain
        let adj = ThreeFactorAdjudicator::new(0.5, 0.6, 0.9, 0.1);
        let v = variant(0.8, 1, 1);
        // progress = quality_delta = 0.8-0.79 = 0.01 ∈ (0, 0.5] → Skeptic Abstain
        let result = adj.adjudicate_variant(&v, &variant(0.79, 1, 0), &smoke(false));
        assert!(matches!(
            result.decision,
            ParliamentDecision::RequestMoreData(_)
        ));
    }

    #[test]
    fn three_factor_pure_function() {
        // 铁律4: 同输入同输出
        let v = variant(0.8, 1, 10);
        let b = variant(0.6, 2, 10);
        let r1 = adjudicator().adjudicate_variant(&v, &b, &smoke(false));
        let r2 = adjudicator().adjudicate_variant(&v, &b, &smoke(false));
        assert_eq!(r1.three_factor, r2.three_factor);
        assert_eq!(r1.decision, r2.decision);
    }

    #[test]
    fn novelty_config_diff_and_history_bonus() {
        let adj = adjudicator();
        // 不同配置 + 50 条历史 → 0.5 + 0.5 = 1.0
        let v = variant(0.8, 2, 50);
        let b = variant(0.6, 1, 0);
        let result = adj.adjudicate_variant(&v, &b, &smoke(false));
        assert!((result.three_factor.novelty - 1.0).abs() < 1e-6);
        // 相同配置 + 空历史 → 0.0
        let v2 = variant(0.8, 1, 0);
        let b2 = variant(0.6, 1, 0);
        let result2 = adj.adjudicate_variant(&v2, &b2, &smoke(false));
        assert!(result2.three_factor.novelty.abs() < 1e-6);
    }

    #[test]
    fn stop_on_max_attempts() {
        let mut ctx = stop_context();
        ctx.attempts = 50;
        let ruling = adjudicator().adjudicate_stop(&ctx);
        assert!(matches!(
            ruling,
            StopRuling::Stop {
                preserve_best: true,
                ..
            }
        ));
    }

    #[test]
    fn stop_on_stagnation() {
        let mut ctx = stop_context();
        ctx.stagnation_count = 5;
        let ruling = adjudicator().adjudicate_stop(&ctx);
        assert!(matches!(ruling, StopRuling::Stop { .. }));
    }

    #[test]
    fn stop_on_score_gap_after_10_attempts() {
        let mut ctx = stop_context();
        ctx.attempts = 11; // > 10 触发 score_gap 检查
        ctx.current_score = 0.5; // 0.5/0.9 ≈ 0.56 < 0.95
        let ruling = adjudicator().adjudicate_stop(&ctx);
        assert!(matches!(ruling, StopRuling::Stop { .. }));
    }

    #[test]
    fn suggest_switch_after_20_attempts() {
        let mut ctx = stop_context();
        ctx.attempts = 21; // > 20 且 < max_attempts
        ctx.max_attempts = 100;
        ctx.stagnation_count = 0;
        ctx.current_score = 0.9; // 0.9/0.9 = 1.0 ≥ 0.95，不触发 score_gap
        ctx.current_operator = AtomicOperator::Draft; // 非 Crossover/Improve
        let ruling = adjudicator().adjudicate_stop(&ctx);
        assert!(matches!(
            ruling,
            StopRuling::SuggestSwitch {
                suggested: AtomicOperator::Crossover,
                ..
            }
        ));
    }

    #[test]
    fn continue_when_no_stop_condition() {
        let ctx = stop_context();
        assert_eq!(adjudicator().adjudicate_stop(&ctx), StopRuling::Continue);
    }

    #[test]
    fn preserve_best_checkpoint_selected() {
        let mut ctx = stop_context();
        ctx.attempts = 50;
        let ruling = adjudicator().adjudicate_stop(&ctx);
        if let StopRuling::Stop {
            selected_checkpoint,
            preserve_best,
            ..
        } = ruling
        {
            assert!(preserve_best, "Ω₉-Preserve 保留历史最佳");
            let ckpt = selected_checkpoint.expect("检查点应携带");
            assert_eq!(ckpt.metadata, "ckpt-1");
        } else {
            panic!("应为 Stop 裁决");
        }
    }
}
