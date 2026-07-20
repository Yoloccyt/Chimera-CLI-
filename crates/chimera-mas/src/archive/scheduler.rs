//! 归档调度器(事件驱动)— Task 17 §17.3 触发机制降级适配
//!
//! 架构层归属: L9 Quest(chimera-mas/archive 子模块)
//! 核心职责: 提供 `ArchiveScheduler::trigger(level, condition)` 事件驱动入口,
//! 校验 INV-8 单调性并生成 `ArchiveOperation` 计划
//!
//! ## 设计决策(WHY)
//!
//! - **不新建调度 crate**(§17.3 / ADR-026 决策 5):
//!   v5.0.0 CronScheduler 降级为"事件驱动 + efficiency-monitor 触发",
//!   本模块仅提供 `trigger()` 纯函数入口,实际调度由 efficiency-monitor
//!   订阅 `NexusEvent` 后调用本函数
//! - **`trigger` 为纯函数**: 输入 `(level, condition)`,输出 `ArchiveOperation`,
//!   不持有状态,不发布事件(调用方负责发布 Critical 事件,§6.2 红线)
//! - **INV-8 校验**: `trigger` 内部调用 `InvariantChecker::check_inv8_archive_monotonicity`,
//!   拒绝任何非 Hot→Warm→Cold→Ice 单向降级的归档操作
//! - **cron 表达式常量**: 三级 cron 表达式抽取为常量(SubTask 17.9 REFACTOR),
//!   供 efficiency-monitor 决定何时调用 `trigger`
//!
//! ## 红线对齐
//!
//! - §4.1: 库层 thiserror,无 unwrap/expect
//! - §6.1: 单函数 ≤ 200 行
//! - §6.2: rusqlite 必须 spawn_blocking(本模块无 rusqlite 直接调用)
//! - §17.3: 不新建调度 crate
//! - INV-8(§17.5): trigger 内部校验单调性

use crate::archive::tier::{ArchiveOperation, ArchiveScheduleLevel, ArchiveTriggerCondition};
use crate::error::Result;
use crate::invariants::InvariantChecker;

// ============================================================
// 常量(SubTask 17.9 REFACTOR — 抽取 τ 时间常数)
// ============================================================

/// 1 月级衰减时间常数 τ(秒)— §17.2 默认 86400s(24h,与 CMT 默认一致)
///
/// 语义:1mo 级归档采用 CMT 默认 τ=24h,priority < 0.1 时降级。
/// 公式:`priority = access_count × exp(-Δt / τ)`
///
/// WHY 抽取为常量:防止 τ 被意外修改导致降级阈值漂移,
/// `should_demote_metadata` 调用时与本常量保持一致。
pub const TAU_1MO_SECONDS: f64 = 86400.0;

/// 3 月级衰减时间常数 τ(秒)— §17.2 "衰减 τ=24h"
///
/// 语义:3mo 级归档衰减 τ=24h(与 1mo 相同),priority < 0.1 时降级。
/// 注:3mo 级主要靠 cron 触发(季度归档),τ 仅用于 Cold 层内的衰减判定。
pub const TAU_3MO_SECONDS: f64 = 86400.0;

/// 6 月级衰减时间常数 τ(秒)— §17.5 "KeepForever,无衰减"
///
/// 语义:6mo 级归档 KeepForever,τ=+∞ 表示无衰减,
/// `should_demote_metadata` 永远返回 false,确保关键决策不压缩、不降级。
///
/// WHY 用 `f64::INFINITY` 而非大数:数学上正确表示"无衰减",
/// `compute_priority` 中 `exp(-Δt / ∞) = exp(0) = 1.0`,priority = access_count,
/// 只要 access_count ≥ 1,priority ≥ 1.0 > 0.1,永不降级。
pub const TAU_6MO_SECONDS: f64 = f64::INFINITY;

// ============================================================
// ArchiveScheduler — 归档调度器(事件驱动)
// ============================================================

/// 归档调度器 — 事件驱动的归档触发入口(§17.3)
///
/// 设计为关联函数(非 `&self` 方法),因为:
/// 1. 调度无状态,无需实例化(不新建调度 crate,§17.3)
/// 2. 关联函数便于 efficiency-monitor 直接调用:
///    `ArchiveScheduler::trigger(level, condition)`
/// 3. 提供 `pub struct` 命名空间,使 API 自描述
///
/// ## 使用示例
///
/// ```
/// use chimera_mas::archive::{
///     ArchiveScheduler, ArchiveScheduleLevel, ArchiveTriggerCondition,
/// };
/// use chimera_mas::invariants::ArchiveTier;
///
/// let op = ArchiveScheduler::trigger(
///     ArchiveScheduleLevel::Month1,
///     ArchiveTriggerCondition::Cron,
/// ).unwrap();
/// assert_eq!(op.target_tier, ArchiveTier::Warm);
/// ```
pub struct ArchiveScheduler;

