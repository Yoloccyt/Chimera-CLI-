//! Task 4: HCW L3 动态容量测试
//!
//! 验证 `WindowTier::effective_capacity` 与 `HcwConfig::effective_capacity_for`
//! 在不同 OSA 稀疏度下的 L3 实际加载容量计算正确性。
//!
//! # 测试覆盖
//! - `sparsity=Some(0.875)` → L3 容量 = l3_capacity × 0.125 = 131072(与 fallback 一致)
//! - `sparsity=Some(0.5)` → L3 容量 = l3_capacity × 0.5 = 524288
//! - `sparsity=None` → L3 容量 = l3_capacity / 8 = 131072(fallback 模式)
//! - `sparsity=Some(0.0)` → L3 容量 = l3_capacity = 1048576(无稀疏,全加载)
//! - `sparsity=Some(1.0)` → clamp 到 0.99 → L3 容量 = l3_capacity × 0.01 = 10485
//! - `sparsity=Some(-0.5)` → clamp 到 0.0 → L3 容量 = l3_capacity(负值防御)
//! - `sparsity=Some(1.5)` → clamp 到 0.99 → L3 容量 = l3_capacity × 0.01(超界防御)
//! - L0/L1/L2 不受 sparsity 影响(忽略参数)
//! - fallback 与 Some(0.875) 等价性验证

use hcw_window::{HcwConfig, WindowTier};

/// 默认 L3 容量 = 1M = 1048576
const DEFAULT_L3_CAPACITY: usize = 1_048_576;

/// 默认 L3 实际加载容量(fallback / sparsity=0.875)= 128K = 131072
const DEFAULT_L3_EFFECTIVE: usize = 131_072;

// ============================================================
// L3 动态容量 — 正常稀疏度场景
// ============================================================

#[test]
fn test_l3_dynamic_capacity_sparsity_0_875_matches_fallback() {
    // sparsity=0.875(8× 压缩比)应与 fallback(l3/8)完全一致
    // WHY:确保动态模式在默认稀疏度下行为等价于 fallback,向后兼容
    let config = HcwConfig::default();
    let dynamic = config.effective_capacity_for(WindowTier::L3, Some(0.875));
    let fallback = config.effective_capacity_for(WindowTier::L3, None);
    assert_eq!(
        dynamic, fallback,
        "sparsity=0.875 应与 fallback 一致(均为 128K)"
    );
    assert_eq!(dynamic, DEFAULT_L3_EFFECTIVE);
}

#[test]
fn test_l3_dynamic_capacity_sparsity_0_5() {
    // sparsity=0.5 → 容量 = l3_capacity × 0.5 = 524288
    let config = HcwConfig::default();
    let capacity = config.effective_capacity_for(WindowTier::L3, Some(0.5));
    assert_eq!(capacity, 524_288, "sparsity=0.5 应加载 50% = 524288");
}

#[test]
fn test_l3_dynamic_capacity_sparsity_none_fallback() {
    // sparsity=None → fallback 到 l3_capacity / 8 = 131072
    let config = HcwConfig::default();
    let capacity = config.effective_capacity_for(WindowTier::L3, None);
    assert_eq!(
        capacity, DEFAULT_L3_EFFECTIVE,
        "None fallback 应为 l3_capacity / 8 = 128K"
    );
}

#[test]
fn test_l3_dynamic_capacity_sparsity_0_full_load() {
    // sparsity=0.0 → 无稀疏,全加载 = l3_capacity = 1048576
    let config = HcwConfig::default();
    let capacity = config.effective_capacity_for(WindowTier::L3, Some(0.0));
    assert_eq!(capacity, DEFAULT_L3_CAPACITY, "sparsity=0.0 应全加载 = 1M");
}

// ============================================================
// L3 动态容量 — 边界与防御性 clamp
// ============================================================

#[test]
fn test_l3_dynamic_capacity_sparsity_1_0_clamped_to_0_99() {
    // sparsity=1.0 → clamp 到 0.99 → 容量 = l3_capacity × 0.01 = 10485
    // WHY:避免 100% 稀疏导致空窗口,确保至少加载 1%
    let config = HcwConfig::default();
    let capacity = config.effective_capacity_for(WindowTier::L3, Some(1.0));
    // 1048576 × 0.01 = 10485.76 → as usize = 10485
    assert_eq!(
        capacity, 10_485,
        "sparsity=1.0 应 clamp 到 0.99,容量 = 10485"
    );
    assert!(capacity > 0, "clamp 后容量必须 > 0,避免空窗口");
}

#[test]
fn test_l3_dynamic_capacity_sparsity_negative_clamped_to_0() {
    // sparsity=-0.5 → clamp 到 0.0 → 容量 = l3_capacity = 1048576
    // WHY:负稀疏度无意义,clamp 到 0.0 等价于全加载
    let config = HcwConfig::default();
    let capacity = config.effective_capacity_for(WindowTier::L3, Some(-0.5));
    assert_eq!(
        capacity, DEFAULT_L3_CAPACITY,
        "负 sparsity 应 clamp 到 0.0,全加载 = 1M"
    );
}

#[test]
fn test_l3_dynamic_capacity_sparsity_over_1_clamped_to_0_99() {
    // sparsity=1.5 → clamp 到 0.99 → 容量 = l3_capacity × 0.01 = 10485
    let config = HcwConfig::default();
    let capacity = config.effective_capacity_for(WindowTier::L3, Some(1.5));
    assert_eq!(
        capacity, 10_485,
        "sparsity=1.5 应 clamp 到 0.99,容量 = 10485"
    );
}

