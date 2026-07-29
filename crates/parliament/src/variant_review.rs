//! Variant Review — Harness 变体三角色审议(polish-v2.7 P3-4)
//!
//! 对应架构层:L8 Parliament(子模块)
//! 对应 ADR:ADR-051 决策 3(三角色审议:Security 一票否决 + 2/3 多数)
//! 对应设计源:`chimera_ultimate_polish_v2.7.md` §12.3(Variant Parliament)
//!
//! # 审议流程(ADR-051 决策 3)
//!
//! ```text
//! VariantContract + security_concerns
//!     │ 1. Security 审查(一票否决):安全红线触碰 → 立即 Reject
//!     │ 2. Skeptic 审查:性能低于池内现役最高 → 反对票
//!     │ 3. Execution 审查:max_regression > 0.2 → 反对票
//!     ▼
//! Security 通过 + (Skeptic/Execution ≥1 赞成) → Approve(2/3 多数)
//! ```
//!
//! # WHY security_concerns 由调用方预检传入
//!
//! 安全红线的语义判定(变体是否降低沙箱策略等)需要 L5 SpecRegistry 的
//! 不可进化面比对,L8 不反向依赖 L5(§2.2 依赖铁律);调用方(L9 编排器)
//! 聚合 L5 校验结果后以结构化 concerns 传入,L8 只做裁决。

use nexus_contracts::VariantContract;
use tracing::info;

use crate::variant_pool::VariantPool;

/// Execution 角色的回归容忍上限(ADR-051 决策 3)
///
/// WHY 0.2:允许变体承诺最多 20% 的回归空间用于探索;更高的回归容忍
/// 意味着变体自己都不确信其收益,应退回 AEGIS 重新变异。
const MAX_REGRESSION_TOLERANCE: f32 = 0.2;

/// 审议决定
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewDecision {
    /// 通过:变体可注册入池并经 SpecRegistry 走候选灰度
    Approve,
    /// 拒绝(携带拒绝原因,供审计与 AEGIS 反馈)
    Reject(String),
}

impl ReviewDecision {
    /// 是否为通过决定
    pub fn is_approved(&self) -> bool {
        matches!(self, Self::Approve)
    }
}

/// 变体审议器 — 三角色规则审议
#[derive(Debug, Default, Clone, Copy)]
pub struct VariantReview;

impl VariantReview {
    /// 创建审议器
    pub fn new() -> Self {
        Self
    }

    /// 审议变体契约(ADR-051 决策 3 三角色流程)
    ///
    /// # 参数
    /// - `contract`:待审议的变体性能契约
    /// - `pool`:变体池(Skeptic 的现役性能基准来源)
    /// - `security_concerns`:调用方预检出的安全关切(非空即一票否决)
    pub fn review(
        &self,
        contract: &VariantContract,
        pool: &VariantPool,
        security_concerns: &[String],
    ) -> ReviewDecision {
        // 角色 1:Security 一票否决(优先级最高,不进入投票)
        if !security_concerns.is_empty() {
            return ReviewDecision::Reject(format!(
                "Security 一票否决: {}",
                security_concerns.join("; ")
            ));
        }

        // 角色 2:Skeptic — 性能不得低于池内同域现役最高(避免劣币入池)
        // WHY 用第一个 task_type 作基准域:契约的主任务类型;通用变体(空)与全池比
        let benchmark_domain = contract
            .task_types
            .first()
            .map(String::as_str)
            .unwrap_or("");
        let skeptic_approves = match pool.best_performance_for(benchmark_domain) {
            // 池内已有更强现役 → Skeptic 反对
            Some(best) => contract.expected_performance >= best,
            // 空池/新域 → Skeptic 赞成(无基准可比,鼓励覆盖新域)
            None => true,
        };

        // 角色 3:Execution — 回归容忍上限守护
        let execution_approves = contract.max_regression <= MAX_REGRESSION_TOLERANCE;

        // 裁决:Security 已通过(计 1 赞成),Skeptic/Execution ≥1 赞成即 2/3 多数
        if skeptic_approves || execution_approves {
            info!(
                variant = %contract.variant_id,
                skeptic = skeptic_approves,
                execution = execution_approves,
                "变体审议通过(2/3 多数)"
            );
            ReviewDecision::Approve
        } else {
            ReviewDecision::Reject(format!(
                "2/3 多数未达成: Skeptic 反对(性能 {:.2} 低于现役), Execution 反对(回归容忍 {:.2} > {MAX_REGRESSION_TOLERANCE})",
                contract.expected_performance, contract.max_regression
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::VariantId;

    fn contract(perf: f32, regression: f32) -> VariantContract {
        VariantContract::new(
            VariantId::new("candidate", 2),
            vec!["code_fix".into()],
            perf,
            regression,
        )
    }

    #[test]
    fn test_security_veto_rejects_immediately() {
        let review = VariantReview::new();
        let pool = VariantPool::new();
        // 即使性能/回归全优,安全关切非空即否决
        let decision = review.review(
            &contract(0.99, 0.01),
            &pool,
            &["变体试图降低沙箱策略".to_string()],
        );
        assert!(!decision.is_approved());
        match decision {
            ReviewDecision::Reject(reason) => assert!(reason.contains("Security 一票否决")),
            _ => panic!("应为 Reject"),
        }
    }

    #[test]
    fn test_approve_on_empty_pool_with_good_contract() {
        let review = VariantReview::new();
        let pool = VariantPool::new();
        // 空池:Skeptic 赞成(新域),Execution 赞成(回归 0.1 ≤ 0.2)
        let decision = review.review(&contract(0.7, 0.1), &pool, &[]);
        assert!(decision.is_approved());
    }

    #[test]
    fn test_reject_when_both_skeptic_and_execution_oppose() {
        let review = VariantReview::new();
        let mut pool = VariantPool::new();
        // 池内现役性能 0.9
        pool.register(VariantContract::new(
            VariantId::new("incumbent", 1),
            vec!["code_fix".into()],
            0.9,
            0.1,
        ));
        // 候选性能 0.5 < 0.9(Skeptic 反对)且回归容忍 0.5 > 0.2(Execution 反对)
        let decision = review.review(&contract(0.5, 0.5), &pool, &[]);
        assert!(!decision.is_approved());
    }

    #[test]
    fn test_approve_with_one_dissenting_role() {
        let review = VariantReview::new();
        let mut pool = VariantPool::new();
        pool.register(VariantContract::new(
            VariantId::new("incumbent", 1),
            vec!["code_fix".into()],
            0.9,
            0.1,
        ));
        // 性能 0.5 < 0.9(Skeptic 反对)但回归 0.1 合规(Execution 赞成)→ 2/3 通过
        let decision = review.review(&contract(0.5, 0.1), &pool, &[]);
        assert!(decision.is_approved());
    }
}