impl ArchiveScheduler {
    /// 触发归档操作(事件驱动入口,§17.3)
    ///
    /// 流程:
    /// 1. 从 `level` 获取 `source_tier` / `target_tier` / `compression_strategy`
    /// 2. 校验 INV-8 单调性(`source_tier.level() < target_tier.level()`)
    /// 3. 构造 `ArchiveOperation` 返回
    ///
    /// ## 参数
    ///
    /// - `level`:归档层级(1mo / 3mo / 6mo)
    /// - `condition`:触发条件(Cron 或 CapacityExceeded)
    ///
    /// ## 返回
    ///
    /// - `Ok(ArchiveOperation)`:归档操作计划,调用方负责执行
    /// - `Err(MasError::ArchiveMonotonicityViolated)`:INV-8 单调性违反
    ///
    /// ## 红线对齐
    ///
    /// - §4.1: 无 unwrap/expect,用 `?` 传播错误
    /// - §6.1: 单函数 ≤ 200 行(本函数 < 30 行)
    /// - §17.3: 不新建调度 crate,事件驱动
    /// - §17.5: INV-8 单调性校验
    pub fn trigger(
        level: ArchiveScheduleLevel,
        condition: ArchiveTriggerCondition,
    ) -> Result<ArchiveOperation> {
        let source_tier = level.source_tier();
        let target_tier = level.target_tier();

        // INV-8 单调性校验:source_tier.level() < target_tier.level()
        // WHY 在 trigger 内部校验:确保所有归档操作都经过 INV-8 守护,
        // 防止调用方绕过不变量检查
        InvariantChecker::check_inv8_archive_monotonicity(source_tier, target_tier)?;

        let compression = level.compression_strategy();
        Ok(ArchiveOperation {
            level,
            condition,
            source_tier,
            target_tier,
            compression,
        })
    }
}

// ============================================================
// 单元测试(模块内,与集成测试 tests/archive_test.rs 互补)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invariants::ArchiveTier;

    /// τ 常量稳定性:防止被意外修改
    #[test]
    fn tau_constants_are_stable() {
        assert_eq!(TAU_1MO_SECONDS, 86400.0);
        assert_eq!(TAU_3MO_SECONDS, 86400.0);
        assert!(TAU_6MO_SECONDS.is_infinite() && TAU_6MO_SECONDS > 0.0);
    }

    /// trigger 三级归档均成功(INV-8 单调性满足)
    #[test]
    fn trigger_all_levels_succeed() {
        for level in [
            ArchiveScheduleLevel::Month1,
            ArchiveScheduleLevel::Month3,
            ArchiveScheduleLevel::Month6,
        ] {
            let result = ArchiveScheduler::trigger(level, ArchiveTriggerCondition::Cron);
            assert!(result.is_ok(), "{level:?} 触发应成功");
        }
    }

    /// trigger 容量触发:保留 condition 字段
    #[test]
    fn trigger_capacity_condition_preserved() {
        let op = ArchiveScheduler::trigger(
            ArchiveScheduleLevel::Month1,
            ArchiveTriggerCondition::CapacityExceeded {
                entry_count: 15000,
                threshold: 10000,
            },
        )
        .expect("容量触发应成功");
        match op.condition {
            ArchiveTriggerCondition::CapacityExceeded {
                entry_count,
                threshold,
            } => {
                assert_eq!(entry_count, 15000);
                assert_eq!(threshold, 10000);
            }
            other => panic!("condition 应保留,实际: {other:?}"),
        }
    }

    /// trigger 生成的 ArchiveOperation 字段完整性
    #[test]
    fn trigger_operation_fields_complete() {
        let op =
            ArchiveScheduler::trigger(ArchiveScheduleLevel::Month3, ArchiveTriggerCondition::Cron)
                .expect("3mo 触发应成功");
        assert_eq!(op.source_tier, ArchiveTier::Warm);
        assert_eq!(op.target_tier, ArchiveTier::Cold);
    }
}