#[test]
fn test_l3_dynamic_capacity_never_zero() {
    // 验证所有极端 sparsity 值下容量都 > 0(不会导致空窗口)
    let config = HcwConfig::default();
    let extreme_values = [1.0_f32, 1.5, 100.0, f32::MAX, f32::INFINITY, f32::NAN];
    for s in extreme_values {
        let capacity = config.effective_capacity_for(WindowTier::L3, Some(s));
        assert!(
            capacity > 0,
            "sparsity={s} 下容量必须 > 0,实际 = {capacity}"
        );
    }
}

#[test]
fn test_l3_dynamic_capacity_nan_falls_back_to_default() {
    // NaN sparsity 应 fallback 到硬编码 8× 压缩比(等价于 None)
    // WHY:f32::NAN.clamp 不生效(NaN 比较全为 false),会导致容量 = 0(空窗口),
    // 因此 NaN 视为异常值,走 fallback 分支
    let config = HcwConfig::default();
    let nan_capacity = config.effective_capacity_for(WindowTier::L3, Some(f32::NAN));
    let none_capacity = config.effective_capacity_for(WindowTier::L3, None);
    assert_eq!(
        nan_capacity, none_capacity,
        "NaN sparsity 应 fallback 到 None 行为(均为 128K)"
    );
    assert_eq!(nan_capacity, 131_072);
}

// ============================================================
// L0/L1/L2 不受 sparsity 影响
// ============================================================

#[test]
fn test_l0_l1_l2_ignore_sparsity_parameter() {
    // L0/L1/L2 的实际容量 = 标称容量,sparsity 参数被完全忽略
    let config = HcwConfig::default();

    // 用极端 sparsity 值验证 L0/L1/L2 容量不变
    let sparsity_values = [None, Some(0.0), Some(0.5), Some(0.875), Some(1.0)];

    for s in sparsity_values {
        assert_eq!(
            config.effective_capacity_for(WindowTier::L0, s),
            4096,
            "L0 容量应不受 sparsity={s:?} 影响"
        );
        assert_eq!(
            config.effective_capacity_for(WindowTier::L1, s),
            32768,
            "L1 容量应不受 sparsity={s:?} 影响"
        );
        assert_eq!(
            config.effective_capacity_for(WindowTier::L2, s),
            131072,
            "L2 容量应不受 sparsity={s:?} 影响"
        );
    }
}

// ============================================================
// 等价性验证:fallback ≡ sparsity=0.875
// ============================================================

#[test]
fn test_fallback_equivalent_to_sparsity_0_875() {
    // WHY:确保向后兼容 — OSA 未下发掩码(None)与下发 0.875 稀疏度行为一致
    let config = HcwConfig::default();
    let fallback = config.effective_capacity_for(WindowTier::L3, None);
    let dynamic_875 = config.effective_capacity_for(WindowTier::L3, Some(0.875));
    assert_eq!(
        fallback, dynamic_875,
        "fallback(None)应等价于 sparsity=0.875(均为 8× 压缩比)"
    );
}

// ============================================================
// 自定义配置验证
// ============================================================

#[test]
fn test_l3_dynamic_capacity_with_custom_config() {
    // 自定义 L3 容量 = 524288(512K),验证动态容量公式正确
    let config = HcwConfig::default().with_l3_capacity(524_288);

    // sparsity=0.5 → 524288 × 0.5 = 262144
    assert_eq!(
        config.effective_capacity_for(WindowTier::L3, Some(0.5)),
        262_144
    );
    // sparsity=None → 524288 / 8 = 65536
    assert_eq!(config.effective_capacity_for(WindowTier::L3, None), 65_536);
    // sparsity=0.875 → 524288 × 0.125 = 65536(与 fallback 一致)
    assert_eq!(
        config.effective_capacity_for(WindowTier::L3, Some(0.875)),
        65_536
    );
}

// ============================================================
// f32 全程精度验证(§4.4 教训 #6)
// ============================================================

#[test]
fn test_l3_dynamic_capacity_f32_no_precision_loss() {
    // WHY(§4.4 教训 #6):sparsity 是 f32,全程保持 f32 计算避免精度膨胀
    // 验证 sparsity=0.4 不会因 f32→f64→f32 转换导致 0.4 变为 > 0.4
    let config = HcwConfig::default();
    let capacity_at_04 = config.effective_capacity_for(WindowTier::L3, Some(0.4));
    // 1048576 × (1 - 0.4) = 1048576 × 0.6 = 629145.6 → as usize = 629145
    assert_eq!(capacity_at_04, 629_145, "f32 全程计算,无精度膨胀");
}

// ============================================================
// WindowTier::effective_capacity 直接调用验证
// ============================================================

#[test]
fn test_window_tier_effective_capacity_direct_call() {
    // 直接调用 WindowTier::effective_capacity(绕过 HcwConfig::effective_capacity_for)
    let config = HcwConfig::default();

    // L3 + sparsity=0.875
    assert_eq!(
        WindowTier::L3.effective_capacity(&config, Some(0.875)),
        131_072
    );
    // L3 + None
    assert_eq!(WindowTier::L3.effective_capacity(&config, None), 131_072);
    // L0 + 任意 sparsity(应被忽略)
    assert_eq!(WindowTier::L0.effective_capacity(&config, Some(0.99)), 4096);
}
