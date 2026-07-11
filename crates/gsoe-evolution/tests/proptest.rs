//! gsoe-evolution 属性测试 — 进化策略选择不变量
//!
//! 对应架构层:L5 Knowledge
//! 对应 SubTask 13.7:v1.5.0-omega 发布就绪差距闭合
//!
//! # 测试覆盖的不变量
//! 1. EvolutionPolicy::new 校验:migration_rate ∈ [0,1]、selection_pressure ≥ 0、
//!    elite_ratio ∈ [0,1]、rollout_count ≥ 2
//! 2. compute_correctness_reward 对任意动作序列返回 [0, 1]
//! 3. compute_process_reward 对任意动作序列返回 [0, 1]
//! 4. mutate 拒绝 rate 越界(< 0 或 > 1)
//! 5. apply_mutation 将 mutation_rate clamp 到 [0.001, 1.0],selection_pressure ≥ 0
//!
//! # 设计要点
//! - 使用整数策略生成 f32 参数(避免 NaN/Inf)
//! - block-named 语法(§4.1 规范)
//! - 256 cases(proptest 默认)

#![forbid(unsafe_code)]

use gsoe_evolution::{
    apply_mutation, compute_correctness_reward, compute_process_reward, mutate, EvolutionPolicy,
    MutationCandidate, MutationType,
};
use proptest::prelude::*;

/// 生成 [0, 2] 区间的 f32(精度 1/1000),用于测试越界
fn ratio_extended_strategy() -> impl Strategy<Value = f32> {
    (0u32..=2000).prop_map(|x| x as f32 / 1000.0)
}

/// 生成非负 f32(精度 1/1000),范围 [0, 5]
fn non_negative_strategy() -> impl Strategy<Value = f32> {
    (0u32..=5000).prop_map(|x| x as f32 / 1000.0)
}

