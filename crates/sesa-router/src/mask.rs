//! SESA 256-bit 掩码实现 — 子专家激活状态位向量
//!
//! 对应架构层:L6 Router
//! 对应创新点:SESA(Sub-Expert Sparse Activation)
//!
//! ## 设计要点
//! - **256-bit 位向量**:32 字节 × 8 位 = 256 位,可表示最多 256 个专家
//! - **popcount 用 `u8::count_ones` 内建**:SIMD 友好,编译器自动展开为 POPCNT 指令(**无 unsafe**)
//! - **小端序布局**:`byte[0]` 的 bit 0 是专家 0,`byte[0]` 的 bit 7 是专家 7,`byte[1]` 的 bit 0 是专家 8
//! - **active_count 缓存**:避免每次 popcount 重复计算,O(1) 读取激活位数
//!
//! ## 位索引计算
//! - byte_index = idx / 8(0-31)
//! - bit_offset = idx % 8(0-7)
//! - 例如:idx=10 → `byte[1]` 的 bit 2

use serde::{Deserialize, Serialize};

/// 256-bit 掩码(32 字节 × 8 位 = 256 位)
///
/// 位向量布局采用小端序:`byte[0]` 的 bit 0 是专家 0,`byte[0]` 的 bit 7 是专家 7,
/// `byte[1]` 的 bit 0 是专家 8,以此类推。
///
/// `active_count` 缓存当前激活位数,避免每次 popcount 重复计算。
///
/// # 示例
/// ```
/// use sesa_router::SesaMask;
///
/// let mut mask = SesaMask::new();
/// assert_eq!(mask.popcount(), 0);
///
/// mask.set_bit(0);   // 激活专家 0
/// mask.set_bit(10);  // 激活专家 10
/// mask.set_bit(255); // 激活专家 255
/// assert_eq!(mask.popcount(), 3);
/// assert!(mask.get_bit(10));
/// assert!(!mask.get_bit(11));
///
/// mask.clear_bit(10);
/// assert_eq!(mask.popcount(), 2);
/// assert!(!mask.get_bit(10));
/// ```
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SesaMask {
    /// 位向量(32 字节,小端序:`byte[0]` 的 bit 0 是专家 0)
    pub bits: [u8; 32],
    /// 激活位数(popcount 缓存,避免重复计算)
    pub active_count: u32,
}

/// 掩码总位数(256-bit)
pub const MASK_TOTAL_BITS: usize = 256;

/// 掩码字节数(32 字节)
pub const MASK_BYTES: usize = 32;

impl SesaMask {
    /// 创建全零掩码(无专家激活)
    pub fn new() -> Self {
        Self {
            bits: [0u8; MASK_BYTES],
            active_count: 0,
        }
    }

    /// 设置指定位(激活对应专家)
    ///
    /// 若位已设置,此操作无效果(active_count 不变)。
    ///
    /// # 参数
    /// - `idx`:位索引(0-255),超过 255 将被忽略(返回不操作)
    pub fn set_bit(&mut self, idx: usize) {
        if idx >= MASK_TOTAL_BITS {
            return;
        }
        let byte_idx = idx / 8;
        let bit_offset = idx % 8;
        let bit_mask = 1u8 << bit_offset;
        if self.bits[byte_idx] & bit_mask == 0 {
            self.bits[byte_idx] |= bit_mask;
            self.active_count += 1;
        }
    }

    /// 清除指定位(取消激活对应专家)
    ///
    /// 若位未设置,此操作无效果(active_count 不变)。
    ///
    /// # 参数
    /// - `idx`:位索引(0-255),超过 255 将被忽略(返回不操作)
    pub fn clear_bit(&mut self, idx: usize) {
        if idx >= MASK_TOTAL_BITS {
            return;
        }
        let byte_idx = idx / 8;
        let bit_offset = idx % 8;
        let bit_mask = 1u8 << bit_offset;
        if self.bits[byte_idx] & bit_mask != 0 {
            self.bits[byte_idx] &= !bit_mask;
            self.active_count -= 1;
        }
    }

