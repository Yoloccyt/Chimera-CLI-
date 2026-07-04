//! 窗口选择器 — 按复杂度自动选择窗口层级
//!
//! 对应架构层:L2 Memory
//! 对应创新点:HCW §四级窗口自动选择
//!
//! # 核心职责
//! - 按 `complexity` 阈值(0.25/0.5/0.75)选择窗口层级
//! - 决策为纯函数,O(1) 复杂度,耗时 < 1ms(性能基准)
//!
//! # 复杂度档位映射
//! - `complexity < 0.25` → L0(4K Token,快速响应,简单任务)
//! - `0.25 ≤ complexity < 0.5` → L1(32K Token,常规任务)
//! - `0.5 ≤ complexity < 0.75` → L2(128K Token,复杂任务)
//! - `complexity ≥ 0.75` → L3(1M Token 等效,超复杂任务)
//!
//! # 设计决策(WHY)
//! - **纯函数无状态**:选择器无内部状态,线程安全,可并发调用
//! - **阈值与 OSA ComplexityBand 对齐**:0.25/0.5/0.75 与 OSA 的
//!   Simple/Regular/Complex/UltraComplex 四档分级一致,确保 HCW 与 OSA 协同

use crate::types::WindowTier;

/// 窗口选择器 — 按复杂度自动选择窗口层级
///
/// 纯函数式选择器,无内部状态,所有方法为关联函数。
///
/// # 性能基准
/// 决策耗时 < 1ms(测试断言)。实际为 O(1) 比较,耗时通常 < 1μs。
///
/// # 示例
/// ```
/// use hcw_window::{WindowSelector, WindowTier};
///
/// assert_eq!(WindowSelector::select(0.1), WindowTier::L0);
/// assert_eq!(WindowSelector::select(0.3), WindowTier::L1);
/// assert_eq!(WindowSelector::select(0.6), WindowTier::L2);
/// assert_eq!(WindowSelector::select(0.8), WindowTier::L3);
/// ```
pub struct WindowSelector;

impl WindowSelector {
    /// 按复杂度选择窗口层级
    ///
    /// 阈值映射(与 OSA ComplexityBand 对齐):
    /// - `complexity < 0.25` → L0(4K,快速响应)
    /// - `0.25 ≤ complexity < 0.5` → L1(32K,常规)
    /// - `0.5 ≤ complexity < 0.75` → L2(128K,复杂)
    /// - `complexity ≥ 0.75` → L3(1M 等效,超复杂)
    ///
    /// WHY:阈值 0.25/0.5/0.75 与 OSA `ComplexityBand::from_complexity` 一致,
    /// 确保 HCW 窗口选择与 OSA 稀疏化策略协同(同一复杂度产生匹配的窗口与掩码)
    ///
    /// # 边界处理
    /// - `complexity < 0.0`:归为 L0(视为最简单)
    /// - `complexity > 1.0`:归为 L3(视为最复杂)
    /// - NaN:归为 L0(NaN 视为最简单,避免 NaN 污染上游)
    pub fn select(complexity: f32) -> WindowTier {
        // NaN 处理:NaN < 0.25 为 false,但 NaN >= 0.75 也为 false,
        // 因此 NaN 会落入最后的 else 分支返回 L3,不符合预期。
        // 显式处理 NaN,归为 L0(最简单,避免不必要的资源消耗)
        if complexity.is_nan() {
            return WindowTier::L0;
        }
        if complexity < 0.25 {
            WindowTier::L0
        } else if complexity < 0.5 {
            WindowTier::L1
        } else if complexity < 0.75 {
            WindowTier::L2
        } else {
            WindowTier::L3
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_l0_simple() {
        assert_eq!(WindowSelector::select(0.0), WindowTier::L0);
        assert_eq!(WindowSelector::select(0.1), WindowTier::L0);
        assert_eq!(WindowSelector::select(0.24), WindowTier::L0);
    }

    #[test]
    fn test_select_l1_regular() {
        assert_eq!(WindowSelector::select(0.25), WindowTier::L1);
        assert_eq!(WindowSelector::select(0.3), WindowTier::L1);
        assert_eq!(WindowSelector::select(0.49), WindowTier::L1);
    }

    #[test]
    fn test_select_l2_complex() {
        assert_eq!(WindowSelector::select(0.5), WindowTier::L2);
        assert_eq!(WindowSelector::select(0.6), WindowTier::L2);
        assert_eq!(WindowSelector::select(0.74), WindowTier::L2);
    }

    #[test]
    fn test_select_l3_ultra_complex() {
        assert_eq!(WindowSelector::select(0.75), WindowTier::L3);
        assert_eq!(WindowSelector::select(0.9), WindowTier::L3);
        assert_eq!(WindowSelector::select(1.0), WindowTier::L3);
    }

    #[test]
    fn test_select_boundary_thresholds() {
        // 边界值:0.25/0.5/0.75 归为更高层级(左闭右开)
        assert_eq!(WindowSelector::select(0.25), WindowTier::L1);
        assert_eq!(WindowSelector::select(0.5), WindowTier::L2);
        assert_eq!(WindowSelector::select(0.75), WindowTier::L3);
    }

    #[test]
    fn test_select_out_of_range() {
        // 超出 [0, 1] 范围:负值归 L0,超过 1 归 L3
        assert_eq!(WindowSelector::select(-0.1), WindowTier::L0);
        assert_eq!(WindowSelector::select(1.5), WindowTier::L3);
    }

    #[test]
    fn test_select_nan_returns_l0() {
        // NaN 归为 L0(最简单,避免不必要的资源消耗)
        assert_eq!(WindowSelector::select(f32::NAN), WindowTier::L0);
    }

    // 注:test_select_performance_under_1ms 已删除(与 tests/selector.rs 中的
    // test_performance_under_1ms 重复,仅保留 tests/ 版本,更接近真实使用场景)
}
