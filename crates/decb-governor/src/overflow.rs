//! DECB 溢出检测与降级 — 后台监控预算消耗,触发自动降级
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:DECB(Dual-tier Cognitive Budget)
//!
//! # 设计决策(WHY)
//! - `OverflowDetector` 为无状态结构体(仅持有配置快照),支持 `Clone`:
//!   后台监控任务需要独立副本,避免与主线程争抢锁
//! - 三级溢出阈值(50%/80%/100%):渐进式降级,避免一次性跌落到 Degraded
//! - `trigger_degradation` 为纯函数:HighTier → LowTier → Degraded,
//!   Degraded 已最低,保持不变(幂等)

use tracing::{info, warn};

use crate::config::DecbConfig;
use crate::types::BudgetTier;

/// 溢出检测阈值:50% 警告,80% 降级到 LowTier,100% 降级到 Degraded
const OVERFLOW_WARN_RATIO: f64 = 0.5;
/// 80% 阈值:触发降级到 LowTier
const OVERFLOW_DEGRADE_RATIO: f64 = 0.8;
/// 100% 阈值:触发降级到 Degraded
const OVERFLOW_CRITICAL_RATIO: f64 = 1.0;

/// 溢出检测器 — 检测预算消耗是否溢出,建议降级档位
///
/// WHY 无状态:检测器仅持有配置快照(total_budget_limit),不持有可变状态。
/// 这使得检测器可以安全 Clone 到后台任务中,无需额外同步开销。
#[derive(Debug, Clone)]
pub struct OverflowDetector {
    /// 总预算上限(从配置快照,用于阈值计算)
    total_budget_limit: f64,
}

impl OverflowDetector {
    /// 创建溢出检测器,从配置快照总预算上限
    ///
    /// WHY 持有快照而非 &DecbConfig:后台监控任务需要 'static 生命周期,
    /// 不能持有引用。快照在构造时一次性读取,保证检测器自包含。
    pub fn new(config: &DecbConfig) -> Self {
        Self {
            total_budget_limit: config.total_budget_limit,
        }
    }

    /// 检测溢出,返回建议降级档位
    ///
    /// # 返回值
    /// - `Some(BudgetTier::Degraded)`:消耗 >= 100% 总预算,降级到 Degraded
    /// - `Some(BudgetTier::LowTier)`:消耗 >= 80% 总预算,降级到 LowTier
    /// - `None`:消耗 >= 50% 总预算(警告)或 < 50%(无溢出)
    ///
    /// WHY 50% 不返回降级建议:50% 阈值仅警告,不触发降级,
    /// 避免过早降级影响正常 Quest 执行。调用方根据 None 判断是否需要降级。
    pub fn check_overflow(&self, current_consumption: f64) -> Option<BudgetTier> {
        let ratio = if self.total_budget_limit > 0.0 {
            current_consumption / self.total_budget_limit
        } else {
            // WHY 预算为 0 时视为已溢出:防止配置错误导致无限消耗
            OVERFLOW_CRITICAL_RATIO
        };

        if ratio >= OVERFLOW_CRITICAL_RATIO {
            warn!(
                consumption = current_consumption,
                limit = self.total_budget_limit,
                ratio = ratio,
                "Budget overflow detected: critical (>=100%), suggest Degraded"
            );
            Some(BudgetTier::Degraded)
        } else if ratio >= OVERFLOW_DEGRADE_RATIO {
            warn!(
                consumption = current_consumption,
                limit = self.total_budget_limit,
                ratio = ratio,
                "Budget overflow detected: high (>=80%), suggest LowTier"
            );
            Some(BudgetTier::LowTier)
        } else if ratio >= OVERFLOW_WARN_RATIO {
            warn!(
                consumption = current_consumption,
                limit = self.total_budget_limit,
                ratio = ratio,
                "Budget usage warning (>=50%), no degradation suggested"
            );
            None
        } else {
            None
        }
    }

    /// 触发降级:返回当前档位降级后的目标档位
    ///
    /// # 降级链路
    /// - `HighTier` → `LowTier`
    /// - `LowTier` → `Degraded`
    /// - `Degraded` → `Degraded`(已最低,幂等)
    ///
    /// WHY 纯函数:降级映射是无副作用的纯函数,便于测试与组合。
    /// 实际档位切换由 `DecbGovernor::switch_tier` 执行(含滞后机制)。
    pub fn trigger_degradation(&self, current_tier: BudgetTier) -> BudgetTier {
        let degraded = match current_tier {
            BudgetTier::HighTier => BudgetTier::LowTier,
            BudgetTier::LowTier => BudgetTier::Degraded,
            BudgetTier::Degraded => {
                // 已最低档,无法继续降级
                info!(tier = %current_tier, "Already at Degraded, cannot degrade further");
                BudgetTier::Degraded
            }
        };
        if degraded != current_tier {
            info!(
                from = %current_tier,
                to = %degraded,
                "Degradation triggered"
            );
        }
        degraded
    }