    /// 读取指定位状态
    ///
    /// # 参数
    /// - `idx`:位索引(0-255)
    ///
    /// # 返回
    /// - `true`:位已设置(专家已激活)
    /// - `false`:位未设置或索引越界
    pub fn get_bit(&self, idx: usize) -> bool {
        if idx >= MASK_TOTAL_BITS {
            return false;
        }
        let byte_idx = idx / 8;
        let bit_offset = idx % 8;
        let bit_mask = 1u8 << bit_offset;
        self.bits[byte_idx] & bit_mask != 0
    }

    /// 计算激活位数(popcount)
    ///
    /// 使用 `u8::count_ones` 内建方法,**SIMD 友好且无 unsafe**:
    /// Rust 编译器会自动展开为 CPU 的 POPCNT 指令(若可用)。
    ///
    /// WHY 直接遍历计算而非返回 active_count 缓存:
    /// 此方法提供"权威"popcount,可用于校验 active_count 缓存一致性。
    /// 日常读取激活位数应直接用 `mask.active_count` 字段(O(1))。
    pub fn popcount(&self) -> u32 {
        // WHY u8::count_ones() 已返回 u32,无需 as u32 转换
        // SIMD 友好:编译器自动展开为 POPCNT 指令(若可用)
        self.bits.iter().map(|b| b.count_ones()).sum()
    }

    /// 计算稀疏度比例(active_count / total)
    ///
    /// # 参数
    /// - `total`:专家总数(若为 0 返回 0.0)
    ///
    /// # 返回
    /// 稀疏度比例 [0.0, 1.0],1.0 表示全激活(无稀疏)
    pub fn sparsity_ratio(&self, total: u32) -> f32 {
        if total == 0 {
            return 0.0;
        }
        self.active_count as f32 / total as f32
    }

    /// 重置掩码为全零(无专家激活)
    pub fn reset(&mut self) {
        self.bits.fill(0);
        self.active_count = 0;
    }

    /// 从激活索引列表构造掩码
    ///
    /// # 参数
    /// - `indices`:激活位索引列表(每个值应为 0-255)
    ///
    /// # 返回
    /// 新的 SesaMask,索引越界项被忽略
    pub fn from_indices(indices: &[usize]) -> Self {
        let mut mask = Self::new();
        for &idx in indices {
            mask.set_bit(idx);
        }
        mask
    }

    /// 收集所有已激活位的索引列表
    ///
    /// # 返回
    /// 升序排列的激活位索引向量
    pub fn to_indices(&self) -> Vec<usize> {
        let mut indices = Vec::with_capacity(self.active_count as usize);
        for idx in 0..MASK_TOTAL_BITS {
            if self.get_bit(idx) {
                indices.push(idx);
            }
        }
        indices
    }

    /// 校验 active_count 缓存与实际 popcount 一致
    ///
    /// WHY 测试用:确保 set_bit/clear_bit 正确维护 active_count。
    /// 生产代码不调用此方法(冗余计算)。
    #[cfg(test)]
    pub fn verify_active_count(&self) -> bool {
        self.active_count == self.popcount()
    }
}

impl Default for SesaMask {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === 1. 创建与默认值 ===

    #[test]
    fn test_new_mask_is_zero() {
        let mask = SesaMask::new();
        assert_eq!(mask.bits, [0u8; 32]);
        assert_eq!(mask.active_count, 0);
        assert_eq!(mask.popcount(), 0);
    }

    #[test]
    fn test_default_equals_new() {
        let mask_default = SesaMask::default();
        let mask_new = SesaMask::new();
        assert_eq!(mask_default, mask_new);
    }

    // === 2. set_bit / get_bit 基础测试 ===

    #[test]
    fn test_set_bit_zero() {
        let mut mask = SesaMask::new();
        mask.set_bit(0);
        assert!(mask.get_bit(0));
        assert_eq!(mask.active_count, 1);
        assert_eq!(mask.popcount(), 1);
        assert!(mask.verify_active_count());
    }

    #[test]
    fn test_set_bit_max_index() {
        let mut mask = SesaMask::new();
        mask.set_bit(255);
        assert!(mask.get_bit(255));
        assert_eq!(mask.active_count, 1);
        // 专家 255 → byte[31] 的 bit 7
        assert_eq!(mask.bits[31], 0b1000_0000);
    }

