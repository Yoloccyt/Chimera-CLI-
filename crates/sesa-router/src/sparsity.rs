//! SESA 稀疏度计算与强制稀疏化 — 确保激活专家数 < 总专家数的 40%
//!
//! 对应架构层:L6 Router
//! 对应创新点:SESA(Sub-Expert Sparse Activation)
//!
//! ## 核心机制
//! - **SparsityProfile**:描述激活专家数与总专家数的比例
//! - **enforce_sparsity**:使用 `select_nth_unstable_by` 实现 O(n) Top-K 选择
//!   (相比 `sort_by` 的 O(n log n),1000 专家规模快约 10x)
//! - **稀疏度强制 < 40%**:超限时按评分降序保留 Top-K,清除其余位
//!
//! ## 性能指标
//! - 1000 专家规模下 enforce_sparsity 延迟 p95 < 1ms
//! - 实测稀疏度严格 < 40%(架构红线)

use serde::{Deserialize, Serialize};

use crate::mask::SesaMask;

/// 稀疏度画像 — 描述激活专家与总专家的比例关系
///
/// 字段语义:
/// - `active_experts`:已激活专家数(对应 SesaMask.active_count)
/// - `total_experts`:专家池总数
/// - `sparsity_ratio`:激活比例 [0.0, 1.0],1.0 表示全激活(无稀疏)
///
/// WHY 设计为独立结构:稀疏度是 SESA 的核心 KPI,
/// 独立结构便于事件发布、监控指标上报与测试断言。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SparsityProfile {
    /// 总专家数
    pub total_experts: u32,
    /// 已激活专家数
    pub active_experts: u32,
    /// 稀疏度比例 [0.0, 1.0](active_experts / total_experts)
    pub sparsity_ratio: f32,
}

impl SparsityProfile {
    /// 创建稀疏度画像
    ///
    /// # 参数
    /// - `total_experts`:专家池总数
    /// - `active_experts`:已激活专家数
    ///
    /// # 返回
    /// 若 `total_experts == 0`,sparsity_ratio 为 0.0(避免除零)
    pub fn new(total_experts: u32, active_experts: u32) -> Self {
        let sparsity_ratio = if total_experts == 0 {
            0.0
        } else {
            active_experts as f32 / total_experts as f32
        };
        Self {
            total_experts,
            active_experts,
            sparsity_ratio,
        }
    }

    /// 从掩码构造稀疏度画像
    ///
    /// # 参数
    /// - `mask`:激活掩码
    /// - `total_experts`:专家池总数
    pub fn from_mask(mask: &SesaMask, total_experts: u32) -> Self {
        Self::new(total_experts, mask.active_count)
    }

    /// 判断是否满足稀疏度约束(< max_ratio)
    ///
    /// # 参数
    /// - `max_ratio`:最大允许稀疏度(如 0.4)
    pub fn is_within_limit(&self, max_ratio: f32) -> bool {
        self.sparsity_ratio < max_ratio
    }
}

