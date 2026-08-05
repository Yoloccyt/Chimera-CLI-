//! PROBE P3.1 有效窗口折减测试 — `WindowTier::effective_fold`
//!
//! 覆盖：
//! - 折减计算（宣称 × 60%，f32 中间值）
//! - 边界（0 / 极小 / 极大宣称）
//! - 与 `effective_capacity_for` 正交叠加取 min 的分流语义
//! - 回归锚点：`effective_capacity_for`（OSA 稀疏度）语义零变化

use hcw_window::types::{HcwConfig, WindowTier, EFFECTIVE_FOLD_FACTOR};

#[test]
fn test_effective_fold_basic() {
    // 1M 宣称 → 600K（×0.6）
    assert_eq!(WindowTier::effective_fold(1_048_576), 629_145);
    // 100K 宣称 → 60K
    assert_eq!(WindowTier::effective_fold(100_000), 60_000);
    // 10K 宣称 → 6K
    assert_eq!(WindowTier::effective_fold(10_000), 6_000);
}

#[test]
fn test_effective_fold_zero_and_small() {
    // 0 宣称 → 0（不 panic）
    assert_eq!(WindowTier::effective_fold(0), 0);
    // 极小宣称（1 token）→ 0（floor）
    assert_eq!(WindowTier::effective_fold(1), 0);
    // 2 token → 1
    assert_eq!(WindowTier::effective_fold(2), 1);
}

#[test]
fn test_effective_fold_large_no_overflow() {
    // 极大宣称（usize::MAX 量级）不 panic（f32 中间值）
    let huge = usize::MAX / 2;
    let folded = WindowTier::effective_fold(huge);
    // f32 精度下 0.6 倍仍为正且有限
    assert!(folded > 0);
}

#[test]
fn test_effective_fold_factor_value() {
    // 折减系数 = 0.6（设计文档 §2.5 P3.1）
    assert_eq!(EFFECTIVE_FOLD_FACTOR, 0.6);
}

#[test]
fn test_fold_min_with_effective_capacity() {
    // 正交叠加取 min：分流判定用 `min(effective_capacity, effective_fold)`
    let config = HcwConfig::default();

    // 场景 1：L3 实际容量（fallback 128K）< 折减（600K）→ 取 128K
    let cap = config.effective_capacity_for(WindowTier::L3, None);
    let fold = WindowTier::effective_fold(1_048_576);
    assert_eq!(
        cap.min(fold),
        cap,
        "系统侧上限更保守时取 effective_capacity"
    );

    // 场景 2：折减（60K）< L2 容量（128K）→ 取 60K
    let cap_l2 = config.effective_capacity_for(WindowTier::L2, None);
    let fold_small = WindowTier::effective_fold(100_000);
    assert!(fold_small < cap_l2);
    assert_eq!(
        cap_l2.min(fold_small),
        fold_small,
        "模型侧上限更保守时取 effective_fold"
    );

    // 场景 3：OSA 稀疏度动态容量与折减取 min（稀疏 0.875 → 128K 与折减 600K → 128K）
    let cap_dynamic = config.effective_capacity_for(WindowTier::L3, Some(0.875));
    assert_eq!(cap_dynamic.min(fold), cap_dynamic);
}

#[test]
fn test_fold_does_not_alter_load_semantics() {
    // 红线：effective_fold 只影响分流判定，L3 实际加载语义零变化
    let config = HcwConfig::default();
    // 与 P3.1 前的 effective_capacity_for 行为一致（回归锚点）
    assert_eq!(config.effective_capacity_for(WindowTier::L0, None), 4096);
    assert_eq!(config.effective_capacity_for(WindowTier::L1, None), 32768);
    assert_eq!(config.effective_capacity_for(WindowTier::L2, None), 131072);
    assert_eq!(config.effective_capacity_for(WindowTier::L3, None), 131072);
    // 动态稀疏度路径不受 effective_fold 影响
    assert_eq!(
        config.effective_capacity_for(WindowTier::L3, Some(0.5)),
        524_288
    );
}