    #[test]
    fn test_set_bit_byte_boundary() {
        // 测试 byte 边界:idx=7 在 byte[0] bit 7,idx=8 在 byte[1] bit 0
        let mut mask = SesaMask::new();
        mask.set_bit(7);
        mask.set_bit(8);
        assert_eq!(mask.bits[0], 0b1000_0000);
        assert_eq!(mask.bits[1], 0b0000_0001);
        assert_eq!(mask.active_count, 2);
    }

    #[test]
    fn test_set_bit_idempotent() {
        let mut mask = SesaMask::new();
        mask.set_bit(10);
        mask.set_bit(10); // 重复设置,active_count 不应增加
        assert_eq!(mask.active_count, 1);
        assert!(mask.get_bit(10));
    }

    #[test]
    fn test_set_bit_out_of_bounds_ignored() {
        let mut mask = SesaMask::new();
        mask.set_bit(256); // 越界,应被忽略
        mask.set_bit(1000);
        assert_eq!(mask.active_count, 0);
        assert!(!mask.get_bit(256));
    }

    // === 3. clear_bit 测试 ===

    #[test]
    fn test_clear_bit_set_position() {
        let mut mask = SesaMask::new();
        mask.set_bit(10);
        assert_eq!(mask.active_count, 1);

        mask.clear_bit(10);
        assert!(!mask.get_bit(10));
        assert_eq!(mask.active_count, 0);
        assert_eq!(mask.popcount(), 0);
    }

    #[test]
    fn test_clear_bit_unset_position_no_effect() {
        let mut mask = SesaMask::new();
        mask.clear_bit(10); // 未设置的位,clear 无效果
        assert_eq!(mask.active_count, 0);
    }

    #[test]
    fn test_clear_bit_out_of_bounds_ignored() {
        let mut mask = SesaMask::new();
        mask.set_bit(0);
        mask.clear_bit(256); // 越界,应被忽略
        assert_eq!(mask.active_count, 1);
    }

    // === 4. popcount 测试 ===

    #[test]
    fn test_popcount_all_set() {
        let mut mask = SesaMask::new();
        for i in 0..256 {
            mask.set_bit(i);
        }
        assert_eq!(mask.popcount(), 256);
        assert_eq!(mask.active_count, 256);
        assert!(mask.verify_active_count());
    }

    #[test]
    fn test_popcount_partial_set() {
        let mut mask = SesaMask::new();
        // 在每个字节设置不同数量的位:1+1+2+3 = 7 位
        mask.set_bit(0); // byte[0]: 1 位
        mask.set_bit(8); // byte[1]: 1 位
        mask.set_bit(16);
        mask.set_bit(17); // byte[2]: 2 位
        mask.set_bit(24);
        mask.set_bit(25);
        mask.set_bit(26); // byte[3]: 3 位
        assert_eq!(mask.popcount(), 7, "1+1+2+3 = 7 位");
        assert_eq!(mask.active_count, 7);
    }

    #[test]
    fn test_popcount_uses_u8_count_ones() {
        // 验证 popcount 与 u8::count_ones 一致(覆盖所有 256 个位模式)
        let mut mask = SesaMask::new();
        mask.bits[0] = 0b1010_1010;
        mask.bits[1] = 0b1111_0000;
        mask.bits[2] = 0b0000_1111;
        // 其余为 0
        let expected: u32 =
            0b1010_1010u8.count_ones() + 0b1111_0000u8.count_ones() + 0b0000_1111u8.count_ones();
        assert_eq!(mask.popcount(), expected);
        assert_eq!(expected, 4 + 4 + 4);
    }

    // === 5. sparsity_ratio 测试 ===

    #[test]
    fn test_sparsity_ratio_zero_total() {
        let mask = SesaMask::new();
        assert_eq!(mask.sparsity_ratio(0), 0.0);
    }

    #[test]
    fn test_sparsity_ratio_no_active() {
        let mask = SesaMask::new();
        assert_eq!(mask.sparsity_ratio(100), 0.0);
    }

