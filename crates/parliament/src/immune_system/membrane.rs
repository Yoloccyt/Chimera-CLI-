//! MembraneController — 膜厚控制器（ADR-046 决策 7）
//!
//! 对应架构层:L8 Parliament
//! 对应 ADR:ADR-046 决策 7（级联联动机制 — 膜自动增厚接口）
//!
//! # 核心职责
//! - 维护膜厚度 [0, 7]（INV-11 不可变量）
//! - 提供 `set_thickness()` 接口（不可进化面,决策 9）
//! - 提供渗透规则查询（可进化面,决策 9）
//!
//! # 膜厚度档位映射（可进化面,决策 9）
//! | thickness | 渗透规则 |
//! |-----------|----------|
//! | 0-1 (Low) | 全部事件允许穿膜 |
//! | 2-3 (Medium) | Normal 级事件本地消化 |
//! | 4-5 (High) | 仅 Critical 事件穿膜 |
//! | 6-7 (Critical) | 仅 SkepticVeto/RedTeamAudit/BudgetExceeded 三类 Critical 事件穿膜 |

use std::sync::atomic::{AtomicU8, Ordering};

// ============================================================
// 常量 — 膜厚度档位（可进化面,决策 9）
// ============================================================

/// 膜厚度最大值（INV-11：membrane_thickness ∈ [0, 7]）
pub const MAX_MEMBRANE_THICKNESS: u8 = 7;

/// 膜厚度档位阈值（可进化面）
const THICKNESS_LOW_MAX: u8 = 1; // 0-1 = Low
const THICKNESS_MEDIUM_MAX: u8 = 3; // 2-3 = Medium
const THICKNESS_HIGH_MAX: u8 = 5; // 4-5 = High
                                  // 6-7 = Critical

// ============================================================
// MembraneThickness — 档位枚举
// ============================================================

/// 膜厚度档位（§6.3 四档反向调节）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MembraneTier {
    /// 低（0-1）：全部事件允许穿膜
    Low,
    /// 中（2-3）：Normal 级事件本地消化
    Medium,
    /// 高（4-5）：仅 Critical 事件穿膜
    High,
    /// 关键（6-7）：仅 SkepticVeto/RedTeamAudit/BudgetExceeded 穿膜
    Critical,
}

impl MembraneTier {
    /// 根据膜厚度推断档位
    pub fn from_thickness(thickness: u8) -> Self {
        match thickness {
            0..=THICKNESS_LOW_MAX => Self::Low,
            2..=THICKNESS_MEDIUM_MAX => Self::Medium,
            4..=THICKNESS_HIGH_MAX => Self::High,
            _ => Self::Critical,
        }
    }

    /// 返回字符串标识
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

// ============================================================
// MembraneController — 膜厚控制器
// ============================================================

/// 膜厚控制器（ADR-046 决策 7）
///
/// # 设计
/// - `AtomicU8` 无锁读取（对齐 stability.rs `CircuitBreaker` 模式）
/// - `set_thickness()` 接口为不可进化面（决策 9）
/// - 档位映射表为可进化面（决策 9）
///
/// # 不变量（INV-11）
/// `membrane_thickness ∈ [0, 7]`
#[derive(Debug)]
pub struct MembraneController {
    thickness: AtomicU8,
}

impl Default for MembraneController {
    fn default() -> Self {
        Self::new()
    }
}

impl MembraneController {
    /// 创建膜厚控制器,初始厚度为 0（Low）
    pub fn new() -> Self {
        Self {
            thickness: AtomicU8::new(0),
        }
    }

    /// 设置膜厚度（不可进化面,决策 9）
    ///
    /// # 参数
    /// - `thickness`: 目标厚度,会 clamp 到 [0, 7]（INV-11 守护）
    pub fn set_thickness(&self, thickness: u8) {
        let clamped = thickness.min(MAX_MEMBRANE_THICKNESS);
        self.thickness.store(clamped, Ordering::Release);
    }

    /// 返回当前膜厚度
    pub fn thickness(&self) -> u8 {
        self.thickness.load(Ordering::Acquire)
    }

    /// 返回当前膜厚度档位
    pub fn tier(&self) -> MembraneTier {
        MembraneTier::from_thickness(self.thickness())
    }

    /// 增厚膜（+1,clamp 到 7）
    pub fn thicken(&self) {
        let current = self.thickness();
        self.set_thickness(current.saturating_add(1));
    }

    /// 变薄膜（-1,clamp 到 0）
    pub fn thin(&self) {
        let current = self.thickness();
        self.set_thickness(current.saturating_sub(1));
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_membrane_controller_new_defaults_to_zero() {
        let m = MembraneController::new();
        assert_eq!(m.thickness(), 0);
        assert_eq!(m.tier(), MembraneTier::Low);
    }

    #[test]
    fn test_membrane_controller_set_thickness_clamps_to_max() {
        let m = MembraneController::new();
        m.set_thickness(100);
        assert_eq!(m.thickness(), MAX_MEMBRANE_THICKNESS);
        assert_eq!(m.tier(), MembraneTier::Critical);
    }

    #[test]
    fn test_membrane_controller_set_thickness_zero() {
        let m = MembraneController::new();
        m.set_thickness(0);
        assert_eq!(m.thickness(), 0);
        assert_eq!(m.tier(), MembraneTier::Low);
    }

    #[test]
    fn test_membrane_controller_thicken_clamps_to_max() {
        let m = MembraneController::new();
        m.set_thickness(6);
        m.thicken();
        assert_eq!(m.thickness(), 7);
        m.thicken(); // 已是 7,不应溢出
        assert_eq!(m.thickness(), 7);
    }

    #[test]
    fn test_membrane_controller_thin_clamps_to_zero() {
        let m = MembraneController::new();
        m.set_thickness(1);
        m.thin();
        assert_eq!(m.thickness(), 0);
        m.thin(); // 已是 0,不应下溢
        assert_eq!(m.thickness(), 0);
    }

    #[test]
    fn test_membrane_tier_boundaries() {
        assert_eq!(MembraneTier::from_thickness(0), MembraneTier::Low);
        assert_eq!(MembraneTier::from_thickness(1), MembraneTier::Low);
        assert_eq!(MembraneTier::from_thickness(2), MembraneTier::Medium);
        assert_eq!(MembraneTier::from_thickness(3), MembraneTier::Medium);
        assert_eq!(MembraneTier::from_thickness(4), MembraneTier::High);
        assert_eq!(MembraneTier::from_thickness(5), MembraneTier::High);
        assert_eq!(MembraneTier::from_thickness(6), MembraneTier::Critical);
        assert_eq!(MembraneTier::from_thickness(7), MembraneTier::Critical);
    }

    #[test]
    fn test_membrane_tier_as_str() {
        assert_eq!(MembraneTier::Low.as_str(), "low");
        assert_eq!(MembraneTier::Medium.as_str(), "medium");
        assert_eq!(MembraneTier::High.as_str(), "high");
        assert_eq!(MembraneTier::Critical.as_str(), "critical");
    }
}
