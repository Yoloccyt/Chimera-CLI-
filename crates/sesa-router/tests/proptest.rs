//! sesa-router 不变量属性测试 — 稀疏化后激活比例严格 < max_ratio
//!
//! 对应架构层:L6 Router
//! 对应创新点:SESA(Sub-Expert Sparse Activation)
//!
//! # 测试目标
//! 验证稀疏化逻辑在随机输入下满足架构红线:
//! 1. **SparsityProfile 比例计算正确** — ratio = active/total,值域 [0, 1]
//! 2. **max_allowed_active 严格 < max_ratio** — 除非保留最低 1 位
//! 3. **enforce_sparsity 裁剪后满足约束** — active_after <= max_allowed
//!
//! # 架构红线
//! SESA 要求激活专家数 < 总专家数的 40%(严格小于),enforce_sparsity
//! 使用 select_nth_unstable_by 实现 O(n) Top-K 选择确保此约束。
//!
//! # 语法约束(§4.4 规则)
//! proptest 1.11+ 用 block-named 语法

#![forbid(unsafe_code)]

use proptest::prelude::*;
use sesa_router::{enforce_sparsity, max_allowed_active, SesaMask, SparsityProfile};

/// 生成 [0.0, 1.0] 范围的有限 f32
fn prop_ratio() -> impl Strategy<Value = f32> {
    (0u32..=10_000u32).prop_map(|v| v as f32 / 10_000.0)
}

proptest! {
    #[test]
    fn prop_sparsity_ratio_invariant(
        total in 1u32..=256u32,
        active_count in 0u32..=256u32,
        max_ratio in prop_ratio(),
    ) {
        let clamped_active = active_count.min(total);
        let profile = SparsityProfile::new(total, clamped_active);

        // === 不变量1:SparsityProfile 比例计算正确 ===
        let expected_ratio = clamped_active as f32 / total as f32;
        prop_assert!(
            (profile.sparsity_ratio - expected_ratio).abs() < 1e-5,
            "sparsity_ratio 应为 {}/{}, got {}",
            clamped_active,
            total,
            profile.sparsity_ratio
        );
        prop_assert!(
            (0.0..=1.0).contains(&profile.sparsity_ratio),
            "sparsity_ratio 应在 [0,1], got {}",
            profile.sparsity_ratio
        );
        prop_assert_eq!(profile.total_experts, total);
        prop_assert_eq!(profile.active_experts, clamped_active);

        // === 不变量2:max_allowed_active 满足严格 < max_ratio(或保留 1 位)===
        let max_allowed = max_allowed_active(total, max_ratio);
        if max_ratio > 0.0 {
            // total > 0 时至少保留 1 位(激活 0 个专家无实际意义)
            prop_assert!(max_allowed >= 1, "total>0 时至少保留 1 位, got {}", max_allowed);

            // 验证严格 < max_ratio:除非 max_allowed 被最低保留 1 限制
            let ratio = max_allowed as f32 / total as f32;
            // WHY 允许 max_allowed=1 时 ratio >= max_ratio:
            // 当 total * max_ratio < 1 时,最低保留 1 位的约束优先于严格 < max_ratio
            prop_assert!(
                ratio < max_ratio || max_allowed == 1,
                "max_allowed {}/{} = {} 应 < {} (或保留最低 1 位)",
                max_allowed,
                total,
                ratio,
                max_ratio
            );
        }
    }

    #[test]
    fn prop_enforce_sparsity_satisfies_limit(
        total in 1u32..=256u32,
        active_bits in prop::collection::vec(0u32..256, 0..256),
        max_ratio in prop_ratio(),
    ) {
        // 构造随机掩码:set_bit 越界项被忽略
        let mut mask = SesaMask::new();
        for &idx in &active_bits {
            mask.set_bit(idx as usize);
        }
        let active_before = mask.active_count;

        // 评分:每位不同分数(用于 Top-K 选择)
        let scores: Vec<f32> = (0..256).map(|i| (i as f32) * 0.1).collect();

        let cleared = enforce_sparsity(&mut mask, &scores, total, max_ratio);
        let active_after = mask.active_count;
        let max_allowed = max_allowed_active(total, max_ratio) as u32;

        // === 不变量3:裁剪后激活数不超过 max_allowed ===
        prop_assert!(
            active_after <= max_allowed,
            "裁剪后 active {} 应 <= max_allowed {}",
            active_after,
            max_allowed
        );

        // === 不变量3b:cleared 数量正确 ===
        if active_before > max_allowed {
            prop_assert_eq!(
                cleared,
                (active_before - max_allowed) as usize,
                "超限时应裁剪 active_before - max_allowed"
            );
        } else {
            prop_assert_eq!(cleared, 0, "未超限时不应裁剪");
            prop_assert_eq!(active_after, active_before, "未超限时掩码不变");
        }

        // === 不变量3c:裁剪后比例严格 < max_ratio(或保留最低 1 位)===
        if max_ratio > 0.0 {
            let ratio = active_after as f32 / total as f32;
            prop_assert!(
                ratio < max_ratio || active_after <= 1,
                "裁剪后比例 {} 应 < {} (或保留 <=1 位)",
                ratio,
                max_ratio
            );
        }
    }
}