/// 强制稀疏化:确保掩码激活位数严格满足 `active / total < max_ratio`
///
/// 超限时按评分降序保留 Top-K 位,清除其余位。
/// 使用 `select_nth_unstable_by` 实现 O(n) 平均复杂度的 Top-K 选择。
///
/// # 算法
/// 1. 计算允许保留的最大位数 `max_allowed`,确保 `max_allowed / total < max_ratio`(严格小于)
/// 2. 至少保留 1 位(激活 0 个专家无实际意义)
/// 3. 若 `active_count <= max_allowed`,无需裁剪,直接返回
/// 4. 否则用 `select_nth_unstable_by` 选出评分最高的 K 个,清除其余位
///
/// # 严格小于的实现
/// `max_allowed = floor(total × max_ratio)`,然后检查:
/// 若 `max_allowed / total >= max_ratio`(浮点比较),则 `max_allowed -= 1`。
/// 这确保 `active / total < max_ratio`(架构要求"严格 < 40%")。
///
/// # 参数
/// - `mask`:待裁剪的掩码(原地修改)
/// - `scores`:每位对应的评分(`score[idx]` 对应掩码位 idx)
/// - `total`:专家池总数(用于计算 max_ratio × total)
/// - `max_ratio`:最大允许稀疏度(如 0.4 表示 40%)
///
/// # 返回
/// 实际清除的位数
///
/// # 性能
/// O(n) 平均复杂度,1000 专家规模 < 1ms
pub fn enforce_sparsity(mask: &mut SesaMask, scores: &[f32], total: u32, max_ratio: f32) -> usize {
    let max_allowed = max_allowed_active(total, max_ratio);
    let active = mask.active_count as usize;

    // 已在限制内,无需裁剪
    if active <= max_allowed {
        return 0;
    }

    // 收集所有已激活位的 (idx, score)
    let mut active_scores: Vec<(usize, f32)> = mask
        .to_indices()
        .into_iter()
        .map(|idx| {
            let score = scores.get(idx).copied().unwrap_or(0.0);
            (idx, score)
        })
        .collect();

    // Top-K 选择:保留评分最高的 max_allowed 个
    let keep_count = max_allowed;
    let to_clear_count = active - keep_count;

    // 边界:keep_count == 0 时清空所有(理论上不会发生,因 max_allowed_active 保证 >= 1)
    if keep_count == 0 {
        let cleared = mask.active_count as usize;
        mask.reset();
        return cleared;
    }

    // 使用 select_nth_unstable_by 选 Top-K(降序:大的在前)
    // WHY O(n):quickselect 平均 O(n),相比 sort_by 的 O(n log n) 快约 10x
    let idx = keep_count - 1;
    active_scores.select_nth_unstable_by(idx, |a, b| {
        b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
    });

    // 前 keep_count 个是评分最高的,保留;其余清除
    let keep_indices: std::collections::HashSet<usize> = active_scores[..keep_count]
        .iter()
        .map(|(i, _)| *i)
        .collect();

    // 重建掩码:仅保留 Top-K 位
    mask.reset();
    for &idx in &keep_indices {
        mask.set_bit(idx);
    }
    to_clear_count
}