    /// 返回总预算上限
    pub fn total_budget_limit(&self) -> f64 {
        self.total_budget_limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_detector(limit: f64) -> OverflowDetector {
        OverflowDetector::new(&DecbConfig {
            total_budget_limit: limit,
            ..Default::default()
        })
    }

    // ============================================================
    // check_overflow 测试
    // ============================================================

    #[test]
    fn test_check_overflow_no_overflow() {
        let detector = make_detector(1_000_000.0);
        // 消耗 10% → 无溢出
        assert!(detector.check_overflow(100_000.0).is_none());
    }

    #[test]
    fn test_check_overflow_warn_threshold() {
        let detector = make_detector(1_000_000.0);
        // 消耗 50% → 警告,无降级建议
        assert!(detector.check_overflow(500_000.0).is_none());
    }

    #[test]
    fn test_check_overflow_degrade_threshold() {
        let detector = make_detector(1_000_000.0);
        // 消耗 80% → 降级到 LowTier
        let result = detector.check_overflow(800_000.0);
        assert_eq!(result, Some(BudgetTier::LowTier));
    }

    #[test]
    fn test_check_overflow_critical_threshold() {
        let detector = make_detector(1_000_000.0);
        // 消耗 100% → 降级到 Degraded
        let result = detector.check_overflow(1_000_000.0);
        assert_eq!(result, Some(BudgetTier::Degraded));
    }

    #[test]
    fn test_check_overflow_above_critical() {
        let detector = make_detector(1_000_000.0);
        // 消耗 150% → 降级到 Degraded
        let result = detector.check_overflow(1_500_000.0);
        assert_eq!(result, Some(BudgetTier::Degraded));
    }

    #[test]
    fn test_check_overflow_zero_budget() {
        // WHY 零预算视为已溢出:防止配置错误导致无限消耗
        let detector = make_detector(0.0);
        let result = detector.check_overflow(1.0);
        assert_eq!(result, Some(BudgetTier::Degraded));
    }

    #[test]
    fn test_check_overflow_just_below_degrade() {
        let detector = make_detector(1_000_000.0);
        // 消耗 79.99% → 警告,无降级
        assert!(detector.check_overflow(799_900.0).is_none());
    }

    #[test]
    fn test_check_overflow_just_below_critical() {
        let detector = make_detector(1_000_000.0);
        // 消耗 99.99% → 降级到 LowTier(未达 100%)
        let result = detector.check_overflow(999_900.0);
        assert_eq!(result, Some(BudgetTier::LowTier));
    }

    // ============================================================
    // trigger_degradation 测试
    // ============================================================

    #[test]
    fn test_trigger_degradation_from_high() {
        let detector = make_detector(1_000_000.0);
        assert_eq!(
            detector.trigger_degradation(BudgetTier::HighTier),
            BudgetTier::LowTier
        );
    }

    #[test]
    fn test_trigger_degradation_from_low() {
        let detector = make_detector(1_000_000.0);
        assert_eq!(
            detector.trigger_degradation(BudgetTier::LowTier),
            BudgetTier::Degraded
        );
    }

    #[test]
    fn test_trigger_degradation_from_degraded() {
        let detector = make_detector(1_000_000.0);
        // Degraded 已最低,保持不变(幂等)
        assert_eq!(
            detector.trigger_degradation(BudgetTier::Degraded),
            BudgetTier::Degraded
        );
    }

    #[test]
    fn test_trigger_degradation_chain() {
        let detector = make_detector(1_000_000.0);
        // 完整降级链路:HighTier → LowTier → Degraded
        let tier1 = detector.trigger_degradation(BudgetTier::HighTier);
        assert_eq!(tier1, BudgetTier::LowTier);
        let tier2 = detector.trigger_degradation(tier1);
        assert_eq!(tier2, BudgetTier::Degraded);
        let tier3 = detector.trigger_degradation(tier2);
        assert_eq!(tier3, BudgetTier::Degraded);
    }

    // ============================================================
    // 访问器测试
    // ============================================================

    #[test]
    fn test_total_budget_limit_accessor() {
        let detector = make_detector(500_000.0);
        assert!((detector.total_budget_limit() - 500_000.0).abs() < 1e-6);
    }

    // ============================================================
    // Clone 测试
    // ============================================================

    #[test]
    fn test_clone_independent() {
        let detector1 = make_detector(1_000_000.0);
        let detector2 = detector1.clone();
        // Clone 后两个检测器行为一致
        assert_eq!(
            detector1.check_overflow(800_000.0),
            detector2.check_overflow(800_000.0)
        );
    }
}
