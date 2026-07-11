//! csn-substitutor 属性测试 — 降级链顺序不变量
//!
//! 对应架构层:L10 Interface
//! 对应 SubTask 13.2:v1.5.0-omega 发布就绪差距闭合
//!
//! # 测试覆盖的不变量
//! 1. DegradationChain::new 创建后 current_level == 0,total_levels == levels.len()
//! 2. next_level 单调推进,到达末端返回 ChainExhausted(且层级保持不变)
//! 3. reset 后 current_level == 0,可重新推进
//! 4. cosine_similarity 对称:cos(a, b) == cos(b, a)
//! 5. cosine_similarity 结果 ∈ [-1.0, 1.0]
//!
//! # 设计要点
//! - 使用整数策略生成 f32 向量(避免 NaN/Inf)
//! - block-named 语法(§4.1 规范)
//! - 256 cases(proptest 默认)

#![forbid(unsafe_code)]

use csn_substitutor::similarity::cosine_similarity;
use csn_substitutor::{CsnError, DegradationChain};
use proptest::prelude::*;

/// 生成非空 levels 列表(1..=10 个层级)
/// WHY 非空:DegradationChain::new 在 levels 为空时填充占位符,
/// 为精确测试 total_levels 不变量,使用非空输入。
fn levels_strategy() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec("[a-z][a-z0-9]{0,7}", 1..=10)
}

/// 生成 f32 向量(维度 1..=20,值域 [0.0, 1.0])
/// WHY 整数策略:避免 proptest 浮点策略生成 NaN/Inf 污染余弦计算
fn vector_strategy() -> impl Strategy<Value = Vec<f32>> {
    prop::collection::vec(0u32..=1000, 1..=20)
        .prop_map(|v| v.into_iter().map(|x| x as f32 / 1000.0).collect())
}

proptest! {
    /// 不变量 1:new 创建后 current_level == 0,total_levels == levels.len()
    ///
    /// 验证降级链的初始状态不变量:起点为 level 0,层级数与输入一致。
    #[test]
    fn prop_chain_new_initial_state(levels in levels_strategy()) {
        let expected_len = levels.len();
        let chain = DegradationChain::new("chain-test", levels.clone());

        prop_assert_eq!(chain.current_level(), 0, "初始 current_level 应为 0");
        prop_assert_eq!(chain.total_levels(), expected_len, "total_levels 应等于 levels.len()");
        prop_assert_eq!(chain.chain_id(), "chain-test");
        // current_target 应为 levels[0](原始能力)
        prop_assert_eq!(chain.current_target(), &levels[0]);
    }

    /// 不变量 2:next_level 单调推进,末端返回 ChainExhausted(层级保持不变)
    ///
    /// 对长度为 N 的降级链,前 N-1 次 next_level 成功,current_level 从 0 升至 N-1;
    /// 第 N 次 next_level 返回 ChainExhausted,current_level 保持 N-1。
    #[test]
    fn prop_next_level_advances_then_exhausts(levels in levels_strategy()) {
        let mut chain = DegradationChain::new("chain-test", levels.clone());
        let n = levels.len();

        // 前 n-1 次成功推进
        for i in 0..(n.saturating_sub(1)) {
            let result = chain.next_level();
            prop_assert!(
                result.is_ok(),
                "第 {} 次 next_level 应成功(链长 {})",
                i + 1,
                n
            );
            prop_assert_eq!(chain.current_level(), i + 1);
        }

        // 第 n 次应返回 ChainExhausted
        let exhausted_level = chain.current_level();
        let result = chain.next_level();
        prop_assert!(
            matches!(result, Err(CsnError::ChainExhausted { .. })),
            "末端 next_level 应返回 ChainExhausted"
        );
        // 层级保持不变(§4.1:失败路径不修改状态)
        prop_assert_eq!(
            chain.current_level(),
            exhausted_level,
            "ChainExhausted 后 current_level 应保持不变"
        );
        // is_exhausted 应为 true
        prop_assert!(chain.is_exhausted(), "末端 is_exhausted 应为 true");
    }

    /// 不变量 3:reset 后 current_level == 0,可重新推进
    ///
    /// 验证 reset 是幂等的:无论当前层级多少,reset 后回到 0,
    /// 且 reset 后可再次推进(降级链状态可重用)。
    #[test]
    fn prop_reset_returns_to_zero(levels in levels_strategy()) {
        let mut chain = DegradationChain::new("chain-test", levels.clone());
        let n = levels.len();

        // 推进若干级(最多 n-1 级,避免触发 ChainExhausted)
        if n > 1 {
            for _ in 0..(n - 1) {
                chain.next_level().ok();
            }
            // 此时 current_level == n - 1
            prop_assert_eq!(chain.current_level(), n - 1);
        }

        // reset
        chain.reset();
        prop_assert_eq!(chain.current_level(), 0, "reset 后 current_level 应为 0");
        prop_assert_eq!(chain.current_target(), &levels[0]);

        // reset 后可重新推进(若 n > 1)
        if n > 1 {
            let result = chain.next_level();
            prop_assert!(result.is_ok(), "reset 后 next_level 应可成功");
            prop_assert_eq!(chain.current_level(), 1);
        }
    }

    /// 不变量 4:cosine_similarity 对称:cos(a, b) == cos(b, a)
    ///
    /// 余弦相似度是内积的归一化,数学上必然对称。
    /// WHY 测试对称性:实现中若点积或模长计算顺序不一致可能引入误差。
    #[test]
    fn prop_cosine_similarity_symmetric(
        a in vector_strategy(),
        b in vector_strategy_with_dim()
    ) {
        // 仅在维度匹配时测试对称性(维度不匹配返回 0.0,对称平凡成立)
        if a.len() == b.len() {
            let score_ab = cosine_similarity(&a, &b);
            let score_ba = cosine_similarity(&b, &a);
            // 浮点比较:差值 < 1e-5
            prop_assert!(
                (score_ab - score_ba).abs() < 1e-5,
                "余弦相似度应对称: cos(a,b)={}, cos(b,a)={}",
                score_ab,
                score_ba
            );
        }
    }

    /// 不变量 5:cosine_similarity 结果 ∈ [-1.0, 1.0]
    ///
    /// 数学保证:cos(θ) ∈ [-1, 1]。实现应保证此不变量,即使输入含零向量。
    #[test]
    fn prop_cosine_similarity_bounded(
        a in vector_strategy(),
        b in vector_strategy_with_dim()
    ) {
        let score = cosine_similarity(&a, &b);
        prop_assert!(
            (-1.0..=1.0).contains(&score),
            "余弦相似度应 ∈ [-1.0, 1.0],实际 {}",
            score
        );
        // NaN 检查(零向量保护应避免 NaN)
        prop_assert!(
            !score.is_nan(),
            "余弦相似度不应为 NaN(零向量应返回 0.0)"
        );
    }
}

/// 生成与主策略独立维度的向量,用于对称性测试
fn vector_strategy_with_dim() -> impl Strategy<Value = Vec<f32>> {
    prop::collection::vec(0u32..=1000, 1..=20)
        .prop_map(|v| v.into_iter().map(|x| x as f32 / 1000.0).collect())
}