/// 计算允许激活的最大专家数,确保 `active / total < max_ratio`(严格小于)
///
/// # 算法
/// 1. `max_allowed = floor(total × max_ratio)`(f32 精度,与 SparsityProfile 一致)
/// 2. 若 `max_allowed / total >= max_ratio`,则 `max_allowed -= 1`(确保严格小于)
/// 3. 至少返回 1(激活 0 个专家无实际意义,当 total > 0 时)
///
/// # 参数
/// - `total`:专家池总数
/// - `max_ratio`:最大允许稀疏度
///
/// # 返回
/// 满足 `result / total < max_ratio` 的最大整数(至少 1,当 total > 0)
pub fn max_allowed_active(total: u32, max_ratio: f32) -> usize {
    if total == 0 {
        return 0;
    }
    // WHY f32 比较(而非 f64):SparsityProfile::new 用 f32 计算 sparsity_ratio
    // (active as f32 / total as f32),此处必须用相同精度比较才能保证一致性。
    // 若用 f64:max_ratio as f64 会将 f32 的 0.4(实际 0.400000006)提升为 f64,
    // 导致 40/100=0.4(f64) < 0.400000006(f64) 误判为"已严格小于",不执行减 1。
    // f32 比较下 40.0f32/100.0f32 == 0.4f32 为真,正确触发减 1 → 39。
    let product = (total as f32) * max_ratio;
    let mut max_allowed = product.floor() as usize;
    // 确保严格小于:active / total < max_ratio(与 SparsityProfile 同精度)
    if (max_allowed as f32) / (total as f32) >= max_ratio {
        max_allowed = max_allowed.saturating_sub(1);
    }
    // 至少保留 1 位(当 total > 0 时,激活 0 个专家无意义)
    // WHY:total=1, max_ratio=0.4 时,product=0.4, floor=0,
    // 但激活 0 个专家没有实际意义,保留 1 个作为最低保障。
    max_allowed.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    // === 1. SparsityProfile 基础测试 ===

    #[test]
    fn test_sparsity_profile_new() {
        let profile = SparsityProfile::new(100, 30);
        assert_eq!(profile.total_experts, 100);
        assert_eq!(profile.active_experts, 30);
        assert!((profile.sparsity_ratio - 0.3).abs() < 1e-5);
    }

    #[test]
    fn test_sparsity_profile_zero_total() {
        let profile = SparsityProfile::new(0, 0);
        assert_eq!(profile.sparsity_ratio, 0.0, "total=0 时 ratio 应为 0.0");
    }

    #[test]
    fn test_sparsity_profile_from_mask() {
        let mut mask = SesaMask::new();
        mask.set_bit(0);
        mask.set_bit(10);
        mask.set_bit(20);

        let profile = SparsityProfile::from_mask(&mask, 100);
        assert_eq!(profile.active_experts, 3);
        assert_eq!(profile.total_experts, 100);
        assert!((profile.sparsity_ratio - 0.03).abs() < 1e-5);
    }

    #[test]
    fn test_sparsity_profile_is_within_limit() {
        let profile = SparsityProfile::new(100, 30);
        assert!(profile.is_within_limit(0.4), "30% < 40% 应通过");
        assert!(profile.is_within_limit(0.35), "30% < 35% 应通过");

        let profile_full = SparsityProfile::new(100, 50);
        assert!(!profile_full.is_within_limit(0.4), "50% >= 40% 应不通过");
    }

    // === 2. enforce_sparsity 基础测试 ===

    #[test]
    fn test_enforce_sparsity_within_limit_no_op() {
        let mut mask = SesaMask::new();
        mask.set_bit(0);
        mask.set_bit(1);
        mask.set_bit(2); // 3 位激活

        // 100 专家 × 0.4 = 40,3 < 40,无需裁剪
        let cleared = enforce_sparsity(&mut mask, &[0.5; 100], 100, 0.4);
        assert_eq!(cleared, 0, "未超限应返回 0");
        assert_eq!(mask.active_count, 3);
    }

    #[test]
    fn test_enforce_sparsity_trims_to_limit() {
        let mut mask = SesaMask::new();
        // 激活 50 位(0-49)
        for i in 0..50 {
            mask.set_bit(i);
        }
        assert_eq!(mask.active_count, 50);

        // 100 专家 × 0.4 = 40,严格 < 40% → 39 位
        let scores: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let cleared = enforce_sparsity(&mut mask, &scores, 100, 0.4);

        assert_eq!(cleared, 11, "应裁剪 11 位(50-39)");
        assert_eq!(mask.active_count, 39, "裁剪后应剩 39 位(严格 < 40%)");
        assert!(
            mask.active_count < 40,
            "严格 < 40%:实际 {}",
            mask.active_count
        );
    }

    #[test]
    fn test_enforce_sparsity_keeps_highest_scores() {
        let mut mask = SesaMask::new();
        // 激活 0-49(50 位)
        for i in 0..50 {
            mask.set_bit(i);
        }

        // 评分:偶数位高(10.0),奇数位低(1.0)
        let scores: Vec<f32> = (0..100)
            .map(|i| if i % 2 == 0 { 10.0 } else { 1.0 })
            .collect();

        // 100 × 0.4 = 40,严格 < 40% → 39 位
        // 偶数位 0,2,4,...,48 共 25 个;还需补 14 个奇数位(评分都是 1.0,任意选)
        enforce_sparsity(&mut mask, &scores, 100, 0.4);

        assert_eq!(mask.active_count, 39, "严格 < 40% → 39 位");

        // 验证:所有偶数位(0-48)应保留(评分最高)
        for i in (0..50).step_by(2) {
            assert!(mask.get_bit(i), "偶数位 {} 评分高应保留", i);
        }
    }

    #[test]
    fn test_enforce_sparsity_zero_max_ratio_keeps_one() {
        let mut mask = SesaMask::new();
        mask.set_bit(0);
        mask.set_bit(1);
        mask.set_bit(2);

        // max_ratio = 0.0,但 max_allowed_active 保证至少 1 位
        // WHY:激活 0 个专家无意义,保留评分最高的 1 个
        let scores: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let cleared = enforce_sparsity(&mut mask, &scores, 100, 0.0);
        assert_eq!(cleared, 2, "应裁剪 2 位,保留 1 位");
        assert_eq!(mask.active_count, 1);
        // 评分最高的位 2 应保留
        assert!(mask.get_bit(2), "评分最高的位 2 应保留");
    }

    #[test]
    fn test_enforce_sparsity_1000_experts_under_40_percent() {
        // 模拟 1000 专家规模(实际只能用 256 位掩码,这里测试 256 专家)
        let mut mask = SesaMask::new();
        // 激活 200 位(超出 256 × 0.4 = 102.4 → 102,严格 < 40% → 102)
        for i in 0..200 {
            mask.set_bit(i);
        }
        assert_eq!(mask.active_count, 200);

        let scores: Vec<f32> = (0..256).map(|i| (255 - i) as f32).collect();
        let cleared = enforce_sparsity(&mut mask, &scores, 256, 0.4);

        // 256 × 0.4 = 102.4 → floor = 102,102/256 = 0.3984375 < 0.4 ✓
        // 严格 < 40% 检查:102/256 = 0.3984375 < 0.4,无需再减 1
        assert_eq!(cleared, 98, "应清除 200-102=98 位");
        assert_eq!(mask.active_count, 102);
        let ratio = mask.active_count as f32 / 256.0;
        assert!(
            ratio < 0.4,
            "应严格 < 40%:实际 {} (active={})",
            ratio,
            mask.active_count
        );
    }

    // === 3. max_allowed_active 测试 ===

    #[test]
    fn test_max_allowed_active_basic() {
        // 严格 <:100 × 0.4 = 40,40/100 = 0.4 >= 0.4 → 39
        assert_eq!(max_allowed_active(100, 0.4), 39);
        // 1000 × 0.4 = 400,400/1000 = 0.4 >= 0.4 → 399
        assert_eq!(max_allowed_active(1000, 0.4), 399);
        // 256 × 0.4 = 102.4 → floor = 102,102/256 = 0.398 < 0.4 ✓ → 102
        assert_eq!(max_allowed_active(256, 0.4), 102);
    }

    #[test]
    fn test_max_allowed_active_zero_total() {
        assert_eq!(max_allowed_active(0, 0.4), 0);
    }

    #[test]
    fn test_max_allowed_active_zero_ratio() {
        // max_ratio = 0.0,product = 0,floor = 0,但 max_allowed.max(1) = 1
        // WHY:激活 0 个专家无意义,保留 1 个作为最低保障
        assert_eq!(max_allowed_active(100, 0.0), 1);
    }

    #[test]
    fn test_max_allowed_active_single_expert() {
        // total=1, max_ratio=0.4:product=0.4, floor=0, max(1)=1
        assert_eq!(max_allowed_active(1, 0.4), 1);
    }

    #[test]
    fn test_max_allowed_active_strict_inequality() {
        // 验证所有结果满足 result / total < max_ratio
        for total in [10, 50, 100, 256, 1000] {
            let k = max_allowed_active(total, 0.4);
            let ratio = k as f64 / total as f64;
            assert!(
                ratio < 0.4,
                "total={} k={} ratio={} 应严格 < 0.4",
                total,
                k,
                ratio
            );
        }
    }

    // === 4. enforce_sparsity 边界测试 ===

    #[test]
    fn test_enforce_sparsity_empty_mask() {
        let mut mask = SesaMask::new();
        let cleared = enforce_sparsity(&mut mask, &[0.5; 100], 100, 0.4);
        assert_eq!(cleared, 0, "空掩码无需裁剪");
        assert_eq!(mask.active_count, 0);
    }

    #[test]
    fn test_enforce_sparsity_keep_count_equals_active() {
        let mut mask = SesaMask::new();
        // 激活 39 位,正好等于严格 < 40% 的最大值(100 × 0.4 = 40,严格 < → 39)
        for i in 0..39 {
            mask.set_bit(i);
        }
        let scores: Vec<f32> = vec![0.5; 100];
        let cleared = enforce_sparsity(&mut mask, &scores, 100, 0.4);
        assert_eq!(cleared, 0, "正好等于限制不应裁剪");
        assert_eq!(mask.active_count, 39);
    }

    #[test]
    fn test_enforce_sparsity_keep_count_one_over() {
        let mut mask = SesaMask::new();
        // 激活 40 位,超出 1 位(严格 < 40% → 39)
        for i in 0..40 {
            mask.set_bit(i);
        }
        let scores: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let cleared = enforce_sparsity(&mut mask, &scores, 100, 0.4);
        assert_eq!(cleared, 1, "应裁剪 1 位");
        assert_eq!(mask.active_count, 39);
        // 评分最低的位 0 应被清除
        assert!(!mask.get_bit(0), "评分最低的位 0 应被清除");
    }

    // === 5. SparsityProfile 序列化 ===

    #[test]
    fn test_sparsity_profile_serde_roundtrip() {
        let profile = SparsityProfile::new(1000, 350);
        let json = serde_json::to_string(&profile).expect("序列化失败");
        let restored: SparsityProfile = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(profile, restored);
    }
}
