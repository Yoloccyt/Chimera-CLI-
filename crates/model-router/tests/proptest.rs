//! model-router 不变量属性测试 — CACR 预算检查逻辑一致性
//!
//! 对应架构层:L1 Core
//! 对应创新点:CACR(Cost-Aware Cognitive Routing)
//!
//! # 测试目标
//! 通过随机预算/成本/阈值组合,验证 CACR 决策逻辑满足三条不变量:
//! 1. **budget=0 永远 Block** — 预算耗尽时不允许任何路由
//! 2. **决策与阈值公式一致** — Block/Downgrade/Allow 与文档定义的
//!    `cost >= block_limit` / `warn_limit <= cost < block_limit` / `cost < warn_limit` 对应
//! 3. **单调性** — 成本升高时,决策严格度不变松(Allow → Downgrade → Block)
//!
//! # 语法约束(§4.4 规则)
//! proptest 1.11+ 用 block-named 语法:`fn name(arg in strategy) { body }`

#![forbid(unsafe_code)]

use model_router::{CacrConfig, CacrDecision, CacrGuard};
use proptest::prelude::*;

/// 生成 [0.0, 1.0] 范围的有限 f32(用整数映射避免 NaN/Inf)
///
/// WHY 用整数驱动:直接生成 f32 可能产生 NaN/Inf,而 CACR 阈值语义上
/// 应为有限比例值。整数 → f32 映射保证生成值有限且均匀分布。
fn prop_ratio() -> impl Strategy<Value = f32> {
    (0u32..=10_000u32).prop_map(|v| v as f32 / 10_000.0)
}

/// 决策严格度排序:Allow(0) < Downgrade(1) < Block(2)
fn decision_rank(d: &CacrDecision) -> u8 {
    match d {
        CacrDecision::Allow => 0,
        CacrDecision::Downgrade(_) => 1,
        CacrDecision::Block(_) => 2,
    }
}

proptest! {
    #[test]
    fn prop_cacr_budget_check_consistency(
        budget in 0u64..=100_000u64,
        cost in 0u64..=200_000u64,
        warn_raw in prop_ratio(),
        block_raw in prop_ratio(),
    ) {
        // 确保 warn_threshold <= block_threshold(语义合理:告警阈值应 <= 阻止阈值)
        let (warn_threshold, block_threshold) = if warn_raw <= block_raw {
            (warn_raw, block_raw)
        } else {
            (block_raw, warn_raw)
        };

        let guard = CacrGuard::new(CacrConfig {
            budget_limit: 10_000_000,
            warn_threshold,
            block_threshold,
        });

        let decision = guard.check(cost, budget);

        // === 不变量1:budget=0 永远 Block ===
        // WHY:预算为 0 时 block_limit = 0,cost >= 0 恒真,必须 Block。
        // 这是任务定义的核心语义:预算耗尽时拒绝一切路由(含零成本)。
        if budget == 0 {
            prop_assert!(
                matches!(decision, CacrDecision::Block(_)),
                "budget=0 必须 Block, got {:?}",
                decision
            );
        } else {
            // === 不变量2:决策与阈值公式一致 ===
            // 镜像 CACR::check 内部的整数阈值计算(保持 f32 精度一致)
            let warn_percent = (warn_threshold * 100.0).round() as u64;
            let block_percent = (block_threshold * 100.0).round() as u64;
            let warn_limit = budget * warn_percent / 100;
            let block_limit = budget * block_percent / 100;

            match &decision {
                CacrDecision::Block(_) => {
                    prop_assert!(
                        cost >= block_limit,
                        "Block 需 cost {} >= block_limit {} (budget {} * block {})",
                        cost, block_limit, budget, block_threshold
                    );
                }
                CacrDecision::Downgrade(_) => {
                    prop_assert!(
                        cost >= warn_limit,
                        "Downgrade 需 cost {} >= warn_limit {} (budget {} * warn {})",
                        cost, warn_limit, budget, warn_threshold
                    );
                    prop_assert!(
                        cost < block_limit,
                        "Downgrade 需 cost {} < block_limit {} (budget {} * block {})",
                        cost, block_limit, budget, block_threshold
                    );
                }
                CacrDecision::Allow => {
                    prop_assert!(
                        cost < warn_limit,
                        "Allow 需 cost {} < warn_limit {} (budget {} * warn {})",
                        cost, warn_limit, budget, warn_threshold
                    );
                }
            }

            // === 不变量3:单调性 — 成本升高,决策不变松 ===
            // 对比 cost-1 的决策:当前决策严格度应 >= 较低成本的决策严格度
            if cost > 0 {
                let lower_decision = guard.check(cost - 1, budget);
                prop_assert!(
                    decision_rank(&decision) >= decision_rank(&lower_decision),
                    "cost {} 决策 {:?} 不应松于 cost-1 决策 {:?}",
                    cost, decision, lower_decision
                );
            }
        }
    }
}
