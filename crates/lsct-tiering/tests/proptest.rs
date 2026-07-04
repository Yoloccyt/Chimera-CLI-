//! LSCT-tiering 属性测试 — 单调性不变量验证
//!
//! 对应架构层:L3 Storage
//!
//! # 不变量
//! 对同一 task_type,intensity 越高 → tier_rank 越小(越热)或相等,
//! 绝不会出现"高强度得到更冷的 tier"。
//!
//! # 语法说明
//! proptest 1.11.0 闭包形式解析不稳定,使用块状命名测试形式
//! (参考 CHTC-bridge fix, 2026-06-26)

use cmt_tiering::Tier;
use lsct_tiering::{compute_target_tier, tier_rank, TaskLoadProfile, TaskType};
use proptest::prelude::*;

proptest! {
    /// 不变量 1:对同一 task_type,intensity 越高 → tier_rank 越小(越热)
    #[test]
    fn test_monotonicity_compile(
        low in 0.0f32..0.5,
        high in 0.5f32..1.0
    ) {
        let tier_low = compute_target_tier(&TaskLoadProfile::new(TaskType::Compile, low, 1));
        let tier_high = compute_target_tier(&TaskLoadProfile::new(TaskType::Compile, high, 1));
        prop_assert!(
            tier_rank(tier_low) >= tier_rank(tier_high),
            "Compile: intensity {} → rank {}, intensity {} → rank {} (违反单调性)",
            low, tier_rank(tier_low), high, tier_rank(tier_high)
        );
    }

    /// 不变量 2:Debug 任务的单调性
    #[test]
    fn test_monotonicity_debug(
        low in 0.0f32..0.5,
        high in 0.5f32..1.0
    ) {
        let tier_low = compute_target_tier(&TaskLoadProfile::new(TaskType::Debug, low, 1));
        let tier_high = compute_target_tier(&TaskLoadProfile::new(TaskType::Debug, high, 1));
        prop_assert!(
            tier_rank(tier_low) >= tier_rank(tier_high),
            "Debug: intensity {} → rank {}, intensity {} → rank {} (违反单调性)",
            low, tier_rank(tier_low), high, tier_rank(tier_high)
        );
    }

    /// 不变量 3:Test 任务的单调性
    #[test]
    fn test_monotonicity_test(
        low in 0.0f32..0.5,
        high in 0.5f32..1.0
    ) {
        let tier_low = compute_target_tier(&TaskLoadProfile::new(TaskType::Test, low, 1));
        let tier_high = compute_target_tier(&TaskLoadProfile::new(TaskType::Test, high, 1));
        prop_assert!(
            tier_rank(tier_low) >= tier_rank(tier_high),
            "Test: intensity {} → rank {}, intensity {} → rank {} (违反单调性)",
            low, tier_rank(tier_low), high, tier_rank(tier_high)
        );
    }

    /// 不变量 4:Run 始终返回 Hot(rank=0),无论 intensity
    #[test]
    fn test_run_always_hot(intensity in 0.0f32..1.0) {
        let tier = compute_target_tier(&TaskLoadProfile::new(TaskType::Run, intensity, 1));
        prop_assert_eq!(tier, Tier::Hot);
        prop_assert_eq!(tier_rank(tier), 0);
    }

    /// 不变量 5:tier_rank 返回值始终在 [0, 3] 范围内
    #[test]
    fn test_tier_rank_in_range(intensity in 0.0f32..1.0) {
        for task_type in [TaskType::Compile, TaskType::Debug, TaskType::Test, TaskType::Run] {
            let tier = compute_target_tier(&TaskLoadProfile::new(task_type, intensity, 1));
            let rank = tier_rank(tier);
            prop_assert!(rank <= 3, "tier_rank {} 超出 [0,3] 范围", rank);
        }
    }
}