proptest! {
    /// 不变量 1:EvolutionPolicy::new 校验各字段合法范围
    ///
    /// - mutation_rate ∈ [0, 1] → Ok,否则 InvalidPolicy
    /// - selection_pressure ≥ 0 → Ok,否则 InvalidPolicy
    /// - elite_ratio ∈ [0, 1] → Ok,否则 InvalidPolicy
    /// - rollout_count ≥ 2 → Ok,否则 InvalidPolicy
    #[test]
    fn prop_policy_new_validation(
        mutation_rate in ratio_extended_strategy(),
        selection_pressure in non_negative_strategy(),
        elite_ratio in ratio_extended_strategy(),
        rollout_count in 1u32..=10,
    ) {
        let result = EvolutionPolicy::new(
            mutation_rate,
            selection_pressure,
            elite_ratio,
            rollout_count,
        );

        let mutation_valid = (0.0..=1.0).contains(&mutation_rate);
        let pressure_valid = selection_pressure >= 0.0;
        let elite_valid = (0.0..=1.0).contains(&elite_ratio);
        let rollout_valid = rollout_count >= 2;

        if mutation_valid && pressure_valid && elite_valid && rollout_valid {
            prop_assert!(
                result.is_ok(),
                "合法参数应通过校验: mr={}, sp={}, er={}, rc={}",
                mutation_rate,
                selection_pressure,
                elite_ratio,
                rollout_count
            );
        } else {
            prop_assert!(
                result.is_err(),
                "非法参数应被拒绝: mr={} (valid={}), sp={} (valid={}), er={} (valid={}), rc={} (valid={})",
                mutation_rate, mutation_valid,
                selection_pressure, pressure_valid,
                elite_ratio, elite_valid,
                rollout_count, rollout_valid
            );
        }
    }

    /// 不变量 2:compute_correctness_reward 对任意动作序列返回 [0, 1]
    ///
    /// correctness = 1.0 - mean(|a_i - optimal|),clamp 到 [0, 1]。
    /// 空序列返回 0.0(边界)。
    /// WHY 必须测试 [0, 1] 范围:适应度评分参与 GRPO 策略更新,
    /// 越界会导致策略梯度爆炸(§6.1 红线)。
    #[test]
    fn prop_correctness_reward_bounded(
        actions in prop::collection::vec(-1000i32..=3000, 0..=20),
        optimal_milli in 0u32..=2000,
    ) {
        let actions_f32: Vec<f32> = actions
            .iter()
            .map(|&x| x as f32 / 1000.0)
            .collect();
        let optimal = optimal_milli as f32 / 1000.0;

        let reward = compute_correctness_reward(&actions_f32, optimal);

        prop_assert!(
            (0.0..=1.0).contains(&reward),
            "correctness_reward 应 ∈ [0, 1],实际 {} (actions={:?}, optimal={})",
            reward,
            actions_f32,
            optimal
        );
        prop_assert!(!reward.is_nan(), "correctness_reward 不应为 NaN");

        // 空序列应返回 0.0
        if actions_f32.is_empty() {
            prop_assert!(
                (reward - 0.0).abs() < 1e-6,
                "空序列应返回 0.0,实际 {}",
                reward
            );
        }
    }

    /// 不变量 3:compute_process_reward 对任意动作序列返回 [0, 1]
    ///
    /// process_reward = (len_bonus * 0.5 + consistency_bonus * 0.5).clamp(0, 1)。
    /// 空序列返回 1.0(边界,无动作即完美一致)。
    #[test]
    fn prop_process_reward_bounded(
        actions in prop::collection::vec(-1000i32..=3000, 0..=20),
    ) {
        let actions_f32: Vec<f32> = actions
            .iter()
            .map(|&x| x as f32 / 1000.0)
            .collect();

        let reward = compute_process_reward(&actions_f32);

        prop_assert!(
            (0.0..=1.0).contains(&reward),
            "process_reward 应 ∈ [0, 1],实际 {} (actions={:?})",
            reward,
            actions_f32
        );
        prop_assert!(!reward.is_nan(), "process_reward 不应为 NaN");

        // 空序列应返回 1.0
        if actions_f32.is_empty() {
            prop_assert!(
                (reward - 1.0).abs() < 1e-6,
                "空序列应返回 1.0,实际 {}",
                reward
            );
        }
    }

    /// 不变量 4:mutate 拒绝 rate 越界(< 0 或 > 1)
    ///
    /// - rate ∈ [0, 1] → Ok(MutationCandidate)
    /// - rate < 0 或 rate > 1 → Err(MutationFailed)
    #[test]
    fn prop_mutate_rejects_invalid_rate(
        rate_milli in 0u32..=2000,
    ) {
        let policy = EvolutionPolicy::new(0.1, 1.5, 0.2, 8).expect("默认策略应有效");
        let rate = rate_milli as f32 / 1000.0; // [0.0, 2.0]
        let result = mutate(&policy, rate);

        if (0.0..=1.0).contains(&rate) {
            prop_assert!(
                result.is_ok(),
                "rate={} ∈ [0, 1] 应被接受",
                rate
            );
            let candidate = result.unwrap();
            // Elite 类型 magnitude == 0.0
            if candidate.mutation_type == MutationType::Elite {
                prop_assert!(
                    (candidate.magnitude - 0.0).abs() < 1e-6,
                    "Elite 类型 magnitude 应为 0.0,实际 {}",
                    candidate.magnitude
                );
            }
        } else {
            prop_assert!(
                result.is_err(),
                "rate={} 越界应返回 MutationFailed",
                rate
            );
        }
    }

    /// 不变量 5:apply_mutation 将 mutation_rate clamp 到 [0.001, 1.0],
    /// selection_pressure clamp 到 ≥ 0
    ///
    /// 对任意 magnitude(包括极端值),apply_mutation 后:
    /// - mutation_rate ∈ [0.001, 1.0]
    /// - selection_pressure ≥ 0.0
    /// - Elite 类型:策略参数不变(直接传承)
    ///
    /// WHY clamp 边界:[0.001, 1.0] 防止 mutation_rate 归零(策略停滞)
    /// 或超过 1.0(策略发散);selection_pressure ≥ 0 防止负压力反向选择。
    #[test]
    fn prop_apply_mutation_clamps_params(
        magnitude_milli in -5000i32..=5000,
        mutation_type_idx in 0u32..3,
    ) {
        let magnitude = magnitude_milli as f32 / 1000.0; // [-5.0, 5.0]
        let mutation_type = match mutation_type_idx {
            0 => MutationType::Gaussian,
            1 => MutationType::Uniform,
            _ => MutationType::Elite,
        };

        let mut policy = EvolutionPolicy::new(0.1, 1.5, 0.2, 8).expect("默认策略应有效");
        let original_mr = policy.mutation_rate;
        let original_sp = policy.selection_pressure;

        let candidate = MutationCandidate {
            policy_id: "p-test".into(),
            mutation_type,
            magnitude,
        };
        apply_mutation(&mut policy, &candidate);

        if mutation_type == MutationType::Elite {
            // Elite 不变异
            prop_assert!(
                (policy.mutation_rate - original_mr).abs() < 1e-6,
                "Elite 类型 mutation_rate 不应变,原 {} 新 {}",
                original_mr,
                policy.mutation_rate
            );
            prop_assert!(
                (policy.selection_pressure - original_sp).abs() < 1e-6,
                "Elite 类型 selection_pressure 不应变"
            );
        } else {
            // 非 Elite:验证 clamp 边界
            prop_assert!(
                policy.mutation_rate >= 0.001 - 1e-6 && policy.mutation_rate <= 1.0 + 1e-6,
                "mutation_rate 应 clamp 到 [0.001, 1.0],实际 {} (magnitude={})",
                policy.mutation_rate,
                magnitude
            );
            prop_assert!(
                policy.selection_pressure >= 0.0,
                "selection_pressure 应 ≥ 0,实际 {} (magnitude={})",
                policy.selection_pressure,
                magnitude
            );
        }
    }
}