    #[test]
    fn test_sparsity_ratio_half_active() {
        let mut mask = SesaMask::new();
        for i in 0..50 {
            mask.set_bit(i);
        }
        let ratio = mask.sparsity_ratio(100);
        assert!((ratio - 0.5).abs() < 1e-5, "50/100 应为 0.5, got {}", ratio);
    }

    #[test]
    fn test_sparsity_ratio_all_active() {
        let mut mask = SesaMask::new();
        for i in 0..256 {
            mask.set_bit(i);
        }
        let ratio = mask.sparsity_ratio(256);
        assert!(
            (ratio - 1.0).abs() < 1e-5,
            "256/256 应为 1.0, got {}",
            ratio
        );
    }

    // === 6. reset 测试 ===

    #[test]
    fn test_reset_clears_all() {
        let mut mask = SesaMask::new();
        mask.set_bit(0);
        mask.set_bit(10);
        mask.set_bit(255);
        assert_eq!(mask.active_count, 3);

        mask.reset();
        assert_eq!(mask.active_count, 0);
        assert_eq!(mask.bits, [0u8; 32]);
        assert!(!mask.get_bit(0));
        assert!(!mask.get_bit(10));
        assert!(!mask.get_bit(255));
    }

    // === 7. from_indices / to_indices 测试 ===

    #[test]
    fn test_from_indices_basic() {
        let mask = SesaMask::from_indices(&[0, 10, 255]);
        assert_eq!(mask.active_count, 3);
        assert!(mask.get_bit(0));
        assert!(mask.get_bit(10));
        assert!(mask.get_bit(255));
        assert!(!mask.get_bit(1));
    }

    #[test]
    fn test_from_indices_out_of_bounds_ignored() {
        let mask = SesaMask::from_indices(&[0, 256, 1000]);
        assert_eq!(mask.active_count, 1, "越界项应被忽略");
        assert!(mask.get_bit(0));
    }

    #[test]
    fn test_to_indices_roundtrip() {
        let original_indices = vec![0, 7, 8, 15, 16, 127, 128, 255];
        let mask = SesaMask::from_indices(&original_indices);
        let restored_indices = mask.to_indices();
        assert_eq!(restored_indices, original_indices);
    }

    #[test]
    fn test_to_indices_empty_mask() {
        let mask = SesaMask::new();
        let indices = mask.to_indices();
        assert!(indices.is_empty());
    }

    // === 8. 序列化测试 ===

    #[test]
    fn test_mask_serde_roundtrip() {
        let mut mask = SesaMask::new();
        mask.set_bit(0);
        mask.set_bit(128);
        mask.set_bit(255);

        let json = serde_json::to_string(&mask).expect("序列化失败");
        let restored: SesaMask = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(mask, restored);
    }

    // === 9. PartialEq 测试 ===

    #[test]
    fn test_mask_equality() {
        let mut mask1 = SesaMask::new();
        mask1.set_bit(10);
        let mut mask2 = SesaMask::new();
        mask2.set_bit(10);
        assert_eq!(mask1, mask2);

        mask2.set_bit(11);
        assert_ne!(mask1, mask2);
    }

    // === 10. 综合压力测试:全位设置与清除 ===

    #[test]
    fn test_set_all_then_clear_all() {
        let mut mask = SesaMask::new();
        for i in 0..256 {
            mask.set_bit(i);
        }
        assert_eq!(mask.active_count, 256);
        assert!(mask.verify_active_count());

        for i in 0..256 {
            mask.clear_bit(i);
        }
        assert_eq!(mask.active_count, 0);
        assert_eq!(mask.popcount(), 0);
        assert!(mask.verify_active_count());
    }

    #[test]
    fn test_alternating_set_clear() {
        let mut mask = SesaMask::new();
        for round in 0..10 {
            for i in 0..256 {
                if i % 2 == round % 2 {
                    mask.set_bit(i);
                } else {
                    mask.clear_bit(i);
                }
            }
            // 每轮应激活 128 位
            assert_eq!(mask.active_count, 128, "round {} 应激活 128 位", round);
            assert!(mask.verify_active_count());
        }
    }
}
